//! A weapon reskin as a mod ships it: a PWAD (built here) with replacement
//! sprites and sounds, SNDINFO, and a DECORATE class replacing Doom's
//! pistol. Checks resource precedence (the PWAD's lumps win over the
//! IWAD's) and the reskin's behaviour in a running game, plus weapon slot
//! selection from the console. Skipped when the IWAD is absent.

use std::path::PathBuf;

use doom_game::{InputState, Session, SessionConfig};
use doom_sim::player::Weapon;
use doom_sim::world::SimEvent;

fn iwad() -> Option<PathBuf> {
    let p = std::env::var_os("DOOM_IWAD")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../WADS/doom1.wad"));
    p.exists().then_some(p).or_else(|| {
        eprintln!("skipping: no IWAD");
        None
    })
}

/// A WAD file from (name, data) lumps.
fn wad(lumps: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut data = Vec::new();
    let mut dir = Vec::new();
    for (name, bytes) in lumps {
        let pos = 12 + data.len();
        data.extend_from_slice(bytes);
        let mut n = [0u8; 8];
        n[..name.len()].copy_from_slice(name.as_bytes());
        dir.extend_from_slice(&(pos as i32).to_le_bytes());
        dir.extend_from_slice(&(bytes.len() as i32).to_le_bytes());
        dir.extend_from_slice(&n);
    }
    let mut out = b"PWAD".to_vec();
    out.extend_from_slice(&(lumps.len() as i32).to_le_bytes());
    out.extend_from_slice(&((12 + data.len()) as i32).to_le_bytes());
    out.extend_from_slice(&data);
    out.extend_from_slice(&dir);
    out
}

/// A solid `w`x`h` Doom picture of one colour.
fn picture(w: u16, h: u8, color: u8) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&w.to_le_bytes());
    out.extend_from_slice(&(h as u16).to_le_bytes());
    out.extend_from_slice(&((w / 2) as i16).to_le_bytes());
    out.extend_from_slice(&((h as i16) - 2).to_le_bytes());
    let header = 8 + 4 * w as usize;
    let col_len = 5 + h as usize;
    for c in 0..w as usize {
        out.extend_from_slice(&((header + c * col_len) as u32).to_le_bytes());
    }
    for _ in 0..w {
        out.extend_from_slice(&[0, h, 0]);
        out.extend(std::iter::repeat_n(color, h as usize));
        out.extend_from_slice(&[0, 0xff]);
    }
    out
}

/// A DMX sound lump (8-bit, 11025 Hz) of `n` samples.
fn sound(n: usize, level: u8) -> Vec<u8> {
    let mut out = vec![3, 0];
    out.extend_from_slice(&11025u16.to_le_bytes());
    out.extend_from_slice(&((n + 32) as u32).to_le_bytes());
    out.extend(std::iter::repeat_n(128u8, 16));
    out.extend((0..n).map(|i| if i % 20 < 10 { level } else { 255 - level }));
    out.extend(std::iter::repeat_n(128u8, 16));
    out
}

const DECORATE: &str = r#"
ACTOR ReskinPuff : BulletPuff
{
	+PUFFGETSOWNER
	States
	{
	Spawn:
	Crash:
		RPUF A 3 Bright
		RPUF A 3 A_GiveToTarget("ReskinPuffRan", 1)
		Stop
	}
}

ACTOR ReskinPistol : Pistol replaces Pistol
{
	AttackSound "weapons/reskinpistol"
	States
	{
	Fire:
		RPST B 4 A_FireBullets(0, 0, 1, 5, "ReskinPuff", FBF_USEAMMO)
		RPST A 6 A_ReFire
		Goto Ready
	Flash:
		RPST C 4 Bright A_Light1
		Goto LightDone
	}
}
"#;

/// A 16-bit mono WAV of `n` samples.
fn wav16(n: usize) -> Vec<u8> {
    let pcm: Vec<u8> = (0..n).flat_map(|i| (((i % 40) as i16 - 20) * 800).to_le_bytes()).collect();
    let mut out = b"RIFF".to_vec();
    out.extend_from_slice(&((36 + pcm.len()) as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&22050u32.to_le_bytes());
    out.extend_from_slice(&44100u32.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    out.extend_from_slice(&pcm);
    out
}

/// A mod weapon's state calls a logical sound defined in SNDINFO, whose
/// resource is a WAV lump (not DMX, not named DS...): firing must reach the
/// mixer with that WAV's decoded PCM.
#[test]
fn mod_weapon_wav_sound_reaches_the_mixer() {
    // Doom II (MAP01); skipped without it.
    let iwad = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../WADS/doom2.wad");
    if !iwad.exists() {
        eprintln!("skipping: no doom2.wad");
        return;
    }
    let decorate = r#"
ACTOR SoundGun : Weapon
{
	Weapon.SlotNumber 5
	States
	{
	Ready:
		PISG A 1 A_WeaponReady
		Loop
	Deselect:
		PISG A 1 A_Lower
		Loop
	Select:
		PISG A 1 A_Raise
		Loop
	Fire:
		PISG B 2 A_PlaySound("test/gunfire", CHAN_WEAPON)
		PISG A 8
		Goto Ready
	}
}
"#;
    let wav = wav16(2205);
    let bytes = wad(&[
        ("DECORATE", decorate.as_bytes().to_vec()),
        // An alias to a long-named logical sound, as mods write them.
        ("SNDINFO", b"test/gunfire_real GUNFIRE1\n$alias test/gunfire test/gunfire_real\n".to_vec()),
        ("GUNFIRE1", wav),
    ]);
    let dir = std::env::temp_dir().join(format!("doom_bevy_wavgun_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let pwad = dir.join("wavgun.wad");
    std::fs::write(&pwad, &bytes).unwrap();
    let mut cfg = SessionConfig::new(iwad);
    cfg.pwads = vec![pwad];
    cfg.map = Some("MAP01".into());
    cfg.nomonsters = true;
    let mut s = Session::new(&cfg).unwrap();
    let sfx = s.world.info.sound("test/gunfire").expect("alias resolves");
    assert_eq!(s.world.info.sound_defs[&sfx.0].lump, s.stack.find_sound("GUNFIRE1"));
    s.exec("give SoundGun; slot 5").unwrap();
    let idle = InputState::default();
    for _ in 0..80 {
        s.tick(&idle);
    }
    s.take_audio_commands();
    s.tick(&InputState { fire: true, ..Default::default() });
    let cmds = s.take_audio_commands();
    let started = cmds.iter().find_map(|c| match c {
        doom_audio::AudioCommand::StartSound { sound, .. } => Some(sound.clone()),
        _ => None,
    });
    let sound = started.expect("the mixer was told to play the sound");
    assert_eq!((sound.rate, sound.channels, sound.frames()), (22050, 1, 2205), "the WAV, decoded");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pwad_reskin_overrides_the_iwad_weapon() {
    let Some(iwad) = iwad() else { return };
    let pisga0 = picture(7, 9, 176);
    let dspistol = sound(400, 200);
    let dsrskpst = sound(300, 220);
    let bytes = wad(&[
        ("DECORATE", DECORATE.as_bytes().to_vec()),
        ("SNDINFO", b"weapons/reskinpistol DSRSKPST\n".to_vec()),
        ("DSPISTOL", dspistol.clone()),
        ("DSRSKPST", dsrskpst.clone()),
        ("S_START", Vec::new()),
        ("PISGA0", pisga0.clone()),
        ("RPSTA0", picture(9, 12, 112)),
        ("RPSTB0", picture(9, 12, 113)),
        ("RPSTC0", picture(9, 12, 231)),
        ("RPUFA0", picture(4, 4, 231)),
        ("S_END", Vec::new()),
    ]);
    let dir = std::env::temp_dir().join(format!("doom_bevy_reskin_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let pwad = dir.join("reskin.wad");
    std::fs::write(&pwad, &bytes).unwrap();

    let mut cfg = SessionConfig::new(iwad);
    cfg.pwads = vec![pwad];
    cfg.map = Some("E1M1".into());
    cfg.nomonsters = true;
    let mut s = Session::new(&cfg).unwrap();
    assert!(!s.warnings.iter().any(|w| w.contains("DECORATE") || w.contains("not supported")), "{:?}", s.warnings);

    // Precedence: the PWAD's sprite and sound replace the IWAD's.
    let pisg = s.world.info.sprnames.iter().position(|n| n == "PISG").unwrap();
    let frame = s.render_data.sprites[pisg].frames[0];
    let lump = s.render_data.sprite_lumps[frame.lump[0] as usize].lump;
    assert_eq!(s.render_data.lump(lump), &pisga0[..], "PISGA0 comes from the PWAD");
    assert_eq!(s.stack.lump("DSPISTOL").unwrap(), &dspistol[..], "DSPISTOL comes from the PWAD");
    let rskn = s.world.info.sound("weapons/reskinpistol").expect("SNDINFO name");
    let def = &s.world.info.sound_defs[&rskn.0];
    assert_eq!(def.lump, s.stack.find_sound("DSRSKPST"), "resolved to the PWAD lump at load");

    // In the game: the starting pistol is the reskin.
    let idle = InputState::default();
    for _ in 0..30 {
        s.tick(&idle);
    }
    s.take_events();
    let rpst = s.world.info.sprnames.iter().position(|n| n == "RPST").unwrap();
    let clips = s.world.players[0].ammo[0];
    let fire = InputState { fire: true, ..Default::default() };
    s.tick(&fire);
    let p = &s.world.players[0];
    let st = s.world.info.state(p.psprites[0].state.unwrap());
    assert_eq!(st.sprite.0 as usize, rpst, "replacement fire frames");
    assert_eq!(p.ammo[0], clips - 1, "the pistol's ammo");
    let events = s.take_events();
    assert!(events.iter().any(|e| matches!(e, SimEvent::StartSound { sfx, .. } if *sfx == rskn)), "replacement attack sound");
    let puff = s.world.info.type_by_name("reskinpuff").unwrap();
    assert!(s.world.thinkers.mobjs().any(|(_, m)| m.mobj_type == puff), "replacement puff");
    assert!(!s.world.thinkers.mobjs().any(|(_, m)| m.mobj_type == doom_sim::info::MT_PUFF), "no vanilla puff");
    for _ in 0..10 {
        s.tick(&idle);
    }
    assert_eq!(s.world.players[0].inventory.get("reskinpuffran"), Some(&1), "puff states ran");

    // Weapon slots from the console go through the real input path.
    s.exec("give weapons").unwrap();
    s.exec("slot 3").unwrap();
    for _ in 0..70 {
        s.tick(&idle);
    }
    assert_eq!(s.world.players[0].readyweapon, Weapon::Shotgun);
    s.exec("slot 2").unwrap();
    for _ in 0..70 {
        s.tick(&idle);
    }
    assert_eq!(s.world.players[0].readyweapon, Weapon::Pistol);
    assert!(s.exec("slot 10").unwrap_err().contains("0-9"));
    let _ = std::fs::remove_dir_all(&dir);
}
