use std::path::{Path, PathBuf};

use glib::prelude::*;
use gstreamer::prelude::*;

use super::Error;
use crate::media_info::MediaInfo;
use crate::media_type::MediaType;
use crate::random_files::RandomFiles;
use crate::stream::streame_queue::StreamQueueItem;

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
    // TODO: Start thread when pipeline is running.
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
    has_audio: bool,
    duration: Option<gstreamer::ClockTime>,
    video_tx: flume::Sender<gstreamer::Sample>,
    audio_tx: flume::Sender<gstreamer::Sample>,
) -> Result<gstreamer::Pipeline, Error> {
    // filesrc -> decodebin -> videoconvert -> capsfilter -> appsink
    let pipeline = gstreamer::Pipeline::builder().name("decoder-pipeline").build();

    // --- Core Pipeline Elements ---
    let filesrc = gstreamer::ElementFactory::make("filesrc")
        .property("location", path.to_str().unwrap())
        .build()?;

    // Remove `no-audio=true` to let decodebin find audio
    let decodebin = gstreamer::ElementFactory::make("decodebin3").build()?;

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
        println!("Decoder: New pad added: {pad_name}");

        if pad_name.starts_with("video_") {
            let sink_pad =
                pipeline.by_name("videoconvert_vid").unwrap().static_pad("sink").unwrap();
            if sink_pad.is_linked() {
                eprintln!("Video sink already linked, ignoring.");
                return;
            }
            if let Err(err) = pad.link(&sink_pad) {
                eprintln!("Failed to link video pad: {}", err);
            }
        } else if pad_name.starts_with("audio_") {
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
            println!("Unknown pad type: {pad_name}");
        }
    });

    // --- AppSink Callbacks ---
    // Video callback
    appsink_video.set_callbacks(
        gstreamer_app::AppSinkCallbacks::builder()
            .new_sample(move |sink| {
                let sample = sink.pull_sample().map_err(|_| gstreamer::FlowError::Eos)?;
                video_tx.send(sample).map_err(|_| gstreamer::FlowError::Error)?;
                Ok(gstreamer::FlowSuccess::Ok)
            })
            .build(),
    );

    // Audio callback
    appsink_audio.set_callbacks(
        gstreamer_app::AppSinkCallbacks::builder()
            .new_sample(move |sink| {
                let sample = sink.pull_sample().map_err(|_| gstreamer::FlowError::Eos)?;
                audio_tx.send(sample).map_err(|_| gstreamer::FlowError::Error)?;
                Ok(gstreamer::FlowSuccess::Ok)
            })
            .build(),
    );

    Ok(pipeline)
}

fn create_image_pipeline(
    path: &Path,
    duration: Option<gstreamer::ClockTime>,
    video_tx: flume::Sender<gstreamer::Sample>,
    audio_tx: flume::Sender<gstreamer::Sample>,
) -> Result<gstreamer::Pipeline, Error> {
    let pipeline = gstreamer::Pipeline::builder().name("image-pipeline").build();

    // --- Video Chain (filesrc -> decodebin -> imagefreeze -> ...) ---
    let filesrc = gstreamer::ElementFactory::make("filesrc")
        .property("location", path.to_str().unwrap())
        .build()?;

    // Remove `no-audio=true` to let decodebin find audio
    let decodebin = gstreamer::ElementFactory::make("decodebin3").build()?;

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
        println!("Decoder: New pad added: {pad_name}");

        if pad_name.starts_with("video_") {
            if imagefreeze_sink_page.is_linked() {
                eprintln!("Image sink already linked, ignoring.");
                return;
            }
            if let Err(err) = pad.link(&imagefreeze_sink_page) {
                eprintln!("Failed to link video pad: {}", err);
            }
        } else {
            println!("Unknown pad type: {pad_name}");
        }
    });

    // --- AppSink Callbacks (Identical to media pipeline) ---
    appsink_video.set_callbacks(
        gstreamer_app::AppSinkCallbacks::builder()
            .new_sample(move |sink| {
                let sample = sink.pull_sample().map_err(|_| gstreamer::FlowError::Eos)?;
                video_tx.send(sample).map_err(|_| gstreamer::FlowError::Error)?;
                Ok(gstreamer::FlowSuccess::Ok)
            })
            .build(),
    );

    appsink_audio.set_callbacks(
        gstreamer_app::AppSinkCallbacks::builder()
            .new_sample(move |sink| {
                let sample = sink.pull_sample().map_err(|_| gstreamer::FlowError::Eos)?;
                audio_tx.send(sample).map_err(|_| gstreamer::FlowError::Error)?;
                Ok(gstreamer::FlowSuccess::Ok)
            })
            .build(),
    );

    Ok(pipeline)
}

/// Task for the thread that feeds the RTSP stream.
/// It waits for file paths from the channel and runs a pipeline for each.
pub fn file_feeder_task(root_dirs: Vec<PathBuf>, queue_tx: flume::Sender<StreamQueueItem>) {
    let (abort_tx, abort_rx) = flume::bounded(1);

    for path in RandomFiles::new(root_dirs) {
        println!("Path: {}", path.display());
        let media_info = match MediaInfo::detect(&path) {
            Ok(media_info) if !media_info.is_empty() => media_info,
            Ok(_) => continue,
            Err(error) => {
                eprintln!("Failed to get media info: {error}");
                continue;
            }
        };

        let media_type = media_info.media_type();
        let duration = media_info.duration;

        println!("File feeder received {media_type:?} file: {}", path.display());

        let (video_tx, video_rx) = flume::bounded(10);
        let (audio_tx, audio_rx) = flume::bounded(10);

        let pipeline_result = match media_type {
            MediaType::VideoWithAudio => {
                create_video_pipeline(&path, true, duration, video_tx, audio_tx)
            }
            MediaType::VideoWithoutAudio => {
                create_video_pipeline(&path, false, duration, video_tx, audio_tx)
            }
            MediaType::Image => create_image_pipeline(&path, duration, video_tx, audio_tx),
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

        if queue_tx
            .send(StreamQueueItem { path, video: video_rx, audio: audio_rx })
            .is_err()
        {
            eprintln!("Queue channel disconnected, aborting.");
            break;
        }

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
    }
    println!("Feeder thread shutting down.");
}
