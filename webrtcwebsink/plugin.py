import os
import json
import asyncio
import threading
import time
import logging
from http.server import HTTPServer
import socket

import gi
gi.require_version('Gst', '1.0')
gi.require_version('GstBase', '1.0')
gi.require_version('GstWebRTC', '1.0')
gi.require_version('GstSdp', '1.0')
from gi.repository import Gst, GObject, GstWebRTC, GstSdp

from .http_server import WebRTCHTTPHandler
from .signaling import SignalingServer

# Configure logging
logging.basicConfig(level=logging.INFO)
logger = logging.getLogger('webrtcwebsink.plugin')

# Define the GObject type
class WebRTCWebSink(Gst.Bin, GObject.Object):
    """
    A GStreamer bin that acts as a WebRTC sink for streaming to web browsers.
    Includes an HTTP server for serving the client webpage and a WebSocket server
    for signaling.
    """

    # Register GObject type
    __gtype_name__ = 'WebRTCWebSink'

    # Register GStreamer plugin metadata
    __gstmetadata__ = (
        'WebRTC Web Sink',
        'Sink',
        'Stream video/audio to browsers using WebRTC',
        'Your Name'
    )

    # Register pad templates
    __gsttemplates__ = (
        Gst.PadTemplate.new(
            'sink',
            Gst.PadDirection.SINK,
            Gst.PadPresence.ALWAYS,
            Gst.Caps.from_string('video/x-raw,format={RGBA,RGB,I420,YV12,YUY2,UYVY,NV12,NV21}')
        ),
    )

    # Register properties
    __gproperties__ = {
        'port': (
            int,
            'HTTP Port',
            'Port for the HTTP server (default: 8080)',
            1,
            65535,
            8080,
            GObject.ParamFlags.READWRITE
        ),
        'ws-port': (
            int,
            'WebSocket Port',
            'Port for the WebSocket signaling server (default: 8081)',
            1,
            65535,
            8081,
            GObject.ParamFlags.READWRITE
        ),
        'bind-address': (
            str,
            'Bind Address',
            'Address to bind servers to (default: 0.0.0.0)',
            '0.0.0.0',
            GObject.ParamFlags.READWRITE
        ),
        'stun-server': (
            str,
            'STUN Server',
            'STUN server URI (default: stun://stun.l.google.com:19302)',
            'stun://stun.l.google.com:19302',
            GObject.ParamFlags.READWRITE
        ),
        'video-codec': (
            str,
            'Video Codec',
            'Video codec to use (default: vp8)',
            'vp8',
            GObject.ParamFlags.READWRITE
        ),
    }

    def __init__(self):
        Gst.Bin.__init__(self)

        # Initialize properties
        self.port = 8080
        self.ws_port = 8081
        self.bind_address = '0.0.0.0'
        self.stun_server = 'stun://stun.l.google.com:19302'
        self.video_codec = 'vp8'

        # Initialize state
        self.http_server = None
        self.http_thread = None
        self.signaling = None
        self.signaling_thread = None
        self.encoder = None
        self.payloader = None
        self.convert = None
        self.tee = None
        self.servers_started = False

        # Create internal elements
        self.setup_pipeline()

    def setup_pipeline(self):
        """Set up the internal GStreamer pipeline."""
        # Create elements
        self.convert = Gst.ElementFactory.make('videoconvert', 'convert')
        if not self.convert:
            raise Exception("Could not create videoconvert")

        # Create encoding elements based on video-codec property
        if self.video_codec == 'vp8':
            self.encoder = Gst.ElementFactory.make('vp8enc', 'encoder')
            self.payloader = Gst.ElementFactory.make('rtpvp8pay', 'payloader')
        elif self.video_codec == 'h264':
            self.encoder = Gst.ElementFactory.make('x264enc', 'encoder')
            self.payloader = Gst.ElementFactory.make('rtph264pay', 'payloader')
        else:
            raise Exception(f"Unsupported video codec: {self.video_codec}")

        # Configure elements
        if self.video_codec == 'h264':
            self.encoder.set_property('tune', 'zerolatency')
            self.encoder.set_property('speed-preset', 'ultrafast')
            self.payloader.set_property('config-interval', -1)
            self.payloader.set_property('aggregate-mode', 'zero-latency')

        # Create tee to split the stream for multiple clients
        self.tee = Gst.ElementFactory.make('tee', 'tee')
        if not self.tee:
            raise Exception("Could not create tee")
        self.tee.set_property('allow-not-linked', True)  # Important for dynamic clients

        # Add elements to bin
        self.add(self.convert)
        self.add(self.encoder)
        self.add(self.payloader)
        self.add(self.tee)

        # Link elements
        self.convert.link(self.encoder)
        self.encoder.link(self.payloader)
        self.payloader.link(self.tee)

        # Create sink pad
        self.sink_pad = Gst.GhostPad.new('sink', self.convert.get_static_pad('sink'))
        self.add_pad(self.sink_pad)

    def create_webrtcbin(self):
        """Create a new WebRTCbin for a client connection."""
        logger.info("Creating new WebRTCbin")

        # Create a new webrtcbin
        webrtcbin = Gst.ElementFactory.make('webrtcbin', None)
        if not webrtcbin:
            logger.error("Failed to create WebRTCbin")
            return None

        # Configure the webrtcbin
        webrtcbin.set_property('stun-server', self.stun_server)

        # Add it to our bin
        self.add(webrtcbin)

        # Create a queue for this client
        queue = Gst.ElementFactory.make('queue', None)
        queue.set_property('leaky', 2)  # Leak downstream (old buffers)
        if not queue:
            logger.error("Failed to create queue")
            webrtcbin.set_state(Gst.State.NULL)
            self.remove(webrtcbin)
            return None

        # Add the queue to our bin
        self.add(queue)

        # Get a source pad from the tee
        tee_src_pad = self.tee.get_request_pad("src_%u")
        if not tee_src_pad:
            logger.error("Failed to get source pad from tee")
            queue.set_state(Gst.State.NULL)
            webrtcbin.set_state(Gst.State.NULL)
            self.remove(queue)
            self.remove(webrtcbin)
            return None

        # Get the sink pad from the queue
        queue_sink_pad = queue.get_static_pad("sink")

        # Link the tee to the queue
        ret = tee_src_pad.link(queue_sink_pad)
        if ret != Gst.PadLinkReturn.OK:
            logger.error(f"Failed to link tee to queue: {ret}")
            tee_src_pad.unlink(queue_sink_pad)
            self.tee.release_request_pad(tee_src_pad)
            queue.set_state(Gst.State.NULL)
            webrtcbin.set_state(Gst.State.NULL)
            self.remove(queue)
            self.remove(webrtcbin)
            return None

        # Link the queue to the webrtcbin
        ret = queue.link(webrtcbin)
        if not ret:
            logger.error("Failed to link queue to webrtcbin")
            tee_src_pad.unlink(queue_sink_pad)
            self.tee.release_request_pad(tee_src_pad)
            queue.set_state(Gst.State.NULL)
            webrtcbin.set_state(Gst.State.NULL)
            self.remove(queue)
            self.remove(webrtcbin)
            return None

        # Sync the element states with the parent
        queue.sync_state_with_parent()
        webrtcbin.sync_state_with_parent()

        logger.info("Successfully created and linked WebRTCbin")
        return webrtcbin

    def do_get_property(self, prop):
        """Handle property reads."""
        if prop.name == 'port':
            return self.port
        elif prop.name == 'ws-port':
            return self.ws_port
        elif prop.name == 'bind-address':
            return self.bind_address
        elif prop.name == 'stun-server':
            return self.stun_server
        elif prop.name == 'video-codec':
            return self.video_codec
        else:
            raise AttributeError(f'Unknown property {prop.name}')

    def do_set_property(self, prop, value):
        """Handle property writes."""
        if prop.name == 'port':
            self.port = value
        elif prop.name == 'ws-port':
            self.ws_port = value
        elif prop.name == 'bind-address':
            self.bind_address = value
        elif prop.name == 'stun-server':
            self.stun_server = value
            # This will apply to new WebRTCbins created
        elif prop.name == 'video-codec':
            self.video_codec = value
        else:
            raise AttributeError(f'Unknown property {prop.name}')

    def handle_message(self, message):
        """Handle GStreamer messages."""
        if message.type == Gst.MessageType.ERROR:
            error, debug = message.parse_error()
            logger.error(f"Error: {error.message}")
            logger.debug(f"Debug info: {debug}")
        return Gst.Bin.handle_message(self, message)

    def do_change_state(self, transition):
        """Handle state changes."""
        if transition == Gst.StateChange.NULL_TO_READY:
            # Start servers only if they haven't been started yet
            if not self.servers_started:
                try:
                    self.start_servers()
                    self.servers_started = True
                except Exception as e:
                    logger.error(f"Failed to start servers: {e}")
                    return Gst.StateChangeReturn.FAILURE
        elif transition == Gst.StateChange.READY_TO_NULL:
            # Stop servers only if they are running
            if self.servers_started:
                self.stop_servers()
                self.servers_started = False

        return Gst.Bin.do_change_state(self, transition)

    def start_servers(self):
        """Start the HTTP and WebSocket servers."""
        # Create HTTP server socket with address reuse
        sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        sock.bind((self.bind_address, self.port))
        sock.listen(1)

        # Create HTTP server with the bound socket
        # Set the WebSocket port in the handler class
        WebRTCHTTPHandler.ws_port = self.ws_port

        self.http_server = HTTPServer(
            (self.bind_address, self.port),
            WebRTCHTTPHandler,
            bind_and_activate=False
        )
        self.http_server.socket = sock

        self.http_thread = threading.Thread(target=self.http_server.serve_forever)
        self.http_thread.daemon = True
        self.http_thread.start()

        # Start WebSocket signaling server
        self.signaling = SignalingServer(
            self.create_webrtcbin,
            host=self.bind_address,
            port=self.ws_port
        )
        self.signaling_thread = threading.Thread(target=self.signaling.start)
        self.signaling_thread.daemon = True
        self.signaling_thread.start()

        # Wait a bit for the server to start
        time.sleep(0.5)

    def stop_servers(self):
        """Stop the HTTP and WebSocket servers."""
        if self.http_server:
            self.http_server.shutdown()
            self.http_server = None
            self.http_thread = None

        if self.signaling:
            self.signaling.stop()
            self.signaling = None
            self.signaling_thread = None

# Register the GObject type
GObject.type_register(WebRTCWebSink)
__gstelementfactory__ = ("webrtcwebsink", Gst.Rank.NONE, WebRTCWebSink)