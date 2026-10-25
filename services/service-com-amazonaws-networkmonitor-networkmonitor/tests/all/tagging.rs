// SPDX-License-Identifier: CC0-1.0

//! `TagResource` against the READ PATHS: one stored tag map per resource, behind
//! every read that reports that resource's tags, and behind no other resource's.
//!
//! The merge table (which pairs a write adds, overwrites and leaves alone), the
//! concurrent-write convergence case, the `TagMap` bound and the 404 for an ARN
//! no stored resource carries live in `resource_tags.rs`, beside
//! `UntagResource`'s cases; this suite reuses that harness rather than building
//! a second one, and asserts what the other read paths report:
//!
//! * `TagResource` answers the no-content status with the modelled EMPTY output
//!   (`TagResourceOutput` declares no members), not a payload and not the
//!   scaffold's 501 — the status rule itself is pinned in
//!   `tag_mutation_no_content.rs`;
//! * `GetMonitor`'s `tags`, its `probes[].tags` and `ListMonitors`' summary
//!   `tags` are all views of the SAME stored map, so a write through one ARN is
//!   reported identically by each of them;
//! * a write reaches only the resource its ARN named: the other monitor, and the
//!   other resource inside the same monitor, keep the tags they had.

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use crate::resource_tags::{
    call, fixture, get_monitor, only_probe, router, tag_resource, TAG_MUTATION_STATUS,
};

async fn list_monitors(router: &MultiServiceRouter) -> (u16, Value) {
    call(router, "GET", "/monitors", None).await
}

/// The `tags` of the `ListMonitors` summary for `name`.
fn summary_tags(listed: &Value, name: &str) -> Value {
    listed
        .get("monitors")
        .and_then(Value::as_array)
        .and_then(|monitors| {
            monitors
                .iter()
                .find(|monitor| {
                    monitor.get("monitorName").and_then(Value::as_str) == Some(name)
                })
                .and_then(|monitor| monitor.get("tags").cloned())
        })
        .unwrap_or(Value::Null)
}

#[tokio::test]
async fn a_tag_write_is_reported_by_every_read_path_of_the_resource_it_named() {
    let router = router();
    let (target_monitor_arn, target_probe_arn) = fixture(
        &router,
        "tagged-target",
        &serde_json::json!({"team": "net"}),
        &serde_json::json!({"role": "edge"}),
    )
    .await;
    // A second monitor with the same starting tags: a write must not reach it.
    let (_, _) = fixture(
        &router,
        "tagged-bystander",
        &serde_json::json!({"team": "net"}),
        &serde_json::json!({"role": "edge"}),
    )
    .await;

    // (case, the ARN written through, the pairs written)
    let writes: Vec<(&str, &str, Value)> = vec![
        (
            "the monitor's own ARN",
            target_monitor_arn.as_str(),
            serde_json::json!({"env": "prod"}),
        ),
        (
            "the probe's ARN",
            target_probe_arn.as_str(),
            serde_json::json!({"owner": "probe"}),
        ),
    ];
    for (case, resource_arn, written) in writes {
        let (status, body) = tag_resource(&router, resource_arn, &written).await;
        assert_eq!(
            status, TAG_MUTATION_STATUS,
            "case {case:?}: expected TagResource {TAG_MUTATION_STATUS}, got {status} {body:?}"
        );
        // `TagResourceOutput` declares no members, so the answer carries no
        // payload members — an empty body or an empty object, never a 501.
        assert!(
            body.is_null() || body == serde_json::json!({}),
            "case {case:?}: TagResource answers an empty output, got {body:?}"
        );
    }

    let want_monitor_tags = serde_json::json!({"team": "net", "env": "prod"});
    let want_probe_tags = serde_json::json!({"role": "edge", "owner": "probe"});

    let (status, got) = get_monitor(&router, "tagged-target").await;
    assert_eq!(status, 200, "expected GetMonitor 200, got {status} {got:?}");
    assert_eq!(
        got.get("tags"),
        Some(&want_monitor_tags),
        "case \"GetMonitor tags\": expected {want_monitor_tags}, got {got:?}"
    );
    assert_eq!(
        only_probe(&got).get("tags"),
        Some(&want_probe_tags),
        "case \"GetMonitor probes[].tags\": expected {want_probe_tags}, got {got:?}"
    );

    let (status, listed) = list_monitors(&router).await;
    assert_eq!(
        status, 200,
        "expected ListMonitors 200, got {status} {listed:?}"
    );
    assert_eq!(
        summary_tags(&listed, "tagged-target"),
        want_monitor_tags,
        "case \"ListMonitors summary tags\": the same stored map must be reported, \
         expected {want_monitor_tags}, got {listed:?}"
    );

    // The bystander monitor, and the bystander's probe, kept their own tags: a
    // write reaches only the resource its ARN named.
    let (status, bystander) = get_monitor(&router, "tagged-bystander").await;
    assert_eq!(
        status, 200,
        "expected GetMonitor 200 for the bystander, got {status} {bystander:?}"
    );
    assert_eq!(
        bystander.get("tags"),
        Some(&serde_json::json!({"team": "net"})),
        "case \"another monitor\": its tags must not change, got {bystander:?}"
    );
    assert_eq!(
        only_probe(&bystander).get("tags"),
        Some(&serde_json::json!({"role": "edge"})),
        "case \"another monitor's probe\": its tags must not change, got {bystander:?}"
    );
    assert_eq!(
        summary_tags(&listed, "tagged-bystander"),
        serde_json::json!({"team": "net"}),
        "case \"ListMonitors summary of another monitor\": its tags must not change, \
         got {listed:?}"
    );
}
