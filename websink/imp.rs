use gst::glib;
use gst::prelude::*;
use gst::subclass::prelude::*;
use gst_base::subclass::prelude::*;

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::LazyLock;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes;
use tokio::runtime::Runtime;
use tokio::sync::mpsc;
use webrtc::api::media_engine::{MIME_TYPE_H264, MIME_TYPE_HEVC, MIME_TYPE_VP8, MIME_TYPE_VP9};
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::media::Sample;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability;
use webrtc::track::track_local::track_local_static_rtp::TrackLocalStaticRTP;
use webrtc::track::track_local::track_local_static_sample::TrackLocalStaticSample;
use webrtc::track::track_local::TrackLocalWriter;

// Import from our server module
use crate::websink::server;

// Debug category for the WebSink element
pub static CAT: LazyLock<gst::DebugCategory> =
    LazyLock::new(|| gst::DebugCategory::new("websink", gst::DebugColorFlags::empty(), Some("webrtc streaming sink element")));

// Video codec enumeration for multi-codec support
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoCodec {
    H264,
    H265,
    VP8,
    VP9,
}

// Stream mode enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamMode {
    Sample,
    Rtp,
}

impl VideoCodec {
    /// Get the MIME type for WebRTC
    pub fn mime_type(&self) -> &'static str {
        match self {
            VideoCodec::H264 => MIME_TYPE_H264,
            VideoCodec::H265 => MIME_TYPE_HEVC,
            VideoCodec::VP8 => MIME_TYPE_VP8,
            VideoCodec::VP9 => MIME_TYPE_VP9,
        }
    }

    /// Detect codec and stream mode from GStreamer caps
    pub fn from_caps(caps: &gst::Caps) -> Option<(Self, StreamMode)> {
        let structure = caps.structure(0)?;
        let name = structure.name();

        match name.as_str() {
            "video/x-h264" => Some((VideoCodec::H264, StreamMode::Sample)),
            "video/x-h265" => Some((VideoCodec::H265, StreamMode::Sample)),
            "video/x-vp8" => Some((VideoCodec::VP8, StreamMode::Sample)),
            "video/x-vp9" => Some((VideoCodec::VP9, StreamMode::Sample)),
            "application/x-rtp" => {
                let encoding_name = structure.get::<String>("encoding-name").ok()?;
                match encoding_name.as_str() {
                    "H264" => Some((VideoCodec::H264, StreamMode::Rtp)),
                    "H265" => Some((VideoCodec::H265, StreamMode::Rtp)),
                    "VP8" => Some((VideoCodec::VP8, StreamMode::Rtp)),
                    "VP9" => Some((VideoCodec::VP9, StreamMode::Rtp)),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// Get human-readable codec name
    pub fn name(&self) -> &'static str {
        match self {
            VideoCodec::H264 => "H.264",
            VideoCodec::H265 => "H.265/HEVC",
            VideoCodec::VP8 => "VP8",
            VideoCodec::VP9 => "VP9",
        }
    }
}

// Default values for properties
const DEFAULT_PORT: u16 = 8091;
const DEFAULT_STUN_SERVER: &str = "stun:stun.l.google.com:19302";

// Property value storage
#[derive(Debug, Clone)]
struct Settings {
    port: u16,
    stun_server: String,
    is_live: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self { port: DEFAULT_PORT, stun_server: String::from(DEFAULT_STUN_SERVER), is_live: false }
    }
}

// Use the State type from server module
type State = server::State;

// Element that keeps track of everything
pub struct WebSink {
    settings: Mutex<Settings>,
    state: Arc<Mutex<State>>,
    render_count: AtomicU32,
}

// Default implementation for our element
impl Default for WebSink {
    fn default() -> Self {
        Self { settings: Mutex::new(Settings::default()), state: Arc::new(Mutex::new(State::default())), render_count: AtomicU32::new(0) }
    }
}

// Implementation of GObject virtual methods for our element
#[glib::object_subclass]
impl ObjectSubclass for WebSink {
    const NAME: &'static str = "WebSink";
    type Type = super::WebSink;
    type ParentType = gst_base::BaseSink;
}

// Implementation of GObject methods
impl ObjectImpl for WebSink {
    fn properties() -> &'static [glib::ParamSpec] {
        use once_cell::sync::Lazy;
        static PROPERTIES: Lazy<Vec<glib::ParamSpec>> = Lazy::new(|| {
            vec![
                glib::ParamSpecUInt::builder("port")
                    .nick("HTTP Port")
                    .blurb("Port to use for the HTTP server (0 for auto)")
                    .minimum(0)
                    .maximum(65535)
                    .default_value(DEFAULT_PORT as u32)
                    .build(),
                glib::ParamSpecString::builder("stun-server")
                    .nick("STUN Server")
                    .blurb("STUN server to use for WebRTC (empty for none)")
                    .default_value(DEFAULT_STUN_SERVER)
                    .build(),
                glib::ParamSpecBoolean::builder("is-live")
                    .nick("Live Mode")
                    .blurb("Whether to block Render without peers (default: false)")
                    .default_value(false)
                    .build(),
            ]
        });

        PROPERTIES.as_ref()
    }

    fn set_property(&self, _id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
        match pspec.name() {
            "port" => {
                let mut settings = self.settings.lock().unwrap();
                let port = value.get::<u32>().expect("type checked upstream") as u16;
                gst::info!(CAT, "Changing port from {} to {}", settings.port, port);
                settings.port = port;
            }
            "stun-server" => {
                let mut settings = self.settings.lock().unwrap();
                let stun_server =
                    value.get::<Option<String>>().expect("type checked upstream").unwrap_or_else(|| DEFAULT_STUN_SERVER.to_string());
                gst::info!(CAT, "Changing stun-server from {} to {}", settings.stun_server, stun_server);
                settings.stun_server = stun_server;
            }
            "is-live" => {
                let mut settings = self.settings.lock().unwrap();
                let is_live = value.get::<bool>().expect("type checked upstream");
                gst::info!(CAT, "Changing is-live from {} to {}", settings.is_live, is_live);
                settings.is_live = is_live;
            }
            _ => unimplemented!(),
        }
    }

    fn property(&self, _id: usize, pspec: &glib::ParamSpec) -> glib::Value {
        match pspec.name() {
            "port" => {
                let settings = self.settings.lock().unwrap();
                glib::Value::from(&(settings.port as u32))
            }
            "stun-server" => {
                let settings = self.settings.lock().unwrap();
                settings.stun_server.to_value()
            }
            "is-live" => {
                let settings = self.settings.lock().unwrap();
                settings.is_live.to_value()
            }
            _ => unimplemented!(),
        }
    }
}

// Implementation of GstObject methods
impl GstObjectImpl for WebSink {}

// Implementation of Element methods
impl ElementImpl for WebSink {
    fn metadata() -> Option<&'static gst::subclass::ElementMetadata> {
        use once_cell::sync::Lazy;
        static ELEMENT_METADATA: Lazy<gst::subclass::ElementMetadata> = Lazy::new(|| {
            gst::subclass::ElementMetadata::new(
                "WebRTC Sink",
                "Sink/Network",
                "Stream H.264/H.265/VP8/VP9 video to web browsers using WebRTC. Supports both raw encoded streams and RTP packets with auto-detection.",
                "Videology Inc <info@videology.com>",
            )
        });

        Some(&*ELEMENT_METADATA)
    }

    fn pad_templates() -> &'static [gst::PadTemplate] {
        use once_cell::sync::Lazy;
        static PAD_TEMPLATES: Lazy<Vec<gst::PadTemplate>> = Lazy::new(|| {
            // Raw encoded caps
            let h264_caps = gst::Caps::builder("video/x-h264").field("stream-format", "byte-stream").field("alignment", "au").build();
            let h265_caps = gst::Caps::builder("video/x-h265").field("stream-format", "byte-stream").field("alignment", "au").build();
            let vp8_caps = gst::Caps::builder("video/x-vp8").build();
            let vp9_caps = gst::Caps::builder("video/x-vp9").build();

            // RTP caps for all supported codecs
            let rtp_caps = gst::Caps::builder("application/x-rtp")
                .field("media", "video")
                .field("encoding-name", gst::List::new(["H264", "H265", "VP8", "VP9"]))
                .field("clock-rate", 90000)
                .build();

            let mut combined_caps = h264_caps;
            combined_caps.merge(h265_caps);
            combined_caps.merge(vp8_caps);
            combined_caps.merge(vp9_caps);
            combined_caps.merge(rtp_caps);

            let sink_pad_template =
                gst::PadTemplate::new("sink", gst::PadDirection::Sink, gst::PadPresence::Always, &combined_caps).unwrap();

            vec![sink_pad_template]
        });

        PAD_TEMPLATES.as_ref()
    }
}

// Implementation of BaseSink methods
impl BaseSinkImpl for WebSink {
    fn set_caps(&self, caps: &gst::Caps) -> Result<(), gst::LoggableError> {
        gst::info!(CAT, "🎯 Setting caps: {}", caps);

        // Detect codec and stream mode from caps
        let (codec, mode) =
            VideoCodec::from_caps(caps).ok_or_else(|| gst::loggable_error!(CAT, "Unsupported video format in caps: {}", caps))?;

        gst::info!(CAT, "🎥 Detected codec: {} in {:?} mode", codec.name(), mode);

        // Create or update video track if we have a runtime
        let state_guard = self.state.lock().unwrap();
        if state_guard.runtime.is_some() {
            drop(state_guard);
            self.create_video_track(codec, mode)?;
        }

        Ok(())
    }

    fn start(&self) -> Result<(), gst::ErrorMessage> {
        gst::info!(CAT, "🚀 Starting WebSink");

        // Initialize Tokio runtime
        gst::debug!(CAT, "⚙️ Initializing Tokio runtime");
        let runtime = match Runtime::new() {
            Ok(rt) => {
                gst::info!(CAT, "✅ Tokio runtime created successfully");
                rt
            }
            Err(err) => {
                gst::error!(CAT, "❌ Failed to create Tokio runtime: {}", err);
                return Err(gst::error_msg!(gst::ResourceError::Failed, ["Failed to create Tokio runtime: {}", err]));
            }
        };

        // Setup an unblock channel for live mode
        let (tx, rx) = mpsc::channel(1);
        gst::info!(CAT, "📺 Created mpsc channel for live mode signaling");

        gst::info!(CAT, "✅ WebRTC components will be initialized per session");

        // Note: Video track will be created in set_caps when codec is detected
        gst::debug!(CAT, "📋 Video track will be created when codec is detected from caps");

        // Configure WebRTC
        let settings = self.settings.lock().unwrap();
        let mut webrtc_config = RTCConfiguration::default();
        if !settings.stun_server.is_empty() {
            webrtc_config.ice_servers = vec![RTCIceServer { urls: vec![settings.stun_server.clone()], ..Default::default() }];
            gst::info!(CAT, "🌐 STUN server configured: {}", settings.stun_server);
        } else {
            gst::info!(CAT, "⚠️ No STUN server configured");
        }
        let port = settings.port;
        drop(settings);

        let mut state = self.state.lock().unwrap();
        state.runtime = Some(runtime);
        state.unblock_tx = Some(tx);
        state.unblock_rx = Some(rx);
        state.webrtc_config = Some(webrtc_config);

        // Start HTTP server
        gst::info!(CAT, "🌐 Starting HTTP server on port {}", port);
        let rt = state.runtime.as_ref().expect("Runtime should be initialized");
        match self.start_http_server(port, rt) {
            Ok((server_handle, actual_port)) => {
                gst::info!(CAT, "✅ HTTP server started successfully on port {}", actual_port);
                state.server_handle = Some(server_handle);
                // Store the actual port used for future reference if needed
                drop(state);
            }
            Err(err) => {
                gst::error!(CAT, "❌ Failed to start HTTP server: {}", err);
                drop(state);
                return Err(gst::error_msg!(gst::ResourceError::Failed, ["Failed to start HTTP server: {}", err]));
            }
        }
        gst::info!(CAT, "✅ WebSink started successfully");

        Ok(())
    }

    fn stop(&self) -> Result<(), gst::ErrorMessage> {
        gst::info!(CAT, "🛑 Stopping WebSink");

        // Clean up resources
        let mut state = self.state.lock().unwrap();

        // Stop the HTTP server
        if let Some(handle) = state.server_handle.take() {
            gst::info!(CAT, "🌐 Aborting HTTP server task...");
            handle.abort();
            gst::info!(CAT, "✅ HTTP server task aborted.");
        } else {
            gst::debug!(CAT, "🌐 No HTTP server handle to abort");
        }

        // Clear peer connections
        let peer_count = state.peer_connections.len();
        state.peer_connections.clear();
        gst::info!(CAT, "👥 Cleared {} peer connections", peer_count);

        // Reset state
        state.unblock_tx = None;
        state.unblock_rx = None;
        state.runtime = None;
        state.video_track = None;
        state.webrtc_config = None;
        gst::debug!(CAT, "🧹 Reset all state components");

        gst::info!(CAT, "✅ WebSink stopped successfully");
        Ok(())
    }

    fn render(&self, buffer: &gst::Buffer) -> Result<gst::FlowSuccess, gst::FlowError> {
        let (num_peers, is_live) = {
            let state_guard = self.state.lock().unwrap();
            let settings_guard = self.settings.lock().unwrap();
            (state_guard.peer_connections.len(), settings_guard.is_live)
        };
        let render_count = self.render_count.fetch_add(1, Ordering::Relaxed);

        if render_count % 600 == 0 {
            gst::trace!(CAT, "🎬 Render called - buffer size: {} bytes, peers: {}", buffer.size(), num_peers);
        }

        if is_live && num_peers == 0 {
            if (render_count % 600) == 0 {
                gst::trace!(CAT, "⏭️ No peers connected in live mode, skipping buffer");
            }
            return Ok(gst::FlowSuccess::Ok);
        }

        let map = buffer.map_readable().map_err(|_| {
            gst::error!(CAT, "❌ Failed to map buffer");
            gst::FlowError::Error
        })?;

        let data = map.as_slice();

        if num_peers > 0 {
            let state = self.state.lock().unwrap();
            if let Some(video_track) = &state.video_track {
                match video_track {
                    server::VideoTrack::Sample(track) => {
                        let track_clone = Arc::clone(track);
                        let data_copy = bytes::Bytes::copy_from_slice(data);
                        let duration = buffer.duration().unwrap_or_else(|| gst::ClockTime::from_nseconds(33_333_333));

                        if let Some(runtime) = &state.runtime {
                            runtime.spawn(async move {
                                let sample =
                                    Sample { data: data_copy, duration: Duration::from_nanos(duration.nseconds()), ..Default::default() };

                                if let Err(e) = track_clone.write_sample(&sample).await {
                                    gst::error!(CAT, "❌ Failed to write sample: {}", e);
                                }
                            });
                        }
                    }
                    server::VideoTrack::Rtp(track) => {
                        let track_clone = Arc::clone(track);
                        let data_copy = data.to_vec();

                        if let Some(runtime) = &state.runtime {
                            runtime.spawn(async move {
                                use util::Unmarshal;

                                let mut buf = &data_copy[..];
                                match rtp::packet::Packet::unmarshal(&mut buf) {
                                    Ok(rtp_packet) => {
                                        if let Err(e) = track_clone.write_rtp(&rtp_packet).await {
                                            gst::error!(CAT, "❌ Failed to write RTP packet: {}", e);
                                        }
                                    }
                                    Err(e) => {
                                        gst::error!(CAT, "❌ Failed to parse RTP packet: {}", e);
                                    }
                                }
                            });
                        }
                    }
                }
            }
        }

        Ok(gst::FlowSuccess::Ok)
    }
}

impl WebSink {
    /// Create video track for the specified codec and mode
    fn create_video_track(&self, codec: VideoCodec, mode: StreamMode) -> Result<(), gst::LoggableError> {
        gst::debug!(CAT, "🎥 Creating video track for {} in {:?} mode", codec.name(), mode);

        let mut state = self.state.lock().unwrap();

        let video_track = match mode {
            StreamMode::Sample => {
                let track = Arc::new(TrackLocalStaticSample::new(
                    RTCRtpCodecCapability { mime_type: codec.mime_type().to_owned(), ..Default::default() },
                    "video".to_owned(),
                    "websink".to_owned(),
                ));
                gst::info!(CAT, "✅ Sample mode video track created for {}", codec.name());
                server::VideoTrack::Sample(track)
            }
            StreamMode::Rtp => {
                let track = Arc::new(TrackLocalStaticRTP::new(
                    RTCRtpCodecCapability { mime_type: codec.mime_type().to_owned(), ..Default::default() },
                    "video".to_owned(),
                    "websink".to_owned(),
                ));
                gst::info!(CAT, "✅ RTP mode video track created for {}", codec.name());
                server::VideoTrack::Rtp(track)
            }
        };

        state.video_track = Some(video_track);

        Ok(())
    }

    fn start_http_server(
        &self,
        port: u16,
        rt: &Runtime,
    ) -> Result<(tokio::task::JoinHandle<()>, u16), Box<dyn std::error::Error + Send + Sync>> {
        gst::info!(CAT, "Starting HTTP server on port {}", port);

        // Clone the state Arc to move into the async block
        let state = Arc::clone(&self.state);

        // Use the server module's start_http_server function
        server::start_http_server(state, port, rt)
    }
}
