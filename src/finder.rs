use std::path::PathBuf;

use crate::random_files::RandomFiles;

#[derive(Debug, Clone)]
pub struct Source {
    pub path: PathBuf,
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
            let source = Source { path };
            if let Err(error) = tx.send(source) {
                eprintln!("[Finder] Channel closed. Shutting down. {error}");
                return;
            }
        }
    });
}
