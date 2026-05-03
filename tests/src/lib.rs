//! herald-smoke-tests — API-drift smoke tests for the herald workspace.
//!
//! This crate has no public API. Each test target under `smoke/` is a
//! standalone integration test that verifies (a) the source file pact
//! decomposed still exists at the expected workspace-relative path, and
//! (b) the public symbols pact captured are still declared `pub` in that
//! source file. Behavioural assertions live inline in each source file's
//! own `#[cfg(test)] mod tests` block.
