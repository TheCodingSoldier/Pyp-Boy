use chrono::prelude::*;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event as CEvent, KeyCode},
	execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use rand::{distributions::Alphanumeric, prelude::*};

use std::fs;
use std::io;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};
use std::error::Error as StdError;
use thiserror::Error;
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Span, Line, Text},
    widgets::{Block, BorderType, Borders, Clear, ListState, Paragraph, Tabs},
    Terminal,
    Frame,
};

use gpsd_client::*;

extern crate linux_embedded_hal as hal;
extern crate max3010x;
extern crate ratatui;

use max3010x::{Max3010x, Led, SampleAveraging};

const DB_PATH: &str = "./data/db.json";

mod render_tabs;
mod menus;
mod kb;

use render_tabs::{
    render_map, render_stat, render_data, render_inv, read_db, get_map_data, Item,
    add_item_to_db, show_quantity_selector,
    render_stat_general, render_stat_status, render_stat_settings,
    render_data_quests, render_data_workshops, render_data_stats,
    render_radio,
};
use menus::{MenuItem, StatSubMenu, InvSubMenu, DataSubMenu};

#[derive(Error, Debug)]
pub enum Error {
    #[error("error reading the DB file: {0}")]
    ReadDBError(#[from] io::Error),
    #[error("error parsing the DB file: {0}")]
    ParseDBError(#[from] serde_json::Error),
}

enum Event<I> {
    Input(I),
    Tick,
}

#[derive(Debug, PartialEq)]
pub struct Coordinates {
    pub latitude: f64,
    pub longitude: f64,
}

pub fn get_current_coordinates_array() -> Result<[f64; 2], Box<dyn StdError>> {
    let mut gps = GPS::connect()
    	.map_err(|e| Box::<dyn StdError>::from(format!("GPS connect error: {:?}", e)))?;
	let data: GPSData = gps.current_data()
    	.map_err(|e| Box::<dyn StdError>::from(format!("GPS data error: {:?}", e)))?;
	if data.lat.is_finite() && data.lon.is_finite() {
    	Ok([data.lat, data.lon])
	} else {
		Err(Box::new(io::Error::new(
			io::ErrorKind::NotFound,
			"Latitude or longitude invalid",
		)))
	}
}

fn show_boot_screen<B: ratatui::backend::Backend>(terminal: &mut Terminal<B>) -> io::Result<()> {
    terminal.clear()?;
    terminal.draw(|f| {
        let area = f.size();
        let block = Block::default()
            .title(" ROBCO INDUSTRIES (TM) UNIFIED OPERATING SYSTEM ")
            .borders(Borders::ALL)
            .border_type(BorderType::Plain)
            .style(Style::default().fg(Color::Green));

        let text = Paragraph::new(vec![
            Line::from(""),
            Line::from(vec![Span::styled(
                "COPYRIGHT 2075 ROBCO INDUSTRIES",
                Style::default().fg(Color::LightGreen).add_modifier(Modifier::BOLD),
            )]),
            Line::from(""),
            Line::from(Span::raw("Initializing Pip-Boy 3000 Mk IV...") ),
            Line::from(""),
            Line::from(Span::raw("  [ OK ]  GPS daemon.......... gpsd://localhost:2947")),
            Line::from(Span::raw("  [ OK ]  Pulse oximeter..... /dev/i2c-1 (MAX30102)")),
            Line::from(Span::raw("  [ OK ]  Radio module....... RTL-SDR V4")),
            Line::from(Span::raw("  [ OK ]  Inventory DB....... ./data/db.json")),
            Line::from(""),
            Line::from(vec![Span::styled(
                "  ALL SYSTEMS NOMINAL. WELCOME, VAULT DWELLER.",
                Style::default().fg(Color::LightGreen),
            )]),
        ])
        .alignment(Alignment::Left)
        .block(block);

        f.render_widget(Clear, area);
        f.render_widget(text, area);
    })?;
    std::thread::sleep(Duration::from_secs(2));
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {

	let coords: [f64; 2] = match get_current_coordinates_array() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("GPS error: {}", e);
            [0.0, 0.0]
        }
    };

	let mut map_data: Option<String> = None;
	let mut last_map_refresh = Instant::now();
    let map_refresh_interval = Duration::from_secs(60);
	let uptime_start = Instant::now();

    // State
	let stat_submenu_titles = vec!["GENERAL", "STATUS", "SETTINGS"];
	let inv_submenu_titles = vec!["WEAPONS", "APPAREL", "AID", "MISC", "JUNK", "MODS", "AMMO"];
	let data_submenu_titles = vec!["QUESTS", "WORKSHOPS", "STATS"];

	let mut active_stat_submenu = StatSubMenu::General;
	let mut active_inv_submenu = InvSubMenu::Weapons;
	let mut active_data_submenu = DataSubMenu::Quests;
	let mut radio_freq_mhz: f64 = 100.1;
	let mut error_message: Option<String> = None;

    enable_raw_mode().expect("can run in raw mode");
	let mut stdout = io::stdout();
	execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;

    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    // Boot screen
    show_boot_screen(&mut terminal)?;

    let (tx, rx) = mpsc::channel();
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
                if let Ok(_) = tx.send(Event::Tick) {
                    last_tick = Instant::now();
                }
            }
        }
    });

    let menu_titles = vec!["STAT", "INV", "DATA", "MAP", "RADIO"];
    let mut active_menu_item = MenuItem::Stat;
    let mut inv_list_state = ListState::default();
    inv_list_state.select(Some(0));

    loop {
        // Refresh map every 60s
        if map_data.is_none() || last_map_refresh.elapsed() >= map_refresh_interval {
            last_map_refresh = Instant::now();
            let fresh_coords = get_current_coordinates_array().unwrap_or([0.0, 0.0]);
            map_data = Some(get_map_data(fresh_coords));
        }

        terminal.draw(|rect| {
            let size = rect.area();

            let menu: Vec<Line> = menu_titles
                .iter()
                .map(|t| {
                    let (first, rest) = t.split_at(1);
                    Line::from(vec![
                        Span::styled(
                            first,
                            Style::default()
                                .fg(Color::Yellow)
                                .add_modifier(Modifier::UNDERLINED),
                        ),
                        Span::styled(rest, Style::default().fg(Color::White)),
                    ])
                })
                .collect();

            let tabs = Tabs::new(menu)
                .select(Some(active_menu_item.into()))
                .block(Block::default().title(" PYP-BOY 3000 ").borders(Borders::ALL).style(Style::default().fg(Color::Green)))
                .style(Style::default().fg(Color::White))
                .highlight_style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))
                .divider(Span::raw(" | "));

			let (submenu_spans, active_index): (Vec<Line>, usize) = match active_menu_item {
				MenuItem::Stat => (
					stat_submenu_titles.iter().map(|t| {
							let (first, rest) = t.split_at(1);
							Line::from(vec![
								Span::styled(first, Style::default().fg(Color::Green).add_modifier(Modifier::UNDERLINED)),
								Span::styled(rest, Style::default().fg(Color::White)),
							])
						}).collect(),
					active_stat_submenu.into(),
				),
				MenuItem::Inv => (
					inv_submenu_titles.iter().map(|t| {
							let (first, rest) = t.split_at(1);
							Line::from(vec![
								Span::styled(first, Style::default().fg(Color::Green).add_modifier(Modifier::UNDERLINED)),
								Span::styled(rest, Style::default().fg(Color::White)),
							])
						}).collect(),
					active_inv_submenu.into(),
				),
				MenuItem::Data => (
					data_submenu_titles.iter().map(|t| {
							let (first, rest) = t.split_at(1);
							Line::from(vec![
								Span::styled(first, Style::default().fg(Color::Green).add_modifier(Modifier::UNDERLINED)),
								Span::styled(rest, Style::default().fg(Color::White)),
							])
						}).collect(),
					active_data_submenu.into(),
				),
				_ => (vec![], 0),
			};

			let show_secondary_menu = !submenu_spans.is_empty();

            // Bottom bar: error or copyright
            let bottom_bar = match &error_message {
                Some(msg) => Paragraph::new(Span::styled(
                        msg.clone(),
                        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                    ))
                    .alignment(Alignment::Center)
                    .block(Block::default().borders(Borders::ALL).title(" SYSTEM WARNING ").style(Style::default().fg(Color::Red))),
                None => Paragraph::new("COPYRIGHT 2075 ROBCO INDUSTRIES (TM)")
                    .style(Style::default().fg(Color::Green))
                    .alignment(Alignment::Center)
                    .block(Block::default().borders(Borders::ALL).style(Style::default().fg(Color::Green)).title(" COPYRIGHT ")
                        .border_type(BorderType::Plain)),
            };

			if show_secondary_menu {
				let secondary_tabs = Tabs::new(submenu_spans)
					.select(active_index)
					.block(Block::default().title(" SUBMENU ").borders(Borders::ALL).style(Style::default().fg(Color::Green)))
					.style(Style::default().fg(Color::White))
					.highlight_style(Style::default().fg(Color::Green).add_modifier(Modifier::BOLD))
					.divider(Span::raw(" | "));

				let adjusted_chunks = Layout::default()
					.direction(Direction::Vertical)
					.margin(1)
					.constraints([
						Constraint::Length(3),
						Constraint::Length(3),
						Constraint::Min(1),
						Constraint::Length(3),
					].as_ref())
					.split(size);

				rect.render_widget(tabs, adjusted_chunks[0]);
				rect.render_widget(secondary_tabs, adjusted_chunks[1]);

				match active_menu_item {
					MenuItem::Stat => match active_stat_submenu {
						StatSubMenu::General => rect.render_widget(
                                render_stat_general(coords, map_data.clone(), uptime_start),
                                adjusted_chunks[2]
                            ),
						StatSubMenu::Status => rect.render_widget(
                                render_stat_status(None, uptime_start),
                                adjusted_chunks[2]
                            ),
						StatSubMenu::Settings => rect.render_widget(
                                render_stat_settings(),
                                adjusted_chunks[2]
                            ),
					},
					MenuItem::Inv => match active_inv_submenu {
						InvSubMenu::Weapons => draw_filtered_inventory(rect, adjusted_chunks[2], &mut inv_list_state, "Weapons"),
						InvSubMenu::Apparel => draw_filtered_inventory(rect, adjusted_chunks[2], &mut inv_list_state, "Apparel"),
						InvSubMenu::Aid => draw_filtered_inventory(rect, adjusted_chunks[2], &mut inv_list_state, "Aid"),
						InvSubMenu::Misc => draw_filtered_inventory(rect, adjusted_chunks[2], &mut inv_list_state, "Misc"),
						InvSubMenu::Junk => draw_filtered_inventory(rect, adjusted_chunks[2], &mut inv_list_state, "Junk"),
						InvSubMenu::Mods => draw_filtered_inventory(rect, adjusted_chunks[2], &mut inv_list_state, "Mods"),
						InvSubMenu::Ammo => draw_filtered_inventory(rect, adjusted_chunks[2], &mut inv_list_state, "Ammo"),
					},
					MenuItem::Data => match active_data_submenu {
						DataSubMenu::Quests => rect.render_widget(render_data_quests(), adjusted_chunks[2]),
						DataSubMenu::Workshops => rect.render_widget(render_data_workshops(), adjusted_chunks[2]),
						DataSubMenu::Stats => rect.render_widget(render_data_stats(), adjusted_chunks[2]),
					},
					_ => {}
				}

				rect.render_widget(bottom_bar, adjusted_chunks[3]);
			} else {
				let chunks = Layout::default()
					.direction(Direction::Vertical)
					.margin(1)
					.constraints([
						Constraint::Length(3),
						Constraint::Min(2),
						Constraint::Length(3),
					].as_ref())
					.split(size);

				rect.render_widget(tabs, chunks[0]);
				match active_menu_item {
					MenuItem::Map => rect.render_widget(render_map(map_data.clone()), chunks[1]),
					MenuItem::Radio => rect.render_widget(render_radio(radio_freq_mhz), chunks[1]),
					_ => {}
				}
				rect.render_widget(bottom_bar, chunks[2]);
			}
        })?;

        match rx.recv()? {
            Event::Input(event) => match event.code {
                KeyCode::Esc => {
                    disable_raw_mode()?;
                    terminal.show_cursor()?;
					ratatui::restore();
                    break;
                }
                KeyCode::Left => {
					active_menu_item = match active_menu_item {
						MenuItem::Stat => MenuItem::Radio,
						MenuItem::Inv => MenuItem::Stat,
						MenuItem::Data => MenuItem::Inv,
						MenuItem::Map => MenuItem::Data,
						MenuItem::Radio => MenuItem::Map,
					};
				}
				KeyCode::Right => {
					active_menu_item = match active_menu_item {
						MenuItem::Stat => MenuItem::Inv,
						MenuItem::Inv => MenuItem::Data,
						MenuItem::Data => MenuItem::Map,
						MenuItem::Map => MenuItem::Radio,
						MenuItem::Radio => MenuItem::Stat,
					};
				}
                KeyCode::Down => {
                    if let Some(selected) = inv_list_state.selected() {
                        let amount_items = read_db().expect("can fetch item list").len();
                        if selected >= amount_items - 1 {
                            inv_list_state.select(Some(0));
                        } else {
                            inv_list_state.select(Some(selected + 1));
                        }
                    }
                }
                KeyCode::Up => {
                    if let Some(selected) = inv_list_state.selected() {
                        let amount_items = read_db().expect("can fetch item list").len();
                        if selected > 0 {
                            inv_list_state.select(Some(selected - 1));
                        } else {
                            inv_list_state.select(Some(amount_items - 1));
                        }
                    }
                }
				KeyCode::Enter => {
					if active_menu_item == MenuItem::Inv {
						if let Some(selected) = inv_list_state.selected() {
							let mut filtered_items: Vec<Item> = read_db()
								.expect("can fetch item list")
								.into_iter()
								.filter(|item| item.category.eq_ignore_ascii_case(active_inv_submenu.as_str()))
								.collect();
							filtered_items.sort_by_key(|item| std::cmp::Reverse(item.created_at));
							if selected == filtered_items.len() {
								add_item_to_db().expect("can add new item");
								inv_list_state.select(Some(0));
							} else {
								let selected_item = &filtered_items[selected];
								let new_item_quantity = show_quantity_selector(&mut terminal, selected_item.quantity)
									.map_err(|e| Error::ReadDBError(io::Error::new(io::ErrorKind::Other, e.to_string())))?;
								update_selected_item_quantity((selected_item.id as usize).try_into().unwrap(), new_item_quantity as u32);
							}
						}
					}
				}

                // Radio frequency tuning
                KeyCode::Char('u') | KeyCode::Char('U') => {
                    if active_menu_item == MenuItem::Radio {
                        radio_freq_mhz = (radio_freq_mhz + 0.1 * 10.0).round() / 10.0;
                        if radio_freq_mhz > 108.0 { radio_freq_mhz = 87.5; }
                    }
                }
                KeyCode::Char('d') | KeyCode::Char('D') => {
                    if active_menu_item == MenuItem::Radio {
                        radio_freq_mhz = (radio_freq_mhz - 0.1 * 10.0).round() / 10.0;
                        if radio_freq_mhz < 87.5 { radio_freq_mhz = 108.0; }
                    }
                }

				KeyCode::Char('+') => {
					match active_menu_item {
						MenuItem::Stat => {
							active_stat_submenu = match active_stat_submenu {
								StatSubMenu::General => StatSubMenu::Settings,
								StatSubMenu::Status => StatSubMenu::General,
								StatSubMenu::Settings => StatSubMenu::Status,
							};
						}
						MenuItem::Inv => {
							active_inv_submenu = match active_inv_submenu {
								InvSubMenu::Weapons => InvSubMenu::Ammo,
								InvSubMenu::Apparel => InvSubMenu::Weapons,
								InvSubMenu::Aid => InvSubMenu::Apparel,
								InvSubMenu::Misc => InvSubMenu::Aid,
								InvSubMenu::Junk => InvSubMenu::Misc,
								InvSubMenu::Mods => InvSubMenu::Junk,
								InvSubMenu::Ammo => InvSubMenu::Mods,
							};
						}
						MenuItem::Data => {
							active_data_submenu = match active_data_submenu {
								DataSubMenu::Quests => DataSubMenu::Stats,
								DataSubMenu::Workshops => DataSubMenu::Quests,
								DataSubMenu::Stats => DataSubMenu::Workshops,
							};
						}
						MenuItem::Radio => {
                            radio_freq_mhz = (radio_freq_mhz + 0.1 * 10.0).round() / 10.0;
                            if radio_freq_mhz > 108.0 { radio_freq_mhz = 87.5; }
						}
						_ => {}
					}
				}
				KeyCode::Char('-') => {
					match active_menu_item {
						MenuItem::Stat => {
							active_stat_submenu = match active_stat_submenu {
								StatSubMenu::General => StatSubMenu::Status,
								StatSubMenu::Status => StatSubMenu::Settings,
								StatSubMenu::Settings => StatSubMenu::General,
							};
						}
						MenuItem::Inv => {
							active_inv_submenu = match active_inv_submenu {
								InvSubMenu::Weapons => InvSubMenu::Apparel,
								InvSubMenu::Apparel => InvSubMenu::Aid,
								InvSubMenu::Aid => InvSubMenu::Misc,
								InvSubMenu::Misc => InvSubMenu::Junk,
								InvSubMenu::Junk => InvSubMenu::Mods,
								InvSubMenu::Mods => InvSubMenu::Ammo,
								InvSubMenu::Ammo => InvSubMenu::Weapons,
							};
						}
						MenuItem::Data => {
							active_data_submenu = match active_data_submenu {
								DataSubMenu::Quests => DataSubMenu::Workshops,
								DataSubMenu::Workshops => DataSubMenu::Stats,
								DataSubMenu::Stats => DataSubMenu::Quests,
							};
						}
						MenuItem::Radio => {
                            radio_freq_mhz = (radio_freq_mhz - 0.1 * 10.0).round() / 10.0;
                            if radio_freq_mhz < 87.5 { radio_freq_mhz = 108.0; }
						}
						_ => {}
					}
				}
                _ => {}
            },
            Event::Tick => {}
        }
    }

    Ok(())
}

fn draw_filtered_inventory<'a>(
    rect: &mut Frame<'a>,
    area: ratatui::layout::Rect,
    inv_list_state: &mut ListState,
    category: &str,
) {
    let inv_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(30), Constraint::Percentage(70)].as_ref())
        .split(area);

    let (left, right) = render_inv(inv_list_state, category);
    rect.render_stateful_widget(left, inv_chunks[0], inv_list_state);
    rect.render_widget(right, inv_chunks[1]);
}

fn update_selected_item_quantity(
    id: usize,
    new_quantity: u32,
) -> Result<(), Error> {
    let db_content = fs::read_to_string(DB_PATH)?;
    let mut parsed: Vec<Item> = serde_json::from_str(&db_content)?;

    if let Some(index) = parsed.iter().position(|item| item.id == id) {
        let item = &parsed[index];
        let name = item.name.clone();
        let details = item.details.clone();
        let category = item.category.clone();
        let created_at = item.created_at;
        parsed.remove(index);

        if new_quantity > 0 {
            let updated_item = Item {
                id: id.try_into().unwrap(),
                name,
                details,
                quantity: new_quantity,
                category,
                created_at,
            };
            parsed.push(updated_item);
        }
        fs::write(DB_PATH, &serde_json::to_vec(&parsed)?)?;
    }
    Ok(())
}
