use std::path::Path;
use std::sync::Arc;

use glib::GString;
use glib::object::{Cast, ObjectExt};
use gstreamer::prelude::{
    ElementExt, ElementExtManual, GstBinExtManual, GstObjectExt, PadExt, PadExtManual,
};
use parking_lot::Mutex;

use super::Error;

#[derive(Clone)]
pub struct InputBin {
    pub bin: gstreamer::Bin,

    uri_decode: gstreamer::Element,

    video_test_src: gstreamer::Element,
    image_freeze: gstreamer::Element,
    video_convert: gstreamer::Element,

    audio_test_src: gstreamer::Element,
    audio_convert: gstreamer::Element,
}

impl InputBin {
    pub fn new<S>(name: S) -> Result<Self, Error>
    where
        S: Into<GString>,
    {
        let bin = gstreamer::Bin::builder().name(name).build();

        let uri_decode =
            gstreamer::ElementFactory::make("uridecodebin3").name("uri_decode").build()?;

        // Video chain
        let video_test_src = gstreamer::ElementFactory::make("videotestsrc")
            .name("video_black")
            .property_from_str("pattern", "black")
            .property("is-live", true)
            .build()?;
        let image_freeze =
            gstreamer::ElementFactory::make("imagefreeze").name("image_freeze").build()?;
        let video_convert =
            gstreamer::ElementFactory::make("videoconvert").name("video_convert").build()?;
        let video_scale = gstreamer::ElementFactory::make("videoscale")
            .name("video_scale")
            .property("add-borders", true)
            .build()?;

        let title_overlay = gstreamer::ElementFactory::make("textoverlay")
            .name("title_overlay")
            .property_from_str("valignment", "bottom") // top, center, bottom
            .property_from_str("halignment", "left") // left, center, right
            .property_from_str("font-desc", "Sans, 6")
            .property_from_str("wrap-mode", "wordchar") // none, word, char, wordchar
            .build()?;

        let duration = Arc::new(Mutex::new(None));
        let counter_overlay = create_counter_overlay("counter_overlay", duration.clone())?;

        let video_caps_filter = gstreamer::ElementFactory::make("capsfilter")
            .property(
                "caps",
                gstreamer::Caps::builder("video/x-raw")
                    // .field("format", gstreamer_video::VideoFormat::I420)
                    .field("width", 1280)
                    .field("height", 720)
                    .field("pixel-aspect-ratio", gstreamer::Fraction::new(1, 1))
                    .build(),
            )
            .build()?;
        let video_queue = gstreamer::ElementFactory::make("queue").name("video_queue").build()?;

        // Audio chain
        let audio_test_src = gstreamer::ElementFactory::make("audiotestsrc")
            .name("audio_silent")
            .property_from_str("wave", "silence")
            .property("volume", 0.0)
            .property("is-live", true)
            .build()?;
        let audio_convert =
            gstreamer::ElementFactory::make("audioconvert").name("audio_convert").build()?;
        let audio_rate = gstreamer::ElementFactory::make("audiorate").name("audio_rate").build()?;
        let audio_resample = gstreamer::ElementFactory::make("audioresample")
            .name("audio_resample")
            .build()?;
        let audio_caps_filter = gstreamer::ElementFactory::make("capsfilter")
            .name("audio_caps_filter")
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
        let audio_queue = gstreamer::ElementFactory::make("queue").name("audio_queue").build()?;

        // Add elements
        bin.add_many([
            &uri_decode,
            // Video
            &video_test_src,
            &image_freeze,
            &video_convert,
            &video_scale,
            &title_overlay,
            &counter_overlay,
            &video_caps_filter,
            &video_queue,
            // Audio
            &audio_test_src,
            &audio_convert,
            &audio_rate,
            &audio_resample,
            &audio_caps_filter,
            &audio_queue,
        ])?;

        // Link video elements
        gstreamer::Element::link_many([
            &video_convert,
            &video_scale,
            &title_overlay,
            &counter_overlay,
            &video_caps_filter,
            &video_queue,
        ])?;

        // Link audio elements
        gstreamer::Element::link_many([
            &audio_convert,
            &audio_rate,
            &audio_resample,
            &audio_caps_filter,
            &audio_queue,
        ])?;

        // Imagefreeze end stream
        let duration_clone = duration.clone();
        let image_freeze_src_pad = image_freeze.static_pad("src").unwrap();
        let video_convert_weak = video_convert.downgrade();
        let audio_convert_weak = audio_convert.downgrade();
        image_freeze_src_pad.add_probe(gstreamer::PadProbeType::BUFFER, move |_pad, info| {
            if let Some(duration) = *duration_clone.lock()
                && let Some(buffer) = info.buffer()
                && let Some(pts) = buffer.pts()
                && pts > duration
            {
                if let Some(video_convert) = video_convert_weak.upgrade() {
                    let src_pad = video_convert.static_pad("src").unwrap();
                    src_pad.push_event(gstreamer::event::Eos::new());
                }
                if let Some(audio_convert) = audio_convert_weak.upgrade() {
                    let src_pad = audio_convert.static_pad("src").unwrap();
                    src_pad.push_event(gstreamer::event::Eos::new());
                }

                return gstreamer::PadProbeReturn::Remove;
            }
            gstreamer::PadProbeReturn::Ok
        });
        // Decodebin dynamic linking and set duration/title for overlays
        let video_convert_weak = video_convert.downgrade();
        let image_freeze_weak = image_freeze.downgrade();
        let audio_convert_weak = audio_convert.downgrade();

        let video_test_src_weak = video_test_src.downgrade();
        let audio_test_src_weak = audio_test_src.downgrade();

        let uri_source_weak = uri_decode.downgrade();
        let title_overlay_weak = title_overlay.downgrade();

        uri_decode.connect_pad_added(move |decodebin, pad| {
            let pad_name = pad.name();
            println!("Decoder: New pad added: {pad_name}");

            let decodebin = decodebin.downcast_ref::<gstreamer::Bin>().unwrap();
            let stream_duration = decodebin
                .query_duration::<gstreamer::ClockTime>()
                .unwrap_or(gstreamer::ClockTime::ZERO);

            let fixed_duration = if stream_duration == gstreamer::ClockTime::ZERO {
                gstreamer::ClockTime::from_seconds(5)
            } else {
                stream_duration
            };

            *duration.lock() = Some(fixed_duration);

            if pad_name.starts_with("video_") {
                if let Some(video_test_src) = video_test_src_weak.upgrade() {
                    if let Err(error) = video_test_src.set_state(gstreamer::State::Null) {
                        eprintln!("Failed to set video test src to null: {error}");
                    }
                    video_test_src.set_locked_state(true);
                }

                // Unlink videotestsrc -> videoconvert
                let Some(video_convert) = video_convert_weak.upgrade() else { return };
                let video_convert_sink_pad = video_convert.static_pad("sink").unwrap();
                if video_convert_sink_pad.is_linked() {
                    let peer = video_convert_sink_pad.peer().unwrap();
                    if let Err(error) = peer.unlink(&video_convert_sink_pad) {
                        eprintln!("Failed to unlink videoconvert: {error}");
                    }
                }

                // Note: This relies on the decodebin duration being correct.
                let is_image = stream_duration == gstreamer::ClockTime::ZERO;
                if is_image {
                    let Some(image_freeze) = image_freeze_weak.upgrade() else { return };

                    // Link pad -> imagefreeze
                    let image_freeze_sink_pad = image_freeze.static_pad("sink").unwrap();
                    if let Err(error) = pad.link(&image_freeze_sink_pad) {
                        eprintln!("Failed to link imagefreeze: {error}");
                    }

                    // Link imagefreeze -> videoconvert
                    let image_freeze_src_pad = image_freeze.static_pad("src").unwrap();
                    if let Err(error) = image_freeze_src_pad.link(&video_convert_sink_pad) {
                        eprintln!("Failed to link imagefreeze -> videoconvert: {error}");
                    }
                } else {
                    // Link pad -> videoconvert
                    if let Err(error) = pad.link(&video_convert_sink_pad) {
                        eprintln!("Failed to link videoconvert: {error}");
                    }
                }
            } else if pad_name.starts_with("audio_") {
                if let Some(audio_test_src) = audio_test_src_weak.upgrade() {
                    if let Err(error) = audio_test_src.set_state(gstreamer::State::Null) {
                        eprintln!("Failed to set audio test src to null: {error}");
                    }
                    audio_test_src.set_locked_state(true);
                }

                // Unlink audiotestsrc -> audioconvert
                let Some(audio_convert) = audio_convert_weak.upgrade() else { return };
                let audio_convert_sink_pad = audio_convert.static_pad("sink").unwrap();
                if audio_convert_sink_pad.is_linked() {
                    let peer = audio_convert_sink_pad.peer().unwrap();
                    if let Err(error) = peer.unlink(&audio_convert_sink_pad) {
                        eprintln!("Failed to unlink audioconvert: {error}");
                    }
                }

                // Link pad -> audioconvert
                if let Err(error) = pad.link(&audio_convert_sink_pad) {
                    eprintln!("Failed to link audioconvert: {error}");
                }
            } else {
                eprintln!("Unknown pad type: {pad_name}");
            }

            if let Some(uri_source) = uri_source_weak.upgrade()
                && let Some(title_overlay) = title_overlay_weak.upgrade()
            {
                let uri = uri_source.property::<GString>("uri");
                title_overlay.set_property("text", uri);
            }
        });

        let video_queue_src_pad = video_queue.static_pad("src").unwrap();
        let audio_queue_src_pad = audio_queue.static_pad("src").unwrap();

        let video_ghost_pad = gstreamer::GhostPad::builder(gstreamer::PadDirection::Src)
            .with_target(&video_queue_src_pad)?
            .name("video_src")
            .build();

        let audio_ghost_pad = gstreamer::GhostPad::builder(gstreamer::PadDirection::Src)
            .with_target(&audio_queue_src_pad)?
            .name("audio_src")
            .build();

        bin.add_pad(&video_ghost_pad)?;
        bin.add_pad(&audio_ghost_pad)?;

        Ok(Self {
            bin,
            uri_decode,
            video_test_src,
            image_freeze,
            video_convert,
            audio_test_src,
            audio_convert,
        })
    }

    pub fn uri(&self) -> GString {
        self.uri_decode.property("uri")
    }

    pub fn set_path(&self, path: &Path) -> Result<(), Error> {
        let uri = glib::filename_to_uri(path, None)?;
        self.set_uri(uri)
    }

    pub fn set_uri(&self, uri: GString) -> Result<(), Error> {
        self.uri_decode.set_property("uri", uri);

        // Unlink imagefreeze -> videoconvert
        let image_freeze_sink_pad = self.image_freeze.static_pad("sink").unwrap();
        if image_freeze_sink_pad.is_linked() {
            let peer = image_freeze_sink_pad.peer().unwrap();
            peer.unlink(&image_freeze_sink_pad)?;
        }

        // Unlink videoconvert
        let video_convert_sink_pad = self.video_convert.static_pad("sink").unwrap();
        if video_convert_sink_pad.is_linked() {
            let peer = video_convert_sink_pad.peer().unwrap();
            peer.unlink(&video_convert_sink_pad)?;
        }

        // Unlink audioconvert
        let audio_convert_sink_pad = self.audio_convert.static_pad("sink").unwrap();
        if audio_convert_sink_pad.is_linked() {
            let peer = audio_convert_sink_pad.peer().unwrap();
            peer.unlink(&audio_convert_sink_pad)?;
        }

        // Link videotestsrc -> videoconvert
        self.video_test_src.link(&self.video_convert)?;

        // Link audiotestsrc -> audioconvert
        self.audio_test_src.link(&self.audio_convert)?;

        self.video_test_src.set_locked_state(false);
        self.audio_test_src.set_locked_state(false);

        Ok(())
    }
}

fn create_counter_overlay<S>(
    name: S,
    duration: Arc<Mutex<Option<gstreamer::ClockTime>>>,
) -> Result<gstreamer::Element, Error>
where
    S: Into<GString>,
{
    let counter_overlay = gstreamer::ElementFactory::make("textoverlay")
        .name(name)
        .property_from_str("halignment", "right")
        .property_from_str("valignment", "top")
        .property_from_str("font-desc", "Sans, 10")
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

                let mut duration_str = String::new();

                if let Some(duration) = *duration.lock() {
                    let minutes = duration.minutes();
                    let seconds = duration.seconds() % 60;
                    duration_str = format!(" / {minutes:02}:{seconds:02}");
                }

                let text = format!("{current}{duration_str}");
                counter_overlay.set_property("text", &text);
            }

            *last_updated_second = Some(current_second);
        }
        gstreamer::PadProbeReturn::Ok
    });

    Ok(counter_overlay)
}
