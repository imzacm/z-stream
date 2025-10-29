use std::path::PathBuf;
use std::time::Duration;

use rand::Rng;
use rand::seq::SliceRandom;

use crate::media_type::{MediaType, get_media_type};

const MIN_SIZE: usize = 50;

#[derive(Debug, Clone)]
pub struct RandomFiles {
    root_dirs: Vec<PathBuf>,
    files: Vec<PathBuf>,
    file_rx: flume::Receiver<PathBuf>,
    rng: rand::rngs::ThreadRng,
}

impl RandomFiles {
    pub fn new<I>(root_dirs: I) -> Self
    where
        I: IntoIterator<Item: Into<PathBuf>>,
    {
        let mut rng = rand::rng();

        let mut root_dirs: Vec<_> = root_dirs.into_iter().map(Into::into).collect();
        root_dirs.shuffle(&mut rng);

        let (file_tx, file_rx) = flume::bounded(500);
        let root_dirs_clone = root_dirs.clone();
        std::thread::spawn(move || walk_roots(root_dirs_clone, file_tx));

        let files = Vec::with_capacity(MIN_SIZE);
        Self { root_dirs, files, file_rx, rng }
    }
}

impl Iterator for RandomFiles {
    type Item = PathBuf;

    fn next(&mut self) -> Option<Self::Item> {
        self.files.reserve(self.file_rx.len());
        while let Ok(path) = self.file_rx.try_recv() {
            if !self.files.contains(&path) {
                self.files.push(path);
            }
        }

        if self.file_rx.is_disconnected() {
            let (file_tx, file_rx) = flume::bounded(100);
            let root_dirs = self.root_dirs.clone();
            std::thread::spawn(move || walk_roots(root_dirs, file_tx));
            self.file_rx = file_rx;
        }

        while self.files.len() < MIN_SIZE {
            if let Ok(path) = self.file_rx.recv_timeout(Duration::from_millis(100)) {
                self.files.push(path);
            }
        }

        if self.files.is_empty() {
            return self.file_rx.recv().ok();
        }

        let index = self.rng.random_range(0..self.files.len());
        Some(self.files.swap_remove(index))
    }
}

fn walk_roots(root_dirs: Vec<PathBuf>, file_tx: flume::Sender<PathBuf>) {
    println!("[RandomFiles] Walking files");

    std::thread::scope(move |s| {
        for root_dir in root_dirs {
            let file_tx = file_tx.clone();
            s.spawn(move || {
                let file_type = match std::fs::metadata(&root_dir) {
                    Ok(metadata) => metadata.file_type(),
                    Err(error) => {
                        eprintln!(
                            "Error getting file metadata for {}: {error}",
                            root_dir.display()
                        );
                        return;
                    }
                };

                if file_type.is_file() {
                    push_file(root_dir, &file_tx);
                } else {
                    walk_one(root_dir, &file_tx);
                }
            });
        }
    });

    println!("[RandomFiles] Finished walking files");
}

fn walk_one(root_dir: PathBuf, file_tx: &flume::Sender<PathBuf>) {
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

        push_file(path, file_tx);
    }
}

fn push_file(path: PathBuf, file_tx: &flume::Sender<PathBuf>) {
    if let Ok(MediaType::Image | MediaType::VideoWithAudio) = get_media_type(&path) {
        let _ = file_tx.send(path);
    }
}
