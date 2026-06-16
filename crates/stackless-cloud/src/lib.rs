//! Shared scaffolding for cloud substrates (Render, Vercel, …).
//!
//! Only verified-identical machinery lives here — credential resolution and
//! operator-side prepare. Each provider keeps its own error type, codes, and
//! deploy/health semantics; the shared helpers return neutral data the provider
//! maps to its own fault, so per-provider error codes stay distinct
//! (ARCHITECTURE.md §2). Deploy/lifecycle is deliberately NOT abstracted here —
//! it differs materially between providers and has too few instances to
//! generalise safely.

pub mod credential;
pub mod prepare;
