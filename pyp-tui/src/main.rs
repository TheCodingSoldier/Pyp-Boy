use chrono::prelude::*;
use crossterm::{
    event::{self, Event as CEvent, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};

use std::fs;
use std::io;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use std::error::Error as StdError;
use thiserror::Error;
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Span, Line},
    widgets::{Block, BorderType, Borders, ListState, Paragraph, Tabs, Gauge},
    Terminal,
    Frame,
};

use gpsd_client::*;

extern crate linux_embedded_hal as hal;
extern crate max3010x;
extern crate ratatui;

const DB_PATH: &str = "./data/db.json";

mod render_tabs;
mod menus;
mod kb;
mod sensors;

use render_tabs::{
    render_map, render_radio, render_inv, read_db,
    get_map_data, Item, add_item_to_db, show_quantity_selector,
};
use menus::{MenuItem, StatSubMenu, InvSubMenu, DataSubMenu};
use sensors::{spawn_heart_rate_thread, read_battery_percent, HeartRateHandle};

// ─── Error ────────────────────────────────────────────────────────────────────
#[derive(Error, Debug)]
pub enum Error {
    #[error("error reading the DB file: {0}")]
    ReadDBError(#[from] io::Error),
    #[error("error parsing the DB file: {0}")]
    ParseDBError(#[from] serde_json::Error),
}

// ─── Event ────────────────────────────────────────────────────────────────────
enum Event<I> {
    Input(I),
    Tick,
}

// ─── AppState ─────────────────────────────────────────────────────────────────
struct AppState {
    active_menu_item: MenuItem,
    active_stat_submenu: StatSubMenu,
    active_inv_submenu: InvSubMenu,
    active_data_submenu: DataSubMenu,
    inv_list_state: ListState,
    map_data: Option<String>,
    gps_coords: Arc<Mutex<[f64; 2]>>,
    heart_rate: HeartRateHandle,
    last_map_refresh: Instant,
    radio_freq_mhz: f64,
    uptime_start: Instant,
}

impl AppState {
    fn new(gps_coords: Arc<Mutex<[f64; 2]>>, heart_rate: HeartRateHandle) -> Self {
        let mut inv_list_state = ListState::default();
        inv_list_state.select(Some(0));
        AppState {
            active_menu_item: MenuItem::Stat,
            active_stat_submenu: StatSubMenu::General,
            active_inv_submenu: InvSubMenu::Weapons,
            active_data_submenu: DataSubMenu::Quests,
            inv_list_state,
            map_data: None,
            gps_coords,
            heart_rate,
            last_map_refresh: Instant::now() - Duration::from_secs(61), // force first refresh
            radio_freq_mhz: 100.1,
            uptime_start: Instant::now(),
        }
    }
}

// ─── GPS helpers ──────────────────────────────────────────────────────────────
fn get_current_coordinates_array() -> Result<[f64; 2], Box<dyn StdError>> {
    let mut gps = GPS::connect()
        .map_err(|e| Box::<dyn StdError>::from(format!("GPS connect error: {:?}", e)))?;
    let data: GPSData = gps.current_data()
        .map_err(|e| Box::<dyn StdError>::from(format!("GPS data error: {:?}", e)))?;
    if data.lat.is_finite() && data.lon.is_finite() {
        Ok([data.lat, data.lon])
    } else {
        Err(Box::new(io::Error::new(
            io::ErrorKind::NotFound,
            "Latitude or longitude invalid in GPS data",
        )))
    }
}

/// Spawn a background thread that refreshes GPS coords every 60 s.
fn spawn_gps_thread() -> Arc<Mutex<[f64; 2]>> {
    let coords_handle: Arc<Mutex<[f64; 2]>> = Arc::new(Mutex::new([0.0, 0.0]));
    let coords_clone = Arc::clone(&coords_handle);
    thread::spawn(move || loop {
        match get_current_coordinates_array() {
            Ok(c) => {
                if let Ok(mut guard) = coords_clone.lock() {
                    *guard = c;
                }
            }
            Err(e) => eprintln!("[gps] refresh error: {}", e),
        }
        thread::sleep(Duration::from_secs(60));
    });
    coords_handle
}

// ─── Main ─────────────────────────────────────────────────────────────────────
fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Spawn background threads
    let gps_coords = spawn_gps_thread();
    let heart_rate = spawn_heart_rate_thread();

    let mut state = AppState::new(gps_coords, heart_rate);

    // Terminal setup
    enable_raw_mode().expect("can run in raw mode");
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;

    let (tx, rx) = std::sync::mpsc::channel();
    let tick_rate = Duration::from_millis(200);
    thread::spawn(move || {
        let mut last_tick = Instant::now();
        loop {
            let timeout = tick_rate
                .checked_sub(last_tick.elapsed())
                .unwrap_or_else(|| Duration::from_secs(0));
            if event::poll(timeout).expect("poll works") {
                if let CEvent::Key(key) = event::read().expect("can read events") {
                    tx.send(Event::Input(key)).expect("can send events");
                }
            }
            if last_tick.elapsed() >= tick_rate {
                if tx.send(Event::Tick).is_ok() {
                    last_tick = Instant::now();
                }
            }
        }
    });

    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    let menu_titles = vec!["STAT", "INV", "DATA", "MAP", "RADIO"];
    let stat_submenu_titles = vec!["GENERAL", "STATUS", "SETTINGS"];
    let inv_submenu_titles  = vec!["WEAPONS", "APPAREL", "AID", "MISC", "JUNK", "MODS", "AMMO"];
    let data_submenu_titles = vec!["QUESTS", "WORKSHOPS", "STATS"];

    loop {
        // ── Refresh map data every 60 s ───────────────────────────────────────
        if state.map_data.is_none() || state.last_map_refresh.elapsed() >= Duration::from_secs(60) {
            let coords = *state.gps_coords.lock().unwrap_or_else(|p| p.into_inner().into());
            // Intentionally ignoring the poisoned-lock edge case — coords will just be stale
            state.map_data = Some(get_map_data(coords));
            state.last_map_refresh = Instant::now();
        }

        // ── Draw ─────────────────────────────────────────────────────────────
        let coords_snap = *state.gps_coords.lock().unwrap();
        let hr_snap     = *state.heart_rate.lock().unwrap();
        let uptime      = state.uptime_start.elapsed();
        let radio_freq  = state.radio_freq_mhz;
        let map_data    = state.map_data.clone();
        let active_menu = state.active_menu_item;
        let active_stat = state.active_stat_submenu;
        let active_inv  = state.active_inv_submenu;
        let active_data = state.active_data_submenu;

        terminal.draw(|rect| {
            let size = rect.area();

            let copyright = Paragraph::new("COPYRIGHT 2075 ROBCO INDUSTRIES(R) | UNIFIED OPERATING SYSTEM")
                .style(Style::default().fg(Color::LightCyan))
                .alignment(Alignment::Center)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .style(Style::default().fg(Color::DarkGray))
                        .title("ROBCO")
                        .border_type(BorderType::Plain),
                );

            let menu: Vec<Line> = menu_titles
                .iter()
                .map(|t| {
                    let (first, rest) = t.split_at(1);
                    Line::from(vec![
                        Span::styled(first, Style::default().fg(Color::Yellow).add_modifier(Modifier::UNDERLINED)),
                        Span::styled(rest,  Style::default().fg(Color::White)),
                    ])
                })
                .collect();

            let tabs = Tabs::new(menu)
                .select(Some(active_menu.into()))
                .block(Block::default().title("PYP-BOY 3000 MARK IV").borders(Borders::ALL))
                .style(Style::default().fg(Color::White))
                .highlight_style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))
                .divider(Span::raw(" | "));

            let (submenu_spans, active_index): (Vec<Line>, usize) = match active_menu {
                MenuItem::Stat => (
                    stat_submenu_titles.iter().map(|t| styled_submenu_tab(t)).collect(),
                    active_stat.into(),
                ),
                MenuItem::Inv => (
                    inv_submenu_titles.iter().map(|t| styled_submenu_tab(t)).collect(),
                    active_inv.into(),
                ),
                MenuItem::Data => (
                    data_submenu_titles.iter().map(|t| styled_submenu_tab(t)).collect(),
                    active_data.into(),
                ),
                _ => (vec![], 0),
            };

            let show_secondary = !submenu_spans.is_empty();

            if show_secondary {
                let secondary_tabs = Tabs::new(submenu_spans)
                    .select(active_index)
                    .block(Block::default().title("SUBMENU").borders(Borders::ALL))
                    .style(Style::default().fg(Color::White))
                    .highlight_style(Style::default().fg(Color::Green).add_modifier(Modifier::BOLD))
                    .divider(Span::raw(" | "));

                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .margin(2)
                    .constraints([
                        Constraint::Length(3),
                        Constraint::Length(3),
                        Constraint::Min(1),
                        Constraint::Length(3),
                    ].as_ref())
                    .split(size);

                rect.render_widget(tabs, chunks[0]);
                rect.render_widget(secondary_tabs, chunks[1]);

                match active_menu {
                    MenuItem::Stat => match active_stat {
                        StatSubMenu::General  => rect.render_widget(render_stat_general(coords_snap),  chunks[2]),
                        StatSubMenu::Status   => rect.render_widget(render_stat_status(hr_snap),       chunks[2]),
                        StatSubMenu::Settings => rect.render_widget(render_stat_settings(),            chunks[2]),
                    },
                    MenuItem::Inv => {
                        draw_filtered_inventory(rect, chunks[2], &mut state.inv_list_state, active_inv.as_str());
                    }
                    MenuItem::Data => match active_data {
                        DataSubMenu::Quests    => rect.render_widget(render_data_quests(),           chunks[2]),
                        DataSubMenu::Workshops => rect.render_widget(render_data_workshops(),        chunks[2]),
                        DataSubMenu::Stats     => rect.render_widget(render_data_stats(uptime),      chunks[2]),
                    },
                    _ => {}
                }
                rect.render_widget(copyright, chunks[3]);
            } else {
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .margin(2)
                    .constraints([
                        Constraint::Length(3),
                        Constraint::Min(2),
                        Constraint::Length(3),
                    ].as_ref())
                    .split(size);

                rect.render_widget(tabs, chunks[0]);
                match active_menu {
                    MenuItem::Map   => rect.render_widget(render_map(map_data), chunks[1]),
                    MenuItem::Radio => rect.render_widget(render_radio(radio_freq), chunks[1]),
                    _ => {}
                }
                rect.render_widget(copyright, chunks[2]);
            }
        })?;

        // ── Input ─────────────────────────────────────────────────────────────
        match rx.recv()? {
            Event::Input(event) => match event.code {
                KeyCode::Esc => {
                    disable_raw_mode()?;
                    execute!(io::stdout(), LeaveAlternateScreen)?;
                    terminal.show_cursor()?;
                    break;
                }
                KeyCode::Left => {
                    state.active_menu_item = match state.active_menu_item {
                        MenuItem::Stat  => MenuItem::Radio,
                        MenuItem::Inv   => MenuItem::Stat,
                        MenuItem::Data  => MenuItem::Inv,
                        MenuItem::Map   => MenuItem::Data,
                        MenuItem::Radio => MenuItem::Map,
                    };
                }
                KeyCode::Right => {
                    state.active_menu_item = match state.active_menu_item {
                        MenuItem::Stat  => MenuItem::Inv,
                        MenuItem::Inv   => MenuItem::Data,
                        MenuItem::Data  => MenuItem::Map,
                        MenuItem::Map   => MenuItem::Radio,
                        MenuItem::Radio => MenuItem::Stat,
                    };
                }
                // Radio frequency tuning with [ and ]
                KeyCode::Char('[') if state.active_menu_item == MenuItem::Radio => {
                    state.radio_freq_mhz = (state.radio_freq_mhz - 0.1).max(87.5);
                }
                KeyCode::Char(']') if state.active_menu_item == MenuItem::Radio => {
                    state.radio_freq_mhz = (state.radio_freq_mhz + 0.1).min(108.0);
                }
                KeyCode::Down => {
                    if let Some(selected) = state.inv_list_state.selected() {
                        let count = read_db().map(|db| db.len()).unwrap_or(1);
                        let next = if selected >= count.saturating_sub(1) { 0 } else { selected + 1 };
                        state.inv_list_state.select(Some(next));
                    }
                }
                KeyCode::Up => {
                    if let Some(selected) = state.inv_list_state.selected() {
                        let count = read_db().map(|db| db.len()).unwrap_or(1);
                        let prev = if selected == 0 { count.saturating_sub(1) } else { selected - 1 };
                        state.inv_list_state.select(Some(prev));
                    }
                }
                KeyCode::Enter => {
                    if state.active_menu_item == MenuItem::Inv {
                        if let Some(selected) = state.inv_list_state.selected() {
                            let mut filtered: Vec<Item> = read_db()
                                .expect("can fetch item list")
                                .into_iter()
                                .filter(|item| item.category.eq_ignore_ascii_case(state.active_inv_submenu.as_str()))
                                .collect();
                            filtered.sort_by_key(|item| std::cmp::Reverse(item.created_at));
                            if selected == filtered.len() {
                                add_item_to_db().expect("can add new item");
                                state.inv_list_state.select(Some(0));
                            } else if let Some(sel_item) = filtered.get(selected) {
                                let new_qty = show_quantity_selector(&mut terminal, sel_item.quantity)
                                    .map_err(|e| Error::ReadDBError(io::Error::new(io::ErrorKind::Other, e.to_string())))?;
                                update_selected_item_quantity(sel_item.id, new_qty)?;
                            }
                        }
                    }
                }
                // Submenu prev (+) / next (-)
                KeyCode::Char('+') => navigate_submenu(&mut state, true),
                KeyCode::Char('-') => navigate_submenu(&mut state, false),

                _ => {}
            },
            Event::Tick => {}
        }
    }

    Ok(())
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn styled_submenu_tab(t: &str) -> Line {
    let (first, rest) = t.split_at(1);
    Line::from(vec![
        Span::styled(first, Style::default().fg(Color::Green).add_modifier(Modifier::UNDERLINED)),
        Span::styled(rest,  Style::default().fg(Color::White)),
    ])
}

fn navigate_submenu(state: &mut AppState, forward: bool) {
    match state.active_menu_item {
        MenuItem::Stat => {
            state.active_stat_submenu = if forward {
                match state.active_stat_submenu {
                    StatSubMenu::General  => StatSubMenu::Status,
                    StatSubMenu::Status   => StatSubMenu::Settings,
                    StatSubMenu::Settings => StatSubMenu::General,
                }
            } else {
                match state.active_stat_submenu {
                    StatSubMenu::General  => StatSubMenu::Settings,
                    StatSubMenu::Status   => StatSubMenu::General,
                    StatSubMenu::Settings => StatSubMenu::Status,
                }
            };
        }
        MenuItem::Inv => {
            state.active_inv_submenu = if forward {
                match state.active_inv_submenu {
                    InvSubMenu::Weapons => InvSubMenu::Apparel,
                    InvSubMenu::Apparel => InvSubMenu::Aid,
                    InvSubMenu::Aid     => InvSubMenu::Misc,
                    InvSubMenu::Misc    => InvSubMenu::Junk,
                    InvSubMenu::Junk    => InvSubMenu::Mods,
                    InvSubMenu::Mods    => InvSubMenu::Ammo,
                    InvSubMenu::Ammo    => InvSubMenu::Weapons,
                }
            } else {
                match state.active_inv_submenu {
                    InvSubMenu::Weapons => InvSubMenu::Ammo,
                    InvSubMenu::Apparel => InvSubMenu::Weapons,
                    InvSubMenu::Aid     => InvSubMenu::Apparel,
                    InvSubMenu::Misc    => InvSubMenu::Aid,
                    InvSubMenu::Junk    => InvSubMenu::Misc,
                    InvSubMenu::Mods    => InvSubMenu::Junk,
                    InvSubMenu::Ammo    => InvSubMenu::Mods,
                }
            };
        }
        MenuItem::Data => {
            state.active_data_submenu = if forward {
                match state.active_data_submenu {
                    DataSubMenu::Quests    => DataSubMenu::Workshops,
                    DataSubMenu::Workshops => DataSubMenu::Stats,
                    DataSubMenu::Stats     => DataSubMenu::Quests,
                }
            } else {
                match state.active_data_submenu {
                    DataSubMenu::Quests    => DataSubMenu::Stats,
                    DataSubMenu::Workshops => DataSubMenu::Quests,
                    DataSubMenu::Stats     => DataSubMenu::Workshops,
                }
            };
        }
        _ => {}
    }
}

fn draw_filtered_inventory<'a>(
    rect: &mut Frame<'a>,
    area: ratatui::layout::Rect,
    inv_list_state: &mut ListState,
    category: &str,
) {
    let inv_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(25), Constraint::Percentage(75)].as_ref())
        .split(area);
    let (left, right) = render_inv(inv_list_state, category);
    rect.render_stateful_widget(left, inv_chunks[0], inv_list_state);
    rect.render_widget(right, inv_chunks[1]);
}

fn update_selected_item_quantity(id: usize, new_quantity: u32) -> Result<(), Error> {
    let db_content = fs::read_to_string(DB_PATH)?;
    let mut parsed: Vec<Item> = serde_json::from_str(&db_content)?;
    if let Some(index) = parsed.iter().position(|item| item.id == id) {
        let item = parsed.remove(index);
        if new_quantity > 0 {
            parsed.push(Item { quantity: new_quantity, ..item });
        }
        fs::write(DB_PATH, &serde_json::to_vec(&parsed)?)?;
    }
    Ok(())
}

// ─── STAT tab renders ─────────────────────────────────────────────────────────

fn render_stat_general<'a>(coords: [f64; 2]) -> Paragraph<'a> {
    let now: DateTime<Local> = Local::now();
    let lines = vec![
        Line::from(vec![Span::styled("── GENERAL INFO ──", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))]),
        Line::from(""),
        Line::from(vec![
            Span::styled("Date/Time : ", Style::default().fg(Color::Green)),
            Span::raw(now.format("%Y-%m-%d  %H:%M:%S").to_string()),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("Latitude  : ", Style::default().fg(Color::Green)),
            Span::raw(format!("{:.6}°", coords[0])),
        ]),
        Line::from(vec![
            Span::styled("Longitude : ", Style::default().fg(Color::Green)),
            Span::raw(format!("{:.6}°", coords[1])),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("System    : ", Style::default().fg(Color::Green)),
            Span::raw("PYP-BOY 3000 MARK IV"),
        ]),
        Line::from(vec![
            Span::styled("OS        : ", Style::default().fg(Color::Green)),
            Span::raw("ROBCO UNIFIED OS v6.2"),
        ]),
    ];
    Paragraph::new(lines)
        .block(Block::default().title("STAT > GENERAL").borders(Borders::ALL).border_type(BorderType::Plain))
        .style(Style::default().fg(Color::White))
}

fn render_stat_status<'a>(heart_rate: Option<u32>) -> Paragraph<'a> {
    let hr_str = heart_rate
        .map(|bpm| format!("{} BPM", bpm))
        .unwrap_or_else(|| "-- BPM  (sensor offline)".to_string());

    let battery = read_battery_percent();
    let bat_str = battery.map(|b| format!("{}%", b)).unwrap_or_else(|| "N/A".to_string());

    let lines = vec![
        Line::from(vec![Span::styled("── LIFE SIGNS ──", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))]),
        Line::from(""),
        Line::from(vec![
            Span::styled("Heart Rate : ", Style::default().fg(Color::Green)),
            Span::styled(hr_str, Style::default().fg(Color::LightRed).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("Battery    : ", Style::default().fg(Color::Green)),
            Span::raw(bat_str),
        ]),
        Line::from(""),
        Line::from(vec![Span::styled("── RADS ──", Style::default().fg(Color::Yellow))]),
        Line::from(vec![
            Span::styled("Radiation  : ", Style::default().fg(Color::Green)),
            Span::styled("0 RAD/SEC", Style::default().fg(Color::LightGreen)),
        ]),
    ];
    Paragraph::new(lines)
        .block(Block::default().title("STAT > STATUS").borders(Borders::ALL).border_type(BorderType::Plain))
        .style(Style::default().fg(Color::White))
}

fn render_stat_settings<'a>() -> Paragraph<'a> {
    let lines = vec![
        Line::from(vec![Span::styled("── SETTINGS ──", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))]),
        Line::from(""),
        Line::from(vec![
            Span::styled("Display    : ", Style::default().fg(Color::Green)),
            Span::raw("Brightness 80%"),
        ]),
        Line::from(vec![
            Span::styled("Volume     : ", Style::default().fg(Color::Green)),
            Span::raw("60%"),
        ]),
        Line::from(vec![
            Span::styled("GPS Refresh: ", Style::default().fg(Color::Green)),
            Span::raw("60 seconds"),
        ]),
        Line::from(vec![
            Span::styled("Theme      : ", Style::default().fg(Color::Green)),
            Span::raw("Green (ROBCO default)"),
        ]),
        Line::from(""),
        Line::from(vec![Span::styled("Controls: [ ] to tune radio | +/- for submenus | Esc to exit",
            Style::default().fg(Color::DarkGray))]),
    ];
    Paragraph::new(lines)
        .block(Block::default().title("STAT > SETTINGS").borders(Borders::ALL).border_type(BorderType::Plain))
        .style(Style::default().fg(Color::White))
}

// ─── DATA tab renders ─────────────────────────────────────────────────────────

fn render_data_quests<'a>() -> Paragraph<'a> {
    let lines = vec![
        Line::from(vec![Span::styled("── ACTIVE QUESTS ──", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))]),
        Line::from(""),
        Line::from(vec![
            Span::styled("[ ] ", Style::default().fg(Color::DarkGray)),
            Span::styled("Build the Pyp-Boy", Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(vec![Span::styled("      Assemble hardware, flash OS, wire sensors.", Style::default().fg(Color::DarkGray))]),
        Line::from(""),
        Line::from(vec![
            Span::styled("[ ] ", Style::default().fg(Color::DarkGray)),
            Span::styled("Electroplate the chassis", Style::default().fg(Color::White)),
        ]),
        Line::from(vec![Span::styled("      Copper sulfate bath for that authentic look.", Style::default().fg(Color::DarkGray))]),
        Line::from(""),
        Line::from(vec![
            Span::styled("[X] ", Style::default().fg(Color::Green)),
            Span::styled("Write the TUI software", Style::default().fg(Color::DarkGray).add_modifier(Modifier::DIM)),
        ]),
    ];
    Paragraph::new(lines)
        .block(Block::default().title("DATA > QUESTS").borders(Borders::ALL).border_type(BorderType::Plain))
        .style(Style::default().fg(Color::White))
}

fn render_data_workshops<'a>() -> Paragraph<'a> {
    let lines = vec![
        Line::from(vec![Span::styled("── SAVED LOCATIONS ──", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))]),
        Line::from(""),
        Line::from(vec![
            Span::styled("HOME BASE     ", Style::default().fg(Color::Green)),
            Span::raw("Indio, CA  33.7206° N  116.2156° W"),
        ]),
        Line::from(""),
        Line::from(vec![Span::styled("Add locations via GPS when hardware is connected.",
            Style::default().fg(Color::DarkGray))]),
    ];
    Paragraph::new(lines)
        .block(Block::default().title("DATA > WORKSHOPS").borders(Borders::ALL).border_type(BorderType::Plain))
        .style(Style::default().fg(Color::White))
}

fn render_data_stats<'a>(uptime: Duration) -> Paragraph<'a> {
    let total_items = read_db().map(|db| db.len()).unwrap_or(0);
    let h = uptime.as_secs() / 3600;
    let m = (uptime.as_secs() % 3600) / 60;
    let s = uptime.as_secs() % 60;

    let lines = vec![
        Line::from(vec![Span::styled("── SESSION STATS ──", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))]),
        Line::from(""),
        Line::from(vec![
            Span::styled("Uptime       : ", Style::default().fg(Color::Green)),
            Span::raw(format!("{:02}h {:02}m {:02}s", h, m, s)),
        ]),
        Line::from(vec![
            Span::styled("Items Stored : ", Style::default().fg(Color::Green)),
            Span::raw(format!("{}", total_items)),
        ]),
        Line::from(vec![
            Span::styled("Caps         : ", Style::default().fg(Color::Green)),
            Span::styled("∞ (modded)", Style::default().fg(Color::Yellow)),
        ]),
        Line::from(vec![
            Span::styled("SPECIAL      : ", Style::default().fg(Color::Green)),
            Span::raw("S7 P5 E6 C4 I8 A5 L9"),
        ]),
        Line::from(""),
        Line::from(vec![Span::styled("..You are the Sole Survivor.",
            Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC))]),
    ];
    Paragraph::new(lines)
        .block(Block::default().title("DATA > STATS").borders(Borders::ALL).border_type(BorderType::Plain))
        .style(Style::default().fg(Color::White))
}
