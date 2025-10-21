#![deny(unused_imports, unsafe_code, clippy::all)]

mod finder;
mod random_files;
mod streamer;
mod xiu;

use crate::finder::start_finder_thread;
use crate::streamer::start_streamer_task;
use crate::xiu::{XiuConfig, XiuServer};

fn main() {
    ffmpeg_next::init().expect("Failed to initialize ffmpeg");
    ffmpeg_next::log::set_level(ffmpeg_next::log::Level::Quiet);

    let root_dirs = std::env::args_os().skip(1);

    // Channel for file paths (Finder -> Streamer)
    let (file_tx, file_rx) = flume::bounded(20);

    // Start the background finder thread
    start_finder_thread(root_dirs, file_tx);

    let config = XiuConfig::default();
    let mut server = XiuServer::start(config).expect("Failed to start xiu server");

    let stream_path = "live/my_stream";
    _ = std::fs::remove_dir_all(stream_path);

    // Start the background streamer task
    start_streamer_task(
        file_rx,
        "ffmpeg".into(),
        format!("rtmp://127.0.0.1:{}/{stream_path}", server.config().rtmp_port),
    );

    println!("Clients can connect to:");
    println!("  RTMP: rtmp://127.0.0.1:{}/{stream_path}", server.config().rtmp_port);
    println!("  RTSP: rtsp://127.0.0.1:{}/{stream_path}", server.config().rtsp_port);
    println!("  WebRTC: webrtc://127.0.0.1:{}/{stream_path}", server.config().webrtc_port);
    println!("  HLS:  http://127.0.0.1:{}/{stream_path}.m3u8", server.config().hls_port);
    println!("\nPress Ctrl+C to shut down.");

    server.wait().expect("xiu server failed");
}
