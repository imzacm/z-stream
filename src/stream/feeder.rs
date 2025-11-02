use std::path::PathBuf;

use super::{AppSources, AppSrcStorage, Command, Event};
use crate::random_files::RandomFiles;
use crate::stream::input_pipeline::InputPipeline;

/// Blocks until the AppSrc is available in the shared storage.
fn get_app_sources(storage: AppSrcStorage) -> AppSources {
    loop {
        let appsrc_opt = storage.lock().clone();
        if let Some(appsrc) = appsrc_opt {
            println!("Feeder thread connected to appsrc.");
            return appsrc;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

/// Task for the thread that feeds the RTSP stream.
/// It waits for file paths from the channel and runs a pipeline for each.
pub fn file_feeder_task(
    root_dirs: Vec<PathBuf>,
    command_rx: flume::Receiver<Command>,
    event_tx: flume::Sender<Event>,
    storage: AppSrcStorage,
) {
    // First, wait for the RTSP client to connect and create the appsrc
    let app_sources = get_app_sources(storage);

    let mut pipeline = InputPipeline::new(RandomFiles::new(root_dirs), 2, app_sources)
        .expect("Failed to create pipeline");

    loop {
        if let Err(error) = pipeline.play(&command_rx) {
            eprintln!("Failed to play pipeline: {error}");
        }
    }

    // while let Ok((path, pipeline)) = pipeline_rx.recv() {
    //     println!("Playing file: {}", path.display());
    //     _ = event_tx.try_send(Event::Playing { path: path.clone() });
    //
    //     if let Err(error) = pipeline.play(&abort_rx) {
    //         eprintln!("Failed to play file: {error}");
    //     }
    //
    //     for appsrc in [&app_sources.video, &app_sources.audio] {
    //         appsrc.send_event(gstreamer::event::FlushStart::new());
    //         appsrc.send_event(gstreamer::event::FlushStop::new(true));
    //     }
    //
    //     // self.pipeline.send_event(gstreamer::event::FlushStart::new());
    //     // self.pipeline.send_event(gstreamer::event::FlushStop::new(true));
    //     //
    //     // self.video_app_sink.send_event(gstreamer::event::FlushStart::new());
    //     // self.audio_app_sink.send_event(gstreamer::event::FlushStart::new());
    //     //
    //     // self.video_app_sink.send_event(gstreamer::event::FlushStop::new(true));
    //     // self.audio_app_sink.send_event(gstreamer::event::FlushStop::new(true));
    //
    //     // Teardown pipeline in another thread.
    //     rayon::spawn(move || {
    //         pipeline
    //             .pipeline
    //             .set_state(gstreamer::State::Null)
    //             .expect("Failed to set pipeline to null");
    //     });
    //
    //     _ = event_tx.try_send(Event::Ended { path: path.clone() });
    // }
    // println!("Feeder thread shutting down.");
}
