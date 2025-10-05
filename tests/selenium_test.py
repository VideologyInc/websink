#!/usr/bin/env python3
import os, shutil
import time
import threading
import pytest
import gi
from selenium import webdriver
from selenium.webdriver.common.by import By
from selenium.webdriver.chrome.options import Options as ChromeOptions
from selenium.webdriver.firefox.options import Options as FirefoxOptions
from selenium.webdriver.support.ui import WebDriverWait
from selenium.webdriver.support import expected_conditions as EC
import cv2
import numpy as np

# set gstremer plugin path for this dir and parent dir
plugin_dir = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
# for rust
if os.path.exists(os.path.join(plugin_dir, "target", "debug")):
    plugin_dir = os.path.join(plugin_dir, "target", "debug")
os.environ["GST_PLUGIN_PATH"] = plugin_dir

# clear gstreamer registry cache
os.system("rm -rf ~/.cache/gstreamer-1.0/")

# Set up GStreamer
gi.require_version('Gst', '1.0')
from gi.repository import Gst, GLib, GObject
Gst.init(None)

# Verify plugin registration
registry = Gst.Registry.get()
factory = registry.find_feature("websink", Gst.ElementFactory)
if not factory:
    print("websink element not found in registry!")
else:
    print("Found websink element in registry")

# Global variables for pipeline and loop
pipeline = None
loop = None
pipeline_thread = None

def start_pipeline():
    """Start the GStreamer pipeline with websink."""
    global pipeline, loop
    try:
        # Create the pipeline
        pipeline_str = '''
            videotestsrc is-live=true ! video/x-raw,width=640,height=480,framerate=30/1 ! videoconvert ! x264enc tune=zerolatency ! websink is-live=true name=wsink
        '''
        print("\nStarting GStreamer pipeline:")
        print(f"Pipeline string: {pipeline_str}")

        print("Creating pipeline")
        pipeline = Gst.parse_launch(pipeline_str)

        # Add bus watch
        bus = pipeline.get_bus()
        bus.add_signal_watch()
        bus.connect("message", on_bus_message)

        # Set pipeline to PLAYING
        print("Setting pipeline to PLAYING state")
        ret = pipeline.set_state(Gst.State.PLAYING)
        if ret == Gst.StateChangeReturn.FAILURE:
            print("Failed to start pipeline")
            return

        print("Pipeline is playing")

        # Run the main loop
        loop.run()
    except Exception as e:
        print(f"Error in pipeline: {e}")
        if loop and loop.is_running():
            loop.quit()

def on_bus_message(bus, message):
    """Handle GStreamer bus messages."""
    global loop
    t = message.type
    if t == Gst.MessageType.EOS:
        print("End-of-stream")
        if loop:
            loop.quit()
    elif t == Gst.MessageType.ERROR:
        err, debug = message.parse_error()
        print(f"Error: {err.message}")
        if debug:
            print(f"Debug info: {debug}")
        if loop:
            loop.quit()
    return True

@pytest.fixture(scope="module")
def gstreamer_pipeline():
    """Set up and tear down the GStreamer pipeline."""
    global loop, pipeline_thread, pipeline

    print("Setting up GStreamer pipeline")
    loop = GLib.MainLoop()

    # Start the pipeline in a separate thread
    pipeline_thread = threading.Thread(target=start_pipeline)
    pipeline_thread.daemon = True
    pipeline_thread.start()

    # Wait for server to start
    time.sleep(1.7)

    # Yield control back to the test
    yield

    # Clean up resources
    print("Tearing down GStreamer pipeline")
    if pipeline:
        print("Setting pipeline to NULL state")
        pipeline.set_state(Gst.State.NULL)
    if loop and loop.is_running():
        print("Quitting GLib main loop")
        loop.quit()
    if pipeline_thread and pipeline_thread.is_alive():
        print("Joining pipeline thread")
        pipeline_thread.join(timeout=2)

@pytest.fixture
def chrome_driver():
    """Set up and tear down the Chrome driver."""
    # Set up Chrome options
    chrome_options = ChromeOptions()
    chrome_options.add_argument("--use-fake-ui-for-media-stream")  # Auto-accept camera/mic permissions
    chrome_options.add_argument("--disable-dev-shm-usage")
    chrome_options.add_argument("--no-sandbox")
    # Enable verbose logging
    chrome_options.add_argument("--enable-logging")
    chrome_options.add_argument("--v=1")
    # Log to a file
    import os
    log_path = os.path.join(os.path.dirname(os.path.abspath(__file__)), 'chrome_debug.log')
    chrome_options.add_argument(f"--log-file={log_path}")
    # Headless mode can be problematic for WebRTC, but we'll try
    chrome_options.add_argument("--headless=new")  # New headless mode for Chrome
    chromium_path = shutil.which("chromium-browser") or shutil.which("chromium")
    if chromium_path:
        chrome_options.binary_location = chromium_path

    # Set up webdriver
    print("Starting Chrome browser")
    driver = webdriver.Chrome(options=chrome_options)
    driver.set_window_size(1024, 768)

    # Yield the driver to the test
    yield driver

    # Clean up
    print("Quitting Chrome browser")
    driver.quit()

@pytest.fixture
def firefox_driver():
    """Set up and tear down the Firefox driver."""
    # Set up Firefox options
    firefox_options = FirefoxOptions()
    firefox_options.log.level = "trace"  # Set log level to trace

    # Set up webdriver
    print("Starting Firefox browser")
    driver = webdriver.Firefox(options=firefox_options)
    driver.set_window_size(1024, 768)

    # Yield the driver to the test
    yield driver

    # Clean up
    print("Quitting Firefox browser")
    driver.quit()

def test_webrtc_stream(gstreamer_pipeline, chrome_driver):
    """Test that the WebRTC stream is working correctly."""
    try:
        # Navigate to the WebRTC page
        url = "http://localhost:8091"
        print(f"Navigating to {url}")
        chrome_driver.get(url)

        # Wait for the video element to appear and start playing
        print("Waiting for video element")
        wait = WebDriverWait(chrome_driver, 20)
        video = wait.until(EC.presence_of_element_located((By.TAG_NAME, "video")))

        # Wait for the video to start playing (this is tricky to detect)
        # Instead, we'll wait a bit to give it time to connect
        print("Waiting for WebRTC connection to establish")
        time.sleep(1)

        # Take a screenshot of just the video element
        print("Taking screenshot of the video element")
        video_screenshot_path = os.path.join(os.path.dirname(os.path.abspath(__file__)), 'video_screenshot.png')
        if os.path.exists(video_screenshot_path):
            return # Skip the test if the screenshot already exists

        # Use JavaScript to check if the video is playing
        is_playing = chrome_driver.execute_script("return arguments[0].currentTime > 0 && !arguments[0].paused && !arguments[0].ended", video)
        print(f"Video is playing: {is_playing}")

        # Get video dimensions
        location = video.location
        size = video.size

        # Take screenshot of the video element
        chrome_driver.save_screenshot(video_screenshot_path)
        print(f"Video screenshot saved to {video_screenshot_path}")

        # Check if video has valid dimensions
        assert size['width'] > 0, "Video width should be greater than 0"
        assert size['height'] > 0, "Video height should be greater than 0"

    except Exception as e:
        print(f"Error in test: {e}")
        import traceback
        traceback.print_exc()
        pytest.fail(f"Test failed with error: {e}")

def test_image_comparison(gstreamer_pipeline, chrome_driver):
    """
    Test capturing a new screenshot and comparing it with reference video_screenshot.png
    using OpenCV to verify similarity.
    """
    try:
        # Navigate to the WebRTC page
        url = "http://localhost:8091"
        print(f"Navigating to {url}")
        chrome_driver.get(url)

        # Wait for the video element to appear
        print("Waiting for video element")
        wait = WebDriverWait(chrome_driver, 20)
        video = wait.until(EC.presence_of_element_located((By.TAG_NAME, "video")))

        # Give time for the WebRTC connection to establish
        print("Waiting for WebRTC connection to establish")
        time.sleep(3)

        # Capture a new screenshot for comparison
        print("Taking new screenshot for comparison")
        new_screenshot_path = os.path.join(os.path.dirname(os.path.abspath(__file__)), 'chrome_screenshot.png')
        chrome_driver.save_screenshot(new_screenshot_path)

        # Path to the reference image
        reference_path = os.path.join(os.path.dirname(os.path.abspath(__file__)), 'video_screenshot.png')

        # Check if reference image exists
        assert os.path.exists(reference_path), "Reference image doesn't exist"

        # Load images with OpenCV
        print("Loading images for comparison")
        reference_img = cv2.imread(reference_path)
        new_img = cv2.imread(new_screenshot_path)

        # Make sure both images were loaded
        assert reference_img is not None, "Failed to load reference image"
        assert new_img is not None, "Failed to load new screenshot"

        # Resize if dimensions don't match
        if reference_img.shape != new_img.shape:
            print("Resizing images to match dimensions")
            new_img = cv2.resize(new_img, (reference_img.shape[1], reference_img.shape[0]))

        # Compare images
        print("Comparing images")
        # Convert images to grayscale for comparison
        ref_gray = cv2.cvtColor(reference_img, cv2.COLOR_BGR2GRAY)
        new_gray = cv2.cvtColor(new_img, cv2.COLOR_BGR2GRAY)

        # Calculate image similarity index
        res = cv2.matchTemplate(new_gray, ref_gray, cv2.TM_CCOEFF_NORMED)
        min_val, max_val, min_loc, max_loc = cv2.minMaxLoc(res)

        threshold = 0.8
        if max_val >= threshold:
            print(f"Chrome image similarity good: Max value: {max_val}")
        else:
            print(f"Chrome image similarity not good: Max value: {max_val}")

        assert max_val >= threshold, f"Images are not similar enough: max_val {max_val} < threshold {threshold}"

        # Clean up the new screenshot after the test
        # if os.path.exists(new_screenshot_path):
        #     os.remove(new_screenshot_path)

    except Exception as e:
        print(f"Error in image comparison test: {e}")
        import traceback
        traceback.print_exc()
        pytest.fail(f"Image comparison test failed with error: {e}")

def test_multi_stream_console_logs(chrome_driver):
    """Test multi-stream with console log capture to debug ontrack events."""
    global loop, pipeline_thread, pipeline

    print("Setting up multi-stream GStreamer pipeline")
    loop = GLib.MainLoop()

    def start_multi_pipeline():
        global pipeline, loop
        try:
            pipeline_str = '''
                websink name=ws port=8092 is-live=true
                videotestsrc pattern=0 is-live=true num-buffers=300 ! video/x-raw,width=640,height=480,framerate=30/1 ! videoconvert ! x264enc tune=zerolatency ! ws.
                videotestsrc pattern=1 is-live=true num-buffers=300 ! video/x-raw,width=640,height=480,framerate=30/1 ! videoconvert ! vp8enc deadline=1 ! ws.
            '''
            print(f"Multi-stream pipeline: {pipeline_str}")
            pipeline = Gst.parse_launch(pipeline_str)

            bus = pipeline.get_bus()
            bus.add_signal_watch()
            bus.connect("message", on_bus_message)

            ret = pipeline.set_state(Gst.State.PLAYING)
            if ret == Gst.StateChangeReturn.FAILURE:
                print("Failed to start pipeline")
                return

            print("Multi-stream pipeline is playing")
            loop.run()
        except Exception as e:
            print(f"Error in multi-stream pipeline: {e}")
            if loop and loop.is_running():
                loop.quit()

    pipeline_thread = threading.Thread(target=start_multi_pipeline)
    pipeline_thread.daemon = True
    pipeline_thread.start()
    time.sleep(2)

    try:
        url = "http://localhost:8092"
        print(f"Navigating to {url}")

        # Enable browser logging
        chrome_driver.execute_cdp_cmd('Log.enable', {})
        chrome_driver.get(url)

        # Wait for connection
        wait = WebDriverWait(chrome_driver, 10)
        wait.until(EC.presence_of_element_located((By.TAG_NAME, "video")))
        time.sleep(3)

        # Get console logs
        logs = chrome_driver.get_log('browser')
        print("\n=== Browser Console Logs ===")
        for log in logs:
            print(f"{log['level']}: {log['message']}")

        # Execute JavaScript to check state
        track_count = chrome_driver.execute_script("""
            console.log('=== Checking WebRTC State ===');
            console.log('videoElements length:', videoElements.length);
            console.log('Video elements in DOM:', document.querySelectorAll('video').length);
            console.log('PC state:', pc.connectionState);
            console.log('PC ice state:', pc.iceConnectionState);

            // Log transceiver info
            const transceivers = pc.getTransceivers();
            console.log('Number of transceivers:', transceivers.length);
            transceivers.forEach((t, i) => {
                console.log(`Transceiver ${i}:`, {
                    direction: t.direction,
                    mid: t.mid,
                    stopped: t.stopped
                });
                if (t.receiver && t.receiver.track) {
                    console.log(`  Track:`, t.receiver.track.id, t.receiver.track.kind);
                }
            });

            return {
                videoElementsCount: videoElements.length,
                domVideoCount: document.querySelectorAll('video').length,
                transceiverCount: transceivers.length
            };
        """)

        print(f"\n=== WebRTC State ===")
        print(f"videoElements array: {track_count['videoElementsCount']}")
        print(f"DOM video elements: {track_count['domVideoCount']}")
        print(f"Transceivers: {track_count['transceiverCount']}")

        # Check assertions
        assert track_count['videoElementsCount'] == 2, f"Expected 2 video elements, got {track_count['videoElementsCount']}"
        assert track_count['domVideoCount'] == 2, f"Expected 2 DOM video elements, got {track_count['domVideoCount']}"

    finally:
        print("Cleaning up multi-stream test")
        if pipeline:
            pipeline.set_state(Gst.State.NULL)
        if loop and loop.is_running():
            loop.quit()
        if pipeline_thread and pipeline_thread.is_alive():
            pipeline_thread.join(timeout=2)

def test_single_stream_debug(gstreamer_pipeline, chrome_driver):
    """Test single-stream with detailed logging to debug peer connection issues."""
    try:
        url = "http://localhost:8091"
        print(f"Navigating to {url}")

        # Enable browser logging
        chrome_driver.execute_cdp_cmd('Log.enable', {})
        chrome_driver.get(url)

        # Wait for connection
        wait = WebDriverWait(chrome_driver, 10)
        wait.until(EC.presence_of_element_located((By.TAG_NAME, "video")))
        time.sleep(3)

        # Get console logs
        logs = chrome_driver.get_log('browser')
        print("\n=== Browser Console Logs ===")
        for log in logs:
            print(f"{log['level']}: {log['message']}")

        # Check peer connection state
        pc_state = chrome_driver.execute_script("""
            console.log('=== Checking Peer Connection State ===');
            console.log('PC state:', pc.connectionState);
            console.log('PC ice state:', pc.iceConnectionState);
            console.log('PC signaling state:', pc.signalingState);

            return {
                connectionState: pc.connectionState,
                iceConnectionState: pc.iceConnectionState,
                signalingState: pc.signalingState,
                localDescription: pc.localDescription ? 'present' : 'null',
                remoteDescription: pc.remoteDescription ? 'present' : 'null'
            };
        """)

        print(f"\n=== Peer Connection State ===")
        for key, value in pc_state.items():
            print(f"{key}: {value}")

        # Check if video is playing
        is_playing = chrome_driver.execute_script("""
            const videos = document.querySelectorAll('video');
            if (videos.length === 0) return 'no video elements';

            const video = videos[0];
            return {
                paused: video.paused,
                ended: video.ended,
                currentTime: video.currentTime,
                readyState: video.readyState,
                networkState: video.networkState
            };
        """)

        print(f"\n=== Video State ===")
        print(f"Video state: {is_playing}")

        # Assert expectations
        assert pc_state['connectionState'] == 'connected', f"Expected connected state, got {pc_state['connectionState']}"
        if isinstance(is_playing, dict):
            assert not is_playing['paused'], "Video should not be paused"
            assert is_playing['currentTime'] > 0, "Video should be playing (currentTime > 0)"

    except Exception as e:
        print(f"Error in test: {e}")
        import traceback
        traceback.print_exc()
        pytest.fail(f"Test failed with error: {e}")

# def test_image_comparison_firefox(gstreamer_pipeline, firefox_driver):
#     """
#     Test capturing a new screenshot with Firefox and comparing it with reference video_screenshot.png
#     using OpenCV to verify similarity.
#     """
#     try:
#         # Navigate to the WebRTC page
#         url = "http://localhost:8091"
#         print(f"Navigating to {url} with Firefox")
#         firefox_driver.get(url)

#         # Wait for the video element to appear
#         print("Waiting for video element in Firefox")
#         wait = WebDriverWait(firefox_driver, 20)
#         video = wait.until(EC.presence_of_element_located((By.TAG_NAME, "video")))

#         # Give time for the WebRTC connection to establish
#         print("Waiting for WebRTC connection to establish in Firefox")
#         time.sleep(9)

#         # Capture a new screenshot for comparison
#         print("Taking new screenshot for comparison with Firefox")
#         new_screenshot_path = os.path.join(os.path.dirname(os.path.abspath(__file__)), 'firefox_screenshot.png')
#         firefox_driver.save_screenshot(new_screenshot_path)

#         # Path to the reference image
#         reference_path = os.path.join(os.path.dirname(os.path.abspath(__file__)), 'video_screenshot.png')

#         # Check if reference image exists
#         assert os.path.exists(reference_path), "Reference image doesn't exist"

#         # Load images with OpenCV
#         print("Loading images for comparison")
#         reference_img = cv2.imread(reference_path)
#         new_img = cv2.imread(new_screenshot_path)

#         # Make sure both images were loaded
#         assert reference_img is not None, "Failed to load reference image"
#         assert new_img is not None, "Failed to load new screenshot"

#         # Resize if dimensions don't match
#         if reference_img.shape != new_img.shape:
#             print(f"Resizing images to match dimensions (reference: {reference_img.shape}, new: {new_img.shape})")
#             new_img = cv2.resize(new_img, (reference_img.shape[1], reference_img.shape[0]))

#         # Compare images
#         print("Comparing images")
#         # Convert images to grayscale for comparison
#         ref_gray = cv2.cvtColor(reference_img, cv2.COLOR_BGR2GRAY)
#         new_gray = cv2.cvtColor(new_img, cv2.COLOR_BGR2GRAY)

#         # Calculate image similarity index
#         res = cv2.matchTemplate(new_gray, ref_gray, cv2.TM_CCOEFF_NORMED)
#         min_val, max_val, min_loc, max_loc = cv2.minMaxLoc(res)

#         threshold = 0.8
#         if max_val >= threshold:
#             print(f"Firefox image similarity good: Max value: {max_val}")
#         else:
#             print(f"Firefox image similarity not good: Max value: {max_val}")

#         assert max_val >= threshold, f"Images are not similar enough: max_val {max_val} < threshold {threshold}"

#         # Clean up the new screenshot after the test
#         # if os.path.exists(new_screenshot_path):
#         #     os.remove(new_screenshot_path)

#     except Exception as e:
#         print(f"Error in Firefox image comparison test: {e}")
#         import traceback
#         traceback.print_exc()
#         pytest.fail(f"Firefox image comparison test failed with error: {e}")

if __name__ == "__main__":
    pytest.main(["-v", __file__])