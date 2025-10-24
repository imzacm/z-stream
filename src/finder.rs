use std::path::PathBuf;

use crate::media_info::MediaInfo;
use crate::random_files::RandomFiles;

#[derive(Debug, Clone)]
pub struct Source {
    pub path: PathBuf,
    pub media_info: MediaInfo,
}

pub fn start_finder_thread<I>(root_dirs: I, source_tx: flume::Sender<Source>)
where
    I: IntoIterator<Item: Into<PathBuf>>,
{
    let random_files = RandomFiles::new(root_dirs);

    std::thread::spawn(move || {
        println!("[Finder] Finder task started.");
        for source in random_files {
            if let Err(error) = source_tx.send(source) {
                eprintln!("[Finder] Channel closed: {error}");
                break;
            }
        }
    });
}
