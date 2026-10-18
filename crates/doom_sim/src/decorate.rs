//! DECORATE loading (kept for callers of the old API): actor definitions
//! now come from `crate::actors`, which reads ZScript and DECORATE alike.

use doom_content::ResourceStack;

use crate::info::Info;

pub struct Decorate;

impl Decorate {
    /// Load every actor definition (ZScript, DECORATE, MAPINFO editor
    /// numbers) and SNDINFO into `info`. Returns warnings.
    pub fn apply_stack(info: &mut Info, stack: &ResourceStack) -> Vec<String> {
        crate::actors::load(info, stack)
    }
}
