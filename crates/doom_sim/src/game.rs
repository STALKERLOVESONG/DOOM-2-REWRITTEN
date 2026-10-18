//! Game flow (g_game.c subset): new game, level setup, player reborn, the
//! per-tic `G_Ticker` for the level state.
//!
//! Content access stays with the host: when the game needs a different level
//! loaded it returns a [`GameAction`] and the host builds the [`Level`] and
//! calls [`World::setup_level`].

use std::collections::BTreeMap;

use doom_content::game::GameMode;
use doom_core::DoomRandom;

use crate::geometry::Opening;
use crate::info::Info;
use crate::level::Level;
use crate::player::{Player, PlayerState, Weapon, MAXPLAYERS, NUMAMMO};
use crate::thinker::Thinkers;
use crate::ticcmd::{bt, TicCmd};
use crate::world::{GameParams, MapGlobals, World, ITEMQUESIZE};

/// What the host must do after a tic (`gameaction_t` subset).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GameAction {
    /// Reload the current level (single-player death: `G_DoReborn`).
    LoadLevel,
    /// The level was exited (`ga_completed`). Intermission and the choice
    /// of next map are the host's job until phase 11.
    Completed(crate::world::GameExit),
}


/// `G_PlayerReborn`: called after a player dies; almost everything is
/// cleared and initialised.
pub fn player_reborn(w: &mut World, player: usize) {
    let old = &w.players[player];
    let (frags, killcount, itemcount, secretcount) = (old.frags, old.killcount, old.itemcount, old.secretcount);
    let mut p = Player { frags, killcount, itemcount, secretcount, ..Default::default() };
    p.usedown = true; // don't do anything immediately
    p.attackdown = true;
    p.playerstate = PlayerState::Live;
    p.health = w.deh.initial_health; // use dehacked value
    p.readyweapon = Weapon::Pistol;
    p.pendingweapon = Weapon::Pistol;
    p.weaponowned[Weapon::Fist as usize] = true;
    p.weaponowned[Weapon::Pistol as usize] = true;
    p.ammo[0] = w.deh.initial_bullets;
    p.maxammo[..NUMAMMO].copy_from_slice(&w.info.maxammo);
    w.players[player] = p;
}

/// `G_InitNew`'s "fast monsters" table patching.
///
/// Vanilla patches the global tables and only undoes the patch when moving
/// from a Nightmare game to a non-Nightmare one, keyed on the *previous*
/// `gameskill` (which starts as `sk_baby`). A host that keeps one `Info`
/// across new games should pass the previous skill to reproduce that
/// (including the bug that `-fast` stays in effect for later games).
pub fn apply_fast_monsters(info: &mut Info, skill: crate::world::Skill, fastparm: bool, prev_skill: crate::world::Skill) {
    use crate::info::*;
    use crate::world::Skill;
    let range = S_SARG_RUN1.idx()..=S_SARG_PAIN2.idx();
    if fastparm || (skill == Skill::Nightmare && prev_skill != Skill::Nightmare) {
        for i in range {
            info.states[i].tics >>= 1;
        }
        info.mobjinfo[MT_BRUISERSHOT.idx()].speed = 20 * doom_core::FRACUNIT;
        info.mobjinfo[MT_HEADSHOT.idx()].speed = 20 * doom_core::FRACUNIT;
        info.mobjinfo[MT_TROOPSHOT.idx()].speed = 20 * doom_core::FRACUNIT;
    } else if skill != Skill::Nightmare && prev_skill == Skill::Nightmare {
        for i in range {
            info.states[i].tics <<= 1;
        }
        info.mobjinfo[MT_BRUISERSHOT.idx()].speed = 15 * doom_core::FRACUNIT;
        info.mobjinfo[MT_HEADSHOT.idx()].speed = 10 * doom_core::FRACUNIT;
        info.mobjinfo[MT_TROOPSHOT.idx()].speed = 10 * doom_core::FRACUNIT;
    }
}

/// `G_InitNew`'s sky selection. Vanilla bug preserved: in Doom II the sky
/// is chosen once per *game*, so it never changes between levels
/// ("Sky never changes in Doom II") — except in the Final Doom alternate
/// executable and Chex Quest, where `G_DoLoadLevel` picks it per map.
pub fn sky_texture_name(mode: GameMode, episode: i32, map: i32) -> &'static str {
    if mode == GameMode::Commercial {
        if map < 12 {
            "SKY1"
        } else if map < 21 {
            "SKY2"
        } else {
            "SKY3"
        }
    } else {
        match episode {
            2 => "SKY2",
            3 => "SKY3",
            4 => "SKY4", // Special Edition sky
            _ => "SKY1",
        }
    }
}

/// The Doom II monsters that `P_LoadThings` refuses outside commercial mode.
fn is_commercial_only(kind: i16) -> bool {
    matches!(kind, 68 | 64 | 88 | 89 | 69 | 67 | 71 | 65 | 66 | 84)
}

impl World {
    /// `G_InitNew` + first `G_DoLoadLevel`: start a new game on `level`.
    pub fn new_game(
        info: Info,
        content: crate::content::ContentInfo,
        params: GameParams,
        playeringame: [bool; MAXPLAYERS],
        level: Level,
    ) -> Result<Self, String> {
        let mut params = params;
        let mut info = info;
        // `G_InitNew`.
        params.respawnmonsters = params.skill == crate::world::Skill::Nightmare || params.respawnmonsters;
        apply_fast_monsters(&mut info, params.skill, params.fastparm, crate::world::Skill::Baby);
        let sky_texture = crate::ids::TexNum(
            content.texture_num(sky_texture_name(params.mode, params.episode, params.map)).unwrap_or(0) as u16,
        );
        let spec = crate::spec::SpecTables::build(&content, params.mode);
        let texture_translation = (0..content.texture_names.len() as u16).collect();
        let flat_translation = (0..content.flat_names.len() as u16).collect();
        // DEHACKED results the playsim reads from the world.
        let (weaponinfo, deh) = (info.weaponinfo, info.misc);
        let mut w = World {
            info,
            content,
            spec,
            texture_translation,
            flat_translation,
            buttons: Default::default(),
            activeceilings: [None; crate::specials::ceilings::MAXCEILINGS],
            activeplats: [None; crate::specials::plats::MAXPLATS],
            line_specials: Vec::new(),
            timelimit: 0,
            level_timer: false,
            level_time_count: 0,
            gameexit: None,
            sky_texture,
            weaponinfo,
            deh,
            gametic: 0,
            bulletslope: doom_core::Fixed(0),
            corpsehit: None,
            braintargets: Vec::new(),
            braintargeton: 0,
            brain_easy: false,
            params,
            level,
            thinkers: Thinkers::new(),
            rng: DoomRandom::new(), // M_ClearRandom
            players: Default::default(),
            playeringame,
            playerstarts: [None; MAXPLAYERS],
            deathmatchstarts: Vec::new(),
            leveltime: 0,
            sky_flat: crate::ids::FlatNum::MISSING,
            totalkills: 0,
            totalitems: 0,
            totalsecret: 0,
            onground: false,
            tm: MapGlobals { opening: Opening::default(), ..Default::default() },
            itemrespawnque: [Default::default(); ITEMQUESIZE],
            itemrespawntime: [0; ITEMQUESIZE],
            iquehead: 0,
            iquetail: 0,
            events: Vec::new(),
            unimplemented: BTreeMap::new(),
            overflows: BTreeMap::new(),
            load_warnings: Vec::new(),
            acs: None,
            acs_globals: Default::default(),
            acs_requests: Vec::new(),
            script_events: Vec::new(),
            exit_target: None,
            script_rng: 0x1234_5678,
            skate: Default::default(),
        };
        // Force players to be initialised upon first level load.
        for p in &mut w.players {
            p.playerstate = PlayerState::Reborn;
        }
        let level = std::mem::replace(&mut w.level, Level::empty());
        w.setup_level(level)?;
        Ok(w)
    }

    /// `G_DoLoadLevel` + `P_SetupLevel` (from `P_InitThinkers` on): install a
    /// freshly loaded level and spawn its things.
    pub fn setup_level(&mut self, level: Level) -> Result<(), String> {
        // G_DoLoadLevel
        for i in 0..MAXPLAYERS {
            if self.playeringame[i] && self.players[i].playerstate == PlayerState::Dead {
                self.players[i].playerstate = PlayerState::Reborn;
            }
            self.players[i].frags = [0; MAXPLAYERS];
        }
        // The "Sky never changes in Doom II" bug was fixed in the id
        // Anthology version of doom2.exe for Final Doom.
        if self.params.mode == GameMode::Commercial
            && matches!(self.params.version, doom_content::game::GameVersion::Final2 | doom_content::game::GameVersion::Chex)
        {
            let name = sky_texture_name(self.params.mode, self.params.episode, self.params.map);
            self.sky_texture = crate::ids::TexNum(self.content.texture_num(name).unwrap_or(0) as u16);
        }
        // P_SetupLevel
        self.totalkills = 0;
        self.totalitems = 0;
        self.totalsecret = 0;
        for p in &mut self.players {
            p.killcount = 0;
            p.secretcount = 0;
            p.itemcount = 0;
            p.mo = None;
        }
        // Initial height of PointOfView will be set by player think.
        self.players[self.params.consoleplayer].viewz = doom_core::Fixed(1);
        self.thinkers = Thinkers::new();
        self.level = level;
        // The grind graph belongs to the old level.
        self.skate.grind = None;
        self.load_warnings.clear();
        self.leveltime = 0;
        self.tm = MapGlobals::default();
        self.sky_flat = self.level.sky_flat;
        self.deathmatchstarts.clear();
        self.gameexit = None;
        // `P_InitPicAnims` translations persist across levels in vanilla
        // (they are recomputed every tic by P_UpdateSpecials anyway).
        self.playerstarts = [None; MAXPLAYERS];

        // P_LoadThings
        let things = self.level.things.clone();
        for mt in &things {
            // Do not spawn cool, new monsters if !commercial. Vanilla quirk:
            // this *stops loading all remaining things*, it does not skip.
            if self.params.mode != GameMode::Commercial && is_commercial_only(mt.kind) {
                break;
            }
            if let Err(e) = crate::mobj::spawn_map_thing(self, mt) {
                // Vanilla: I_Error. Mods routinely place things this engine
                // does not define (ZScript actors...): skip them.
                self.load_warnings.push(e);
            }
        }
        if self.params.deathmatch == 0 {
            for i in 0..MAXPLAYERS {
                if self.playeringame[i] && self.playerstarts[i].is_none() {
                    return Err(format!("P_LoadThings: Player {} start missing (vanilla crashes here)", i + 1));
                }
            }
        } else {
            // TODO(phase 11): G_DeathMatchSpawnPlayer.
            self.note_unimplemented("G_DeathMatchSpawnPlayer");
        }

        // Clear special respawning queue.
        self.iquehead = 0;
        self.iquetail = 0;
        // Set up world state.
        crate::spec::spawn_specials(self);
        // Scripts: OPEN, then ENTER for each player.
        crate::acs_host::start_level(self);
        Ok(())
    }

    /// One `G_Ticker` step in `GS_LEVEL`: copy commands into players, handle
    /// reborns and run the playsim. `cmds[i]` is ignored for players not in
    /// the game.
    pub fn tick(&mut self, cmds: &[TicCmd; MAXPLAYERS]) -> Option<GameAction> {
        // `ga_completed` set during the previous tic is handled first.
        if let Some(exit) = self.gameexit.take() {
            return Some(GameAction::Completed(exit));
        }
        // Do player reborns if needed.
        for i in 0..MAXPLAYERS {
            if self.playeringame[i] && self.players[i].playerstate == PlayerState::Reborn {
                if !self.params.netgame {
                    // `G_DoReborn` in single player: reload the level.
                    return Some(GameAction::LoadLevel);
                }
                // TODO(phase 11): netgame/deathmatch respawn (G_CheckSpot).
                self.note_unimplemented("G_DoReborn (netgame)");
            }
        }
        for i in 0..MAXPLAYERS {
            if self.playeringame[i] {
                self.players[i].cmd = cmds[i];
            }
        }
        // Special buttons (pause / save) are host-level actions; they are
        // stripped here exactly where P_PlayerThink would ignore them.
        for i in 0..MAXPLAYERS {
            if self.playeringame[i] && self.players[i].cmd.buttons & bt::SPECIAL != 0 {
                self.note_unimplemented("BT_SPECIAL (pause/save) handling");
            }
        }
        crate::tick::p_ticker(self);
        // D_RunTic increments gametic after G_Ticker.
        self.gametic = self.gametic.wrapping_add(1);
        None
    }

    /// `G_DoCompleted` + `G_DoWorldDone`: finish the current level for every
    /// player (`G_PlayerFinishLevel`) and start `episode`/`map` with the
    /// players' health, weapons and ammo carried over. The host chooses the
    /// next map and loads its [`Level`].
    ///
    /// Deliberate deviation: the sky texture is recomputed for the new map in
    /// every game mode. Vanilla Doom II keeps the first sky until a new game
    /// ("sky never changes" bug); that only affects visuals.
    pub fn next_level(&mut self, episode: i32, map: i32, level: Level) -> Result<(), String> {
        for i in 0..MAXPLAYERS {
            if !self.playeringame[i] {
                continue;
            }
            // G_PlayerFinishLevel
            let p = &mut self.players[i];
            p.powers = Default::default();
            p.cards = Default::default();
            p.extralight = 0; // cancel gun flashes
            p.fixedcolormap = 0; // cancel ir goggles
            p.damagecount = 0; // no palette changes
            p.bonuscount = 0;
            if let Some(mo) = p.mo {
                if let Some(m) = self.thinkers.get_mobj_mut(mo) {
                    m.flags &= !crate::mobj::flags::MF_SHADOW; // cancel invisibility
                }
            }
        }
        self.params.episode = episode;
        self.params.map = map;
        let name = sky_texture_name(self.params.mode, episode, map);
        self.sky_texture = crate::ids::TexNum(self.content.texture_num(name).unwrap_or(0) as u16);
        self.setup_level(level)
    }

    /// Lossy summary for diagnostics and cross-implementation comparison.
    pub fn console_player(&self) -> &Player {
        &self.players[self.params.consoleplayer]
    }
}
