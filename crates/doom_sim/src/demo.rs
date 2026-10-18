//! Vanilla demo lumps (g_game.c `G_DoPlayDemo` / `G_ReadDemoTiccmd`).

use crate::player::MAXPLAYERS;
use crate::ticcmd::TicCmd;
use crate::world::Skill;

/// End-of-demo marker byte.
pub const DEMOMARKER: u8 = 0x80;

#[derive(Clone, Debug)]
pub struct Demo {
    /// Version byte (109 = v1.9; `None` for the headerless <= 1.2 format).
    pub version: Option<u8>,
    pub skill: Skill,
    pub episode: i32,
    pub map: i32,
    pub deathmatch: i32,
    pub respawnparm: bool,
    pub fastparm: bool,
    pub nomonsters: bool,
    pub consoleplayer: usize,
    pub playeringame: [bool; MAXPLAYERS],
    /// Commands per tic, for every player slot (unused slots stay default).
    pub tics: Vec<[TicCmd; MAXPLAYERS]>,
}

impl Demo {
    /// Parse a vanilla (short-tics) demo. Doom 1.91 "longtics" is not
    /// supported yet.
    pub fn parse(data: &[u8]) -> Result<Demo, String> {
        let mut p = 0usize;
        let mut byte = |what: &str| -> Result<u8, String> {
            let b = *data.get(p).ok_or_else(|| format!("demo truncated reading {what}"))?;
            p += 1;
            Ok(b)
        };
        let first = byte("version")?;
        let (version, skill) = if first <= 4 { (None, first) } else { (Some(first), byte("skill")?) };
        if let Some(v) = version {
            if v == 111 {
                return Err("Doom 1.91 longtics demos are not supported yet".into());
            }
            if !(104..=109).contains(&v) {
                return Err(format!("unsupported demo version {v}"));
            }
        }
        let episode = byte("episode")? as i32;
        let map = byte("map")? as i32;
        let (mut deathmatch, mut respawnparm, mut fastparm, mut nomonsters, mut consoleplayer) = (0, false, false, false, 0);
        if version.is_some() {
            deathmatch = byte("deathmatch")? as i32;
            respawnparm = byte("respawn")? != 0;
            fastparm = byte("fast")? != 0;
            nomonsters = byte("nomonsters")? != 0;
            consoleplayer = byte("consoleplayer")? as usize;
        }
        let mut playeringame = [false; MAXPLAYERS];
        for pig in &mut playeringame {
            *pig = byte("playeringame")? != 0;
        }
        let skill = match skill {
            0 => Skill::Baby,
            1 => Skill::Easy,
            2 => Skill::Medium,
            3 => Skill::Hard,
            _ => Skill::Nightmare,
        };

        let mut tics = Vec::new();
        'outer: loop {
            let mut cmds = [TicCmd::default(); MAXPLAYERS];
            for (i, cmd) in cmds.iter_mut().enumerate() {
                if !playeringame[i] {
                    continue;
                }
                match data.get(p) {
                    None | Some(&DEMOMARKER) => break 'outer,
                    _ => {}
                }
                if p + 4 > data.len() {
                    break 'outer;
                }
                cmd.forwardmove = data[p] as i8;
                cmd.sidemove = data[p + 1] as i8;
                cmd.angleturn = ((data[p + 2] as u16) << 8) as i16;
                cmd.buttons = data[p + 3];
                p += 4;
            }
            tics.push(cmds);
        }
        Ok(Demo {
            version,
            skill,
            episode,
            map,
            deathmatch,
            respawnparm,
            fastparm,
            nomonsters,
            consoleplayer,
            playeringame,
            tics,
        })
    }
}
