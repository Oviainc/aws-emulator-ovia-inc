// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `DeleteDiscoverer`.
//!
//! The rule these cases pin: `DeleteDiscoverer` removes the ONE discoverer its
//! `DiscovererId` names, whole — its source, its state, its description and its tags —
//! and answers with no body at the modelled 204; afterwards that id names no resource,
//! so every read of it and every further delete of it is refused as a missing resource
//! (404), the event source it subscribed to carries no discoverer and may be subscribed
//! to again, and every other resource of the store is left exactly as it was. An id
//! this store holds no discoverer for is refused the same way, and nothing is removed.
//!
//! The operation models only `DiscovererId` (an `@httpLabel`) in and `Unit` out
//! (`smithy_query(query="operation", operation="DeleteDiscoverer")`: `DELETE
//! /v1/discoverers/id/{DiscovererId} -> 204`), so there is no request body to vary: the
//! rule is asserted over the DISCOVERERS the call is made against — which members they
//! carry, how many tags they hold, which state they are in and how long the start/stop
//! path that got them there was, how often the delete is repeated, and what else the
//! store holds beside them.
//!
//! `tests/all/discoverer_state.rs` owns the create round-trip and the start/stop state
//! machine; these cases own the removal. Both drive the real wire path — a signed
//! `restJson1` request through `MultiServiceRouter::dispatch`, exactly as the production
//! router serves it — and share that suite's helpers and the crate's one HTTP harness.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use service_com_amazonaws_schemas_schemas::state::CODE_RESOURCE_NOT_FOUND;

use crate::discoverer_state::{
    create_discoverer, discoverer_uri, source_arn, state_of, stop_uri, Move, DISCOVERERS_URI,
};
use crate::registry_schema_lifecycle::{
    create_registry, registry_uri, schema_uri, Answer, Emulator,
};
use crate::resource_tags::tags_uri;

/// One discoverer the table below deletes, and the way it got into the state it is in.
#[derive(Debug)]
struct DeleteCase {
    case: &'static str,
    /// The bus this row's discoverer subscribes to; distinct per row, since one source
    /// carries one discoverer.
    bus: &'static str,
    description: Option<String>,
    /// The `CrossAccount` the create sends, or `None` to omit the member.
    cross_account: Option<bool>,
    tags: BTreeMap<&'static str, &'static str>,
    /// The start/stop calls made BEFORE the delete, so a discoverer is deleted from
    /// each state it can be in, reached by paths of differing length.
    prelude: Vec<Move>,
    /// How many times the delete is repeated after the first one.
    repeats: usize,
}

impl DeleteCase {
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

/// The table. Rows differ along what the rule generalises over: which members the
/// deleted discoverer carries (a description at the published bounds of
/// `__stringMin0Max256` — source: `smithy_query(query="operation",
/// operation="CreateDiscoverer")` — the presence and value of `CrossAccount`, and how
/// many tags it holds), the state it is in when deleted, the length of the start/stop
/// path that got it there, and how often the delete is repeated.
fn delete_cases() -> Vec<DeleteCase> {
    use Move::{Start, Stop};
    vec![
        DeleteCase {
            case: "a freshly created, bare discoverer",
            bus: "bare",
            description: None,
            cross_account: None,
            tags: BTreeMap::new(),
            prelude: vec![],
            repeats: 1,
        },
        DeleteCase {
            case: "every member set, deleted while started",
            bus: "furnished",
            description: Some("orders bus discoverer".to_string()),
            cross_account: Some(false),
            tags: BTreeMap::from([("env", "test"), ("team", "schemas")]),
            prelude: vec![],
            repeats: 2,
        },
        DeleteCase {
            case: "deleted while stopped",
            bus: "stopped",
            description: Some(String::new()),
            cross_account: Some(true),
            tags: BTreeMap::from([("only", "one")]),
            prelude: vec![Stop],
            repeats: 1,
        },
        DeleteCase {
            case: "description at the modelled upper bound, restarted then deleted",
            bus: "long-description",
            description: Some("d".repeat(256)),
            cross_account: None,
            tags: BTreeMap::new(),
            prelude: vec![Stop, Start],
            repeats: 3,
        },
        DeleteCase {
            case: "deleted after a long alternating path, ending stopped",
            bus: "long-path",
            description: Some("caf\u{e9} \u{2014} \u{30b9}\u{30ad}\u{30fc}\u{30de}".to_string()),
            cross_account: Some(true),
            tags: BTreeMap::from([("a", ""), ("b", "2"), ("c", "3")]),
            prelude: vec![Start, Stop, Start, Stop],
            repeats: 1,
        },
    ]
}

/// Assert that `answer` is a refusal of a discoverer this store does not hold: the
/// service's one not-found status, code and modelled exception shape.
fn assert_missing_discoverer(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 404,
        "case {case:?}: expected 404, got {} {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.str_at("Code"),
        CODE_RESOURCE_NOT_FOUND,
        "case {case:?}: expected Code {CODE_RESOURCE_NOT_FOUND}, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.error_type(),
        "NotFoundException",
        "case {case:?}: expected the modelled NotFoundException, got {:?}",
        answer.header("x-amzn-errortype")
    );
}

/// Assert that the delete answered the modelled 204 with no body, framed with its
/// length.
fn assert_deleted(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 204,
        "case {case:?}: expected 204, got {} {:?}",
        answer.status, answer.body
    );
    assert!(
        answer.raw.is_empty(),
        "case {case:?}: a delete answers no body, got {:?}",
        String::from_utf8_lossy(&answer.raw)
    );
    assert_eq!(
        answer.header("content-length"),
        Some("0"),
        "case {case:?}: an answer with no body states its length, got {:?}",
        answer.header("content-length")
    );
}

/// RULE: a delete removes the whole discoverer its id names, whatever members it
/// carries and whatever state it is in — so the id then names no resource (every read
/// of it, and every further delete, is a missing resource), its tags are gone with it,
/// and the source it subscribed to is free to carry a new discoverer.
#[tokio::test]
async fn a_delete_removes_the_whole_discoverer_and_frees_its_source() {
    let emulator = Emulator::new();
    let mut minted: BTreeSet<String> = BTreeSet::new();

    for case in delete_cases() {
        let created = emulator
            .call("POST", DISCOVERERS_URI, Some(case.body()))
            .await;
        assert_eq!(
            created.status, 201,
            "case {:?}: expected 201 from CreateDiscoverer, got {} {:?}",
            case.case, created.status, created.body
        );
        let discoverer_id = created.str_at("DiscovererId");
        let discoverer_arn = created.str_at("DiscovererArn");
        assert!(
            minted.insert(discoverer_id.clone()),
            "case {:?}: no two discoverers share an id, got {discoverer_id:?}",
            case.case
        );

        for (step, movement) in case.prelude.iter().enumerate() {
            let answer = emulator
                .call("POST", &movement.uri(&discoverer_id), None)
                .await;
            assert_eq!(
                state_of(&answer, case.case),
                movement.leaves(),
                "case {:?}: prelude step {step} ({movement:?}) must leave {}, got {:?}",
                case.case,
                movement.leaves(),
                answer.body
            );
        }

        // The discoverer and its tags are both readable right up to the delete.
        let before = emulator
            .call("GET", &discoverer_uri(&discoverer_id), None)
            .await;
        assert_eq!(
            before.status, 200,
            "case {:?}: expected 200 from DescribeDiscoverer before the delete, got {} {:?}",
            case.case, before.status, before.body
        );
        let tagged = emulator.call("GET", &tags_uri(&discoverer_arn), None).await;
        assert_eq!(
            tagged.status, 200,
            "case {:?}: expected 200 from ListTagsForResource before the delete, got {} {:?}",
            case.case, tagged.status, tagged.body
        );

        let deleted = emulator
            .call("DELETE", &discoverer_uri(&discoverer_id), None)
            .await;
        assert_deleted(case.case, &deleted);

        // The id now names no resource — to the reader, to the tag listing of the ARN
        // the create minted, to the start/stop pair, and to a repeated delete.
        assert_missing_discoverer(
            &format!("{}: DescribeDiscoverer after the delete", case.case),
            &emulator
                .call("GET", &discoverer_uri(&discoverer_id), None)
                .await,
        );
        assert_missing_discoverer(
            &format!("{}: ListTagsForResource after the delete", case.case),
            &emulator.call("GET", &tags_uri(&discoverer_arn), None).await,
        );
        assert_missing_discoverer(
            &format!("{}: StopDiscoverer after the delete", case.case),
            &emulator.call("POST", &stop_uri(&discoverer_id), None).await,
        );
        for repeat in 0..case.repeats {
            assert_missing_discoverer(
                &format!("{}: delete repeat {repeat}", case.case),
                &emulator
                    .call("DELETE", &discoverer_uri(&discoverer_id), None)
                    .await,
            );
        }

        // The source is undiscovered again, so the very create that was a conflict
        // while the discoverer existed now succeeds — with a NEW id, never the freed
        // one.
        let again = emulator
            .call("POST", DISCOVERERS_URI, Some(case.body()))
            .await;
        assert_eq!(
            again.status, 201,
            "case {:?}: the freed source must take a new discoverer, got {} {:?}",
            case.case, again.status, again.body
        );
        let new_id = again.str_at("DiscovererId");
        assert_ne!(
            new_id, discoverer_id,
            "case {:?}: a deleted id is never minted again, got {new_id:?}",
            case.case
        );
        assert!(
            minted.insert(new_id.clone()),
            "case {:?}: no two discoverers share an id, got {new_id:?}",
            case.case
        );
        assert_eq!(
            state_of(&again, case.case),
            "STARTED",
            "case {:?}: the replacement discoverer is created discovering, got {:?}",
            case.case,
            again.body
        );
        // And the replacement carries nothing of the deleted one: it is the resource
        // this create stored, read back through the reader.
        let described = emulator.call("GET", &discoverer_uri(&new_id), None).await;
        assert_eq!(
            described.str_at("SourceArn"),
            source_arn(case.bus),
            "case {:?}: the replacement discovers the same source, got {:?}",
            case.case,
            described.body
        );
        assert_eq!(
            described.str_at("DiscovererArn"),
            again.str_at("DiscovererArn"),
            "case {:?}: the replacement's ARN reads back as the create minted it",
            case.case
        );
    }
}

/// RULE: a delete removes the ONE discoverer its id names, so every other resource the
/// store holds — the other discoverers, and the registry and schema tree a discoverer
/// has nothing to do with — reads back unchanged across the call.
#[tokio::test]
async fn a_delete_leaves_every_other_resource_of_the_store_unchanged() {
    let emulator = Emulator::new();
    let case = "one discoverer deleted among registries, schemas and discoverers";

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

    // Three discoverers; the middle one is the one deleted.
    let mut ids = Vec::new();
    for bus in ["bus-a", "bus-b", "bus-c"] {
        ids.push((bus, create_discoverer(&emulator, case, bus).await));
    }
    let (_, deleted_id) = ids[1].clone();

    // Every resource that must survive the delete, as it stands before it.
    let mut snapshots: BTreeMap<String, Value> = BTreeMap::new();
    let survivors: Vec<(String, String)> = vec![
        ("registry".to_string(), registry_uri(registry)),
        ("schema".to_string(), schema_uri(registry, schema)),
        ("bus-a".to_string(), discoverer_uri(&ids[0].1)),
        ("bus-c".to_string(), discoverer_uri(&ids[2].1)),
    ];
    for (name, uri) in &survivors {
        let described = emulator.call("GET", uri, None).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: expected 200 reading {name}, got {} {:?}",
            described.status, described.body
        );
        snapshots.insert(name.clone(), described.body);
    }

    let deleted = emulator
        .call("DELETE", &discoverer_uri(&deleted_id), None)
        .await;
    assert_deleted(case, &deleted);
    assert_missing_discoverer(
        &format!("{case}: the deleted discoverer"),
        &emulator.call("GET", &discoverer_uri(&deleted_id), None).await,
    );

    for (name, uri) in &survivors {
        let described = emulator.call("GET", uri, None).await;
        assert_eq!(
            described.body, snapshots[name],
            "case {case:?}: {name} must read back unchanged across the delete; expected \
             {:?}, got {:?}",
            snapshots[name], described.body
        );
    }
}

/// RULE: a discoverer is named by the id the service minted for it, so an id this store
/// holds no discoverer for is refused as a missing resource and nothing is removed.
#[tokio::test]
async fn an_id_this_store_holds_no_discoverer_for_deletes_nothing() {
    let emulator = Emulator::new();
    let case = "a discoverer that exists";

    let existing = create_discoverer(&emulator, case, "present").await;
    let before = emulator.call("GET", &discoverer_uri(&existing), None).await;

    // Rows: ids of every shape a caller can send — a well-formed id nothing minted, a
    // value that is not of the minted form at all, and the existing id perturbed. The
    // last row replaces the existing id's first character with a letter OUTSIDE the hex
    // alphabet the service mints ids from, so it differs from that id whatever digits
    // the digest produced.
    for absent in [
        "0".repeat(32),
        "ffffffffffffffffffffffffffffffff".to_string(),
        "no-such-discoverer".to_string(),
        format!("{existing}x"),
        existing[1..].to_string(),
        format!("z{}", &existing[1..]),
    ] {
        let row = format!("DeleteDiscoverer on {absent:?}");
        let answer = emulator.call("DELETE", &discoverer_uri(&absent), None).await;
        assert_missing_discoverer(&row, &answer);
        let message = answer.str_at("Message");
        assert!(
            message.contains(&absent),
            "case {row:?}: the refusal names the id the request sent, got {message:?}"
        );
    }

    // And the discoverer that does exist survived every one of them, unchanged.
    let after = emulator.call("GET", &discoverer_uri(&existing), None).await;
    assert_eq!(
        after.body, before.body,
        "a refused delete must leave the stored discoverer as it was; expected {:?}, got {:?}",
        before.body, after.body
    );
}
