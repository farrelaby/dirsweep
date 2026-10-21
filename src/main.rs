use std::io;
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use clap::{Parser, Subcommand};
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use rayon::prelude::*;

use dirsweep::app::{AppPhase, AppState, DeletePreference, TreeEntry};

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
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
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
            execute!(stdout, EnterAlternateScreen)?;
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
    terminal.draw(|f| dirsweep::ui::render(state, f))?;
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

        terminal.draw(|f| dirsweep::ui::render(state, f))?;

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
        if let Event::Key(KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            ..
        }) = event
        {
            if code == KeyCode::Char('c') && modifiers.contains(KeyModifiers::CONTROL) {
                state.phase = AppPhase::Quit;
                break;
            }
            match state.phase {
                AppPhase::Browsing => handle_browsing_key(code, state)?,
                AppPhase::ConfirmDelete => handle_confirm_key(code, state)?,
                AppPhase::ConfirmQuit => {
                    handle_confirm_quit_key(code, state)?;
                    if state.phase == AppPhase::Quit {
                        break;
                    }
                }
                AppPhase::OrderDialog => handle_order_key(code, state)?,
                _ => {}
            }
        }
    }

    Ok(())
}

fn handle_browsing_key(code: KeyCode, state: &mut AppState) -> io::Result<()> {
    match code {
        KeyCode::Char('q') | KeyCode::Esc => {
            state.phase = AppPhase::ConfirmQuit;
        }
        KeyCode::Up | KeyCode::Char('k') => {
            state.clear_notification();
            state.move_up();
        }
        KeyCode::Down | KeyCode::Char('j') => {
            state.clear_notification();
            state.move_down();
        }
        KeyCode::Char(' ') => {
            state.clear_notification();
            state.toggle_selection();
        }
        KeyCode::Char('a') => {
            state.clear_notification();
            state.select_all();
        }
        KeyCode::Char('d') => {
            state.clear_notification();
            state.deselect_all();
        }
        KeyCode::Enter if state.selection_count() > 0 => {
            state.clear_notification();
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
            state.order_cursor = state.order_cursor.prev();
        }
        KeyCode::Down | KeyCode::Char('j') => {
            state.order_cursor = state.order_cursor.next();
        }
        KeyCode::Enter => {
            state.ordered_by = state.order_cursor;
            state.order_tree(state.order_cursor);
            state.phase = AppPhase::Browsing;
        }
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
            state.delete_preference = match state.delete_preference {
                DeletePreference::DryRun => DeletePreference::Permanent,
                DeletePreference::Trash => DeletePreference::DryRun,
                DeletePreference::Permanent => DeletePreference::Trash,
            };
        }
        KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => {
            state.delete_preference = match state.delete_preference {
                DeletePreference::DryRun => DeletePreference::Trash,
                DeletePreference::Trash => DeletePreference::Permanent,
                DeletePreference::Permanent => DeletePreference::DryRun,
            };
        }
        KeyCode::Enter => {
            state.deleting_paths = state.selected.iter().cloned().collect();
            state.deleting_index = 0;
            state.deleting_failed = Vec::new();
            state.phase = AppPhase::Deleting;
        }
        _ => {}
    }
    Ok(())
}

fn handle_confirm_quit_key(code: KeyCode, state: &mut AppState) -> io::Result<()> {
    match code {
        KeyCode::Enter => {
            state.phase = AppPhase::Quit;
        }
        KeyCode::Esc | KeyCode::Char('q') => {
            state.phase = AppPhase::Browsing;
        }
        _ => {}
    }
    Ok(())
}
