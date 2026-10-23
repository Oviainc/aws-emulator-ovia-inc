// SPDX-License-Identifier: CC0-1.0

//! SUPERSEDED and EMPTY: this suite holds no tests, and is declared in `tests/all.rs`
//! only because the crate's layout check requires every file under `tests/all/` to be
//! declared and this agent's tools cannot delete a file. Delete the file and its
//! declaration together.
//!
//! It covered `CreateEnvironmentConnector`/`ListEnvironmentConnectors` together with
//! the host surface. Mainline landed its own, fuller connector suite while this branch
//! was open — `tests/all/connectors.rs`, which also covers
//! `UpdateEnvironmentConnector` — so the connector half is gone, and the host half
//! moved to `tests/all/hosts.rs`.
