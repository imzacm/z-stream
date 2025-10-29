use std::path::{Path, PathBuf};

use glib::prelude::*;
use gstreamer::prelude::*;

use super::{AppSources, AppSrcStorage, Command, Error, Event};
use crate::media_info::MediaInfo;
use crate::media_type::MediaType;

/// Blocks until the AppSrc is available in the shared storage.
fn get_app_sources(storage: AppSrcStorage) -> AppSources {
    loop {
        let appsrc_opt = storage.lock().clone();
        if let Some(appsrc) = appsrc_opt {
            println!("Feeder thread connected to appsrc.");
            return appsrc;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

// TODO
// let counter_overlay = gstreamer::ElementFactory::make("textoverlay")
//     .name("counter_overlay")
//     .property_from_str("halignment", "right")
//     .property_from_str("valignment", "top")
//     .property_from_str("font-desc", "Sans, 20")
//     .property_from_str("text", "00:00")
//     .build()?;

fn create_title_overlay(path: &Path) -> Result<gstreamer::Element, Error> {
    let name = path.file_name().unwrap().to_string_lossy();
    let element = gstreamer::ElementFactory::make("textoverlay")
        .name("textoverlay")
        .property("text", name.as_ref())
        .property_from_str("valignment", "bottom") // top, center, bottom
        .property_from_str("halignment", "left") // left, center, right
        .property_from_str("font-desc", "Sans, 14")
        .build()?;
    Ok(element)
}

fn create_counter_overlay(
    duration: Option<gstreamer::ClockTime>,
) -> Result<gstreamer::Element, Error> {
    fn counter_text(
        elapsed: std::time::Duration,
        duration: Option<gstreamer::ClockTime>,
    ) -> String {
        let elapsed = gstreamer::ClockTime::from_seconds_f64(elapsed.as_secs_f64());
        let e_mins = elapsed.minutes();
        let e_secs = elapsed.seconds();
        if let Some(duration) = duration {
            let d_mins = duration.minutes();
            let d_secs = duration.seconds();
            format!("{e_mins}:{e_secs} / {d_mins}:{d_secs}")
        } else {
            format!("{e_mins}:{e_secs}")
        }
    }

    let counter_overlay = gstreamer::ElementFactory::make("textoverlay")
        .name("counter_overlay")
        .property_from_str("halignment", "right")
        .property_from_str("valignment", "top")
        .property_from_str("font-desc", "Sans, 14")
        .property_from_str("text", &counter_text(std::time::Duration::default(), duration))
        .build()?;

    let counter_overlay_weak = counter_overlay.downgrade();
    std::thread::spawn(move || {
        let start = std::time::Instant::now();
        loop {
            std::thread::sleep(std::time::Duration::from_secs(1));
            if let Some(overlay) = counter_overlay_weak.upgrade() {
                let elapsed = start.elapsed();
                let text = counter_text(elapsed, duration);
                overlay.set_property("text", &text);
            } else {
                break;
            }
        }
    });

    Ok(counter_overlay)
}

fn create_silent_audio(pipeline: &gstreamer::Pipeline) -> Result<gstreamer_app::AppSink, Error> {
    // --- Audio Chain (audiotestsrc -> ...) ---
    let audiotestsrc = gstreamer::ElementFactory::make("audiotestsrc")
        // Generate silence
        .property_from_str("wave", "silence")
        .build()?;

    let audioconvert_aud = gstreamer::ElementFactory::make("audioconvert").build()?;
    let audiorate_aud = gstreamer::ElementFactory::make("audiorate").build()?;
    let capsfilter_aud = gstreamer::ElementFactory::make("capsfilter")
        .property(
            "caps",
            gstreamer::Caps::builder("audio/x-raw")
                .field("format", "S16LE")
                .field("layout", "interleaved")
                .field("rate", 48000)
                .field("channels", 2)
                .build(),
        )
        .build()?;
    let appsink_audio = gstreamer_app::AppSink::builder().name("appsink_audio").build();

    pipeline.add_many([
        &audiotestsrc,
        &audioconvert_aud,
        &audiorate_aud,
        &capsfilter_aud,
        appsink_audio.upcast_ref(),
    ])?;

    gstreamer::Element::link_many([
        &audiotestsrc,
        &audioconvert_aud,
        &audiorate_aud,
        &capsfilter_aud,
        appsink_audio.upcast_ref(),
    ])?;

    Ok(appsink_audio)
}

fn create_audio_chain(pipeline: &gstreamer::Pipeline) -> Result<gstreamer_app::AppSink, Error> {
    // --- Audio Chain ---
    let audioconvert_aud = gstreamer::ElementFactory::make("audioconvert")
        .name("audioconvert_aud") // Unique name
        .build()?;
    let audio_resample = gstreamer::ElementFactory::make("audioresample")
        .name("audio_resample")
        .build()?;
    // These caps MUST match the caps in media_factory.rs
    let capsfilter_aud = gstreamer::ElementFactory::make("capsfilter")
        .property(
            "caps",
            gstreamer::Caps::builder("audio/x-raw")
                .field("format", "S16LE")
                .field("layout", "interleaved")
                .field("rate", 48000)
                .field("channels", 2)
                .build(),
        )
        .build()?;
    let appsink_audio = gstreamer_app::AppSink::builder().name("appsink_audio").build();

    pipeline.add_many([
        &audioconvert_aud,
        &audio_resample,
        &capsfilter_aud,
        appsink_audio.upcast_ref(),
    ])?;

    // Pre-link the audio chain
    gstreamer::Element::link_many([
        &audioconvert_aud,
        &audio_resample,
        &capsfilter_aud,
        appsink_audio.upcast_ref(),
    ])?;

    Ok(appsink_audio)
}

fn create_video_pipeline(
    path: &Path,
    app_sources: &AppSources,
    has_audio: bool,
    duration: Option<gstreamer::ClockTime>,
) -> Result<gstreamer::Pipeline, Error> {
    // filesrc -> decodebin -> videoconvert -> capsfilter -> appsink
    let pipeline = gstreamer::Pipeline::builder().name("decoder-pipeline").build();

    // --- Core Pipeline Elements ---
    let filesrc = gstreamer::ElementFactory::make("filesrc")
        .property("location", path.to_str().unwrap())
        .build()?;

    // Remove `no-audio=true` to let decodebin find audio
    let decodebin = gstreamer::ElementFactory::make("decodebin").build()?;

    // --- Video Chain ---
    let videoconvert_vid = gstreamer::ElementFactory::make("videoconvert")
        .name("videoconvert_vid") // Unique name
        .build()?;

    let videoscale_vid = gstreamer::ElementFactory::make("videoscale")
        .name("videoscale_vid")
        .property("add-borders", true)
        .build()?;

    let title_overlay = create_title_overlay(path)?;
    let counter_overlay = create_counter_overlay(duration)?;

    let capsfilter_vid = gstreamer::ElementFactory::make("capsfilter")
        .property(
            "caps",
            gstreamer::Caps::builder("video/x-raw")
                .field("format", gstreamer_video::VideoFormat::I420.to_string())
                .field("width", 1280)
                .field("height", 720)
                .field("pixel-aspect-ratio", gstreamer::Fraction::new(1, 1))
                .build(),
        )
        .build()?;
    let appsink_video = gstreamer_app::AppSink::builder().name("appsink_video").build();

    // --- Add all elements to pipeline ---
    pipeline.add_many([
        &filesrc,
        &decodebin,
        &videoconvert_vid,
        &videoscale_vid,
        &title_overlay,
        &counter_overlay,
        &capsfilter_vid,
        appsink_video.upcast_ref(),
    ])?;

    // Link static parts
    gstreamer::Element::link_many([&filesrc, &decodebin])?;

    // Pre-link the video chain
    gstreamer::Element::link_many([
        &videoconvert_vid,
        &videoscale_vid,
        &title_overlay,
        &counter_overlay,
        &capsfilter_vid,
        appsink_video.upcast_ref(),
    ])?;

    let appsink_audio = if has_audio {
        create_audio_chain(&pipeline)?
    } else {
        create_silent_audio(&pipeline)?
    };

    // --- Dynamic Pad Linking ---
    let pipeline_weak = pipeline.downgrade();
    decodebin.connect_pad_added(move |_, pad| {
        let Some(pipeline) = pipeline_weak.upgrade() else { return };

        let pad_name = pad.name();
        println!("Decoder: New pad added: {}", pad_name);

        let caps = match pad.current_caps() {
            Some(c) => c,
            None => {
                eprintln!("Pad {} has no caps yet", pad.name());
                return;
            }
        };

        let s = match caps.structure(0) {
            Some(s) => s,
            None => return,
        };

        let media_type = s.name();
        println!("Decoder: New pad added: {} ({})", pad.name(), media_type);

        if media_type.starts_with("video/") {
            let sink_pad =
                pipeline.by_name("videoconvert_vid").unwrap().static_pad("sink").unwrap();
            if sink_pad.is_linked() {
                eprintln!("Video sink already linked, ignoring.");
                return;
            }
            if let Err(err) = pad.link(&sink_pad) {
                eprintln!("Failed to link video pad: {}", err);
            }
        } else if media_type.starts_with("audio/") {
            let sink_pad =
                pipeline.by_name("audioconvert_aud").unwrap().static_pad("sink").unwrap();
            if sink_pad.is_linked() {
                eprintln!("Audio sink already linked, ignoring.");
                return;
            }
            if let Err(err) = pad.link(&sink_pad) {
                eprintln!("Failed to link audio pad: {}", err);
            }
        } else {
            println!("Unknown pad type: {media_type}");
        }
    });

    // --- AppSink Callbacks ---
    // Video callback
    let appsrc_video_weak = app_sources.video.downgrade();
    appsink_video.set_callbacks(
        gstreamer_app::AppSinkCallbacks::builder()
            .new_sample(move |sink| {
                let Some(appsrc_video) = appsrc_video_weak.upgrade() else {
                    return Err(gstreamer::FlowError::Error);
                };
                let sample = sink.pull_sample().map_err(|_| gstreamer::FlowError::Eos)?;
                appsrc_video.push_sample(&sample).map_err(|_| gstreamer::FlowError::Error)
            })
            .build(),
    );

    // Audio callback
    let appsrc_audio_weak = app_sources.audio.downgrade();
    appsink_audio.set_callbacks(
        gstreamer_app::AppSinkCallbacks::builder()
            .new_sample(move |sink| {
                let Some(appsrc_audio) = appsrc_audio_weak.upgrade() else {
                    return Err(gstreamer::FlowError::Error);
                };
                let sample = sink.pull_sample().map_err(|_| gstreamer::FlowError::Eos)?;
                appsrc_audio.push_sample(&sample).map_err(|_| gstreamer::FlowError::Error)
            })
            .build(),
    );

    Ok(pipeline)
}

fn create_image_pipeline(
    path: &Path,
    app_sources: &AppSources,
    duration: Option<gstreamer::ClockTime>,
) -> Result<gstreamer::Pipeline, Error> {
    let pipeline = gstreamer::Pipeline::builder().name("image-pipeline").build();

    // --- Video Chain (filesrc -> decodebin -> imagefreeze -> ...) ---
    let filesrc = gstreamer::ElementFactory::make("filesrc")
        .property("location", path.to_str().unwrap())
        .build()?;

    // Remove `no-audio=true` to let decodebin find audio
    let decodebin = gstreamer::ElementFactory::make("decodebin").build()?;

    let imagefreeze = gstreamer::ElementFactory::make("imagefreeze").build()?;

    let videoconvert_vid = gstreamer::ElementFactory::make("videoconvert").build()?;

    let videoscale_vid = gstreamer::ElementFactory::make("videoscale")
        .property("add-borders", true)
        .build()?;
    let videorate_vid = gstreamer::ElementFactory::make("videorate").build()?;

    let title_overlay = create_title_overlay(path)?;
    let counter_overlay = create_counter_overlay(duration)?;

    let capsfilter_vid = gstreamer::ElementFactory::make("capsfilter")
        .property(
            "caps",
            gstreamer::Caps::builder("video/x-raw")
                .field("format", gstreamer_video::VideoFormat::I420.to_string())
                .field("width", 1280)
                .field("height", 720)
                .field("pixel-aspect-ratio", gstreamer::Fraction::new(1, 1))
                .field("framerate", gstreamer::Fraction::new(30, 1))
                .build(),
        )
        .build()?;
    let appsink_video = gstreamer_app::AppSink::builder().name("appsink_video").build();

    // Add all elements
    pipeline.add_many([
        &filesrc,
        &decodebin,
        &imagefreeze,
        &videoconvert_vid,
        &videoscale_vid,
        &videorate_vid,
        &title_overlay,
        &counter_overlay,
        &capsfilter_vid,
        appsink_video.upcast_ref(),
    ])?;

    filesrc.link(&decodebin)?;

    // Link static chains
    gstreamer::Element::link_many([
        &imagefreeze,
        &videoconvert_vid,
        &videoscale_vid,
        &videorate_vid,
        &title_overlay,
        &counter_overlay,
        &capsfilter_vid,
        appsink_video.upcast_ref(),
    ])?;

    let appsink_audio = create_silent_audio(&pipeline)?;

    // --- Dynamic linking for decodebin ---
    let imagefreeze_sink_page = imagefreeze.static_pad("sink").unwrap();
    decodebin.connect_pad_added(move |_, pad| {
        let pad_name = pad.name();
        println!("Decoder: New pad added: {}", pad_name);

        let caps = match pad.current_caps() {
            Some(c) => c,
            None => {
                eprintln!("Pad {} has no caps yet", pad.name());
                return;
            }
        };

        let s = match caps.structure(0) {
            Some(s) => s,
            None => return,
        };

        let media_type = s.name();
        println!("Decoder: New pad added: {} ({})", pad.name(), media_type);

        if media_type.starts_with("video/") {
            if imagefreeze_sink_page.is_linked() {
                eprintln!("Image sink already linked, ignoring.");
                return;
            }
            if let Err(err) = pad.link(&imagefreeze_sink_page) {
                eprintln!("Failed to link video pad: {}", err);
            }
        } else if media_type.starts_with("image/") {
            if imagefreeze_sink_page.is_linked() {
                eprintln!("Image sink already linked, ignoring.");
                return;
            }
            if let Err(err) = pad.link(&imagefreeze_sink_page) {
                eprintln!("Failed to link image pad: {}", err);
            }
        } else {
            println!("Unknown pad type: {media_type}");
        }
    });

    // --- AppSink Callbacks (Identical to media pipeline) ---
    let appsrc_video = app_sources.video.clone();
    appsink_video.set_callbacks(
        gstreamer_app::AppSinkCallbacks::builder()
            .new_sample(move |sink| {
                let sample = sink.pull_sample().map_err(|_| gstreamer::FlowError::Eos)?;
                appsrc_video.push_sample(&sample).map_err(|_| gstreamer::FlowError::Error)
            })
            .build(),
    );

    let appsrc_audio = app_sources.audio.clone();
    appsink_audio.set_callbacks(
        gstreamer_app::AppSinkCallbacks::builder()
            .new_sample(move |sink| {
                let sample = sink.pull_sample().map_err(|_| gstreamer::FlowError::Eos)?;
                appsrc_audio.push_sample(&sample).map_err(|_| gstreamer::FlowError::Error)
            })
            .build(),
    );

    Ok(pipeline)
}

/// Task for the thread that feeds the RTSP stream.
/// It waits for file paths from the channel and runs a pipeline for each.
pub fn file_feeder_task(
    file_rx: flume::Receiver<PathBuf>,
    command_rx: flume::Receiver<Command>,
    event_tx: flume::Sender<Event>,
    storage: AppSrcStorage,
) {
    // First, wait for the RTSP client to connect and create the appsrc
    let appsrcs = get_app_sources(storage);

    // let context = glib::MainContext::new();
    // let _guard = context.acquire().unwrap();
    // let event_loop = glib::MainLoop::new(Some(&context), false);

    let (abort_tx, abort_rx) = flume::bounded(1);

    // let event_loop_clone = event_loop.clone();
    let abort_tx_clone = abort_tx.clone();
    std::thread::spawn(move || {
        while let Ok(command) = command_rx.recv() {
            match command {
                Command::Skip => {
                    println!("Skipping file");
                    if abort_tx_clone.send(()).is_err() {
                        break;
                    }
                }
            }
        }
    });

    loop {
        let path = match file_rx.recv() {
            Ok(path) => path,
            Err(_) => {
                // Channel is closed, the app is shutting down
                println!("Feeder thread: Channel closed. Sending EOS to RTSP stream.");
                // Send EOS to both streams
                if let Err(error) = appsrcs.video.end_of_stream() {
                    eprintln!("Failed to send EOS to video: {error}");
                }
                if let Err(error) = appsrcs.audio.end_of_stream() {
                    eprintln!("Failed to send EOS to audio: {error}");
                }
                break;
            }
        };

        let media_info = match MediaInfo::detect(&path) {
            Ok(media_type) => media_type,
            Err(error) => {
                eprintln!("Failed to get media type: {error}");
                continue;
            }
        };
        let media_type = media_info.media_type();
        let duration = media_info.duration;

        println!("File feeder received {media_type:?} file: {}", path.display());

        let pipeline_result = match media_type {
            MediaType::VideoWithAudio => create_video_pipeline(&path, &appsrcs, true, duration),
            MediaType::VideoWithoutAudio => create_video_pipeline(&path, &appsrcs, false, duration),
            MediaType::Image => create_image_pipeline(&path, &appsrcs, duration),
            MediaType::Unknown => {
                eprintln!("File feeder received unknown media type: {:?}", path);
                continue;
            }
        };

        let pipeline = match pipeline_result {
            Ok(pipeline) => pipeline,
            Err(error) => {
                eprintln!("Failed to create pipeline: {error}");
                continue;
            }
        };

        println!("Playing file: {:?}", path);
        _ = event_tx.try_send(Event::Playing { path: path.clone() });

        // Start the file decoding pipeline
        pipeline.set_state(gstreamer::State::Playing).expect("Failed to start pipeline");

        if media_type == MediaType::Image {
            let abort_tx = abort_tx.clone();
            let pipeline_weak = pipeline.downgrade();
            // TODO: This is a potential memory leak.
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(10));
                if pipeline_weak
                    .upgrade()
                    .is_none_or(|p| p.current_state() == gstreamer::State::Null)
                {
                    return;
                }
                println!("Image loop: 10 seconds elapsed, stopping.");
                _ = abort_tx.send(());
            });
        }

        // --- Bus Message Handling ---
        let bus = pipeline.bus().unwrap();

        'main: loop {
            if let Ok(()) = abort_rx.recv_timeout(std::time::Duration::from_millis(10)) {
                break 'main;
            }

            for msg in bus.iter_timed(gstreamer::ClockTime::from_mseconds(10)) {
                use gstreamer::MessageView;
                match msg.view() {
                    MessageView::Eos(..) => {
                        break 'main;
                    }
                    MessageView::Error(err) => {
                        eprintln!("Error on pipeline: {} (debug: {:?})", err.error(), err.debug());
                        break 'main;
                    }
                    _ => (),
                }
            }
        }

        _ = pipeline.set_state(gstreamer::State::Null);

        _ = event_tx.try_send(Event::Ended { path: path.clone() });

        for appsrc in [&appsrcs.video, &appsrcs.audio] {
            appsrc.send_event(gstreamer::event::FlushStart::new());
            appsrc.send_event(gstreamer::event::FlushStop::new(true));
        }
    }
    println!("Feeder thread shutting down.");
}
