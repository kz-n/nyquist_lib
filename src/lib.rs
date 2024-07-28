#![feature(let_chains)]

use std::ops::DerefMut;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use kira::manager::{backend::DefaultBackend, AudioManager, AudioManagerSettings};
use kira::sound::streaming::{StreamingSoundData, StreamingSoundHandle};
use kira::sound::FromFileError;
use kira::sound::PlaybackState::{Paused, Playing};
use kira::track::TrackBuilder;
use kira::tween::Tween;
use parking_lot::Mutex;
use parking_lot_mpsc::*;

// Message Passer (useful to Nyquist struct)
struct MessagePasser {
    tx: Sender<(Message, MessageValue)>,
}

// Lib entry point, this object has to stay alive for the lib to function
pub struct Nyquist {
    pub playlist: Arc<Mutex<Playlist>>,
    message_passer: MessagePasser,
}

impl Default for Nyquist {
    fn default() -> Self {
        Self::new()
    }
}

impl Nyquist {
    pub fn new() -> Self {
        let (tx, rx) = channel::<(Message, MessageValue)>();
        let playlist = create_playlist();

        let playlist_clone = Arc::clone(&playlist);
        thread::spawn(move || manager_thread(playlist_clone));

        let playlist_clone = Arc::clone(&playlist);
        thread::spawn(move || receiver_thread(playlist_clone, rx));

        Self {
            playlist,
            message_passer: MessagePasser { tx },
        }
    }

    pub fn add_to_playlist(&self, track: Track) {
        let mut playlist_guard = self.playlist.lock();
        playlist_guard.queue.push(track.clone());
        playlist_guard.playing = Some(track);
        println!("bazinga")
    }

    pub fn list(&self) -> Vec<Track> {
        self.playlist.lock().queue.clone()
    }

    pub fn pause_playback(&self) -> Result<(), SendError<(Message, MessageValue)>> {
        self.message_passer
            .tx
            .send((Message::PlaybackPause, MessageValue::none()))
    }

    pub fn resume_playback(&self) -> Result<(), SendError<(Message, MessageValue)>> {
        self.message_passer
            .tx
            .send((Message::PlaybackResume, MessageValue::none()))
    }

    pub fn get_time(&self) -> (Duration, Duration) {
        let playlist_guard = self.playlist.lock();
        (playlist_guard.current_duration, playlist_guard.current_time)
    }
    pub fn get_vol(&self) -> f64 {
        return self.playlist.lock().current_volume;
    }

    pub fn set_vol(&self, vol: f64) {
        self.playlist.lock().current_volume = vol;
    }
}

// Track structure representing a single audio track
#[derive(Clone, Debug)]
pub struct Track {
    pub path: String,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum Message {
    None,
    PlaylistUpdated,
    PlaybackPause,
    PlaybackResume,
    EffectVolume,
}

#[derive(Clone, Debug)]
pub struct MessageValue {
    pub float: Option<f64>,
    pub int: Option<i32>,
    pub string: Option<String>,
}

impl MessageValue {
    pub fn none() -> MessageValue {
        Self {
            float: None,
            int: None,
            string: None,
        }
    }

    pub fn float(float: f64) -> Self {
        Self {
            float: Some(float),
            int: None,
            string: None,
        }
    }

    pub fn int(int: Option<i32>) -> Self {
        Self {
            float: None,
            int,
            string: None,
        }
    }

    pub fn string(string: Option<String>) -> Self {
        Self {
            float: None,
            int: None,
            string,
        }
    }
}

// Playlist structure maintaining the queue of tracks and playback state
pub struct Playlist {
    pub queue: Vec<Track>,
    pub playing: Option<Track>,
    pub paused: bool,
    pub current_duration: Duration,
    pub current_time: Duration,
    pub current_volume: f64,
    sound_handle: Option<StreamingSoundHandle<FromFileError>>,
}

// Creates the playlist for use in the program
pub fn create_playlist() -> Arc<Mutex<Playlist>> {
    Arc::new(Mutex::new(Playlist {
        queue: vec![],
        playing: None,
        paused: false,
        current_duration: Default::default(),
        current_time: Default::default(),
        current_volume: 100.0,
        sound_handle: None,
    }))
}

// Thread that queues more songs after they are done playing
fn manager_thread(playlist: Arc<Mutex<Playlist>>) {
    let mut manager = AudioManager::<DefaultBackend>::new(AudioManagerSettings::default()).unwrap();
    let kira_track = manager.add_sub_track(TrackBuilder::default()).unwrap();

    loop {
        let mut guard = playlist.lock();
        let playlist_mut = guard.deref_mut();

        let handle_option = &mut playlist_mut.sound_handle;
        match handle_option {
            None => {
                // Check if there is a track to play
                if let Some(playing) = &playlist_mut.playing {
                    let sound_data = StreamingSoundData::from_file(&playing.path)
                        .unwrap()
                        .output_destination(&kira_track);
                    playlist_mut.current_duration = sound_data.duration();

                    let mut handle = manager.play(sound_data).unwrap();
                    // Pause the sound immediately if the playlist is in a paused state
                    if playlist_mut.paused {
                        handle.pause(Tween::default());
                    }

                    *handle_option = Some(handle);
                }
            }
            Some(handle) => {
                playlist_mut.current_time = Duration::from_secs_f64(handle.position());
            }
        }

        // Yields to ensure this doesn't loop infinitely
        thread::yield_now();
    }
}

// Receiver thread that listens for messages and controls playback
fn receiver_thread(playlist: Arc<Mutex<Playlist>>, rx: Receiver<(Message, MessageValue)>) {
    // Instead of using Tokio::yield(), the iterator of rx automatically blocks this thread until a new message is ready
    // The iterator ends after the channel hungs up
    for (kind, value) in rx.into_iter() {
        match kind {
            Message::PlaybackPause => {
                let mut guard = playlist.lock();
                let playlist_mut = guard.deref_mut();

                // Pause the current sound if it's playing
                if let Some(handle) = playlist_mut.sound_handle.as_mut()
                    && handle.state() == Playing
                {
                    handle.pause(Tween::default());
                    println!("Playback paused");
                    playlist_mut.paused = true;
                }
            }
            Message::PlaybackResume => {
                let mut guard = playlist.lock();
                let playlist_mut = guard.deref_mut();

                // Resume the current sound if it's paused
                if let Some(handle) = playlist_mut.sound_handle.as_mut()
                    && handle.state() == Paused
                {
                    handle.resume(Tween::default());
                    println!("Playback resumed");
                    playlist_mut.paused = false;
                }
            }
            Message::EffectVolume => {
                let mut guard = playlist.lock();
                let playlist_mut = guard.deref_mut();

                if let Some(handle) = playlist_mut.sound_handle.as_mut() {
                    playlist_mut.current_volume = value.float.unwrap();
                    handle.set_volume(playlist_mut.current_volume, Default::default());
                }
            }
            Message::None | Message::PlaylistUpdated => {}
        }
    }
}
