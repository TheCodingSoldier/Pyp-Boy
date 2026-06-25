use reverse_geocoder::ReverseGeocoder;
use chrono::prelude::*;
use serde::{Deserialize, Serialize};
use std::io;
use ratatui::prelude::*;
use crossterm::{
    event::{self, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use std::fs;
use thiserror::Error;
use rand::prelude::*;

use crate::kb;
use kb::{show_virtual_keyboard, centered_rect};

const DB_PATH: &str = "./data/db.json";

use ratatui::widgets::{
    Block, BorderType, Borders, Cell, List, ListItem, ListState, Paragraph, Row, Table, Wrap,
};
use ratatui::layout::{Alignment, Constraint};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Span, Line, Text};

// ─── Item ─────────────────────────────────────────────────────────────────────
#[derive(Serialize, Deserialize, Clone)]
pub struct Item {
    pub id: usize,
    pub name: String,
    pub details: String,
    pub quantity: u32,
    pub category: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Error, Debug)]
pub enum Error {
    #[error("error reading the DB file: {0}")]
    ReadDBError(#[from] io::Error),
    #[error("error parsing the DB file: {0}")]
    ParseDBError(#[from] serde_json::Error),
}

// ─── Map ──────────────────────────────────────────────────────────────────────
pub fn get_map_data(coordinates: [f64; 2]) -> String {
    let geocoder = ReverseGeocoder::new();
    let coord_tuple = (coordinates[0], coordinates[1]);
    let search_result = geocoder.search(coord_tuple);

    // Try timezone lookup — gracefully ignore if it fails
    let tz_str = spatialtime::osm::lookup(coordinates[0], coordinates[1])
        .map(|st| format!("{:?}", st.tzid))
        .unwrap_or_else(|_| "Unknown".to_string());

    format!(
        "Latitude  : {:.6}°\nLongitude : {:.6}°\n\nLocation  : {}, {}, {}\nCountry   : {}\nTime Zone : {}",
        coordinates[0],
        coordinates[1],
        search_result.record.name,
        search_result.record.admin1,
        search_result.record.admin2,
        search_result.record.cc,
        tz_str,
    )
}

pub fn render_map<'a>(map_text: Option<String>) -> Paragraph<'a> {
    let text_content: std::borrow::Cow<'a, str> = match map_text {
        Some(s) => std::borrow::Cow::Owned(s),
        None    => std::borrow::Cow::Borrowed(">> Acquiring GPS signal..."),
    };
    Paragraph::new(Text::styled(text_content, Style::default().add_modifier(Modifier::BOLD)))
        .alignment(Alignment::Left)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("MAP > LOCATION")
                .border_type(BorderType::Plain),
        )
}

// ─── Radio ────────────────────────────────────────────────────────────────────
/// Render the Radio tab with frequency display and a simulated signal bar.
pub fn render_radio<'a>(freq_mhz: f64) -> Paragraph<'a> {
    // Simulate signal strength — in a real build shell out to rtl_power
    let signal_pct = simulate_signal_strength(freq_mhz);
    let bar_len = (signal_pct / 5) as usize; // 0–20 blocks
    let bar: String = "█".repeat(bar_len) + &"░".repeat(20 - bar_len);

    let lines = vec![
        Line::from(vec![Span::styled("── RTL-SDR RECEIVER ──", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))]),
        Line::from(""),
        Line::from(vec![
            Span::styled("Frequency : ", Style::default().fg(Color::Green)),
            Span::styled(
                format!("{:.1} MHz FM", freq_mhz),
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("Signal    : ", Style::default().fg(Color::Green)),
            Span::styled(bar, Style::default().fg(Color::LightGreen)),
            Span::styled(format!(" {}%", signal_pct), Style::default().fg(Color::White)),
        ]),
        Line::from(""),
        Line::from(vec![Span::styled(
            "[ ] tune down  |  [ ] tune up  |  [ tuned to VaultTec FM ]",
            Style::default().fg(Color::DarkGray),
        )]),
        Line::from(""),
        Line::from(vec![Span::styled(
            "Use [ and ] keys to tune frequency (±0.1 MHz)",
            Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
        )]),
    ];

    Paragraph::new(lines)
        .alignment(Alignment::Left)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .style(Style::default().fg(Color::White))
                .title("RADIO")
                .border_type(BorderType::Plain),
        )
}

fn simulate_signal_strength(freq: f64) -> u8 {
    // Strong stations at common FM frequencies
    let strong: &[(f64, u8)] = &[
        (88.1, 85), (91.5, 72), (94.1, 90), (98.7, 78),
        (100.1, 95), (102.3, 65), (105.5, 80), (107.9, 70),
    ];
    for &(f, s) in strong {
        if (freq - f).abs() < 0.15 { return s; }
    }
    // Background noise
    15
}

// ─── Inventory ────────────────────────────────────────────────────────────────
pub fn render_inv<'a>(inv_list_state: &ListState, category_filter: &'a str) -> (List<'a>, Paragraph<'a>) {
    let inv_list = read_db().expect("can fetch item list");

    let mut filtered: Vec<Item> = inv_list
        .into_iter()
        .filter(|item| item.category.eq_ignore_ascii_case(category_filter))
        .collect();
    filtered.sort_by(|a, b| b.created_at.cmp(&a.created_at));

    let mut items: Vec<_> = filtered
        .iter()
        .map(|item| {
            ListItem::new(Line::from(Span::styled(
                format!("{}  x{}", item.name, item.quantity),
                Style::default(),
            )))
        })
        .collect();

    items.push(ListItem::new(Line::from(Span::styled(
        "+ Add New",
        Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
    ))));

    let selected_item = filtered
        .get(inv_list_state.selected().unwrap_or(0))
        .cloned()
        .unwrap_or(Item {
            id: 0,
            name: "ERR".into(),
            details: "ERR".into(),
            quantity: 0,
            category: category_filter.into(),
            created_at: Utc::now(),
        });

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(category_filter)
                .border_type(BorderType::Plain),
        )
        .highlight_style(
            Style::default()
                .bg(Color::Yellow)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD),
        );

    let detail_lines = vec![
        Line::from(Span::styled(
            format!("Name: {}", selected_item.name),
            Style::default().fg(Color::LightBlue).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::raw(format!("Added: {}", selected_item.created_at.format("%Y-%m-%d %H:%M")))),
        Line::from(""),
        Line::from(Span::styled("Details:", Style::default().fg(Color::LightBlue))),
        Line::from(Span::raw(selected_item.details.clone())),
        Line::from(""),
        Line::from(Span::styled("Quantity:", Style::default().fg(Color::LightBlue))),
        Line::from(Span::raw(format!("x{}", selected_item.quantity))),
    ];

    let paragraph = Paragraph::new(detail_lines)
        .block(
            Block::default()
                .title("Item Detail")
                .borders(Borders::ALL)
                .border_type(BorderType::Plain),
        )
        .wrap(Wrap { trim: true });

    (list, paragraph)
}

// ─── Add item ─────────────────────────────────────────────────────────────────
pub fn add_item_to_db() -> Result<Vec<Item>, Error> {
    let stdout = io::stdout();
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let mut rng = rand::thread_rng();
    let db_content = fs::read_to_string(DB_PATH)?;
    let mut parsed: Vec<Item> = serde_json::from_str(&db_content)?;

    let name     = show_virtual_keyboard(&mut terminal, "Item Name")
        .map_err(|e| Error::ReadDBError(io::Error::new(io::ErrorKind::Other, e.to_string())))?;
    let category = show_category_selector(&mut terminal)
        .map_err(|e| Error::ReadDBError(io::Error::new(io::ErrorKind::Other, e.to_string())))?;
    let details  = show_virtual_keyboard(&mut terminal, "Item Details")
        .map_err(|e| Error::ReadDBError(io::Error::new(io::ErrorKind::Other, e.to_string())))?;
    let quantity = show_quantity_selector(&mut terminal, 1)
        .map_err(|e| Error::ReadDBError(io::Error::new(io::ErrorKind::Other, e.to_string())))?;

    parsed.push(Item {
        id: rng.gen_range(0, 9_999_999),
        name,
        details,
        quantity,
        category,
        created_at: Utc::now(),
    });
    fs::write(DB_PATH, &serde_json::to_vec(&parsed)?)?;
    Ok(parsed)
}

// ─── Selectors ────────────────────────────────────────────────────────────────
pub fn show_category_selector<B: Backend>(terminal: &mut Terminal<B>) -> io::Result<String> {
    let categories = vec!["Weapons", "Apparel", "Aid", "Misc", "Junk", "Mods", "Ammo"];
    let mut state = ListState::default();
    state.select(Some(0));

    loop {
        terminal.draw(|f| {
            let size = centered_rect(70, 50, f.area());
            let items: Vec<ListItem> = categories.iter().map(|c| ListItem::new(*c)).collect();
            let list = List::new(items)
                .block(Block::default().borders(Borders::ALL).title("Select Category"))
                .highlight_style(
                    Style::default().bg(Color::LightBlue).fg(Color::Black).add_modifier(Modifier::BOLD),
                );
            f.render_stateful_widget(list, size, &mut state);
        })?;

        if let Event::Key(key) = event::read().map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))? {
            match key.code {
                KeyCode::Up | KeyCode::Char('w') | KeyCode::Char('W') => {
                    let i = state.selected().map(|i| if i > 0 { i - 1 } else { 0 }).unwrap_or(0);
                    state.select(Some(i));
                }
                KeyCode::Down | KeyCode::Char('s') | KeyCode::Char('S') => {
                    let i = state.selected().map(|i| (i + 1).min(categories.len() - 1)).unwrap_or(0);
                    state.select(Some(i));
                }
                KeyCode::Enter => {
                    if let Some(i) = state.selected() {
                        return Ok(categories[i].to_string());
                    }
                }
                KeyCode::Esc => {
                    return Err(io::Error::new(io::ErrorKind::Interrupted, "Selection cancelled"));
                }
                _ => {}
            }
        }
    }
}

pub fn show_quantity_selector<B: Backend>(terminal: &mut Terminal<B>, initial: u32) -> io::Result<u32> {
    let entries: Vec<String> = (0u32..=999).map(|i| i.to_string()).collect();
    let mut state = ListState::default();
    state.select(Some((initial as usize).min(entries.len() - 1)));

    loop {
        terminal.draw(|f| {
            let size = centered_rect(40, 60, f.area());
            let items: Vec<ListItem> = entries.iter().map(|c| ListItem::new(c.as_str())).collect();
            let list = List::new(items)
                .block(Block::default().borders(Borders::ALL).title("Select Quantity (0 = delete)"))
                .highlight_style(
                    Style::default().bg(Color::LightBlue).fg(Color::Black).add_modifier(Modifier::BOLD),
                );
            f.render_stateful_widget(list, size, &mut state);
        })?;

        if let Event::Key(key) = event::read().map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))? {
            match key.code {
                KeyCode::Up | KeyCode::Char('w') | KeyCode::Char('W') => {
                    let i = state.selected().map(|i| if i > 0 { i - 1 } else { entries.len() - 1 }).unwrap_or(0);
                    state.select(Some(i));
                }
                KeyCode::Down | KeyCode::Char('s') | KeyCode::Char('S') => {
                    let i = state.selected().map(|i| (i + 1) % entries.len()).unwrap_or(0);
                    state.select(Some(i));
                }
                KeyCode::Enter => {
                    if let Some(i) = state.selected() {
                        return entries[i].parse::<u32>()
                            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()));
                    }
                }
                KeyCode::Esc => {
                    return Err(io::Error::new(io::ErrorKind::Interrupted, "Cancelled"));
                }
                _ => {}
            }
        }
    }
}

// ─── Misc render helpers ──────────────────────────────────────────────────────
/// Legacy export kept for compatibility
pub fn render_data<'a>() -> Paragraph<'a> {
    Paragraph::new("Welcome to Pyp-Boy")
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL).title("Home"))
}

pub fn read_db() -> Result<Vec<Item>, Error> {
    let db_content = fs::read_to_string(DB_PATH)?;
    let parsed: Vec<Item> = serde_json::from_str(&db_content)?;
    Ok(parsed)
}

/// Deprecated — kept so any external callers don't break
pub fn render_stat<'a>() -> Paragraph<'a> {
    Paragraph::new("Use STAT tab").block(Block::default().title("Radio").borders(Borders::ALL))
}
