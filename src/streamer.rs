use std::path::Path;

use glib::GString;
use glib::object::ObjectExt;
use gstreamer::gobject::GObjectExtManualGst;
use gstreamer::prelude::{
    ElementExt, ElementExtManual, GstBinExt, GstBinExtManual, GstObjectExt, PadExt, PadExtManual,
};

use crate::finder::Source;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Glib(#[from] glib::Error),
    #[error(transparent)]
    GlibBool(#[from] glib::BoolError),
    #[error(transparent)]
    StateChange(#[from] gstreamer::StateChangeError),
    #[error(transparent)]
    PadLink(#[from] gstreamer::PadLinkError),
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub enum Output {
    Rtmp(String),
    Srt(String),
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum X264Tune {
    ZeroLatency,
    FastDecode,
    Film,
    Animation,
}

impl X264Tune {
    fn as_str(self) -> &'static str {
        match self {
            Self::ZeroLatency => "zerolatency",
            Self::FastDecode => "fastdecode",
            Self::Film => "film",
            Self::Animation => "animation",
        }
    }
}

struct SourcePipeline {
    source: gstreamer::Element,
    decoder: gstreamer::Element,
}

impl SourcePipeline {
    fn create() -> Result<Self, Error> {
        let source = gstreamer::ElementFactory::make("filesrc").name("source").build()?;
        let decoder = gstreamer::ElementFactory::make("decodebin").name("decode").build()?;
        Ok(Self { source, decoder })
    }

    fn link(&self) -> Result<(), Error> {
        self.source.link(&self.decoder)?;
        Ok(())
    }

    fn elements(&self) -> impl IntoIterator<Item = &gstreamer::Element> {
        [&self.source, &self.decoder]
    }

    fn set_location(&self, path: &Path) {
        self.source.set_property("location", path);
    }

    fn start(&self) {
        // Bring them back up; decodebin will re-emit stream-start -> caps -> segment
        let _ = self.source.set_state(gstreamer::State::Playing);
        let _ = self.decoder.set_state(gstreamer::State::Playing);
    }

    fn stop(&self) {
        // Collect dynamic src pads and unlink if linked
        for pad in self.decoder.src_pads() {
            if let Some(peer) = pad.peer() {
                let _ = pad.unlink(&peer);
            }
        }

        _ = self.decoder.set_state(gstreamer::State::Null);
        _ = self.source.set_state(gstreamer::State::Null);
    }
}

struct VideoPipeline {
    // image: gstreamer::Element,
    convert: gstreamer::Element,
    rate: gstreamer::Element,
    scale: gstreamer::Element,
    caps: gstreamer::Element,
    timecodestamper: Option<gstreamer::Element>,
    encoder: gstreamer::Element,
    timestamper: Option<gstreamer::Element>,
    queue: gstreamer::Element,
}

impl VideoPipeline {
    fn create() -> Result<Self, Error> {
        // let image = gstreamer::ElementFactory::make("imagefreeze").name("v_image").build()?;
        // image.set_property("is-live", true);

        let convert = gstreamer::ElementFactory::make("videoconvert").name("v_convert").build()?;
        let rate = gstreamer::ElementFactory::make("videorate").name("v_rate").build()?;
        let scale = gstreamer::ElementFactory::make("videoscale").name("v_scale").build()?;

        let caps = gstreamer::ElementFactory::make("capsfilter").name("v_caps").build()?;
        caps.set_property_from_str(
            "caps",
            "video/x-raw,width=1280,height=720,pixel-aspect-ratio=1/1,framerate=30/1",
        );

        let mut timecodestamper = None;
        match gstreamer::ElementFactory::make("timecodestamper")
            .name("v_timecodestamper")
            .build()
        {
            Ok(t) => {
                timecodestamper = Some(t);
                eprintln!("[Streamer] Using timecodestamper");
            }
            Err(error) => {
                eprintln!("[Streamer] Not using timecodestamper: {error}");
            }
        }

        let mut encoder = None;
        for name in ["nvh264enc", "vah264enc"] {
            match gstreamer::ElementFactory::make(name).name("v_encode").build() {
                Ok(e) => {
                    encoder = Some(e);
                    eprintln!("[Streamer] Using {name}");
                    break;
                }
                Err(error) => {
                    eprintln!("[Streamer] Not using {name}: {error}");
                }
            }
        }

        let encoder = if let Some(encoder) = encoder {
            encoder
        } else {
            gstreamer::ElementFactory::make("x264enc").name("v_encode").build()?
        };

        if encoder.has_property("tune") {
            encoder.set_property_from_str("tune", X264Tune::ZeroLatency.as_str());
        }
        if encoder.has_property("byte-stream") {
            encoder.set_property("byte-stream", true);
        }
        encoder.set_property("key-int-max", 30u32);
        encoder.set_property("bitrate", 3000u32);

        let mut timestamper = None;
        match gstreamer::ElementFactory::make("h264timestamper").name("v_timestamper").build() {
            Ok(t) => {
                timestamper = Some(t);
                eprintln!("[Streamer] Using h264timestamper");
            }
            Err(error) => {
                eprintln!("[Streamer] Not using h264timestamper: {error}");
            }
        }

        // Queue to decouple muxer from video encoder
        let queue = gstreamer::ElementFactory::make("queue").name("v_queue").build()?;

        Ok(Self {
            convert,
            rate,
            scale,
            caps,
            timecodestamper,
            encoder,
            timestamper,
            queue,
        })
    }

    fn link(&self, pipeline: &gstreamer::Pipeline) -> Result<(), Error> {
        // TODO: image
        gstreamer::Element::link_many([&self.convert, &self.rate, &self.scale])?;

        let has_add_borders = self.scale.has_property("add-borders");
        if has_add_borders {
            self.scale.set_property("add-borders", true);

            self.scale.link(&self.caps)?;
        } else {
            let compositor =
                gstreamer::ElementFactory::make("compositor").name("v_compositor").build()?;
            // Black
            compositor.set_property("background", 1);

            pipeline.add(&compositor)?;

            self.scale.link(&compositor)?;
            compositor.link(&self.caps)?;
        }

        let iter = [&self.caps]
            .into_iter()
            .chain(self.timecodestamper.as_ref())
            .chain([&self.encoder])
            .chain(self.timestamper.as_ref())
            .chain([&self.queue]);
        gstreamer::Element::link_many(iter)?;

        Ok(())
    }

    fn elements(&self) -> impl IntoIterator<Item = &gstreamer::Element> {
        [&self.convert, &self.rate, &self.scale, &self.caps]
            .into_iter()
            .chain(self.timecodestamper.as_ref())
            .chain([&self.encoder])
            .chain(self.timestamper.as_ref())
            .chain([&self.queue])
    }
}

struct AudioPipeline {
    convert: gstreamer::Element,
    resample: gstreamer::Element,
    caps: gstreamer::Element,
    encoder: gstreamer::Element,
    queue: gstreamer::Element,
}

impl AudioPipeline {
    fn create() -> Result<Self, Error> {
        let convert = gstreamer::ElementFactory::make("audioconvert").name("a_convert").build()?;
        let resample =
            gstreamer::ElementFactory::make("audioresample").name("a_resample").build()?;

        // Force raw audio caps that avenc_aac reliably accepts
        let caps = gstreamer::ElementFactory::make("capsfilter").name("a_caps").build()?;
        caps.set_property_from_str("caps", "audio/x-raw,channels=2,rate=48000");

        let encoder = gstreamer::ElementFactory::make("avenc_aac").name("a_encode").build()?;
        encoder.set_property("bitrate", 128000i32);

        // Queue to decouple muxer push
        let queue = gstreamer::ElementFactory::make("queue").name("a_queue").build()?;

        Ok(Self { convert, resample, caps, encoder, queue })
    }

    fn link(&self) -> Result<(), Error> {
        gstreamer::Element::link_many([
            &self.convert,
            &self.resample,
            &self.caps,
            &self.encoder,
            &self.queue,
        ])?;
        Ok(())
    }

    fn elements(&self) -> impl IntoIterator<Item = &gstreamer::Element> {
        [&self.convert, &self.resample, &self.caps, &self.encoder, &self.queue]
    }
}

struct OutputPipeline {
    muxer: gstreamer::Element,
    sink: gstreamer::Element,
}

impl OutputPipeline {
    fn create(output: Output) -> Result<Self, Error> {
        let muxer: gstreamer::Element;
        let sink: gstreamer::Element;

        match output {
            Output::Rtmp(url) => {
                muxer = gstreamer::ElementFactory::make("flvmux").name("muxer").build()?;
                muxer.set_property("streamable", true);

                sink = gstreamer::ElementFactory::make("rtmpsink").name("sink").build()?;
                sink.set_property("location", url);

                setup_muxer_drop_eos(&muxer);
                muxer.link(&sink)?;
            }
            Output::Srt(url) => {
                muxer = gstreamer::ElementFactory::make("mpegtsmux").name("muxer").build()?;
                // Make sure the muxer can start streaming immediately
                muxer.set_property("alignment", 7i32);

                sink = gstreamer::ElementFactory::make("srtsink").name("sink").build()?;
                sink.set_property("uri", url);

                setup_muxer_drop_eos(&muxer);
                muxer.link(&sink)?;
            }
        }

        Ok(Self { muxer, sink })
    }

    fn link(&self) -> Result<(), Error> {
        gstreamer::Element::link_many([&self.muxer, &self.sink])?;
        Ok(())
    }

    fn elements(&self) -> impl IntoIterator<Item = &gstreamer::Element> {
        [&self.muxer, &self.sink]
    }
}

/*
    TODO: Support the following media file scenarios:
      - Image file: stream image for 5 seconds with silent audio
      - Audio file: stream audio with black background
      - Video file: stream video and audio, add silent audio if file does not have any

    Output must be consistent with the following options:
      - Video: 720p, 30 fps, black borders
      - Audio: AAC, stereo, 48 kHz, 128 kbps

    Output stream must never disconnect, the server should never notice that a different file is being streamed.
*/
struct Pipeline {
    pipeline: gstreamer::Pipeline,
    source: SourcePipeline,
    video: VideoPipeline,
    audio: AudioPipeline,
    output: OutputPipeline,
    eos_rx: flume::Receiver<()>,
    playing: bool,
}

impl Pipeline {
    fn create(output: Output) -> Result<Self, Error> {
        let (eos_tx, eos_rx) = flume::bounded(1);

        let pipeline = gstreamer::Pipeline::new();

        let source = SourcePipeline::create()?;
        let video = VideoPipeline::create()?;
        let audio = AudioPipeline::create()?;

        let output = OutputPipeline::create(output)?;

        let elements = source
            .elements()
            .into_iter()
            .chain(video.elements())
            .chain(audio.elements())
            .chain(output.elements());

        pipeline.add_many(elements)?;

        source.link()?;
        video.link(&pipeline)?;
        audio.link()?;

        output.link()?;
        video.queue.link(&output.muxer)?;
        audio.queue.link(&output.muxer)?;

        let v_convert_weak = video.convert.downgrade();
        // let v_image_weak = video.image.downgrade();
        let a_convert_weak = audio.convert.downgrade();
        source.decoder.connect_pad_added(move |_decoder, pad| {
            let caps = pad.current_caps().unwrap_or_else(|| pad.query_caps(None));
            let Some(s) = caps.structure(0) else { return };
            let media_type = s.name();

            if media_type.starts_with("video/") {
                eprintln!("[Streamer] Video stream: {caps}");
                let Some(v_convert) = v_convert_weak.upgrade() else { return };
                let Some(sink) = v_convert.static_pad("sink") else { return };
                if sink.is_linked() {
                    eprintln!("[Streamer] Sink already linked");
                    return;
                }
                if let Err(error) = pad.link(&sink) {
                    eprintln!("Failed to link video pad -> x264enc: {error}");
                } else {
                    // Ensure downstream gets a fresh time segment before buffers
                    send_segment_start(&sink)
                }
            } else if media_type.starts_with("audio/") {
                eprintln!("[Streamer] Audio stream: {caps}");
                let Some(a_convert) = a_convert_weak.upgrade() else { return };
                let Some(sink) = a_convert.static_pad("sink") else { return };
                if sink.is_linked() {
                    eprintln!("[Streamer] Sink already linked");
                    return;
                }
                if let Err(error) = pad.link(&sink) {
                    eprintln!("Failed to link audio pad -> audioconvert: {error}");
                } else {
                    // Ensure downstream gets a fresh time segment before buffers
                    send_segment_start(&sink)
                }
            } else if media_type.starts_with("image/") {
                eprintln!("[Streamer] Image stream: {caps}");
                let Some(v_convert) = v_convert_weak.upgrade() else { return };
                let Some(sink) = v_convert.static_pad("sink") else { return };
                if sink.is_linked() {
                    eprintln!("[Streamer] Sink already linked");
                    return;
                }
                if let Err(error) = pad.link(&sink) {
                    eprintln!("Failed to link image pad -> imagefreeze: {error}");
                } else {
                    // Ensure downstream gets a fresh time segment before buffers
                    send_segment_start(&sink)
                }

                // TODO: Need imagefreeze
                //
                // let eos_tx = eos_tx.clone();
                // std::thread::spawn(move || {
                //     std::thread::sleep(std::time::Duration::from_secs(5));
                //     _ = eos_tx.try_send(());
                // });
            } else {
                eprintln!("[Streamer] Unhandled pad media-type: {media_type} - {pad:?}");
            }

            let eos_tx = eos_tx.clone();
            pad.add_probe(gstreamer::PadProbeType::EVENT_DOWNSTREAM, move |pad, info| {
                if let Some(gstreamer::PadProbeData::Event(event)) = &info.data
                    && event.type_() == gstreamer::EventType::Eos
                {
                    eprintln!("[Streamer] EOS received on pad: {pad:?}");

                    send_segment_start(pad);

                    // Notify control loop to switch the file
                    let _ = eos_tx.try_send(());

                    // Drop the original EOS so it doesn't reach muxer (extra safety)
                    return gstreamer::PadProbeReturn::Drop;
                }
                gstreamer::PadProbeReturn::Ok
            });
        });

        Ok(Self { pipeline, source, audio, video, output, eos_rx, playing: false })
    }

    fn wait_for_eos(&self) {
        let bus = self.pipeline.bus().unwrap();

        loop {
            if let Ok(()) = self.eos_rx.recv_timeout(std::time::Duration::from_millis(100)) {
                break;
            }

            for msg in bus.iter_timed(gstreamer::ClockTime::from_mseconds(100)) {
                use gstreamer::MessageView;

                match msg.view() {
                    MessageView::Error(err) => {
                        eprintln!(
                            "Error received from element {:?}: {}",
                            err.src().map(|s| s.path_string()),
                            err.error()
                        );
                        eprintln!(
                            "Debugging info: {}",
                            err.debug().unwrap_or(GString::from("None"))
                        );
                        break;
                    }
                    MessageView::Eos(eos) => {
                        println!("End-Of-Stream received: {eos:?}");
                        break;
                    }
                    _ => (), // Ignore other messages
                }
            }
        }
    }

    fn stop(&mut self) {
        if !self.playing {
            return;
        }

        self.pipeline
            .set_state(gstreamer::State::Null)
            .expect("Failed to set pipeline to Null state");
        self.playing = false;
    }

    fn stream_file(&mut self, path: &Path) -> Result<(), Error> {
        if self.playing {
            self.source.stop();
        }

        self.source.set_location(path);

        // Ensure no duplicate EOS is pending.
        for _ in self.eos_rx.drain() {}

        eprintln!("[Streamer] Starting pipeline: {}", path.display());

        if !self.playing {
            self.pipeline.set_state(gstreamer::State::Playing)?;
            self.playing = true;

            #[cfg(debug_assertions)]
            std::thread::spawn(|| {
                std::thread::sleep(std::time::Duration::from_secs(2));
                eprintln!("Spawning VLC process to play stream");
                let mut child = std::process::Command::new("vlc")
                    .arg("rtsp://127.0.0.1:8554/my_stream")
                    .spawn()
                    .unwrap();
                child.wait().unwrap();
            });
        }

        // if path.extension().map(|e| e == "jpg").unwrap_or(false) {
        //
        // }

        self.source.start();
        self.wait_for_eos();

        Ok(())
    }
}

fn setup_muxer_drop_eos(muxer: &gstreamer::Element) {
    let muxer_on_add_probe = |_pad: &gstreamer::Pad, info: &mut gstreamer::PadProbeInfo| {
        if let Some(gstreamer::PadProbeData::Event(event)) = &info.data
            && event.type_() == gstreamer::EventType::Eos
        {
            return gstreamer::PadProbeReturn::Drop;
        }
        gstreamer::PadProbeReturn::Ok
    };

    muxer.connect_pad_added(move |_muxer, pad| {
        pad.add_probe(gstreamer::PadProbeType::EVENT_DOWNSTREAM, muxer_on_add_probe);
    });
    // Add probes to already present sink pads.
    for pad in muxer.sink_pads() {
        pad.add_probe(gstreamer::PadProbeType::EVENT_DOWNSTREAM, muxer_on_add_probe);
    }
}

fn send_segment_start(pad: &gstreamer::Pad) {
    // Flush downstream so any pending data/EOS is cleared
    // decodebin will re-emit sticky events for the next file
    let _ = pad.send_event(gstreamer::event::FlushStart::new());
    let _ = pad.send_event(gstreamer::event::FlushStop::new(false));

    let has_start = pad.sticky_event::<gstreamer::event::StreamStart>(0).is_some();
    let has_caps = pad.sticky_event::<gstreamer::event::Caps>(0).is_some();

    if !has_start || !has_caps {
        return;
    }

    // Start a new segment at running-time 0 for the next file chunk
    let mut segment = gstreamer::FormattedSegment::<gstreamer::ClockTime>::new();
    segment.set_rate(1.0);
    segment.set_flags(gstreamer::SegmentFlags::RESET);
    segment.set_start(gstreamer::ClockTime::from_nseconds(0));
    let _ = pad.send_event(gstreamer::event::Segment::new(&segment));
}

fn transcode_files(file_rx: flume::Receiver<Source>, output: Output) -> Result<(), Error> {
    let mut pipeline = Pipeline::create(output)?;

    loop {
        let source = match file_rx.recv() {
            Ok(source) => source,
            Err(flume::RecvError::Disconnected) => {
                eprintln!("[Streamer] File channel closed. Shutting down.");
                break;
            }
        };

        println!("[Streamer] Starting source: {source:?}");

        if let Err(error) = pipeline.stream_file(&source.path) {
            eprintln!("[Streamer] Failed to transcode file {}: {error}", source.path.display());
        } else {
            println!("[Streamer] Finished file: {}", source.path.display());
        }
    }

    pipeline.stop();

    Ok(())
}

pub fn start_streamer_task(file_rx: flume::Receiver<Source>, output: Output) {
    std::thread::spawn(move || {
        println!("[Streamer] Streamer task started.");

        match transcode_files(file_rx, output) {
            Ok(()) => println!("[Streamer] Streamer task finished."),
            Err(error) => eprintln!("[Streamer] Streamer task failed: {error}"),
        }
    });
}
