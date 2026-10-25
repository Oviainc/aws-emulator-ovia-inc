// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `StopDiscoverer`.
//!
//! The rule these cases pin: `StopDiscoverer` writes exactly ONE member of exactly ONE
//! stored discoverer — that discoverer's `State`, to `STOPPED` — answers it from the
//! stored resource together with the id it acted on, and leaves every other member of
//! that discoverer and every other resource of the store exactly as it found them;
//! because the write names a state rather than a change to one, repeating the call
//! answers the same thing again.
//!
//! The operation models only `DiscovererId` (an `@httpLabel`) in and `DiscovererId` plus
//! `State` out (`smithy_query(query="operation", operation="StopDiscoverer")`: `POST
//! /v1/discoverers/id/{DiscovererId}/stop -> 200`), so there is no request body to vary:
//! the rule is asserted over the DISCOVERERS the call is made against — how they were
//! created, which state they are already in, how long the path that got them there was,
//! how often the stop is repeated, and what else the store holds beside them.
//!
//! `tests/all/discoverer_state.rs` owns the start/stop state machine and the refusal of
//! an id nothing minted; these cases own what a stop must NOT touch. Both drive the real
//! wire path through `MultiServiceRouter::dispatch`, and share that suite's helpers and
//! the crate's one HTTP harness.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use service_com_amazonaws_schemas_schemas::ops::types::DISCOVERER_STATE_VALUES;

use crate::discoverer_state::{
    create_discoverer, discoverer_uri, source_arn, state_of, stop_uri, Move, DISCOVERERS_URI,
};
use crate::registry_schema_lifecycle::{create_registry, registry_uri, schema_uri, Emulator};

/// A `DescribeDiscoverer` body with `State` dropped: everything about a discoverer that
/// a stop must leave exactly as it was.
fn without_state(body: &Value) -> Value {
    let mut copy = body.clone();
    copy.as_object_mut()
        .expect("DescribeDiscoverer answers a JSON object")
        .remove("State");
    copy
}

/// One discoverer the table below stops, and the way it got into the state it is in.
#[derive(Debug)]
struct StopCase {
    case: &'static str,
    /// The bus this row's discoverer subscribes to; distinct per row, since one source
    /// carries one discoverer.
    bus: &'static str,
    description: Option<String>,
    /// The `CrossAccount` the create sends, or `None` to omit the member.
    cross_account: Option<bool>,
    tags: BTreeMap<&'static str, &'static str>,
    /// The start/stop calls made BEFORE the stop under test, so the stop is exercised
    /// from each state a discoverer can be in, reached by paths of differing length.
    prelude: Vec<Move>,
    /// How many times the stop under test is repeated.
    repeats: usize,
}

impl StopCase {
    /// The `CreateDiscoverer` body this row sends. `Tags` travels as `tags`
    /// (`smithy_query(query="operation", operation="CreateDiscoverer")`: the member
    /// carries `jsonName: "tags"`).
    fn body(&self) -> Value {
        let mut body = json!({"SourceArn": source_arn(self.bus)});
        if let Some(description) = &self.description {
            body["Description"] = json!(description);
        }
        if let Some(cross_account) = self.cross_account {
            body["CrossAccount"] = json!(cross_account);
        }
        if !self.tags.is_empty() {
            body["tags"] = json!(self.tags);
        }
        body
    }
}

/// The table. Rows differ along what the rule generalises over: which members the stopped
/// discoverer carries (a description at the published bounds of `__stringMin0Max256` —
/// source: `smithy_query(query="operation", operation="CreateDiscoverer")` — the presence
/// and value of `CrossAccount`, and how many tags it holds), the state it is already in,
/// the length of the start/stop path that got it there, and how often the stop repeats.
fn stop_cases() -> Vec<StopCase> {
    use Move::{Start, Stop};
    vec![
        StopCase {
            case: "a freshly created, bare discoverer, stopped once",
            bus: "bare",
            description: None,
            cross_account: None,
            tags: BTreeMap::new(),
            prelude: vec![],
            repeats: 1,
        },
        StopCase {
            case: "every member set, stopped once",
            bus: "furnished",
            description: Some("orders bus discoverer".to_string()),
            cross_account: Some(false),
            tags: BTreeMap::from([("env", "test"), ("team", "schemas")]),
            prelude: vec![],
            repeats: 1,
        },
        StopCase {
            case: "already stopped, stopped again twice",
            bus: "already-stopped",
            description: Some(String::new()),
            cross_account: Some(true),
            tags: BTreeMap::from([("only", "one")]),
            prelude: vec![Stop],
            repeats: 2,
        },
        StopCase {
            case: "description at the modelled upper bound, stopped three times",
            bus: "long-description",
            description: Some("d".repeat(256)),
            cross_account: None,
            tags: BTreeMap::new(),
            prelude: vec![],
            repeats: 3,
        },
        StopCase {
            case: "restarted after a stop, then stopped again",
            bus: "restarted",
            description: Some("caf\u{e9} \u{2014} \u{30b9}\u{30ad}\u{30fc}\u{30de}".to_string()),
            cross_account: Some(true),
            tags: BTreeMap::from([("a", ""), ("b", "2")]),
            prelude: vec![Stop, Start],
            repeats: 1,
        },
        StopCase {
            case: "stopped after a long alternating path",
            bus: "long-path",
            description: None,
            cross_account: Some(false),
            tags: BTreeMap::from([("a", "1"), ("b", "2"), ("c", "3")]),
            prelude: vec![Start, Stop, Start, Stop, Start],
            repeats: 2,
        },
    ]
}

/// RULE: `StopDiscoverer` writes one member of the discoverer its id names — `State`, to
/// `STOPPED` — answers it from the stored resource beside that id, and leaves every other
/// member of the discoverer untouched, however it was created, whatever state it was
/// already in, and however often the call is repeated.
#[tokio::test]
async fn a_stop_writes_the_state_and_nothing_else_about_the_discoverer() {
    let emulator = Emulator::new();

    for case in stop_cases() {
        let created = emulator
            .call("POST", DISCOVERERS_URI, Some(case.body()))
            .await;
        assert_eq!(
            created.status, 201,
            "case {:?}: expected 201 from CreateDiscoverer, got {} {:?}",
            case.case, created.status, created.body
        );
        let discoverer_id = created.str_at("DiscovererId");

        for (step, movement) in case.prelude.iter().enumerate() {
            let answer = emulator
                .call("POST", &movement.uri(&discoverer_id), None)
                .await;
            assert_eq!(
                answer.status,
                200,
                "case {:?}: prelude step {step} ({movement:?}) expected 200, got {} {:?}",
                case.case,
                answer.status,
                answer.body
            );
            assert_eq!(
                state_of(&answer, case.case),
                movement.leaves(),
                "case {:?}: prelude step {step} ({movement:?}) must leave {}, got {:?}",
                case.case,
                movement.leaves(),
                answer.body
            );
        }

        // The discoverer as it stands just before the stop under test.
        let before = emulator
            .call("GET", &discoverer_uri(&discoverer_id), None)
            .await;
        assert_eq!(
            before.status, 200,
            "case {:?}: expected 200 from DescribeDiscoverer, got {} {:?}",
            case.case, before.status, before.body
        );

        let mut first_answer: Option<Value> = None;
        for repeat in 0..case.repeats {
            let row = format!("{} [stop {repeat}]", case.case);
            let answer = emulator.call("POST", &stop_uri(&discoverer_id), None).await;
            assert_eq!(
                answer.status, 200,
                "case {row:?}: expected 200, got {} {:?}",
                answer.status, answer.body
            );

            // The answer carries exactly the two members the operation models, and both
            // name the stored resource rather than anything the request carried.
            let members: BTreeSet<&str> = answer
                .body
                .as_object()
                .unwrap_or_else(|| panic!("case {row:?}: expected a JSON object, got {:?}", answer.body))
                .keys()
                .map(String::as_str)
                .collect();
            assert_eq!(
                members,
                BTreeSet::from(["DiscovererId", "State"]),
                "case {row:?}: StopDiscovererResponse models DiscovererId and State only, \
                 got {:?}",
                answer.body
            );
            assert_eq!(
                answer.str_at("DiscovererId"),
                discoverer_id,
                "case {row:?}: the answer names the discoverer it stopped, got {:?}",
                answer.body
            );
            let state = state_of(&answer, &row);
            assert!(
                DISCOVERER_STATE_VALUES.contains(&state.as_str()),
                "case {row:?}: State must be one of {DISCOVERER_STATE_VALUES:?}, got {state:?}"
            );
            assert_eq!(
                state, "STOPPED",
                "case {row:?}: a stopped discoverer reports STOPPED, got {:?}",
                answer.body
            );

            // Repeating the call answers the same thing again: the stop names the state
            // it leaves, not a change to it.
            match &first_answer {
                None => first_answer = Some(answer.body.clone()),
                Some(first) => assert_eq!(
                    &answer.body, first,
                    "case {row:?}: a repeated stop must answer what the first one did \
                     ({first:?}), got {:?}",
                    answer.body
                ),
            }

            // And the stored discoverer agrees with the answer in `State` and is
            // otherwise byte-for-byte what it was before the stop.
            let after = emulator
                .call("GET", &discoverer_uri(&discoverer_id), None)
                .await;
            assert_eq!(
                state_of(&after, &row),
                "STOPPED",
                "case {row:?}: DescribeDiscoverer must report the stored STOPPED, got {:?}",
                after.body
            );
            assert_eq!(
                without_state(&after.body),
                without_state(&before.body),
                "case {row:?}: a stop writes State only; expected the rest of {:?}, got {:?}",
                before.body,
                after.body
            );
        }
    }
}

/// RULE: the state a stop writes belongs to the ONE discoverer its id names, so every
/// other resource the store holds — the other discoverers, and the registry and schema
/// tree a discoverer has nothing to do with — reads back unchanged across the call.
#[tokio::test]
async fn a_stop_leaves_every_other_resource_of_the_store_unchanged() {
    let emulator = Emulator::new();
    let case = "one discoverer stopped among registries, schemas and discoverers";

    let registry = "ledger";
    create_registry(&emulator, registry).await;
    let schema = "order-placed";
    let written = emulator
        .call(
            "POST",
            &schema_uri(registry, schema),
            Some(json!({"Content": "{\"openapi\":\"3.0.0\"}", "Type": "OpenApi3"})),
        )
        .await;
    assert_eq!(
        written.status, 201,
        "case {case:?}: expected 201 from CreateSchema, got {} {:?}",
        written.status, written.body
    );

    // Three discoverers; the middle one is the one stopped.
    let mut ids = Vec::new();
    for bus in ["bus-a", "bus-b", "bus-c"] {
        ids.push((bus, create_discoverer(&emulator, case, bus).await));
    }
    let (_, stopped_id) = ids[1].clone();

    // Every resource of the store, as it stands before the stop.
    let mut snapshots: BTreeMap<String, Value> = BTreeMap::new();
    let mut uris: Vec<(String, String)> = vec![
        ("registry".to_string(), registry_uri(registry)),
        ("schema".to_string(), schema_uri(registry, schema)),
    ];
    for (bus, id) in &ids {
        uris.push(((*bus).to_string(), discoverer_uri(id)));
    }
    for (name, uri) in &uris {
        let described = emulator.call("GET", uri, None).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: expected 200 reading {name}, got {} {:?}",
            described.status, described.body
        );
        snapshots.insert(name.clone(), described.body);
    }

    let answer = emulator.call("POST", &stop_uri(&stopped_id), None).await;
    assert_eq!(
        state_of(&answer, case),
        "STOPPED",
        "case {case:?}: the stopped discoverer reports STOPPED, got {:?}",
        answer.body
    );

    for (name, uri) in &uris {
        let described = emulator.call("GET", uri, None).await;
        let before = &snapshots[name];
        if name == "bus-b" {
            assert_eq!(
                state_of(&described, case),
                "STOPPED",
                "case {case:?}: {name} is the discoverer stopped, got {:?}",
                described.body
            );
            assert_eq!(
                without_state(&described.body),
                without_state(before),
                "case {case:?}: {name} must change in State only; expected the rest of \
                 {before:?}, got {:?}",
                described.body
            );
        } else {
            assert_eq!(
                described.body, *before,
                "case {case:?}: {name} must read back unchanged across the stop; expected \
                 {before:?}, got {:?}",
                described.body
            );
        }
    }
}
