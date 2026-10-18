//! ZScript: GZDoom's actor and gameplay language.
//!
//! - [`lexer`] and [`parser`] turn lumps into an [`ast::File`];
//! - the class table (inheritance, fields, defaults, states, functions)
//!   is built from all parsed files by the engine side, which owns the
//!   actor definitions.

pub mod ast;
pub mod lexer;
pub mod parser;

pub use parser::{parse, parse_decorate, parse_expr, ParseError};
