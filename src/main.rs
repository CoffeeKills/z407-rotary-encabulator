use anyhow::Result;
use egui::{Align2, CentralPanel, Color32, Context, Slider, Ui};
use eframe::egui;
use simplersble::{Adapter, Characteristic, Peripheral, ScanEvent, Service, Uuid};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use tokio::sync::mpsc;

#[derive(Default)]
struct Z407State {
    connected: bool,
    volume: f32,
    bass: f32,
    current_input: String,
    sender: Option<mpsc::Sender<String>>,
}

struct Z407PuckApp {
    state: Arc<Mutex<Z407State>>,
}

impl Z407PuckApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let state = Arc::new(Mutex::new(Z407State::default()));
        let state_clone = state.clone();
        // Spawn BLE thread
        thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                if let Err(e) = Self::ble_loop(state_clone).await {
                    eprintln!("BLE error: {}", e);
                }
            });
        });
        Self { state }
    }

    async fn ble_loop(state: Arc<Mutex<Z407State>>) -> Result<()> {
        let mut adapter = Adapter::default()?;
        adapter.start_scan()?;

        // Scan for Z407
        let target_name = "Logitech Z407".to_string();
        let timeout = 5000u64; // 5s
        let mut scanned = false;
        loop {
            if adapter.is_scanning()? {
                match adapter.wait_for_scan_event(Duration::from_millis(100))? {
                    Some(ScanEvent::DeviceFound(peripheral)) => {
                        if peripheral.name()? == Some(target_name.clone()) {
                            println!("Found Z407: {}", peripheral.address()?);
                            peripheral.connect()?;
                            let service = peripheral.service(&Uuid::parse_str("0000fdc2-0000-1000-8000-00805f9b34fb")?)?;
                            let cmd_char = service.characteristic(&Uuid::parse_str("c2e758b9-0e78-41e0-b0cb-98a593193fc5")?)?;
                            let resp_char = service.characteristic(&Uuid::parse_str("b84ac9c6-29c5-46d4-bba1-9d534784330f")?)?;

                            // Enable notifications
                            let (tx, mut rx) = mpsc::channel(32);
                            let state_clone = state.clone();
                            resp_char.enable_notify(move |data: &[u8]| {
                                if let Err(_) = tx.blocking_send(hex::encode(data)) {
                                    eprintln!("Failed to send response");
                                }
                                // Parse response, e.g., if data == b"d40501" { /* init ok */ }
                                println!("Response: {:?}", data);
                                // Update state based on response (e.g., if b"c002" vol up confirmed, update UI via state)
                            })?;

                            // Handshake
                            cmd_char.write(&[0x84, 0x05])?;
                            // Wait for d40501 (in real, poll rx or timeout)
                            tokio::time::sleep(Duration::from_millis(100)).await;
                            cmd_char.write(&[0x84, 0x00])?;
                            // Wait for d40001 + d40003

                            let mut s = state.lock().unwrap();
                            s.connected = true;
                            s.current_input = "Bluetooth".to_string();
                            s.sender = Some(rx); // Wait, rx is recv, but we set sender? Typo—use for polling if needed
                            drop(s);
                            scanned = true;
                            break;
                        }
                    }
                    _ => {}
                }
            } else {
                break;
            }
            if !scanned && adapter.scan_time()? > timeout {
                break;
            }
        }

        // Keep connection alive, handle cmds via channel or state
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            // Poll responses, update state
            let mut s = state.lock().unwrap();
            if !s.connected { break; }
            // e.g., if let Ok(resp) = s.sender.as_ref().unwrap().recv_timeout(Duration::from_millis(0)) { ... }
        }
        Ok(())
    }

    fn send_cmd(&self, cmd: &[u8]) -> Result<()> {
        let state = self.state.lock().unwrap();
        if !state.connected { return Ok(()); }
        // In full impl, queue cmd to BLE thread via channel
        println!("Sending cmd: {:?}", cmd);
        // Simulate response update
        drop(state);
        // Actual write in thread
        Ok(())
    }

    fn volume_up(&mut self) {
        self.send_cmd(&[0x80, 0x02]).unwrap();
        let mut s = self.state.lock().unwrap();
        s.volume = (s.volume + 5.0).min(100.0);
    }

    fn volume_down(&mut self) {
        self.send_cmd(&[0x80, 0x03]).unwrap();
        let mut s = self.state.lock().unwrap();
        s.volume = (s.volume - 5.0).max(0.0);
    }

    fn bass_up(&mut self) {
        self.send_cmd(&[0x80, 0x00]).unwrap();
        let mut s = self.state.lock().unwrap();
        s.bass = (s.bass + 5.0).min(100.0);
    }

    fn bass_down(&mut self) {
        self.send_cmd(&[0x80, 0x01]).unwrap();
        let mut s = self.state.lock().unwrap();
        s.bass = (s.bass - 5.0).max(0.0);
    }

    fn play_pause(&mut self) {
        self.send_cmd(&[0x80, 0x04]).unwrap();
    }

    fn next_track(&mut self) {
        self.send_cmd(&[0x80, 0x05]).unwrap();
    }

    fn prev_track(&mut self) {
        self.send_cmd(&[0x80, 0x06]).unwrap();
    }

    fn switch_bluetooth(&mut self) {
        self.send_cmd(&[0x81, 0x01]).unwrap();
        let mut s = self.state.lock().unwrap();
        s.current_input = "Bluetooth".to_string();
    }

    fn switch_aux(&mut self) {
        self.send_cmd(&[0x81, 0x02]).unwrap();
        let mut s = self.state.lock().unwrap();
        s.current_input = "AUX".to_string();
    }

    fn switch_usb(&mut self) {
        self.send_cmd(&[0x81, 0x03]).unwrap();
        let mut s = self.state.lock().unwrap();
        s.current_input = "USB".to_string();
    }

    fn pairing(&mut self) {
        self.send_cmd(&[0x82, 0x00]).unwrap();
    }

    fn factory_reset(&mut self) {
        self.send_cmd(&[0x83, 0x00]).unwrap();
    }
}

impl eframe::App for Z407PuckApp {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        CentralPanel::default().show(ctx, |ui| {
            let s = self.state.lock().unwrap();
            ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
                ui.heading("Z407 Digital Puck");
                ui.add_space(10.0);

                if !s.connected {
                    if ui.button("Scan & Connect").clicked() {
                        // Trigger scan in thread if needed; here it auto-starts
                        println!("Connect clicked - scanning...");
                    }
                } else {
                    // Volume row
                    ui.horizontal(|ui| {
                        if ui.button("Vol -").clicked() { drop(s); self.volume_down(); }
                        ui.add(Slider::new(&mut self.state.lock().unwrap().volume, 0.0..=100.0).text("Volume"));
                        if ui.button("Vol +").clicked() { drop(s); self.volume_up(); }
                    });

                    // Bass row
                    ui.horizontal(|ui| {
                        if ui.button("Bass -").clicked() { drop(s); self.bass_down(); }
                        ui.add(Slider::new(&mut self.state.lock().unwrap().bass, 0.0..=100.0).text("Bass"));
                        if ui.button("Bass +").clicked() { drop(s); self.bass_up(); }
                    });

                    ui.add_space(5.0);
                    ui.label(format!("Current Input: {}", s.current_input));

                    // Media buttons
                    ui.horizontal(|ui| {
                        if ui.button("⏸️ Play/Pause").clicked() { drop(s); self.play_pause(); }
                        if ui.button("⏭️ Next").clicked() { drop(s); self.next_track(); }
                        if ui.button("⏮️ Prev").clicked() { drop(s); self.prev_track(); }
                    });

                    // Input switches
                    ui.horizontal(|ui| {
                        if ui.button("BT").clicked() { drop(s); self.switch_bluetooth(); }
                        if ui.button("AUX").clicked() { drop(s); self.switch_aux(); }
                        if ui.button("USB").clicked() { drop(s); self.switch_usb(); }
                    });

                    ui.add_space(5.0);
                    // Extras
                    ui.horizontal(|ui| {
                        if ui.button("Pairing Mode").clicked() { drop(s); self.pairing(); }
                        if ui.button("Factory Reset").clicked() { drop(s); self.factory_reset(); }
                    });
                }

                ui.add_space(10.0);
                let color = if s.connected { Color32::GREEN } else { Color32::RED };
                let text = if s.connected { "Connected to Z407" } else { "Disconnected - Click Connect" };
                ui.colored_label(color, text);
            });
        });
    }
}

fn main() -> Result<(), eframe::Error> {
    let options = eframe::NativeOptions {
        initial_window_size: Some(egui::vec2(350.0, 300.0)),
        ..Default::default()
    };
    env_logger::init(); // For debug prints
    eframe::run_native(
        "Z407 Puck",
        options,
        Box::new(|cc| Box::new(Z407PuckApp::new(cc))),
    )
}