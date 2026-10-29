use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use clap::{Parser, Subcommand};
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend, layout::Position};
use rayon::prelude::*;

use dirsweep::app::{
    AppPhase, AppState, DeletePreference, DeletedTarget, MouseAction, MouseZones, OrderBy,
    TreeEntry,
};

enum SizeUpdate {
    Update {
        path: PathBuf,
        size: u64,
        last_modified: DateTime<Utc>,
        error: Option<String>,
    },
    Done,
}

/// One result from the background delete worker, streamed per directory so
/// the interface can show live progress instead of freezing on the UI
/// thread.
enum DeleteUpdate {
    /// One path finished; `ok` records whether the removal succeeded.
    Progress { path: PathBuf, ok: bool },
    /// All armed paths handled.
    Done,
}

struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture);
        let backend = CrosstermBackend::new(io::stdout());
        if let Ok(mut term) = Terminal::new(backend) {
            let _ = term.show_cursor();
        }
    }
}

#[derive(Parser, Debug)]
#[command(
    name = "dirsweep",
    about = "Find and remove bloated project directories",
    version = env!("CARGO_PKG_VERSION")
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Directory to scan (default: current directory)
    #[arg(short, long, default_value = ".")]
    dir: PathBuf,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Update dirsweep to the latest version
    Update,
    /// Uninstall dirsweep from this system
    Uninstall {
        /// Skip confirmation prompt
        #[arg(long, short)]
        force: bool,
    },
}

fn main() -> io::Result<()> {
    let args = Cli::parse();

    match args.command {
        Some(Commands::Update) => {
            if let Err(e) = dirsweep::commands::cmd_update() {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
            return Ok(());
        }
        Some(Commands::Uninstall { force }) => {
            if let Err(e) = dirsweep::commands::cmd_uninstall(force) {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
            return Ok(());
        }
        None => {
            let scan_path = if args.dir.is_absolute() {
                args.dir
            } else {
                std::env::current_dir()?.join(&args.dir)
            };
            enable_raw_mode()?;
            let mut stdout = io::stdout();
            execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
            let backend = CrosstermBackend::new(stdout);
            let mut terminal = Terminal::new(backend)?;

            let guard = TerminalGuard;
            let mut state = AppState::new(scan_path);
            let run_result = run_app(&mut terminal, &mut state);
            drop(guard);
            run_result?;
            if state.total_deleted_count > 0 {
                let noun = if state.total_deleted_count == 1 {
                    "directory"
                } else {
                    "directories"
                };
                let size_str = humansize::format_size(state.total_deleted_size, humansize::BINARY);
                println!(
                    "Sweeped {} {} ({} reclaimed)",
                    state.total_deleted_count, noun, size_str
                );
            }
        }
    }

    Ok(())
}

fn run_app(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    state: &mut AppState,
) -> io::Result<()> {
    let mut mouse_zones = MouseZones::new();
    terminal.draw(|f| mouse_zones = dirsweep::ui::render(state, f))?;
    let start = Instant::now();
    let output = dirsweep::scanner::scan(&state.scan_path);
    state.scan_duration_ms = start.elapsed().as_millis() as u64;
    state.build_tree(output);
    state.phase = AppPhase::Browsing;

    let (tx, rx) = mpsc::channel::<SizeUpdate>();
    let mut delete_rx: Option<mpsc::Receiver<DeleteUpdate>> = None;
    // A Ctrl-C pressed during a sweep defers itself; the app exits once the
    // sweep finalizes instead of killing the worker mid-delete.
    let mut pending_quit = false;

    let target_paths: Vec<PathBuf> = state
        .tree
        .iter()
        .filter_map(|e| {
            if let TreeEntry::TargetDir { path, .. } = e {
                Some(path.clone())
            } else {
                None
            }
        })
        .collect();

    // Fan the size walk across rayon rather than walking every target
    // sequentially on one thread. Each target is an independent subtree, so
    // this scales with cores. Messages still stream per target as they finish,
    // so the UI fills in progressively instead of all at once at the end.
    //
    // The sender is cloned rather than shared: std mpsc is multi-producer, so
    // each rayon task sends through its own handle and nothing needs a lock.
    let tx_updates = tx.clone();
    thread::spawn(move || {
        target_paths.into_par_iter().for_each(|path| {
            let update = match dirsweep::scanner::scan_target_size(&path) {
                Ok((size, last_modified)) => SizeUpdate::Update {
                    path,
                    size,
                    last_modified,
                    error: None,
                },
                Err(e) => SizeUpdate::Update {
                    path,
                    size: 0,
                    last_modified: DateTime::UNIX_EPOCH,
                    error: Some(e.to_string()),
                },
            };
            let _ = tx_updates.send(update);
        });
        let _ = tx.send(SizeUpdate::Done);
    });

    loop {
        loop {
            match rx.try_recv() {
                Ok(update) => match update {
                    SizeUpdate::Update {
                        path,
                        size,
                        last_modified,
                        error,
                    } => {
                        if !state.target_index.contains_key(&path) {
                            continue;
                        }
                        state.sizes_found += 1;
                        if let Some(msg) = error {
                            state.errors.push(msg);
                        }
                        state.apply_size_update(&path, size, last_modified);
                    }
                    SizeUpdate::Done => {
                        state.sizes_complete = true;
                    }
                },
                Err(mpsc::TryRecvError::Disconnected) => {
                    state.sizes_complete = true;
                    break;
                }
                Err(mpsc::TryRecvError::Empty) => break,
            }
        }

        // Live deletion: drain the worker's results without blocking, then
        // draw. The worker removes one directory at a time, so the dialog
        // animates at the event loop's cadence instead of freezing per call.
        if state.phase == AppPhase::Deleting {
            match delete_rx.as_mut() {
                Some(rx) => loop {
                    match rx.try_recv() {
                        Ok(update) => {
                            apply_delete_update(state, update);
                            if state.phase != AppPhase::Deleting {
                                // Done already consumed; drop the finished
                                // receiver so the next run spawns fresh.
                                delete_rx = None;
                                break;
                            }
                        }
                        Err(mpsc::TryRecvError::Empty) => break,
                        // The worker always sends Done before dropping its
                        // sender; a disconnect before Done is a panicked
                        // worker — finalize the sweep as interrupted rather
                        // than wedge the interface in a locked phase.
                        Err(mpsc::TryRecvError::Disconnected) => {
                            delete_rx = None;
                            recover_interrupted_sweep(state);
                            break;
                        }
                    }
                },
                None => delete_rx = Some(spawn_delete_worker(state)),
            }
        }

        // A deferred Ctrl-C: the sweep has finalized, so exit now.
        if pending_quit && state.phase != AppPhase::Deleting {
            state.phase = AppPhase::Quit;
            break;
        }

        terminal.draw(|f| mouse_zones = dirsweep::ui::render(state, f))?;

        if state.phase == AppPhase::Deleting && state.deleting_paths.is_empty() {
            state.phase = AppPhase::Browsing;
        }

        let timeout = Duration::from_millis(50);
        if !event::poll(timeout)? {
            continue;
        }

        let event = event::read()?;
        match event {
            Event::Key(KeyEvent {
                code,
                modifiers,
                kind: KeyEventKind::Press,
                ..
            }) => {
                if code == KeyCode::Char('c') && modifiers.contains(KeyModifiers::CONTROL) {
                    if handle_ctrl_c(state) {
                        state.phase = AppPhase::Quit;
                        break;
                    }
                    pending_quit = true;
                }
                // One page = half a screenful of list rows, re-read per
                // key so a resized terminal pages correctly.
                let page_rows = page_rows(terminal.size().unwrap_or_default().height);
                handle_key(code, page_rows, state)?;
            }
            Event::Mouse(mouse) => handle_mouse(mouse, state, &mouse_zones),
            _ => {}
        }

        if state.phase == AppPhase::Quit {
            break;
        }
    }

    Ok(())
}

/// Page size for `PageUp`/`PageDown`: half a screenful of list rows
/// (frame minus the header and the action bar), `less`-style — the previous
/// view stays half visible. Floors at one row so a tiny terminal still pages.
fn page_rows(terminal_height: u16) -> usize {
    ((terminal_height as usize).saturating_sub(2) / 2).max(1)
}

/// Route a key press to the handler of the active phase. Shared by
/// `run_app`'s event loop so the dispatch is testable in isolation.
fn handle_key(code: KeyCode, page_rows: usize, state: &mut AppState) -> io::Result<()> {
    match state.phase {
        AppPhase::Browsing => handle_browsing_key(code, page_rows, state)?,
        AppPhase::ConfirmDelete => handle_confirm_key(code, state)?,
        AppPhase::OrderDialog => handle_order_key(code, state)?,
        // Help is read-only: any key returns to browsing. `q` closes the
        // help first; quitting needs a second press.
        AppPhase::Help => state.phase = AppPhase::Browsing,
        // A sweep runs while the worker mutates the tree underneath, but
        // watching is not being locked: cursor movement stays live. Only
        // actions that could fight the sweep — selection changes, dialogs,
        // and quitting — stay inert until it finishes.
        AppPhase::Deleting => match code {
            KeyCode::Up | KeyCode::Char('k') => state.move_up(),
            KeyCode::Down | KeyCode::Char('j') => state.move_down(),
            KeyCode::Home => state.move_start(),
            KeyCode::End => state.move_end(),
            KeyCode::PageUp => state.page_up(page_rows),
            KeyCode::PageDown => state.page_down(page_rows),
            _ => {}
        },
        _ => {}
    }
    Ok(())
}

/// Route a Ctrl-C press. Mid-sweep it cannot exit — that would kill the
/// worker mid-delete and leave a half-swept tree — so it defers instead and
/// the sweep finishes before the app quits. Returns whether the app should
/// exit immediately.
fn handle_ctrl_c(state: &mut AppState) -> bool {
    if state.phase == AppPhase::Deleting {
        state
            .errors
            .push("Ctrl-C pressed — the sweep will finish before exit".to_string());
        false
    } else {
        true
    }
}

/// Hit-test a mouse event against the newest frame's clickable regions.
///
/// Regions are checked in reverse render order so widgets drawn last
/// (dialogs over the list) win. A left click that hits nothing dismisses a
/// result notice; clicks on empty space are otherwise ignored.
fn handle_mouse(mouse: MouseEvent, state: &mut AppState, zones: &MouseZones) {
    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            let pos = Position::new(mouse.column, mouse.row);
            for (rect, action) in zones.iter().rev() {
                if rect.contains(pos) {
                    state.clear_notification();
                    apply_mouse_action(*action, state);
                    break;
                }
            }
        }
        MouseEventKind::ScrollUp => state.move_up(),
        MouseEventKind::ScrollDown => state.move_down(),
        _ => {}
    }
}

/// Mouse actions mirror the keyboard paths exactly — every branch here has a
/// key that does the same thing, so mouse and keyboard can never disagree
/// about state (e.g. `Delete` is as inert as `Enter` when nothing is
/// selected).
fn apply_mouse_action(action: MouseAction, state: &mut AppState) {
    // A sweep runs while the worker mutates the tree underneath: the whole
    // interface is locked, mouse included, until it finishes. (The renderer
    // also offers no clickable zones — this is the second safety net.)
    if state.phase == AppPhase::Deleting {
        return;
    }
    match action {
        MouseAction::Toggle => state.toggle_selection(),
        MouseAction::MoveTo(i) => state.jump_to(i),
        MouseAction::ToggleAt(i) => {
            state.jump_to(i);
            state.toggle_selection();
        }
        MouseAction::SelectAll => state.select_all(),
        MouseAction::DeselectAll => state.deselect_all(),
        MouseAction::OpenSort => {
            state.order_cursor = state.ordered_by;
            state.phase = AppPhase::OrderDialog;
        }
        MouseAction::OpenHelp => {
            if state.phase == AppPhase::Browsing {
                state.phase = AppPhase::Help;
            }
        }
        MouseAction::Delete => {
            if state.selection_count() > 0 {
                state.phase = AppPhase::ConfirmDelete;
            }
        }
        MouseAction::Quit => state.phase = AppPhase::Quit,
        MouseAction::Confirm => match state.phase {
            AppPhase::ConfirmDelete if !state.selected.is_empty() => begin_delete(state),
            AppPhase::OrderDialog => apply_sort(state),
            _ => {}
        },
        MouseAction::Cancel => match state.phase {
            AppPhase::ConfirmDelete | AppPhase::OrderDialog => state.phase = AppPhase::Browsing,
            _ => {}
        },
        MouseAction::SetDeleteMode(preference) => {
            if state.phase == AppPhase::ConfirmDelete {
                state.delete_preference = preference;
            }
        }
        MouseAction::SortBy(order) => {
            if state.phase == AppPhase::OrderDialog {
                state.order_cursor = order;
            }
        }
        MouseAction::ApplySort => {
            if state.phase == AppPhase::OrderDialog {
                apply_sort(state);
            }
        }
        MouseAction::Dismiss => {
            match state.phase {
                AppPhase::Browsing => state.clear_notification(),
                // Any click on the help overlay closes it.
                AppPhase::Help => state.phase = AppPhase::Browsing,
                _ => {}
            }
        }
    }
}

/// Arm the deletion pipeline. Shared by the Enter key and the dialog's
/// confirm button so the two entry points can never drift. Every mode takes
/// the armed rows out of the list immediately — a preview is a dress
/// rehearsal that plays out the sweep and puts everything back afterwards.
/// The work itself starts on the next event-loop pass, which spawns the
/// worker and owns its channel.
fn begin_delete(state: &mut AppState) {
    state.deleting_paths = state.selected.iter().cloned().collect();
    state.deleting_index = 0;
    state.deleting_failed = Vec::new();
    state.deleting_reclaimed = 0;
    state.deleting_stash.clear();
    state.phase = AppPhase::Deleting;
    state.arm_sweep();
}

/// Start the background delete worker. One thread removes the armed paths
/// sequentially — the safest option on every storage type — and streams a
/// result per directory so the UI can show live progress. Returns the
/// channel the event loop drains.
fn spawn_delete_worker(state: &AppState) -> mpsc::Receiver<DeleteUpdate> {
    let preference = state.delete_preference;
    let paths = state.deleting_paths.clone();
    let (tx, rx) = mpsc::channel();

    thread::spawn(move || {
        for path in paths {
            let ok = match preference {
                DeletePreference::Trash => trash::delete(&path).is_ok(),
                DeletePreference::Permanent => std::fs::remove_dir_all(&path).is_ok(),
                DeletePreference::DryRun => {
                    thread::sleep(Duration::from_millis(500));
                    true
                }
            };
            // A failed send means the interface is already gone; stop
            // instead of deleting further from a zombie progress stream.
            if tx.send(DeleteUpdate::Progress { path, ok }).is_err() {
                break;
            }
        }
        let _ = tx.send(DeleteUpdate::Done);
    });

    rx
}

/// Apply one worker result to the shared state. Kept separate from the
/// event loop so the reclamation accounting and the completion notice are
/// testable without threads.
fn apply_delete_update(state: &mut AppState, update: DeleteUpdate) {
    match update {
        DeleteUpdate::Progress { path, ok } => {
            state.deleting_index += 1;
            if !ok {
                // The stashed row waits: failed deletes reappear checked at
                // the end of the sweep, ready for a one-Enter retry.
                state.deleting_failed.push(path);
            } else if state.delete_preference == DeletePreference::DryRun {
                // A preview changes nothing: the stash stays for the full
                // restore at the end, so the size is read without popping.
                state.deleting_reclaimed += peek_stashed_size(state, &path);
            } else {
                state.deleting_reclaimed += take_stashed_size(state, &path);
            }
        }
        DeleteUpdate::Done => finish_delete(state, false),
    }
}

/// Fail-safe for a panicked deletion worker: the channel is gone, so every
/// row still in the stash has unknown fate. Restore them all checked and
/// report an interrupted sweep instead of wedging the interface in a locked
/// phase it can never leave.
fn recover_interrupted_sweep(state: &mut AppState) {
    let unreported = state.deleting_stash.keys().cloned().collect::<Vec<_>>();
    for path in unreported {
        state.deleting_failed.push(path);
    }
    finish_delete(state, true);
}

/// Read a stashed row's size without removing it — a preview must keep the
/// whole stash for the end-of-sweep restore.
fn peek_stashed_size(state: &AppState, path: &Path) -> u64 {
    match state.deleting_stash.get(path) {
        Some(DeletedTarget {
            entry: TreeEntry::TargetDir { size, .. },
            ..
        }) => *size,
        _ => 0,
    }
}

/// Pop a stashed row and report its size for the sweep's reclaim total.
fn take_stashed_size(state: &mut AppState, path: &Path) -> u64 {
    match state.deleting_stash.remove(path) {
        Some(DeletedTarget {
            entry: TreeEntry::TargetDir { size, .. },
            ..
        }) => size,
        _ => 0,
    }
}

/// Summarize a completed sweep and return to browsing. There is no panel:
/// the notice takes over the header's stats slot until dismissed, and the
/// shrinking "you can free" total has been the running confirmation all
/// along. Stashed rows go back now — every failure in a real sweep (checked,
/// one Enter from a retry), and everything, checked, when the sweep was a
/// preview.
fn finish_delete(state: &mut AppState, interrupted: bool) {
    let failed = std::mem::take(&mut state.deleting_failed);
    let count = state.deleting_paths.len() - failed.len();
    let size_str = humansize::format_size(state.deleting_reclaimed, humansize::BINARY);

    let stashed = std::mem::take(&mut state.deleting_stash);
    for (path, target) in stashed {
        if state.delete_preference == DeletePreference::DryRun || failed.contains(&path) {
            state.reinsert_target(target);
        }
    }
    // Restored rows must land where the active sort says, not tacked onto
    // the end of their group.
    state.order_tree(state.ordered_by);
    // A preview deleted nothing: lifetime totals stay untouched.
    if state.delete_preference != DeletePreference::DryRun {
        state.accumulate_deletion(count, state.deleting_reclaimed);
    }

    let outro = if state.delete_preference == DeletePreference::DryRun {
        format!("would sweep {} \u{b7} {} reclaimable", count, size_str)
    } else {
        format!("swept {} \u{b7} {} reclaimed", count, size_str)
    };
    state.delete_result_summary = Some(if interrupted {
        // The worker died mid-sweep: unknown-fate rows are back in the list
        // (checked), and the count only covers what was confirmed deleted.
        format!("sweep interrupted · {} restored", failed.len())
    } else if failed.is_empty() {
        outro
    } else {
        format!(
            "{} \u{b7} {} failed",
            outro, // "swept 2 · 1.2 GiB reclaimed · 1 failed"
            failed.len()
        )
    });

    state.deleting_paths.clear();
    state.deleting_reclaimed = 0;
    state.phase = AppPhase::Browsing;
}

/// Commit the sort picked in the sort dialog. Shared by Enter and the
/// dialog's apply button.
fn apply_sort(state: &mut AppState) {
    state.ordered_by = state.order_cursor;
    state.order_tree(state.order_cursor);
    state.phase = AppPhase::Browsing;
}

fn handle_browsing_key(code: KeyCode, page_rows: usize, state: &mut AppState) -> io::Result<()> {
    // Any action dismisses the last sweep's result notice: the user has seen
    // it and is moving on.
    state.clear_notification();
    match code {
        // Exit is immediate: nothing destructive happens on close, selection
        // is trivially re-made, and Trash is the default. A confirm step cost
        // more than it protected.
        KeyCode::Char('?') => {
            state.phase = AppPhase::Help;
        }
        KeyCode::Char('q') | KeyCode::Esc => {
            state.phase = AppPhase::Quit;
        }
        KeyCode::Up | KeyCode::Char('k') => {
            state.move_up();
        }
        KeyCode::Down | KeyCode::Char('j') => {
            state.move_down();
        }
        KeyCode::Home => state.move_start(),
        KeyCode::End => state.move_end(),
        KeyCode::PageUp => state.page_up(page_rows),
        KeyCode::PageDown => state.page_down(page_rows),
        KeyCode::Char(' ') => {
            state.toggle_selection();
        }
        KeyCode::Char('a') => {
            state.select_all();
        }
        KeyCode::Char('d') => {
            state.deselect_all();
        }
        KeyCode::Enter if state.selection_count() > 0 => {
            state.phase = AppPhase::ConfirmDelete;
        }
        KeyCode::Char('o') => {
            state.order_cursor = state.ordered_by;
            state.phase = AppPhase::OrderDialog;
        }
        _ => {}
    }
    Ok(())
}

fn handle_order_key(code: KeyCode, state: &mut AppState) -> io::Result<()> {
    match code {
        KeyCode::Up | KeyCode::Char('k') => {
            state.order_cursor = state.order_cursor.step(-1);
        }
        KeyCode::Down | KeyCode::Char('j') => {
            state.order_cursor = state.order_cursor.step(1);
        }
        // 1–6 jump straight to an order (top to bottom, matching the labels).
        // Select only: Enter — or the Apply hint — still commits.
        KeyCode::Char(digit) if digit.is_ascii_digit() => {
            let pick = digit.to_digit(10).unwrap_or(0) as usize;
            if (1..=OrderBy::ALL.len()).contains(&pick) {
                state.order_cursor = OrderBy::ALL[pick - 1];
            }
        }
        KeyCode::Enter => apply_sort(state),
        KeyCode::Esc => {
            state.phase = AppPhase::Browsing;
        }
        _ => {}
    }
    Ok(())
}

fn handle_confirm_key(code: KeyCode, state: &mut AppState) -> io::Result<()> {
    if state.selected.is_empty() {
        state.phase = AppPhase::Browsing;
        return Ok(());
    }

    match code {
        KeyCode::Esc => {
            state.phase = AppPhase::Browsing;
        }
        KeyCode::Up | KeyCode::Char('k') => {
            state.delete_preference = state.delete_preference.step_back();
        }
        KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => {
            state.delete_preference = state.delete_preference.step_forward();
        }
        KeyCode::Enter => begin_delete(state),
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use dirsweep::app::OrderBy;
    use dirsweep::scanner::{ProjectInfo, ScanOutput, ScannedDir};

    fn scan_output() -> ScanOutput {
        let project = ProjectInfo {
            path: PathBuf::from("/test/app"),
            name: "app".to_string(),
            languages: vec!["js".to_string()],
            children: vec![PathBuf::from("/test/app/node_modules")],
        };
        ScanOutput {
            target_dirs: vec![ScannedDir {
                path: PathBuf::from("/test/app/node_modules"),
                size: 1_000,
                last_modified: Utc::now(),
                error: None,
            }],
            projects: vec![project],
            ..Default::default()
        }
    }

    fn seeded_state() -> AppState {
        let mut state = AppState::new(PathBuf::from("/test"));
        state.build_tree(scan_output());
        state.phase = AppPhase::Browsing;
        state
    }

    #[test]
    fn mouse_row_click_moves_cursor_and_toggles() {
        // Tree: [0] header, [1] node_modules.
        let mut state = seeded_state();
        assert_eq!(state.list_index, 1);

        apply_mouse_action(MouseAction::ToggleAt(1), &mut state);
        assert_eq!(state.selection_count(), 1);
        assert_eq!(state.list_index, 1);

        apply_mouse_action(MouseAction::ToggleAt(1), &mut state);
        assert_eq!(state.selection_count(), 0);
    }

    #[test]
    fn mouse_delete_is_inert_without_selection() {
        let mut state = seeded_state();
        apply_mouse_action(MouseAction::Delete, &mut state);
        assert_eq!(state.phase, AppPhase::Browsing);

        state.select_all();
        apply_mouse_action(MouseAction::Delete, &mut state);
        assert_eq!(state.phase, AppPhase::ConfirmDelete);
    }

    #[test]
    fn mouse_select_all_deselect_all_and_quit() {
        let mut state = seeded_state();
        apply_mouse_action(MouseAction::SelectAll, &mut state);
        assert_eq!(state.selection_count(), 1);
        apply_mouse_action(MouseAction::DeselectAll, &mut state);
        assert_eq!(state.selection_count(), 0);

        apply_mouse_action(MouseAction::Quit, &mut state);
        assert_eq!(state.phase, AppPhase::Quit);
    }

    #[test]
    fn confirm_dialog_nav_stays_in_mode_order() {
        let mut state = seeded_state();
        state.select_all();
        state.phase = AppPhase::ConfirmDelete;
        state.delete_preference = DeletePreference::DryRun;

        // Up at the top must not wrap onto the destructive option.
        handle_confirm_key(KeyCode::Up, &mut state).unwrap();
        assert_eq!(state.delete_preference, DeletePreference::DryRun);

        handle_confirm_key(KeyCode::Down, &mut state).unwrap();
        assert_eq!(state.delete_preference, DeletePreference::Trash);
        handle_confirm_key(KeyCode::Down, &mut state).unwrap();
        assert_eq!(state.delete_preference, DeletePreference::Permanent);
        handle_confirm_key(KeyCode::Down, &mut state).unwrap();
        assert_eq!(
            state.delete_preference,
            DeletePreference::Permanent,
            "Down at the bottom must stay put"
        );

        // Enter arms the deletion pipeline.
        handle_confirm_key(KeyCode::Enter, &mut state).unwrap();
        assert_eq!(state.phase, AppPhase::Deleting);
        assert_eq!(state.deleting_paths.len(), 1);
    }

    #[test]
    fn mouse_dialog_actions_work_only_in_their_phase() {
        let mut state = seeded_state();
        state.select_all();
        state.phase = AppPhase::ConfirmDelete;

        apply_mouse_action(
            MouseAction::SetDeleteMode(DeletePreference::Permanent),
            &mut state,
        );
        assert_eq!(state.delete_preference, DeletePreference::Permanent);

        apply_mouse_action(MouseAction::Cancel, &mut state);
        assert_eq!(state.phase, AppPhase::Browsing);

        // Selection survives a cancel; the chooser re-opens by mouse or Enter.
        apply_mouse_action(MouseAction::Delete, &mut state);
        assert_eq!(state.phase, AppPhase::ConfirmDelete);
        apply_mouse_action(MouseAction::Confirm, &mut state);
        assert_eq!(state.phase, AppPhase::Deleting);

        let mut state = seeded_state();
        state.phase = AppPhase::OrderDialog;
        apply_mouse_action(MouseAction::SortBy(OrderBy::SizeDesc), &mut state);
        assert_eq!(state.order_cursor, OrderBy::SizeDesc);
        apply_mouse_action(MouseAction::ApplySort, &mut state);
        assert_eq!(state.phase, AppPhase::Browsing);
        assert_eq!(state.ordered_by, OrderBy::SizeDesc);

        // Sort actions are inert outside the sort dialog.
        let mut state = seeded_state();
        apply_mouse_action(MouseAction::SortBy(OrderBy::SizeDesc), &mut state);
        assert_eq!(state.phase, AppPhase::Browsing);
        assert_ne!(state.ordered_by, OrderBy::SizeDesc);
    }

    #[test]
    fn quits_immediately_without_confirmation() {
        let mut state = seeded_state();
        handle_browsing_key(KeyCode::Char('q'), 24, &mut state).unwrap();
        assert_eq!(state.phase, AppPhase::Quit);

        let mut state = seeded_state();
        handle_browsing_key(KeyCode::Esc, 24, &mut state).unwrap();
        assert_eq!(state.phase, AppPhase::Quit);
    }

    #[test]
    fn help_opens_with_question_mark_and_closes_on_any_key() {
        let mut state = seeded_state();
        handle_key(KeyCode::Char('?'), 24, &mut state).unwrap();
        assert_eq!(state.phase, AppPhase::Help);

        // `q` inside the help closes it; quitting needs a second press.
        handle_key(KeyCode::Char('q'), 24, &mut state).unwrap();
        assert_eq!(state.phase, AppPhase::Browsing);

        // Mouse can open it too, and clicking the overlay closes it.
        apply_mouse_action(MouseAction::OpenHelp, &mut state);
        assert_eq!(state.phase, AppPhase::Help);
        apply_mouse_action(MouseAction::Dismiss, &mut state);
        assert_eq!(state.phase, AppPhase::Browsing);
    }

    /// Two projects, each with one dir: [0] header, [1] target, [2] header,
    /// [3] target — the layout that exposes header mistakes.
    fn two_project_scan_output() -> ScanOutput {
        let mk = |name: &str| ProjectInfo {
            path: PathBuf::from(format!("/test/{name}")),
            name: name.to_string(),
            languages: vec!["js".to_string()],
            children: vec![PathBuf::from(format!("/test/{name}/node_modules"))],
        };

        ScanOutput {
            target_dirs: vec![
                ScannedDir {
                    path: PathBuf::from("/test/app-a/node_modules"),
                    size: 1_000,
                    last_modified: Utc::now(),
                    error: None,
                },
                ScannedDir {
                    path: PathBuf::from("/test/app-b/node_modules"),
                    size: 2_000,
                    last_modified: Utc::now(),
                    error: None,
                },
            ],
            projects: vec![mk("app-a"), mk("app-b")],
            ..Default::default()
        }
    }

    #[test]
    fn starts_ends_and_pages_jump_to_targets() {
        let mut state = AppState::new(PathBuf::from("/test"));
        state.build_tree(two_project_scan_output());
        state.phase = AppPhase::Browsing;

        state.list_index = 3;
        handle_browsing_key(KeyCode::End, 40, &mut state).unwrap();
        assert_eq!(state.list_index, 3, "end has nothing further to reach");
        handle_browsing_key(KeyCode::Home, 40, &mut state).unwrap();
        assert_eq!(state.list_index, 1, "home lands on the first target");

        state.list_index = 1;
        handle_browsing_key(KeyCode::PageDown, 40, &mut state).unwrap();
        assert_eq!(state.list_index, 3, "page down reaches the last target");
        handle_browsing_key(KeyCode::PageUp, 40, &mut state).unwrap();
        assert_eq!(state.list_index, 1, "page up returns to the first target");

        // Steps clamp at the extreme targets and never strand on a header.
        state.list_index = 3;
        handle_browsing_key(KeyCode::PageDown, 1, &mut state).unwrap();
        assert_eq!(state.list_index, 3, "page down at the bottom stays put");
        assert!(matches!(
            state.current_entry(),
            Some(TreeEntry::TargetDir { .. })
        ));
    }

    #[test]
    fn arming_a_sweep_vanishes_the_rows_and_the_notice_replays_the_result() {
        let mut state = seeded_state();
        state.select_all();
        let path = PathBuf::from("/test/app/node_modules");
        begin_delete(&mut state);

        // Committed removal: whole group gone, row stashed for failures,
        // selection drained, and the total matches what's on disk now.
        assert_eq!(state.phase, AppPhase::Deleting);
        assert_eq!(state.tree.len(), 0);
        assert_eq!(state.selection_count(), 0);
        assert_eq!(state.total_reclaimable, 0);
        assert_eq!(state.deleting_stash.len(), 1);
        assert_eq!(state.deleting_paths, vec![path.clone()]);

        apply_delete_update(
            &mut state,
            DeleteUpdate::Progress {
                path: path.clone(),
                ok: true,
            },
        );
        assert_eq!(state.deleting_index, 1);
        assert_eq!(state.deleting_reclaimed, 1_000, "size came from the stash");
        assert_eq!(state.phase, AppPhase::Deleting, "still sweeping");

        apply_delete_update(&mut state, DeleteUpdate::Done);
        assert_eq!(state.phase, AppPhase::Browsing);
        assert!(state.deleting_paths.is_empty());
        assert!(state.deleting_stash.is_empty());
        assert_eq!(state.total_deleted_count, 1);
        assert_eq!(state.total_deleted_size, 1_000);
        assert_eq!(
            state.delete_result_summary.as_deref(),
            Some("swept 1 \u{b7} 1000 B reclaimed")
        );
    }

    #[test]
    fn failed_deletes_reappear_checked_when_the_sweep_ends() {
        let mut state = seeded_state();
        state.select_all();
        let path = PathBuf::from("/test/app/node_modules");
        begin_delete(&mut state);
        assert_eq!(state.tree.len(), 0, "armed rows vanish immediately");

        apply_delete_update(&mut state, DeleteUpdate::Progress { path, ok: false });
        apply_delete_update(&mut state, DeleteUpdate::Done);

        assert_eq!(state.phase, AppPhase::Browsing);
        let summary = state.delete_result_summary.as_ref().unwrap();
        assert!(
            summary.contains("swept 0") && summary.contains("1 failed"),
            "{summary}"
        );
        // The group (header + target) is rebuilt with the checkbox on, so a
        // retry is literally one Enter away.
        assert_eq!(state.tree.len(), 2);
        assert_eq!(state.selection_count(), 1);
        assert_eq!(state.total_reclaimable, 1_000);
    }

    #[test]
    fn restored_rows_land_in_the_active_sort_order() {
        // One project, three targets sized so their build order (1k, 3k, 2k)
        // differs from a size-descending order (3k, 2k, 1k).
        let mut state = AppState::new(PathBuf::from("/test"));
        state.build_tree(ScanOutput {
            target_dirs: vec![
                ScannedDir {
                    path: PathBuf::from("/test/p/node-small"),
                    size: 1_000,
                    last_modified: Utc::now(),
                    error: None,
                },
                ScannedDir {
                    path: PathBuf::from("/test/p/node-big"),
                    size: 3_000,
                    last_modified: Utc::now(),
                    error: None,
                },
                ScannedDir {
                    path: PathBuf::from("/test/p/node-mid"),
                    size: 2_000,
                    last_modified: Utc::now(),
                    error: None,
                },
            ],
            projects: vec![ProjectInfo {
                path: PathBuf::from("/test/p"),
                name: "p".to_string(),
                languages: vec!["js".to_string()],
                children: vec![
                    PathBuf::from("/test/p/node-small"),
                    PathBuf::from("/test/p/node-big"),
                    PathBuf::from("/test/p/node-mid"),
                ],
            }],
            ..Default::default()
        });
        state.order_tree(OrderBy::SizeDesc);

        state.select_all();
        begin_delete(&mut state);
        // Two failures come back; without the re-sort they could land in any
        // order at the group's end (the stash is a hash map).
        apply_delete_update(
            &mut state,
            DeleteUpdate::Progress {
                path: PathBuf::from("/test/p/node-mid"),
                ok: false,
            },
        );
        apply_delete_update(
            &mut state,
            DeleteUpdate::Progress {
                path: PathBuf::from("/test/p/node-small"),
                ok: false,
            },
        );
        apply_delete_update(
            &mut state,
            DeleteUpdate::Progress {
                path: PathBuf::from("/test/p/node-big"),
                ok: true,
            },
        );
        apply_delete_update(&mut state, DeleteUpdate::Done);

        assert_eq!(state.phase, AppPhase::Browsing);
        let sizes: Vec<u64> = state
            .tree
            .iter()
            .filter_map(|e| match e {
                TreeEntry::TargetDir { size, .. } => Some(*size),
                _ => None,
            })
            .collect();
        assert_eq!(sizes, vec![2_000, 1_000], "size-desc across restored rows");
        assert_eq!(state.ordered_by, OrderBy::SizeDesc, "sort was not changed");
        let summary = state.delete_result_summary.as_ref().unwrap();
        assert!(
            summary.contains("2 failed") && summary.contains("swept 1 · 2.93 KiB"),
            "{summary}"
        );
    }

    #[test]
    fn a_panicked_worker_finalizes_as_an_interrupted_sweep() {
        let mut state = seeded_state();
        state.select_all();
        begin_delete(&mut state);
        assert_eq!(state.tree.len(), 0);

        // Simulates the channel dying mid-sweep: nothing was reported ok, so
        // every stashed row has unknown fate and must come back.
        recover_interrupted_sweep(&mut state);

        assert_eq!(state.phase, AppPhase::Browsing, "never stays locked");
        assert!(state.deleting_paths.is_empty());
        assert!(state.deleting_failed.is_empty());
        assert!(state.deleting_stash.is_empty());
        assert_eq!(state.tree.len(), 2);
        assert_eq!(state.selection_count(), 1, "returns checked for a retry");
        assert_eq!(state.total_reclaimable, 1_000);
        assert_eq!(state.total_deleted_count, 0, "nothing confirmed deleted");
        let summary = state.delete_result_summary.as_ref().unwrap();
        assert!(
            summary.contains("interrupted") && summary.contains("1 restored"),
            "{summary}"
        );
    }

    #[test]
    fn ctrl_c_quits_now_except_mid_sweep() {
        let mut state = seeded_state();
        assert!(handle_ctrl_c(&mut state), "browsing quits immediately");

        // A sweep in flight: defer with a plain-language explanation.
        state.select_all();
        begin_delete(&mut state);
        assert!(!handle_ctrl_c(&mut state));
        assert_eq!(state.phase, AppPhase::Deleting, "no exit mid-sweep");
        assert_eq!(state.errors.len(), 1);
        assert!(
            state.errors[0].contains("Ctrl-C") && state.errors[0].contains("sweep"),
            "{}",
            state.errors[0]
        );
    }

    #[test]
    fn a_preview_rehearses_then_restores_everything() {
        let mut state = seeded_state();
        state.select_all();
        state.delete_preference = DeletePreference::DryRun;
        let path = PathBuf::from("/test/app/node_modules");
        begin_delete(&mut state);

        // A preview plays out the sweep: rows leave exactly as they would
        // in a real sweep.
        assert_eq!(state.tree.len(), 0);
        assert_eq!(state.deleting_stash.len(), 1);

        apply_delete_update(&mut state, DeleteUpdate::Progress { path, ok: true });
        assert_eq!(state.deleting_reclaimed, 1_000);
        assert_eq!(state.tree.len(), 0, "still rehearsing");

        apply_delete_update(&mut state, DeleteUpdate::Done);
        assert_eq!(
            state.delete_result_summary.as_deref(),
            Some("would sweep 1 \u{b7} 1000 B reclaimable")
        );
        // ...and the world is put back unchanged: every row returns with
        // its checkmark, nothing was deleted.
        assert_eq!(state.tree.len(), 2);
        assert_eq!(state.selection_count(), 1);
        assert_eq!(state.total_reclaimable, 1_000);
        assert!(state.deleting_stash.is_empty());
        assert_eq!(state.total_deleted_count, 0, "preview deleted nothing");
        assert_eq!(state.total_deleted_size, 0);
    }

    #[test]
    fn sweeps_allow_cursor_navigation_but_nothing_else() {
        let mut state = AppState::new(PathBuf::from("/test"));
        state.build_tree(two_project_scan_output());
        // A sweep of app-b's dir is in flight; app-a's rows remain.
        state.list_index = 3;
        state.toggle_selection();
        begin_delete(&mut state);
        assert_eq!(state.tree.len(), 2, "only the armed row left the list");
        assert_eq!(state.list_index, 1, "clamped back onto a target");

        // Cursor movement stays live while the worker works.
        handle_key(KeyCode::Down, 24, &mut state).unwrap();
        assert_eq!(state.list_index, 1, "nothing below the last target");
        handle_key(KeyCode::Up, 24, &mut state).unwrap();
        assert_eq!(state.list_index, 1, "clamped at the first target");
        handle_key(KeyCode::Home, 24, &mut state).unwrap();
        assert_eq!(state.list_index, 1);

        // Everything that could fight the sweep stays inert.
        handle_key(KeyCode::Char(' '), 24, &mut state).unwrap();
        assert_eq!(state.selection_count(), 0);
        handle_key(KeyCode::Char('o'), 24, &mut state).unwrap();
        assert_eq!(state.phase, AppPhase::Deleting);
        handle_key(KeyCode::Char('q'), 24, &mut state).unwrap();
        assert_eq!(state.phase, AppPhase::Deleting);
        handle_key(KeyCode::Enter, 24, &mut state).unwrap();
        assert_eq!(state.phase, AppPhase::Deleting);
    }

    #[test]
    fn delete_worker_reports_one_update_per_path_then_done() {
        use tempfile::TempDir;

        // Two real directories so the worker exercises true removal.
        let tmp = TempDir::new().unwrap();
        let dirs = ["dir-one", "dir-two"]
            .map(|d| tmp.path().join(d))
            .map(|p| {
                std::fs::create_dir_all(&p).unwrap();
                p
            })
            .to_vec();

        let mut state = AppState::new(PathBuf::from(tmp.path()));
        state.delete_preference = DeletePreference::Permanent;
        state.deleting_paths = dirs.clone();

        let rx = spawn_delete_worker(&state);
        let mut results = Vec::new();
        while let Ok(update) = rx.recv() {
            match update {
                DeleteUpdate::Progress { .. } => results.push(update),
                DeleteUpdate::Done => break,
            }
        }

        assert_eq!(results.len(), 2, "one result per path");
        for (update, dir) in results.into_iter().zip(&dirs) {
            match update {
                DeleteUpdate::Progress { path, ok } => {
                    assert_eq!(path, *dir);
                    assert!(ok, "removing a real empty dir succeeds");
                }
                DeleteUpdate::Done => unreachable!(),
            }
        }
        assert!(!dirs[0].exists() && !dirs[1].exists());
    }

    #[test]
    fn page_size_is_half_the_window_after_chrome() {
        // 24-row terminal minus header and bar = 22 list rows → 11 per page.
        assert_eq!(page_rows(24), 11);
        // Odd rows round down.
        assert_eq!(page_rows(25), 11);
        // Tiny terminals still page, by at least one row.
        assert_eq!(page_rows(2), 1);
        assert_eq!(page_rows(0), 1);
    }

    #[test]
    fn sort_dialog_digits_pick_their_order() {
        let mut state = seeded_state();
        state.phase = AppPhase::OrderDialog;

        handle_order_key(KeyCode::Char('3'), &mut state).unwrap();
        assert_eq!(state.order_cursor, OrderBy::DateAsc, "3 picks DateAsc");

        handle_order_key(KeyCode::Char('6'), &mut state).unwrap();
        assert_eq!(state.order_cursor, OrderBy::SizeDesc);

        // 6 is the last choice; 0 and 7 pick nothing.
        handle_order_key(KeyCode::Char('0'), &mut state).unwrap();
        assert_eq!(state.order_cursor, OrderBy::SizeDesc);
        handle_order_key(KeyCode::Char('7'), &mut state).unwrap();
        assert_eq!(state.order_cursor, OrderBy::SizeDesc);

        // Enter still commits.
        handle_order_key(KeyCode::Enter, &mut state).unwrap();
        assert_eq!(state.phase, AppPhase::Browsing);
        assert_eq!(state.ordered_by, OrderBy::SizeDesc);
    }

    #[test]
    fn any_browsing_action_dismisses_the_result_notice() {
        let mut state = seeded_state();
        state.delete_result_summary = Some("Done! 1 deleted".to_string());

        handle_browsing_key(KeyCode::Up, 24, &mut state).unwrap();
        assert_eq!(state.delete_result_summary, None);
    }
}
