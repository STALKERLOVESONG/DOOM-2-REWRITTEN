//! Mod weapons (DECORATE `Weapon` classes) in the player's hands, on E1M1.
//! Skipped without an IWAD.

use std::path::PathBuf;

use doom_content::game::{identify, GameVersion};
use doom_content::texture::TextureSet;
use doom_content::ResourceStack;
use doom_sim::content::ContentInfo;
use doom_sim::info::Info;
use doom_sim::level::Level;
use doom_sim::player::MAXPLAYERS;
use doom_sim::pspr::PS_FLASH;
use doom_sim::ticcmd::{bt, TicCmd};
use doom_sim::{GameParams, Skill, World};

/// A rifle weapon in the style of common weapon mods (one bullet per shot,
/// clips, slot 4, flash through the base class's `LightDone`).
const XM15: &str = r#"
ACTOR XM15 : weapon
{
	Tag "Rifle"
	Radius 20
	Height 16
	Obituary "%o was shot."
	Weapon.AmmoUse 1
	Weapon.AmmoGive 50
	Weapon.AmmoType "Clip"
	Weapon.Slotnumber 4
	Inventory.Pickupmessage "Picked up a rifle."
	States
	{
	Spawn:
		PIST A -1
		Stop
	Ready:
		X15A A 1 A_WeaponReady
		Loop
	Deselect:
		X15A A 1 A_Lower
		Loop
	Select:
		X15A A 1 A_Raise
		Loop
	Fire:
		X15A A 2
		TNT1 A 0 A_Playsound ("AR15_FP4")
		X15A B 2 A_FireBullets (1, 1, -1, 13, "BulletPuff")
		X15A CD 2
		X15A A 2 A_ReFire
		Goto Ready
	Flash:
		TNT1 A 7 Bright A_Light1
		Goto LightDone
		TNT1 A 7 Bright A_Light1
		Goto LightDone
	}
}
"#;

fn world(decorate: &str) -> Option<World> {
    let iwad = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../WADS/doom1.wad");
    if !iwad.exists() {
        return None;
    }
    let stack = ResourceStack::load(&iwad, &[]).unwrap();
    let textures = TextureSet::load(&stack).unwrap();
    let game = identify(&stack, None).unwrap();
    let mut info = Info::doom();
    let warnings = doom_sim::actors::load_text(&mut info, decorate, "");
    assert!(warnings.iter().all(|w| !w.contains("unknown label") && !w.contains("unknown parent")), "{warnings:?}");
    let level = Level::load(&stack, &textures, "E1M1").unwrap();
    let params = GameParams {
        mode: game.mode,
        version: GameVersion::Ultimate,
        skill: Skill::Medium,
        episode: 1,
        map: 1,
        deathmatch: 0,
        netgame: false,
        respawnmonsters: false,
        nomonsters: true,
        fastparm: false,
        consoleplayer: 0,
    };
    let mut pig = [false; MAXPLAYERS];
    pig[0] = true;
    Some(World::new_game(info, ContentInfo::from_content(&stack, &textures), params, pig, level).unwrap())
}

fn tic(w: &mut World, buttons: u8, n: usize) {
    for _ in 0..n {
        let mut cmds = [TicCmd::default(); MAXPLAYERS];
        cmds[0].buttons = buttons;
        w.tick(&cmds);
    }
}

#[test]
fn decorate_weapon_is_given_selected_and_fires() {
    let Some(mut w) = world(XM15) else { return };
    let xm15 = w.info.type_by_name("xm15").unwrap();
    let def = w.info.weapons.get(&xm15.0).cloned().unwrap();
    assert_eq!((def.slot, def.ammo_use, def.ammo_give), (4, 1, 50));
    // IDKFA-style: every slotted mod weapon.
    doom_sim::weapons::give_all(&mut w, 0);
    assert_eq!(w.players[0].class_weapons, [xm15]);
    tic(&mut w, 0, 20);
    // Slot key 4: chaingun isn't owned, so the XM15.
    tic(&mut w, bt::CHANGE | (3 << bt::WEAPONSHIFT), 1);
    tic(&mut w, 0, 40);
    assert_eq!(w.players[0].ready_class, Some(xm15), "lowered the pistol, raised the XM15");
    let ready = w.info.find_label(xm15, "ready").unwrap();
    assert_eq!(w.players[0].psprites[0].state, Some(ready));
    // Fire: one bullet per shot from the clip.
    let clips = w.players[0].ammo[0];
    tic(&mut w, bt::ATTACK, 10);
    assert!(w.players[0].ammo[0] < clips, "fired: {} -> {}", clips, w.players[0].ammo[0]);
    tic(&mut w, 0, 20);
    assert_eq!(w.players[0].psprites[PS_FLASH].state, None, "flash ended through LightDone");
    assert_eq!(w.players[0].extralight, 0, "LightDone reset the light");
    // Slot 2 goes back to the pistol.
    tic(&mut w, bt::CHANGE | (1 << bt::WEAPONSHIFT), 1);
    tic(&mut w, 0, 40);
    assert_eq!(w.players[0].ready_class, None);
    assert_eq!(w.players[0].readyweapon, doom_sim::player::Weapon::Pistol);
    assert!(w.unimplemented.is_empty(), "{:?}", w.unimplemented);
}

#[test]
fn weapon_pickup_gives_ammo_and_switches() {
    let decorate = format!("{XM15}\nactor XM15Pickup : XM15 30123 {{}}");
    let Some(mut w) = world(&decorate) else { return };
    let t = w.info.type_for_doomednum(30123).unwrap();
    let pmo = w.players[0].mo.unwrap();
    // In front of the player, who walks onto it.
    let (x, y, an) = (w.mo(pmo).x, w.mo(pmo).y, w.mo(pmo).angle);
    let ahead = |v: doom_core::Fixed| v * 48;
    let (x, y) = (x + ahead(doom_core::tables::cosine(an)), y + ahead(doom_core::tables::sine(an)));
    let z = w.mo(pmo).z;
    let clips = w.players[0].ammo[0];
    doom_sim::hexen::spawn_at(&mut w, t, x, y, z, doom_core::Angle(0), 0, false);
    for _ in 0..12 {
        let mut cmds = [TicCmd::default(); MAXPLAYERS];
        cmds[0].forwardmove = 25;
        w.tick(&cmds);
    }
    tic(&mut w, 0, 45);
    assert_eq!(w.players[0].class_weapons, [t]);
    assert_eq!(w.players[0].ammo[0], (clips + 50).min(200));
    assert_eq!(w.players[0].ready_class, Some(t), "switched to the new weapon");
}
