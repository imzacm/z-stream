use std::collections::HashMap;
use std::path::PathBuf;

use camino::Utf8Path;

use crate::media_info::MediaInfo;
use crate::random_files::RandomFiles;

#[derive(Debug, Clone)]
pub struct Source {
    pub path: PathBuf,
    pub media_info: MediaInfo,
}

pub fn start_finder_thread<I>(root_dirs: I, tx: flume::Sender<Source>)
where
    I: IntoIterator<Item: Into<PathBuf>>,
{
    let mut random_files = RandomFiles::default().with_roots(root_dirs).cycle_files(true);

    std::thread::spawn(move || {
        println!("[Finder] Finder task started.");
        let mut info_map: HashMap<PathBuf, MediaInfo> = HashMap::new();
        loop {
            let path = random_files.next_if(|path| {
                // We've already used this file before.
                if let Some(media_info) = info_map.get(path) {
                    return !media_info.is_empty();
                }

                let Some(path) = Utf8Path::from_path(path) else { return false };

                match MediaInfo::detect(path) {
                    Ok(media_info) => {
                        // eprintln!("[Finder] Media info for file {path}: {media_info:?}");
                        info_map.insert(path.as_std_path().to_path_buf(), media_info);
                        !media_info.is_empty()
                    }
                    Err(_error) => {
                        // eprintln!("Failed to detect media info for file {path}: {error}");
                        false
                    }
                }
            });

            let Some(path) = path else { continue };

            let media_info = info_map.get(&path).unwrap();
            let source = Source { path, media_info: *media_info };
            if let Err(error) = tx.send(source) {
                eprintln!("[Finder] Channel closed. Shutting down. {error}");
                return;
            }
        }
    });
}
