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
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                if let Err(e) = Self::ble_loop(state_clone, cmd_rx, resp_tx).await {
                    eprintln!("BLE error: {}", e);
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
            return Ok(());
        }
        drop(s);

        let Some(adapter) = Adapter::default().await else {
            return Err(anyhow::anyhow!("No adapter"));
        };
        adapter.wait_available().await?;

        let target_name = "Logitech Z407".to_string();
        let mut scan_handle = adapter.scan(&[]).await?;
        let mut device_opt: Option<Device> = None;

        // Scan for device
        while let Some(adv_device) = scan_handle.next().await {
            if let Some(name) = adv_device.device.name() {
                if name == target_name {
                    device_opt = Some(adv_device.device);
                    break;
                }
            }
        }

        let Some(mut device) = device_opt else {
            eprintln!("Z407 not found");
            return Ok(());
        };

        adapter.connect_device(&mut device).await?;

        let service_uuid = Uuid::parse_str("0000fdc2-0000-1000-8000-00805f9b34fb")?;
        let cmd_uuid = Uuid::parse_str("c2e758b9-0e78-41e0-b0cb-98a593193fc5")?;
        let resp_uuid = Uuid::parse_str("b84ac9c6-29c5-46d4-bba1-9d534784330f")?;

        let services = device.services().await?;
        let service = services
            .into_iter()
            .find(|s| s.uuid() == service_uuid)
            .ok_or(anyhow::anyhow!("Service not found"))?;

        let chars = service.characteristics().await?;
        let cmd_char = chars
            .iter()
            .find(|c| c.uuid() == cmd_uuid)
            .cloned()
            .ok_or(anyhow::anyhow!("Cmd char not found"))?;
        let resp_char = chars
            .iter()
            .find(|c| c.uuid() == resp_uuid)
            .cloned()
            .ok_or(anyhow::anyhow!("Resp char not found"))?;

        // Enable notifications
        let resp_tx_clone = resp_tx.clone();
        let resp_char_clone = resp_char.clone();
        tokio::spawn(async move {
            if let Ok(mut notifs) = resp_char_clone.notify().await {
                while let Some(data) = notifs.next().await {
                    if let Ok(data) = data {
                        let _ = resp_tx_clone.send(hex::encode(data));
                    }
                }
            }
        });

        // Handshake
        cmd_char.write(&[0x84, 0x05]).await?;
        sleep(Duration::from_millis(100)).await;
        cmd_char.write(&[0x84, 0x00]).await?;
        sleep(Duration::from_millis(100)).await;

        // Set connected
        let mut s = state.lock().unwrap();
        s.connected = true;
        s.current_input = "Bluetooth".to_string();
        drop(s);

        // Command loop
        loop {
            if let Ok(cmd) = cmd_rx.recv_timeout(Duration::from_millis(100)) {
                let _ = cmd_char.write(&cmd).await;
            } else {
                sleep(Duration::from_millis(100)).await;
            }
        }
    }

    fn send_cmd(&self, cmd: &[u8]) {
        let s = self.state.lock().unwrap();
        if let Some(ref tx) = s.cmd_tx {
            let _ = tx.send(cmd.to_vec());
        }
    }

    fn volume_up(&mut self) {
        self.send_cmd(&[0x80, 0x02]);
        let mut s = self.state.lock().unwrap();
        s.volume = (s.volume + 5.0).min(100.0);
    }

    fn volume_down(&mut self) {
        self.send_cmd(&[0x80, 0x03]);
        let mut s = self.state.lock().unwrap();
        s.volume = (s.volume - 5.0).max(0.0);
    }

    fn bass_up(&mut self) {
        self.send_cmd(&[0x80, 0x00]);
        let mut s = self.state.lock().unwrap();
        s.bass = (s.bass + 5.0).min(100.0);
    }

    fn bass_down(&mut self) {
        self.send_cmd(&[0x80, 0x01]);
        let mut s = self.state.lock().unwrap();
        s.bass = (s.bass - 5.0).max(0.0);
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
        let mut s = self.state.lock().unwrap();
        s.current_input = "Bluetooth".to_string();
    }

    fn switch_aux(&mut self) {
        self.send_cmd(&[0x81, 0x02]);
        let mut s = self.state.lock().unwrap();
        s.current_input = "AUX".to_string();
    }

    fn switch_usb(&mut self) {
        self.send_cmd(&[0x81, 0x03]);
        let mut s = self.state.lock().unwrap();
        s.current_input = "USB".to_string();
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
        // Poll responses (for future parsing, e.g., confirm vol up)
        let s = self.state.lock().unwrap();
        if let Some(ref rx) = s.resp_rx {
            while let Ok(resp_hex) = rx.try_recv() {
                println!("Response: {}", resp_hex);  // e.g., if resp_hex == "c002" { s.volume += 1.0; }
            }
        }
        let connected = s.connected;
        let current_input = s.current_input.clone();
        drop(s);

        CentralPanel::default().show(ctx, |ui| {
            ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
                ui.heading("Z407 Digital Puck");
                ui.add_space(10.0);

                if !connected {
                    if ui.button("Scan & Connect").clicked() {
                        let mut s = self.state.lock().unwrap();
                        s.scan_requested = true;
                    }
                } else {
                    // Volume slider (local copy)
                    let mut volume = {
                        let s_guard = self.state.lock().unwrap();
                        s_guard.volume
                    };
                    ui.horizontal(|ui| {
                        if ui.button("Vol -").clicked() {
                            self.volume_down();
                        }
                        ui.add(Slider::new(&mut volume, 0.0..=100.0).text("Volume"));
                        if ui.button("Vol +").clicked() {
                            self.volume_up();
                        }
                    });
                    // Update state post-slider
                    {
                        let mut s = self.state.lock().unwrap();
                        s.volume = volume;
                    }

                    // Bass slider (local copy)
                    let mut bass = {
                        let s_guard = self.state.lock().unwrap();
                        s_guard.bass
                    };
                    ui.horizontal(|ui| {
                        if ui.button("Bass -").clicked() {
                            self.bass_down();
                        }
                        ui.add(Slider::new(&mut bass, 0.0..=100.0).text("Bass"));
                        if ui.button("Bass +").clicked() {
                            self.bass_up();
                        }
                    });
                    // Update state post-slider
                    {
                        let mut s = self.state.lock().unwrap();
                        s.bass = bass;
                    }

                    ui.add_space(5.0);
                    ui.label(format!("Current Input: {}", current_input));

                    // Media buttons
                    ui.horizontal(|ui| {
                        if ui.button("⏸️ Play/Pause").clicked() {
                            self.play_pause();
                        }
                        if ui.button("⏭️ Next").clicked() {
                            self.next_track();
                        }
                        if ui.button("⏮️ Prev").clicked() {
                            self.prev_track();
                        }
                    });

                    // Input switches
                    ui.horizontal(|ui| {
                        if ui.button("BT").clicked() {
                            self.switch_bluetooth();
                        }
                        if ui.button("AUX").clicked() {
                            self.switch_aux();
                        }
                        if ui.button("USB").clicked() {
                            self.switch_usb();
                        }
                    });

                    ui.add_space(5.0);
                    // Extras
                    ui.horizontal(|ui| {
                        if ui.button("Pairing Mode").clicked() {
                            self.pairing();
                        }
                        if ui.button("Factory Reset").clicked() {
                            self.factory_reset();
                        }
                    });
                }

                ui.add_space(10.0);
                let color = if connected { Color32::GREEN } else { Color32::RED };
                let text = if connected { "Connected to Z407" } else { "Disconnected - Click Connect" };
                ui.colored_label(color, text);
            });
        });
    }
}

fn main() -> Result<(), eframe::Error> {
    env_logger::init();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size(vec2(350.0f32, 300.0f32)),
        ..Default::default()
    };
    eframe::run_native(
        "Z407 Puck",
        options,
        Box::new(|cc| Box::new(Z407PuckApp::new(cc))),
    )
}