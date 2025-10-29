use std::path::PathBuf;

use crate::media_info::MediaInfo;
use crate::random_files::RandomFiles;

#[derive(Debug, Clone)]
pub struct Source {
    pub path: PathBuf,
    pub media_info: MediaInfo,
}

pub fn start_finder_thread<I>(root_dirs: I, file_tx: flume::Sender<PathBuf>)
where
    I: IntoIterator<Item: Into<PathBuf>>,
{
    let root_dirs: Vec<_> = root_dirs.into_iter().map(Into::into).collect();

    std::thread::spawn(move || {
        println!("[Finder] Finder task started.");
        let random_files = RandomFiles::new(root_dirs);
        for source in random_files {
            if let Err(error) = file_tx.send(source) {
                eprintln!("[Finder] Channel closed: {error}");
                break;
            }
        }
    });
}
