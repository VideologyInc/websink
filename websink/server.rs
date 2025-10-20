use axum::{
    extract::State as AxumState,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use get_if_addrs::get_if_addrs;
use hostname::get as get_hostname;
use rust_embed::RustEmbed;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use tokio::runtime::Runtime;
use tokio::sync::mpsc;
use uuid::Uuid;

// WebRTC imports
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::MediaEngine;
use webrtc::api::APIBuilder;
use webrtc::interceptor::registry::Registry;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::track::track_local::track_local_static_rtp::TrackLocalStaticRTP;
use webrtc::track::track_local::track_local_static_sample::TrackLocalStaticSample;
use webrtc::track::track_local::TrackLocal;

// Color codes for terminal output
const GREEN: &str = "\x1b[32m";
const RESET: &str = "\x1b[0m";

// Debug category using the same category from imp.rs
use crate::websink::imp::CAT;

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
    pub negotiated_codec: Option<String>,
}
// Video track enum to support both Sample and RTP modes
#[derive(Clone)]
pub enum VideoTrack {
    Sample(Arc<TrackLocalStaticSample>),
    Rtp(Arc<TrackLocalStaticRTP>),
}

impl VideoTrack {
    pub fn as_track_local(&self) -> Arc<dyn TrackLocal + Send + Sync> {
        match self {
            VideoTrack::Sample(track) => Arc::clone(track) as Arc<dyn TrackLocal + Send + Sync>,
            VideoTrack::Rtp(track) => Arc::clone(track) as Arc<dyn TrackLocal + Send + Sync>,
        }
    }

    pub fn codec_mime_type(&self) -> String {
        match self {
            VideoTrack::Sample(track) => track.codec().clone().mime_type,
            VideoTrack::Rtp(track) => track.codec().clone().mime_type,
        }
    }
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
    pub video_track: Option<VideoTrack>,
    pub webrtc_config: Option<RTCConfiguration>,
}

// Handle WebRTC session request (create peer connection and answer)
pub async fn handle_session_request(
    req: SessionRequest,
    state: Arc<Mutex<State>>,
) -> Result<SessionResponse, Box<dyn std::error::Error + Send + Sync>> {
    gst::info!(CAT, "🎯 Processing WebRTC session request");

    // Get the shared video track and config from state
    let (webrtc_config, video_track) = {
        let state_guard = state.lock().unwrap();
        let config = state_guard.webrtc_config.clone().ok_or("WebRTC config not initialized")?;
        let track = state_guard.video_track.clone().ok_or("Video track not initialized")?;
        (config, track)
    };

    // Detect what codec we're actually sending
    let actual_codec = video_track.codec_mime_type().to_lowercase();
    gst::info!(CAT, "🎥 Sending {} codec to client", actual_codec.to_uppercase());

    // Create a new MediaEngine and API for this session
    let mut m = MediaEngine::default();
    m.register_default_codecs()?;

    let mut registry = Registry::new();
    registry = register_default_interceptors(registry, &mut m)?;

    let api = APIBuilder::new().with_media_engine(m).with_interceptor_registry(registry).build();

    // Create a new peer connection using the API and shared config
    let peer_connection = Arc::new(api.new_peer_connection(webrtc_config).await?);
    gst::info!(CAT, "📞 Created new peer connection");

    let _rtp_sender = peer_connection.add_track(video_track.as_track_local()).await?;
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
    peer_connection.set_local_description(answer).await.map_err(|e| {
        if e.to_string().contains("codec is not supported") {
            gst::error!(
                CAT,
                "❌ It seems the codec is not supported by the browser. Ensure your GStreamer pipeline uses H.264 or H.265 codec."
            );
            let return_error_string = format!("Server is sending {}. Codec is not supported by browser.", actual_codec.to_uppercase());
            // Return a string error
            Box::<dyn std::error::Error + Send + Sync>::from(return_error_string)
        } else {
            Box::<dyn std::error::Error + Send + Sync>::from(e)
        }
    })?;
    gst::info!(CAT, "🏠 Set local description");

    // Wait for ICE gathering to complete
    let mut gather_complete = peer_connection.gathering_complete_promise().await;
    let _ = gather_complete.recv().await;
    gst::info!(CAT, "🧊 ICE gathering completed");

    // Get the final answer with ICE candidates
    let final_answer = peer_connection.local_description().await.ok_or("Failed to get local description")?;

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
            RTCPeerConnectionState::Disconnected | RTCPeerConnectionState::Failed | RTCPeerConnectionState::Closed => {
                gst::info!(CAT, "🔌 Peer disconnected, removing session: {}", session_id_clone);
                state_guard.peer_connections.remove(&session_id_clone);
                // Update peer count and send notification
                if let Some(tx) = &state_guard.unblock_tx {
                    let _ = tx.try_send(state_guard.peer_connections.len() as i32);
                }
                gst::info!(CAT, "📊 Updated peer count to: {}", state_guard.peer_connections.len() as i32);
            }
            RTCPeerConnectionState::Connected => {
                gst::debug!(CAT, "🕼 Peer connected successfully: {}, num peers: {}", session_id_clone, state_guard.peer_connections.len());
            }
            _ => {
                gst::debug!(CAT, "🔄 Peer connection state: {:?}", s);
            }
        }

        Box::pin(async {})
    }));

    // Serialize answer to JSON
    let answer_json = serde_json::to_value(&final_answer)?;

    let response = SessionResponse { answer: answer_json, session_id: session_id.clone(), negotiated_codec: Some(actual_codec.clone()) };

    gst::info!(CAT, "✅ WebRTC session established with ID: {} using codec: {}", session_id, actual_codec);
    Ok(response)
}

fn next_free_port(mut port: u16) -> u16 {
    loop {
        if TcpListener::bind(("127.0.0.1", port)).is_ok() {
            return port;
        }
        port += 1;
    }
}

async fn handle_session(
    AxumState(state): AxumState<Arc<Mutex<State>>>,
    Json(req): Json<SessionRequest>,
) -> Result<Json<SessionResponse>, AppError> {
    gst::info!(CAT, "Received WebRTC session request");
    let response = handle_session_request(req, state).await?;
    gst::info!(CAT, "Successfully handled WebRTC session request");
    Ok(Json(response))
}

async fn serve_static(uri: axum::http::Uri) -> impl IntoResponse {
    let path = uri.path().trim_start_matches('/');
    let path_to_serve = if path.is_empty() { "index.html" } else { path };

    gst::debug!(CAT, "Static asset request for: {}", path_to_serve);

    match Asset::get(path_to_serve) {
        Some(content) => {
            let mime = mime_guess::from_path(path_to_serve).first_or_octet_stream();
            let data: &[u8] = &content.data;
            gst::debug!(CAT, "Serving static asset: {} ({} bytes, mime: {})", path_to_serve, data.len(), mime.as_ref());

            Response::builder().header("Content-Type", mime.as_ref()).body(axum::body::Body::from(data.to_vec())).unwrap()
        }
        None => {
            gst::warning!(CAT, "Static asset not found: {}", path_to_serve);
            Response::builder().status(StatusCode::NOT_FOUND).body(axum::body::Body::from("Not Found")).unwrap()
        }
    }
}

struct AppError(Box<dyn std::error::Error + Send + Sync>);

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        gst::error!(CAT, "Failed to handle WebRTC session request: {}", self.0);
        (StatusCode::INTERNAL_SERVER_ERROR, self.0.to_string()).into_response()
    }
}

impl<E> From<E> for AppError
where
    E: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    fn from(err: E) -> Self {
        Self(err.into())
    }
}

pub fn start_http_server(
    state: Arc<Mutex<State>>,
    requested_port: u16,
    rt: &Runtime,
) -> Result<(tokio::task::JoinHandle<()>, u16), Box<dyn std::error::Error + Send + Sync>> {
    // Find an available port
    let port = next_free_port(requested_port);
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
        green = GREEN,
        host = hostname,
        port = port_str,
        ip = ext_ip,
        reset = RESET
    );

    let app = Router::new().route("/api/session", post(handle_session)).fallback(get(serve_static)).with_state(state);

    let addr = format!("[::]:{}", port);

    let handle = rt.spawn(async move {
        let listener = match tokio::net::TcpListener::bind(&addr).await {
            Ok(l) => l,
            Err(e) => {
                gst::error!(CAT, "Failed to bind to {}: {}", addr, e);
                return;
            }
        };

        gst::info!(CAT, "Starting HTTP server on {}", addr);
        if let Err(e) = axum::serve(listener, app).await {
            gst::error!(CAT, "HTTP server error: {}", e);
        }
        gst::info!(CAT, "HTTP server stopped");
    });

    Ok((handle, port))
}
