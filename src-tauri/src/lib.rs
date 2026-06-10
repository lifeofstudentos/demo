mod session;

use std::sync::Arc;
use tokio::sync::Mutex as AsyncMutex;
use iroh_net::endpoint::SendStream;

pub mod commands {
    use super::*;
    use tauri::{AppHandle, State, ipc::Channel};
    use tokio::sync::oneshot;
    use std::sync::Arc;
    use tokio::sync::Mutex as AsyncMutex;

    #[tauri::command]
    pub async fn rdp_start_host(
        app_handle: AppHandle,
        state: State<'_, crate::session::AppState>,
    ) -> Result<String, String> {
        let mut host_opt = state.host_session.lock().await;
        if host_opt.is_some() {
            return Err("Hosting session already running".into());
        }

        let (cancel_tx, cancel_rx) = oneshot::channel();
        let (node_addr_tx, node_addr_rx) = oneshot::channel();

        let app_handle_clone = app_handle.clone();
        let state_clone = state.host_session.clone();
        tokio::spawn(async move {
            if let Err(e) = crate::session::start_host_loop(app_handle_clone, cancel_rx, node_addr_tx).await {
                log::error!("Host loop error: {:?}", e);
            }
            let mut h_opt = state_clone.lock().await;
            *h_opt = None;
        });

        let node_addr = node_addr_rx.await.map_err(|_| "Failed to start host endpoint".to_string())?;

        *host_opt = Some(crate::session::HostSession {
            cancel_tx,
            node_addr: node_addr.clone(),
        });

        Ok(node_addr)
    }

    #[tauri::command]
    pub async fn rdp_stop_host(state: State<'_, crate::session::AppState>) -> Result<(), String> {
        let mut host_opt = state.host_session.lock().await;
        if let Some(session) = host_opt.take() {
            let _ = session.cancel_tx.send(());
            Ok(())
        } else {
            Err("No active host session".into())
        }
    }

    #[tauri::command]
    pub async fn rdp_connect_viewer(
        host_addr: String,
        channel: Channel<serde_json::Value>,
        state: State<'_, crate::session::AppState>,
    ) -> Result<(), String> {
        // Check for existing session without holding the lock across the await below.
        {
            let viewer_opt = state.viewer_session.lock().await;
            if viewer_opt.is_some() {
                return Err("Viewer session already running".into());
            }
        } // lock released here

        let (cancel_tx, cancel_rx) = oneshot::channel();
        let (send_stream_tx, send_stream_rx) = oneshot::channel();

        let state_clone = state.viewer_session.clone();
        tokio::spawn(async move {
            if let Err(e) = crate::session::run_viewer_loop(host_addr, channel, cancel_rx, send_stream_tx).await {
                log::error!("Viewer loop error: {:?}", e);
            }
            let mut v_opt = state_clone.lock().await;
            *v_opt = None;
        });

        // Await the stream without holding the mutex — this allows rdp_stop_viewer
        // to acquire the lock and cancel if the user disconnects during connection.
        let send_stream: SendStream = send_stream_rx.await.map_err(|_| "Failed to connect to host".to_string())?;

        // Re-acquire the lock to store the established session.
        let mut viewer_opt = state.viewer_session.lock().await;
        *viewer_opt = Some(crate::session::ViewerSession {
            cancel_tx,
            send_stream: Arc::new(AsyncMutex::new(send_stream)),
        });

        Ok(())
    }

    #[tauri::command]
    pub async fn rdp_stop_viewer(state: State<'_, crate::session::AppState>) -> Result<(), String> {
        let mut viewer_opt = state.viewer_session.lock().await;
        if let Some(session) = viewer_opt.take() {
            let _ = session.cancel_tx.send(());
            Ok(())
        } else {
            Err("No active viewer session".into())
        }
    }

    #[tauri::command]
    pub async fn rdp_send_input(
        event: crate::session::InputEvent,
        state: State<'_, crate::session::AppState>,
    ) -> Result<(), String> {
        let viewer_opt = state.viewer_session.lock().await;
        if let Some(ref session) = *viewer_opt {
            let payload = serde_json::to_vec(&event).map_err(|e| e.to_string())?;
            let len = payload.len() as u32;
            let mut send_stream = session.send_stream.lock().await;
            send_stream.write_all(&len.to_be_bytes()).await.map_err(|e| e.to_string())?;
            send_stream.write_all(&payload).await.map_err(|e| e.to_string())?;
            Ok(())
        } else {
            Err("No active viewer session".into())
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .manage(session::AppState {
            host_session: Arc::new(AsyncMutex::new(None)),
            viewer_session: Arc::new(AsyncMutex::new(None)),
        })
        .invoke_handler(tauri::generate_handler![
            commands::rdp_start_host,
            commands::rdp_stop_host,
            commands::rdp_connect_viewer,
            commands::rdp_stop_viewer,
            commands::rdp_send_input
        ])
        .setup(|app| {
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
