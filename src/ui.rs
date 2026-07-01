use chrono::{DateTime, Utc};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph},
};

use crate::app::{AppPhase, AppState, DeletePreference, OrderBy, TreeEntry};

mod palette {
    use ratatui::style::Color;

    pub const BG: Color = Color::Black;
    pub const HEADER: Color = Color::Rgb(180, 200, 255);
    pub const STATUS: Color = Color::Rgb(180, 220, 200);
    pub const NOTIFY: Color = Color::Rgb(80, 220, 120);

    pub const PROJECT: Color = Color::Rgb(100, 200, 255);
    pub const SELECTED: Color = Color::Rgb(80, 255, 120);
    pub const SIZE: Color = Color::Rgb(255, 220, 80);
    pub const TIME: Color = Color::Rgb(160, 165, 175);
    pub const HIGHLIGHT: Color = Color::Rgb(255, 200, 100);

    pub const DIALOG_BORDER: Color = Color::Rgb(100, 160, 220);
    pub const DIALOG_TITLE: Color = Color::White;
    pub const DIALOG_TEXT: Color = Color::Rgb(220, 220, 220);
    pub const DIALOG_HINT: Color = Color::Rgb(160, 165, 175);

    pub const ERROR: Color = Color::Rgb(255, 180, 60);
    pub const ERROR_BORDER: Color = Color::Rgb(220, 140, 60);

    pub const SCANNING: Color = Color::Rgb(100, 220, 255);
    pub const DELETING: Color = Color::Rgb(255, 220, 80);
}

pub fn render(state: &mut AppState, frame: &mut Frame) {
    frame.render_widget(
        Block::default().style(Style::default().bg(palette::BG)),
        frame.area(),
    );

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
                .fg(palette::HEADER)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(stats, Style::default().fg(palette::SELECTED)),
    ]);

    frame.render_widget(Paragraph::new(header), area);
}

fn render_main(state: &mut AppState, frame: &mut Frame, area: Rect) {
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
                Style::default().fg(palette::ERROR),
            ))
        })
        .collect();

    let text = Paragraph::new(Text::from(errors))
        .block(
            Block::default()
                .borders(Borders::TOP)
                .title(" Errors")
                .style(Style::default().fg(palette::ERROR_BORDER)),
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
                .fg(palette::SCANNING)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            " Walking directory tree for known build artifacts",
            Style::default().fg(palette::TIME),
        )),
    ]))
    .style(Style::default())
    .block(Block::default());

    frame.render_widget(text, area);
}

fn render_browsing(state: &mut AppState, frame: &mut Frame, area: Rect) {
    if state.tree.is_empty() {
        let text = Paragraph::new(" No sweepable directories found.")
            .style(Style::default().fg(palette::TIME))
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

    let max_path_width = state
        .tree
        .iter()
        .filter_map(|e| {
            if let TreeEntry::TargetDir { path, .. } = e {
                let display_path = path.strip_prefix(&state.scan_path).unwrap_or(path);
                Some(display_path.to_string_lossy().len())
            } else {
                None
            }
        })
        .max()
        .unwrap_or(0);

    // Use table alignment only when terminal has enough room.
    // Overhead: highlight(2) + prefix(3) + checkbox(3) + spaces(3) + max_size(8) + max_time(12) + margin(2)
    let use_alignment = max_path_width + 33 <= area.width as usize;

    let items: Vec<ListItem> = slice
        .iter()
        .map(|entry| match entry {
            TreeEntry::ProjectHeader {
                name,
                package_manager,
            } => {
                let pm = package_manager
                    .as_deref()
                    .map(|s| format!(" ({})", s))
                    .unwrap_or_default();

                let text = Line::from(vec![Span::styled(
                    format!("  {}{}", name, pm),
                    Style::default().fg(palette::PROJECT),
                )]);
                ListItem::new(text)
            }
            TreeEntry::TargetDir {
                path,
                size,
                last_modified,
                is_last,
                ..
            } => {
                let checked = if state.selected.contains(path) {
                    "\u{25CF}"
                } else {
                    " "
                };
                let prefix = if *is_last { "\u{2514}" } else { "\u{251C}" };
                let display_path = path.strip_prefix(&state.scan_path).unwrap_or(path);
                let path_str = display_path.to_string_lossy().to_string();
                let sized_str = humansize::format_size(*size, humansize::BINARY);
                let modified = relative_time(*last_modified);

                let selected_style = if state.selected.contains(path) {
                    Style::default()
                        .fg(palette::SELECTED)
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
                        Span::styled(sized_str, Style::default().fg(palette::SIZE)),
                        Span::raw(" "),
                        Span::styled(modified, Style::default().fg(palette::TIME)),
                    ]);
                    ListItem::new(text)
                } else {
                    let text = Line::from(vec![
                        Span::raw(format!(" {} ", prefix)),
                        Span::styled(format!("[{}]", checked), selected_style),
                        Span::raw(" "),
                        Span::styled(path_str, selected_style),
                        Span::raw(" "),
                        Span::styled(sized_str, Style::default().fg(palette::SIZE)),
                        Span::raw(" "),
                        Span::styled(modified, Style::default().fg(palette::TIME)),
                    ]);
                    ListItem::new(text)
                }
            }
        })
        .collect();

    let mut list_state = ListState::default();
    list_state.select(Some(state.list_index - scroll_offset));

    let list = List::new(items)
        .block(Block::default())
        .highlight_style(
            Style::default()
                .fg(palette::HIGHLIGHT)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("\u{25B6} ");

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
            Style::default()
                .fg(palette::DIALOG_TITLE)
                .add_modifier(Modifier::BOLD),
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
                Span::styled("  \u{25CF} ", Style::default().fg(palette::SELECTED)),
                Span::styled(
                    dir_name.to_string(),
                    Style::default().fg(palette::DIALOG_TEXT),
                ),
                Span::raw(" "),
                Span::styled(
                    format!("({})", size_str),
                    Style::default().fg(palette::SIZE),
                ),
            ]));
        }
    }

    let total_str = humansize::format_size(state.total_selected_size, humansize::BINARY);
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!("  Total: {}", total_str),
        Style::default()
            .fg(palette::DIALOG_TITLE)
            .add_modifier(Modifier::BOLD),
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
            palette::SELECTED
        } else {
            palette::DIALOG_TEXT
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
        Style::default().fg(palette::DIALOG_HINT),
    )));

    let dialog = Paragraph::new(Text::from(lines))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .style(Style::default().fg(palette::DIALOG_BORDER)),
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
        Line::from(Span::styled(
            " Quit?",
            Style::default()
                .fg(palette::DIALOG_TITLE)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            " [Enter] confirm  [Esc] cancel",
            Style::default().fg(palette::DIALOG_HINT),
        )),
    ];

    let dialog = Paragraph::new(Text::from(lines))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .style(Style::default().fg(palette::DIALOG_BORDER)),
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
                .fg(palette::DELETING)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            format!("  {}", current),
            Style::default().fg(palette::DIALOG_TEXT),
        )),
        Line::from(""),
        Line::from(Span::styled(
            format!("  {} complete", progress),
            Style::default().fg(palette::TIME),
        )),
    ];

    let dialog = Paragraph::new(Text::from(lines))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .style(Style::default().fg(palette::DIALOG_BORDER)),
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
                        .fg(palette::NOTIFY)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                Span::styled(
                    format!("  {}/{}", state.list_index + 1, state.tree.len()),
                    Style::default().fg(palette::STATUS),
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
            Span::styled(hints, Style::default().fg(palette::STATUS)),
            Span::raw(" "),
            Span::styled(extra, Style::default().fg(palette::STATUS)),
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
        Line::from(Span::styled(
            " Order by",
            Style::default()
                .fg(palette::DIALOG_TITLE)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];

    for variant in &variants {
        let is_active = *variant == state.order_cursor;
        let bullet = if is_active { "\u{25CF}" } else { "\u{25CB}" };
        let fg = if is_active {
            palette::SELECTED
        } else {
            palette::DIALOG_TEXT
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
        Style::default().fg(palette::DIALOG_HINT),
    )));

    let dialog = Paragraph::new(Text::from(lines))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .style(Style::default().fg(palette::DIALOG_BORDER)),
        )
        .alignment(Alignment::Left);

    frame.render_widget(dialog, dialog_area);
}
