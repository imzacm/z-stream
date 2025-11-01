use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use gstreamer::prelude::ElementExtManual;

use crate::stream::{AppSources, AppSrcStorage, Command, Event};

/// Blocks until the AppSrc is available in the shared storage.
fn get_app_sources(storage: AppSrcStorage) -> AppSources {
    loop {
        let appsrc_opt = storage.lock().clone();
        if let Some(appsrc) = appsrc_opt {
            println!("Stream queue thread connected to appsrc.");
            return appsrc;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

pub struct StreamQueueItem {
    pub path: PathBuf,
    pub video: flume::Receiver<gstreamer::Sample>,
    pub audio: flume::Receiver<gstreamer::Sample>,
}

pub fn stream_queue_task(
    queue_rx: flume::Receiver<StreamQueueItem>,
    storage: AppSrcStorage,
    command_rx: flume::Receiver<Command>,
    event_tx: flume::Sender<Event>,
) {
    let abort = Arc::new(AtomicBool::new(false));

    let abort_clone = abort.clone();
    std::thread::spawn(move || {
        while let Ok(command) = command_rx.recv() {
            match command {
                Command::Skip => {
                    println!("Skipping file");
                    abort_clone.store(true, std::sync::atomic::Ordering::Relaxed);
                }
            }
        }
    });

    let app_sources = get_app_sources(storage);

    let push_video = Arc::new(AtomicBool::new(false));
    let push_audio = Arc::new(AtomicBool::new(false));

    let push_video_clone1 = push_video.clone();
    let push_video_clone2 = push_video.clone();
    let video_callbacks = gstreamer_app::AppSrcCallbacks::builder()
        .need_data(move |_appsrc, _length| {
            push_video_clone1.store(true, std::sync::atomic::Ordering::Relaxed);
        })
        .enough_data(move |_appsrc| {
            push_video_clone2.store(false, std::sync::atomic::Ordering::Relaxed);
        })
        .build();

    let push_audio_clone1 = push_audio.clone();
    let push_audio_clone2 = push_audio.clone();
    let audio_callbacks = gstreamer_app::AppSrcCallbacks::builder()
        .need_data(move |_appsrc, _length| {
            push_audio_clone1.store(true, std::sync::atomic::Ordering::Relaxed);
        })
        .enough_data(move |_appsrc| {
            push_audio_clone2.store(false, std::sync::atomic::Ordering::Relaxed);
        })
        .build();

    app_sources.video.set_callbacks(video_callbacks);
    app_sources.audio.set_callbacks(audio_callbacks);

    while let Ok(stream_item) = queue_rx.recv() {
        println!("Playing file: {}", stream_item.path.display());
        _ = event_tx.try_send(Event::Playing { path: stream_item.path.clone() });

        std::thread::scope(|s| {
            s.spawn(|| {
                // Video
                push_buffers(&push_video, &stream_item.video, &app_sources.video, &abort);
            });
            s.spawn(|| {
                // Audio
                push_buffers(&push_audio, &stream_item.audio, &app_sources.audio, &abort);
            });

            for appsrc in [&app_sources.video, &app_sources.audio] {
                appsrc.send_event(gstreamer::event::FlushStart::new());
                appsrc.send_event(gstreamer::event::FlushStop::new(true));
            }

            _ = event_tx.try_send(Event::Ended { path: stream_item.path });
            abort.store(false, std::sync::atomic::Ordering::Relaxed);
        });
    }

    eprintln!("Stream queue thread disconnected from queue channel.");
}

fn push_buffers(
    push_data: &AtomicBool,
    buffer_rx: &flume::Receiver<gstreamer::Sample>,
    app_src: &gstreamer_app::AppSrc,
    abort: &AtomicBool,
) {
    loop {
        if abort.load(std::sync::atomic::Ordering::Relaxed) {
            break;
        }
        if !push_data.load(std::sync::atomic::Ordering::Relaxed) {
            continue;
        }
        if let Ok(sample) = buffer_rx.recv_timeout(std::time::Duration::from_millis(100)) {
            app_src.push_sample(&sample).expect("Failed to push sample");
        }
    }
}
