// SPDX-License-Identifier: CC0-1.0

//! Single integration-test target for this crate.
//!
//! Put a new suite at `tests/all/<name>.rs` and declare it below, keeping the list
//! sorted.
//!
//! A suite that is not declared here compiles nowhere and its tests never run. A
//! suite left at the top level of `tests/` fails the build.

// `tests/all.rs` is a test-target CRATE ROOT, so a bare `mod` resolves to a SIBLING
// (`tests/state_roundtrip.rs`) — which the layout check refuses. The `#[path]` is what
// points it into `tests/all/`.
#[path = "all/harness.rs"]
mod harness;
#[path = "all/ses_identity_policies.rs"]
mod ses_identity_policies;
#[path = "all/ses_identity_send_loop.rs"]
mod ses_identity_send_loop;
#[path = "all/ses_store_bounds.rs"]
mod ses_store_bounds;
#[path = "all/ses_template_listing.rs"]
mod ses_template_listing;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
// The `zz_` prefix keeps this service-wide suite sorted after the per-operation
// ones; `tests/all.rs` lists suites in file order.
#[path = "all/zz_dump.rs"]
mod zz_dump;
