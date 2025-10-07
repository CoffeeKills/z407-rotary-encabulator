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
        println!("Adapter ready: {:?}", adapter.address());

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
            let addr = adv_device.device.address();
            println!("Scanned device: addr={:?}", addr);
            if let Some(name) = adv_device.device.name() {
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
        let resp_char_clone = resp_char.clone();
        tokio::spawn(async move {
            if let Ok(mut notifs) = resp_char_clone.notify().await {
                println!("Notifications enabled");
                while let Some(data) = notifs.next().await {
                    if let Ok(data) = data {
                        let hex = hex::encode(data);
                        println!("Response: {}", hex);
                        let _ = resp_tx_clone.send(hex);
                    }
                }
            } else {
                eprintln!("Failed to enable notifications");
            }
        });

        // Handshake
        println!("Sending INITIATE (84 05)");
        cmd_char.write(&[0x84, 0x05]).await?;
        sleep(Duration::from_millis(200)).await;  // Give time for response
        println!("Sending ACKNOWLEDGE (84 00)");
        cmd_char.write(&[0x84, 0x00]).await?;
        sleep(Duration::from_millis(200)).await;
        println!("Handshake complete");

        // Set connected
        let mut s = state.lock().unwrap();
        s.connected = true;
        s.current_input = "Bluetooth".to_string();
        drop(s);

        println!("Entering command loop");

        // Command loop
        loop {
            if let Ok(cmd) = cmd_rx.recv_timeout(Duration::from_millis(100)) {
                println!("Sending cmd: {:?}", cmd);
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

    fn test_ping(&mut self) {
        self.send_cmd(&[0x85, 0x00]);  // UNKNOWN_1 - silent test
    }

    // ... (rest of methods unchanged: volume_up, etc.)
}

impl eframe::App for Z407PuckApp {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        // Poll responses (for future parsing, e.g., confirm vol up)
        let s = self.state.lock().unwrap();
        if let Some(ref rx) = s.resp_rx {
            while let Ok(resp_hex) = rx.try_recv() {
                println!("Response: {}", resp_hex);  // Already logged in spawn, but for UI sync
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
                    // ... (volume/bass/media/input buttons unchanged)

                    ui.add_space(5.0);
                    // Test ping button
                    if ui.button("Test Ping (Silent)").clicked() {
                        self.test_ping();
                    }
                }

                // ... (status label unchanged)
            });
        });
    }
}

// ... (main unchanged)