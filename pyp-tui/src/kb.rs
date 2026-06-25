use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Paragraph, Table, Row, Clear},
};
use ratatui::widgets::Cell;
use crossterm::{
    event::{self, Event, KeyCode},
};
use std::io;
use std::time::{Duration, Instant};

pub fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints(
            [
                Constraint::Percentage((100 - percent_y) / 2),
                Constraint::Percentage(percent_y),
                Constraint::Percentage((100 - percent_y) / 2),
            ]
            .as_ref(),
        )
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints(
            [
                Constraint::Percentage((100 - percent_x) / 2),
                Constraint::Percentage(percent_x),
                Constraint::Percentage((100 - percent_x) / 2),
            ]
            .as_ref(),
        )
        .split(popup_layout[1])[1]
}

pub fn show_virtual_keyboard(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    kb_title: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let mut input = String::new();
    let keyboard_layout = vec![
        vec!['A', 'B', 'C', 'D', 'E', 'F', 'G', 'H', 'I', 'J'],
        vec!['K', 'L', 'M', 'N', 'O', 'P', 'Q', 'R', 'S', 'T'],
        vec!['U', 'V', 'W', 'X', 'Y', 'Z', '0', '1', '2', '3'],
        vec!['4', '5', '6', '7', '8', '9', ' ', '←', '✓', ' '],
    ];

    let mut cursor_pos = (0usize, 0usize);

    loop {
        terminal.clear();
        terminal.draw(|f| {
            let outer_area = f.size();

            // Input preview box above keyboard
            let layout = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Percentage(20),
                    Constraint::Length(3),
                    Constraint::Percentage(50),
                    Constraint::Length(3),
                ].as_ref())
                .split(outer_area);

            let preview = Paragraph::new(format!(" {} ", input))
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(format!(" {} ", kb_title))
                        .style(Style::default().fg(Color::Green))
                )
                .style(Style::default().fg(Color::LightCyan).add_modifier(Modifier::BOLD));
            f.render_widget(Clear, layout[1]);
            f.render_widget(preview, layout[1]);

            let area = centered_rect(80, 60, outer_area);
            f.render_widget(Clear, area);

            let rows: Vec<Row> = keyboard_layout
                .iter()
                .enumerate()
                .map(|(y, row)| {
                    Row::new(
                        row.iter()
                            .enumerate()
                            .map(|(x, &ch)| {
                                let content = match ch {
                                    '←' => "[<]".to_string(),
                                    '✓' => "[OK]".to_string(),
                                    _ => ch.to_string(),
                                };

                                if (y, x) == cursor_pos {
                                    Cell::from(Span::styled(
                                        format!(">{}<", content),
                                        Style::default()
                                            .fg(Color::Black)
                                            .bg(Color::Green)
                                            .add_modifier(Modifier::BOLD),
                                    ))
                                } else {
                                    Cell::from(Span::styled(
                                        format!(" {} ", content),
                                        Style::default().fg(Color::White),
                                    ))
                                }
                            })
                            .collect::<Vec<_>>(),
                    )
                })
                .collect();

            let max_cols = keyboard_layout.iter().map(|row| row.len()).max().unwrap_or(1);
            let widths: Vec<Constraint> = vec![Constraint::Length(6); max_cols];

            let table = Table::new(rows, widths)
                .block(
                    Block::default()
                        .title(" KEYBOARD - W/A/S/D: move | Enter: select | ESC: cancel ")
                        .borders(Borders::ALL)
                        .style(Style::default().fg(Color::Green))
                );

            f.render_widget(table, area);

            // Footer hint
            let hint = Paragraph::new(" W/S: row up/down    A/D: col left/right    Enter: type    [<]: backspace    [OK]: done ")
                .style(Style::default().fg(Color::DarkGray))
                .alignment(Alignment::Center);
            f.render_widget(hint, layout[3]);
        })?;

        if let Event::Key(key) = event::read().map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))? {
            match key.code {
                KeyCode::Char('A') | KeyCode::Char('a') | KeyCode::Left => {
                    if cursor_pos.1 > 0 { cursor_pos.1 -= 1; }
                }
                KeyCode::Char('D') | KeyCode::Char('d') | KeyCode::Right => {
                    if cursor_pos.1 < keyboard_layout[cursor_pos.0].len() - 1 {
                        cursor_pos.1 += 1;
                    }
                }
                KeyCode::Char('W') | KeyCode::Char('w') | KeyCode::Up => {
                    if cursor_pos.0 > 0 {
                        cursor_pos.0 -= 1;
                        if cursor_pos.1 >= keyboard_layout[cursor_pos.0].len() {
                            cursor_pos.1 = keyboard_layout[cursor_pos.0].len() - 1;
                        }
                    }
                }
                KeyCode::Char('S') | KeyCode::Char('s') | KeyCode::Down => {
                    if cursor_pos.0 < keyboard_layout.len() - 1 {
                        cursor_pos.0 += 1;
                        if cursor_pos.1 >= keyboard_layout[cursor_pos.0].len() {
                            cursor_pos.1 = keyboard_layout[cursor_pos.0].len() - 1;
                        }
                    }
                }
                KeyCode::Enter => {
                    let ch = keyboard_layout[cursor_pos.0][cursor_pos.1];
                    match ch {
                        '←' => { input.pop(); }
                        '✓' => return Ok(input),
                        ' ' if cursor_pos == (3, 6) => input.push(' '),
                        ' ' => {}
                        _ => input.push(ch),
                    }
                }
                KeyCode::Backspace => { input.pop(); }
                KeyCode::Esc => {
                    return Err(Box::new(io::Error::new(io::ErrorKind::Interrupted, "Keyboard cancelled")));
                }
                _ => {}
            }
        }
    }
}
