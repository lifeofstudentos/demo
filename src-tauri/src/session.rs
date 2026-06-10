#![allow(deprecated)]

use std::sync::Arc;
use tokio::sync::{Mutex as AsyncMutex, oneshot};
use tauri::{AppHandle, ipc::Channel};
use anyhow::{Result, anyhow};
use scap::capturer::{Capturer, Options};
use scap::frame::Frame;
use openh264::encoder::Encoder as H264Encoder;
use openh264::decoder::Decoder as H264Decoder;
use openh264::formats::{BgraSliceU8, YUVBuffer, YUVSource};
use jpeg_encoder::{Encoder as JpegEncoder, ColorType};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use enigo::{Enigo, Mouse, Keyboard, Settings, Coordinate, Button, Key, Direction};
use iroh_net::{Endpoint, NodeAddr};
use iroh_net::endpoint::{SendStream, RecvStream, Connection};
use serde::{Serialize, Deserialize};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ScreenSize {
    pub width: u32,
    pub height: u32,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type")]
pub enum InputEvent {
    #[serde(rename = "mouse_move")]
    MouseMove { x: f64, y: f64 },
    #[serde(rename = "mouse_click")]
    MouseClick { button: String, down: bool },
    #[serde(rename = "key")]
    Key { key: String, down: bool },
}

pub struct HostSession {
    pub cancel_tx: oneshot::Sender<()>,
    pub node_addr: String,
}

pub struct ViewerSession {
    pub cancel_tx: oneshot::Sender<()>,
    pub send_stream: Arc<AsyncMutex<SendStream>>,
}

pub struct AppState {
    pub host_session: Arc<AsyncMutex<Option<HostSession>>>,
    pub viewer_session: Arc<AsyncMutex<Option<ViewerSession>>>,
}

pub async fn start_host_loop(
    _app_handle: AppHandle,
    cancel_rx: oneshot::Receiver<()>,
    node_addr_tx: oneshot::Sender<String>,
) -> Result<()> {
    // Bind the endpoint
    let endpoint: Endpoint = Endpoint::builder()
        .alpns(vec![b"syntro-rdp".to_vec()])
        .bind()
        .await?;

    let node_addr = endpoint.node_addr().await?;
    let node_addr_str = serde_json::to_string(&node_addr)?;

    // Send the address back to the caller
    let _ = node_addr_tx.send(node_addr_str);

    // We need to cancel both the accept-wait phase and the session phase with the same signal.
    // Wrap cancel_rx in an Arc<Mutex<Option<...>>> so we can share it, or simply use a
    // watch channel. Here we use a simple approach: spawn a task that waits for cancel_rx
    // and then sends on two inner channels.
    let (cancel_accept_tx, cancel_accept_rx) = oneshot::channel::<()>();
    let (cancel_session_tx, cancel_session_rx) = oneshot::channel::<()>();

    tokio::spawn(async move {
        let _ = cancel_rx.await;
        let _ = cancel_accept_tx.send(());
        let _ = cancel_session_tx.send(());
    });

    // Listen for a connection or cancel
    let maybe_conn: Option<Connection> = tokio::select! {
        res = endpoint.accept() => {
            let incoming: iroh_net::endpoint::Incoming = res.ok_or_else(|| anyhow!("Accept failed: endpoint closed"))?;
            Some(incoming.await?)
        }
        _ = cancel_accept_rx => {
            None
        }
    };

    if let Some(conn) = maybe_conn {
        run_sharing_session(conn, cancel_session_rx).await?;
    }

    Ok(())
}

async fn run_sharing_session(
    conn: Connection,
    cancel_rx: oneshot::Receiver<()>,
) -> Result<()> {
    // Accept bidirectional stream from viewer
    let (send_stream, mut recv_stream): (SendStream, RecvStream) = conn.accept_bi().await.map_err(|e| anyhow!("Accept stream failed: {}", e))?;

    // Check permissions
    if !scap::is_supported() {
        return Err(anyhow!("Screen capture not supported on this platform"));
    }
    if !scap::has_permission() {
        scap::request_permission();
    }

    let (frame_tx, mut frame_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(30);
    let (cancel_sender_tx, mut cancel_sender_rx) = tokio::sync::mpsc::channel::<()>(1);

    // Spawn CPU-heavy screen capture & encoding thread
    std::thread::spawn(move || {
        let options = Options {
            fps: 30,
            target: None,
            show_cursor: true,
            show_highlight: false,
            output_type: scap::frame::FrameType::BGRAFrame,
            ..Default::default()
        };

        let mut capturer = match Capturer::build(options) {
            Ok(c) => c,
            Err(e) => {
                log::error!("Failed to build capturer: {:?}", e);
                return;
            }
        };
        capturer.start_capture();

        let mut encoder: Option<H264Encoder> = None;

        loop {
            if cancel_sender_rx.try_recv().is_ok() {
                break;
            }

            let frame = match capturer.get_next_frame() {
                Ok(f) => f,
                Err(e) => {
                    log::error!("Error capturing frame: {:?}", e);
                    std::thread::sleep(std::time::Duration::from_millis(30));
                    continue;
                }
            };

            match frame {
                Frame::Video(scap::frame::VideoFrame::BGRA(frame_data)) => {
                    let width = frame_data.width as usize;
                    let height = frame_data.height as usize;

                    if encoder.is_none() {
                        match H264Encoder::new() {
                            Ok(enc) => {
                                encoder = Some(enc);
                                // Send initial resolution metadata to client
                                let init_msg = serde_json::json!({
                                    "type": "init",
                                    "width": width,
                                    "height": height,
                                });
                                let init_bytes = serde_json::to_vec(&init_msg).unwrap();
                                if frame_tx.blocking_send(init_bytes).is_err() {
                                    break;
                                }
                            }
                            Err(e) => {
                                log::error!("Failed to init encoder: {:?}", e);
                                break;
                            }
                        }
                    }

                    if let Some(ref mut enc) = encoder {
                        let bgra_slice = BgraSliceU8::new(&frame_data.data, (width, height));
                        let yuv_buf = YUVBuffer::from_rgb_source(bgra_slice);
                        match enc.encode(&yuv_buf) {
                            Ok(bitstream) => {
                                let bytes = bitstream.to_vec();
                                if !bytes.is_empty() {
                                    if frame_tx.blocking_send(bytes).is_err() {
                                        break;
                                    }
                                }
                            }
                            Err(e) => {
                                log::error!("Encoding error: {:?}", e);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        capturer.stop_capture();
    });

    // Spawn network sender task
    let mut send_stream_clone: SendStream = send_stream;
    let sender_task = tokio::spawn(async move {
        while let Some(bytes) = frame_rx.recv().await {
            let len = bytes.len() as u32;
            if send_stream_clone.write_all(&len.to_be_bytes()).await.is_err() {
                break;
            }
            if send_stream_clone.write_all(&bytes).await.is_err() {
                break;
            }
        }
    });

    // Input injection: Enigo is !Send, so it must live on a dedicated OS thread.
    // The async receiver reads from the network and forwards events via a channel.
    let (input_tx, input_rx) = std::sync::mpsc::channel::<InputEvent>();

    std::thread::spawn(move || {
        let mut enigo = match Enigo::new(&Settings::default()) {
            Ok(e) => e,
            Err(err) => {
                log::error!("Failed to init Enigo: {:?}", err);
                return;
            }
        };
        while let Ok(event) = input_rx.recv() {
            match event {
                InputEvent::MouseMove { x, y } => {
                    let _ = enigo.move_mouse(x as i32, y as i32, Coordinate::Abs);
                }
                InputEvent::MouseClick { button, down } => {
                    let btn = match button.as_str() {
                        "left" => Button::Left,
                        "right" => Button::Right,
                        "middle" => Button::Middle,
                        _ => Button::Left,
                    };
                    let dir = if down { Direction::Press } else { Direction::Release };
                    let _ = enigo.button(btn, dir);
                }
                InputEvent::Key { key, down } => {
                    let dir = if down { Direction::Press } else { Direction::Release };
                    let enigo_key = match key.as_str() {
                        "Return" | "Enter" => Key::Return,
                        "Space" | " " => Key::Space,
                        "Backspace" => Key::Backspace,
                        "Escape" => Key::Escape,
                        "Tab" => Key::Tab,
                        "Shift" => Key::Shift,
                        "Control" => Key::Control,
                        "Alt" => Key::Alt,
                        "Meta" => Key::Meta,
                        "ArrowUp" => Key::UpArrow,
                        "ArrowDown" => Key::DownArrow,
                        "ArrowLeft" => Key::LeftArrow,
                        "ArrowRight" => Key::RightArrow,
                        other => {
                            if other.chars().count() == 1 {
                                Key::Unicode(other.chars().next().unwrap())
                            } else {
                                continue;
                            }
                        }
                    };
                    let _ = enigo.key(enigo_key, dir);
                }
            }
        }
    });

    // Async task: read input events from the network and forward to the Enigo thread.
    let receiver_task = async {
        loop {
            let mut len_bytes = [0u8; 4];
            if recv_stream.read_exact(&mut len_bytes).await.is_err() {
                break;
            }
            let len = u32::from_be_bytes(len_bytes) as usize;

            let mut payload = vec![0u8; len];
            if recv_stream.read_exact(&mut payload).await.is_err() {
                break;
            }

            if let Ok(event) = serde_json::from_slice::<InputEvent>(&payload) {
                if input_tx.send(event).is_err() {
                    break;
                }
            }
        }
    };

    tokio::select! {
        _ = cancel_rx => {
            let _ = cancel_sender_tx.send(()).await;
        }
        _ = receiver_task => {
            let _ = cancel_sender_tx.send(()).await;
        }
    }

    let _ = sender_task.await;
    Ok(())
}

pub async fn run_viewer_loop(
    host_addr_str: String,
    channel: Channel<serde_json::Value>,
    cancel_rx: oneshot::Receiver<()>,
    send_stream_tx: oneshot::Sender<SendStream>,
) -> Result<()> {
    let host_addr: NodeAddr = serde_json::from_str(&host_addr_str)?;

    let endpoint: Endpoint = Endpoint::builder()
        .bind()
        .await?;

    let conn: Connection = endpoint.connect(host_addr, b"syntro-rdp").await?;

    let (mut send_stream, mut recv_stream): (SendStream, RecvStream) = conn.open_bi().await?;
    send_stream.write_all(&0u32.to_be_bytes()).await?;
    let _ = send_stream_tx.send(send_stream);

    let mut decoder = H264Decoder::new()?;

    let receive_loop = async {
        loop {
            let mut len_bytes = [0u8; 4];
            if recv_stream.read_exact(&mut len_bytes).await.is_err() {
                break;
            }
            let len = u32::from_be_bytes(len_bytes) as usize;

            let mut payload = vec![0u8; len];
            if recv_stream.read_exact(&mut payload).await.is_err() {
                break;
            }

            // Check for initial resolution metadata
            if payload.starts_with(b"{\"type\":\"init\"") {
                if let Ok(init_val) = serde_json::from_slice::<serde_json::Value>(&payload) {
                    let _ = channel.send(init_val);
                }
                continue;
            }

            // Decode frame
            if let Ok(Some(yuv)) = decoder.decode(&payload) {
                let (width, height) = yuv.dimensions();
                let mut rgb_raw = vec![0u8; yuv.estimate_rgb_u8_size()];
                yuv.write_rgb8(&mut rgb_raw);

                // Compress RGB raw to JPEG for optimal transfer size
                let mut jpeg_bytes = Vec::new();
                let jpeg_enc = JpegEncoder::new(&mut jpeg_bytes, 70);
                if jpeg_enc.encode(&rgb_raw, width as u16, height as u16, ColorType::Rgb).is_ok() {
                    let b64 = STANDARD.encode(&jpeg_bytes);
                    let data_url = format!("data:image/jpeg;base64,{}", b64);
                    
                    let msg = serde_json::json!({
                        "type": "frame",
                        "data": data_url,
                    });
                    let _ = channel.send(msg);
                }
            }
        }
    };

    tokio::select! {
        _ = cancel_rx => {}
        _ = receive_loop => {}
    }

    Ok(())
}
