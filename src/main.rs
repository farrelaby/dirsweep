use std::io;
use std::path::PathBuf;
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

use dirsweep::app::{AppPhase, AppState, DeletePreference, MouseAction, MouseZones, TreeEntry};

enum SizeUpdate {
    Update {
        path: PathBuf,
        size: u64,
        last_modified: DateTime<Utc>,
        error: Option<String>,
    },
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

        terminal.draw(|f| mouse_zones = dirsweep::ui::render(state, f))?;

        if state.phase == AppPhase::Deleting && state.deleting_paths.is_empty() {
            state.phase = AppPhase::Browsing;
            continue;
        }

        if state.phase == AppPhase::Deleting && state.deleting_index < state.deleting_paths.len() {
            let path = state.deleting_paths[state.deleting_index].clone();

            let ok = match state.delete_preference {
                DeletePreference::Trash => trash::delete(&path).is_ok(),
                DeletePreference::Permanent => std::fs::remove_dir_all(&path).is_ok(),
                DeletePreference::DryRun => {
                    std::thread::sleep(Duration::from_millis(500));
                    true
                }
            };

            state.deleting_index += 1;

            if !ok {
                state.deleting_failed.push(path);
            }

            if state.deleting_index == state.deleting_paths.len() {
                let count = state.deleting_paths.len() - state.deleting_failed.len();
                let reclaimed_size: u64 = state
                    .selected
                    .iter()
                    .filter(|p| !state.deleting_failed.contains(p))
                    .filter_map(|p| {
                        state
                            .target_index
                            .get(p)
                            .and_then(|&idx| state.tree.get(idx))
                            .and_then(|entry| match entry {
                                TreeEntry::TargetDir { size, .. } => Some(*size),
                                _ => None,
                            })
                    })
                    .sum();
                let size_str = humansize::format_size(reclaimed_size, humansize::BINARY);

                for failed in &state.deleting_failed {
                    state.selected.remove(failed);
                }

                state.accumulate_deletion(count, reclaimed_size);
                state.delete_result_summary = Some(if state.deleting_failed.is_empty() {
                    format!(
                        "Done! {} directory(ies) deleted, {} reclaimed",
                        state.deleting_paths.len(),
                        size_str
                    )
                } else {
                    format!(
                        "Done! {} deleted, {} failed, {} reclaimed",
                        count,
                        state.deleting_failed.len(),
                        size_str
                    )
                });

                state.remove_deleted_from_tree();
                state.deleting_paths.clear();
                state.deleting_failed.clear();
                state.phase = AppPhase::Browsing;
            }

            continue;
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
                    state.phase = AppPhase::Quit;
                    break;
                }
                match state.phase {
                    AppPhase::Browsing => handle_browsing_key(code, state)?,
                    AppPhase::ConfirmDelete => handle_confirm_key(code, state)?,
                    AppPhase::OrderDialog => handle_order_key(code, state)?,
                    _ => {}
                }
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
            if state.phase == AppPhase::Browsing {
                state.clear_notification();
            }
        }
    }
}

/// Arm the deletion pipeline. Shared by the Enter key and the dialog's
/// confirm button so the two entry points can never drift.
fn begin_delete(state: &mut AppState) {
    state.deleting_paths = state.selected.iter().cloned().collect();
    state.deleting_index = 0;
    state.deleting_failed = Vec::new();
    state.phase = AppPhase::Deleting;
}

/// Commit the sort picked in the sort dialog. Shared by Enter and the
/// dialog's apply button.
fn apply_sort(state: &mut AppState) {
    state.ordered_by = state.order_cursor;
    state.order_tree(state.order_cursor);
    state.phase = AppPhase::Browsing;
}

fn handle_browsing_key(code: KeyCode, state: &mut AppState) -> io::Result<()> {
    // Any action dismisses the last sweep's result notice: the user has seen
    // it and is moving on.
    state.clear_notification();
    match code {
        // Exit is immediate: nothing destructive happens on close, selection
        // is trivially re-made, and Trash is the default. A confirm step cost
        // more than it protected.
        KeyCode::Char('q') | KeyCode::Esc => {
            state.phase = AppPhase::Quit;
        }
        KeyCode::Up | KeyCode::Char('k') => {
            state.move_up();
        }
        KeyCode::Down | KeyCode::Char('j') => {
            state.move_down();
        }
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
        handle_browsing_key(KeyCode::Char('q'), &mut state).unwrap();
        assert_eq!(state.phase, AppPhase::Quit);

        let mut state = seeded_state();
        handle_browsing_key(KeyCode::Esc, &mut state).unwrap();
        assert_eq!(state.phase, AppPhase::Quit);
    }

    #[test]
    fn any_browsing_action_dismisses_the_result_notice() {
        let mut state = seeded_state();
        state.delete_result_summary = Some("Done! 1 deleted".to_string());

        handle_browsing_key(KeyCode::Up, &mut state).unwrap();
        assert_eq!(state.delete_result_summary, None);
    }
}
