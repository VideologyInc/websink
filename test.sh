#! /usr/bin/bash

script_dir=$(dirname $0)
rm -rf ~/.cache/gstreamer-1.0
sudo mkdir -p /usr/lib/gstreamer-1.0/python/
sudo cp -f gstreamer_file.py /usr/lib/gstreamer-1.0/python/
# export GST_PLUGIN_PATH=$script_dir::$GST_PLUGIN_PATH
# export GST_DEBUG=3
gst-launch-1.0 videotestsrc is-live=true ! videoconvert ! video/x-raw,format=RGBA,width=640,height=480,framerate=30/1 ! queue ! webrtcwebsink name=sink