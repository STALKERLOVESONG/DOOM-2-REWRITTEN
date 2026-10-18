//! Standard MIDI files (SMF format 0, 1 and 2, and the RIFF `RMID` wrapper),
//! parsed into per-track event lists for the OPL player.
//!
//! Only what DMX reacts to is decoded (notes, controllers, program changes,
//! pitch bends, tempo, end of track); every other event is kept as
//! [`MidiEventKind::Other`] so it still takes part in timing. Chocolate Doom
//! schedules each event separately and truncates every delay to whole
//! microseconds, so dropping events would shift playback slightly.

/// One parsed event and the delay before it, in ticks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MidiEvent {
    pub delta: u32,
    pub kind: MidiEventKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MidiEventKind {
    NoteOff { channel: u8, key: u8 },
    /// Velocity 0 is a note-off (handled by the player, as in DMX).
    NoteOn { channel: u8, key: u8, velocity: u8 },
    Controller { channel: u8, controller: u8, value: u8 },
    ProgramChange { channel: u8, program: u8 },
    /// The 14-bit bend's most significant 7 bits (DMX ignores the LSB).
    PitchBend { channel: u8, msb: u8 },
    /// Microseconds per quarter note.
    Tempo(u32),
    EndOfTrack,
    /// Aftertouch, sysex and meta events without an effect.
    Other,
}

/// How MIDI ticks map to time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeDivision {
    /// Ticks per quarter note; the tempo comes from tempo events
    /// (default 500000 µs per quarter note).
    TicksPerBeat(u16),
    /// SMPTE timing: absolute ticks per second, tempo events ignored.
    TicksPerSecond(u32),
}

#[derive(Clone, Debug)]
pub struct MidiSong {
    pub division: TimeDivision,
    pub tracks: Vec<Vec<MidiEvent>>,
}

/// Whether `data` looks like a MIDI file (SMF or RMID).
pub fn is_midi(data: &[u8]) -> bool {
    data.starts_with(b"MThd") || (data.len() >= 12 && &data[0..4] == b"RIFF" && &data[8..12] == b"RMID")
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn byte(&mut self) -> Result<u8, String> {
        let b = *self.data.get(self.pos).ok_or("unexpected end of MIDI track")?;
        self.pos += 1;
        Ok(b)
    }

    fn var_len(&mut self) -> Result<u32, String> {
        let mut v = 0u32;
        // At most four bytes in a valid file; tolerate (and cap) longer runs.
        for _ in 0..5 {
            let b = self.byte()?;
            v = (v << 7) | (b & 0x7f) as u32;
            if b & 0x80 == 0 {
                return Ok(v);
            }
        }
        Err("variable-length value too long".into())
    }

    fn skip(&mut self, n: usize) -> Result<&[u8], String> {
        let end = self.pos.checked_add(n).filter(|&e| e <= self.data.len()).ok_or("MIDI event runs past track end")?;
        let s = &self.data[self.pos..end];
        self.pos = end;
        Ok(s)
    }
}

fn be32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

impl MidiSong {
    pub fn parse(data: &[u8]) -> Result<MidiSong, String> {
        let data = unwrap_rmid(data)?;
        if data.len() < 14 || &data[0..4] != b"MThd" {
            return Err("not a MIDI file".into());
        }
        let header_len = be32(&data[4..8]) as usize;
        if header_len < 6 || 8 + header_len > data.len() {
            return Err("bad MIDI header".into());
        }
        let format = u16::from_be_bytes([data[8], data[9]]);
        let ntracks = u16::from_be_bytes([data[10], data[11]]) as usize;
        let raw_division = u16::from_be_bytes([data[12], data[13]]);
        if format > 2 {
            return Err(format!("unsupported MIDI format {format}"));
        }
        let division = if raw_division & 0x8000 != 0 {
            // High byte: negative frames per second (-24, -25, -29, -30).
            let fps = (-((raw_division >> 8) as i8) as i32).max(1) as u32;
            TimeDivision::TicksPerSecond(fps * (raw_division & 0xff).max(1) as u32)
        } else {
            TimeDivision::TicksPerBeat(raw_division.max(1))
        };

        let mut tracks = Vec::with_capacity(ntracks);
        let mut p = 8 + header_len;
        // Chunks other than MTrk are skipped; a short final chunk ends the file.
        while tracks.len() < ntracks && p + 8 <= data.len() {
            let id = &data[p..p + 4];
            let len = be32(&data[p + 4..p + 8]) as usize;
            let body = &data[p + 8..(p + 8).saturating_add(len).min(data.len())];
            p = p + 8 + len;
            if id == b"MTrk" {
                tracks.push(parse_track(body)?);
            }
        }
        if tracks.is_empty() {
            return Err("MIDI file has no tracks".into());
        }
        Ok(MidiSong { division, tracks })
    }
}

/// RMID: a RIFF container whose `data` chunk is an SMF.
fn unwrap_rmid(data: &[u8]) -> Result<&[u8], String> {
    if !(data.len() >= 12 && &data[0..4] == b"RIFF" && &data[8..12] == b"RMID") {
        return Ok(data);
    }
    let mut p = 12;
    while p + 8 <= data.len() {
        let len = u32::from_le_bytes([data[p + 4], data[p + 5], data[p + 6], data[p + 7]]) as usize;
        let body = &data[p + 8..(p + 8).saturating_add(len).min(data.len())];
        if &data[p..p + 4] == b"data" {
            return Ok(body);
        }
        p += 8 + len + (len & 1);
    }
    Err("RMID file without a data chunk".into())
}

fn parse_track(data: &[u8]) -> Result<Vec<MidiEvent>, String> {
    let mut r = Reader { data, pos: 0 };
    let mut events = Vec::new();
    let mut running: Option<u8> = None;
    while r.pos < data.len() {
        let delta = r.var_len()?;
        let mut status = r.byte()?;
        let first_data = if status < 0x80 {
            // Running status: this byte is the first data byte.
            let first = status;
            status = running.ok_or("MIDI data byte without a status")?;
            Some(first)
        } else {
            None
        };
        let kind = match status {
            0x80..=0xef => {
                running = Some(status);
                let channel = status & 0x0f;
                let a = match first_data {
                    Some(b) => b,
                    None => r.byte()?,
                } & 0x7f;
                match status & 0xf0 {
                    0xc0 => MidiEventKind::ProgramChange { channel, program: a },
                    0xd0 => MidiEventKind::Other,
                    hi => {
                        let b = r.byte()? & 0x7f;
                        match hi {
                            0x80 => MidiEventKind::NoteOff { channel, key: a },
                            0x90 => MidiEventKind::NoteOn { channel, key: a, velocity: b },
                            0xb0 => MidiEventKind::Controller { channel, controller: a, value: b },
                            0xe0 => MidiEventKind::PitchBend { channel, msb: b },
                            _ => MidiEventKind::Other, // 0xa0 polyphonic aftertouch
                        }
                    }
                }
            }
            0xf0 | 0xf7 => {
                // Sysex cancels running status.
                running = None;
                let len = r.var_len()? as usize;
                r.skip(len)?;
                MidiEventKind::Other
            }
            0xff => {
                running = None;
                let ty = r.byte()?;
                let len = r.var_len()? as usize;
                let body = r.skip(len)?;
                match ty {
                    0x2f => MidiEventKind::EndOfTrack,
                    0x51 if len >= 3 => MidiEventKind::Tempo(u32::from_be_bytes([0, body[0], body[1], body[2]])),
                    _ => MidiEventKind::Other,
                }
            }
            // System common/real-time bytes have no business in a file;
            // treat them as single-byte events.
            _ => MidiEventKind::Other,
        };
        events.push(MidiEvent { delta, kind });
        if kind == MidiEventKind::EndOfTrack {
            break;
        }
    }
    if events.last().map(|e| e.kind) != Some(MidiEventKind::EndOfTrack) {
        events.push(MidiEvent { delta: 0, kind: MidiEventKind::EndOfTrack });
    }
    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn smf(division: u16, tracks: &[&[u8]]) -> Vec<u8> {
        let mut v = b"MThd\0\0\0\x06".to_vec();
        v.extend_from_slice(&1u16.to_be_bytes());
        v.extend_from_slice(&(tracks.len() as u16).to_be_bytes());
        v.extend_from_slice(&division.to_be_bytes());
        for t in tracks {
            v.extend_from_slice(b"MTrk");
            v.extend_from_slice(&(t.len() as u32).to_be_bytes());
            v.extend_from_slice(t);
        }
        v
    }

    #[test]
    fn running_status_meta_and_sysex() {
        let track: &[u8] = &[
            0x00, 0xff, 0x51, 0x03, 0x07, 0xa1, 0x20, // tempo 500000
            0x00, 0x90, 60, 100, // note on
            0x60, 64, 90, // running status note on
            0x81, 0x00, 0xf0, 0x02, 0x7e, 0xf7, // sysex after 128 ticks
            0x00, 0xc9, 5, // program change ch 9
            0x10, 0xe0, 0x00, 0x40, // pitch bend centre
            0x00, 0xff, 0x2f, 0x00,
        ];
        let s = MidiSong::parse(&smf(96, &[track])).unwrap();
        assert_eq!(s.division, TimeDivision::TicksPerBeat(96));
        let k: Vec<_> = s.tracks[0].iter().map(|e| (e.delta, e.kind)).collect();
        assert_eq!(
            k,
            vec![
                (0, MidiEventKind::Tempo(500000)),
                (0, MidiEventKind::NoteOn { channel: 0, key: 60, velocity: 100 }),
                (0x60, MidiEventKind::NoteOn { channel: 0, key: 64, velocity: 90 }),
                (128, MidiEventKind::Other),
                (0, MidiEventKind::ProgramChange { channel: 9, program: 5 }),
                (0x10, MidiEventKind::PitchBend { channel: 0, msb: 0x40 }),
                (0, MidiEventKind::EndOfTrack),
            ]
        );
    }

    #[test]
    fn missing_end_of_track_and_rmid() {
        let track: &[u8] = &[0x00, 0x80, 60, 0];
        let file = smf(0xe728, &[track]); // SMPTE -25 fps, 40 ticks/frame
        let mut rmid = b"RIFF\0\0\0\0RMIDdata".to_vec();
        rmid.extend_from_slice(&(file.len() as u32).to_le_bytes());
        rmid.extend_from_slice(&file);
        assert!(is_midi(&rmid));
        let s = MidiSong::parse(&rmid).unwrap();
        assert_eq!(s.division, TimeDivision::TicksPerSecond(1000));
        assert_eq!(s.tracks[0].last().unwrap().kind, MidiEventKind::EndOfTrack);
    }
}
