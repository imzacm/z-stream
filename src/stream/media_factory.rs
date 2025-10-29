use std::sync::Arc;

use gstreamer_rtsp_server::subclass::prelude::*;
use parking_lot::Mutex;

#[derive(Clone)]
pub struct AppSources {
    pub video: gstreamer_app::AppSrc,
    pub audio: gstreamer_app::AppSrc,
}

/// Shared storage for the AppSrc element.
/// This allows the feeder thread to find the AppSrc created by the RTSP factory.
pub type AppSrcStorage = Arc<Mutex<Option<AppSources>>>;

// GObject Subclass Implementation
mod imp {
    use glib::subclass::prelude::*;
    use gstreamer::prelude::*; // Add GStreamer traits here
    use gstreamer_rtsp_server::subclass::prelude::*;
    use parking_lot::Mutex;

    use super::*;
    use crate::stream::encoder::create_video_encoder; // This pulls in AppSrcStorage, etc.

    #[derive(Default)]
    pub struct MyMediaFactory {
        pub(super) storage: Mutex<Option<AppSrcStorage>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MyMediaFactory {
        const NAME: &'static str = "MyMediaFactory";
        type Type = super::MyMediaFactory;
        type ParentType = gstreamer_rtsp_server::RTSPMediaFactory;
    }

    impl ObjectImpl for MyMediaFactory {}
    impl GstObjectImpl for MyMediaFactory {}

    impl RTSPMediaFactoryImpl for MyMediaFactory {
        /// This function is called once per client connection.
        /// Since we set `set_shared(true)`, the pipeline created here
        /// will be shared among all clients.
        fn create_element(
            &self,
            _url: &gstreamer_rtsp_server::gst_rtsp::RTSPUrl,
        ) -> Option<gstreamer::Element> {
            println!("RTSP CLIENT CONNECTED: Building shared pipeline...");
            let storage = self.storage.lock();
            let storage = storage.as_ref().expect("Storage not set");

            // This is the pipeline that will be served via RTSP
            let bin = gstreamer::Bin::builder().name("rtsp-pipeline").build();

            // --- 1. Video Branch ---
            let appsrc_video = gstreamer_app::AppSrc::builder()
                .name("videosrc")
                .is_live(true)
                .stream_type(gstreamer_app::AppStreamType::Stream)
                .format(gstreamer::Format::Time)
                .build();

            let video_caps = gstreamer::Caps::builder("video/x-raw")
                .field("format", gstreamer_video::VideoFormat::I420.to_string())
                .field("width", 1280)
                .field("height", 720)
                .field("framerate", gstreamer::Fraction::new(30, 1))
                .build();
            appsrc_video.set_caps(Some(&video_caps));

            let queue_vid = gstreamer::ElementFactory::make("queue").build().ok()?;
            let videoconvert = gstreamer::ElementFactory::make("videoconvert").build().ok()?;
            let x264enc = create_video_encoder().ok()?;
            let pay_vid = gstreamer::ElementFactory::make("rtph264pay")
                .property("name", "pay0") // MUST be "pay0"
                .property("pt", 96_u32)
                .build()
                .ok()?;

            // --- 2. Audio Branch (NEW) ---
            let appsrc_audio = gstreamer_app::AppSrc::builder()
                .name("audiosrc")
                .is_live(true)
                .stream_type(gstreamer_app::AppStreamType::Stream)
                .format(gstreamer::Format::Time)
                .build();

            // This caps MUST match the caps in feeder.rs
            let audio_caps = gstreamer::Caps::builder("audio/x-raw")
                .field("format", "S16LE")
                .field("layout", "interleaved")
                .field("rate", 48000)
                .field("channels", 2)
                .build();
            appsrc_audio.set_caps(Some(&audio_caps));

            let queue_aud = gstreamer::ElementFactory::make("queue").build().ok()?;
            let audioconvert = gstreamer::ElementFactory::make("audioconvert").build().ok()?;
            let audiorate = gstreamer::ElementFactory::make("audiorate").build().ok()?;
            let avenc_aac = gstreamer::ElementFactory::make("avenc_aac").build().ok()?;
            let pay_aud = gstreamer::ElementFactory::make("rtpmp4apay")
                .property("name", "pay1") // MUST be "pay1"
                .property("pt", 97_u32)
                .build()
                .ok()?;

            // --- 3. Add to Bin and Link ---
            bin.add_many([
                // Video elements
                appsrc_video.upcast_ref(),
                &queue_vid,
                &videoconvert,
                &x264enc,
                &pay_vid,
                // Audio elements
                appsrc_audio.upcast_ref(),
                &queue_aud,
                &audioconvert,
                &audiorate,
                &avenc_aac,
                &pay_aud,
            ])
            .ok()?;

            // Link video branch
            gstreamer::Element::link_many([
                appsrc_video.upcast_ref(),
                &queue_vid,
                &videoconvert,
                &x264enc,
                &pay_vid,
            ])
            .ok()?;

            // Link audio branch
            gstreamer::Element::link_many([
                appsrc_audio.upcast_ref(),
                &queue_aud,
                &audioconvert,
                &audiorate,
                &avenc_aac,
                &pay_aud,
            ])
            .ok()?;

            // Save the appsrc to the shared storage so the feeder thread can find it
            *storage.lock() = Some(AppSources { video: appsrc_video, audio: appsrc_audio });
            println!("RTSP pipeline built.");
            Some(bin.upcast())
        }
    }
}

// Public wrapper for the GObject
glib::wrapper! {
    pub struct MyMediaFactory(ObjectSubclass<imp::MyMediaFactory>)
        @extends gstreamer_rtsp_server::RTSPMediaFactory, gstreamer::Object;
}

// Public constructor
impl MyMediaFactory {
    pub fn new(storage: AppSrcStorage) -> Self {
        let factory: Self = glib::Object::new();
        // Store the AppSrcStorage handle in our factory's implementation struct
        *factory.imp().storage.lock() = Some(storage);
        factory
    }
}
