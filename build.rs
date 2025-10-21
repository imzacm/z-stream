// https://github.com/bluenviron/mediamtx/releases/download/v1.15.3/mediamtx_v1.15.3_darwin_amd64.tar.gz

use std::io::{Seek, Write};
use std::path::{Path, PathBuf};

fn main() {
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    let target = Target::detect();
    download_mediamtx(target, &out_dir);
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
enum Target {
    DarwinAmd64,
    DarwinArm64,
    LinuxAmd64,
    LinuxArm64,
    LinuxArmv6,
    LinuxArmv7,
    WindowsAmd64,
}

impl Target {
    fn detect() -> Self {
        let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();
        let target_arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap();

        match (target_os.as_str(), target_arch.as_str()) {
            ("macos", "x86_64") => Target::DarwinAmd64,
            ("macos", "aarch64") => Target::DarwinArm64,
            ("linux", "x86_64") => Target::LinuxAmd64,
            ("linux", "aarch64") => Target::LinuxArm64,
            ("linux", "arm") if std::env::var("CARGO_CFG_TARGET_ENV").unwrap() == "gnueabihf" => {
                // Distinguish between armv6 and armv7 based on target features
                let target_feature = std::env::var("CARGO_CFG_TARGET_FEATURE");
                if target_feature.as_ref().map(|s| s.contains("v7")).unwrap_or(false) {
                    Target::LinuxArmv7
                } else {
                    Target::LinuxArmv6
                }
            }
            ("linux", "arm") => Target::LinuxArmv7, // Default to armv7
            ("windows", "x86_64") => Target::WindowsAmd64,
            _ => panic!("Unsupported target: {} {}", target_os, target_arch),
        }
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
enum ArchiveFormat {
    TarGz,
    Zip,
}

impl ArchiveFormat {
    fn file_extension(self) -> &'static str {
        match self {
            ArchiveFormat::TarGz => "tar.gz",
            ArchiveFormat::Zip => "zip",
        }
    }
}

fn download_mediamtx(target: Target, out_dir: &Path) {
    const VERSION: &str = "v1.15.3";
    const BASE_URL: &str = "https://github.com/bluenviron/mediamtx/releases/download";

    let mut format = ArchiveFormat::TarGz;
    let file_suffix = match target {
        Target::DarwinAmd64 => "darwin_amd64",
        Target::DarwinArm64 => "darwin_arm64",
        Target::LinuxAmd64 => "linux_amd64",
        Target::LinuxArm64 => "linux_arm64",
        Target::LinuxArmv6 => "linux_armv6",
        Target::LinuxArmv7 => "linux_armv7",
        Target::WindowsAmd64 => {
            format = ArchiveFormat::Zip;
            "windows_amd64"
        }
    };

    let url = format!(
        "{BASE_URL}/{VERSION}/mediamtx_{VERSION}_{file_suffix}.{}",
        format.file_extension()
    );

    let archive_file_path =
        out_dir.join(format!("mediamtx_{}.{}", VERSION, format.file_extension()));
    _ = std::fs::remove_file(&archive_file_path);

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .read(true)
        .open(&archive_file_path)
        .expect("Failed to create archive file");

    {
        let response = ureq::get(&url).call().expect("Failed to download mediamtx");
        let body = response.into_body();
        let mut body = body.into_reader();

        std::io::copy(&mut body, &mut file).expect("Failed to write temporary file");
        file.flush().expect("Failed to flush temporary file");

        file.seek(std::io::SeekFrom::Start(0))
            .expect("Failed to seek to start of temporary file");
    }

    let dir_path = out_dir.join(format!("mediamtx_{}", VERSION));
    _ = std::fs::remove_dir_all(&dir_path);
    std::fs::create_dir(&dir_path).expect("Failed to create temporary directory");

    match format {
        ArchiveFormat::TarGz => {
            let tar_gz = flate2::read::GzDecoder::new(file);
            let mut archive = tar::Archive::new(tar_gz);
            archive.unpack(&dir_path).expect("Failed to unpack mediamtx");
        }
        ArchiveFormat::Zip => {
            let mut zip = zip::ZipArchive::new(file).expect("Failed to unpack mediamtx");
            zip.extract(&dir_path).expect("Failed to unpack mediamtx");
        }
    }

    let mediamtx_filename =
        if target == Target::WindowsAmd64 { "mediamtx.exe" } else { "mediamtx" };

    let mediamtx_path = dir_path.join(mediamtx_filename);
    std::fs::copy(mediamtx_path, out_dir.join("mediamtx")).expect("Failed to copy mediamtx");
}
