//! Mod weapon behaviour in the running game (E1M1): custom puff classes,
//! secondary fire, all ten weapon slots, ZScript weapon methods and
//! reskins of Doom's weapons. Every test fires real shots through the
//! psprite machinery and checks what happened in the world.
//! Skipped without an IWAD.

use std::path::PathBuf;

use doom_content::game::{identify, GameVersion};
use doom_content::texture::TextureSet;
use doom_content::ResourceStack;
use doom_sim::content::ContentInfo;
use doom_sim::info::{Info, MobjType};
use doom_sim::level::Level;
use doom_sim::player::{Weapon, MAXPLAYERS};
use doom_sim::ticcmd::{bt, ext, TicCmd};
use doom_sim::world::{SimEvent, SoundSource};
use doom_sim::{GameParams, Skill, World};

/// Two puff classes: tokens given to the shooter show their states ran.
const PUFFS: &str = r#"
ACTOR GreenPuff : BulletPuff
{
	+PUFFONACTORS
	+PUFFGETSOWNER
	VSpeed 2
	States
	{
	Spawn:
		PUFF A 2 Bright
		PUFF B 4 A_GiveToTarget("GreenHit", 1)
		Stop
	Crash:
		PUFF C 4 A_GiveToTarget("GreenWall", 1)
		PUFF D 4
		Stop
	}
}

ACTOR BluePuff : BulletPuff
{
	+PUFFGETSOWNER
	States
	{
	Spawn:
	Crash:
		PUFF A 4 A_GiveToTarget("BlueWall", 1)
		Stop
	}
}

ACTOR TestToken : Inventory {}
"#;

/// A rifle with primary fire on clips and a secondary burst on shells.
const RIFLE: &str = r#"
ACTOR TestRifle : Weapon
{
	Weapon.SlotNumber 4
	Weapon.SelectionOrder 650
	Weapon.AmmoType "Clip"
	Weapon.AmmoUse 1
	Weapon.AmmoGive 20
	Weapon.AmmoType2 "Shell"
	Weapon.AmmoUse2 2
	Weapon.AmmoGive2 8
	AttackSound "plasma"
	States
	{
	Spawn:
		PIST A -1
		Stop
	Ready:
		RIFL A 1 A_WeaponReady
		Loop
	Deselect:
		RIFL A 1 A_Lower
		Loop
	Select:
		RIFL A 1 A_Raise
		Loop
	Fire:
		RIFL B 3 A_FireBullets(0, 0, 1, 5, "GreenPuff", FBF_USEAMMO)
		RIFL A 3 A_ReFire
		Goto Ready
	AltFire:
		RIFL C 2 A_GiveInventory("TestToken", 1)
		RIFL D 4 A_FireBullets(0, 0, 1, 5, "BluePuff", FBF_USEAMMO)
		RIFL A 4 A_ReFire
		Goto Ready
	AltHold:
		RIFL E 4 A_FireBullets(0, 0, 1, 5, "BluePuff", FBF_USEAMMO)
		RIFL A 4 A_ReFire
		Goto Ready
	}
}
"#;

/// Weapons in slots 8, 9 and 0 (same states, different slots).
fn slot_weapon(name: &str, slot: i32) -> String {
    format!(
        r#"
ACTOR {name} : Weapon
{{
	Weapon.SlotNumber {slot}
	States
	{{
	Ready:
		SLOT A 1 A_WeaponReady
		Loop
	Deselect:
		SLOT A 1 A_Lower
		Loop
	Select:
		SLOT A 1 A_Raise
		Loop
	Fire:
		SLOT A 4
		Goto Ready
	}}
}}
"#
    )
}

/// A ZScript weapon whose fire state calls its own methods: an action
/// method with a loop, a helper returning a value, and a field on the
/// weapon (`invoker.shots`).
const BLASTER_ZS: &str = r#"
class TestBlaster : Weapon
{
	int shots;

	Default
	{
		Weapon.SlotNumber 8;
		Weapon.AmmoType "Cell";
		Weapon.AmmoUse 1;
		Weapon.AmmoGive 40;
	}

	int ShotDamage(int base)
	{
		return base * 2 + 1;
	}

	action void A_BlasterVolley(int count)
	{
		for (int i = 0; i < count; i++)
		{
			A_FireBullets(0, 0, 1, invoker.ShotDamage(3), "GreenPuff", FBF_USEAMMO);
		}
		invoker.shots += count;
		if (invoker.shots >= 4)
		{
			A_GiveInventory("BlasterMilestone", 1);
		}
	}

	States
	{
	Spawn:
		BLST A -1;
		Stop;
	Ready:
		BLST A 1 A_WeaponReady;
		Loop;
	Deselect:
		BLST A 1 A_Lower;
		Loop;
	Select:
		BLST A 1 A_Raise;
		Loop;
	Fire:
		BLST B 3 A_BlasterVolley(2);
		BLST A 6;
		Goto Ready;
	AltFire:
		BLST C 3
		{
			if (CheckAmmo(1, false, true)) { A_GiveInventory("AltHasAmmo", 1); }
			else { A_GiveInventory("AltDryFire", 1); }
		}
		Goto Ready;
	}
}
"#;

/// An ordinary reskin: Doom's pistol replaced by a class that inherits
/// from it, overriding only the Fire state, the attack sound and the puff.
const RESKIN: &str = r#"
ACTOR ReskinPistol : Pistol replaces Pistol
{
	AttackSound "plasma"
	States
	{
	Fire:
		RPST B 4 A_FireBullets(0, 0, 1, 5, "GreenPuff", FBF_USEAMMO)
		RPST A 6 A_ReFire
		Goto Ready
	}
}
"#;

fn world(decorate: &str, zscript: &str) -> Option<World> {
    let iwad = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../WADS/doom1.wad");
    if !iwad.exists() {
        return None;
    }
    let stack = ResourceStack::load(&iwad, &[]).unwrap();
    let textures = TextureSet::load(&stack).unwrap();
    let game = identify(&stack, None).unwrap();
    let mut info = Info::doom();
    let warnings = doom_sim::actors::load_text(&mut info, decorate, zscript);
    assert!(
        warnings.iter().all(|w| !w.contains("unknown label") && !w.contains("unknown parent") && !w.contains("not supported") && !w.contains("ZScript:")),
        "{warnings:?}"
    );
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

fn cmd(buttons: u8, ext_bits: u8, slot: Option<u8>) -> [TicCmd; MAXPLAYERS] {
    let mut cmds = [TicCmd::default(); MAXPLAYERS];
    cmds[0].buttons = buttons;
    cmds[0].ext = ext_bits;
    cmds[0].weapon_slot = slot;
    cmds
}

fn tic(w: &mut World, buttons: u8, ext_bits: u8, n: usize) {
    for _ in 0..n {
        w.tick(&cmd(buttons, ext_bits, None));
    }
}

/// Press a slot key and wait for the old weapon to lower and the new one
/// to come up.
fn select_slot(w: &mut World, slot: u8) {
    w.tick(&cmd(0, 0, Some(slot)));
    tic(w, 0, 0, 70);
}

fn count(w: &World, t: MobjType) -> usize {
    w.thinkers.mobjs().filter(|(_, m)| m.mobj_type == t).count()
}

fn inv(w: &World, item: &str) -> i32 {
    w.players[0].inventory.get(item).copied().unwrap_or(0)
}

fn give(w: &mut World, class: &str) -> MobjType {
    let t = w.info.type_by_name(class).unwrap();
    doom_sim::weapons::give_weapon(w, 0, t, false);
    t
}

/// Face the nearest solid wall, so shots hit a wall close by.
fn face_wall(w: &mut World) {
    let mo = w.players[0].mo.unwrap();
    w.mo_mut(mo).angle = doom_core::Angle(0x4000_0000); // north in E1M1's start room
}

#[test]
fn custom_puffs_spawn_and_run_their_states() {
    let Some(mut w) = world(&format!("{PUFFS}{RIFLE}"), "") else { return };
    let green = w.info.type_by_name("greenpuff").unwrap();
    let rifle = give(&mut w, "TestRifle");
    face_wall(&mut w);
    tic(&mut w, 0, 0, 5);
    select_slot(&mut w, 4);
    assert_eq!(w.players[0].ready_class, Some(rifle));
    let vanilla_puffs = count(&w, doom_sim::info::MT_PUFF);

    // Primary fire: a GreenPuff, its Crash state (a wall) ran.
    tic(&mut w, bt::ATTACK, 0, 1);
    assert_eq!(count(&w, green), 1, "the weapon's puff class spawned");
    let puff = w.thinkers.mobjs().find(|(_, m)| m.mobj_type == green).map(|(id, _)| id).unwrap();
    assert_eq!(w.mo(puff).momz.0, 2 << 16, "VSpeed 2");
    assert_eq!(w.mo(puff).target, w.players[0].mo, "+PUFFGETSOWNER");
    assert_eq!(inv(&w, "greenwall"), 1, "Crash state action ran");
    assert_eq!(count(&w, doom_sim::info::MT_PUFF), vanilla_puffs, "no vanilla puff for this weapon");
    tic(&mut w, 0, 0, 20);

    // Secondary fire uses a different puff class at the same time.
    tic(&mut w, 0, ext::ALTATTACK, 4);
    assert!(inv(&w, "bluewall") >= 1, "BluePuff ran its states");
    tic(&mut w, 0, 0, 20);

    // Hitting a bleeding actor: +PUFFONACTORS puffs appear on it too (Spawn state).
    let mo = w.players[0].mo.unwrap();
    let (x, y, z) = (w.mo(mo).x, w.mo(mo).y, w.mo(mo).z);
    let imp = doom_sim::mobj::spawn_mobj(&mut w, x, y + doom_core::Fixed::from_int(64), z, doom_sim::info::MT_TROOP);
    let _ = imp;
    tic(&mut w, bt::ATTACK, 0, 1);
    tic(&mut w, 0, 0, 4);
    assert_eq!(inv(&w, "greenhit"), 1, "puff on the actor ran its Spawn states");
    assert!(w.unimplemented.is_empty(), "{:?}", w.unimplemented);
}

#[test]
fn altfire_is_its_own_attack_with_its_own_ammo() {
    let Some(mut w) = world(&format!("{PUFFS}{RIFLE}"), "") else { return };
    give(&mut w, "TestRifle");
    face_wall(&mut w);
    select_slot(&mut w, 4);
    let (clips, shells) = (w.players[0].ammo[0], w.players[0].ammo[1]);
    assert!(shells >= 8, "AmmoGive2 gave shells");

    // One secondary shot: AltFire states, two shells, no clips.
    tic(&mut w, 0, ext::ALTATTACK, 1);
    let altfire = w.info.find_label(w.players[0].ready_class.unwrap(), "altfire").unwrap();
    assert_eq!(w.players[0].psprites[0].state, Some(altfire), "entered AltFire");
    assert_eq!(w.players[0].fire_mode, doom_sim::weapons::FireMode::Alt);
    tic(&mut w, 0, 0, 3);
    assert_eq!(w.players[0].ammo[1], shells - 2, "AmmoUse2 from AmmoType2");
    assert_eq!(w.players[0].ammo[0], clips, "primary ammo untouched");
    assert_eq!(inv(&w, "testtoken"), 1, "AltFire action ran");
    // The attack sound is the weapon's own.
    let plasma = w.info.sound("plasma").unwrap();
    assert!(w.events.iter().any(|e| matches!(e, SimEvent::StartSound { sfx, origin: SoundSource::Mobj(_), .. } if *sfx == plasma)));
    tic(&mut w, 0, 0, 20);

    // Holding it refires through AltHold until the shells run out.
    w.players[0].ammo[1] = 4;
    let althold = w.info.find_label(w.players[0].ready_class.unwrap(), "althold").unwrap();
    let mut saw_hold = false;
    for _ in 0..60 {
        tic(&mut w, 0, ext::ALTATTACK, 1);
        saw_hold |= w.players[0].psprites[0].state == Some(althold);
    }
    assert!(saw_hold, "refired into AltHold");
    assert_eq!(w.players[0].ammo[1], 0, "fired until empty");
    assert!(inv(&w, "testtoken") == 2, "AltFire entered once per press");
    // Out of secondary ammo: the weapon stays up and primary still works.
    tic(&mut w, 0, 0, 20);
    let clips = w.players[0].ammo[0];
    tic(&mut w, bt::ATTACK, 0, 1);
    tic(&mut w, 0, 0, 10);
    assert_eq!(w.players[0].ammo[0], clips - 1);
    assert_eq!(w.players[0].fire_mode, doom_sim::weapons::FireMode::Primary);

    // Doom's weapons have no secondary fire: nothing happens.
    select_slot(&mut w, 2);
    assert_eq!(w.players[0].ready_class, None);
    let (clips, state) = (w.players[0].ammo[0], w.players[0].psprites[0].state);
    tic(&mut w, 0, ext::ALTATTACK, 5);
    assert_eq!(w.players[0].ammo[0], clips);
    assert_eq!(w.players[0].psprites[0].state.map(|s| w.info.state(s).sprite), state.map(|s| w.info.state(s).sprite));
}

#[test]
fn all_ten_weapon_slots_select() {
    let decorate = format!("{}{}{}", slot_weapon("SlotEight", 8), slot_weapon("SlotNine", 9), slot_weapon("SlotZero", 0));
    let Some(mut w) = world(&decorate, "") else { return };
    // Every vanilla weapon too (shareware: no plasma/BFG selection).
    w.players[0].weaponowned = [true; 9];
    w.players[0].ammo = w.players[0].maxammo;
    let eight = give(&mut w, "SlotEight");
    let nine = give(&mut w, "SlotNine");
    let zero = give(&mut w, "SlotZero");
    tic(&mut w, 0, 0, 10);

    select_slot(&mut w, 9);
    assert_eq!(w.players[0].ready_class, Some(nine), "slot 9");
    select_slot(&mut w, 0);
    assert_eq!(w.players[0].ready_class, Some(zero), "slot 0");
    select_slot(&mut w, 8);
    assert_eq!(w.players[0].ready_class, Some(eight), "slot 8");
    let shareware = w.params.mode == doom_content::game::GameMode::Shareware;
    for (slot, want) in [(1, Weapon::Chainsaw), (2, Weapon::Pistol), (3, Weapon::Shotgun), (4, Weapon::Chaingun), (5, Weapon::Missile), (6, Weapon::Plasma), (7, Weapon::Bfg)] {
        select_slot(&mut w, slot);
        if shareware && matches!(want, Weapon::Plasma | Weapon::Bfg) {
            continue;
        }
        assert_eq!((w.players[0].ready_class, w.players[0].readyweapon), (None, want), "slot {slot}");
    }
    // From a mod weapon, slot 1 follows Doom's rule too (chainsaw first).
    select_slot(&mut w, 0);
    select_slot(&mut w, 1);
    assert_eq!((w.players[0].ready_class, w.players[0].readyweapon), (None, Weapon::Chainsaw));
    // An empty slot does nothing.
    w.players[0].class_weapons.retain(|&t| t != nine);
    let before = (w.players[0].ready_class, w.players[0].readyweapon);
    select_slot(&mut w, 9);
    assert_eq!((w.players[0].ready_class, w.players[0].readyweapon), before);
}

#[test]
fn weapon_priority_on_empty() {
    let Some(mut w) = world(&format!("{PUFFS}{RIFLE}"), "") else { return };
    give(&mut w, "TestRifle");
    w.players[0].weaponowned[Weapon::Shotgun as usize] = true;
    w.players[0].weaponowned[Weapon::Chaingun as usize] = true;
    select_slot(&mut w, 4);
    select_slot(&mut w, 4);
    assert!(w.players[0].ready_class.is_some());
    // Out of clips: the best other weapon with ammo by selection order is
    // the shotgun (1300) over the pistol (1900); the chaingun has no clips.
    w.players[0].ammo[0] = 0;
    w.players[0].ammo[1] = 10;
    tic(&mut w, bt::ATTACK, 0, 1);
    tic(&mut w, 0, 0, 40);
    assert_eq!((w.players[0].ready_class, w.players[0].readyweapon), (None, Weapon::Shotgun));
}

#[test]
fn zscript_weapon_methods_execute() {
    let Some(mut w) = world(PUFFS, BLASTER_ZS) else { return };
    let blaster = w.info.type_by_name("testblaster").unwrap();
    assert!(w.info.find_method(blaster, "a_blastervolley").is_some());
    let green = w.info.type_by_name("greenpuff").unwrap();
    give(&mut w, "TestBlaster");
    face_wall(&mut w);
    select_slot(&mut w, 8);
    assert_eq!(w.players[0].ready_class, Some(blaster));
    let cells = w.players[0].ammo[2];
    assert_eq!(cells, 40);

    tic(&mut w, bt::ATTACK, 0, 1);
    // A_BlasterVolley(2): two bullets (loop), each its own puff and cell.
    assert_eq!(w.players[0].ammo[2], cells - 2, "the method's loop fired twice");
    assert_eq!(count(&w, green), 2);
    assert_eq!(w.players[0].weapon_vars.get("testblaster.shots").copied(), Some(2.0), "invoker field written");
    assert_eq!(inv(&w, "blastermilestone"), 0);
    tic(&mut w, 0, 0, 15);
    tic(&mut w, bt::ATTACK, 0, 1);
    assert_eq!(w.players[0].weapon_vars.get("testblaster.shots").copied(), Some(4.0));
    assert_eq!(inv(&w, "blastermilestone"), 1, "if on a field read back");

    // The helper's return value reached A_FireBullets: damage 3*2+1 = 7
    // per hit. A shootable target in front takes exactly 7 per bullet
    // (FBF_NORANDOM is not set, so the roll is 1-3x: 7, 14 or 21).
    let mo = w.players[0].mo.unwrap();
    let (x, y, z) = (w.mo(mo).x, w.mo(mo).y, w.mo(mo).z);
    let barrel = doom_sim::mobj::spawn_mobj(&mut w, x, y + doom_core::Fixed::from_int(48), z, doom_sim::info::MT_BARREL);
    w.mo_mut(barrel).health = 1000;
    tic(&mut w, 0, 0, 15);
    tic(&mut w, bt::ATTACK, 0, 1);
    let lost = 1000 - w.mo(barrel).health;
    assert!(lost > 0 && lost % 7 == 0, "damage from the method's value: {lost}");

    // Code block calling a native that returns a value (CheckAmmo).
    tic(&mut w, 0, 0, 15);
    tic(&mut w, 0, ext::ALTATTACK, 1);
    // No AmmoType2: the secondary attack needs no ammo, as in ZDoom.
    assert_eq!((inv(&w, "althasammo"), inv(&w, "altdryfire")), (1, 0), "CheckAmmo's value decided the branch");
    // Methods are callable from code too.
    assert_eq!(doom_sim::actions::call_method_on_class(&mut w, mo, blaster, "shotdamage", &[10.0]), Some(21.0));
    assert!(w.unimplemented.is_empty(), "{:?}", w.unimplemented);
}

#[test]
fn reskinned_pistol_uses_the_replacement() {
    let Some(mut w) = world(&format!("{PUFFS}{RESKIN}"), "") else { return };
    let green = w.info.type_by_name("greenpuff").unwrap();
    let pistol = w.info.vanilla_weapon_classes[Weapon::Pistol as usize];
    assert!(w.info.vanilla_weapon_replaced[Weapon::Pistol as usize]);
    face_wall(&mut w);
    tic(&mut w, 0, 0, 30);
    // The starting pistol is the reskin: Ready inherited from Pistol,
    // Fire from the replacement.
    assert_eq!(w.players[0].readyweapon, Weapon::Pistol);
    assert_eq!(doom_sim::weapons::active_class(&w, 0), Some(pistol));
    let ready = w.players[0].psprites[0].state.unwrap();
    assert_eq!(w.info.sprnames[w.info.state(ready).sprite.0 as usize], "PISG", "inherited Ready frames");
    let clips = w.players[0].ammo[0];
    tic(&mut w, bt::ATTACK, 0, 1);
    let fire = w.players[0].psprites[0].state.unwrap();
    assert_eq!(w.info.sprnames[w.info.state(fire).sprite.0 as usize], "RPST", "replacement Fire frames");
    assert_eq!(count(&w, green), 1, "replacement puff");
    assert_eq!(count(&w, doom_sim::info::MT_PUFF), 0, "not the vanilla puff");
    assert_eq!(w.players[0].ammo[0], clips - 1, "pistol ammo");
    let plasma = w.info.sound("plasma").unwrap();
    assert!(w.events.iter().any(|e| matches!(e, SimEvent::StartSound { sfx, .. } if *sfx == plasma)), "replacement sound");
    // Refire holds, ammo keeps being used.
    tic(&mut w, bt::ATTACK, 0, 30);
    assert!(w.players[0].ammo[0] < clips - 1);
}

#[test]
fn replacing_a_parent_class_does_not_loop() {
    // `X : ZombieMan replaces ZombieMan` must not make label lookups cycle.
    let decorate = "ACTOR LoopZombie : ZombieMan replaces ZombieMan { States { Death: POSS H 5 Stop } }";
    let Some(w) = world(decorate, "") else { return };
    let z = w.info.type_by_name("zombieman").unwrap();
    assert!(w.info.find_label(z, "no-such-label").is_none());
    assert!(w.info.find_label(z, "see").is_some(), "inherited label kept");
    assert!(w.info.find_label(z, "death").is_some());
}
