// Integration test for the WebSink GStreamer plugin
// This test launches a pipeline similar to the main app and checks for errors.

use gst::glib;
use gst::prelude::*;
use std::sync::{Arc, Mutex};
use websink::websink::WebSink;

#[test]
fn test_websink_sample_h264() {
    gst::init().expect("Failed to initialize gst_init");
    gst::log::set_default_threshold(gst::DebugLevel::Warning);
    gst::log::set_threshold_for_name("websink", gst::DebugLevel::Debug);

    println!("🚀 Testing H.264 Sample Mode");
    gst::Element::register(None, "websink", gst::Rank::NONE, WebSink::static_type()).unwrap();

    let pls = "videotestsrc is-live=true num-buffers=300 ! video/x-raw,width=640,height=480,framerate=30/1 ! videoconvert ! x264enc tune=zerolatency ! websink port=8087";
    run_pipeline(pls, 8087);
}

#[test]
fn test_websink_rtp_h264() {
    gst::init().expect("Failed to initialize gst_init");
    gst::log::set_default_threshold(gst::DebugLevel::Warning);
    gst::log::set_threshold_for_name("websink", gst::DebugLevel::Debug);

    println!("🚀 Testing H.264 RTP Mode");
    gst::Element::register(None, "websink", gst::Rank::NONE, WebSink::static_type()).unwrap();

    let pls = "videotestsrc is-live=true num-buffers=300 ! video/x-raw,width=640,height=480,framerate=30/1 ! videoconvert ! x264enc tune=zerolatency ! rtph264pay ! websink port=8088";
    run_pipeline(pls, 8088);
}

#[test]
fn test_websink_rtp_vp8() {
    gst::init().expect("Failed to initialize gst_init");
    gst::log::set_default_threshold(gst::DebugLevel::Warning);
    gst::log::set_threshold_for_name("websink", gst::DebugLevel::Debug);

    println!("🚀 Testing VP8 RTP Mode");
    gst::Element::register(None, "websink", gst::Rank::NONE, WebSink::static_type()).unwrap();

    let pls = "videotestsrc is-live=true num-buffers=300 ! video/x-raw,width=640,height=480,framerate=30/1 ! videoconvert ! vp8enc deadline=1 ! rtpvp8pay ! websink port=8089";
    run_pipeline(pls, 8089);
}

#[test]
fn test_websink_rtp_vp9() {
    gst::init().expect("Failed to initialize gst_init");
    gst::log::set_default_threshold(gst::DebugLevel::Warning);
    gst::log::set_threshold_for_name("websink", gst::DebugLevel::Debug);

    println!("🚀 Testing VP9 RTP Mode");
    gst::Element::register(None, "websink", gst::Rank::NONE, WebSink::static_type()).unwrap();

    let pls = "videotestsrc is-live=true num-buffers=300 ! video/x-raw,width=640,height=480,framerate=30/1 ! videoconvert ! vp9enc deadline=1 ! rtpvp9pay ! websink port=8090";
    run_pipeline(pls, 8090);
}

#[test]
fn test_websink_sample_vp8() {
    gst::init().expect("Failed to initialize gst_init");
    gst::log::set_default_threshold(gst::DebugLevel::Warning);
    gst::log::set_threshold_for_name("websink", gst::DebugLevel::Debug);

    println!("🚀 Testing VP8 Sample Mode");
    gst::Element::register(None, "websink", gst::Rank::NONE, WebSink::static_type()).unwrap();

    let pls = "videotestsrc is-live=true num-buffers=300 ! video/x-raw,width=640,height=480,framerate=30/1 ! videoconvert ! vp8enc deadline=1 ! websink port=8091";
    run_pipeline(pls, 8091);
}

#[test]
fn test_websink_sample_vp9() {
    gst::init().expect("Failed to initialize gst_init");
    gst::log::set_default_threshold(gst::DebugLevel::Warning);
    gst::log::set_threshold_for_name("websink", gst::DebugLevel::Debug);

    println!("🚀 Testing VP9 Sample Mode");
    gst::Element::register(None, "websink", gst::Rank::NONE, WebSink::static_type()).unwrap();

    let pls = "videotestsrc is-live=true num-buffers=300 ! video/x-raw,width=640,height=480,framerate=30/1 ! videoconvert ! vp9enc deadline=1 ! websink port=8092";
    run_pipeline(pls, 8092);
}

fn run_pipeline(pls: &str, port: u16) {
    let pipeline = gst::parse::launch(pls).unwrap();
    let pipeline = pipeline.downcast::<gst::Pipeline>().unwrap();

    pipeline.set_state(gst::State::Playing).expect("Failed to set pipeline to `Playing`");

    let pipeline = pipeline.downcast::<gst::Pipeline>().unwrap();

    let bus = pipeline.bus().unwrap();
    let errors = Arc::new(Mutex::new(Vec::new()));
    let errors_clone = errors.clone();
    let main_loop = glib::MainLoop::new(None, false);
    let main_loop_clone = main_loop.clone();

    let _bus_watch = bus
        .add_watch(move |_, msg| {
            use gst::MessageView;
            match msg.view() {
                MessageView::Eos(..) => {
                    main_loop_clone.quit();
                }
                MessageView::Error(err) => {
                    let mut errors = errors_clone.lock().unwrap();
                    errors.push(format!("Error from {:?}: {} ({:?})", err.src().map(|s| s.path_string()), err.error(), err.debug()));
                    main_loop_clone.quit();
                }
                _ => (),
            }
            glib::ControlFlow::Continue
        })
        .expect("failed to add bus watch");

    // Open browser to the URL where your app is running
    webbrowser::open(&format!("http://localhost:{}", port)).expect("Failed to open web browser");

    // Run for a short time, then send EOS
    std::thread::spawn({
        let pipeline = pipeline.clone();
        move || {
            std::thread::sleep(std::time::Duration::from_secs(20));
            pipeline.send_event(gst::event::Eos::new());
        }
    });

    main_loop.run();
    pipeline.set_state(gst::State::Null).expect("Failed to set pipeline to Null");

    let errors = errors.lock().unwrap();
    assert!(errors.is_empty(), "Pipeline errors: {:?}", *errors);
}
