// SPDX-License-Identifier: CC0-1.0

//! How many sources ONE FLOW may hold, end to end over HTTP.
//!
//! The rule under test comes from `FailoverConfig$State`
//! (`smithy_query(query="doc",shape="FailoverConfig",member="State")`): "The state of
//! source failover on the flow. If the state is inactive, the flow can have only one
//! source. If the state is active, the flow can have one or two sources." So a flow's
//! source count is bounded by its OWN failover state — one source while that state is
//! inactive (no `SourceFailoverConfig` at all, or one whose `State` is not ENABLED), two
//! while it is active — and every request that would leave the flow outside that bound is
//! refused with the modelled `BadRequestException` (HTTP 400), leaving the flow exactly as
//! it was.
//!
//! The rule binds every operation that decides a flow's source count: CreateFlow (which
//! stores the sources), AddFlowSources (which appends to them) and UpdateFlow (which moves
//! the bound by changing the failover state under the sources the flow already holds).
//! Each has its own table below.
//!
//! The cases drive the real request path (routing, restJson1 deserialization, constraint
//! validation, handler, serialization) through the harness
//! [`crate::flow_lifecycle::Emulator`], so what they assert is what a MediaConnect client
//! sees on the wire.

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_lifecycle::Emulator;
use crate::flow_sources::{add_flow_sources, described_sources};

/// The two counts `FailoverConfig$State` publishes: "if the state is inactive, the flow
/// can have only one source. If the state is active, the flow can have one or two
/// sources."
const ACTIVE_BOUND: usize = 2;
/// See [`ACTIVE_BOUND`].
const INACTIVE_BOUND: usize = 1;

/// `count` source requests, named `src-<n>`, each on a protocol the service accepts.
fn sources(count: usize) -> Value {
    (0..count)
        .map(|index| {
            json!({
                "name": format!("src-{index}"),
                "protocol": "rtp",
                "whitelistCidr": "10.9.0.0/16"
            })
        })
        .collect()
}

/// The `SourceFailoverConfig` forms a request can carry, paired with whether each one
/// leaves source failover ACTIVE. `State` targets the `State` enum
/// (`smithy_query(query="shape",shape="State")`: ENABLED | DISABLED), and the member is
/// optional, so a config may also leave it unset — which is not the active state either.
fn failover_forms() -> Vec<(&'static str, Option<Value>, bool)> {
    vec![
        ("no sourceFailoverConfig at all", None, false),
        (
            "failover state DISABLED",
            Some(json!({ "state": "DISABLED" })),
            false,
        ),
        (
            "a failover config that names a mode but no state",
            Some(json!({ "failoverMode": "MERGE" })),
            false,
        ),
        (
            "failover state ENABLED",
            Some(json!({ "state": "ENABLED" })),
            true,
        ),
        (
            "failover state ENABLED with a mode and a recovery window",
            Some(json!({ "state": "ENABLED", "failoverMode": "FAILOVER", "recoveryWindow": 3000 })),
            true,
        ),
    ]
}

/// The bound a flow whose failover config is `failover` holds its sources under.
fn bound(active: bool) -> usize {
    if active {
        ACTIVE_BOUND
    } else {
        INACTIVE_BOUND
    }
}

/// A CreateFlow body for `count` sources under `failover`.
fn create_body(name: &str, failover: Option<&Value>, count: usize) -> Value {
    let mut body = json!({ "name": name, "sources": sources(count) });
    if let Some(failover) = failover {
        body["sourceFailoverConfig"] = failover.clone();
    }
    body
}

/// The bound the crate publishes is the one the model publishes: the two constants the
/// handlers read it from carry the counts of `FailoverConfig$State`.
#[tokio::test]
async fn the_published_bound_is_the_documented_one() {
    assert_eq!(
        (mc::MAX_FLOW_SOURCES, mc::MAX_FLOW_SOURCES_FAILOVER_INACTIVE),
        (ACTIVE_BOUND, INACTIVE_BOUND),
        "case \"published bound\": a flow holds one source while failover is inactive and \
         two while it is active, got {} / {}",
        mc::MAX_FLOW_SOURCES,
        mc::MAX_FLOW_SOURCES_FAILOVER_INACTIVE
    );
}

/// CreateFlow stores a flow whose source count is inside the bound its own failover state
/// sets, and refuses — with `BadRequestException` (400) — one past it, storing no flow.
///
/// The rows span the failover forms a request can carry (none, inactive, active) crossed
/// with the counts around each form's bound: every count up to it, the first count past
/// it, and one far past it.
#[tokio::test]
async fn create_flow_bounds_a_flow_s_sources_by_its_failover_state() {
    let emulator = Emulator::new();

    for (form, failover, active) in failover_forms() {
        let limit = bound(active);
        // Every count the bound allows (a flow must have at least one source), the first
        // count past it, and one far past it — the generic collection bound CreateFlow
        // applies to its OTHER collections, which may not be used as a flow's source
        // ceiling.
        let counts: Vec<usize> = (1..=limit)
            .chain([limit + 1, mc::MAX_FLOW_COLLECTION_ENTRIES])
            .collect();
        for count in counts {
            let case = format!("{form}, {count} source(s)");
            let name = format!(
                "bound-create-{}-{count}",
                form.replace(' ', "-").replace(',', "")
            );
            let (status, code, body) = emulator
                .create_flow(create_body(&name, failover.as_ref(), count))
                .await;
            if count <= limit {
                assert_eq!(
                    (status, code.as_deref()),
                    (201, None),
                    "case {case:?}: a flow inside its bound of {limit} is created, got \
                     {status} {code:?}: {body}"
                );
                let flow_arn = body["flow"]["flowArn"].as_str().unwrap_or_default();
                let stored = described_sources(&emulator, &case, flow_arn).await;
                assert_eq!(
                    stored.len(),
                    count,
                    "case {case:?}: the stored flow holds the sources the create named, \
                     got {stored:?}"
                );
            } else {
                assert_eq!(
                    (status, code.as_deref()),
                    (400, Some("BadRequestException")),
                    "case {case:?}: {count} sources is past the bound of {limit}, got \
                     {status} {code:?}: {body}"
                );
                // A refused create stores nothing: no flow answers under that name.
                let (status, _, listed) = emulator.list_flows("").await;
                assert_eq!(status, 200, "case {case:?}: ListFlows must answer 200");
                let named = listed["flows"]
                    .as_array()
                    .map(|flows| {
                        flows
                            .iter()
                            .any(|flow| flow["name"].as_str() == Some(name.as_str()))
                    })
                    .unwrap_or_default();
                assert!(
                    !named,
                    "case {case:?}: a refused create stores no flow, yet {name:?} is listed: \
                     {listed}"
                );
            }
        }
    }
}

/// A single-source create is expressible through `Source` as well as through `Sources`,
/// and the bound is the same either way: the one source it stores is inside the inactive
/// bound, so a flow created that way is accepted and still refuses a second source.
#[tokio::test]
async fn the_bound_counts_the_source_member_too() {
    let emulator = Emulator::new();
    let case = "a flow created through Source alone";

    let (status, code, created) = emulator
        .create_flow(json!({
            "name": "bound-single-source-member",
            "source": { "name": "only", "protocol": "rtp", "whitelistCidr": "10.0.0.0/16" }
        }))
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case {case:?}: one source is inside the inactive bound, got {status} {code:?}: \
         {created}"
    );
    let flow_arn = created["flow"]["flowArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let (status, code, body) = add_flow_sources(
        &emulator,
        &flow_arn,
        json!([{ "name": "second", "protocol": "rtp", "whitelistCidr": "10.1.0.0/16" }]),
    )
    .await;
    assert_eq!(
        (status, code.as_deref()),
        (400, Some("BadRequestException")),
        "case {case:?}: its failover is inactive, so a second source is refused, got \
         {status} {code:?}: {body}"
    );
    assert_eq!(
        described_sources(&emulator, case, &flow_arn).await.len(),
        INACTIVE_BOUND,
        "case {case:?}: the refused add stores nothing"
    );
}

/// AddFlowSources counts the STORED flow: a call is accepted exactly while the sources the
/// flow already holds plus the ones the call adds stay inside the bound the flow's
/// failover state sets, and anything past it is refused with `BadRequestException` (400)
/// and appends nothing.
///
/// The rows span the failover forms crossed with how many sources the flow already holds
/// and how many one call adds — so a call refused for overshooting on its own is covered
/// beside one refused for a flow that is already full.
#[tokio::test]
async fn add_flow_sources_bounds_a_flow_s_sources_by_its_failover_state() {
    let emulator = Emulator::new();

    for (index, (form, failover, active)) in failover_forms().into_iter().enumerate() {
        let limit = bound(active);
        for held in 1..=limit {
            for adding in [1usize, 2, 3] {
                let case = format!("{form}, {held} held, {adding} added");
                let name = format!("bound-add-{index}-{held}-{adding}");
                let (status, code, created) = emulator
                    .create_flow(create_body(&name, failover.as_ref(), held))
                    .await;
                assert_eq!(
                    (status, code.as_deref()),
                    (201, None),
                    "case {case:?}: the flow under test must be created, got {status} \
                     {code:?}: {created}"
                );
                let flow_arn = created["flow"]["flowArn"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                let before = described_sources(&emulator, &case, &flow_arn).await;

                let add: Value = (0..adding)
                    .map(|offset| {
                        json!({
                            "name": format!("added-{offset}"),
                            "protocol": "udp",
                            "whitelistCidr": "10.8.0.0/16"
                        })
                    })
                    .collect();
                let (status, code, body) = add_flow_sources(&emulator, &flow_arn, add).await;
                let after = described_sources(&emulator, &case, &flow_arn).await;
                if held + adding <= limit {
                    assert_eq!(
                        (status, code.as_deref()),
                        (201, None),
                        "case {case:?}: the add stays inside the bound of {limit}, got \
                         {status} {code:?}: {body}"
                    );
                    assert_eq!(
                        after.len(),
                        held + adding,
                        "case {case:?}: the flow holds what it held plus what was added, \
                         got {after:?}"
                    );
                } else {
                    assert_eq!(
                        (status, code.as_deref()),
                        (400, Some("BadRequestException")),
                        "case {case:?}: {} sources is past the bound of {limit}, got \
                         {status} {code:?}: {body}",
                        held + adding
                    );
                    assert_eq!(
                        after, before,
                        "case {case:?}: a refused add appends nothing, got {after:?}"
                    );
                }
            }
        }
    }
}

/// UpdateFlow moves the bound rather than the source count: a request that leaves the
/// flow's failover state inactive under a pair of sources is refused with
/// `BadRequestException` (400) and writes nothing, and one that turns failover ON widens
/// the flow's bound, so the add that was refused before it is accepted after.
#[tokio::test]
async fn update_flow_moves_the_bound_the_flow_holds_its_sources_under() {
    let emulator = Emulator::new();

    // A pair of sources under ACTIVE failover: switching the state off would leave the
    // flow over the inactive bound, so it is refused and the stored config is untouched.
    let case = "failover switched off under a pair of sources";
    let (status, code, created) = emulator
        .create_flow(create_body(
            "bound-update-pair",
            Some(&json!({ "state": "ENABLED", "failoverMode": "MERGE" })),
            ACTIVE_BOUND,
        ))
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case {case:?}: the flow under test must be created, got {status} {code:?}: {created}"
    );
    let pair_arn = created["flow"]["flowArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let stored_config = created["flow"]["sourceFailoverConfig"].clone();

    for off in [json!({ "state": "DISABLED" }), json!({ "state": "DISABLED", "failoverMode": "FAILOVER" })] {
        let (status, code, body) = emulator
            .send(
                "PUT",
                &format!("/v1/flows/{pair_arn}"),
                Some(json!({ "sourceFailoverConfig": off })),
            )
            .await;
        assert_eq!(
            (status, code.as_deref()),
            (400, Some("BadRequestException")),
            "case {case:?}: expected 400 BadRequestException for {off}, got {status} \
             {code:?}: {body}"
        );
        let (_, _, described) = emulator.describe_flow(&pair_arn).await;
        assert_eq!(
            described["flow"]["sourceFailoverConfig"], stored_config,
            "case {case:?}: a refused update writes nothing, got {:?}",
            described["flow"]["sourceFailoverConfig"]
        );
    }

    // One source under INACTIVE failover: the second source is refused, the update that
    // turns failover on is accepted, and the same add is accepted afterwards.
    let case = "failover turned on widens the bound";
    let (status, code, created) = emulator
        .create_flow(create_body("bound-update-single", None, 1))
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case {case:?}: the flow under test must be created, got {status} {code:?}: {created}"
    );
    let flow_arn = created["flow"]["flowArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let second = json!([{ "name": "second", "protocol": "rtp", "whitelistCidr": "10.2.0.0/16" }]);

    let (status, code, body) = add_flow_sources(&emulator, &flow_arn, second.clone()).await;
    assert_eq!(
        (status, code.as_deref()),
        (400, Some("BadRequestException")),
        "case {case:?}: before the update the bound is {INACTIVE_BOUND}, got {status} \
         {code:?}: {body}"
    );

    let (status, code, body) = emulator
        .send(
            "PUT",
            &format!("/v1/flows/{flow_arn}"),
            Some(json!({ "sourceFailoverConfig": { "state": "ENABLED" } })),
        )
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (202, None),
        "case {case:?}: turning failover on under one source is accepted, got {status} \
         {code:?}: {body}"
    );

    let (status, code, body) = add_flow_sources(&emulator, &flow_arn, second).await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case {case:?}: with failover active the pair is accepted, got {status} {code:?}: \
         {body}"
    );
    assert_eq!(
        described_sources(&emulator, case, &flow_arn).await.len(),
        ACTIVE_BOUND,
        "case {case:?}: the flow holds the failover pair"
    );

    // And the pair is still the ceiling: a third source is refused however active the
    // failover state is.
    let (status, code, body) = add_flow_sources(
        &emulator,
        &flow_arn,
        json!([{ "name": "third", "protocol": "rtp", "whitelistCidr": "10.3.0.0/16" }]),
    )
    .await;
    assert_eq!(
        (status, code.as_deref()),
        (400, Some("BadRequestException")),
        "case {case:?}: a third source is past the active bound of {ACTIVE_BOUND}, got \
         {status} {code:?}: {body}"
    );
}
