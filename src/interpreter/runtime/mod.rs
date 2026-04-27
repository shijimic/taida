//! D29 Track-θ runtime support modules for `Value::Str` rope path.
//!
//! See `.dev/D29_BLOCKERS.md::D29B-016` and Lock-K verdict for the design
//! rationale (`Value::Str` interior wrapping with a gap-buffer rope path,
//! transparent promotion at the 1024-byte concat threshold).

pub mod str_rope;
