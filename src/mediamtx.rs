use std::process::{Child, Command, Stdio};
use std::sync::{Arc, OnceLock};

const CONFIG_YAML: &str = r#"
###############################################
# Path settings
paths:
  my_stream:

  # Settings under path "all_others" are applied to all paths that
  # do not match another entry.
  all_others:
"#;

const MEDIAMTX_BIN: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/mediamtx"));

fn get_mediamtx_dir() -> &'static Result<Arc<tempfile::TempDir>, Arc<std::io::Error>> {
    static MEDIAMTX_DIR: OnceLock<Result<Arc<tempfile::TempDir>, Arc<std::io::Error>>> =
        OnceLock::new();

    MEDIAMTX_DIR.get_or_init(|| {
        let dir = tempfile::tempdir()?;

        let mut mediamtx_bin = dir.path().join("mediamtx");
        if cfg!(windows) {
            mediamtx_bin.set_extension("exe");
        }
        std::fs::write(&mediamtx_bin, MEDIAMTX_BIN)?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let mut perms = std::fs::metadata(&mediamtx_bin)?.permissions();
            perms.set_mode(0o755); // rwxr-xr-x
            std::fs::set_permissions(&mediamtx_bin, perms)?;
        }

        let mediamtx_yml = dir.path().join("mediamtx.yml");
        std::fs::write(&mediamtx_yml, CONFIG_YAML)?;

        Ok(Arc::new(dir))
    })
}

pub fn start() -> Result<Child, Arc<std::io::Error>> {
    let dir = get_mediamtx_dir().as_ref().map_err(Arc::clone)?;

    let mut mediamtx_bin = dir.path().join("mediamtx");
    if cfg!(windows) {
        mediamtx_bin.set_extension("exe");
    }

    Command::new(mediamtx_bin)
        .current_dir(dir.path())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(Arc::new)
}
