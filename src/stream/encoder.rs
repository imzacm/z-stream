use glib::object::ObjectExt;
use gstreamer::gobject::GObjectExtManualGst;

use super::Error;

pub fn create_video_encoder() -> Result<gstreamer::Element, Error> {
    if let Ok(encoder) = create_video_encoder_inner("nvh264enc") {
        eprintln!("Using nvh264enc");
        return Ok(encoder);
    }

    if let Ok(encoder) = create_video_encoder_inner("vah264enc") {
        eprintln!("Using vah264enc");
        return Ok(encoder);
    }

    create_video_encoder_inner("x264enc")
}

fn create_video_encoder_inner(factory: &str) -> Result<gstreamer::Element, Error> {
    let encoder = gstreamer::ElementFactory::make(factory).name("v_encode").build()?;

    match factory {
        "nvh264enc" => {
            // Use preset for a better quality/latency balance than "tune"
            encoder.set_property_from_str("preset", "low-latency-hq");
            // Use Constant Bitrate (CBR) for streaming
            encoder.set_property_from_str("rc-mode", "cbr");
            encoder.set_property("zerolatency", true);
        }
        "vah264enc" => {
            encoder.set_property_from_str("rate-control", "cbr");
        }
        "x264enc" => {
            encoder.set_property("profile", "high");
        }
        _ => (),
    }

    if encoder.has_property("tune") && factory != "nvh264enc" {
        encoder.set_property_from_str("tune", "zerolatency");
    }

    if encoder.has_property("aud") {
        encoder.set_property("aud", true);
    }

    if encoder.has_property("bitrate") {
        // Set a target bitrate (e.g., 4 Mbps for 720p)
        encoder.set_property("bitrate", 6000u32);
    }

    if encoder.has_property("key-int-max") {
        encoder.set_property("key-int-max", 60u32);
    }

    if encoder.has_property("bframes") {
        encoder.set_property("bframes", 2u32);
    }

    if encoder.has_property("cabac") {
        encoder.set_property("cabac", true);
    }

    Ok(encoder)
}
