import gi
gi.require_version('Gst', '1.0')
from gi.repository import Gst, GObject, GLib

class ScailxWebSink(Gst.Bin):
    GST_PLUGIN_NAME = 'scailxwebsink'
    __gstmetadata__ = ("Scailx Web Sink",
                      "Video/Sink",
                      "autovideosink target for sending h264 to webrtc client",
                      "Kobus Goosen")

    __gproperties__ = {
        'ts-offset': (int,                # type
                     'Timestamp offset',   # nick
                     'Offset to apply to timestamps in nanoseconds', # blurb
                     GLib.MININT, GLib.MAXINT, 0,
                     GObject.ParamFlags.READWRITE # flags
                     )
    }

    def __init__(self):
        self.ts_offset = 0  # Initialize property value
        Gst.Bin.__init__(self)

        # Create the internal pipeline using parse_launch
        pipeline_str = "imxvideoconvert_g2d ! vpuenc_h264 qp-max=30 qp-min=18 ! websink"
        bin = Gst.parse_launch(pipeline_str)

        # Get the sink pad of the first element and the source pad of the last element
        first_element = bin.get_by_name("imxvideoconvert_g2d0")
        if not first_element:
            Gst.error("Failed to get first element")
            return None

        self.add(bin)
        # Create sink pad
        self.sink_pad = Gst.GhostPad.new('sink', first_element.get_static_pad('sink'))
        self.add_pad(self.sink_pad)

    def do_set_property(self, prop, value):
        pass

# Register the GObject type
GObject.type_register(ScailxWebSink)
__gstelementfactory__ = (ScailxWebSink.GST_PLUGIN_NAME, Gst.Rank.PRIMARY, ScailxWebSink)
