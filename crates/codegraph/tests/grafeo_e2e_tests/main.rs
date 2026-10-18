//! E2E integration tests for Grafeo-based ingestion and DTO correctness.
//!
//! Proves the full pipeline: JSON schema → Grafeo ingestion → code generation
//! produces correct Candidate DTOs with proper field types for each
//! classification variant.
//!
//! Split from the former single-file `grafeo_e2e_tests.rs` (issue #376); the
//! directory form keeps the same `grafeo_e2e_tests` test-target name.

mod compile_gate;
mod cross_layer;
mod dto_content;
mod full_generation;
mod include_shapes;
mod ingest;
mod scaffold;
mod setup;
