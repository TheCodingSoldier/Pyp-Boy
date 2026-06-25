// src/main.rs

use rppal::gpio::{Gpio, InputPin, Level};

use evdev::{Key, InputEvent, EventType, AttributeSet};
use evdev::uinput::{VirtualDevice, VirtualDeviceBuilder};

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use std::thread;
use anyhow::{Result, Context};
use log::{info, debug};
use env_logger;

// Define GPIO pins for each encoder and the main button
const ENCODER_PINS: [(u8, u8); 5] = [
    (17, 27), // Encoder 1: Left/Right (main tab nav)
    (23, 24), // Encoder 2: Up/Down (list nav)
    (5, 6),   // Encoder 3: A/D (secondary nav)
    (19, 26), // Encoder 4: W/S (secondary nav)
    (20, 21), // Encoder 5: +/- (submenu nav)
];

const MAIN_BUTTON_PIN: u8 = 10;

struct Encoder {
    pin_a: InputPin,
    pin_b: InputPin,
    last_state_a: Level,
}

impl Encoder {
    fn new(gpio: &Gpio, pin_a_num: u8, pin_b_num: u8) -> Result<Self> {
        let pin_a = gpio.get(pin_a_num)?.into_input_pullup();
        let pin_b = gpio.get(pin_b_num)?.into_input_pullup();
        let initial_state_a = pin_a.read();
        Ok(Encoder {
            pin_a,
            pin_b,
            last_state_a: initial_state_a,
        })
    }

    fn update(&mut self) -> Option<i8> {
        let current_state_a = self.pin_a.read();
        let current_state_b = self.pin_b.read();

        if current_state_a != self.last_state_a {
            self.last_state_a = current_state_a;
            if current_state_a == Level::Low {
                if current_state_b == Level::Low {
                    return Some(1);  // Clockwise
                } else {
                    return Some(-1); // Counter-clockwise
                }
            }
        }
        None
    }
}

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    info!("Starting Pyp-Boy encoder input service...");

    let gpio = Gpio::new().context("Failed to initialize GPIO")?;

    // Setup encoders
    let mut encoders: Vec<Encoder> = Vec::with_capacity(ENCODER_PINS.len());
    for (i, &(pin_a_num, pin_b_num)) in ENCODER_PINS.iter().enumerate() {
        let encoder = Encoder::new(&gpio, pin_a_num, pin_b_num)
            .with_context(|| format!("Failed to set up encoder {}", i + 1))?;
        encoders.push(encoder);
        info!("Encoder {} (GPIO {}/{}) configured.", i + 1, pin_a_num, pin_b_num);
    }

    // Setup main button
    let main_button_pin = gpio.get(MAIN_BUTTON_PIN)?.into_input_pullup();
    info!("Main Button (GPIO {}) configured.", MAIN_BUTTON_PIN);

    // Setup evdev virtual device
    let uinput_device = Arc::new(Mutex::new(
        VirtualDeviceBuilder::new()?
            .name("EC12 Multi-Encoder Keyboard")
            .with_keys(&AttributeSet::from_iter(vec![
                Key::KEY_LEFT, Key::KEY_RIGHT,
                Key::KEY_UP, Key::KEY_DOWN,
                Key::KEY_A, Key::KEY_D,
                Key::KEY_W, Key::KEY_S,
                Key::KEY_KPPLUS, Key::KEY_KPMINUS,
                Key::KEY_ENTER,
            ]))?
            .build()
            .context("Failed to build virtual device")?
    ));
    info!("Virtual input device created.");

    let mut handles = Vec::new();

    // Spawn encoder threads
    for (i, mut encoder) in encoders.into_iter().enumerate() {
        let uinput_device_clone: Arc<Mutex<VirtualDevice>> = Arc::clone(&uinput_device);
        let key_map = match i {
            0 => (Key::KEY_LEFT,   Key::KEY_RIGHT),
            1 => (Key::KEY_UP,     Key::KEY_DOWN),
            2 => (Key::KEY_A,      Key::KEY_D),
            3 => (Key::KEY_W,      Key::KEY_S),
            4 => (Key::KEY_KPPLUS, Key::KEY_KPMINUS),
            _ => unreachable!(),
        };

        let handle = thread::spawn(move || -> Result<()> {
            let mut last_detection_time = Instant::now();
            let debounce_delay = Duration::from_millis(5);

            loop {
                thread::sleep(Duration::from_micros(100));

                if last_detection_time.elapsed() < debounce_delay {
                    continue;
                }

                if let Some(direction) = encoder.update() {
                    let mut device = uinput_device_clone.lock().unwrap();
                    if direction == 1 {
                        debug!("Encoder {} CW: {:?}", i + 1, key_map.0);
                        device.emit(&[
                            InputEvent::new(EventType::KEY, key_map.0.0, 1),
                            InputEvent::new(EventType::KEY, key_map.0.0, 0),
                        ])?;
                    } else {
                        debug!("Encoder {} CCW: {:?}", i + 1, key_map.1);
                        device.emit(&[
                            InputEvent::new(EventType::KEY, key_map.1.0, 1),
                            InputEvent::new(EventType::KEY, key_map.1.0, 0),
                        ])?;
                    }
                    last_detection_time = Instant::now();
                }
            }
        });
        handles.push(handle);
    }

    // Spawn button thread
    let uinput_device_clone: Arc<Mutex<VirtualDevice>> = Arc::clone(&uinput_device);
    let button_handle = thread::spawn(move || -> Result<()> {
        let mut last_state = main_button_pin.read();
        let mut last_press_time = Instant::now();
        let debounce_delay = Duration::from_millis(50);

        loop {
            thread::sleep(Duration::from_millis(10));
            let current_state = main_button_pin.read();

            if current_state != last_state {
                if last_press_time.elapsed() < debounce_delay {
                    continue;
                }
                last_state = current_state;
                if current_state == Level::Low {
                    info!("Main Button Pressed: KEY_ENTER");
                    let mut device = uinput_device_clone.lock().unwrap();
                    device.emit(&[
                        InputEvent::new(EventType::KEY, Key::KEY_ENTER.0, 1),
                        InputEvent::new(EventType::KEY, Key::KEY_ENTER.0, 0),
                    ])?;
                }
                last_press_time = Instant::now();
            }
        }
    });
    handles.push(button_handle);

    // Wait for all threads — FIX: .join() returns Result<Result<()>>, unwrap outer panic,
    // then propagate inner anyhow::Error correctly. Do NOT use ? on .expect() output.
    for handle in handles {
        match handle.join() {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                eprintln!("Encoder/button thread returned error: {:#}", e);
            }
            Err(_) => {
                eprintln!("A thread panicked.");
            }
        }
    }

    info!("Service stopped.");
    Ok(())
}
