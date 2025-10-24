mod new_pipeline;

use glib::object::ObjectExt;
use gstreamer::prelude::{ElementExt, GstBinExt, PadExt, PadExtManual};

use self::new_pipeline::FilePipeline;
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
    #[error(transparent)]
    Flow(#[from] gstreamer::FlowError),
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub enum Output {
    Rtmp(String),
    Srt(String),
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct VideoOptions {
    pub width: u32,
    pub height: u32,
    pub fps: u8,
    pub bitrate: u32,
}

impl VideoOptions {
    pub const HD_720: Self = Self {
        width: 1280,
        height: 720,
        fps: 30,
        // ~3Mbps
        bitrate: 3000,
    };

    pub const PIXEL_9_PRO_FOLD: Self = Self {
        width: 2076,
        height: 2152,
        fps: 30,
        // ~12Mbps
        bitrate: 12000,
    };

    pub const PIXEL_9_PRO_FOLD_LIGHT: Self = Self {
        width: 696,
        height: 722,
        fps: 30,
        // ~3Mbps
        bitrate: 3000,
    };
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

#[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum NvH264Tune {
    Default,
    HighQuality,
    LowLatency,
    UltraLowLatency,
    Lossless,
}

impl NvH264Tune {
    fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::HighQuality => "high-quality",
            Self::LowLatency => "low-latency",
            Self::UltraLowLatency => "ultra-low-latency",
            Self::Lossless => "lossless",
        }
    }
}

fn run_pipeline(
    pipeline: &gstreamer::Pipeline,
    abort_signal: &flume::Receiver<()>,
) -> Result<(), Error> {
    let bus = pipeline.bus().unwrap();

    'main: loop {
        if let Ok(()) = abort_signal.recv_timeout(std::time::Duration::from_millis(100)) {
            break 'main;
        }

        for msg in bus.iter_timed(gstreamer::ClockTime::from_mseconds(100)) {
            use gstreamer::MessageView;
            match msg.view() {
                MessageView::Eos(..) => {
                    break 'main;
                }
                MessageView::Error(err) => {
                    eprintln!(
                        "[Streamer] Error on pipeline: {} (debug: {:?})",
                        err.error(),
                        err.debug()
                    );
                    break 'main;
                }
                _ => (),
            }
        }
    }

    Ok(())
}

fn transcode_files(
    file_rx: flume::Receiver<Source>,
    output: Output,
    video: &VideoOptions,
) -> Result<(), Error> {
    let mut sink = None;

    loop {
        let source = match file_rx.recv() {
            Ok(source) => source,
            Err(flume::RecvError::Disconnected) => {
                eprintln!("[Streamer] File channel closed. Shutting down.");
                break;
            }
        };

        println!("Creating pipeline for source: {source:?}");
        let result =
            FilePipeline::create(&source.path, &source.media_info, video, &output, sink.clone());
        let pipeline = match result {
            Ok(pipeline) => pipeline,
            Err(error) => {
                eprintln!("[Streamer] Failed to create pipeline for source: {source:?}: {error}");
                continue;
            }
        };

        if sink.is_none() {
            sink = Some(pipeline.sink.clone());
        }

        let (eos_tx, eos_rx) = flume::bounded(1);
        {
            let pipeline_weak = pipeline.pipeline.downgrade();
            let sink_weak = pipeline.sink.downgrade();

            let muxer_pad = pipeline.muxer.static_pad("src").unwrap();
            muxer_pad.add_probe(gstreamer::PadProbeType::EVENT_DOWNSTREAM, move |pad, info| {
                if let Some(event) = info.event()
                    && event.type_() == gstreamer::EventType::Eos
                {
                    let Some(pipeline) = pipeline_weak.upgrade() else {
                        return gstreamer::PadProbeReturn::Ok;
                    };
                    let Some(sink) = sink_weak.upgrade() else {
                        return gstreamer::PadProbeReturn::Ok;
                    };

                    if let Err(error) = sink.set_state(gstreamer::State::Paused) {
                        eprintln!("[Streamer] Failed to pause sink: {error}");
                    }

                    let sink_pad = sink.static_pad("sink").unwrap();
                    if let Err(error) = pad.unlink(&sink_pad) {
                        eprintln!("[Streamer] Failed to unlink muxer pad from sink pad: {error}");
                    }
                    if let Err(error) = pipeline.remove(&sink) {
                        eprintln!("[Streamer] Failed to remove sink from pipeline: {error}");
                    }

                    eos_tx.send(()).unwrap();
                    return gstreamer::PadProbeReturn::Drop;
                }
                gstreamer::PadProbeReturn::Ok
            });
        }

        println!("[Streamer] Starting pipeline for {}", source.path.display());

        pipeline.pipeline.set_state(gstreamer::State::Playing)?;

        {
            static mut STARTED: bool = false;

            #[allow(unsafe_code)]
            if unsafe { !STARTED } {
                #[allow(unsafe_code)]
                unsafe {
                    STARTED = true
                };
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
        }

        if let Err(error) = run_pipeline(&pipeline.pipeline, &eos_rx) {
            eprintln!(
                "[Streamer] Failed to run pipeline for file {}: {error}",
                source.path.display()
            );
        } else {
            println!("[Streamer] Finished file: {}", source.path.display());
        }

        _ = pipeline.pipeline.set_state(gstreamer::State::Null);
    }

    if let Some(sink) = sink {
        _ = sink.set_state(gstreamer::State::Null);
    }

    Ok(())
}

pub fn start_streamer_task(
    file_rx: flume::Receiver<Source>,
    output: Output,
    options: VideoOptions,
) {
    std::thread::spawn(move || {
        println!("[Streamer] Streamer task started.");

        match transcode_files(file_rx, output, &options) {
            Ok(()) => println!("[Streamer] Streamer task finished."),
            Err(error) => eprintln!("[Streamer] Streamer task failed: {error}"),
        }
    });
}
