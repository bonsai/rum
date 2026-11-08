use crate::app::{App, Mode, Source};
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
    // 0: Main editor area (Sidebar + Editor)
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

    // Split main_area into 3 panes: Sidebar (25%), Editor (50%), Notes/Memos (25%)
    let h_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(25), Constraint::Percentage(50), Constraint::Percentage(25)])
        .split(main_area);

    let sidebar_area = h_chunks[0];
    let editor_area = h_chunks[1];
    let notes_area = h_chunks[2];

    render_sidebar(frame, app, sidebar_area);
    render_editor(frame, app, editor_area);
    render_notes(frame, app, notes_area);
    render_status(frame, app, status_area);
    render_command(frame, app, command_area);
}

fn render_notes(frame: &mut Frame, app: &App, area: Rect) {
    let title = match &app.source {
        Source::Db(_) => format!(" Notes ({}) ", app.notes.len()),
        Source::Dir(_) => format!(" Memos ({}) ", app.files.len()),
    };

    let items: Vec<ListItem> = match &app.source {
        Source::Db(_) => app
            .notes
            .iter()
            .enumerate()
            .map(|(i, n)| {
                let is_selected = i == app.note_index;
                let style = if is_selected {
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };
                let name = if n.title.trim().is_empty() { n.id.clone() } else { n.title.clone() };
                ListItem::new(format!("{}{}", if is_selected { "▶ " } else { "  " }, name))
                    .style(style)
            })
            .collect(),
        Source::Dir(_) => app
            .files
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let is_selected = i == app.file_index;
                let style = if is_selected {
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };
                let name = p
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| "unnamed".to_string());
                ListItem::new(format!("{}{}", if is_selected { "▶ " } else { "  " }, name))
                    .style(style)
            })
            .collect(),
    };

    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .title(title)
            .border_style(Style::default().fg(Color::Green)),
    );

    frame.render_widget(list, area);
}

fn render_sidebar(frame: &mut Frame, app: &App, area: Rect) {
    let (names, selected, title): (Vec<String>, usize, String) = match &app.source {
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
            format!(" Files ({}) ", app.files.len()),
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
            format!(" Notes ({}) ", app.notes.len()),
        ),
    };

    let items: Vec<ListItem> = names
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let is_selected = i == selected;
            let style = if is_selected {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };

            let prefix = if is_selected { "▶ " } else { "  " };
            ListItem::new(format!("{}{}", prefix, name)).style(style)
        })
        .collect();
    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .title(title)
            .border_style(Style::default().fg(Color::Cyan)),
    );

    frame.render_widget(list, area);
}

fn render_editor(frame: &mut Frame, app: &mut App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Blue));
    let inner_area = block.inner(area);
    frame.render_widget(block, area);

    if inner_area.height == 0 || inner_area.width == 0 {
        return;
    }

    let line_count = app.buffer.line_count();
    let line_num_width = format!("{}", line_count.max(1)).len() + 1;

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
        if actual_row < line_count {
            let line_text = &app.buffer.lines[actual_row];
            let num_str = format!("{:>width$} ", actual_row + 1, width = line_num_width - 1);

            let mut spans = vec![Span::styled(num_str, Style::default().fg(Color::DarkGray))];

            // Colorize visual selection or search matches
            if let Some((v_start, v_end)) = visual_range {
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

            lines_to_render.push(Line::from(spans));
        } else {
            // Tilde for lines past EOF
            lines_to_render.push(Line::from(vec![Span::styled(
                "~",
                Style::default().fg(Color::DarkGray),
            )]));
        }
    }

    let paragraph = Paragraph::new(lines_to_render);
    frame.render_widget(paragraph, inner_area);

    // Set cursor position in editor
    if app.mode == Mode::Normal || app.mode == Mode::Insert || app.mode == Mode::Visual {
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

    let left = format!(" {} {}{}{}", mode_str, file_name, mod_flag, fm_info);

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
