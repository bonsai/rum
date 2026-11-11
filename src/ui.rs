use crate::app::{App, Mode, Pane, Source};
use crate::buffer::MAX_SHEET_LINES;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Frame,
};

pub fn render(frame: &mut Frame, app: &mut App) {
    let size = frame.area();

    // Vertical layout:
    // 0: Main 3-pane area (BOX + 紙 + エディタ)
    // 1: Status line (1 row)
    // 2: Command/Search/Echo line (1 row)
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(size);

    let main_area = chunks[0];
    let status_area = chunks[1];
    let command_area = chunks[2];

    // Split main_area horizontally into 3 panes:
    // BOX (18%), 紙/シート (24%), エディタ (58%)
    let h_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(18),
            Constraint::Percentage(24),
            Constraint::Percentage(58),
        ])
        .split(main_area);

    let box_area = h_chunks[0];
    let sheet_area = h_chunks[1];
    let editor_area = h_chunks[2];

    render_box_pane(frame, app, box_area);
    render_sheet_pane(frame, app, sheet_area);
    render_editor(frame, app, editor_area);
    render_status(frame, app, status_area);
    render_command(frame, app, command_area);
}

fn render_box_pane(frame: &mut Frame, app: &App, area: Rect) {
    let is_focused = app.focused_pane == Pane::Box;
    let border_color = if is_focused {
        Color::Yellow
    } else {
        Color::DarkGray
    };

    let items: Vec<ListItem> = app
        .boxes
        .iter()
        .enumerate()
        .map(|(i, b)| {
            let is_selected = i == app.box_index;
            let style = if is_selected {
                Style::default()
                    .fg(if is_focused {
                        Color::Yellow
                    } else {
                        Color::Cyan
                    })
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };

            let prefix = if is_selected { "▶ " } else { "  " };
            let tag = format!("[{}] ", b.source);
            ListItem::new(format!("{}{}{}", prefix, tag, b.name)).style(style)
        })
        .collect();

    let title = if is_focused {
        format!(" [BOX ({})] ", app.boxes.len())
    } else {
        format!(" BOX ({}) ", app.boxes.len())
    };

    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .title(title)
            .border_style(Style::default().fg(border_color)),
    );

    frame.render_widget(list, area);
}

fn render_sheet_pane(frame: &mut Frame, app: &App, area: Rect) {
    let is_focused = app.focused_pane == Pane::Sheet;
    let border_color = if is_focused {
        Color::Yellow
    } else {
        Color::DarkGray
    };

    let (names, selected, count_str): (Vec<String>, usize, String) = match &app.source {
        Source::Dir(_) => (
            app.files
                .iter()
                .map(|p| {
                    p.file_name()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_else(|| "unnamed".to_string())
                })
                .collect(),
            app.file_index,
            format!("{}", app.files.len()),
        ),
        Source::Db(_) => (
            app.notes
                .iter()
                .map(|n| {
                    if n.title.trim().is_empty() {
                        n.id.clone()
                    } else {
                        n.title.clone()
                    }
                })
                .collect(),
            app.note_index,
            format!("{}", app.notes.len()),
        ),
    };

    let items: Vec<ListItem> = names
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let is_selected = i == selected;
            let style = if is_selected {
                Style::default()
                    .fg(if is_focused {
                        Color::Yellow
                    } else {
                        Color::Cyan
                    })
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };

            let prefix = if is_selected { "▶ " } else { "  " };
            ListItem::new(format!("{}{}", prefix, name)).style(style)
        })
        .collect();

    let title = if is_focused {
        format!(" [紙 ({})] ", count_str)
    } else {
        format!(" 紙 ({}) ", count_str)
    };

    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .title(title)
            .border_style(Style::default().fg(border_color)),
    );

    frame.render_widget(list, area);
}

fn render_editor(frame: &mut Frame, app: &mut App, area: Rect) {
    let is_focused = app.focused_pane == Pane::Editor;
    let border_color = if is_focused {
        Color::Yellow
    } else {
        Color::Blue
    };

    let title = if is_focused {
        format!(
            " [エディタ (24行の紙{})] ",
            if app.ura_enabled {
                " / 裏写り:ON"
            } else {
                ""
            }
        )
    } else {
        format!(
            " エディタ (24行の紙{}) ",
            if app.ura_enabled {
                " / 裏写り:ON"
            } else {
                ""
            }
        )
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(border_color));
    let inner_area = block.inner(area);
    frame.render_widget(block, area);

    if inner_area.height == 0 || inner_area.width == 0 {
        return;
    }

    let line_count = app.buffer.line_count();
    let line_num_width = 3; // 行番号 1..24

    // Adjust viewport vertical scroll
    let cursor_row = app.buffer.cursor.row;
    let visible_rows = inner_area.height as usize;

    if cursor_row < app.viewport_top {
        app.viewport_top = cursor_row;
    } else if cursor_row >= app.viewport_top + visible_rows {
        app.viewport_top = cursor_row - visible_rows + 1;
    }

    // Determine visual selection range if in Visual mode
    let visual_range = if app.mode == Mode::Visual {
        let start = app.visual_start.unwrap_or(app.buffer.cursor);
        let end = app.buffer.cursor;
        if start.row < end.row || (start.row == end.row && start.col <= end.col) {
            Some((start, end))
        } else {
            Some((end, start))
        }
    } else {
        None
    };

    let mut lines_to_render = Vec::new();

    for row_idx in 0..visible_rows {
        let actual_row = app.viewport_top + row_idx;
        if actual_row < MAX_SHEET_LINES {
            let num_str = format!("{:>2} ", actual_row + 1);

            let mut spans = vec![Span::styled(num_str, Style::default().fg(Color::DarkGray))];

            if actual_row < line_count {
                let line_text = &app.buffer.lines[actual_row];

                // Check for bleed-through (裏写り):
                // 「現在の紙の空行にだけ、直前の紙（前の面）の行を淡く重ねる。入力の邪魔をしない（非空行には出さない）」
                let is_empty_line = line_text.trim().is_empty();
                let bleed_through = if app.ura_enabled && is_empty_line {
                    app.prev_doc_lines
                        .as_ref()
                        .and_then(|lines| lines.get(actual_row))
                        .filter(|s| !s.trim().is_empty())
                } else {
                    None
                };

                if let Some(bleed_text) = bleed_through {
                    // うっすら透ける淡い演出（DarkGray / Faint）
                    spans.push(Span::styled(
                        bleed_text.clone(),
                        Style::default()
                            .fg(Color::Rgb(80, 80, 80))
                            .add_modifier(Modifier::DIM),
                    ));
                } else if let Some((v_start, v_end)) = visual_range {
                    // Visual selection
                    if actual_row >= v_start.row && actual_row <= v_end.row {
                        let chars: Vec<char> = line_text.chars().collect();
                        let sel_start = if actual_row == v_start.row {
                            v_start.col
                        } else {
                            0
                        };
                        let sel_end = if actual_row == v_end.row {
                            v_end.col
                        } else {
                            chars.len().saturating_sub(1)
                        };

                        let before: String = chars[..sel_start.min(chars.len())].iter().collect();
                        let selected: String = if sel_start <= sel_end && sel_start < chars.len() {
                            chars[sel_start..=sel_end.min(chars.len() - 1)]
                                .iter()
                                .collect()
                        } else {
                            String::new()
                        };
                        let after: String = if sel_end + 1 < chars.len() {
                            chars[sel_end + 1..].iter().collect()
                        } else {
                            String::new()
                        };

                        spans.push(Span::raw(before));
                        spans.push(Span::styled(
                            selected,
                            Style::default().bg(Color::DarkGray).fg(Color::White),
                        ));
                        spans.push(Span::raw(after));
                    } else {
                        spans.push(Span::raw(line_text.clone()));
                    }
                } else {
                    spans.push(Span::raw(line_text.clone()));
                }
            } else {
                // Within 24 lines of sheet, but empty after current buffer lines
                let bleed_through = if app.ura_enabled {
                    app.prev_doc_lines
                        .as_ref()
                        .and_then(|lines| lines.get(actual_row))
                        .filter(|s| !s.trim().is_empty())
                } else {
                    None
                };
                if let Some(bleed_text) = bleed_through {
                    spans.push(Span::styled(
                        bleed_text.clone(),
                        Style::default()
                            .fg(Color::Rgb(80, 80, 80))
                            .add_modifier(Modifier::DIM),
                    ));
                }
            }

            lines_to_render.push(Line::from(spans));
        } else {
            // Lines past 24: sheet boundary, show ~
            lines_to_render.push(Line::from(vec![Span::styled(
                "~",
                Style::default().fg(Color::DarkGray),
            )]));
        }
    }

    let paragraph = Paragraph::new(lines_to_render);
    frame.render_widget(paragraph, inner_area);

    // Set cursor position in editor
    if is_focused
        && (app.mode == Mode::Normal || app.mode == Mode::Insert || app.mode == Mode::Visual)
    {
        let cursor_screen_y = inner_area.y + (cursor_row - app.viewport_top) as u16;
        let visual_col = app.buffer.visual_col(4);
        let cursor_screen_x = inner_area.x + line_num_width as u16 + visual_col as u16;

        if cursor_screen_x < inner_area.x + inner_area.width
            && cursor_screen_y < inner_area.y + inner_area.height
        {
            frame.set_cursor_position((cursor_screen_x, cursor_screen_y));
        }
    }
}

fn render_status(frame: &mut Frame, app: &App, area: Rect) {
    let mode_str = match app.mode {
        Mode::Normal => "-- NORMAL --",
        Mode::Insert => "-- INSERT --",
        Mode::Visual => "-- VISUAL --",
        Mode::Command => "-- COMMAND --",
        Mode::Search => "-- SEARCH --",
    };

    let file_name = app
        .doc
        .path
        .as_ref()
        .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
        .or_else(|| {
            app.doc
                .front_matter
                .as_ref()
                .map(|f| f.title.clone())
                .filter(|t| !t.is_empty())
        })
        .unwrap_or_else(|| "[No Name]".to_string());

    let mod_flag = if app.doc.modified { " [+]" } else { "" };

    let pos_str = format!(
        "{}:{}   {}%",
        app.buffer.cursor.row + 1,
        app.buffer.cursor.col + 1,
        if app.buffer.line_count() > 0 {
            ((app.buffer.cursor.row + 1) * 100) / app.buffer.line_count()
        } else {
            100
        }
    );

    let fm_info = if let Some(fm) = &app.doc.front_matter {
        format!(" [id: {} | {}]", &fm.id, &fm.title)
    } else {
        String::new()
    };

    let box_info = app
        .boxes
        .get(app.box_index)
        .map(|b| format!(" [{}]", b.name))
        .unwrap_or_default();
    let ura_info = if app.ura_enabled { " [ura]" } else { "" };

    let left = format!(
        " {} {}{}{}{}{}",
        mode_str, file_name, mod_flag, box_info, ura_info, fm_info
    );

    let spaces = area
        .width
        .saturating_sub(left.len() as u16 + pos_str.len() as u16 + 2);
    let fill = " ".repeat(spaces as usize);

    let status_line = Line::from(vec![
        Span::styled(
            left,
            Style::default()
                .fg(Color::Black)
                .bg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(fill, Style::default().bg(Color::White)),
        Span::styled(pos_str, Style::default().fg(Color::Black).bg(Color::White)),
    ]);

    frame.render_widget(Paragraph::new(status_line), area);
}

fn render_command(frame: &mut Frame, app: &App, area: Rect) {
    let mut spans = Vec::new();

    match app.mode {
        Mode::Command => {
            spans.push(Span::styled(":", Style::default().fg(Color::Yellow)));
            spans.push(Span::raw(&app.command_input));
            frame.set_cursor_position((area.x + 1 + app.command_cursor as u16, area.y));
        }
        Mode::Search => {
            spans.push(Span::styled("/", Style::default().fg(Color::Yellow)));
            spans.push(Span::raw(&app.search_input));
            frame.set_cursor_position((area.x + 1 + app.search_cursor as u16, area.y));
        }
        _ => {
            if let Some(msg) = &app.status_message {
                spans.push(Span::styled(msg, Style::default().fg(Color::White)));
            }
        }
    }

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}
