use std::path::{Path, PathBuf};
use std::sync::Arc;

use camino::Utf8Path;
use parking_lot::Mutex;

use crate::finder::Source;
use crate::media_info::MediaInfo;

#[derive(Debug, Clone)]
pub struct RandomFiles {
    root_dirs: Vec<PathBuf>,
    files: Vec<Source>,
}

impl RandomFiles {
    pub fn new<I>(root_dirs: I) -> Self
    where
        I: IntoIterator<Item: Into<PathBuf>>,
    {
        let root_dirs = root_dirs.into_iter().map(Into::into).collect();
        Self { root_dirs, files: Vec::new() }
    }
}

impl Iterator for RandomFiles {
    type Item = Source;

    fn next(&mut self) -> Option<Self::Item> {
        if self.files.is_empty() {
            walk(&self.root_dirs, &mut self.files);
            println!("[RandomFiles] Shuffling files");
            let mut rand = urandom::new();
            rand.shuffle(&mut self.files);
            println!("[RandomFiles] Finished shuffling files");
        }
        self.files.pop()
    }
}

fn walk(root_dirs: &[PathBuf], into_files: &mut Vec<Source>) {
    println!("[RandomFiles] Walking files");
    let files = Arc::new(Mutex::new(std::mem::take(into_files)));
    let files_clone = files.clone();
    std::thread::scope(move |s| {
        for root_dir in root_dirs {
            let files = files_clone.clone();
            s.spawn(move || {
                walk_one(root_dir, &files);
            });
        }
    });
    let files = Arc::try_unwrap(files).unwrap();
    let mut files = files.into_inner();
    files.shrink_to_fit();
    println!("[RandomFiles] Finished walking files, found {} files", files.len());
    *into_files = files;
}

fn walk_one(root_dir: &Path, files: &Mutex<Vec<Source>>) {
    for entry in walkdir::WalkDir::new(root_dir).follow_links(true) {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                eprintln!("Error walking dir: {error}");
                continue;
            }
        };

        if !entry.file_type().is_file() {
            continue;
        }

        let path = entry.into_path();

        let Some(utf8_path) = Utf8Path::from_path(&path) else { continue };

        let Ok(media_info) = MediaInfo::detect(utf8_path) else { continue };
        if media_info.is_empty() {
            continue;
        }
        let source = Source { path, media_info };
        files.lock().push(source);
    }
}
