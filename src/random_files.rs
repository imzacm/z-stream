use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;
use rand::Rng;
use rand::seq::SliceRandom;

#[derive(Default, Debug)]
pub struct RandomFiles {
    root_dirs: Vec<PathBuf>,
    files: Arc<Mutex<Vec<PathBuf>>>,
    file_notify_rx: Option<flume::Receiver<()>>,
    cycle: bool,
}

impl RandomFiles {
    pub fn with_root<P>(mut self, root: P) -> Self
    where
        P: Into<PathBuf>,
    {
        self.root_dirs.push(root.into());
        self
    }

    pub fn with_roots<I>(mut self, roots: I) -> Self
    where
        I: IntoIterator<Item: Into<PathBuf>>,
    {
        self.root_dirs.extend(roots.into_iter().map(|r| r.into()));
        self
    }

    pub fn cycle_files(mut self, cycle: bool) -> Self {
        self.cycle = cycle;
        self
    }

    fn ensure_files(&mut self) {
        if !self.cycle || !self.files.lock().is_empty() {
            return;
        }
        if self.file_notify_rx.as_ref().is_none_or(|rx| rx.is_disconnected()) {
            let rx = walk(self.root_dirs.clone(), self.files.clone());
            self.file_notify_rx = Some(rx);
        }
        if let Some(rx) = &self.file_notify_rx {
            _ = rx.recv();
        }
    }

    pub fn multi_if<F>(&mut self, mut if_fn: F) -> Vec<PathBuf>
    where
        F: FnMut(&Path) -> bool,
    {
        self.ensure_files();

        let mut files = self.files.lock();

        let mut filtered = files
            .iter()
            .enumerate()
            .filter_map(|(index, path)| if if_fn(path) { Some(index) } else { None })
            .collect::<Vec<_>>();

        if filtered.is_empty() {
            return Vec::new();
        }

        let mut rng = rand::rng();
        filtered.shuffle(&mut rng);

        let n: usize = rng.random_range(1..filtered.len());
        let mut items = Vec::with_capacity(n);
        for index in filtered.into_iter().take(n) {
            let path = files.remove(index);
            items.push(path);
        }

        items
    }

    pub fn next_if<F>(&mut self, mut if_fn: F) -> Option<PathBuf>
    where
        F: FnMut(&Path) -> bool,
    {
        self.ensure_files();

        let mut files = self.files.lock();

        let filtered = files
            .iter()
            .enumerate()
            .filter_map(|(index, path)| if if_fn(path) { Some(index) } else { None })
            .collect::<Vec<_>>();

        if filtered.is_empty() {
            return None;
        }

        let mut rng = rand::rng();
        let n: usize = rng.random_range(..filtered.len());
        let index = filtered[n];
        Some(files.remove(index))
    }
}

impl Iterator for RandomFiles {
    type Item = PathBuf;

    fn next(&mut self) -> Option<Self::Item> {
        self.ensure_files();
        let mut files = self.files.lock();

        let mut rng = rand::rng();
        let index: usize = rng.random_range(..files.len());
        Some(files.remove(index))
    }
}

fn walk(root_dirs: Vec<PathBuf>, files: Arc<Mutex<Vec<PathBuf>>>) -> flume::Receiver<()> {
    let (tx, rx) = flume::bounded(1);
    std::thread::spawn(move || {
        for dir in root_dirs {
            let files = files.clone();
            let tx = tx.clone();
            std::thread::spawn(move || {
                walk_one(&dir, &files);
                _ = tx.send(());
            });
        }
    });
    rx
}

fn walk_one(root_dir: &Path, files: &Mutex<Vec<PathBuf>>) {
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

        files.lock().push(entry.into_path());
    }
}
