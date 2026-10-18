//! The thinker list (p_tick.c): an ordered, intrusive doubly linked list of
//! everything that runs each tic.
//!
//! Order is behaviour: thinkers run in insertion order, new thinkers are
//! appended, and removal only *marks* a thinker; it is unlinked and freed when
//! `P_RunThinkers` next reaches it. Code that holds a reference to a removed
//! thinker can therefore still read it until then, as in vanilla.

use crate::ids::ThinkerId;
use crate::mobj::Mobj;
use crate::specials::SpecialThinker;

/// `thinker->function` state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThinkFn {
    /// Runs its think routine every tic.
    Active,
    /// `function == NULL`: stays linked but does nothing (ceilings/plats "in
    /// stasis").
    Stasis,
    /// `function == -1`: marked for removal.
    Removed,
}

#[derive(Clone, Debug)]
pub enum ThinkerData {
    Free,
    Sentinel,
    Mobj(Box<Mobj>),
    Special(SpecialThinker),
}

#[derive(Clone, Debug)]
struct Slot {
    generation: u32,
    prev: u32,
    next: u32,
    func: ThinkFn,
    data: ThinkerData,
}

/// Arena + ordered list. Slot 0 is `thinkercap`.
#[derive(Clone, Debug)]
pub struct Thinkers {
    slots: Vec<Slot>,
    free: Vec<u32>,
}

const CAP: u32 = 0;

impl Default for Thinkers {
    fn default() -> Self {
        Self::new()
    }
}

impl Thinkers {
    /// `P_InitThinkers`.
    pub fn new() -> Self {
        Self {
            slots: vec![Slot { generation: 0, prev: CAP, next: CAP, func: ThinkFn::Stasis, data: ThinkerData::Sentinel }],
            free: Vec::new(),
        }
    }

    /// `P_AddThinker`: allocate and append to the end of the list.
    pub fn add(&mut self, data: ThinkerData, func: ThinkFn) -> ThinkerId {
        let index = match self.free.pop() {
            Some(i) => i,
            None => {
                self.slots.push(Slot { generation: 0, prev: CAP, next: CAP, func, data: ThinkerData::Free });
                (self.slots.len() - 1) as u32
            }
        };
        let last = self.slots[CAP as usize].prev;
        {
            let s = &mut self.slots[index as usize];
            s.prev = last;
            s.next = CAP;
            s.func = func;
            s.data = data;
        }
        self.slots[last as usize].next = index;
        self.slots[CAP as usize].prev = index;
        ThinkerId { index, generation: self.slots[index as usize].generation }
    }

    /// `P_RemoveThinker`: mark only.
    pub fn remove(&mut self, id: ThinkerId) {
        if let Some(s) = self.slot_mut(id) {
            s.func = ThinkFn::Removed;
        }
    }

    pub fn set_func(&mut self, id: ThinkerId, func: ThinkFn) {
        if let Some(s) = self.slot_mut(id) {
            s.func = func;
        }
    }

    pub fn func(&self, id: ThinkerId) -> Option<ThinkFn> {
        self.slot(id).map(|s| s.func)
    }

    /// True while the handle refers to a live (possibly removal-marked) thinker.
    pub fn exists(&self, id: ThinkerId) -> bool {
        self.slot(id).is_some()
    }

    pub fn is_removed(&self, id: ThinkerId) -> bool {
        self.func(id) != Some(ThinkFn::Active) && self.func(id) != Some(ThinkFn::Stasis)
    }

    fn slot(&self, id: ThinkerId) -> Option<&Slot> {
        let s = self.slots.get(id.index as usize)?;
        (s.generation == id.generation && !matches!(s.data, ThinkerData::Free | ThinkerData::Sentinel)).then_some(s)
    }

    fn slot_mut(&mut self, id: ThinkerId) -> Option<&mut Slot> {
        let s = self.slots.get_mut(id.index as usize)?;
        (s.generation == id.generation && !matches!(s.data, ThinkerData::Free | ThinkerData::Sentinel)).then_some(s)
    }

    pub fn data(&self, id: ThinkerId) -> Option<&ThinkerData> {
        self.slot(id).map(|s| &s.data)
    }

    pub fn data_mut(&mut self, id: ThinkerId) -> Option<&mut ThinkerData> {
        self.slot_mut(id).map(|s| &mut s.data)
    }

    pub fn get_mobj(&self, id: ThinkerId) -> Option<&Mobj> {
        match self.data(id)? {
            ThinkerData::Mobj(m) => Some(m),
            _ => None,
        }
    }

    pub fn get_mobj_mut(&mut self, id: ThinkerId) -> Option<&mut Mobj> {
        match self.data_mut(id)? {
            ThinkerData::Mobj(m) => Some(m),
            _ => None,
        }
    }

    /// The first thinker in list order.
    pub fn first(&self) -> Option<ThinkerId> {
        self.after_index(CAP)
    }

    /// The thinker following `id` in list order (read *after* running `id`,
    /// as `P_RunThinkers` does).
    pub fn next(&self, id: ThinkerId) -> Option<ThinkerId> {
        self.after_index(id.index)
    }

    fn after_index(&self, index: u32) -> Option<ThinkerId> {
        let n = self.slots[index as usize].next;
        (n != CAP).then(|| ThinkerId { index: n, generation: self.slots[n as usize].generation })
    }

    /// Unlink and free a removal-marked thinker; returns its successor.
    /// Only `P_RunThinkers` (and level teardown) should call this.
    pub fn free(&mut self, id: ThinkerId) -> Option<ThinkerId> {
        let next = self.next(id);
        let (prev, nx) = {
            let s = &self.slots[id.index as usize];
            (s.prev, s.next)
        };
        self.slots[nx as usize].prev = prev;
        self.slots[prev as usize].next = nx;
        let s = &mut self.slots[id.index as usize];
        s.data = ThinkerData::Free;
        s.generation = s.generation.wrapping_add(1);
        self.free.push(id.index);
        next
    }

    /// Iterate in list order (snapshot-free; do not mutate while iterating).
    pub fn iter(&self) -> impl Iterator<Item = (ThinkerId, &ThinkerData, ThinkFn)> {
        let mut cur = self.first();
        std::iter::from_fn(move || {
            let id = cur?;
            cur = self.next(id);
            let s = &self.slots[id.index as usize];
            Some((id, &s.data, s.func))
        })
    }

    pub fn mobjs(&self) -> impl Iterator<Item = (ThinkerId, &Mobj)> {
        self.iter().filter_map(|(id, d, f)| match d {
            ThinkerData::Mobj(m) if f != ThinkFn::Removed => Some((id, &**m)),
            _ => None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy() -> ThinkerData {
        ThinkerData::Mobj(Box::default())
    }

    #[test]
    fn order_and_deferred_free() {
        let mut t = Thinkers::new();
        let a = t.add(dummy(), ThinkFn::Active);
        let b = t.add(dummy(), ThinkFn::Active);
        let c = t.add(dummy(), ThinkFn::Active);
        assert_eq!(t.iter().map(|x| x.0).collect::<Vec<_>>(), vec![a, b, c]);
        t.remove(b);
        assert!(t.exists(b), "removed thinkers stay readable until freed");
        assert_eq!(t.free(b), Some(c));
        assert!(!t.exists(b));
        let d = t.add(dummy(), ThinkFn::Active);
        assert_eq!(d.index, b.index, "slot reused");
        assert_ne!(d.generation, b.generation, "but stale handle does not resolve");
        assert_eq!(t.iter().map(|x| x.0).collect::<Vec<_>>(), vec![a, c, d]);
    }
}
