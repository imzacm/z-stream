use std::path::{Path, PathBuf};
use std::sync::Arc;

use glib::prelude::*;
use gstreamer::prelude::*;
use parking_lot::Mutex;

use super::{AppSources, AppSrcStorage, Command, Error, Event};
use crate::media_info::MediaInfo;
use crate::media_type::MediaType;
use crate::random_files::RandomFiles;

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

fn create_title_overlay(path: &Path) -> Result<gstreamer::Element, Error> {
    let name = path.to_string_lossy();
    let element = gstreamer::ElementFactory::make("textoverlay")
        .name("textoverlay")
        .property("text", name.as_ref())
        .property_from_str("valignment", "bottom") // top, center, bottom
        .property_from_str("halignment", "left") // left, center, right
        .property_from_str("font-desc", "Sans, 6")
        .property_from_str("wrap-mode", "wordchar") // none, word, char, wordchar
        .build()?;
    Ok(element)
}

fn create_counter_overlay(
    duration: Option<gstreamer::ClockTime>,
) -> Result<gstreamer::Element, Error> {
    let duration_str = duration.map(|duration| {
        let minutes = duration.minutes();
        let seconds = duration.seconds() % 60;
        format!("{minutes:02}:{seconds:02}")
    });

    let initial_text = if let Some(duration) = &duration_str {
        format!("00:00 / {duration}")
    } else {
        "00:00".to_string()
    };

    let counter_overlay = gstreamer::ElementFactory::make("textoverlay")
        .name("counter_overlay")
        .property_from_str("halignment", "right")
        .property_from_str("valignment", "top")
        .property_from_str("font-desc", "Sans, 10")
        .property_from_str("text", &initial_text)
        .build()?;

    let last_updated_second = Arc::new(Mutex::new(None));
    let sink_pad = counter_overlay.static_pad("video_sink").unwrap();
    let counter_overlay_weak = counter_overlay.downgrade();
    sink_pad.add_probe(gstreamer::PadProbeType::BUFFER, move |_pad, info| {
        if let Some(buffer) = info.buffer()
            && let Some(pts) = buffer.pts()
            && let Some(counter_overlay) = counter_overlay_weak.upgrade()
        {
            let current_second = pts.seconds();
            let mut last_updated_second = last_updated_second.lock();

            if last_updated_second.is_none_or(|v| v != current_second) {
                let minutes = pts.minutes();
                let seconds = pts.seconds() % 60;

                let current = format!("{minutes:02}:{seconds:02}");

                let text = if let Some(duration) = &duration_str {
                    format!("{current} / {duration}")
                } else {
                    current
                };
                counter_overlay.set_property("text", &text);
            }

            *last_updated_second = Some(current_second);
        }
        gstreamer::PadProbeReturn::Ok
    });

    Ok(counter_overlay)
}

fn create_silent_audio(pipeline: &gstreamer::Pipeline) -> Result<gstreamer_app::AppSink, Error> {
    // --- Audio Chain (audiotestsrc -> ...) ---
    let audiotestsrc = gstreamer::ElementFactory::make("audiotestsrc")
        .name("audiotestsrc")
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
    let queue_audio = gstreamer::ElementFactory::make("queue").name("a_queue").build()?;
    let appsink_audio = gstreamer_app::AppSink::builder().name("appsink_audio").build();

    pipeline.add_many([
        &audioconvert_aud,
        &audio_resample,
        &capsfilter_aud,
        &queue_audio,
        appsink_audio.upcast_ref(),
    ])?;

    // Pre-link the audio chain
    gstreamer::Element::link_many([
        &audioconvert_aud,
        &audio_resample,
        &capsfilter_aud,
        &queue_audio,
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

    let queue_video = gstreamer::ElementFactory::make("queue").name("v_queue").build()?;
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
        &queue_video,
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
        &queue_video,
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
    duration: gstreamer::ClockTime,
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
    let counter_overlay = create_counter_overlay(Some(duration))?;

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

    let queue_video = gstreamer::ElementFactory::make("queue").name("v_queue").build()?;
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
        &queue_video,
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
        &queue_video,
        appsink_video.upcast_ref(),
    ])?;

    let appsink_audio = create_silent_audio(&pipeline)?;

    let imagefreeze_src_pad = imagefreeze.static_pad("src").unwrap();
    let audio_src_pad_weak =
        pipeline.by_name("audiotestsrc").unwrap().static_pad("src").unwrap().downgrade();
    imagefreeze_src_pad.add_probe(gstreamer::PadProbeType::BUFFER, move |pad, info| {
        if let Some(buffer) = info.buffer()
            && let Some(pts) = buffer.pts()
            && pts > duration
        {
            pad.push_event(gstreamer::event::Eos::new());
            if let Some(pad) = audio_src_pad_weak.upgrade() {
                pad.push_event(gstreamer::event::Eos::new());
            }
            return gstreamer::PadProbeReturn::Remove;
        }
        gstreamer::PadProbeReturn::Ok
    });

    // --- Dynamic linking for decodebin ---
    let imagefreeze_sink_pad = imagefreeze.static_pad("sink").unwrap();
    decodebin.connect_pad_added(move |_, pad| {
        let pad_name = pad.name();
        println!("Decoder: New pad added: {pad_name}");

        if pad_name.starts_with("video_") {
            if imagefreeze_sink_pad.is_linked() {
                eprintln!("Image sink already linked, ignoring.");
                return;
            }
            if let Err(err) = pad.link(&imagefreeze_sink_pad) {
                eprintln!("Failed to link video pad: {}", err);
            }
        } else {
            println!("Unknown pad type: {pad_name}");
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

fn create_pipeline(
    path: &Path,
    app_sources: &AppSources,
) -> Option<(MediaType, gstreamer::Pipeline)> {
    let media_info = match MediaInfo::detect(path) {
        Ok(media_info) if !media_info.is_empty() => media_info,
        Ok(_) => return None,
        Err(error) => {
            eprintln!("Failed to get media info: {error}");
            return None;
        }
    };

    let media_type = media_info.media_type();
    let duration = media_info.duration;

    let pipeline_result = match media_type {
        MediaType::VideoWithAudio => create_video_pipeline(path, app_sources, true, duration),
        MediaType::VideoWithoutAudio => create_video_pipeline(path, app_sources, false, duration),
        MediaType::Image => {
            let duration = if let Some(duration) = duration
                && duration != gstreamer::ClockTime::ZERO
            {
                duration
            } else {
                5 * gstreamer::ClockTime::SECOND
            };
            create_image_pipeline(path, app_sources, duration)
        }
        MediaType::Unknown => {
            eprintln!(
                "File feeder received unknown media type {} - {media_info:?}",
                path.display()
            );
            return None;
        }
    };

    let pipeline = match pipeline_result {
        Ok(pipeline) => pipeline,
        Err(error) => {
            eprintln!("Failed to create pipeline: {error}");
            return None;
        }
    };

    Some((media_type, pipeline))
}

/// Task for the thread that feeds the RTSP stream.
/// It waits for file paths from the channel and runs a pipeline for each.
pub fn file_feeder_task(
    root_dirs: Vec<PathBuf>,
    command_rx: flume::Receiver<Command>,
    event_tx: flume::Sender<Event>,
    storage: AppSrcStorage,
) {
    // First, wait for the RTSP client to connect and create the appsrc
    let appsrcs = get_app_sources(storage);

    let (abort_tx, abort_rx) = flume::bounded(1);
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

    for path in RandomFiles::new(root_dirs) {
        let Some((media_type, pipeline)) = create_pipeline(&path, &appsrcs) else { continue };

        println!("File feeder received {media_type:?} file: {}", path.display());

        println!("Playing file: {:?}", path);
        _ = event_tx.try_send(Event::Playing { path: path.clone() });

        // Start the file decoding pipeline
        pipeline.set_state(gstreamer::State::Playing).expect("Failed to start pipeline");

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

        for appsrc in [&appsrcs.video, &appsrcs.audio] {
            appsrc.send_event(gstreamer::event::FlushStart::new());
            appsrc.send_event(gstreamer::event::FlushStop::new(true));
        }

        pipeline.send_event(gstreamer::event::FlushStart::new());

        _ = pipeline.set_state(gstreamer::State::Null);
        _ = event_tx.try_send(Event::Ended { path: path.clone() });
    }
    println!("Feeder thread shutting down.");
}
