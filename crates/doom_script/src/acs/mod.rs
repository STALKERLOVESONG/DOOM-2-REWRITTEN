//! ACS: Hexen/ZDoom's compiled map scripting language.
//!
//! [`Module`] loads `BEHAVIOR` lumps and libraries, [`AcsRuntime`] runs
//! scripts tic by tic against an engine implementing [`AcsHost`].

mod host;
mod module;
pub mod pcode;
mod vm;

pub use host::{AcsHost, Activation, Builtin, PrintKind, Strings, DYNAMIC_STRINGS};
pub use module::{script_flags, Format, FuncDef, LoadError, Module, ScriptDef, ScriptKind, DEFAULT_SCRIPT_VARS, NUM_MAPVARS};
pub use vm::{AcsGlobals, AcsRuntime, RequestKind, ScriptId, ScriptRequest, RUNAWAY_LIMIT};
