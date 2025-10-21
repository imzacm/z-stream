use std::path::PathBuf;
use std::process::{Command, Stdio};

use crate::finder::Source;

pub fn start_streamer_task(
    file_rx: flume::Receiver<Source>,
    ffmpeg_path: PathBuf,
    rtmp_url: String,
) {
    std::thread::spawn(move || {
        println!("[Streamer] Streamer task started.");

        loop {
            let source = match file_rx.recv() {
                Ok(source) => source,
                Err(flume::RecvError::Disconnected) => {
                    println!("[Streamer] File channel closed. Shutting down.");
                    break;
                }
            };

            eprintln!("Source: {source:?}");

            let file_path = &source.path;
            let fmt_file_path = file_path.display();
            println!("[Streamer] Starting new file: {fmt_file_path}");

            let child_result = Command::new(&ffmpeg_path)
                .arg("-re")
                .arg("-i")
                .arg(file_path)
                .args(["-c:v", "libx264"])
                .args(["-preset", "veryfast"])
                .args(["-tune", "zerolatency"])
                .args(["-c:a", "aac"])
                .args(["-ac", "2"])
                .args(["-f", "flv"])
                .arg(&rtmp_url)
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .spawn();
            let mut child = match child_result {
                Ok(child) => child,
                Err(error) => {
                    eprintln!("[Streamer] Failed to spawn ffmpeg for {fmt_file_path}: {error}");
                    continue;
                }
            };

            let exit_status = match child.wait() {
                Ok(exit_status) => exit_status,
                Err(error) => {
                    eprintln!("[Streamer] Failed to wait for ffmpeg for {fmt_file_path}: {error}");
                    continue;
                }
            };

            if exit_status.success() {
                println!("[Streamer] Finished file: {fmt_file_path}");
                continue;
            }

            eprintln!("[Streamer] FFmpeg failed for {fmt_file_path} with {exit_status}");
        }
    });
}
