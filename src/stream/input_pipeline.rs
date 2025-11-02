use std::path::PathBuf;

use glib::object::{Cast, ObjectExt, ObjectType};
use gstreamer::prelude::{
    ElementExt, ElementExtManual, GstBinExt, GstBinExtManual, GstObjectExt, PadExt,
};

use super::{AppSources, Command, Error};
use crate::stream::input_bin::InputBin;

struct InputBinItem {
    bin: InputBin,
    video_sink_pad: gstreamer::Pad,
    audio_sink_pad: gstreamer::Pad,
    pre_rolled: bool,
}

pub struct InputPipeline<I> {
    file_iter: I,
    pub pipeline: gstreamer::Pipeline,
    input_bins: Vec<InputBinItem>,
    active_bin: usize,
    video_input_selector: gstreamer::Element,
    audio_input_selector: gstreamer::Element,
    video_app_sink: gstreamer_app::AppSink,
    audio_app_sink: gstreamer_app::AppSink,
    app_sources: AppSources,
}

impl<I> InputPipeline<I>
where
    I: Iterator<Item = PathBuf>,
{
    pub fn new<II>(
        file_iter: II,
        pre_roll_count: usize,
        app_sources: AppSources,
    ) -> Result<Self, Error>
    where
        II: IntoIterator<IntoIter = I>,
    {
        let pipeline = gstreamer::Pipeline::builder().name("input-pipeline").build();

        let video_input_selector = gstreamer::ElementFactory::make("input-selector")
            .name("video_input_selector")
            .property("cache-buffers", true)
            .property_from_str("sync-mode", "clock")
            .property("sync-streams", true)
            .build()?;
        let audio_input_selector = gstreamer::ElementFactory::make("input-selector")
            .name("audio_input_selector")
            .property("cache-buffers", true)
            .property_from_str("sync-mode", "clock")
            .property("sync-streams", true)
            .build()?;

        let video_app_sink = gstreamer_app::AppSink::builder().name("video_app_sink").build();
        let audio_app_sink = gstreamer_app::AppSink::builder().name("audio_app_sink").build();

        pipeline.add_many([
            &video_input_selector,
            &audio_input_selector,
            video_app_sink.upcast_ref(),
            audio_app_sink.upcast_ref(),
        ])?;

        video_input_selector.link(&video_app_sink)?;
        audio_input_selector.link(&audio_app_sink)?;

        let mut file_iter = file_iter.into_iter();

        let mut input_bins = Vec::with_capacity(pre_roll_count + 1);
        for index in 0..=pre_roll_count {
            let bin = InputBin::new(format!("input_bin_{index}"))?;
            pipeline.add(&bin.bin)?;

            let path = file_iter.next().expect("File iterator returned None");
            bin.set_path(&path)?;

            let video_sink_pad = video_input_selector
                .request_pad_simple("sink_%u")
                .expect("Failed to request video pad");
            let audio_sink_pad = audio_input_selector
                .request_pad_simple("sink_%u")
                .expect("Failed to request audio pad");

            let video_src_pad = bin.bin.static_pad("video_src").unwrap();
            let audio_src_pad = bin.bin.static_pad("audio_src").unwrap();

            video_src_pad.link(&video_sink_pad)?;
            audio_src_pad.link(&audio_sink_pad)?;

            if index == 0 {
                video_input_selector.set_property("active-pad", &video_sink_pad);
                audio_input_selector.set_property("active-pad", &audio_sink_pad);
            }

            println!("Pre-rolling {}", bin.uri());

            let item = InputBinItem { bin, video_sink_pad, audio_sink_pad, pre_rolled: false };
            input_bins.push(item);
        }

        // Link appsinks -> appsrcs
        let video_app_src = app_sources.video.downgrade();
        video_app_sink.set_callbacks(
            gstreamer_app::AppSinkCallbacks::builder()
                .new_sample(move |sink| {
                    let Some(video_app_src) = video_app_src.upgrade() else {
                        return Err(gstreamer::FlowError::Eos);
                    };
                    let sample = sink.pull_sample().map_err(|_| gstreamer::FlowError::Eos)?;
                    video_app_src.push_sample(&sample).map_err(|_| gstreamer::FlowError::Error)
                })
                .build(),
        );

        let audio_app_src = app_sources.audio.downgrade();
        audio_app_sink.set_callbacks(
            gstreamer_app::AppSinkCallbacks::builder()
                .new_sample(move |sink| {
                    let Some(audio_app_src) = audio_app_src.upgrade() else {
                        return Err(gstreamer::FlowError::Eos);
                    };
                    let sample = sink.pull_sample().map_err(|_| gstreamer::FlowError::Eos)?;
                    audio_app_src.push_sample(&sample).map_err(|_| gstreamer::FlowError::Error)
                })
                .build(),
        );

        Ok(Self {
            file_iter,
            pipeline,
            input_bins,
            active_bin: 0,
            video_input_selector,
            audio_input_selector,
            video_app_sink,
            audio_app_sink,
            app_sources,
        })
    }

    fn switch_bin_uri(&mut self, index: usize) -> Result<(), Error> {
        let item = &mut self.input_bins[index];
        item.pre_rolled = false;

        let bin = &item.bin;

        bin.bin.set_state(gstreamer::State::Null)?;
        bin.bin.send_event(gstreamer::event::FlushStart::new());
        bin.bin.send_event(gstreamer::event::FlushStop::new(true));

        let path = self.file_iter.next().expect("File iterator returned None");
        bin.set_path(&path)?;

        bin.bin.set_state(gstreamer::State::Paused)?;

        println!("Pre-rolling {}", bin.uri());

        Ok(())
    }

    fn set_active(&mut self, index: usize) -> Result<(), Error> {
        assert!(index < self.input_bins.len());
        self.active_bin = index;

        self.video_input_selector.set_state(gstreamer::State::Paused)?;
        self.audio_input_selector.set_state(gstreamer::State::Paused)?;

        let item = &self.input_bins[index];
        self.video_input_selector.set_property("active-pad", &item.video_sink_pad);
        self.audio_input_selector.set_property("active-pad", &item.audio_sink_pad);

        if item.pre_rolled {
            item.bin.bin.set_state(gstreamer::State::Playing)?;
            println!("Playing {}", item.bin.uri());
        }

        self.video_input_selector.set_state(gstreamer::State::Playing)?;
        self.audio_input_selector.set_state(gstreamer::State::Playing)?;

        Ok(())
    }

    fn switch_next(&mut self) -> Result<(), Error> {
        let mut next_index = self.active_bin + 1;
        loop {
            if next_index == self.input_bins.len() {
                next_index = 0;
            }
            if next_index == self.active_bin {
                // No files are pre-rolled.
                if next_index + 1 < self.input_bins.len() {
                    next_index += 1;
                }
                break;
            }

            // Prefer pre-rolled files.
            if self.input_bins[next_index].pre_rolled {
                break;
            }

            next_index += 1;
        }

        self.set_active(next_index)?;
        Ok(())
    }

    fn handle_message(&mut self, message: &gstreamer::Message) -> Result<(), Error> {
        use gstreamer::MessageView;

        let source_bin = self.get_message_bin(message);
        let bin_index = source_bin.map(|(index, _)| index);
        let bin_item = source_bin.map(|(_, item)| item);

        match message.view() {
            MessageView::Eos(..) if bin_index.is_some_and(|i| i == self.active_bin) => {
                self.switch_next()?;
                self.switch_bin_uri(bin_index.unwrap())?;
            }
            MessageView::AsyncDone(_)
                if bin_item
                    .is_some_and(|i| i.bin.bin.current_state() == gstreamer::State::Paused) =>
            {
                let item = &mut self.input_bins[bin_index.unwrap()];
                item.pre_rolled = true;
                let active = &self.input_bins[self.active_bin];
                if bin_index.unwrap() == self.active_bin || !active.pre_rolled {
                    self.set_active(bin_index.unwrap())?;
                }
            }
            MessageView::Error(err) => {
                eprintln!("Error on pipeline: {} (debug: {:?})", err.error(), err.debug());
                if let Some(bin_index) = bin_index {
                    if bin_index == self.active_bin {
                        self.switch_next()?;
                    }
                    self.switch_bin_uri(bin_index)?;
                }
            }
            _ => (),
        }

        Ok(())
    }

    fn get_message_bin(&self, message: &gstreamer::Message) -> Option<(usize, &InputBinItem)> {
        let mut object = message.src()?.clone();
        loop {
            let object_ptr = object.as_ptr();
            for (index, item) in self.input_bins.iter().enumerate() {
                let bin_ptr = item.bin.bin.upcast_ref::<gstreamer::Object>().as_ptr();
                if std::ptr::eq(bin_ptr, object_ptr) {
                    return Some((index, item));
                }
            }

            object = object.parent()?;
        }
    }

    pub fn play(&mut self, command_rx: &flume::Receiver<Command>) -> Result<(), Error> {
        self.pipeline.set_state(gstreamer::State::Playing)?;

        let bus = self.pipeline.bus().unwrap();

        loop {
            match command_rx.recv_timeout(std::time::Duration::from_millis(10)) {
                Ok(Command::Skip) => {
                    let bin_index = self.active_bin;
                    self.switch_next()?;
                    self.switch_bin_uri(bin_index)?;
                }
                Ok(Command::Abort) => break,
                _ => (),
            }

            for msg in bus.iter_timed(gstreamer::ClockTime::from_mseconds(10)) {
                self.handle_message(&msg)?;
            }
        }

        _ = self.pipeline.set_state(gstreamer::State::Null);

        Ok(())
    }
}
