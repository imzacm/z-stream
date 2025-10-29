use std::path::Path;
use std::sync::Arc;

use gstreamer::prelude::*;
use gstreamer_pbutils::prelude::DiscovererStreamInfoExt;
use gstreamer_pbutils::{
    Discoverer, DiscovererContainerInfo, DiscovererResult, DiscovererStreamInfo,
};
use parking_lot::Mutex;

use crate::media_type::MediaType;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Glib(#[from] glib::Error),
    #[error(transparent)]
    GlibBool(#[from] glib::BoolError),
}

#[derive(Default, Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum ImageCodec {
    Jpeg,
    Png,
    #[default]
    Unknown,
}

impl ImageCodec {
    pub const ALL: [Self; 3] = [Self::Jpeg, Self::Png, Self::Unknown];
}

#[derive(Default, Debug, Copy, Clone, PartialEq, PartialOrd)]
pub struct ImageInfo {
    pub codec: ImageCodec,
    pub horizontal_ppi: Option<f64>,
    pub vertical_ppi: Option<f64>,
}

#[derive(Default, Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct StreamInfo {
    pub max_bitrate: Option<u32>,
    pub bitrate: Option<u32>,
}

#[derive(Default, Debug, Copy, Clone, PartialEq, PartialOrd)]
pub struct MediaInfo {
    pub duration: Option<gstreamer::ClockTime>,
    pub image: Option<ImageInfo>,
    pub video: Option<StreamInfo>,
    pub audio: Option<StreamInfo>,
}

impl MediaInfo {
    pub fn detect(path: &Path) -> Result<Self, Error> {
        detect_media(path)
    }

    pub fn is_empty(&self) -> bool {
        self.image.is_none() && self.video.is_none() && self.audio.is_none()
    }

    pub fn play_duration(&self) -> gstreamer::ClockTime {
        self.duration.unwrap_or_else(|| 5 * gstreamer::ClockTime::SECOND)
    }

    pub fn media_type(&self) -> MediaType {
        if self.video.is_some() {
            if self.audio.is_some() {
                MediaType::VideoWithAudio
            } else {
                MediaType::VideoWithoutAudio
            }
        } else if self.image.is_none() {
            MediaType::Image
        } else {
            MediaType::Unknown
        }
    }
}

fn add_stream_info(info: &DiscovererStreamInfo, media_info: &Mutex<MediaInfo>) {
    let stream_nick = info.stream_type_nick();

    if stream_nick == "container" {
        return;
    }

    let caps_str = if let Some(caps) = info.caps() {
        if caps.is_fixed() {
            gstreamer_pbutils::pb_utils_get_codec_description(&caps)
        } else {
            glib::GString::from(caps.to_string())
        }
    } else {
        glib::GString::from("")
    };

    let mut media_info = media_info.lock();

    let is_image = stream_nick == "video(image)";
    let is_video = stream_nick == "video";
    let is_audio = stream_nick == "audio";

    if is_image {
        if media_info.image.is_some() {
            eprintln!("Image already set");
            return;
        }
        media_info.image = Some(ImageInfo::default());
    } else if is_video {
        if media_info.video.is_some() {
            eprintln!("Video already set");
            return;
        }
        media_info.video = Some(StreamInfo::default());
    } else if is_audio {
        if media_info.audio.is_some() {
            eprintln!("Audio already set");
            return;
        }
        media_info.audio = Some(StreamInfo::default());
    } else {
        eprintln!("Unhandled stream type: stream_nick={stream_nick} caps={caps_str}");
        return;
    }

    let Some(tags) = info.tags() else { return };

    if is_image {
        let image = media_info.image.as_mut().unwrap();

        // Detect codec from caps structure name (e.g., "image/jpeg", "image/png").
        if let Some(caps) = info.caps()
            && let Some(structure) = caps.structure(0)
        {
            let name = structure.name();
            image.codec = if name.eq_ignore_ascii_case("image/jpeg") {
                ImageCodec::Jpeg
            } else if name.eq_ignore_ascii_case("image/png") {
                ImageCodec::Png
            } else {
                ImageCodec::Unknown
            };
        }

        for (tag, mut values) in tags.iter_generic() {
            if tag == "image-horizontal-ppi"
                && let Some(value) = values.next()
            {
                match value.get::<f64>() {
                    Ok(value) => image.horizontal_ppi = Some(value),
                    Err(error) => eprintln!("Failed to get image-horizontal-ppi: {error}"),
                }
            }

            if tag == "image-vertical-ppi"
                && let Some(value) = values.next()
            {
                match value.get::<f64>() {
                    Ok(value) => image.vertical_ppi = Some(value),
                    Err(error) => eprintln!("Failed to get image-vertical-ppi: {error}"),
                }
            }
        }
    } else if is_video {
        let video = media_info.video.as_mut().unwrap();

        if let Some(value) = tags.get::<gstreamer::tags::MaximumBitrate>() {
            video.max_bitrate = Some(value.get());
        }
        if let Some(value) = tags.get::<gstreamer::tags::Bitrate>() {
            video.bitrate = Some(value.get());
        }
    } else if is_audio {
        let audio = media_info.audio.as_mut().unwrap();

        if let Some(value) = tags.get::<gstreamer::tags::MaximumBitrate>() {
            audio.max_bitrate = Some(value.get());
        }
        if let Some(value) = tags.get::<gstreamer::tags::Bitrate>() {
            audio.bitrate = Some(value.get());
        }
    }
}

fn add_topology(info: &DiscovererStreamInfo, media_info: &Mutex<MediaInfo>) {
    add_stream_info(info, media_info);

    if let Some(next) = info.next() {
        add_topology(&next, media_info);
    } else if let Some(container_info) = info.downcast_ref::<DiscovererContainerInfo>() {
        for stream in container_info.streams() {
            add_topology(&stream, media_info);
        }
    }
}

fn detect_media(path: &Path) -> Result<MediaInfo, Error> {
    let loop_ = glib::MainLoop::new(None, false);
    let timeout = 5 * gstreamer::ClockTime::SECOND;

    let uri = glib::filename_to_uri(path, None)?;
    let discoverer = Discoverer::new(timeout)?;

    let media_info = Arc::new(Mutex::new(MediaInfo::default()));

    let media_info_clone = media_info.clone();
    discoverer.connect_discovered(move |_discoverer, info, error| {
        let uri = info.uri();
        match info.result() {
            DiscovererResult::Ok => {
                // println!("Discovered {uri}");
            }
            DiscovererResult::UriInvalid => eprintln!("Invalid uri {uri}"),
            DiscovererResult::Error => {
                if let Some(msg) = error {
                    eprintln!("{msg}");
                } else {
                    eprintln!("Unknown error")
                }
            }
            DiscovererResult::Timeout => eprintln!("Timeout"),
            DiscovererResult::Busy => eprintln!("Busy"),
            DiscovererResult::MissingPlugins => {
                if let Some(s) = info.misc() {
                    eprintln!("{s}");
                }
            }
            _ => eprintln!("Unknown result"),
        }

        if info.result() != DiscovererResult::Ok {
            return;
        }

        media_info_clone.lock().duration = info.duration();
        if let Some(stream_info) = info.stream_info() {
            add_topology(&stream_info, &media_info_clone);
        }
    });

    let loop_clone = loop_.clone();
    discoverer.connect_finished(move |_| loop_clone.quit());
    discoverer.start();
    discoverer.discover_uri_async(&uri)?;

    loop_.run();
    discoverer.stop();

    let media_info = *media_info.lock();
    Ok(media_info)
}
