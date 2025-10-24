use std::path::Path;

use glib::WeakRef;
use glib::object::ObjectExt;
use gstreamer::gobject::GObjectExtManualGst;
use gstreamer::prelude::{ElementExt, ElementExtManual, GstBinExt, GstBinExtManual, PadExt};

use crate::media_info::MediaInfo;
use crate::streamer::{Error, NvH264Tune, Output, VideoOptions, X264Tune};

pub struct FilePipeline {
    pub pipeline: gstreamer::Pipeline,
    pub muxer: gstreamer::Element,
    pub sink: gstreamer::Element,
}

impl FilePipeline {
    pub fn create(
        path: &Path,
        media_info: &MediaInfo,
        video_options: &VideoOptions,
        output: &Output,
        sink: Option<gstreamer::Element>,
    ) -> Result<Self, Error> {
        create_pipeline(path, media_info, video_options, output, sink)
    }
}

fn create_pipeline(
    path: &Path,
    media_info: &MediaInfo,
    video_options: &VideoOptions,
    output: &Output,
    sink: Option<gstreamer::Element>,
) -> Result<FilePipeline, Error> {
    let pipeline = gstreamer::Pipeline::new();

    let source = gstreamer::ElementFactory::make("filesrc").name("source").build()?;
    source.set_property("location", path);

    let decoder = gstreamer::ElementFactory::make("decodebin").name("decode").build()?;

    pipeline.add_many([&source, &decoder])?;
    source.link(&decoder)?;

    let video_pipeline = create_video_pipeline(&pipeline, media_info, video_options, output)?;
    let audio_pipeline = create_audio_pipeline(&pipeline, media_info, video_options, output)?;

    let muxer = match output {
        Output::Rtmp(_) => {
            let muxer = gstreamer::ElementFactory::make("flvmux").name("muxer").build()?;
            muxer.set_property("streamable", true);
            muxer
        }
        Output::Srt(_) => {
            let muxer = gstreamer::ElementFactory::make("mpegtsmux").name("muxer").build()?;
            // Make sure the muxer can start streaming immediately
            muxer.set_property("alignment", 7i32);
            muxer
        }
    };

    let sink = if let Some(sink) = sink {
        sink
    } else {
        match output {
            Output::Rtmp(url) => {
                let sink = gstreamer::ElementFactory::make("rtmpsink").name("sink").build()?;
                sink.set_property("location", url);
                sink
            }
            Output::Srt(url) => {
                let sink = gstreamer::ElementFactory::make("srtsink").name("sink").build()?;
                sink.set_property("uri", url);
                sink
            }
        }
    };

    // let (muxer, sink) = match output {
    //     Output::Rtmp(url) => {
    //         let muxer = gstreamer::ElementFactory::make("flvmux").name("muxer").build()?;
    //         muxer.set_property("streamable", true);
    //
    //         let sink = gstreamer::ElementFactory::make("rtmpsink").name("sink").build()?;
    //         sink.set_property("location", url);
    //
    //         (muxer, sink)
    //     }
    //     Output::Srt(url) => {
    //         let muxer = gstreamer::ElementFactory::make("mpegtsmux").name("muxer").build()?;
    //         // Make sure the muxer can start streaming immediately
    //         muxer.set_property("alignment", 7i32);
    //
    //         let sink = gstreamer::ElementFactory::make("srtsink").name("sink").build()?;
    //         sink.set_property("uri", url);
    //
    //         (muxer, sink)
    //     }
    // };

    pipeline.add_many([&muxer, &sink])?;

    video_pipeline.sink.link(&muxer)?;
    audio_pipeline.sink.link(&muxer)?;
    muxer.link(&sink)?;

    let v_source_weak = video_pipeline.source.map(|v| v.downgrade());
    let a_source_weak = audio_pipeline.source.map(|v| v.downgrade());
    decoder.connect_pad_added(move |_decoder, src_pad| {
        let Some(caps) = src_pad.current_caps() else { return };
        let Some(structure) = caps.structure(0) else { return };
        let name = structure.name();

        let v_source = v_source_weak.as_ref().and_then(WeakRef::upgrade);
        let a_source = a_source_weak.as_ref().and_then(WeakRef::upgrade);

        // eprintln!(
        //     "Pad added {name} - v_source={} a_source={}",
        //     v_source.is_some(),
        //     a_source.is_some()
        // );

        if name.starts_with("video/") {
            let Some(v_source) = v_source else { return };

            let sink_pad = v_source.static_pad("sink").unwrap();
            if sink_pad.is_linked() {
                eprintln!("[Video] Video sink pad already linked {name}");
                return;
            }
            if let Err(error) = src_pad.link(&sink_pad) {
                eprintln!("[Video] Failed to link video pad {name}: {error}");
            }
        } else if name.starts_with("audio/") {
            let Some(a_source) = a_source else { return };

            let sink_pad = a_source.static_pad("sink").unwrap();
            if sink_pad.is_linked() {
                eprintln!("[Audio] Audio sink pad already linked {name}");
                return;
            }
            if let Err(error) = src_pad.link(&sink_pad) {
                eprintln!("[Audio] Failed to link audio pad {name}: {error}");
            }
        } else if name.starts_with("image/") {
            let Some(v_source) = v_source else { return };

            let sink_pad = v_source.static_pad("sink").unwrap();
            if sink_pad.is_linked() {
                eprintln!("[Image] Video sink pad already linked {name}");
                return;
            }
            if let Err(error) = src_pad.link(&sink_pad) {
                eprintln!("[Image] Failed to link video pad {name}: {error}");
            }
        } else {
            eprintln!("Ignoring pad {name}");
        }
    });

    Ok(FilePipeline { pipeline, muxer, sink })
}

struct SubPipeline {
    source: Option<gstreamer::Element>,
    sink: gstreamer::Element,
}

fn create_video_pipeline(
    pipeline: &gstreamer::Pipeline,
    media_info: &MediaInfo,
    video_options: &VideoOptions,
    output: &Output,
) -> Result<SubPipeline, Error> {
    let queue = gstreamer::ElementFactory::make("queue").name("v_queue").build()?;
    let convert = gstreamer::ElementFactory::make("videoconvert").name("v_convert").build()?;

    let rate = gstreamer::ElementFactory::make("videorate").name("v_rate").build()?;
    let scale = gstreamer::ElementFactory::make("videoscale").name("v_scale").build()?;

    let caps = gstreamer::ElementFactory::make("capsfilter").name("v_caps").build()?;
    caps.set_property_from_str(
        "caps",
        &format!(
            "video/x-raw,width={width},\
             height={height},\
             pixel-aspect-ratio=1/1,\
             framerate={fps}/1",
            width = video_options.width,
            height = video_options.height,
            fps = video_options.fps
        ),
    );

    let timecodestamper = gstreamer::ElementFactory::make("timecodestamper")
        .name("v_timecodestamper")
        .build()?;

    let encoder = create_video_encoder(video_options)?;

    let parser = gstreamer::ElementFactory::make("h264parse").name("v_parse").build()?;
    parser.set_property("config-interval", 1i32);

    let timestamper = gstreamer::ElementFactory::make("h264timestamper")
        .name("v_timestamper")
        .build()?;

    pipeline.add_many([
        &queue,
        &convert,
        &rate,
        &scale,
        &caps,
        &timecodestamper,
        &encoder,
        &parser,
        &timestamper,
    ])?;
    gstreamer::Element::link_many([
        &queue,
        &convert,
        &rate,
        &scale,
        &caps,
        &timecodestamper,
        &encoder,
        &parser,
        &timestamper,
    ])?;

    let mut sub_pipeline = SubPipeline { source: None, sink: timestamper };

    if media_info.video.is_some() {
        sub_pipeline.source = Some(queue);
    } else if media_info.image.is_some() {
        let image = gstreamer::ElementFactory::make("imagefreeze").name("image").build()?;
        image.set_property("is-live", true);

        let duration_secs = media_info.play_duration().seconds() as i32;
        let loop_count = duration_secs * video_options.fps as i32;
        image.set_property("num-buffers", loop_count);

        pipeline.add(&image)?;
        image.link(&queue)?;
        sub_pipeline.source = Some(image);
    } else {
        let video = gstreamer::ElementFactory::make("videotestsrc").name("video").build()?;
        video.set_property_from_str("pattern", "black");

        pipeline.add(&video)?;
        video.link(&queue)?;
        // sub_pipeline.source = Some(video);
    }

    Ok(sub_pipeline)
}

fn create_audio_pipeline(
    pipeline: &gstreamer::Pipeline,
    media_info: &MediaInfo,
    video_options: &VideoOptions,
    output: &Output,
) -> Result<SubPipeline, Error> {
    let queue = gstreamer::ElementFactory::make("queue").name("a_queue").build()?;
    let convert = gstreamer::ElementFactory::make("audioconvert").name("a_convert").build()?;
    let resample = gstreamer::ElementFactory::make("audioresample").name("a_resample").build()?;

    let caps = gstreamer::ElementFactory::make("capsfilter").name("a_caps").build()?;
    caps.set_property_from_str("caps", "audio/x-raw,channels=2,rate=44100");

    let encoder = gstreamer::ElementFactory::make("avenc_aac").name("a_encode").build()?;
    // encoder.set_property("bitrate", 128000i32);

    let parser = gstreamer::ElementFactory::make("aacparse").name("a_parse").build()?;

    pipeline.add_many([&queue, &convert, &resample, &caps, &encoder, &parser])?;
    gstreamer::Element::link_many([&queue, &convert, &resample, &caps, &encoder, &parser])?;

    let mut sub_pipeline = SubPipeline { source: None, sink: parser };
    if media_info.audio.is_some() {
        sub_pipeline.source = Some(queue);
    } else {
        let audio = gstreamer::ElementFactory::make("audiotestsrc").name("audio").build()?;
        audio.set_property("is-live", true);
        audio.set_property("volume", 0.0);
        if audio.has_property("wave") {
            audio.set_property_from_str("wave", "silence");
        }

        let duration_secs = media_info.play_duration().seconds_f64();
        // TODO: How do I calculate max-buffers?

        let rate = 44100.0;
        let buffer_count = ((duration_secs * rate) / 1024.0).ceil() as i32;
        audio.set_property("num-buffers", buffer_count);

        pipeline.add(&audio)?;
        audio.link(&queue)?;
        // sub_pipeline.source = Some(audio);
    }

    Ok(sub_pipeline)
}

fn create_video_encoder(options: &VideoOptions) -> Result<gstreamer::Element, Error> {
    if let Ok(encoder) = create_video_encoder_inner(options, "nvh264enc") {
        eprintln!("[Streamer] Using nvh264enc");
        return Ok(encoder);
    }

    if let Ok(encoder) = create_video_encoder_inner(options, "vah264enc") {
        eprintln!("[Streamer] Using vah264enc");
        return Ok(encoder);
    }

    create_video_encoder_inner(options, "x264enc")
}

fn create_video_encoder_inner(
    options: &VideoOptions,
    factory: &str,
) -> Result<gstreamer::Element, Error> {
    let encoder = gstreamer::ElementFactory::make(factory).name("v_encode").build()?;

    if encoder.has_property("tune") {
        let tune_value = if factory == "nvh264enc" {
            NvH264Tune::UltraLowLatency.as_str()
        } else {
            X264Tune::ZeroLatency.as_str()
        };
        encoder.set_property_from_str("tune", tune_value);
    }

    if encoder.has_property("key-int-max") {
        encoder.set_property("key-int-max", 30u32);
    }

    if encoder.has_property("bitrate") {
        encoder.set_property("bitrate", options.bitrate);
    }

    Ok(encoder)
}
