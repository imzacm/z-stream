mod encoder;
mod feeder;
mod media_factory;

use std::path::PathBuf;

use gstreamer_rtsp_server::prelude::{RTSPMediaFactoryExt, RTSPMountPointsExt, RTSPServerExt};

pub use self::feeder::*;
pub use self::media_factory::*;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("GStreamer/GLib error: {0}")]
    Glib(#[from] glib::Error),

    #[error("GStreamer boolean error: {0}")]
    GlibBool(#[from] glib::error::BoolError),

    #[error("GStreamer state change error: {0}")]
    GstStateChange(#[from] gstreamer::StateChangeError),
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum Command {
    Skip,
}

#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum Event {
    Playing { path: PathBuf },
    Ended { path: PathBuf },
}

pub fn create_server(
    file_rx: flume::Receiver<PathBuf>,
    command_rx: flume::Receiver<Command>,
    event_tx: flume::Sender<Event>,
    rtsp_port: u16,
    stream_key: &str,
) -> Result<gstreamer_rtsp_server::RTSPServer, Error> {
    let appsrc_storage = AppSrcStorage::default();

    let server = gstreamer_rtsp_server::RTSPServer::new();
    server.set_service(&rtsp_port.to_string());

    let factory = MyMediaFactory::new(appsrc_storage.clone());
    factory.set_shared(true);

    let mounts = server.mount_points().unwrap();
    let path = format!("/{stream_key}");
    mounts.add_factory(&path, factory.clone());

    std::thread::spawn(move || file_feeder_task(file_rx, command_rx, event_tx, appsrc_storage));

    Ok(server)
}
