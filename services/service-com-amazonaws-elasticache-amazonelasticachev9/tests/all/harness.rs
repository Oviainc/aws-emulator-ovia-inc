// SPDX-License-Identifier: CC0-1.0

//! The two drivers the snapshot-rule suite beside this file makes its calls
//! through: `CreateSnapshot` and a `DescribeSnapshots` read.
//!
//! This is NOT the test target's harness. The router-backed `Emulator`, the
//! `Reply` reader and the cluster helpers live in
//! `tests/all/cache_cluster_lifecycle.rs` and are `pub(crate)` there, which is
//! what every suite of this target imports — including these two drivers. They
//! sit in their own file because this crate's build script requires every file
//! under `tests/all/` to be declared in `tests/all.rs`, so a suite's helpers
//! cannot be an undeclared include.

use crate::cache_cluster_lifecycle::{params, Emulator, PAST_TRANSITION};
use service_com_amazonaws_elasticache_amazonelasticachev9::state;

/// Take one snapshot named `name` of `cluster` and assert it was accepted.
///
/// Also the one place the wait these tests use is checked against the span the
/// service publishes: a wait that did not out-last
/// [`state::SNAPSHOT_TRANSITION`] would let a test read a snapshot the clock had
/// not settled yet and report it as a failure of the handler.
pub(crate) async fn snapshot(emulator: &Emulator, name: &str, cluster: &str) {
    assert!(
        PAST_TRANSITION > state::SNAPSHOT_TRANSITION,
        "these tests wait {PAST_TRANSITION:?}, which must out-last the snapshot span ({:?}) this \
         service publishes",
        state::SNAPSHOT_TRANSITION
    );
    let created = emulator
        .call(
            "CreateSnapshot",
            &params(&[("SnapshotName", name), ("CacheClusterId", cluster)]),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case {name}: CreateSnapshot must answer 200, got {} {}",
        created.status, created.body
    );
}

/// Read one snapshot back by name, asserting the describe itself succeeded.
pub(crate) async fn describe_snapshot(
    emulator: &Emulator,
    name: &str,
) -> crate::cache_cluster_lifecycle::Reply {
    let described = emulator
        .call("DescribeSnapshots", &params(&[("SnapshotName", name)]))
        .await;
    assert_eq!(
        described.status, 200,
        "case {name}: DescribeSnapshots must answer 200, got {} {}",
        described.status, described.body
    );
    described
}
