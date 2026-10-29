use chrono::{DateTime, Utc};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph},
};

use crate::app::{
    AppPhase, AppState, DeletePreference, MouseAction, MouseZones, OrderBy, TreeEntry,
};
use crate::theme;

/// Renders one frame and returns its clickable regions for mouse hit-testing.
pub fn render(state: &AppState, frame: &mut Frame) -> MouseZones {
    // No background paint: the terminal's own theme shows through.
    let areas = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .split(frame.area());

    let mut zones = MouseZones::new();

    render_header(state, frame, areas[0]);
    render_main(state, frame, areas[1], &mut zones);
    render_button_bar(state, frame, areas[2], &mut zones);

    // A running sweep locks the whole interface: whatever zones the list or
    // the bar registered, none is offered while the worker works underneath.
    if state.phase == AppPhase::Deleting {
        zones.clear();
    }

    zones
}

fn render_header(state: &AppState, frame: &mut Frame, area: Rect) {
    let scan_path = state.scan_path.to_string_lossy();

    // Measuring is checked before the reclaimable total: sizes stream in
    // asynchronously, so a zero total mid-flight is "unknown", not "empty".
    let (stats, stats_color) = if state.phase == AppPhase::Scanning {
        (String::new(), theme::SELECT)
    } else if let Some(summary) = state.delete_result_summary.as_ref() {
        // A finished sweep shows as a plain header notice — no centered
        // panel for bookkeeping the shrinking "you can free" total already
        // confirmed. It vanishes on the next action.
        let color = if summary.contains("failed") || summary.contains("interrupted") {
            theme::DANGER
        } else {
            theme::MUTED
        };
        (format!("  {summary}"), color)
    } else if state.phase == AppPhase::Deleting {
        // Same slot as the size pass: a one-line counter while rows vanish
        // from the list in place — no modal takeover for a sweep.
        let word = if state.delete_preference == DeletePreference::DryRun {
            "previewing"
        } else {
            "sweeping"
        };
        (
            format!(
                "  {} {}/{}",
                word,
                state.deleting_index.min(state.deleting_paths.len()),
                state.deleting_paths.len()
            ),
            theme::CAUTION,
        )
    } else if state.sizes_total > 0 && !state.sizes_complete {
        (
            format!(
                "  measuring sizes {}/{}",
                state.sizes_found, state.sizes_total
            ),
            theme::CAUTION,
        )
    } else if state.total_reclaimable == 0 {
        (String::from("  nothing found to clean"), theme::MUTED)
    } else {
        (
            format!(
                "  {} you can free  \u{b7}  scanned in {:.1}s",
                humansize::format_size(state.total_reclaimable, humansize::BINARY),
                state.scan_duration_ms as f64 / 1000.0,
            ),
            theme::SELECT,
        )
    };

    let header = Line::from(vec![
        Span::styled(
            format!(" dirsweep \u{2014} {}", scan_path),
            Style::default()
                .fg(theme::STRUCTURE)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(stats, Style::default().fg(stats_color)),
    ]);

    frame.render_widget(Paragraph::new(header), area);
}

fn render_main(state: &AppState, frame: &mut Frame, area: Rect, zones: &mut MouseZones) {
    match state.phase {
        AppPhase::Scanning => render_scanning(state, frame, area),
        AppPhase::Browsing | AppPhase::Deleting => {
            if state.errors.is_empty() {
                render_browsing(state, frame, area, zones);
            } else {
                let [tree_area, error_area] =
                    Layout::vertical([Constraint::Min(0), Constraint::Length(3)]).areas(area);
                render_browsing(state, frame, tree_area, zones);
                render_errors(state, frame, error_area);
            }
        }
        AppPhase::ConfirmDelete => {
            render_browsing(state, frame, area, zones);
            render_confirm_dialog(state, frame, area, zones);
        }
        AppPhase::OrderDialog => {
            render_browsing(state, frame, area, zones);
            render_order_dialog(state, frame, area, zones);
        }
        AppPhase::Help => {
            render_browsing(state, frame, area, zones);
            render_help_dialog(frame, area, zones);
        }
        AppPhase::Quit => {}
    }
}

/// The `?` help overlay: every key and button on one screen, grouped by
/// purpose, in plain language. Rendered over the browsing view. Any key or a
/// click on the overlay returns to browsing.
fn render_help_dialog(frame: &mut Frame, area: Rect, zones: &mut MouseZones) {
    fn row(key: &str, desc: &str) -> Line<'static> {
        Line::from(vec![
            Span::raw("  "),
            Span::styled(format!("{:<10}", key), theme::keycap()),
            Span::styled(desc.to_string(), Style::default()),
        ])
    }

    fn group(title: &str) -> Line<'static> {
        Line::from(Span::styled(
            format!(" {}", title),
            Style::default()
                .fg(theme::STRUCTURE)
                .add_modifier(Modifier::BOLD),
        ))
    }

    let lines = vec![
        Line::from(Span::styled(" Help", theme::emphasis())),
        Line::from(""),
        group("Navigate"),
        row("[\u{2191}/\u{2193}]", "Move the cursor"),
        row("[Home/End]", "First or last directory"),
        row("[PgUp/PgDn]", "Jump a page at a time"),
        row("[Space]", "Toggle the selected row"),
        group("Select"),
        row("[a]", "Select everything"),
        row("[d]", "Select nothing"),
        group("Actions"),
        row("[Enter]", "Delete what you selected"),
        row("[o]", "Sort the list"),
        group("Other"),
        row("[?]", "Show or close this help"),
        row("[q]", "Quit"),
    ];

    let content_height = lines.len();
    let dialog_width = 50.min(area.width.saturating_sub(4));
    let dialog_height = ((content_height + 2) as u16).min(area.height.saturating_sub(4).max(3));
    let x = (area.width - dialog_width) / 2;
    let y = (area.height - dialog_height) / 2;

    let dialog_area = Rect {
        x: area.x + x,
        y: area.y + y,
        width: dialog_width,
        height: dialog_height,
    };

    frame.render_widget(Clear, dialog_area);
    zones.push((dialog_area, MouseAction::Dismiss));

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

fn render_scanning(state: &AppState, frame: &mut Frame, area: Rect) {
    let text = Paragraph::new(Text::from(vec![
        Line::from(""),
        Line::from(Span::styled(
            format!(" Scanning {} \u{2026}", state.scan_path.display()),
            Style::default()
                .fg(theme::STRUCTURE)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            " Looking for build folders \u{2014} node_modules, target, build, dist, \u{2026}",
            Style::default().fg(theme::MUTED),
        )),
    ]))
    .style(Style::default())
    .block(Block::default());

    frame.render_widget(text, area);
}

fn render_browsing(state: &AppState, frame: &mut Frame, area: Rect, zones: &mut MouseZones) {
    if state.tree.is_empty() {
        let text = Paragraph::new(Text::from(vec![
            Line::from(Span::styled(
                format!(
                    " No build folders found under {}.",
                    state.scan_path.display()
                ),
                Style::default(),
            )),
            Line::from(""),
            Line::from(Span::styled(
                " Try sweeping a parent folder:  dirsweep --dir ~/projects",
                theme::muted(),
            )),
        ]));
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
        // No spacer rows between project groups: groups separate by color
        // and bold alone, keeping the list one row tighter per group.

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

        // Track the cursor's row while building: one item per tree entry, so
        // the position is list_index - scroll_offset — kept derived rather
        // than assumed in case inter-item rows return (e.g. group spacing).
        if scroll_offset + i == state.list_index {
            selected_row = items.len();
        }
        // Every visible row is clickable: headers move the cursor, target rows
        // toggle their checkbox.
        zones.push((
            Rect::new(area.x, area.y + items.len() as u16, area.width, 1),
            match entry {
                TreeEntry::TargetDir { .. } => MouseAction::ToggleAt(scroll_offset + i),
                TreeEntry::ProjectHeader { .. } => MouseAction::MoveTo(scroll_offset + i),
            },
        ));
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

/// How many content lines the chooser needs, given `entries` visible rows and
/// whether a "more" overflow notice is shown.
fn confirm_content_height(entries: usize, truncated: bool) -> usize {
    // title + blank + entries + [more] + blank + total + blank + 3 options
    // + blank + hint
    9 + entries + usize::from(truncated)
}

fn render_confirm_dialog(state: &AppState, frame: &mut Frame, area: Rect, zones: &mut MouseZones) {
    let selected: Vec<(&std::path::Path, u64)> = state
        .tree
        .iter()
        .filter_map(|e| match e {
            TreeEntry::TargetDir { path, size, .. } if state.selected.contains(path) => {
                Some((path.as_path(), *size))
            }
            _ => None,
        })
        .collect();

    // Size the dialog from its content, then clamp to the terminal. When the
    // entries don't all fit, reserve a line for the "… and N more" notice
    // instead of silently overflowing (long selections used to spill past
    // the border).
    let height_cap = area.height.saturating_sub(4);
    let content_cap = height_cap.saturating_sub(2);
    let entries_shown = if selected.len() + 9 <= content_cap as usize {
        selected.len()
    } else {
        content_cap.saturating_sub(10) as usize
    };
    let truncated = selected.len() > entries_shown;
    let content_height = confirm_content_height(entries_shown, truncated);
    let dialog_width = 62.min(area.width.saturating_sub(4));
    let dialog_height = ((content_height + 2) as u16).min(height_cap.max(3));
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

    for (path, size) in selected.iter().take(entries_shown) {
        let dir_name = path
            .file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default();
        lines.push(Line::from(vec![
            Span::styled("  \u{25CF} ", Style::default().fg(theme::SELECT)),
            Span::styled(dir_name.to_string(), Style::default()),
            Span::raw(format!(
                " ({})",
                humansize::format_size(*size, humansize::BINARY)
            )),
        ]));
    }
    if truncated {
        lines.push(Line::from(Span::styled(
            format!("  \u{2026} and {} more", selected.len() - entries_shown),
            theme::muted(),
        )));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!(
            "  Total: {}",
            humansize::format_size(state.total_selected_size, humansize::BINARY)
        ),
        theme::emphasis(),
    )));
    lines.push(Line::from(""));

    // One row per mode: the active mode carries the color of its
    // consequence, but every mode stays visible with a plain-language
    // description. The destructive option is the only red on screen.
    let variants = [
        DeletePreference::DryRun,
        DeletePreference::Trash,
        DeletePreference::Permanent,
    ];

    let options_start = lines.len();
    for variant in &variants {
        let active = *variant == state.delete_preference;
        let bullet = if active { "\u{25CF}" } else { "\u{25CB}" };
        let fg = match (*variant, active) {
            (DeletePreference::Permanent, true) => theme::DANGER,
            (_, true) => theme::SELECT,
            (_, false) => Color::Reset,
        };
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(
                format!("{} {}", bullet, variant.label()),
                Style::default().fg(fg).add_modifier(if active {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
            ),
            Span::raw("  "),
            Span::styled(variant.description(), theme::muted()),
        ]));
    }
    let hint_index = lines.len();
    lines.push(Line::from(Span::styled(
        " [\u{2191}/\u{2193}] choose  [Enter] confirm  [Esc] cancel",
        theme::muted(),
    )));

    // The chooser is fully clickable: mode rows pick a mode, the Enter and
    // Esc hints act as confirm/cancel buttons for mouse-only users.
    for (i, variant) in variants.iter().enumerate() {
        zones.push((
            Rect::new(
                dialog_area.x,
                dialog_area.y + 1 + (options_start + i) as u16,
                dialog_area.width,
                1,
            ),
            MouseAction::SetDeleteMode(*variant),
        ));
    }
    let confirm_x = " [\u{2191}/\u{2193}] choose  ".len() as u16;
    for (rect, action) in [
        (
            Rect::new(
                dialog_area.x + confirm_x,
                dialog_area.y + 1 + hint_index as u16,
                "[Enter] confirm".len() as u16,
                1,
            ),
            MouseAction::Confirm,
        ),
        (
            Rect::new(
                dialog_area.x + confirm_x + "[Enter] confirm".len() as u16 + 2,
                dialog_area.y + 1 + hint_index as u16,
                "[Esc] cancel".len() as u16,
                1,
            ),
            MouseAction::Cancel,
        ),
    ] {
        zones.push((rect, action));
    }

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

/// Centered summary of a completed sweep. The border color carries the
/// outcome at a glance: green for clean, red when anything failed. Clicking
/// anywhere on the panel (or pressing any key) dismisses it.
/// Persistent action bar: every action is a visible, clickable button that
/// also names its keyboard shortcut, so the mouse and the keys teach each
/// other. The cursor position indicator sits separately on the right.
///
/// The bar renders only in `Browsing` — dialogs and overlays cover it, and
/// during `Deleting` no action is safe to offer.
fn render_button_bar(state: &AppState, frame: &mut Frame, area: Rect, zones: &mut MouseZones) {
    if state.phase != AppPhase::Browsing {
        return;
    }

    // Position indicator on the right, with a two-column gap from the buttons.
    // Counts sweepable targets only: project headers are group titles the
    // navigation skips, not rows the user can act on.
    let total = state.target_count();
    let pos = if total == 0 {
        String::from("0/0")
    } else {
        format!("{}/{}", state.target_position(), total)
    };

    let selected = state.selection_count();
    let delete_label = if selected > 0 {
        format!(
            "Delete ({} \u{b7} {})",
            selected,
            humansize::format_size(state.total_selected_size, humansize::BINARY)
        )
    } else {
        String::from("Delete")
    };

    struct Button {
        key: &'static str,
        label: String,
        action: MouseAction,
        enabled: bool,
    }

    let buttons = [
        Button {
            key: "Space",
            label: String::from("Toggle"),
            action: MouseAction::Toggle,
            enabled: true,
        },
        Button {
            key: "a",
            label: String::from("All"),
            action: MouseAction::SelectAll,
            enabled: true,
        },
        Button {
            key: "d",
            label: String::from("None"),
            action: MouseAction::DeselectAll,
            enabled: true,
        },
        Button {
            key: "o",
            label: String::from("Sort"),
            action: MouseAction::OpenSort,
            enabled: true,
        },
        Button {
            key: "?",
            label: String::from("Help"),
            action: MouseAction::OpenHelp,
            enabled: true,
        },
        Button {
            key: "Enter",
            label: delete_label,
            action: MouseAction::Delete,
            enabled: selected > 0,
        },
        Button {
            key: "q",
            label: String::from("Quit"),
            action: MouseAction::Quit,
            enabled: true,
        },
    ];

    // Buttons must fit to the left of the position indicator.
    let usable = area
        .width
        .saturating_sub(pos.len() as u16)
        .saturating_sub(2);

    let mut spans: Vec<Span> = Vec::new();
    let mut x = area.x;
    for b in &buttons {
        // Each button renders as `[key] label` plus a two-column separator.
        let key_text = format!("[{}]", b.key);
        let width = (key_text.len() + 1 + b.label.len()) as u16;
        if x + width + 2 > area.x + usable {
            break;
        }

        let (key_style, label_style) = if b.enabled {
            (theme::keycap(), Style::default())
        } else {
            (theme::muted(), theme::muted())
        };

        let rect = Rect::new(x, area.y, width, 1);
        zones.push((rect, b.action));

        spans.push(Span::styled(key_text, key_style));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(b.label.clone(), label_style));
        spans.push(Span::raw("  "));
        x += width + 2;
    }

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
    frame.render_widget(
        Paragraph::new(Span::styled(pos, theme::muted())).alignment(Alignment::Right),
        area,
    );
}

fn render_order_dialog(state: &AppState, frame: &mut Frame, area: Rect, zones: &mut MouseZones) {
    let variants = OrderBy::ALL;

    // title + blank + one row per variant + blank + hint, plus borders.
    let content_height = 4 + variants.len();
    let dialog_width = 40.min(area.width.saturating_sub(4));
    let dialog_height = ((content_height as u16) + 2).min(area.height.saturating_sub(4).max(3));
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
        Line::from(Span::styled(" Sort by", theme::emphasis())),
        Line::from(""),
    ];

    for (i, variant) in variants.iter().enumerate() {
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
                format!("{} {}. {}", bullet, i + 1, variant.label()),
                Style::default().fg(fg).add_modifier(if is_active {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
            ),
        ]));
    }

    lines.push(Line::from(""));
    let hint_index = lines.len();
    let hint = " [1\u{2013}6] pick  [\u{2191}/\u{2193}] choose  [Enter] apply  [Esc] cancel";
    lines.push(Line::from(Span::styled(hint, theme::muted())));

    for (i, variant) in variants.iter().enumerate() {
        zones.push((
            Rect::new(
                dialog_area.x,
                dialog_area.y + 1 + (2 + i) as u16,
                dialog_area.width,
                1,
            ),
            MouseAction::SortBy(*variant),
        ));
    }
    // Zone offsets come straight from the hint text so they can never drift
    // out of sync with what is rendered.
    let apply_x = hint.find("[Enter]").unwrap_or(0) as u16;
    let cancel_x = hint.find("[Esc]").unwrap_or(0) as u16;
    for (rect, action) in [
        (
            Rect::new(
                dialog_area.x + apply_x,
                dialog_area.y + 1 + hint_index as u16,
                "[Enter] apply".len() as u16,
                1,
            ),
            MouseAction::Confirm,
        ),
        (
            Rect::new(
                dialog_area.x + cancel_x,
                dialog_area.y + 1 + hint_index as u16,
                "[Esc] cancel".len() as u16,
                1,
            ),
            MouseAction::Cancel,
        ),
    ] {
        zones.push((rect, action));
    }

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
    fn tree_render_keeps_groups_adjacent_with_glyphs_and_cursor() {
        let mut state = AppState::new(PathBuf::from("/test"));
        state.build_tree(two_project_output());
        state.phase = AppPhase::Browsing;
        state.list_index = 3;
        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
        let mut unused_zones = MouseZones::new();
        terminal
            .draw(|f| {
                unused_zones = render(&state, f);
            })
            .unwrap();

        // Groups are adjacent: the row above the app-b header is the last
        // child of app-a, not a blank spacer.
        let header_b_row = (0..20u16)
            .find(|&r| row(&terminal, r).contains(" app-b (js) "))
            .unwrap();
        let row_above = row(&terminal, header_b_row - 1);
        assert!(
            row_above.contains("app-a/node_modules"),
            "expected the previous group's target above the header, got {row_above:?}"
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

    #[test]
    fn button_bar_registers_zone_per_button_and_row() {
        let mut state = AppState::new(PathBuf::from("/test"));
        state.build_tree(two_project_output());
        state.phase = AppPhase::Browsing;
        state.list_index = 3;
        state.select_all();

        let mut terminal = Terminal::new(TestBackend::new(120, 20)).unwrap();
        let mut zones = MouseZones::new();
        terminal.draw(|f| zones = render(&state, f)).unwrap();

        let actions: Vec<_> = zones.iter().map(|(_, a)| *a).collect();
        // Bar buttons.
        for expected in [
            MouseAction::Toggle,
            MouseAction::SelectAll,
            MouseAction::DeselectAll,
            MouseAction::OpenSort,
            MouseAction::OpenHelp,
            MouseAction::Delete,
            MouseAction::Quit,
        ] {
            assert!(actions.contains(&expected), "missing zone for {expected:?}");
        }
        // Tree rows: headers move, targets toggle. Tree is
        // [0] header, [1] target, [2] header, [3] target.
        assert!(actions.contains(&MouseAction::ToggleAt(1)));
        assert!(actions.contains(&MouseAction::ToggleAt(3)));
        assert!(actions.contains(&MouseAction::MoveTo(0)));
        assert!(actions.contains(&MouseAction::MoveTo(2)));

        // Every zone stays inside the 120x20 frame.
        for (rect, _) in &zones {
            assert!(rect.y < 20 && rect.x + rect.width <= 120, "zone {rect:?}");
        }

        // The bar itself names keys, the selection totals, and the position.
        let bar = row(&terminal, 19);
        assert!(bar.contains("[Space] Toggle"), "{bar}");
        assert!(bar.contains("[Enter] Delete (2"), "{bar}");
        assert!(bar.contains("[q] Quit"), "{bar}");
        // Position counts targets only: 4 tree entries, 2 of them headers.
        assert!(bar.contains("2/2"), "{bar}");
    }

    fn many_targets(n: usize) -> crate::scanner::ScanOutput {
        use crate::scanner::{ProjectInfo, ScannedDir};
        use chrono::Utc;
        let mut children = Vec::new();
        let mut dirs = Vec::new();
        for i in 0..n {
            let path = PathBuf::from(format!("/test/app/target-{:02}", i));
            children.push(path.clone());
            dirs.push(ScannedDir {
                path,
                size: 1_000,
                last_modified: Utc::now(),
                error: None,
            });
        }
        crate::scanner::ScanOutput {
            target_dirs: dirs,
            projects: vec![ProjectInfo {
                path: PathBuf::from("/test/app"),
                name: "app".to_string(),
                languages: vec!["js".to_string()],
                children,
            }],
            ..Default::default()
        }
    }

    #[test]
    fn confirm_dialog_explains_each_mode_in_plain_language() {
        let mut state = AppState::new(PathBuf::from("/test"));
        state.build_tree(two_project_output());
        state.phase = AppPhase::ConfirmDelete;
        state.select_all();
        // Default preference is Trash, the recommended mode.
        assert_eq!(state.delete_preference, DeletePreference::Trash);

        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
        let mut zones = MouseZones::new();
        terminal
            .draw(|f| {
                zones = render(&state, f);
            })
            .unwrap();

        let text: String = (0..20u16).map(|r| row(&terminal, r)).collect();
        assert!(text.contains("Delete selected directories?"), "{text}");
        assert!(text.contains("Preview only"), "{text}");
        assert!(text.contains("Move to Trash"), "{text}");
        assert!(text.contains("Delete permanently"), "{text}");
        assert!(text.contains("cannot be undone"), "{text}");
        assert!(text.contains("recoverable"), "{text}");
        assert!(text.contains("Total"), "{text}");

        // Each mode row is individually clickable.
        for preference in [
            DeletePreference::DryRun,
            DeletePreference::Trash,
            DeletePreference::Permanent,
        ] {
            assert!(
                zones
                    .iter()
                    .any(|(_, a)| *a == MouseAction::SetDeleteMode(preference)),
                "missing SetDeleteMode zone for {preference:?}"
            );
        }
        assert!(zones.iter().any(|(_, a)| *a == MouseAction::Confirm));
        assert!(zones.iter().any(|(_, a)| *a == MouseAction::Cancel));
    }

    #[test]
    fn confirm_dialog_truncates_overflow_with_a_count() {
        let mut state = AppState::new(PathBuf::from("/test"));
        state.build_tree(many_targets(20));
        state.phase = AppPhase::ConfirmDelete;
        state.select_all();

        let mut terminal = Terminal::new(TestBackend::new(60, 24)).unwrap();
        let mut zones = MouseZones::new();
        terminal
            .draw(|f| {
                zones = render(&state, f);
            })
            .unwrap();

        let text: String = (0..24u16).map(|r| row(&terminal, r)).collect();
        assert!(
            text.contains("\u{2026} and 14 more"),
            "expected a truncated-entries notice, got {text}"
        );
        // All three modes stay reachable even when entries don't fit.
        assert!(text.contains("Delete permanently"), "{text}");
        assert!(
            zones
                .iter()
                .any(|(_, a)| matches!(a, MouseAction::SetDeleteMode(_)))
        );
    }

    #[test]
    fn deleting_state_keeps_the_list_with_a_header_counter() {
        let mut state = AppState::new(PathBuf::from("/test"));
        state.build_tree(two_project_output());
        state.select_all();
        state.phase = AppPhase::Deleting;
        state.deleting_paths = vec![
            PathBuf::from("/test/app-a/node_modules"),
            PathBuf::from("/test/app-b/node_modules"),
        ];
        state.deleting_index = 1;
        state.arm_sweep();

        // Wide enough that every bar button — including Delete and Quit —
        // gets drawn and would otherwise register a zone.
        let mut terminal = Terminal::new(TestBackend::new(120, 24)).unwrap();
        let mut zones = MouseZones::new();
        terminal
            .draw(|f| {
                zones = render(&state, f);
            })
            .unwrap();

        let text: String = (0..24u16).map(|r| row(&terminal, r)).collect();
        // A real sweep takes the whole armed selection out of the list at
        // once — rows must not linger as stale entries over folders that
        // are already on their way out.
        assert!(text.contains("sweeping 1/2"), "{text}");
        assert!(!text.contains("app-a/node_modules"), "{text}");
        assert!(!text.contains("app-b/node_modules"), "{text}");
        // The whole interface is locked while the worker mutates the tree:
        // no destructive actions are offered.
        assert!(!zones.iter().any(|(_, a)| matches!(
            a,
            MouseAction::Delete | MouseAction::Confirm | MouseAction::Quit
        )));

        // A preview rehearses the same vanishing act; only the counter
        // wording differs.
        state.delete_preference = DeletePreference::DryRun;
        state.build_tree(two_project_output());
        state.select_all();
        state.phase = AppPhase::Deleting;
        state.deleting_index = 1;
        state.arm_sweep();
        terminal
            .draw(|f| {
                let _ = render(&state, f);
            })
            .unwrap();
        let text: String = (0..24u16).map(|r| row(&terminal, r)).collect();
        assert!(text.contains("previewing 1/2"), "{text}");
        assert!(!text.contains("app-a/node_modules"), "{text}");
    }

    #[test]
    fn sweep_result_is_a_header_notice_until_dismissed() {
        let mut state = AppState::new(PathBuf::from("/test"));
        state.build_tree(two_project_output());
        state.phase = AppPhase::Browsing;
        state.delete_result_summary = Some(String::from("swept 2 \u{b7} 2.9 MiB reclaimed"));

        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
        let mut zones = MouseZones::new();
        terminal
            .draw(|f| {
                zones = render(&state, f);
            })
            .unwrap();

        let text: String = (0..20u16).map(|r| row(&terminal, r)).collect();
        // No centered panel — the outcome rides in the header.
        assert!(text.contains("swept 2"), "{text}");
        assert!(!text.contains("Sweep complete"), "{text}");

        // Any action dismisses the notice; the header stats return.
        state.clear_notification();
        state.sizes_complete = true; // the scan's size pass would have finished
        terminal
            .draw(|f| {
                let _ = render(&state, f);
            })
            .unwrap();
        let text: String = (0..20u16).map(|r| row(&terminal, r)).collect();
        assert!(!text.contains("swept 2"), "{text}");
        assert!(text.contains("you can free"), "{text}");
    }

    #[test]
    fn sort_dialog_lists_orders_and_registers_zones() {
        let mut state = AppState::new(PathBuf::from("/test"));
        state.build_tree(two_project_output());
        state.phase = AppPhase::OrderDialog;
        state.order_cursor = OrderBy::SizeDesc;

        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
        let mut zones = MouseZones::new();
        terminal
            .draw(|f| {
                zones = render(&state, f);
            })
            .unwrap();

        let text: String = (0..20u16).map(|r| row(&terminal, r)).collect();
        assert!(text.contains("Sort by"), "{text}");
        assert!(text.contains("Largest first"), "{text}");
        // Every option shows its number so the 1–6 keys match the labels.
        assert!(text.contains("1. Name (A \u{2192} Z)"), "{text}");
        assert!(text.contains("2. Name (Z \u{2192} A)"), "{text}");
        assert!(text.contains("3. Oldest first"), "{text}");
        assert!(text.contains("4. Newest first"), "{text}");
        assert!(text.contains("5. Smallest first"), "{text}");
        assert!(text.contains("6. Largest first"), "{text}");
        assert!(text.contains("[1\u{2013}6] pick"), "{text}");

        for order in OrderBy::ALL {
            assert!(
                zones.iter().any(|(_, a)| *a == MouseAction::SortBy(order)),
                "missing SortBy zone for {order:?}"
            );
        }
        assert!(zones.iter().any(|(_, a)| *a == MouseAction::Confirm));
        assert!(zones.iter().any(|(_, a)| *a == MouseAction::Cancel));
    }

    #[test]
    fn help_overlay_lists_keys_and_closes_on_click() {
        let mut state = AppState::new(PathBuf::from("/test"));
        state.build_tree(two_project_output());
        state.phase = AppPhase::Help;

        // Standard 24-row terminal: fits the full help without clipping.
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let mut zones = MouseZones::new();
        terminal
            .draw(|f| {
                zones = render(&state, f);
            })
            .unwrap();

        let text: String = (0..24u16).map(|r| row(&terminal, r)).collect();
        assert!(text.contains("Help"), "{text}");
        assert!(text.contains("Navigate"), "{text}");
        assert!(text.contains("Move the cursor"), "{text}");
        assert!(text.contains("First or last directory"), "{text}");
        assert!(text.contains("Jump a page at a time"), "{text}");
        assert!(text.contains("Toggle the selected row"), "{text}");
        assert!(text.contains("Delete what you selected"), "{text}");
        assert!(text.contains("Sort the list"), "{text}");

        // Any click on the overlay closes it.
        assert!(zones.iter().any(|(_, a)| *a == MouseAction::Dismiss));
    }

    #[test]
    fn button_bar_truncates_on_narrow_terminal() {
        let mut state = AppState::new(PathBuf::from("/test"));
        state.build_tree(two_project_output());
        state.phase = AppPhase::Browsing;

        let mut terminal = Terminal::new(TestBackend::new(40, 10)).unwrap();
        let mut zones = MouseZones::new();
        terminal.draw(|f| zones = render(&state, f)).unwrap();

        // Buttons that don't fit are dropped from the right, never wrapped
        // or clipped.
        let bar = row(&terminal, 9);
        assert!(bar.contains("[Space] Toggle"), "{bar}");
        assert!(bar.contains("[d] None"), "{bar}");
        assert!(!bar.contains("Delete"), "{bar}");
        assert!(!bar.contains("Quit"), "{bar}");

        // Every zone stays inside the 40-column frame.
        for (rect, _) in &zones {
            assert!(rect.y < 10 && rect.x + rect.width <= 40, "zone {rect:?}");
        }
    }
}
