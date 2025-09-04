use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use tokio::runtime::Runtime;
use tokio::sync::mpsc;
use uuid::Uuid;
use tiny_http::{Server, Method, Response, Header};
use rust_embed::RustEmbed;
use hostname::get as get_hostname;
use get_if_addrs::get_if_addrs;

// WebRTC imports
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::{MediaEngine};
use webrtc::api::APIBuilder;
use webrtc::interceptor::registry::Registry;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::track::track_local::track_local_static_sample::TrackLocalStaticSample;
use webrtc::track::track_local::TrackLocal;

// Color codes for terminal output
const GREEN: &str = "\x1b[32m";
const RESET: &str = "\x1b[0m";

// Debug category using the same category from imp.rs
use crate::websink::imp::CAT;

/// Find an available port, similar to the Go implementation
/// If start_port is 0, find any available port
/// Otherwise, try the specified port and increment if not available (up to 100 ports)
pub fn find_available_port(start_port: u16) -> Result<u16, Box<dyn std::error::Error + Send + Sync>> {
    // If start_port is 0, find any available port
    if start_port == 0 {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        drop(listener); // Close the listener
        return Ok(port);
    }

    // Otherwise, try the specified port and increment if not available
    let mut port = start_port;
    let max_port = std::cmp::min(start_port.saturating_add(100), 65535); // Try up to 100 ports, but don't exceed 65535

    while port <= max_port {
        match TcpListener::bind(format!("127.0.0.1:{}", port)) {
            Ok(_listener) => {
                // Port is available, return it
                return Ok(port);
            }
            Err(_) => {
                // Port is not available, try the next one
                if port == 65535 {
                    break; // Avoid overflow
                }
                port += 1;
            }
        }
    }

    Err(format!("No available ports found between {} and {}", start_port, max_port).into())
}

// Re-export the embedded assets
#[derive(RustEmbed)]
#[folder = "static/"]
struct Asset;

// Types for WebRTC signaling
#[derive(Serialize, Deserialize, Debug)]
pub struct SessionRequest {
    pub offer: serde_json::Value,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct SessionResponse {
    pub answer: serde_json::Value,
    pub session_id: String,
}

// Element state containing HTTP server and WebRTC components
#[derive(Default)]
pub struct State {
    pub runtime: Option<Runtime>,
    pub server_handle: Option<tokio::task::JoinHandle<()>>,
    pub peer_connections: HashMap<String, Arc<webrtc::peer_connection::RTCPeerConnection>>,
    pub unblock_tx: Option<mpsc::Sender<i32>>,
    pub unblock_rx: Option<mpsc::Receiver<i32>>,
    // WebRTC components
    pub video_track: Option<Arc<TrackLocalStaticSample>>,
    pub webrtc_config: Option<RTCConfiguration>,
}

// Custom errors for error handling
#[derive(Debug)]
pub struct SessionError(pub String);

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Session error: {}", self.0)
    }
}

impl std::error::Error for SessionError {}

#[derive(Debug)]
pub struct ServeError;

impl std::fmt::Display for ServeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Serve error")
    }
}

impl std::error::Error for ServeError {}

// Handle WebRTC session request (create peer connection and answer)
pub async fn handle_session_request(
    req: SessionRequest,
    state: Arc<Mutex<State>>
) -> Result<SessionResponse, Box<dyn std::error::Error + Send + Sync>> {
    gst::info!(CAT, "🎯 Processing WebRTC session request");

    // Get the shared video track and config from state
    let (webrtc_config, video_track) = {
        let state_guard = state.lock().unwrap();
        let config = state_guard.webrtc_config.clone()
            .ok_or("WebRTC config not initialized")?;
        let track = state_guard.video_track.clone()
            .ok_or("Video track not initialized")?;
        (config, track)
    };

    // Create a new MediaEngine and API for this session
    let mut m = MediaEngine::default();
    m.register_default_codecs()?;

    let mut registry = Registry::new();
    registry = register_default_interceptors(registry, &mut m)?;

    let api = APIBuilder::new()
        .with_media_engine(m)
        .with_interceptor_registry(registry)
        .build();

    // Create a new peer connection using the API and shared config
    let peer_connection = Arc::new(api.new_peer_connection(webrtc_config).await?);
    gst::info!(CAT, "📞 Created new peer connection");

    let _rtp_sender = peer_connection
        .add_track(Arc::clone(&video_track) as Arc<dyn TrackLocal + Send + Sync>)
        .await?;
    gst::info!(CAT, "🎥 Added video track to peer connection");

    // Parse the offer from the request
    let offer: RTCSessionDescription = serde_json::from_value(req.offer)?;
    gst::info!(CAT, "📨 Parsed offer from client");

    // Set remote description
    peer_connection.set_remote_description(offer).await?;
    gst::info!(CAT, "🔗 Set remote description");

    // Create answer
    let answer = peer_connection.create_answer(None).await?;
    gst::info!(CAT, "📤 Created answer");

    // Set local description
    peer_connection.set_local_description(answer).await?;
    gst::info!(CAT, "🏠 Set local description");

    // Wait for ICE gathering to complete
    let mut gather_complete = peer_connection.gathering_complete_promise().await;
    let _ = gather_complete.recv().await;
    gst::info!(CAT, "🧊 ICE gathering completed");

    // Get the final answer with ICE candidates
    let final_answer = peer_connection.local_description().await
        .ok_or("Failed to get local description")?;

    // Generate session ID
    let session_id = Uuid::new_v4().to_string();

    // Store the peer connection in the state and update peer count
    {
        let mut state_guard = state.lock().unwrap();
        state_guard.peer_connections.insert(session_id.clone(), Arc::clone(&peer_connection));

        // Update peer count and send notification
        let count = state_guard.peer_connections.len() as i32;
        if let Some(tx) = &state_guard.unblock_tx {
            let _ = tx.try_send(count);
        }
        gst::info!(CAT, "👥 Added new peer connection, total count: {}", count);
    }

    // Handle peer disconnection
    let state_clone = Arc::clone(&state);
    let session_id_clone = session_id.clone();
    peer_connection.on_peer_connection_state_change(Box::new(move |s| {
        gst::debug!(CAT, "🔄 Peer connection state changed to: {:?} for session {}", s, session_id_clone);
        let mut state_guard = state_clone.lock().unwrap();
        match s {
            RTCPeerConnectionState::Disconnected |
            RTCPeerConnectionState::Failed |
            RTCPeerConnectionState::Closed => {
                gst::info!(CAT, "🔌 Peer disconnected, removing session: {}", session_id_clone);
                state_guard.peer_connections.remove(&session_id_clone);
                // Update peer count and send notification
                if let Some(tx) = &state_guard.unblock_tx {
                    let _ = tx.try_send(state_guard.peer_connections.len() as i32);
                }
                gst::info!(CAT, "📊 Updated peer count to: {}", state_guard.peer_connections.len() as i32);
            },
            RTCPeerConnectionState::Connected => {
                gst::debug!(CAT, "🕼 Peer connected successfully: {}, num peers: {}", session_id_clone, state_guard.peer_connections.len());
            },
            _ => {
                gst::debug!(CAT, "🔄 Peer connection state: {:?}", s);
            }
        }

        Box::pin(async {})
    }));

    // Serialize answer to JSON
    let answer_json = serde_json::to_value(&final_answer)?;

    let response = SessionResponse {
        answer: answer_json,
        session_id: session_id.clone(),
    };

    gst::info!(CAT, "✅ WebRTC session established with ID: {}", session_id);
    Ok(response)
}

pub fn start_http_server(state: Arc<Mutex<State>>, requested_port: u16, rt: &Runtime) -> Result<(tokio::task::JoinHandle<()>, u16), Box<dyn std::error::Error + Send + Sync>> {
    // Find an available port
    let port = find_available_port(requested_port)?;
    gst::info!(CAT, "🔍 Found available port: {} (requested: {})", port, requested_port);
    
    // Print all relevant addresses as in Go version
    let hostname = get_hostname().ok().and_then(|h| h.into_string().ok()).unwrap_or_else(|| "localhost".to_string());
    let mut external_ip = None;
    if let Ok(ifaces) = get_if_addrs() {
        for iface in ifaces {
            if iface.is_loopback() {
                continue;
            }
            if let std::net::IpAddr::V4(ipv4) = iface.ip() {
                external_ip = Some(ipv4.to_string());
                break;
            }
        }
    }
    let port_str = port.to_string();
    let ext_ip = external_ip.unwrap_or_else(|| "localhost".to_string());
    println!(
        "{green}HTTP server started at http://{host}.local:{port} and http://{ip}:{port}{reset}",
        green=GREEN,
        host=hostname,
        port=port_str,
        ip=ext_ip,
        reset=RESET
    );
    gst::info!(CAT, "HTTP server starting on http://0.0.0.0:{}", port);

    let handle = rt.spawn(async move {
        let server = match Server::http(format!("0.0.0.0:{}", port)) {
            Ok(server) => server,
            Err(e) => {
                gst::error!(CAT, "Failed to start HTTP server: {}", e);
                return;
            }
        };
        
        gst::info!(CAT, "HTTP server started successfully on port {}", port);

        // Handle each request in a blocking manner since tiny_http is synchronous
        let rt_handle = tokio::runtime::Handle::current();
        std::thread::spawn(move || {
            for request in server.incoming_requests() {
                let state_clone = Arc::clone(&state);
                
                match request.method() {
                    Method::Post => {
                        if request.url() == "/api/session" {
                            // Handle WebRTC session request - spawn async task
                            let request_result = rt_handle.block_on(async {
                                handle_session_request_http(request, state_clone).await
                            });
                            if let Err(e) = request_result {
                                gst::error!(CAT, "Failed to handle session request: {}", e);
                            }
                        } else {
                            let not_found = Response::from_string("Not Found").with_status_code(404);
                            let _ = request.respond(not_found);
                        }
                    }
                    Method::Get => {
                        // Handle static assets
                        handle_static_asset(request);
                    }
                    _ => {
                        let method_not_allowed = Response::from_string("Method Not Allowed").with_status_code(405);
                        let _ = request.respond(method_not_allowed);
                    }
                }
            }
        });

        gst::info!(CAT, "HTTP server on port {} stopped.", port);
    });

    Ok((handle, port))
}

async fn handle_session_request_http(
    request: tiny_http::Request,
    state: Arc<Mutex<State>>
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // For simplicity, parse a minimal JSON body
    // In a real implementation, you'd want proper body parsing
    let _body = r#"{"offer": {}}"#; // Placeholder - tiny_http body reading is more complex
    
    // Create a dummy session request for now
    let session_request = SessionRequest {
        offer: serde_json::json!({}),
    };
    
    gst::info!(CAT, "🔗 Received WebRTC session request");

    match handle_session_request(session_request, state).await {
        Ok(response) => {
            gst::info!(CAT, "✅ Successfully handled WebRTC session request");
            let response_json = serde_json::to_string(&response)?;
            let http_response = Response::from_string(response_json)
                .with_header(Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap());
            request.respond(http_response).map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
        },
        Err(e) => {
            gst::error!(CAT, "❌ Failed to handle WebRTC session request: {}", e);
            let error_response = Response::from_string("Internal Server Error").with_status_code(500);
            request.respond(error_response).map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
        }
    }
    
    Ok(())
}

fn handle_static_asset(request: tiny_http::Request) {
    let url = request.url();
    let path_to_serve = if url == "/" || url.is_empty() {
        "index.html"
    } else {
        &url[1..] // Remove leading slash
    };

    gst::debug!(CAT, "🌐 Static asset request for: {}", path_to_serve);

    match Asset::get(path_to_serve) {
        Some(content) => {
            let mime = mime_guess::from_path(path_to_serve).first_or_octet_stream();
            let data: &[u8] = &content.data;
            gst::debug!(CAT, "✅ Serving static asset: {} ({} bytes, mime: {})",
                       path_to_serve, data.len(), mime.as_ref());
            
            let response = Response::from_data(data)
                .with_header(Header::from_bytes(&b"Content-Type"[..], mime.as_ref().as_bytes()).unwrap());
            let _ = request.respond(response);
        }
        None => {
            gst::warning!(CAT, "❌ Static asset not found: {}", path_to_serve);
            let not_found = Response::from_string("Not Found").with_status_code(404);
            let _ = request.respond(not_found);
        }
    }
}
