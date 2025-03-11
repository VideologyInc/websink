import gi
gi.require_version('Gst', '1.0')
from gi.repository import Gst, GObject

Gst.init(None)

from webrtcwebsink import WebRTCWebSink

GObject.type_register(WebRTCWebSink)
__gstelementfactory__ = ("webrtcwebsink", Gst.Rank.NONE, WebRTCWebSink)