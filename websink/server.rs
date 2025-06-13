use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::runtime::Runtime;
use tokio::sync::mpsc;
use uuid::Uuid;
use warp::Filter;
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
pub struct SessionError();
impl warp::reject::Reject for SessionError {}

#[derive(Debug)]
pub struct ServeError;
impl warp::reject::Reject for ServeError {}

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

        match s {
            RTCPeerConnectionState::Disconnected |
            RTCPeerConnectionState::Failed |
            RTCPeerConnectionState::Closed => {
                gst::info!(CAT, "🔌 Peer disconnected, removing session: {}", session_id_clone);

                // Remove the peer connection from state
                if let Ok(mut state_guard) = state_clone.lock() {
                    state_guard.peer_connections.remove(&session_id_clone);

                    // Update peer count and send notification
                    let count = state_guard.peer_connections.len() as i32;
                    if let Some(tx) = &state_guard.unblock_tx {
                        let _ = tx.try_send(count);
                    }

                    gst::info!(CAT, "📊 Updated peer count to: {}", count);
                } else {
                    gst::error!(CAT, "❌ Failed to lock state for peer disconnection cleanup");
                }
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

pub fn start_http_server(state: Arc<Mutex<State>>, port: u16, rt: &Runtime) -> tokio::task::JoinHandle<()> {
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

    rt.spawn(async move {
        // API session handler - now with actual WebRTC signaling
        let api_session = warp::path!("api" / "session")
            .and(warp::post())
            .and(warp::body::json())
            .and(warp::any().map(move || Arc::clone(&state)))
            .and_then(|body: SessionRequest, state: Arc<Mutex<State>>| async move {
                gst::info!(CAT, "🔗 Received WebRTC session request");
                gst::debug!(CAT, "📨 Session request body: {:?}", body);

                match handle_session_request(body, state).await {
                    Ok(response) => {
                        gst::info!(CAT, "✅ Successfully handled WebRTC session request");
                        Ok(warp::reply::json(&response))
                    },
                    Err(e) => {
                        gst::error!(CAT, "❌ Failed to handle WebRTC session request: {}", e);
                        Err(warp::reject::custom(SessionError()))
                    }
                }
            });

        let static_assets = warp::path::tail().and_then(|tail: warp::path::Tail| async move {
            let path = tail.as_str();
            let path_to_serve = if path.is_empty() || path == "/" {
                "index.html"
            } else {
                path
            };

            gst::debug!(CAT, "🌐 Static asset request for: {}", path_to_serve);

            match Asset::get(path_to_serve) {
                Some(content) => {
                    let mime = mime_guess::from_path(path_to_serve).first_or_octet_stream();
                    let body: Cow<'static, [u8]> = content.data;
                    gst::debug!(CAT, "✅ Serving static asset: {} ({} bytes, mime: {})",
                               path_to_serve, body.len(), mime.as_ref());
                    let response = warp::http::Response::builder()
                        .header("Content-Type", mime.as_ref())
                        .body(body)
                        .map_err(|_| warp::reject::custom(ServeError))?;
                    Ok(response)
                }
                None => {
                    gst::warning!(CAT, "❌ Static asset not found: {}", path_to_serve);
                    Err(warp::reject::not_found())
                }
            }
        });

        let routes = api_session.or(static_assets);

        warp::serve(routes).run(([0, 0, 0, 0], port)).await;
        gst::info!(CAT, "HTTP server on port {} stopped.", port);
    })
}
