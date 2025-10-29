#![deny(unused_imports, unsafe_code, clippy::all)]

mod api;
mod finder;
mod media_info;
mod media_type;
mod mediamtx;
mod random_files;
mod stream;

use gstreamer_rtsp_server::prelude::RTSPServerExtManual;

use crate::finder::start_finder_thread;

const STREAM_KEY: &str = "my_stream";
const RTSP_PORT: u16 = 18554;
const API_PORT: u16 = 18080;

fn main() {
    gstreamer::init().expect("Failed to initialize GStreamer");

    let root_dirs = std::env::args_os().skip(1);

    // Channel for file paths (Finder -> Streamer)
    let (file_tx, file_rx) = flume::bounded(20);

    // Start the background finder thread
    start_finder_thread(root_dirs, file_tx);

    let (command_tx, command_rx) = flume::bounded(20);
    let (event_tx, event_rx) = flume::bounded(20);
    api::start_api_task(API_PORT, command_tx);

    let rtmp_port: u16 = 1935;
    let hls_port: u16 = 8888;
    let rtsp_port: u16 = 8554;
    let srt_port: u16 = 8890;
    let webrtc_port: u16 = 8889;

    std::thread::spawn(move || {
        let mut mediamtx = mediamtx::start().expect("Failed to start mediamtx");

        let exit_status = mediamtx.wait().expect("Failed to wait for mediamtx to exit");
        println!("Exit status: {}", exit_status);
        if !exit_status.success() {
            std::process::exit(1);
        }
    });

    let main_loop = glib::MainLoop::new(None, false);

    // ommand_rx: flume::Receiver<Command>, event_tx

    let server = stream::create_server(file_rx, command_rx, event_tx, RTSP_PORT, STREAM_KEY)
        .expect("Failed to start RTSP server");

    let context = main_loop.context();
    server
        .attach(Some(&context))
        .expect("Failed to attach RTSP server to main loop");

    println!("Clients can connect to:");
    println!("  RTMP: rtmp://127.0.0.1:{rtmp_port}/{STREAM_KEY}");
    println!("  RTSP: rtsp://127.0.0.1:{rtsp_port}/{STREAM_KEY}");
    println!("  SRT: srt://127.0.0.1:{srt_port}?streamid=read:{STREAM_KEY}");
    println!("  WebRTC: http://127.0.0.1:{webrtc_port}/{STREAM_KEY}");
    println!("  HLS:  http://127.0.0.1:{hls_port}/{STREAM_KEY}/index.m3u8");
    println!("\nPress Ctrl+C to shut down.");

    main_loop.run();
}
