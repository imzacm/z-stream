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

    if encoder.has_property("tune") {
        let tune_value = if factory == "nvh264enc" { "ultra-low-latency" } else { "zerolatency" };
        encoder.set_property_from_str("tune", tune_value);
    }

    if encoder.has_property("key-int-max") {
        encoder.set_property("key-int-max", 30u32);
    }

    // if encoder.has_property("bitrate") {
    //     encoder.set_property("bitrate", options.bitrate);
    // }

    Ok(encoder)
}
