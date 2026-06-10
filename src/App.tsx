import React, { useState, useEffect, useRef } from "react";
import { invoke, Channel } from "@tauri-apps/api/core";
import { 
  Monitor, 
  Terminal, 
  Copy, 
  Check, 
  StopCircle, 
  X, 
  Tv, 
  Lock, 
  Wifi, 
  ChevronRight 
} from "lucide-react";

interface Resolution {
  width: number;
  height: number;
}

type Mode = "menu" | "host" | "viewer";

export default function App() {
  const [mode, setMode] = useState<Mode>("menu");
  const [hostCode, setHostCode] = useState("");
  const [viewerCode, setViewerCode] = useState("");
  const [isConnected, setIsConnected] = useState(false);
  const [isConnecting, setIsConnecting] = useState(false);
  const [frame, setFrame] = useState<string | null>(null);
  const [remoteResolution, setRemoteResolution] = useState<Resolution | null>(null);
  const [copied, setCopied] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const imageRef = useRef<HTMLImageElement>(null);

  // Copy host code to clipboard
  const handleCopyCode = async () => {
    try {
      await navigator.clipboard.writeText(hostCode);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    } catch (err) {
      console.error("Failed to copy code: ", err);
    }
  };

  // Start hosting screen
  const startHosting = async () => {
    setError(null);
    try {
      const code = await invoke<string>("rdp_start_host");
      setHostCode(code);
      setMode("host");
    } catch (err: any) {
      setError(err?.toString() || "Failed to start hosting");
    }
  };

  // Stop hosting screen
  const stopHosting = async () => {
    try {
      await invoke("rdp_stop_host");
    } catch (err) {
      console.error(err);
    }
    setHostCode("");
    setMode("menu");
  };

  // Connect to a host
  const connectToHost = async () => {
    if (!viewerCode.trim()) return;
    setError(null);
    setFrame(null);
    setRemoteResolution(null);
    setIsConnected(false);
    setIsConnecting(true);
    setMode("viewer");

    try {
      const channel = new Channel<any>();
      channel.onmessage = (msg: any) => {
        if (msg.type === "init") {
          setRemoteResolution({ width: msg.width, height: msg.height });
          setIsConnected(true);
          setIsConnecting(false);
        } else if (msg.type === "frame") {
          setFrame(msg.data);
        }
      };

      await invoke("rdp_connect_viewer", { hostAddr: viewerCode, channel });
    } catch (err: any) {
      setError(err?.toString() || "Failed to connect to host");
      setFrame(null);
      setRemoteResolution(null);
      setIsConnected(false);
      setIsConnecting(false);
      setMode("menu");
    }
  };

  // Disconnect from viewer session
  const disconnectViewer = async () => {
    try {
      await invoke("rdp_stop_viewer");
    } catch (err) {
      console.error(err);
    }
    setViewerCode("");
    setFrame(null);
    setRemoteResolution(null);
    setIsConnected(false);
    setIsConnecting(false);
    setMode("menu");
  };

  // Listen to keyboard inputs in viewer mode
  useEffect(() => {
    if (mode !== "viewer" || !isConnected) return;

    const handleKeyDown = (e: KeyboardEvent) => {
      // Prevent browser keys like Tab, Cmd+R, Backspace navigation
      e.preventDefault();
      invoke("rdp_send_input", {
        event: {
          type: "key",
          key: e.key,
          down: true,
        }
      }).catch(console.error);
    };

    const handleKeyUp = (e: KeyboardEvent) => {
      e.preventDefault();
      invoke("rdp_send_input", {
        event: {
          type: "key",
          key: e.key,
          down: false,
        }
      }).catch(console.error);
    };

    window.addEventListener("keydown", handleKeyDown);
    window.addEventListener("keyup", handleKeyUp);

    return () => {
      window.removeEventListener("keydown", handleKeyDown);
      window.removeEventListener("keyup", handleKeyUp);
    };
  }, [mode, isConnected]);

  // Handle Mouse Events for Viewer
  const handleMouseMove = (e: React.MouseEvent<HTMLImageElement>) => {
    if (!remoteResolution || !imageRef.current) return;
    const rect = imageRef.current.getBoundingClientRect();
    
    const x = e.clientX - rect.left;
    const y = e.clientY - rect.top;

    const scaledX = (x / rect.width) * remoteResolution.width;
    const scaledY = (y / rect.height) * remoteResolution.height;

    invoke("rdp_send_input", {
      event: {
        type: "mouse_move",
        x: scaledX,
        y: scaledY,
      }
    }).catch(console.error);
  };

  const handleMouseDown = (e: React.MouseEvent<HTMLImageElement>) => {
    e.preventDefault();
    if (!remoteResolution || !imageRef.current) return;

    const buttonMap: Record<number, string> = {
      0: "left",
      1: "middle",
      2: "right",
    };
    const buttonName = buttonMap[e.button] || "left";

    handleMouseMove(e);

    invoke("rdp_send_input", {
      event: {
        type: "mouse_click",
        button: buttonName,
        down: true,
      }
    }).catch(console.error);
  };

  const handleMouseUp = (e: React.MouseEvent<HTMLImageElement>) => {
    e.preventDefault();
    if (!remoteResolution || !imageRef.current) return;

    const buttonMap: Record<number, string> = {
      0: "left",
      1: "middle",
      2: "right",
    };
    const buttonName = buttonMap[e.button] || "left";

    invoke("rdp_send_input", {
      event: {
        type: "mouse_click",
        button: buttonName,
        down: false,
      }
    }).catch(console.error);
  };

  const handleContextMenu = (e: React.MouseEvent) => {
    e.preventDefault();
  };

  return (
    <div className="app-container">
      {/* Top Header */}
      {mode !== "viewer" && (
        <header className="app-header">
          <div className="header-logo">
            <div className="logo-icon">
              <Tv className="h-5 w-5 text-white" />
            </div>
            <div className="logo-text">
              <h1>Syntro Remote</h1>
              <p>P2P Encrypted Remote Desktop</p>
            </div>
          </div>
          <div className="status-indicator">
            <span className="status-dot"></span>
            <span>P2P Network Active</span>
          </div>
        </header>
      )}

      {/* Main Container */}
      <main className="app-main">
        {error && (
          <div className="error-alert">
            <span style={{ height: "8px", width: "8px", borderRadius: "50%", background: "var(--accent-red)", flexShrink: 0 }}></span>
            <div style={{ flex: 1 }}>{error}</div>
            <button onClick={() => setError(null)} className="error-close">
              <X className="h-4 w-4" />
            </button>
          </div>
        )}

        {/* 1. Menu Selection */}
        {mode === "menu" && (
          <div className="menu-grid animate-fadeIn">
            {/* Host Card */}
            <div className="app-card">
              <div>
                <div className="card-icon">
                  <Monitor className="h-6 w-6" />
                </div>
                <h2>Share Your Screen</h2>
                <p>
                  Host a secure remote desktop session. Share your unique endpoint ID to let others view and control your desktop.
                </p>
              </div>
              <button
                onClick={startHosting}
                className="app-button"
              >
                <span>Start Sharing</span>
                <ChevronRight className="h-4 w-4" />
              </button>
            </div>

            {/* Viewer Card */}
            <div className="app-card">
              <div>
                <div className="card-icon viewer">
                  <Terminal className="h-6 w-6" />
                </div>
                <h2>Control Remote Desktop</h2>
                <p>
                  Paste the host's session ID to establish a high-performance, low-latency control tunnel.
                </p>
                <input
                  type="text"
                  placeholder="Enter Session ID..."
                  value={viewerCode}
                  onChange={(e) => setViewerCode(e.target.value)}
                  className="app-input"
                />
              </div>
              <button
                onClick={connectToHost}
                className="app-button viewer"
              >
                <span>Establish Connection</span>
                <ChevronRight className="h-4 w-4" />
              </button>
            </div>
          </div>
        )}

        {/* 2. Host Screen Info Panel */}
        {mode === "host" && (
          <div className="info-panel animate-fadeIn">
            <div className="info-icon">
              <Wifi className="h-8 w-8" />
            </div>
            <h2>Screen Sharing Active</h2>
            <p>
              Copy the session ID below and share it with the remote viewer. Keep this window open.
            </p>

            {/* Session ID display */}
            <div className="code-box">
              <span className="code-box-label">Your Session ID</span>
              <div className="code-box-content">
                <span className="code-text">{hostCode}</span>
                <button
                  onClick={handleCopyCode}
                  className="copy-btn"
                  title="Copy Code"
                >
                  {copied ? <Check className="h-4 w-4 text-emerald-400" style={{ color: "var(--accent-green)" }} /> : <Copy className="h-4 w-4" />}
                </button>
              </div>
            </div>

            {/* Status information */}
            <div style={{ width: "100%", display: "flex", justifyContent: "space-between", fontSize: "11px", color: "var(--muted)", borderTop: "1px solid var(--border)", paddingTop: "20px" }}>
              <span style={{ display: "flex", alignItems: "center", gap: "6px" }}>
                <Lock className="h-3.5 w-3.5" style={{ color: "var(--accent)" }} />
                End-to-End Encrypted
              </span>
              <span>30 FPS H.264 stream</span>
            </div>

            <button
              onClick={stopHosting}
              className="stop-btn"
              style={{ marginTop: "24px" }}
            >
              <StopCircle className="h-4 w-4" />
              <span>Stop Sharing</span>
            </button>
          </div>
        )}

        {/* 3. Fullscreen Remote Desktop Viewer */}
        {mode === "viewer" && (
          <div className="viewer-container">
            {/* Controls header */}
            <div className="viewer-header">
              <div className="viewer-info">
                <div className="viewer-info-dot"></div>
                <span className="viewer-info-text">
                  Connected to Remote Desktop
                </span>
                {remoteResolution && (
                  <span className="viewer-info-res">
                    {remoteResolution.width}x{remoteResolution.height}
                  </span>
                )}
              </div>
              <button
                onClick={disconnectViewer}
                className="viewer-disconnect-btn"
              >
                <X className="h-3.5 w-3.5" />
                <span>Disconnect</span>
              </button>
            </div>

            {/* Display/Streaming viewport */}
            <div className="viewer-content">
              {isConnecting && (
                <div className="viewer-loader">
                  <div className="viewer-spinner"></div>
                  <p>Connecting to remote host...</p>
                </div>
              )}

              {!isConnecting && !isConnected && (
                <div className="viewer-loader">
                  <div className="viewer-spinner"></div>
                  <p>Buffering stream...</p>
                </div>
              )}

              {isConnected && !frame && (
                <p style={{ color: "var(--muted)" }}>Waiting for remote display feed...</p>
              )}

              {isConnected && frame && (
                <img
                  ref={imageRef}
                  src={frame}
                  alt="Remote Desktop"
                  onMouseMove={handleMouseMove}
                  onMouseDown={handleMouseDown}
                  onMouseUp={handleMouseUp}
                  onContextMenu={handleContextMenu}
                  className="viewer-image"
                  draggable={false}
                />
              )}
            </div>
          </div>
        )}
      </main>
    </div>
  );
}
