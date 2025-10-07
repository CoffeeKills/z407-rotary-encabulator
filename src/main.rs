use anyhow::Result;
use egui::{CentralPanel, Color32, Context, Slider, vec2};
use eframe::egui;
use bluest::{Adapter, Device, Uuid};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;
use tokio::time::sleep;
use futures_util::stream::StreamExt;

#[derive(Default)]
struct Z407State {
    connected: bool,
    volume: f32,
    bass: f32,
    current_input: String,
    cmd_tx: Option<mpsc::Sender<Vec<u8>>>,
    resp_rx: Option<mpsc::Receiver<String>>,
    scan_requested: bool,
}

struct Z407PuckApp {
    state: Arc<Mutex<Z407State>>,
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

        // Set up channels in state
        {
            let mut s = state.lock().unwrap();
            s.cmd_tx = Some(cmd_tx);
            s.resp_rx = Some(resp_rx);
        }

        // Spawn BLE thread
        thread::spawn(move || {
            println!("BLE thread started");
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                if let Err(e) = Self::ble_loop(state_clone, cmd_rx, resp_tx).await {
                    eprintln!("BLE loop error: {}", e);
                }
            });
        });

        Self { state }
    }

    async fn ble_loop(
        state: Arc<Mutex<Z407State>>,
        cmd_rx: mpsc::Receiver<Vec<u8>>,
        resp_tx: mpsc::Sender<String>,
    ) -> Result<()> {
        let s = state.lock().unwrap();
        if !s.scan_requested {
            println!("Scan not requested");
            return Ok(());
        }
        drop(s);

        println!("Getting default adapter...");
        let Some(adapter) = Adapter::default().await else {
            eprintln!("No Bluetooth adapter found");
            return Err(anyhow::anyhow!("No adapter"));
        };

        // FIX #1: adapter.address() is now an async function and must be awaited.
        let adapter_addr = adapter.address().await?;
        println!("Adapter ready: {:?}", adapter_addr);


        println!("Waiting for adapter to be available...");
        adapter.wait_available().await?;
        println!("Adapter available");

        let target_name = "Logitech Z407".to_string();
        println!("Starting scan for '{}'", target_name);
        let mut scan_handle = adapter.scan(&[]).await?;
        let mut device_opt: Option<Device> = None;
        let scan_start = std::time::Instant::now();
        let scan_timeout = Duration::from_secs(10);

        // Scan for device
        while let Some(adv_device) = scan_handle.next().await {
            if scan_start.elapsed() > scan_timeout {
                println!("Scan timeout");
                break;
            }
            // FIX #2: The device address is retrieved correctly here, the error was for the adapter.
            let addr = adv_device.device.address();
            println!("Scanned device: addr={:?}", addr);

            // FIX #3: device.name() is also now an async function and must be awaited.
            if let Ok(Some(name)) = adv_device.device.name().await {
                println!("  Name: '{}'", name);
                if name == target_name {
                    println!("  MATCH! Connecting to {:?}", addr);
                    device_opt = Some(adv_device.device);
                    break;
                }
            } else {
                println!("  No name");
            }
        }

        let Some(mut device) = device_opt else {
            eprintln!("Z407 not found in scan");
            return Ok(());
        };

        println!("Connecting to device...");
        adapter.connect_device(&mut device).await?;
        println!("Connected! Discovering services...");

        let service_uuid = Uuid::parse_str("0000fdc2-0000-1000-8000-00805f9b34fb")?;
        let cmd_uuid = Uuid::parse_str("c2e758b9-0e78-41e0-b0cb-98a593193fc5")?;
        let resp_uuid = Uuid::parse_str("b84ac9c6-29c5-46d4-bba1-9d534784330f")?;

        let services = device.services().await?;
        println!("Found {} services", services.len());
        let service = services
            .into_iter()
            .find(|s| s.uuid() == service_uuid)
            .ok_or(anyhow::anyhow!("Service not found"))?;
        println!("Found target service: {:?}", service.uuid());

        let chars = service.characteristics().await?;
        println!("Found {} characteristics", chars.len());
        let cmd_char = chars
            .iter()
            .find(|c| c.uuid() == cmd_uuid)
            .cloned()
            .ok_or(anyhow::anyhow!("Cmd char not found"))?;
        println!("Found cmd char: {:?}", cmd_char.uuid());
        let resp_char = chars
            .iter()
            .find(|c| c.uuid() == resp_uuid)
            .cloned()
            .ok_or(anyhow::anyhow!("Resp char not found"))?;
        println!("Found resp char: {:?}", resp_char.uuid());

        // Enable notifications
        let resp_tx_clone = resp_tx.clone();
        tokio::spawn(async move {
            if let Ok(mut notifs) = resp_char.notify().await {
                println!("Notifications enabled");
                while let Some(data) = notifs.next().await {
                    let hex = hex::encode(data);
                    println!("Response: {}", hex);
                    let _ = resp_tx_clone.send(hex);
                }
            } else {
                eprintln!("Failed to enable notifications");
            }
        });

        // Handshake
        println!("Sending INITIATE (84 05)");
        cmd_char.write(&[0x84, 0x05]).await?;
        sleep(Duration::from_millis(200)).await;
        println!("Sending ACKNOWLEDGE (84 00)");
        cmd_char.write(&[0x84, 0x00]).await?;
        sleep(Duration::from_millis(200)).await;
        println!("Handshake complete");

        {
            let mut s = state.lock().unwrap();
            s.connected = true;
            s.current_input = "Bluetooth".to_string();
        }

        println!("Entering command loop");

        // Command loop
        loop {
            if let Ok(cmd) = cmd_rx.try_recv() {
                println!("Sending cmd: {:?}", cmd);
                if cmd_char.write(&cmd).await.is_err() {
                    eprintln!("Failed to write command, device may have disconnected.");
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
        Ok(())
    }

    fn send_cmd(&self, cmd: &[u8]) {
        let s = self.state.lock().unwrap();
        if let Some(ref tx) = s.cmd_tx {
            let _ = tx.send(cmd.to_vec());
        }
    }

    fn volume_up(&mut self) {
        self.send_cmd(&[0x80, 0x02]);
    }

    fn volume_down(&mut self) {
        self.send_cmd(&[0x80, 0x03]);
    }

    fn bass_up(&mut self) {
        self.send_cmd(&[0x80, 0x00]);
    }

    fn bass_down(&mut self) {
        self.send_cmd(&[0x80, 0x01]);
    }

    fn play_pause(&mut self) {
        self.send_cmd(&[0x80, 0x04]);
    }

    fn next_track(&mut self) {
        self.send_cmd(&[0x80, 0x05]);
    }

    fn prev_track(&mut self) {
        self.send_cmd(&[0x80, 0x06]);
    }
    
    fn switch_bluetooth(&mut self) {
        self.send_cmd(&[0x81, 0x01]);
    }

    fn switch_aux(&mut self) {
        self.send_cmd(&[0x81, 0x02]);
    }

    fn switch_usb(&mut self) {
        self.send_cmd(&[0x81, 0x03]);
    }
    
    fn pairing(&mut self) {
        self.send_cmd(&[0x82, 0x00]);
    }
    
    fn factory_reset(&mut self) {
        self.send_cmd(&[0x83, 0x00]);
    }
}

impl eframe::App for Z407PuckApp {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        let mut s = self.state.lock().unwrap();
        // Poll responses
        if let Some(ref rx) = s.resp_rx {
            while let Ok(resp_hex) = rx.try_recv() {
                // Here you can parse responses and update the state
                // For now, it's just logged in the BLE thread
                match resp_hex.as_str() {
                    "c101" => s.current_input = "Bluetooth".to_string(),
                    "c102" => s.current_input = "AUX".to_string(),
                    "c103" => s.current_input = "USB".to_string(),
                    _ => {}
                }
            }
        }
        
        let connected = s.connected;
        let current_input = s.current_input.clone();

        CentralPanel::default().show(ctx, |ui| {
            ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
                ui.heading("Z407 Digital Puck");
                ui.add_space(10.0);

                if !connected {
                    if ui.button("Scan & Connect").clicked() {
                        s.scan_requested = true;
                        // You might need to restart the BLE thread here if it has exited.
                        // This implementation assumes the app is restarted.
                    }
                } else {
                    // Volume slider
                    ui.horizontal(|ui| {
                        if ui.button("Vol -").clicked() { self.volume_down(); }
                        ui.add(Slider::new(&mut s.volume, 0.0..=100.0).text("Volume"));
                        if ui.button("Vol +").clicked() { self.volume_up(); }
                    });

                    // Bass slider
                    ui.horizontal(|ui| {
                        if ui.button("Bass -").clicked() { self.bass_down(); }
                        ui.add(Slider::new(&mut s.bass, 0.0..=100.0).text("Bass"));
                        if ui.button("Bass +").clicked() { self.bass_up(); }
                    });

                    ui.add_space(5.0);
                    ui.label(format!("Current Input: {}", current_input));

                    // Media buttons
                    ui.horizontal(|ui| {
                        if ui.button("⏮️ Prev").clicked() { self.prev_track(); }
                        if ui.button("⏸️ Play/Pause").clicked() { self.play_pause(); }
                        if ui.button("⏭️ Next").clicked() { self.next_track(); }
                    });

                    // Input switches
                    ui.horizontal(|ui| {
                        if ui.button("BT").clicked() { self.switch_bluetooth(); }
                        if ui.button("AUX").clicked() { self.switch_aux(); }
                        if ui.button("USB").clicked() { self.switch_usb(); }
                    });

                    ui.add_space(5.0);
                    // Extras
                    ui.horizontal(|ui| {
                        if ui.button("Pairing Mode").clicked() { self.pairing(); }
                        if ui.button("Factory Reset").clicked() { self.factory_reset(); }
                    });
                }

                ui.add_space(10.0);
                let (color, text) = if connected {
                    (Color32::GREEN, "Connected to Z407")
                } else {
                    (Color32::RED, "Disconnected - Click to Scan")
                };
                ui.colored_label(color, text);
            });
        });
        
        // Request a repaint to keep the UI responsive
        ctx.request_repaint();
    }
}

fn main() -> Result<(), eframe::Error> {
    env_logger::init(); // Initialize the logger
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size(vec2(350.0, 400.0)), // Adjusted size for better layout
        ..Default::default()
    };
    eframe::run_native(
        "Z407 Puck",
        options,
        Box::new(|cc| Box::new(Z407PuckApp::new(cc))),
    )
}