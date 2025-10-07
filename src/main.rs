use anyhow::Result;
use eframe::egui;
use bluest::{Adapter, Device, Uuid};
use egui::{CentralPanel, Color32, Context, Slider, vec2};
use futures_util::stream::StreamExt;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;
use tokio::time::sleep;

#[derive(Default, Clone)]
struct Z407State {
    connected: bool,
    volume: f32,
    bass: f32,
    current_input: String,
    scan_requested: bool,
}

struct Z407PuckApp {
    state: Arc<Mutex<Z407State>>,
    cmd_tx: mpsc::Sender<Vec<u8>>,
    resp_rx: mpsc::Receiver<String>,
}

impl Z407PuckApp {
    fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let state = Arc::new(Mutex::new(Z407State {
            scan_requested: true,
            ..Default::default()
        }));
        let state_clone = state.clone();
        let (cmd_tx, cmd_rx) = mpsc::channel::<Vec<u8>>();
        let (resp_tx, resp_rx) = mpsc::channel::<String>();

        thread::spawn(move || {
            println!("BLE thread started");
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                if let Err(e) = Self::ble_loop(state_clone, cmd_rx, resp_tx).await {
                    eprintln!("BLE loop error: {}", e);
                }
            });
        });

        Self { state, cmd_tx, resp_rx }
    }

    async fn ble_loop(
        state: Arc<Mutex<Z407State>>,
        cmd_rx: mpsc::Receiver<Vec<u8>>,
        resp_tx: mpsc::Sender<String>,
    ) -> Result<()> {
        // --- FIX: Define the Service UUID to scan for ---
        let service_uuid = Uuid::parse_str("0000fdc2-0000-1000-8000-00805f9b34fb")?;
        
        loop {
            {
                let s = state.lock().unwrap();
                if !s.scan_requested {
                    sleep(Duration::from_millis(200)).await;
                    continue;
                }
            }

            println!("Scan requested. Getting default adapter...");
            let adapter = Adapter::default().await.ok_or(anyhow::anyhow!("No Bluetooth adapter found"))?;
            adapter.wait_available().await?;
            println!("Adapter available");

            // --- FIX: Scan specifically for the Service UUID ---
            println!("Starting scan for Z407 service UUID...");
            let mut scan = adapter.scan(&[service_uuid]).await?;
            
            println!("Waiting for device...");
            let device_opt = tokio::time::timeout(Duration::from_secs(10), scan.next()).await;

            let Some(Ok(Some(adv_device))) = device_opt else {
                eprintln!("Z407 not found in scan. Resetting scan request.");
                let mut s = state.lock().unwrap();
                s.scan_requested = false;
                continue;
            };

            let mut device = adv_device.device;
            println!("Found device: {:?}, connecting...", device.name().unwrap_or_else(|_| "Unknown".to_string()));

            adapter.connect_device(&mut device).await?;
            println!("Connected! Discovering services...");

            let cmd_uuid = Uuid::parse_str("c2e758b9-0e78-41e0-b0cb-98a593193fc5")?;
            let resp_uuid = Uuid::parse_str("b84ac9c6-29c5-46d4-bba1-9d534784330f")?;

            let service = device.services().await?
                .into_iter()
                .find(|s| s.uuid() == service_uuid)
                .ok_or(anyhow::anyhow!("Service not found"))?;

            let cmd_char = service.characteristics().await?
                .into_iter()
                .find(|c| c.uuid() == cmd_uuid)
                .ok_or(anyhow::anyhow!("Cmd char not found"))?;
            let resp_char = service.characteristics().await?
                .into_iter()
                .find(|c| c.uuid() == resp_uuid)
                .ok_or(anyhow::anyhow!("Resp char not found"))?;

            let resp_tx_clone = resp_tx.clone();
            tokio::spawn(async move {
                if let Ok(mut notifs) = resp_char.notify().await {
                    println!("Notifications enabled");
                    while let Some(data_res) = notifs.next().await {
                        if let Ok(data) = data_res {
                            let hex = hex::encode(data);
                            let _ = resp_tx_clone.send(hex);
                        }
                    }
                }
            });

            cmd_char.write(&[0x84, 0x05]).await?;
            sleep(Duration::from_millis(200)).await;
            cmd_char.write(&[0x84, 0x00]).await?;
            println!("Handshake complete");

            {
                let mut s = state.lock().unwrap();
                s.connected = true;
                s.scan_requested = false;
            }

            loop {
                if !device.is_connected().await? {
                    println!("Device disconnected.");
                    break;
                }
                if let Ok(cmd) = cmd_rx.try_recv() {
                    if cmd_char.write(&cmd).await.is_err() {
                        eprintln!("Failed to write command.");
                        break;
                    }
                }
                sleep(Duration::from_millis(50)).await;
            }

            println!("Command loop exited.");
            {
                let mut s = state.lock().unwrap();
                s.connected = false;
            }
        }
    }
    
    fn send_cmd(&self, cmd: &[u8]) { let _ = self.cmd_tx.send(cmd.to_vec()); }
    fn volume_up(&self) { self.send_cmd(&[0x80, 0x02]); }
    fn volume_down(&self) { self.send_cmd(&[0x80, 0x03]); }
    fn bass_up(&self) { self.send_cmd(&[0x80, 0x00]); }
    fn bass_down(&self) { self.send_cmd(&[0x80, 0x01]); }
    fn play_pause(&self) { self.send_cmd(&[0x80, 0x04]); }
    fn next_track(&self) { self.send_cmd(&[0x80, 0x05]); }
    fn prev_track(&self) { self.send_cmd(&[0x80, 0x06]); }
    fn switch_bluetooth(&self) { self.send_cmd(&[0x81, 0x01]); }
    fn switch_aux(&self) { self.send_cmd(&[0x81, 0x02]); }
    fn switch_usb(&self) { self.send_cmd(&[0x81, 0x03]); }
    fn pairing(&self) { self.send_cmd(&[0x82, 0x00]); }
    fn factory_reset(&self) { self.send_cmd(&[0x83, 0x00]); }
}

impl eframe::App for Z407PuckApp {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        let responses: Vec<String> = self.resp_rx.try_iter().collect();
        if !responses.is_empty() {
            let mut s = self.state.lock().unwrap();
            for resp_hex in responses {
                 match resp_hex.as_str() {
                    "c101" => s.current_input = "Bluetooth".to_string(),
                    "c102" => s.current_input = "AUX".to_string(),
                    "c103" => s.current_input = "USB".to_string(),
                    _ => {}
                }
            }
        }

        let mut current_state = self.state.lock().unwrap().clone();

        CentralPanel::default().show(ctx, |ui| {
            ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
                ui.heading("Z407 Digital Puck");
                ui.add_space(10.0);

                if !current_state.connected {
                    if ui.button("Scan & Connect").clicked() {
                        self.state.lock().unwrap().scan_requested = true;
                    }
                } else {
                    ui.horizontal(|ui| {
                        if ui.button("Vol -").clicked() { self.volume_down(); }
                        ui.add(Slider::new(&mut current_state.volume, 0.0..=100.0).text("Volume"));
                        if ui.button("Vol +").clicked() { self.volume_up(); }
                    });
                    ui.horizontal(|ui| {
                        if ui.button("Bass -").clicked() { self.bass_down(); }
                        ui.add(Slider::new(&mut current_state.bass, 0.0..=100.0).text("Bass"));
                        if ui.button("Bass +").clicked() { self.bass_up(); }
                    });
                    ui.add_space(5.0);
                    ui.label(format!("Current Input: {}", current_state.current_input));
                    ui.horizontal(|ui| {
                        if ui.button("⏮️").clicked() { self.prev_track(); }
                        if ui.button("⏯️").clicked() { self.play_pause(); }
                        if ui.button("⏭️").clicked() { self.next_track(); }
                    });
                    ui.horizontal(|ui| {
                        if ui.button("BT").clicked() { self.switch_bluetooth(); }
                        if ui.button("AUX").clicked() { self.switch_aux(); }
                        if ui.button("USB").clicked() { self.switch_usb(); }
                    });
                    ui.add_space(5.0);
                    ui.horizontal(|ui| {
                        if ui.button("Pairing Mode").clicked() { self.pairing(); }
                        if ui.button("Factory Reset").clicked() { self.factory_reset(); }
                    });
                }

                ui.add_space(10.0);
                let (color, text) = if current_state.connected {
                    (Color32::GREEN, "Connected to Z407")
                } else {
                    (Color32::RED, "Disconnected - Click to Scan")
                };
                ui.colored_label(color, text);
            });
        });

        {
            let mut s = self.state.lock().unwrap();
            s.volume = current_state.volume;
            s.bass = current_state.bass;
        }

        ctx.request_repaint_after(Duration::from_millis(50));
    }
}

fn main() -> Result<(), eframe::Error> {
    env_logger::init();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size(vec2(350.0, 400.0)),
        ..Default::default()
    };
    eframe::run_native(
        "Z407 Puck",
        options,
        Box::new(|cc| Box::new(Z407PuckApp::new(cc))),
    )
}