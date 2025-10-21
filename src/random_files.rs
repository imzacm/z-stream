use parking_lot::Mutex;
use rand::Rng;
use rand::seq::SliceRandom;
use std::path::{Path, PathBuf};

#[derive(Default, Debug)]
pub struct RandomFiles {
    root_dirs: Vec<PathBuf>,
    files: Mutex<Vec<PathBuf>>,
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

    fn ensure_files(&self) {
        if self.cycle && self.files.lock().is_empty() {
            walk(&self.root_dirs, &self.files);
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

fn walk(root_dirs: &[PathBuf], files: &Mutex<Vec<PathBuf>>) {
    std::thread::scope(|s| {
        for dir in root_dirs {
            s.spawn(|| walk_one(dir, files));
        }
    });
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
