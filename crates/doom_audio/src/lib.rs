//! Doom audio: sound-effect lumps, MUS music played through an emulated
//! OPL chip with the DMX GENMIDI instruments, and a software mixer.
//!
//! No game knowledge lives here: which sounds play, from where and how loud
//! is decided by the game layer (`doom_game`'s port of `s_sound.c`); this
//! crate turns those decisions into PCM. Hosts run [`Mixer::render`] on
//! their audio thread and feed it [`AudioCommand`]s.

pub mod decode;
pub mod mixer;
pub mod music;
pub mod opl;
mod opl_tables;

pub use decode::{decode, sniff, SoundFormat};
pub use mixer::{AudioCommand, Mixer, Sound};
pub use music::{Genmidi, MusicPlayer, Song};
