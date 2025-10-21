use std::fs;
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::process::{Child, Command, ExitStatus, Stdio};

const XIU_BYTES: &[u8] = include_bytes!(env!("CARGO_BIN_FILE_XIU"));

#[derive(Debug)]
pub struct XiuConfig {
    pub rtmp_port: u16,
    pub rtsp_port: u16,
    pub webrtc_port: u16,
    pub hls_port: u16,
    pub http_flv_port: u16,
}

impl Default for XiuConfig {
    fn default() -> Self {
        Self {
            rtmp_port: 1935,
            rtsp_port: 5540,
            webrtc_port: 8900,
            hls_port: 8081,
            http_flv_port: 8080,
        }
    }
}

pub struct XiuServer {
    config: XiuConfig,
    process: Child,
}

impl XiuServer {
    pub fn start(config: XiuConfig) -> std::io::Result<Self> {
        let mut temp_file = {
            let mut builder = tempfile::Builder::new();
            if cfg!(target_os = "windows") {
                builder.suffix(".exe");
            }
            builder.tempfile()?
        };

        temp_file.write_all(XIU_BYTES)?;
        temp_file.flush()?;

        let (_file, path) = temp_file.keep()?;
        drop(_file);

        #[cfg(unix)]
        {
            let mut perms = fs::metadata(&path)?.permissions();
            perms.set_mode(0o755); // rwxr-xr-x
            fs::set_permissions(&path, perms)?;
        }

        println!("Extracted xiu to: {path:?}");

        println!("Starting xiu server...");
        let process = Command::new(path)
            .args(["-r", &config.rtmp_port.to_string()])
            .args(["-t", &config.rtsp_port.to_string()])
            .args(["-w", &config.webrtc_port.to_string()])
            .args(["-s", &config.hls_port.to_string()])
            .args(["-f", &config.http_flv_port.to_string()])
            .arg("-l")
            .arg("info")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::inherit())
            .spawn()?;

        println!("xiu server started with PID: {}", process.id());

        Ok(XiuServer { config, process })
    }

    pub fn config(&self) -> &XiuConfig {
        &self.config
    }

    pub fn wait(&mut self) -> std::io::Result<ExitStatus> {
        self.process.wait()
    }
}

impl Drop for XiuServer {
    fn drop(&mut self) {
        println!("Stopping xiu server (PID: {})...", self.process.id());
        if let Err(e) = self.process.kill() {
            eprintln!("Failed to kill xiu server: {}", e);
        }
    }
}
