#![deny(unused_imports, unsafe_code, clippy::all)]

mod api;
mod media_info;
mod media_type;
mod mediamtx;
mod random_files;
mod stream;

use std::path::PathBuf;

use gstreamer_rtsp_server::prelude::RTSPServerExtManual;

const STREAM_KEY: &str = "my_stream";
const RTSP_PORT: u16 = 18554;
const API_PORT: u16 = 18080;

fn main() {
    gstreamer::init().expect("Failed to initialize GStreamer");

    let mut args = std::env::args_os().skip(1).peekable();
    if args.peek().is_some_and(|v| v == "--test") {
        args.next();
        std::process::Command::new("pkill")
            .arg("mediamtx")
            .spawn()
            .unwrap()
            .wait()
            .unwrap();

        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(100));
            std::process::Command::new("ffplay")
                .args(["-v", "info", "rtsp://127.0.0.1:8554/my_stream"])
                .spawn()
                .unwrap()
                .wait()
                .unwrap();

            std::thread::sleep(std::time::Duration::from_secs(5));
            std::process::Command::new("pkill")
                .arg("mediamtx")
                .spawn()
                .unwrap()
                .wait()
                .unwrap();
            std::process::exit(0);
        });
    }

    let root_dirs = std::env::args_os().skip(1).map(PathBuf::from).collect::<Vec<_>>();

    let (command_tx, command_rx) = flume::bounded(20);
    let (event_tx, _event_rx) = flume::bounded(20);
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

    let server = stream::create_server(root_dirs, command_rx, event_tx, RTSP_PORT, STREAM_KEY)
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
