use std::path::Path;
use std::sync::Arc;

use gstreamer::prelude::*;
use parking_lot::Mutex;

use crate::stream::Error;

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum MediaType {
    VideoWithAudio,
    VideoWithoutAudio,
    Image,
    Unknown,
}

/// Uses GStreamer's typefind to check if a file is a video or image.
pub fn get_media_type(path: &Path) -> Result<MediaType, Error> {
    // println!("TypeFind: Checking file {:?}", path);
    let context = glib::MainContext::new();
    let typefind_loop = glib::MainLoop::new(Some(&context), false);

    let result: Arc<Mutex<Option<MediaType>>> = Arc::new(Mutex::new(None));

    let pipeline = gstreamer::Pipeline::builder().name("typefind-pipeline").build();
    let pipeline_clone = pipeline.clone();
    let run_result = context.block_on(async {
        let filesrc = gstreamer::ElementFactory::make("filesrc")
            .property("location", path.to_str().unwrap())
            .build()?;
        let typefind = gstreamer::ElementFactory::make("typefind").build()?;

        pipeline.add_many([&filesrc, &typefind])?;
        gstreamer::Element::link_many([&filesrc, &typefind])?;

        let result_clone = result.clone();
        let typefind_loop_clone = typefind_loop.clone();

        // Connect to typefind's "have-type" signal
        typefind.connect("have-type", false, move |values| {
            // values[0] = &Element
            // values[1] = &u32 (probability)
            // values[2] = &gst::Caps

            if let Some(caps) = values.get(2).and_then(|v| v.get::<gstreamer::Caps>().ok()) {
                let name = caps.structure(0).map(|s| s.name().as_str()).unwrap_or("<unknown>");
                // println!("TypeFind: Found caps: {}", name);

                let media_type = if name.starts_with("video/") {
                    MediaType::VideoWithAudio
                } else if name.starts_with("image/") {
                    MediaType::Image
                } else {
                    MediaType::Unknown
                };
                *result_clone.lock() = Some(media_type);
                typefind_loop_clone.quit();
            }
            None
        });

        // Also handle bus messages for errors or premature EOS
        let bus = pipeline.bus().unwrap();
        let typefind_loop_clone = typefind_loop.clone();
        let result_clone = result.clone();

        let _bus_watch = bus.add_watch_local(move |_, msg| {
            match msg.view() {
                // EOS before typefind could find anything (e.g., empty file)
                gstreamer::MessageView::Eos(_) => {
                    if result_clone.lock().is_none() {
                        *result_clone.lock() = Some(MediaType::Unknown);
                    }
                    typefind_loop_clone.quit();
                }
                gstreamer::MessageView::Error(_) => {
                    *result_clone.lock() = Some(MediaType::Unknown);
                    typefind_loop_clone.quit();
                }
                _ => {}
            }
            glib::ControlFlow::Continue
        })?;

        pipeline.set_state(gstreamer::State::Playing)?;
        Ok::<(), Error>(())
    });

    _ = pipeline_clone.set_state(gstreamer::State::Null);

    // Check for GStreamer errors
    run_result?;

    // This blocks until the typefind_loop (running in `block_on`) is quit
    typefind_loop.run();
    // println!("TypeFind: Loop finished.");

    // Return the found type
    Ok(result.lock().take().unwrap_or(MediaType::Unknown))
}
