use reverse_geocoder::ReverseGeocoder;
use chrono::prelude::*;
use serde::{Deserialize, Serialize};
use std::io;
use std::time::{Duration, Instant};
use ratatui::prelude::*;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use std::fs;
use thiserror::Error;
use rand::{distributions::Alphanumeric, prelude::*};

use crate::kb;
use kb::{show_virtual_keyboard, centered_rect};

const DB_PATH: &str = "./data/db.json";

use ratatui::widgets::{
    Block, BorderType, Borders, Cell, List, ListItem, ListState, Paragraph, Row, Table, Wrap, Gauge,
};
use ratatui::layout::{Alignment, Constraint};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Span, Line, Text};

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

pub fn get_map_data<'a>(coordinates: [f64; 2]) -> String {
    let geocoder = ReverseGeocoder::new();
    let spatial_time = spatialtime::osm::lookup(coordinates[0], coordinates[1]).unwrap();
    let coord_tuple: (f64, f64) = (coordinates[0], coordinates[1]);
    let search_result = geocoder.search(coord_tuple);
    format!(
        "Latitude: {}\nLongitude: {}\nAddress: {}, {}, {}, {}\nTime Zone: {:?}",
        coordinates[0],
        coordinates[1],
        search_result.record.name,
        search_result.record.admin1,
        search_result.record.admin2,
        search_result.record.cc,
        spatial_time.tzid
    )
}

pub fn render_map<'a>(map_text: Option<String>) -> Paragraph<'a> {
    let bold_style = Style::default().add_modifier(Modifier::BOLD);
    let text_content: std::borrow::Cow<'a, str> = match map_text {
        Some(s) => std::borrow::Cow::Owned(s),
        None => std::borrow::Cow::Borrowed("Map data not available"),
    };

    let map_text_finished = Text::styled(text_content, bold_style);

    Paragraph::new(map_text_finished)
        .alignment(Alignment::Left)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" [ MAP ] ")
                .border_type(BorderType::Plain),
        )
}

// ─────────────────────────────────────────────
// STAT SUBMENUS
// ─────────────────────────────────────────────

pub fn render_stat_general<'a>(
    coords: [f64; 2],
    map_text: Option<String>,
    uptime_start: Instant,
) -> Paragraph<'a> {
    let now = Local::now();
    let uptime_secs = uptime_start.elapsed().as_secs();
    let uptime_str = format!("{}h {}m {}s", uptime_secs / 3600, (uptime_secs % 3600) / 60, uptime_secs % 60);

    let gps_status = if coords[0] != 0.0 || coords[1] != 0.0 {
        format!("GPS: {:.4}°N  {:.4}°W", coords[0], coords[1].abs())
    } else {
        "GPS: SEARCHING...".to_string()
    };

    let location_str = match map_text {
        Some(ref text) => {
            let addr_line = text.lines()
                .find(|l| l.starts_with("Address:"))
                .unwrap_or("Address: Unknown")
                .to_string();
            addr_line
        }
        None => "Location: Unknown".to_string(),
    };

    let lines = vec![
        Line::from(vec![
            Span::styled(" DATE/TIME  ", Style::default().fg(Color::Black).bg(Color::Green).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(Span::styled(
            format!(" {} ", now.format("%a %Y-%m-%d  %H:%M:%S")),
            Style::default().fg(Color::LightCyan),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled(" LOCATION   ", Style::default().fg(Color::Black).bg(Color::Green).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(Span::styled(
            format!(" {} ", gps_status),
            Style::default().fg(Color::LightGreen),
        )),
        Line::from(Span::raw(format!(" {} ", location_str))),
        Line::from(""),
        Line::from(vec![
            Span::styled(" S.P.E.C.I.A.L ", Style::default().fg(Color::Black).bg(Color::Green).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(vec![
            Span::styled(" STR ", Style::default().fg(Color::LightGreen)),
            Span::raw("5    "),
            Span::styled(" PER ", Style::default().fg(Color::LightGreen)),
            Span::raw("6    "),
            Span::styled(" END ", Style::default().fg(Color::LightGreen)),
            Span::raw("5"),
        ]),
        Line::from(vec![
            Span::styled(" CHA ", Style::default().fg(Color::LightGreen)),
            Span::raw("4    "),
            Span::styled(" INT ", Style::default().fg(Color::LightGreen)),
            Span::raw("7    "),
            Span::styled(" AGI ", Style::default().fg(Color::LightGreen)),
            Span::raw("5"),
        ]),
        Line::from(vec![
            Span::styled(" LCK ", Style::default().fg(Color::LightGreen)),
            Span::raw("6"),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            format!(" UPTIME: {} ", uptime_str),
            Style::default().fg(Color::DarkGray),
        )),
    ];

    Paragraph::new(lines)
        .alignment(Alignment::Left)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" [ STAT > GENERAL ] ")
                .border_type(BorderType::Plain)
                .style(Style::default().fg(Color::Green)),
        )
        .wrap(Wrap { trim: false })
}

pub fn render_stat_status<'a>(
    heart_rate_bpm: Option<u32>,
    uptime_start: Instant,
) -> Paragraph<'a> {
    let hr_text = match heart_rate_bpm {
        Some(hr) => format!("{} BPM", hr),
        None => "OFFLINE".to_string(),
    };
    let hr_color = if heart_rate_bpm.is_some() { Color::LightGreen } else { Color::Red };

    let uptime_secs = uptime_start.elapsed().as_secs();

    // Try to read battery from sysfs; fallback to N/A
    let battery_str = std::fs::read_to_string("/sys/class/power_supply/BAT0/capacity")
        .or_else(|_| std::fs::read_to_string("/sys/class/power_supply/battery/capacity"))
        .map(|s| format!("{}%", s.trim()))
        .unwrap_or_else(|_| "N/A".to_string());

    let lines = vec![
        Line::from(vec![
            Span::styled(" VITALS     ", Style::default().fg(Color::Black).bg(Color::Green).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(vec![
            Span::raw(" HEART RATE:  "),
            Span::styled(hr_text, Style::default().fg(hr_color).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(vec![
            Span::raw(" BATTERY:     "),
            Span::styled(battery_str, Style::default().fg(Color::LightCyan)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled(" SENSORS    ", Style::default().fg(Color::Black).bg(Color::Green).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(vec![
            Span::raw(" GPS         "),
            Span::styled("ONLINE", Style::default().fg(Color::LightGreen)),
        ]),
        Line::from(vec![
            Span::raw(" PULSE OX    "),
            Span::styled(
                if heart_rate_bpm.is_some() { "ONLINE" } else { "OFFLINE" },
                Style::default().fg(if heart_rate_bpm.is_some() { Color::LightGreen } else { Color::Red }),
            ),
        ]),
        Line::from(vec![
            Span::raw(" RTL-SDR     "),
            Span::styled("ONLINE", Style::default().fg(Color::LightGreen)),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            format!(" UPTIME: {}s ", uptime_secs),
            Style::default().fg(Color::DarkGray),
        )),
    ];

    Paragraph::new(lines)
        .alignment(Alignment::Left)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" [ STAT > STATUS ] ")
                .border_type(BorderType::Plain)
                .style(Style::default().fg(Color::Green)),
        )
}

pub fn render_stat_settings<'a>() -> Paragraph<'a> {
    let lines = vec![
        Line::from(vec![
            Span::styled(" DISPLAY    ", Style::default().fg(Color::Black).bg(Color::Green).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(Span::raw(" Brightness:   75%")),
        Line::from(Span::raw(" Theme:        Classic Green")),
        Line::from(""),
        Line::from(vec![
            Span::styled(" AUDIO      ", Style::default().fg(Color::Black).bg(Color::Green).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(Span::raw(" Volume:       50%")),
        Line::from(""),
        Line::from(vec![
            Span::styled(" SYSTEM     ", Style::default().fg(Color::Black).bg(Color::Green).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(Span::raw(" Safe Shutdown: ENABLED")),
        Line::from(Span::raw(" I2C:           /dev/i2c-1")),
        Line::from(Span::raw(" GPS:           gpsd://localhost:2947")),
        Line::from(""),
        Line::from(Span::styled(
            " ROBCO INDUSTRIES (TM) UNIFIED OS v7.1.0.8 ",
            Style::default().fg(Color::DarkGray),
        )),
    ];

    Paragraph::new(lines)
        .alignment(Alignment::Left)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" [ STAT > SETTINGS ] ")
                .border_type(BorderType::Plain)
                .style(Style::default().fg(Color::Green)),
        )
}

// ─────────────────────────────────────────────
// DATA SUBMENUS
// ─────────────────────────────────────────────

pub fn render_data_quests<'a>() -> Paragraph<'a> {
    let lines = vec![
        Line::from(vec![
            Span::styled(" ACTIVE QUESTS ", Style::default().fg(Color::Black).bg(Color::Green).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled(" [ ] ", Style::default().fg(Color::Yellow)),
            Span::raw("Fix radio antenna"),
        ]),
        Line::from(vec![
            Span::styled(" [X] ", Style::default().fg(Color::LightGreen)),
            Span::styled("Calibrate GPS module", Style::default().fg(Color::DarkGray).add_modifier(Modifier::CROSSED_OUT)),
        ]),
        Line::from(vec![
            Span::styled(" [ ] ", Style::default().fg(Color::Yellow)),
            Span::raw("Add new casing screws"),
        ]),
        Line::from(vec![
            Span::styled(" [ ] ", Style::default().fg(Color::Yellow)),
            Span::raw("Test RTL-SDR in field"),
        ]),
        Line::from(vec![
            Span::styled(" [X] ", Style::default().fg(Color::LightGreen)),
            Span::styled("Build TUI in Rust", Style::default().fg(Color::DarkGray).add_modifier(Modifier::CROSSED_OUT)),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            " TIP: Quest editing via virtual keyboard coming soon ",
            Style::default().fg(Color::DarkGray),
        )),
    ];

    Paragraph::new(lines)
        .alignment(Alignment::Left)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" [ DATA > QUESTS ] ")
                .border_type(BorderType::Plain)
                .style(Style::default().fg(Color::Green)),
        )
}

pub fn render_data_workshops<'a>() -> Paragraph<'a> {
    let lines = vec![
        Line::from(vec![
            Span::styled(" SAVED LOCATIONS ", Style::default().fg(Color::Black).bg(Color::Green).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled(" HOME BASE     ", Style::default().fg(Color::LightGreen)),
            Span::raw("33.7200°N  116.2150°W"),
        ]),
        Line::from(vec![
            Span::styled(" SCHOOL        ", Style::default().fg(Color::LightGreen)),
            Span::raw("33.7180°N  116.2300°W"),
        ]),
        Line::from(vec![
            Span::styled(" INDIO MARKET  ", Style::default().fg(Color::LightGreen)),
            Span::raw("33.7211°N  116.2175°W"),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            " TIP: Save new locations from MAP tab in a future update ",
            Style::default().fg(Color::DarkGray),
        )),
    ];

    Paragraph::new(lines)
        .alignment(Alignment::Left)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" [ DATA > WORKSHOPS ] ")
                .border_type(BorderType::Plain)
                .style(Style::default().fg(Color::Green)),
        )
}

pub fn render_data_stats<'a>() -> Paragraph<'a> {
    let total_items = read_db().map(|db| db.len()).unwrap_or(0);
    let now = Local::now();

    let lines = vec![
        Line::from(vec![
            Span::styled(" PIP-BOY STATS ", Style::default().fg(Color::Black).bg(Color::Green).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::raw(" Inventory items:    "),
            Span::styled(format!("{}", total_items), Style::default().fg(Color::LightCyan)),
        ]),
        Line::from(vec![
            Span::raw(" Quests completed:   "),
            Span::styled("2", Style::default().fg(Color::LightCyan)),
        ]),
        Line::from(vec![
            Span::raw(" Active quests:      "),
            Span::styled("3", Style::default().fg(Color::Yellow)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled(" BUILD INFO     ", Style::default().fg(Color::Black).bg(Color::Green).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(vec![
            Span::raw(" Builder:   "),
            Span::styled("TheCodingSoldier", Style::default().fg(Color::LightGreen)),
        ]),
        Line::from(vec![
            Span::raw(" Sponsor:   "),
            Span::styled("Hack Club", Style::default().fg(Color::LightGreen)),
        ]),
        Line::from(vec![
            Span::raw(" Session:   "),
            Span::styled(format!("{}", now.format("%Y-%m-%d")), Style::default().fg(Color::LightCyan)),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            " ROBCO INDUSTRIES (TM) UNIFIED OS v7.1.0.8 ",
            Style::default().fg(Color::DarkGray),
        )),
    ];

    Paragraph::new(lines)
        .alignment(Alignment::Left)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" [ DATA > STATS ] ")
                .border_type(BorderType::Plain)
                .style(Style::default().fg(Color::Green)),
        )
}

// ─────────────────────────────────────────────
// RADIO TAB
// ─────────────────────────────────────────────

pub fn render_radio<'a>(freq_mhz: f64) -> Paragraph<'a> {
    // Fake signal bar based on freq proximity to known stations
    let known_stations: &[(&str, f64)] = &[
        ("DIAMOND CITY RADIO", 100.1),
        ("CLASSICAL RADIO",    98.5),
        ("GALAXY NEWS RADIO",  101.5),
        ("RAIDER PIRATE WAVE", 95.7),
    ];

    let nearest = known_stations.iter().min_by(|a, b| {
        (a.1 - freq_mhz).abs().partial_cmp(&(b.1 - freq_mhz).abs()).unwrap()
    });

    let (station_name, signal_bars) = if let Some((name, freq)) = nearest {
        let diff = (freq - freq_mhz).abs();
        let bars = if diff < 0.1 { 5 } else if diff < 0.3 { 4 } else if diff < 0.5 { 3 } else if diff < 1.0 { 2 } else { 1 };
        (*name, bars)
    } else {
        ("SCANNING...", 0)
    };

    let bar_str: String = (0..5).map(|i| if i < signal_bars { "█" } else { "░" }).collect();

    let lines = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled(" RADIO      ", Style::default().fg(Color::Black).bg(Color::Green).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::raw("   FREQUENCY:  "),
            Span::styled(
                format!("{:.1} MHz", freq_mhz),
                Style::default().fg(Color::LightCyan).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("   STATION:    "),
            Span::styled(station_name.to_string(), Style::default().fg(Color::LightGreen)),
        ]),
        Line::from(vec![
            Span::raw("   SIGNAL:     "),
            Span::styled(bar_str, Style::default().fg(Color::LightGreen)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled(" PRESETS    ", Style::default().fg(Color::Black).bg(Color::Green).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(Span::raw("   1. DIAMOND CITY RADIO   100.1 MHz")),
        Line::from(Span::raw("   2. CLASSICAL RADIO       98.5 MHz")),
        Line::from(Span::raw("   3. GALAXY NEWS RADIO    101.5 MHz")),
        Line::from(Span::raw("   4. RAIDER PIRATE WAVE    95.7 MHz")),
        Line::from(""),
        Line::from(Span::styled(
            " Left/Right knob: tune ±0.1 MHz   +/-: preset jump ",
            Style::default().fg(Color::DarkGray),
        )),
    ];

    Paragraph::new(lines)
        .alignment(Alignment::Left)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" [ RADIO ] ")
                .border_type(BorderType::Plain)
                .style(Style::default().fg(Color::Green)),
        )
}

// ─────────────────────────────────────────────
// INVENTORY
// ─────────────────────────────────────────────

pub fn render_stat<'a>() -> Paragraph<'a> {
    render_radio(100.1)
}

pub fn render_inv<'a>(mut inv_list_state: &ListState, category_filter: &'a str) -> (List<'a>, Paragraph<'a>) {
    let invs = Block::default()
        .borders(Borders::ALL)
        .style(Style::default().fg(Color::White))
        .title(category_filter)
        .border_type(BorderType::Plain);

    let inv_list = read_db().expect("can fetch item list");

    let mut filtered_items: Vec<Item> = inv_list
        .into_iter()
        .filter(|item| item.category.eq_ignore_ascii_case(category_filter))
        .collect();

    filtered_items.sort_by(|a, b| b.created_at.cmp(&a.created_at));

    let mut items: Vec<_> = filtered_items
        .iter()
        .map(|item| {
            ListItem::new(Line::from(vec![Span::styled(
                item.name.clone(),
                Style::default(),
            )]))
        })
        .collect();

    items.push(ListItem::new(Line::from(vec![Span::styled(
        "+ Add New",
        Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
    )])));

    let selected_item = filtered_items
        .get(inv_list_state.selected().unwrap_or(0))
        .cloned()
        .unwrap_or(Item {
            id: 0,
            name: "ERR".into(),
            details: "ERR".into(),
            quantity: 0,
            category: category_filter.into(),
            created_at: chrono::Utc::now(),
        });

    let list = List::new(items).block(invs).highlight_style(
        Style::default()
            .bg(Color::Green)
            .fg(Color::Black)
            .add_modifier(Modifier::BOLD),
    );

    let detail_lines = vec![
        Line::from(vec![Span::styled(
            format!(" {} ", selected_item.name),
            Style::default().fg(Color::Black).bg(Color::Green).add_modifier(Modifier::BOLD),
        )]),
        Line::from(""),
        Line::from(vec![Span::raw(format!(" Added: {}", selected_item.created_at.format("%Y-%m-%d")))] ),
        Line::from(""),
        Line::from(vec![Span::styled(" Details:", Style::default().fg(Color::LightGreen))]),
        Line::from(vec![Span::raw(format!(" {}", selected_item.details))]),
        Line::from(""),
        Line::from(vec![Span::styled(" Quantity:", Style::default().fg(Color::LightGreen))]),
        Line::from(vec![Span::styled(
            format!(" {} ", selected_item.quantity),
            Style::default().fg(Color::LightCyan).add_modifier(Modifier::BOLD),
        )]),
    ];

    let paragraph = Paragraph::new(detail_lines)
        .block(
            Block::default()
                .title(" [ Item Detail ] ")
                .borders(Borders::ALL)
                .border_type(BorderType::Plain)
                .style(Style::default().fg(Color::Green)),
        )
        .wrap(Wrap { trim: true });

    (list, paragraph)
}

pub fn add_item_to_db() -> Result<Vec<Item>, Error> {
    let stdout = io::stdout();
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let mut rng = rand::thread_rng();
    let db_content = fs::read_to_string(DB_PATH)?;
    let mut parsed: Vec<Item> = serde_json::from_str(&db_content)?;

    let name = show_virtual_keyboard(&mut terminal, "Item Name")
        .map_err(|e| Error::ReadDBError(io::Error::new(io::ErrorKind::Other, e.to_string())))?;
    let category = show_category_selector(&mut terminal)
        .map_err(|e| Error::ReadDBError(io::Error::new(io::ErrorKind::Other, e.to_string())))?;
    let details = show_virtual_keyboard(&mut terminal, "Item Details")
        .map_err(|e| Error::ReadDBError(io::Error::new(io::ErrorKind::Other, e.to_string())))?;
    let quantity = show_quantity_selector(&mut terminal, 0)
        .map_err(|e| Error::ReadDBError(io::Error::new(io::ErrorKind::Other, e.to_string())))?;
    let new_item = Item {
        id: rng.gen_range(0..9999999),
        name,
        details,
        quantity,
        category,
        created_at: Utc::now(),
    };

    parsed.push(new_item);
    fs::write(DB_PATH, &serde_json::to_vec(&parsed)?)?;
    Ok(parsed)
}

pub fn show_category_selector<B: Backend>(terminal: &mut Terminal<B>) -> io::Result<String> {
    let categories = vec!["Weapons", "Apparel", "Aid", "Misc", "Junk", "Mods", "Ammo"];
    let mut state = ListState::default();
    state.select(Some(0));

    loop {
        terminal.clear();
        terminal.draw(|f| {
            let size = centered_rect(70, 50, f.area());
            let items: Vec<ListItem> = categories.iter().map(|c| ListItem::new(*c)).collect();

            let list = List::new(items)
                .block(Block::default().borders(Borders::ALL).title(" Select Category ").style(Style::default().fg(Color::Green)))
                .highlight_style(
                    Style::default()
                        .bg(Color::Green)
                        .fg(Color::Black)
                        .add_modifier(Modifier::BOLD),
                );

            f.render_stateful_widget(list, size, &mut state);
        })?;

        if let Event::Key(key) = event::read().map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))? {
            match key.code {
                KeyCode::Char('w') | KeyCode::Char('W') | KeyCode::Up => {
                    let i = match state.selected() {
                        Some(i) if i > 0 => i - 1,
                        _ => 0,
                    };
                    state.select(Some(i));
                }
                KeyCode::Char('s') | KeyCode::Char('S') | KeyCode::Down => {
                    let i = match state.selected() {
                        Some(i) if i < categories.len() - 1 => i + 1,
                        _ => categories.len() - 1,
                    };
                    state.select(Some(i));
                }
                KeyCode::Enter => {
                    if let Some(i) = state.selected() {
                        return Ok(categories[i].to_string());
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

pub fn show_quantity_selector<B: Backend>(terminal: &mut Terminal<B>, initial_quantity: u32) -> io::Result<u32> {
    let categories: Vec<String> = (0..101).map(|i| i.to_string()).collect();
    let mut state = ListState::default();
    state.select(Some(initial_quantity as usize));

    loop {
        terminal.clear();
        terminal.draw(|f| {
            let size = centered_rect(70, 50, f.area());
            let items: Vec<ListItem> = categories.iter().map(|c| ListItem::new(c.as_str())).collect();

            let list = List::new(items)
                .block(Block::default().borders(Borders::ALL).title(" Select Quantity ").style(Style::default().fg(Color::Green)))
                .highlight_style(
                    Style::default()
                        .bg(Color::Green)
                        .fg(Color::Black)
                        .add_modifier(Modifier::BOLD),
                );

            f.render_stateful_widget(list, size, &mut state);
        })?;

        if let Event::Key(key) = event::read().map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))? {
            match key.code {
                KeyCode::Char('w') | KeyCode::Char('W') | KeyCode::Up => {
                    let i = match state.selected() {
                        Some(i) if i > 0 => i - 1,
                        _ => categories.len() - 1,
                    };
                    state.select(Some(i));
                }
                KeyCode::Char('s') | KeyCode::Char('S') | KeyCode::Down => {
                    let i = match state.selected() {
                        Some(i) if i < categories.len() - 1 => i + 1,
                        _ => 0,
                    };
                    state.select(Some(i));
                }
                KeyCode::Enter => {
                    if let Some(selected_index) = state.selected() {
                        let quantity: u32 = categories[selected_index].parse().map_err(|e| {
                            io::Error::new(io::ErrorKind::InvalidData, format!("Failed to parse: {}", e))
                        })?;
                        return Ok(quantity);
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

pub fn render_data<'a>() -> Paragraph<'a> {
    Paragraph::new(vec![
        Line::from(vec![Span::raw("")]),
        Line::from(vec![Span::raw("Welcome")]),
        Line::from(vec![Span::raw("")]),
        Line::from(vec![Span::raw("to")]),
        Line::from(vec![Span::raw("")]),
        Line::from(vec![Span::styled("Pyp-Boy", Style::default().fg(Color::LightGreen))]),
        Line::from(vec![Span::raw("")]),
        Line::from(vec![Span::raw("Press Enter on inventory to add items.")]),
    ])
    .alignment(Alignment::Center)
    .block(
        Block::default()
            .borders(Borders::ALL)
            .style(Style::default().fg(Color::Green))
            .title(" Home ")
            .border_type(BorderType::Plain),
    )
}

pub fn read_db() -> Result<Vec<Item>, Error> {
    let db_content = fs::read_to_string(DB_PATH)?;
    let parsed: Vec<Item> = serde_json::from_str(&db_content)?;
    Ok(parsed)
}
