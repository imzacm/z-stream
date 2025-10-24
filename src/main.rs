#![deny(unused_imports, unsafe_code, clippy::all)]

mod finder;
mod media_info;
mod mediamtx;
mod random_files;
mod streamer;

use crate::finder::start_finder_thread;
use crate::streamer::start_streamer_task;

fn main() {
    gstreamer::init().expect("Failed to initialize GStreamer");

    let root_dirs = std::env::args_os().skip(1);

    // Channel for file paths (Finder -> Streamer)
    let (file_tx, file_rx) = flume::bounded(20);

    // Start the background finder thread
    start_finder_thread(root_dirs, file_tx);

    let rtmp_port: u16 = 1935;
    let hls_port: u16 = 8888;
    let rtsp_port: u16 = 8554;
    let srt_port: u16 = 8890;
    let webrtc_port: u16 = 8889;

    let mut mediamtx = mediamtx::start().expect("Failed to start mediamtx");

    let stream_key = "my_stream";

    // let output = streamer::Output::Rtmp(format!("rtmp://127.0.0.1:{rtmp_port}/{stream_key}"));
    let output = streamer::Output::Srt(format!(
        "srt://127.0.0.1:{srt_port}?streamid=publish:{stream_key}&pkt_size=1316"
    ));

    let video_options = streamer::VideoOptions::HD_720;
    let video_options = streamer::VideoOptions::PIXEL_9_PRO_FOLD_LIGHT;

    // Start the background streamer task
    start_streamer_task(file_rx, output, video_options);

    println!("Clients can connect to:");
    println!("  RTMP: rtmp://127.0.0.1:{rtmp_port}/{stream_key}");
    println!("  RTSP: rtsp://127.0.0.1:{rtsp_port}/{stream_key}");
    println!("  SRT: srt://127.0.0.1:{srt_port}?streamid=read:{stream_key}");
    println!("  WebRTC: http://127.0.0.1:{webrtc_port}/{stream_key}");
    println!("  HLS:  http://127.0.0.1:{hls_port}/{stream_key}/index.m3u8");
    println!("\nPress Ctrl+C to shut down.");

    let exit_status = mediamtx.wait().expect("Failed to wait for mediamtx to exit");
    println!("Exit status: {}", exit_status);
    if !exit_status.success() {
        std::process::exit(1);
    }
}
