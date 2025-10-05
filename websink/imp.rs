use gst::glib;
use gst::prelude::*;
use gst::subclass::prelude::*;

use std::collections::HashMap;
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

#[derive(Debug, Clone)]
struct PadInfo {
    codec: VideoCodec,
    mode: StreamMode,
    track_index: usize,
}

pub struct WebSink {
    settings: Mutex<Settings>,
    state: Arc<Mutex<State>>,
    pad_counter: AtomicU32,
    pad_info: Mutex<HashMap<String, PadInfo>>,
}

impl Default for WebSink {
    fn default() -> Self {
        Self {
            settings: Mutex::new(Settings::default()),
            state: Arc::new(Mutex::new(State::default())),
            pad_counter: AtomicU32::new(0),
            pad_info: Mutex::new(HashMap::new()),
        }
    }
}

#[glib::object_subclass]
impl ObjectSubclass for WebSink {
    const NAME: &'static str = "WebSink";
    type Type = super::WebSink;
    type ParentType = gst::Element;
}

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
                gst::PadTemplate::new("sink_%u", gst::PadDirection::Sink, gst::PadPresence::Request, &combined_caps).unwrap();

            vec![sink_pad_template]
        });

        PAD_TEMPLATES.as_ref()
    }

    fn request_new_pad(
        &self,
        templ: &gst::PadTemplate,
        _name: Option<&str>,
        _caps: Option<&gst::Caps>,
    ) -> Option<gst::Pad> {
        let pad_id = self.pad_counter.fetch_add(1, Ordering::Relaxed);
        let pad_name = format!("sink_{}", pad_id);

        gst::info!(CAT, "Creating new pad: {}", pad_name);

        let pad = gst::Pad::builder_from_template(templ)
            .name(&pad_name)
            .chain_function(|pad, parent, buffer| {
                WebSink::catch_panic_pad_function(
                    parent,
                    || Err(gst::FlowError::Error),
                    |websink| websink.sink_chain(pad, buffer),
                )
            })
            .event_function(|pad, parent, event| {
                WebSink::catch_panic_pad_function(
                    parent,
                    || false,
                    |websink| websink.sink_event(pad, event),
                )
            })
            .build();

        pad.set_active(true).ok()?;

        let element = self.obj();
        element.add_pad(&pad).ok()?;

        gst::info!(CAT, "Pad {} created successfully", pad_name);
        Some(pad)
    }

    fn release_pad(&self, pad: &gst::Pad) {
        gst::info!(CAT, "Releasing pad: {}", pad.name());

        let mut pad_info = self.pad_info.lock().unwrap();
        if let Some(info) = pad_info.remove(pad.name().as_str()) {
            gst::debug!(CAT, "Removed track at index {} for pad {}", info.track_index, pad.name());
        }

        let element = self.obj();
        element.remove_pad(pad).ok();
    }

    fn change_state(
        &self,
        transition: gst::StateChange,
    ) -> Result<gst::StateChangeSuccess, gst::StateChangeError> {
        match transition {
            gst::StateChange::NullToReady => {
                self.start().map_err(|e| {
                    gst::error!(CAT, "Failed to start: {}", e);
                    gst::StateChangeError
                })?;
            }
            gst::StateChange::ReadyToNull => {
                self.stop().map_err(|e| {
                    gst::error!(CAT, "Failed to stop: {}", e);
                    gst::StateChangeError
                })?;
            }
            _ => {}
        }

        self.parent_change_state(transition)
    }
}

impl WebSink {
    fn sink_chain(&self, pad: &gst::Pad, buffer: gst::Buffer) -> Result<gst::FlowSuccess, gst::FlowError> {
        let pad_name = pad.name();

        let (num_peers, is_live, track_index) = {
            let state_guard = self.state.lock().unwrap();
            let settings_guard = self.settings.lock().unwrap();
            let pad_info_guard = self.pad_info.lock().unwrap();

            let track_idx = pad_info_guard.get(pad_name.as_str()).map(|info| info.track_index);
            (state_guard.peer_connections.len(), settings_guard.is_live, track_idx)
        };

        if is_live && num_peers == 0 {
            return Ok(gst::FlowSuccess::Ok);
        }

        let Some(track_index) = track_index else {
            gst::error!(CAT, "❌ No track info for pad {}", pad_name);
            return Err(gst::FlowError::Error);
        };

        gst::trace!(CAT, "📦 Pad {} sending buffer to track {} (size: {}, peers: {})", pad_name, track_index, buffer.size(), num_peers);

        let map = buffer.map_readable().map_err(|_| {
            gst::error!(CAT, "Failed to map buffer");
            gst::FlowError::Error
        })?;

        let data = map.as_slice();

        if num_peers > 0 {
            let state = self.state.lock().unwrap();
            if let Some(video_track) = state.video_tracks.get(track_index) {
                match video_track {
                    server::VideoTrack::Sample(track) => {
                        let track_clone = Arc::clone(track);
                        let data_copy = bytes::Bytes::copy_from_slice(data);
                        let duration = buffer.duration().unwrap_or_else(|| gst::ClockTime::from_nseconds(33_333_333));
                        let pad_name_clone = pad_name.to_string();

                        if let Some(runtime) = &state.runtime {
                            runtime.spawn(async move {
                                let sample = Sample { data: data_copy, duration: Duration::from_nanos(duration.nseconds()), ..Default::default() };

                                match track_clone.write_sample(&sample).await {
                                    Ok(_) => {
                                        gst::trace!(CAT, "✅ Wrote sample to track from pad {}", pad_name_clone);
                                    }
                                    Err(e) => {
                                        gst::error!(CAT, "❌ Failed to write sample from pad {}: {}", pad_name_clone, e);
                                    }
                                }
                            });
                        }
                    }
                    server::VideoTrack::Rtp(track) => {
                        let track_clone = Arc::clone(track);
                        let data_copy = data.to_vec();
                        let pad_name_clone = pad_name.to_string();

                        if let Some(runtime) = &state.runtime {
                            runtime.spawn(async move {
                                use util::Unmarshal;

                                let mut buf = &data_copy[..];
                                match rtp::packet::Packet::unmarshal(&mut buf) {
                                    Ok(rtp_packet) => {
                                        match track_clone.write_rtp(&rtp_packet).await {
                                            Ok(_) => {
                                                gst::trace!(CAT, "✅ Wrote RTP to track from pad {}", pad_name_clone);
                                            }
                                            Err(e) => {
                                                gst::error!(CAT, "❌ Failed to write RTP from pad {}: {}", pad_name_clone, e);
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        gst::error!(CAT, "❌ Failed to parse RTP packet from pad {}: {}", pad_name_clone, e);
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

    fn sink_event(&self, pad: &gst::Pad, event: gst::Event) -> bool {
        use gst::EventView;

        match event.view() {
            EventView::Caps(e) => {
                let caps = e.caps_owned();
                gst::info!(CAT, "Setting caps on pad {}: {}", pad.name(), caps);

                let (codec, mode) = match VideoCodec::from_caps(&caps) {
                    Some(c) => c,
                    None => {
                        gst::error!(CAT, "Unsupported caps: {}", caps);
                        return false;
                    }
                };

                gst::info!(CAT, "Detected codec: {} in {:?} mode", codec.name(), mode);

                if let Err(e) = self.create_video_track_for_pad(pad, codec, mode) {
                    gst::error!(CAT, "Failed to create track: {}", e);
                    return false;
                }

                true
            }
            _ => gst::Pad::event_default(pad, Some(&*self.obj()), event),
        }
    }

    fn create_video_track_for_pad(&self, pad: &gst::Pad, codec: VideoCodec, mode: StreamMode) -> Result<(), gst::LoggableError> {
        gst::debug!(CAT, "Creating video track for pad {} with {} in {:?} mode", pad.name(), codec.name(), mode);

        let mut state = self.state.lock().unwrap();

        let track_id = format!("track_{}", pad.name());
        let stream_id = format!("stream_{}", pad.name());

        let video_track = match mode {
            StreamMode::Sample => {
                let track = Arc::new(TrackLocalStaticSample::new(
                    RTCRtpCodecCapability { mime_type: codec.mime_type().to_owned(), ..Default::default() },
                    track_id.clone(),
                    stream_id.clone(),
                ));
                gst::info!(CAT, "✅ Sample mode video track created for {} (track: {}, stream: {})", codec.name(), track_id, stream_id);
                server::VideoTrack::Sample(track)
            }
            StreamMode::Rtp => {
                let track = Arc::new(TrackLocalStaticRTP::new(
                    RTCRtpCodecCapability { mime_type: codec.mime_type().to_owned(), ..Default::default() },
                    track_id.clone(),
                    stream_id.clone(),
                ));
                gst::info!(CAT, "RTP mode video track created for {} (track: {}, stream: {})", codec.name(), track_id, stream_id);
                server::VideoTrack::Rtp(track)
            }
        };

        let track_index = state.video_tracks.len();
        state.video_tracks.push(video_track);
        drop(state);

        let mut pad_info = self.pad_info.lock().unwrap();
        pad_info.insert(pad.name().to_string(), PadInfo { codec, mode, track_index });

        gst::info!(CAT, "Pad {} mapped to track index {}", pad.name(), track_index);

        Ok(())
    }

    fn start(&self) -> Result<(), gst::ErrorMessage> {
        gst::info!(CAT, "Starting WebSink");

        let runtime = match Runtime::new() {
            Ok(rt) => {
                gst::info!(CAT, "Tokio runtime created successfully");
                rt
            }
            Err(err) => {
                gst::error!(CAT, "Failed to create Tokio runtime: {}", err);
                return Err(gst::error_msg!(gst::ResourceError::Failed, ["Failed to create Tokio runtime: {}", err]));
            }
        };

        let (tx, rx) = mpsc::channel(1);
        gst::info!(CAT, "Created mpsc channel for live mode signaling");

        let settings = self.settings.lock().unwrap();
        let mut webrtc_config = RTCConfiguration::default();
        if !settings.stun_server.is_empty() {
            webrtc_config.ice_servers = vec![RTCIceServer { urls: vec![settings.stun_server.clone()], ..Default::default() }];
            gst::info!(CAT, "STUN server configured: {}", settings.stun_server);
        }
        let port = settings.port;
        drop(settings);

        let mut state = self.state.lock().unwrap();
        state.runtime = Some(runtime);
        state.unblock_tx = Some(tx);
        state.unblock_rx = Some(rx);
        state.webrtc_config = Some(webrtc_config);

        gst::info!(CAT, "Starting HTTP server on port {}", port);
        let rt = state.runtime.as_ref().expect("Runtime should be initialized");
        match self.start_http_server(port, rt) {
            Ok((server_handle, actual_port)) => {
                gst::info!(CAT, "HTTP server started successfully on port {}", actual_port);
                state.server_handle = Some(server_handle);
            }
            Err(err) => {
                gst::error!(CAT, "Failed to start HTTP server: {}", err);
                return Err(gst::error_msg!(gst::ResourceError::Failed, ["Failed to start HTTP server: {}", err]));
            }
        }
        gst::info!(CAT, "WebSink started successfully");

        Ok(())
    }

    fn stop(&self) -> Result<(), gst::ErrorMessage> {
        gst::info!(CAT, "Stopping WebSink");

        let mut state = self.state.lock().unwrap();

        if let Some(handle) = state.server_handle.take() {
            gst::info!(CAT, "Aborting HTTP server task...");
            handle.abort();
        }

        let peer_count = state.peer_connections.len();
        state.peer_connections.clear();
        gst::info!(CAT, "Cleared {} peer connections", peer_count);

        state.unblock_tx = None;
        state.unblock_rx = None;
        state.runtime = None;
        state.video_tracks.clear();
        state.webrtc_config = None;

        drop(state);

        let mut pad_info = self.pad_info.lock().unwrap();
        pad_info.clear();

        gst::info!(CAT, "WebSink stopped successfully");
        Ok(())
    }

    fn start_http_server(
        &self,
        port: u16,
        rt: &Runtime,
    ) -> Result<(tokio::task::JoinHandle<()>, u16), Box<dyn std::error::Error + Send + Sync>> {
        gst::info!(CAT, "Starting HTTP server on port {}", port);
        let state = Arc::clone(&self.state);
        server::start_http_server(state, port, rt)
    }
}
