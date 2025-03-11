import gi
gi.require_version('Gst', '1.0')
gi.require_version('GstBase', '1.0')
gi.require_version('GstWebRTC', '1.0')
gi.require_version('GstSdp', '1.0')
from gi.repository import Gst, GObject

from .plugin import WebRTCWebSink

# Initialize GStreamer
Gst.init(None)

def plugin_init(plugin):
    """Register the plugin."""
    return Gst.Element.register(plugin, "webrtcwebsink",
                              Gst.Rank.NONE, WebRTCWebSink)

def register():
    """Register the plugin for outside-of-gst loading."""
    if not Gst.Plugin.register_static(
        Gst.VERSION_MAJOR,
        Gst.VERSION_MINOR,
        "webrtcwebsink",
        "WebRTC Web Sink",
        plugin_init,
        "1.0",
        "LGPL",
        "webrtcwebsink",
        "webrtcwebsink",
        ""
    ):
        raise RuntimeError("Failed to register webrtcwebsink plugin")
    return True

# Register the plugin immediately when the module is imported
register()