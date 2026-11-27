//! Device checks outside calls on Windows and Linux: not there yet (`media_tester.rs` reads AVFoundation). The
//! Settings page keeps its device pickers; the microphone level, play-back check, chime and camera preview do nothing.
use crate::media_engine::{MediaMsg, Window};
use crate::media_video::FrameSlot;
use crate::oxide::Oxide;
use futures::channel::mpsc::UnboundedSender;
use gpui::Context;
use std::sync::Arc;

/// Which screen a tester belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Owner {
    Settings,
    Prejoin,
}

pub enum TesterMsg {}

pub struct MicrophoneTester {
    pub level: f32,
    pub running: bool,
    pub recording: bool,
    pub playing_back: bool,
    pub error: Option<String>,
}

impl MicrophoneTester {
    pub fn new(_owner: Owner) -> Self {
        Self {
            level: 0.,
            running: false,
            recording: false,
            playing_back: false,
            error: None,
        }
    }
}

#[derive(Default)]
pub struct SpeakerTester {
    pub playing: bool,
    pub error: Option<String>,
}

pub fn start_mic(_this: &mut Oxide, _owner: Owner, _cx: &mut Context<Oxide>) {}

pub fn stop_mic(_this: &mut Oxide, _owner: Owner) {}

pub fn echo(_this: &mut Oxide, _owner: Owner, _cx: &mut Context<Oxide>) {}

pub fn play_chime(_this: &mut Oxide) {}

pub struct CameraPreview {
    pub slot: Arc<FrameSlot>,
}

impl CameraPreview {
    pub fn new(_window: Window, _tx: UnboundedSender<MediaMsg>) -> Option<Self> {
        None
    }

    pub fn show<T>(&mut self, _device: Option<T>) {}
}
