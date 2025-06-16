// Integration test for the WebSink GStreamer plugin
// This test launches a pipeline similar to the main app and checks for errors.

use gst::prelude::*;
use std::sync::{Arc, Mutex};
use gst::glib;
use websink::websink::WebSink;

#[test]
fn test_websink_pipeline() {
    // Initialize GStreamer
    gst::init().expect("Failed to initialize gst_init");

    /* Enable stdout debug for websink category */
    gst::log::set_default_threshold(gst::DebugLevel::Warning);
    gst::log::set_threshold_for_name("websink", gst::DebugLevel::Debug);

    println!("🚀 Starting Rust WebSink test application");

    // Register the WebSink element with GStreamer
    gst::Element::register(None, "websink", gst::Rank::NONE, WebSink::static_type()).unwrap();

    let main_loop = glib::MainLoop::new(None, false);

    // Start stream

    let pls = "videotestsrc is-live=true ! video/x-raw,width=640,height=480,framerate=30/1 ! videoconvert ! x264enc tune=zerolatency ! websink name=wsink port=8087";
    let pipeline = gst::parse::launch(pls).unwrap();
    let pipeline = pipeline.downcast::<gst::Pipeline>().unwrap();

    pipeline
        .set_state(gst::State::Playing)
        .expect("Failed to set pipeline to `Playing`");

    let pipeline = pipeline.downcast::<gst::Pipeline>().unwrap();

    let main_loop_cloned = main_loop.clone();
    let bus = pipeline.bus().unwrap();
    let errors = Arc::new(Mutex::new(Vec::new()));
    let errors_clone = errors.clone();
    let main_loop = glib::MainLoop::new(None, false);
    let main_loop_clone = main_loop.clone();

    let _bus_watch = bus.add_watch(move |_, msg| {
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
    }).expect("failed to add bus watch");

    // Open browser to the URL where your app is running
    webbrowser::open("http://localhost:8087").expect("Failed to open web browser");

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
