//! Tic commands (d_ticcmd.h) and their construction from input
//! (`G_BuildTiccmd`).

use doom_core::fixed::fixed_mul;
use doom_core::Fixed;

/// One player's input for one tic. Field types match `ticcmd_t`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct TicCmd {
    /// *2048 for move.
    pub forwardmove: i8,
    /// *2048 for move.
    pub sidemove: i8,
    /// <<16 for angle delta.
    pub angleturn: i16,
    pub chatchar: u8,
    pub buttons: u8,
    pub consistancy: u8,
    /// Buttons vanilla has no bits for ([`ext`]). Never stored in demos,
    /// so vanilla demo playback leaves it 0.
    pub ext: u8,
    /// Weapon slot key pressed this tic: `Some(0..=9)` is the slot of that
    /// number (slot 0 is the key after 9), `None` no selection. Vanilla's
    /// 3-bit `bt::CHANGE` field can only say "weapon index 0..7"; demos use
    /// that, live input uses this.
    pub weapon_slot: Option<u8>,
}

/// Number of weapon slots (keys 1-9 and 0).
pub const NUM_WEAPON_SLOTS: u8 = 10;

/// [`TicCmd::ext`] bits.
pub mod ext {
    /// Jump / ollie (held).
    pub const JUMP: u8 = 1;
    /// Secondary fire (held).
    pub const ALTATTACK: u8 = 2;
}

/// `buttoncode_t`.
pub mod bt {
    pub const ATTACK: u8 = 1;
    pub const USE: u8 = 2;
    /// Flag: game events, not really buttons.
    pub const SPECIAL: u8 = 128;
    pub const SPECIALMASK: u8 = 3;
    /// Flag, weapon change pending. If true, the next 3 bits hold the weapon.
    pub const CHANGE: u8 = 4;
    pub const WEAPONMASK: u8 = 8 + 16 + 32;
    pub const WEAPONSHIFT: u8 = 3;
    /// Pause the game.
    pub const BTS_PAUSE: u8 = 1;
    /// Save the game at each console.
    pub const BTS_SAVEGAME: u8 = 2;
    pub const BTS_SAVEMASK: u8 = 4 + 8 + 16;
    pub const BTS_SAVESHIFT: u8 = 2;
}

pub const FORWARDMOVE: [i32; 2] = [0x19, 0x32];
pub const SIDEMOVE: [i32; 2] = [0x18, 0x28];
/// Normal, fast, and slow (first `SLOWTURNTICS` of a keyboard turn).
pub const ANGLETURN: [i32; 3] = [640, 1280, 320];
pub const SLOWTURNTICS: i32 = 6;
pub const MAXPLMOVE: i32 = FORWARDMOVE[1];

/// Abstract input for one tic, already mapped from the host's devices.
/// Mouse deltas are in Doom units *after* sensitivity scaling (vanilla:
/// `ev->data2 * (mouseSensitivity + 5) / 10`, accumulated per tic).
#[derive(Clone, Copy, Debug, Default)]
pub struct InputState {
    pub forward: bool,
    pub backward: bool,
    pub turn_left: bool,
    pub turn_right: bool,
    pub strafe_left: bool,
    pub strafe_right: bool,
    /// Strafe modifier (turn keys strafe instead).
    pub strafe: bool,
    /// Run.
    pub speed: bool,
    pub fire: bool,
    /// Secondary fire. Not a vanilla button.
    pub alt_fire: bool,
    pub use_: bool,
    /// Jump (the skateboard's ollie). Not a vanilla button.
    pub jump: bool,
    /// Weapon slot key pressed this tic: the slot number, 0..=9 (keys 1-9
    /// are slots 1-9, key 0 is slot 0).
    pub weapon_slot: Option<u8>,
    pub mouse_x: i32,
    pub mouse_y: i32,
    /// Analog joystick axes, `FRACUNIT` = full deflection (Chocolate's
    /// `use_analog` path); 0 when unused.
    pub joy_turn: Fixed,
    pub joy_move: Fixed,
    pub joy_strafe: Fixed,
}

/// Persistent state of `G_BuildTiccmd` across tics.
#[derive(Clone, Copy, Debug, Default)]
pub struct TicCmdBuilder {
    turnheld: i32,
    /// Chocolate's `lowres_turn` carry (for recording vanilla demos).
    lowres_carry: i16,
    /// Round turning to 256 units as vanilla demos store it.
    pub lowres_turn: bool,
}

impl TicCmdBuilder {
    /// `G_BuildTiccmd` (single tic, `ticdup` = 1). Keyboard, mouse and
    /// analog-joystick paths; double-click-use, chat and special buttons are
    /// left to the caller.
    pub fn build(&mut self, inp: &InputState) -> TicCmd {
        let mut cmd = TicCmd::default();
        let strafe = inp.strafe;
        let speed = inp.speed as usize;
        let (mut forward, mut side) = (0i32, 0i32);
        let mut angleturn = 0i32;

        if inp.turn_left || inp.turn_right || inp.joy_turn.0 != 0 {
            self.turnheld += 1;
        } else {
            self.turnheld = 0;
        }
        let tspeed = if self.turnheld < SLOWTURNTICS { 2 } else { speed };

        if strafe {
            if inp.turn_right {
                side += SIDEMOVE[speed];
            }
            if inp.turn_left {
                side -= SIDEMOVE[speed];
            }
            if inp.joy_turn.0 != 0 {
                let j = inp.joy_turn.0.clamp(-doom_core::FRACUNIT, doom_core::FRACUNIT);
                side += fixed_mul(Fixed(SIDEMOVE[speed]), Fixed(j)).0;
            }
        } else {
            if inp.turn_right {
                angleturn -= ANGLETURN[tspeed];
            }
            if inp.turn_left {
                angleturn += ANGLETURN[tspeed];
            }
            if inp.joy_turn.0 != 0 {
                // Cubic response curve, as in Chocolate Doom.
                let j = inp.joy_turn;
                let j = fixed_mul(fixed_mul(j, j), j);
                angleturn -= fixed_mul(Fixed(ANGLETURN[1]), j).0;
            }
        }

        if inp.forward {
            forward += FORWARDMOVE[speed];
        }
        if inp.backward {
            forward -= FORWARDMOVE[speed];
        }
        if inp.joy_move.0 != 0 {
            let j = inp.joy_move.0.clamp(-doom_core::FRACUNIT, doom_core::FRACUNIT);
            forward -= fixed_mul(Fixed(FORWARDMOVE[speed]), Fixed(j)).0;
        }
        if inp.strafe_left {
            side -= SIDEMOVE[speed];
        }
        if inp.strafe_right {
            side += SIDEMOVE[speed];
        }
        if inp.joy_strafe.0 != 0 {
            let j = inp.joy_strafe.0.clamp(-doom_core::FRACUNIT, doom_core::FRACUNIT);
            side += fixed_mul(Fixed(SIDEMOVE[speed]), Fixed(j)).0;
        }

        if inp.fire {
            cmd.buttons |= bt::ATTACK;
        }
        if inp.use_ {
            cmd.buttons |= bt::USE;
        }
        if inp.jump {
            cmd.ext |= ext::JUMP;
        }
        if inp.alt_fire {
            cmd.ext |= ext::ALTATTACK;
        }
        cmd.weapon_slot = inp.weapon_slot.filter(|&s| s < NUM_WEAPON_SLOTS);

        forward += inp.mouse_y;
        if strafe {
            side += inp.mouse_x * 2;
        } else {
            angleturn -= inp.mouse_x * 0x8;
        }

        forward = forward.clamp(-MAXPLMOVE, MAXPLMOVE);
        side = side.clamp(-MAXPLMOVE, MAXPLMOVE);
        cmd.forwardmove = forward as i8;
        cmd.sidemove = side as i8;
        // `cmd->angleturn` is a short; the C arithmetic truncates.
        cmd.angleturn = angleturn as i16;

        if self.lowres_turn {
            let desired = cmd.angleturn.wrapping_add(self.lowres_carry);
            cmd.angleturn = (desired.wrapping_add(128) as u16 & 0xff00) as i16;
            self.lowres_carry = desired.wrapping_sub(cmd.angleturn);
        }
        cmd
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slow_turn_then_fast() {
        let mut b = TicCmdBuilder::default();
        let inp = InputState { turn_left: true, ..Default::default() };
        for _ in 0..5 {
            assert_eq!(b.build(&inp).angleturn, 320);
        }
        // turnheld reaches SLOWTURNTICS on the 6th tic.
        assert_eq!(b.build(&inp).angleturn, 640);
    }

    #[test]
    fn all_ten_weapon_slots_are_distinct() {
        let mut b = TicCmdBuilder::default();
        assert_eq!(b.build(&InputState::default()).weapon_slot, None);
        let slots: Vec<Option<u8>> = (0..10).map(|s| b.build(&InputState { weapon_slot: Some(s), ..Default::default() }).weapon_slot).collect();
        assert_eq!(slots, (0..10).map(Some).collect::<Vec<_>>());
        // No vanilla change bits: those are only for demos.
        assert_eq!(b.build(&InputState { weapon_slot: Some(9), ..Default::default() }).buttons & bt::CHANGE, 0);
        assert_eq!(b.build(&InputState { weapon_slot: Some(10), ..Default::default() }).weapon_slot, None);
        assert_eq!(b.build(&InputState { alt_fire: true, ..Default::default() }).ext, ext::ALTATTACK);
    }

    #[test]
    fn run_and_clamp() {
        let mut b = TicCmdBuilder::default();
        let inp = InputState { forward: true, speed: true, mouse_y: 100, ..Default::default() };
        assert_eq!(b.build(&inp).forwardmove, 50);
    }
}
