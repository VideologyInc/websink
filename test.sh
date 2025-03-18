#! /usr/bin/bash

script_dir=$(dirname $0)
cd $script_dir
export GST_PLUGIN_PATH=$script_dir:$GST_PLUGIN_PATH
rm -rf ~/.cache/gstreamer-1.0
# sudo mkdir -p /usr/lib/gstreamer-1.0/python/
# sudo cp -f scailxwebsink.py /usr/lib/gstreamer-1.0/python/
# export GST_PLUGIN_PATH=$script_dir::$GST_PLUGIN_PATH
# export GST_DEBUG=3
gst-launch-1.0 videotestsrc ! video/x-raw,width=640,height=480,framerate=30/1 ! videoconvert ! x264enc tune=zerolatency ! websink
# gst-launch-1.0 v4l2src device=/dev/video-isi-csi0 ! video/x-raw,width=1920,height=1080,framerate=60/1 ! webrtcwebsink name=sink
