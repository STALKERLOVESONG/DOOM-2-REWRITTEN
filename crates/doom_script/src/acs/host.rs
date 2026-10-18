//! What ACS needs from the engine: the [`AcsHost`] trait.
//!
//! The VM computes, stores variables and schedules scripts itself; every
//! instruction that reads or changes the world becomes one
//! [`AcsHost::builtin`] call with a [`Builtin`] and its stack arguments in
//! source order, so the playsim stays the single owner of game state.

/// Who and what started a script: the activator (an engine actor handle,
/// opaque to the VM) and the line and side that triggered it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Activation {
    pub activator: Option<u32>,
    pub line: Option<u32>,
    /// 0 = front, 1 = back.
    pub side: u8,
}

/// Engine operations, named after their ACS functions. Arguments arrive as
/// pushed (first argument first). Strings are ACS string values; resolve
/// them with [`Strings::get`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Builtin {
    // Things.
    ThingCount,
    ThingCountName,
    ThingCountSector,
    ThingCountNameSector,
    Spawn,
    SpawnSpot,
    SpawnSpotFacing,
    SpawnProjectile,
    ThingProjectile2,
    SetThingSpecial,
    ThingDamage2,
    ActivatorTid,
    GetActorX,
    GetActorY,
    GetActorZ,
    GetActorFloorZ,
    GetActorCeilingZ,
    GetActorAngle,
    GetActorPitch,
    GetActorLightLevel,
    SetActorPosition,
    SetActorAngle,
    SetActorPitch,
    SetActorState,
    SetActorProperty,
    GetActorProperty,
    ClassifyActor,
    CheckActorFloorTexture,
    CheckActorCeilingTexture,
    MorphActor,
    UnmorphActor,
    // Inventory (of the activator / of actors by TID).
    ClearInventory,
    GiveInventory,
    TakeInventory,
    CheckInventory,
    UseInventory,
    ClearActorInventory,
    GiveActorInventory,
    TakeActorInventory,
    CheckActorInventory,
    UseActorInventory,
    CheckWeapon,
    SetWeapon,
    GetAmmoCapacity,
    SetAmmoCapacity,
    // Players.
    PlayerCount,
    PlayerNumber,
    PlayerInGame,
    PlayerIsBot,
    PlayerClass,
    PlayerHealth,
    PlayerArmorPoints,
    PlayerFrags,
    /// Key check; argument 0 is the key (0 blue skull, 1 red skull, 2
    /// yellow skull, 3 master skull, 4 blue card, 5 red card, 6 yellow
    /// card, 7 master card, 8 black skull, 9 silver skull, 10 gold skull,
    /// 11 black card, 12 silver card).
    PlayerKey,
    GetPlayerInput,
    GetPlayerInfo,
    // Game.
    GameType,
    GameSkill,
    SinglePlayer,
    IsNetworkGame,
    GetLevelInfo,
    GetCvar,
    ChangeLevel,
    // Map.
    ChangeFloor,
    ChangeCeiling,
    SetLineTexture,
    SetLineBlocking,
    SetLineMonsterBlocking,
    SetLineSpecial,
    ClearLineSpecial,
    GetLineRowOffset,
    GetSectorFloorZ,
    GetSectorCeilingZ,
    GetSectorLightLevel,
    SetFloorTrigger,
    SetCeilingTrigger,
    SectorDamage,
    ReplaceTextures,
    ChangeSky,
    SetGravity,
    SetAirControl,
    // Sound and music.
    SectorSound,
    AmbientSound,
    LocalAmbientSound,
    ThingSound,
    ActivatorSound,
    SoundSequence,
    SetMusic,
    LocalSetMusic,
    // Presentation.
    SetFont,
    SetHudSize,
    GetScreenWidth,
    GetScreenHeight,
    FadeTo,
    FadeRange,
    CancelFade,
    SetMugShotState,
    SetCameraToTexture,
    CheckPlayerCamera,
    StartTranslation,
    TranslationRange1,
    TranslationRange2,
    TranslationRange3,
    TranslationRange4,
    TranslationRange5,
    EndTranslation,
    /// `CallFunc` extension function `ACSF_*` by number.
    Func(u16),
}

/// How a finished print buffer is shown.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PrintKind {
    /// `Print`: the activator's screen.
    Print,
    /// `PrintBold`: everyone.
    Bold,
    /// `Log`: the console only.
    Log,
    /// `HudMessage` (bold: to everyone): type, id, colour, x, y (fixed),
    /// hold time (fixed seconds), then the type's extra arguments.
    HudMessage { bold: bool, args: [i32; 9], count: usize },
}

/// ACS string values: module string-table entries tagged with their module
/// (`module << 16 | index`), and strings made at run time.
#[derive(Debug, Default)]
pub struct Strings {
    pub(crate) tables: Vec<Vec<String>>,
    pub(crate) dynamic: Vec<String>,
}

/// Module tag used for strings made at run time.
pub const DYNAMIC_STRINGS: i32 = 0x7fff;

impl Strings {
    pub fn get(&self, v: i32) -> &str {
        let (lib, i) = ((v >> 16) & 0x7fff, (v & 0xffff) as usize);
        let s = if lib == DYNAMIC_STRINGS {
            self.dynamic.get(i)
        } else {
            self.tables.get(lib as usize).and_then(|t| t.get(i))
        };
        s.map_or("", String::as_str)
    }

    /// Make a string value (reusing an identical dynamic string).
    pub fn add(&mut self, s: String) -> i32 {
        let i = match self.dynamic.iter().position(|d| *d == s) {
            Some(i) => i,
            None => {
                self.dynamic.push(s);
                self.dynamic.len() - 1
            }
        };
        (DYNAMIC_STRINGS << 16) | (i as i32 & 0xffff)
    }
}

/// Where engine effects go. Values are ACS integers (fixed point where ACS
/// uses fixed point); actors are referred to by TID or the activator.
pub trait AcsHost {
    /// Run a line special (Hexen numbering) and return its result. Script
    /// control specials (80-85, 226) never reach the host.
    fn line_special(&mut self, act: &Activation, special: i32, args: [i32; 5], strings: &Strings) -> i32;

    /// Is a sector with this tag running a floor/ceiling/door mover
    /// (`TagWait`)?
    fn tag_busy(&self, tag: i32) -> bool;

    /// Is this polyobject moving (`PolyWait`)?
    fn poly_busy(&self, _po: i32) -> bool {
        false
    }

    /// `Random(min, max)`, inclusive, from the playsim's deterministic RNG.
    fn random(&mut self, min: i32, max: i32) -> i32;

    /// `Timer()`: tics since the level started.
    fn level_time(&self) -> i32;

    /// Any engine operation. The activator can be changed (`SetActivator`).
    fn builtin(&mut self, act: &mut Activation, f: Builtin, args: &[i32], strings: &mut Strings) -> i32;

    fn print(&mut self, act: &Activation, kind: PrintKind, text: &str);

    /// `PrintName(n)`: a player's name (0 = the activator).
    fn player_name(&self, _act: &Activation, _player: i32) -> String {
        "Player".to_string()
    }

    /// `l"TEXT"` / `PrintLocalized`: the LANGUAGE string for an id.
    fn localize(&self, id: &str) -> String {
        id.to_string()
    }

    /// Script starts requested by the engine while scripts were running
    /// (a monster's death special starting a script, ...).
    fn take_requests(&mut self) -> Vec<crate::acs::ScriptRequest> {
        Vec::new()
    }
}
