//! Typed indices for level and simulation objects.
//!
//! The C code uses raw pointers everywhere; we use indices so the data can be
//! stored in flat `Vec`s, cloned for savegames/rewind, and compared cheaply.

macro_rules! id_type {
    ($(#[$m:meta])* $name:ident) => {
        $(#[$m])*
        #[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Default)]
        pub struct $name(pub u32);

        impl $name {
            #[inline]
            pub const fn idx(self) -> usize {
                self.0 as usize
            }
        }
    };
}

id_type!(VertexId);
id_type!(SectorId);
id_type!(SideId);
id_type!(LineId);
id_type!(SegId);
id_type!(SubsectorId);
id_type!(NodeId);
/// Handle to a thinker (map objects are thinkers too).
///
/// Generational: once the thinker is freed (which happens when
/// `P_RunThinkers` reaches a removed thinker, exactly as in vanilla) the
/// slot's generation changes and stale handles stop resolving. Vanilla keeps
/// raw pointers to freed zone memory instead; see docs/COMPATIBILITY.md.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct ThinkerId {
    pub index: u32,
    pub generation: u32,
}

/// Map objects are thinkers; the alias documents intent at use sites.
pub type MobjId = ThinkerId;

/// Texture number (index into `doom_content::texture::TextureSet`); 0 = none.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct TexNum(pub u16);

/// Flat number (index into the merged flat namespace).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct FlatNum(pub u16);

impl FlatNum {
    /// A flat name that does not exist. Vanilla aborts with
    /// "R_FlatNumForName: %s not found"; we keep loading and let the
    /// renderer show a placeholder.
    pub const MISSING: FlatNum = FlatNum(u16::MAX);
}
