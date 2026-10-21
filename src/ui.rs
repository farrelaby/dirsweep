use chrono::{DateTime, Utc};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph},
};

use crate::app::{AppPhase, AppState, DeletePreference, OrderBy, TreeEntry};
use crate::theme;

pub fn render(state: &AppState, frame: &mut Frame) {
    // No background paint: the terminal's own theme shows through.
    let areas = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .split(frame.area());

    render_header(state, frame, areas[0]);
    render_main(state, frame, areas[1]);
    render_status_bar(state, frame, areas[2]);
}

fn render_header(state: &AppState, frame: &mut Frame, area: Rect) {
    let scan_path = state.scan_path.to_string_lossy();
    let stats = if state.phase == AppPhase::Scanning {
        String::new()
    } else if state.total_reclaimable == 0 {
        String::from("  nothing to reclaim")
    } else {
        format!(
            "  {} reclaimable  |  scan {:.1}s",
            humansize::format_size(state.total_reclaimable, humansize::BINARY),
            state.scan_duration_ms as f64 / 1000.0,
        )
    };

    let header = Line::from(vec![
        Span::styled(
            format!(" dirsweep — {}", scan_path),
            Style::default()
                .fg(theme::STRUCTURE)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(stats, Style::default().fg(theme::SELECT)),
    ]);

    frame.render_widget(Paragraph::new(header), area);
}

fn render_main(state: &AppState, frame: &mut Frame, area: Rect) {
    match state.phase {
        AppPhase::Scanning => render_scanning(frame, area),
        AppPhase::Browsing => {
            if state.errors.is_empty() {
                render_browsing(state, frame, area);
            } else {
                let [tree_area, error_area] =
                    Layout::vertical([Constraint::Min(0), Constraint::Length(3)]).areas(area);
                render_browsing(state, frame, tree_area);
                render_errors(state, frame, error_area);
            }
        }
        AppPhase::ConfirmDelete => {
            render_browsing(state, frame, area);
            render_confirm_dialog(state, frame, area);
        }
        AppPhase::Deleting => {
            render_browsing(state, frame, area);
            render_deleting(state, frame, area);
        }
        AppPhase::ConfirmQuit => {
            render_browsing(state, frame, area);
            render_confirm_quit_dialog(state, frame, area);
        }
        AppPhase::OrderDialog => {
            render_browsing(state, frame, area);
            render_order_dialog(state, frame, area);
        }
        AppPhase::Quit => {}
    }
}

fn render_errors(state: &AppState, frame: &mut Frame, area: Rect) {
    let errors: Vec<Line> = state
        .errors
        .iter()
        .map(|e| {
            Line::from(Span::styled(
                format!(" \u{26A0} {}", e),
                Style::default().fg(theme::DANGER),
            ))
        })
        .collect();

    let text = Paragraph::new(Text::from(errors))
        .block(
            Block::default()
                .borders(Borders::TOP)
                .title(" Errors")
                .style(Style::default().fg(theme::DANGER)),
        )
        .scroll((0, 0));

    frame.render_widget(text, area);
}

fn render_scanning(frame: &mut Frame, area: Rect) {
    let text = Paragraph::new(Text::from(vec![
        Line::from(""),
        Line::from(Span::styled(
            " Scanning...",
            Style::default()
                .fg(theme::STRUCTURE)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            " Walking directory tree for known build artifacts",
            Style::default().fg(theme::MUTED),
        )),
    ]))
    .style(Style::default())
    .block(Block::default());

    frame.render_widget(text, area);
}

fn render_browsing(state: &AppState, frame: &mut Frame, area: Rect) {
    if state.tree.is_empty() {
        let text = Paragraph::new(" No sweepable directories found.")
            .style(Style::default().fg(theme::MUTED))
            .block(Block::default());
        frame.render_widget(text, area);
        return;
    }

    let total = state.tree.len();
    let visible = (area.height as usize).saturating_sub(1);
    let half = visible / 2;

    let scroll_offset = if state.list_index < half {
        0
    } else if state.list_index + half >= total {
        total.saturating_sub(visible)
    } else {
        state.list_index - half
    };

    let end = (scroll_offset + visible).min(total);
    let slice = &state.tree[scroll_offset..end];

    // Cached on AppState rather than recomputed here. Deriving this per frame walked every
    // target in the tree, so frame cost tracked total tree size rather than the visible
    // slice it exists to serve. The inputs change only when the tree does.
    let max_path_width = state.max_path_width();

    // Use table alignment only when terminal has enough room.
    // Overhead: prefix(3) + checkbox(3) + spaces(3) + max_size(8) + max_time(12) + margin(2)
    let use_alignment = max_path_width + 31 <= area.width as usize;

    let mut items: Vec<ListItem> = Vec::new();
    let mut selected_row = 0;

    for (i, entry) in slice.iter().enumerate() {
        // Blank row between project groups. Skipped on the first visible row:
        // if the viewport starts on a header, a spacer above it wastes a line.
        if i > 0 && matches!(entry, TreeEntry::ProjectHeader { .. }) {
            items.push(ListItem::new(""));
        }

        let item = match entry {
            TreeEntry::ProjectHeader { name, languages } => {
                let lang_str = if languages.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", languages.join(", "))
                };

                let text = Line::from(vec![Span::styled(
                    format!("  {}{}", name, lang_str),
                    Style::default().fg(theme::STRUCTURE),
                )]);
                ListItem::new(text)
            }
            TreeEntry::TargetDir {
                path,
                size,
                last_modified,
            } => {
                let checked = if state.selected.contains(path) {
                    "\u{25CF}"
                } else {
                    " "
                };
                // A target dir is the last child when the entry after it starts a new
                // project group (or the tree ends). Derived here so the model never has
                // to store and re-maintain a rendering flag on every tree mutation.
                let is_last = matches!(
                    state.tree.get(i + scroll_offset + 1),
                    None | Some(TreeEntry::ProjectHeader { .. })
                );
                let prefix = if is_last { "\u{2514}" } else { "\u{251C}" };
                let display_path = path.strip_prefix(&state.scan_path).unwrap_or(path);
                let path_str = display_path.to_string_lossy().to_string();
                let sized_str = humansize::format_size(*size, humansize::BINARY);
                let modified = relative_time(*last_modified);

                let selected_style = if state.selected.contains(path) {
                    Style::default()
                        .fg(theme::SELECT)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };

                if use_alignment {
                    let padded_path = format!("{:width$}", path_str, width = max_path_width + 2);
                    let text = Line::from(vec![
                        Span::raw(format!(" {} ", prefix)),
                        Span::styled(format!("[{}]", checked), selected_style),
                        Span::raw(" "),
                        Span::styled(padded_path, selected_style),
                        Span::raw(" "),
                        Span::raw(sized_str),
                        Span::raw(" "),
                        Span::styled(modified, Style::default().fg(theme::MUTED)),
                    ]);
                    ListItem::new(text)
                } else {
                    let text = Line::from(vec![
                        Span::raw(format!(" {} ", prefix)),
                        Span::styled(format!("[{}]", checked), selected_style),
                        Span::raw(" "),
                        Span::styled(path_str, selected_style),
                        Span::raw(" "),
                        Span::raw(sized_str),
                        Span::raw(" "),
                        Span::styled(modified, Style::default().fg(theme::MUTED)),
                    ]);
                    ListItem::new(text)
                }
            }
        };

        // Track the cursor's row while building: spacers shift rows down, so the
        // position can no longer be derived as list_index - scroll_offset.
        if scroll_offset + i == state.list_index {
            selected_row = items.len();
        }
        items.push(item);
    }

    let mut list_state = ListState::default();
    list_state.select(Some(selected_row));

    let list = List::new(items)
        .block(Block::default())
        .highlight_style(theme::cursor());

    frame.render_stateful_widget(list, area, &mut list_state);
}

fn relative_time(dt: DateTime<Utc>) -> String {
    let now = Utc::now();
    let diff = now.signed_duration_since(dt);

    if diff.num_seconds() < 60 {
        "just now".to_string()
    } else if diff.num_minutes() < 60 {
        format!("{} min ago", diff.num_minutes())
    } else if diff.num_hours() < 24 {
        format!("{} hours ago", diff.num_hours())
    } else if diff.num_days() < 30 {
        format!("{} days ago", diff.num_days())
    } else if diff.num_days() < 365 {
        format!("{} months ago", diff.num_days() / 30)
    } else {
        format!("{} years ago", diff.num_days() / 365)
    }
}

fn render_confirm_dialog(state: &AppState, frame: &mut Frame, area: Rect) {
    let dialog_width = 60.min(area.width.saturating_sub(4));
    let dialog_height = 18.min(area.height.saturating_sub(4));
    let x = (area.width - dialog_width) / 2;
    let y = (area.height - dialog_height) / 2;

    let dialog_area = Rect {
        x: area.x + x,
        y: area.y + y,
        width: dialog_width,
        height: dialog_height,
    };

    frame.render_widget(Clear, dialog_area);

    let mut lines = vec![
        Line::from(Span::styled(
            " Delete selected directories?",
            theme::emphasis(),
        )),
        Line::from(""),
    ];

    for entry in &state.tree {
        if let TreeEntry::TargetDir { path, size, .. } = entry {
            if !state.selected.contains(path) {
                continue;
            }
            let dir_name = path
                .file_name()
                .map(|n| n.to_string_lossy())
                .unwrap_or_default();
            let size_str = humansize::format_size(*size, humansize::BINARY);
            lines.push(Line::from(vec![
                Span::styled("  \u{25CF} ", Style::default().fg(theme::SELECT)),
                Span::styled(dir_name.to_string(), Style::default()),
                Span::raw(format!("({})", size_str)),
            ]));
        }
    }

    let total_str = humansize::format_size(state.total_selected_size, humansize::BINARY);
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!("  Total: {}", total_str),
        theme::emphasis(),
    )));
    lines.push(Line::from(""));

    let variants = [
        (DeletePreference::DryRun, "Dry run (show only)"),
        (DeletePreference::Trash, "Trash (move to trash)"),
        (DeletePreference::Permanent, "Permanent delete"),
    ];

    for (variant, label) in &variants {
        let is_active = *variant == state.delete_preference;
        let bullet = if is_active { "\u{25CF}" } else { "\u{25CB}" };
        let fg = if is_active {
            theme::SELECT
        } else {
            Color::Reset
        };
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(
                format!("{} {}", bullet, label),
                Style::default().fg(fg).add_modifier(if is_active {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
            ),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " [Enter] confirm  [Esc] cancel  [\u{2191}/\u{2193}] toggle mode",
        Style::default().fg(theme::MUTED),
    )));

    let dialog = Paragraph::new(Text::from(lines))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .style(Style::default().fg(theme::STRUCTURE)),
        )
        .alignment(Alignment::Left);

    frame.render_widget(dialog, dialog_area);
}

fn render_confirm_quit_dialog(_state: &AppState, frame: &mut Frame, area: Rect) {
    let dialog_width = 40.min(area.width.saturating_sub(4));
    let dialog_height = 7.min(area.height.saturating_sub(4));
    let x = (area.width - dialog_width) / 2;
    let y = (area.height - dialog_height) / 2;

    let dialog_area = Rect {
        x: area.x + x,
        y: area.y + y,
        width: dialog_width,
        height: dialog_height,
    };

    frame.render_widget(Clear, dialog_area);

    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(" Quit?", theme::emphasis())),
        Line::from(""),
        Line::from(Span::styled(
            " [Enter] confirm  [Esc] cancel",
            Style::default().fg(theme::MUTED),
        )),
    ];

    let dialog = Paragraph::new(Text::from(lines))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .style(Style::default().fg(theme::STRUCTURE)),
        )
        .alignment(Alignment::Center);

    frame.render_widget(dialog, dialog_area);
}

fn render_deleting(state: &AppState, frame: &mut Frame, area: Rect) {
    let dialog_width = 36.min(area.width.saturating_sub(4));
    let dialog_height = 9.min(area.height.saturating_sub(4));
    let x = (area.width - dialog_width) / 2;
    let y = (area.height - dialog_height) / 2;

    let dialog_area = Rect {
        x: area.x + x,
        y: area.y + y,
        width: dialog_width,
        height: dialog_height,
    };

    frame.render_widget(Clear, dialog_area);

    let current = state
        .deleting_paths
        .get(state.deleting_index)
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    let progress = format!(
        "{}/{}",
        state.deleting_index + 1,
        state.deleting_paths.len()
    );

    let lines = vec![
        Line::from(Span::styled(
            " Deleting...",
            Style::default()
                .fg(theme::CAUTION)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(format!("  {}", current), Style::default())),
        Line::from(""),
        Line::from(Span::styled(
            format!("  {} complete", progress),
            Style::default().fg(theme::MUTED),
        )),
    ];

    let dialog = Paragraph::new(Text::from(lines))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .style(Style::default().fg(theme::STRUCTURE)),
        )
        .alignment(Alignment::Left);

    frame.render_widget(dialog, dialog_area);
}

fn render_status_bar(state: &AppState, frame: &mut Frame, area: Rect) {
    if state.phase == AppPhase::Browsing {
        let (hints, extra) = if let Some(ref summary) = state.delete_result_summary {
            let text = Line::from(vec![
                Span::styled(
                    format!(" {}", summary),
                    Style::default()
                        .fg(theme::SELECT)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                Span::styled(
                    format!("  {}/{}", state.list_index + 1, state.tree.len()),
                    Style::default().fg(theme::MUTED),
                ),
            ]);
            let status = Paragraph::new(text);
            frame.render_widget(status, area);
            return;
        } else {
            let hints =
                " [Space] toggle  [a] all  [d] none  [Enter] delete  [o] order-by  [q] quit";

            let mut extra_parts = Vec::new();
            if let Some(ref size_status) = state.sizes_status() {
                extra_parts.push(size_status.clone());
            }
            if state.selection_count() > 0 {
                extra_parts.push(format!("{} selected", state.selection_count()));
                extra_parts.push(humansize::format_size(
                    state.total_selected_size,
                    humansize::BINARY,
                ));
            }
            extra_parts.push(format!("{}/{}", state.list_index + 1, state.tree.len()));
            let extra = format!("  {}", extra_parts.join(" | "));

            (hints, extra)
        };

        let text = Line::from(vec![
            Span::styled(hints, Style::default().fg(theme::MUTED)),
            Span::raw(" "),
            Span::styled(extra, Style::default().fg(theme::MUTED)),
        ]);

        let status = Paragraph::new(text);
        frame.render_widget(status, area);
    }
}

fn render_order_dialog(state: &AppState, frame: &mut Frame, area: Rect) {
    let dialog_width = 36.min(area.width.saturating_sub(4));
    let dialog_height = 13.min(area.height.saturating_sub(4));
    let x = (area.width - dialog_width) / 2;
    let y = (area.height - dialog_height) / 2;

    let dialog_area = Rect {
        x: area.x + x,
        y: area.y + y,
        width: dialog_width,
        height: dialog_height,
    };

    frame.render_widget(Clear, dialog_area);

    let variants = [
        OrderBy::NameAsc,
        OrderBy::NameDesc,
        OrderBy::DateAsc,
        OrderBy::DateDesc,
        OrderBy::SizeAsc,
        OrderBy::SizeDesc,
    ];

    let mut lines = vec![
        Line::from(Span::styled(" Order by", theme::emphasis())),
        Line::from(""),
    ];

    for variant in &variants {
        let is_active = *variant == state.order_cursor;
        let bullet = if is_active { "\u{25CF}" } else { "\u{25CB}" };
        let fg = if is_active {
            theme::SELECT
        } else {
            Color::Reset
        };
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(
                format!("{} {}", bullet, variant.label()),
                Style::default().fg(fg).add_modifier(if is_active {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
            ),
        ]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " [Enter] apply  [Esc] cancel  [\u{2191}/\u{2193}] change",
        Style::default().fg(theme::MUTED),
    )));

    let dialog = Paragraph::new(Text::from(lines))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .style(Style::default().fg(theme::STRUCTURE)),
        )
        .alignment(Alignment::Left);

    frame.render_widget(dialog, dialog_area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};
    use std::path::PathBuf;

    fn two_project_output() -> crate::scanner::ScanOutput {
        use crate::scanner::{ProjectInfo, ScannedDir};
        use chrono::Utc;
        let projects = vec![
            ProjectInfo {
                path: PathBuf::from("/test/app-a"),
                name: "app-a".to_string(),
                languages: vec!["js".to_string()],
                children: vec![PathBuf::from("/test/app-a/node_modules")],
            },
            ProjectInfo {
                path: PathBuf::from("/test/app-b"),
                name: "app-b".to_string(),
                languages: vec!["js".to_string()],
                children: vec![PathBuf::from("/test/app-b/node_modules")],
            },
        ];
        let dirs = vec![
            ScannedDir {
                path: PathBuf::from("/test/app-a/node_modules"),
                size: 1_000_000,
                last_modified: Utc::now(),
                error: None,
            },
            ScannedDir {
                path: PathBuf::from("/test/app-b/node_modules"),
                size: 2_000_000,
                last_modified: Utc::now(),
                error: None,
            },
        ];
        crate::scanner::ScanOutput {
            target_dirs: dirs,
            projects,
            errors: Vec::new(),
            stats: crate::scanner::ScanStats::default(),
        }
    }

    fn row(term: &Terminal<TestBackend>, r: u16) -> String {
        let buf = term.backend().buffer();
        (0..buf.area.width)
            .map(|x| {
                buf.cell((x, r))
                    .map(|c| c.symbol().to_string())
                    .unwrap_or_default()
            })
            .collect()
    }

    fn reversed_rows(term: &Terminal<TestBackend>) -> Vec<u16> {
        let buf = term.backend().buffer();
        (0..buf.area.height)
            .filter(|&r| {
                (0..buf.area.width).any(|x| {
                    buf.cell((x, r))
                        .is_some_and(|c| c.style().add_modifier.contains(Modifier::REVERSED))
                })
            })
            .collect()
    }

    #[test]
    fn tree_render_has_spacer_and_glyphs_and_cursor() {
        let mut state = AppState::new(PathBuf::from("/test"));
        state.build_tree(two_project_output());
        state.phase = AppPhase::Browsing;
        state.list_index = 3;
        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
        terminal.draw(|f| render(&state, f)).unwrap();

        let header_b_row = (0..20u16)
            .find(|&r| row(&terminal, r).contains(" app-b (js) "))
            .unwrap();
        let spacer_row = row(&terminal, header_b_row - 1);
        assert!(
            spacer_row.trim().is_empty(),
            "expected blank spacer, got {:?}",
            spacer_row
        );
        let first_dir_row = (0..20u16)
            .find(|&r| row(&terminal, r).contains("app-a/node_modules"))
            .unwrap();
        assert!(
            row(&terminal, first_dir_row).contains('\u{2514}'),
            "first group's only child should be last-glyph"
        );
        let cursor_rows: Vec<_> = reversed_rows(&terminal);
        assert_eq!(
            cursor_rows.len(),
            1,
            "expected exactly one reverse-video cursor row"
        );
        assert!(row(&terminal, cursor_rows[0]).contains("app-b/node_modules"));
    }
}
