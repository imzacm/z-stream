use std::path::PathBuf;
use std::time::Duration;

use crate::random_files::RandomFiles;

#[derive(Debug, Clone)]
pub struct Source {
    pub path: PathBuf,
    pub duration: Duration,
}

pub fn start_finder_thread<I>(root_dirs: I, tx: flume::Sender<Source>)
where
    I: IntoIterator<Item: Into<PathBuf>>,
{
    let mut random_files = RandomFiles::default().with_roots(root_dirs).cycle_files(true);

    std::thread::spawn(move || {
        println!("[Finder] Finder task started.");
        loop {
            let path = random_files.next_if(|path| {
                let Ok(ctx) = ffmpeg_next::format::input(path) else { return false };
                ctx.streams().any(|s| {
                    matches!(
                        s.parameters().medium(),
                        ffmpeg_next::media::Type::Video | ffmpeg_next::media::Type::Audio
                    )
                })
            });

            let Some(path) = path else { continue };

            let mut duration = Duration::default();
            {
                let ctx = ffmpeg_next::format::input(&path).unwrap();
                for stream in ctx.streams() {
                    let time_base: f64 = stream.time_base().into();
                    let duration_secs = stream.duration() as f64 * time_base;
                    duration = duration.max(Duration::from_secs_f64(duration_secs));
                }
            }

            // let Some(path) = random_files.next() else { continue };
            let source = Source { path, duration };
            if let Err(error) = tx.send(source) {
                eprintln!("[Finder] Channel closed. Shutting down. {error}");
                return;
            }
        }
    });
}
