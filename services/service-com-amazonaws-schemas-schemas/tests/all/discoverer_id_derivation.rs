// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the IDENTITY of a discoverer: the id a create gives it and
//! the two sentences the discoverer paths refuse with.
//!
//! RULE: a discoverer IS the subscription to ONE event source, so the `DiscovererId` a
//! create gives it is DERIVED from the `SourceArn` the request named — the source ARN's
//! service namespace followed by the resource it names, with every separator inside the
//! resource rendered as `-`, and no other field of the ARN — so the id is a function of
//! the source alone: a caller can compute it, it is the same whatever else the store has
//! been asked for before or after, and it is the same again after the discoverer is
//! deleted and re-created. The refusals name that id and nothing else: a source already
//! discovered is refused "Discoverer «id» already exists." carrying the id of the
//! discoverer already held, and an id this store holds no discoverer for is refused
//! "Discoverer «id» does not exist." by every id-addressed discoverer operation.
//!
//! The rule is asserted over TABLES whose rows differ along what it generalises over:
//! the service namespace of the source, the depth of its resource path, which separator
//! that path uses, whether the partition, region and account fields are filled or blank,
//! the alphabet and length of the resource, and — for the refusals — which operation is
//! called and what shape the absent id has. The recorded `event-bus/default` source is
//! ONE row of it.
//!
//! Every case drives the real wire path: a signed `restJson1` request through
//! `MultiServiceRouter::dispatch`, exactly as the production router serves it. The HTTP
//! harness is the crate's one harness, imported from `registry_schema_lifecycle`; the
//! discoverer routes come from `discoverer_state`.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_schemas_schemas::state::{
    CODE_RESOURCE_ALREADY_EXIST, CODE_RESOURCE_NOT_FOUND,
};

use crate::discoverer_state::{discoverer_uri, source_arn, stop_uri, DISCOVERERS_URI};
use crate::registry_schema_lifecycle::{Emulator, ACCOUNT, REGION};

/// The id the rule says a source carries, computed here from the source ARN alone —
/// independently of the emulator, so a handler that derived it some other way would have
/// to disagree with this.
///
/// An ARN's six colon-separated fields are `arn`, the partition, the service, the region,
/// the account and the resource; the id is the service and the resource, with every
/// separator inside them rendered as `-`. A value with fewer fields carries no ARN
/// resource to read, so the whole value is rendered the same way.
pub(crate) fn expected_discoverer_id(source_arn: &str) -> String {
    let fields: Vec<&str> = source_arn.splitn(6, ':').collect();
    let derived = if fields.len() == 6 {
        format!("{}:{}", fields[2], fields[5])
    } else {
        source_arn.to_string()
    };
    derived.replace(['/', ':'], "-")
}

/// One source ARN, and the id the rule says the discoverer of it carries.
#[derive(Debug)]
struct SourceCase {
    case: &'static str,
    source_arn: String,
    /// The `service` field of the source ARN — the token the id must begin with.
    service: &'static str,
    /// The id this row's source derives, written out by hand.
    expected_id: String,
}

/// The table. Rows differ along the service namespace of the source, the depth of its
/// resource path, the separator that path uses (`/` and `:` both separate an ARN
/// resource), whether the partition, region and account fields are filled or blank, and
/// the alphabet and length of the resource — including a source at the published lower
/// bound of `__stringMin20Max1600` (len 20..1600, per
/// `smithy_query(query="operation", operation="CreateDiscoverer")`). The recorded
/// `event-bus/default` source is the first row.
fn source_cases() -> Vec<SourceCase> {
    vec![
        SourceCase {
            case: "an event bus, the form the trace records",
            source_arn: source_arn("default"),
            service: "events",
            expected_id: "events-event-bus-default".to_string(),
        },
        SourceCase {
            case: "an event bus in another partition, region and account",
            source_arn: "arn:aws-cn:events:cn-north-1:210987654321:event-bus/orders".to_string(),
            service: "events",
            expected_id: "events-event-bus-orders".to_string(),
        },
        SourceCase {
            case: "an event bus whose partition, region and account fields are blank",
            source_arn: "arn::events:::event-bus/global".to_string(),
            service: "events",
            expected_id: "events-event-bus-global".to_string(),
        },
        SourceCase {
            case: "a resource path of three segments",
            source_arn: format!("arn:aws:events:{REGION}:{ACCOUNT}:rule/orders/on-placed"),
            service: "events",
            expected_id: "events-rule-orders-on-placed".to_string(),
        },
        SourceCase {
            case: "a resource separated by a colon rather than a slash",
            source_arn: format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:processor"),
            service: "lambda",
            expected_id: "lambda-function-processor".to_string(),
        },
        SourceCase {
            case: "a resource with no type segment at all",
            source_arn: "arn:aws:s3:::reporting-bucket".to_string(),
            service: "s3",
            expected_id: "s3-reporting-bucket".to_string(),
        },
        SourceCase {
            case: "another service's queue",
            source_arn: format!("arn:aws:sqs:{REGION}:{ACCOUNT}:orders-queue"),
            service: "sqs",
            expected_id: "sqs-orders-queue".to_string(),
        },
        SourceCase {
            case: "a source at the modelled lower bound of 20 characters",
            source_arn: "arn:aws:events:::bbb".to_string(),
            service: "events",
            expected_id: "events-bbb".to_string(),
        },
        SourceCase {
            case: "a resource mixing case, dots and underscores",
            source_arn: format!("arn:aws:events:{REGION}:{ACCOUNT}:event-bus/Order.Placed_v1"),
            service: "events",
            expected_id: "events-event-bus-Order.Placed_v1".to_string(),
        },
        SourceCase {
            case: "a long resource name",
            source_arn: format!(
                "arn:aws:events:{REGION}:{ACCOUNT}:event-bus/{}",
                "b".repeat(200)
            ),
            service: "events",
            expected_id: format!("events-event-bus-{}", "b".repeat(200)),
        },
    ]
}

/// RULE (above): the id a create gives a discoverer is the one its source derives — the
/// source's service namespace and resource path, separators rendered as `-` — the ARN is
/// this service's own for that id, and the discoverer is readable under it.
#[tokio::test]
async fn the_id_a_create_gives_is_the_one_its_source_derives() {
    let emulator = Emulator::new();
    let cases = source_cases();
    let mut seen: BTreeMap<String, &str> = BTreeMap::new();

    for case in &cases {
        // The hand-written expectation and the independently computed one agree, so a
        // typo in the table cannot weaken the row.
        let expected = expected_discoverer_id(&case.source_arn);
        assert_eq!(
            expected, case.expected_id,
            "case {:?}: the table's expected id must be what the rule derives, got \
             {expected:?}",
            case.case
        );

        let created = emulator
            .call(
                "POST",
                DISCOVERERS_URI,
                Some(json!({"SourceArn": case.source_arn})),
            )
            .await;
        assert_eq!(
            created.status, 201,
            "case {:?}: expected 201 from CreateDiscoverer, got {} {:?}",
            case.case, created.status, created.body
        );
        let discoverer_id = created.str_at("DiscovererId");
        assert_eq!(
            discoverer_id, expected,
            "case {:?}: expected the derived id {expected:?}, got {discoverer_id:?}",
            case.case
        );

        // The id carries the source's SERVICE and RESOURCE and nothing else: no ARN
        // separator survives, and no partition, region or account field of the source
        // appears in it.
        assert!(
            discoverer_id.starts_with(case.service),
            "case {:?}: the id begins with the source's service token {:?}, got \
             {discoverer_id:?}",
            case.case,
            case.service
        );
        assert!(
            !discoverer_id.contains(':') && !discoverer_id.contains('/'),
            "case {:?}: the id renders every ARN separator as '-', got {discoverer_id:?}",
            case.case
        );

        // Distinct sources derive distinct ids, so the id is a handle on one resource.
        if let Some(other) = seen.insert(discoverer_id.clone(), case.case) {
            panic!(
                "case {:?}: {other:?} already derived {discoverer_id:?}; distinct \
                 sources must derive distinct ids",
                case.case
            );
        }

        assert_eq!(
            created.str_at("DiscovererArn"),
            format!("arn:aws:schemas:{REGION}:{ACCOUNT}:discoverer/{discoverer_id}"),
            "case {:?}: the ARN embeds the derived id, got {:?}",
            case.case,
            created.body.get("DiscovererArn")
        );

        // And the derived id is the handle the resource is readable under.
        let described = emulator
            .call("GET", &discoverer_uri(&discoverer_id), None)
            .await;
        assert_eq!(
            described.status, 200,
            "case {:?}: expected 200 from DescribeDiscoverer under the derived id, got \
             {} {:?}",
            case.case, described.status, described.body
        );
        assert_eq!(
            described.str_at("SourceArn"),
            case.source_arn,
            "case {:?}: the discoverer the derived id names discovers that source, got \
             {:?}",
            case.case,
            described.body
        );
    }
}

/// RULE (an invariant): the id is a function of the SOURCE alone, so it does not depend
/// on how many discoverers were created before it, on the order the creates came in, or
/// on the resource having been created once already and deleted.
#[tokio::test]
async fn the_derived_id_depends_on_the_source_and_on_nothing_else() {
    let cases = source_cases();

    // Created in the table's order, and again in the reverse order in a store of its
    // own: the two stores give every source the same id.
    let mut forward: BTreeMap<String, String> = BTreeMap::new();
    let emulator = Emulator::new();
    for case in &cases {
        let created = emulator
            .call(
                "POST",
                DISCOVERERS_URI,
                Some(json!({"SourceArn": case.source_arn})),
            )
            .await;
        assert_eq!(
            created.status, 201,
            "case {:?}: expected 201, got {} {:?}",
            case.case, created.status, created.body
        );
        forward.insert(case.source_arn.clone(), created.str_at("DiscovererId"));
    }

    let reversed = Emulator::new();
    for case in cases.iter().rev() {
        let created = reversed
            .call(
                "POST",
                DISCOVERERS_URI,
                Some(json!({"SourceArn": case.source_arn})),
            )
            .await;
        assert_eq!(
            created.status, 201,
            "case {:?}: expected 201 from the reversed store, got {} {:?}",
            case.case, created.status, created.body
        );
        let id = created.str_at("DiscovererId");
        assert_eq!(
            Some(&id),
            forward.get(&case.source_arn),
            "case {:?}: the create order must not change the id, got {id:?}",
            case.case
        );
    }

    // Deleted and asked for again: the same source gets the same id back.
    for case in &cases {
        let id = forward
            .get(&case.source_arn)
            .expect("every row was created above")
            .clone();
        let deleted = emulator.call("DELETE", &discoverer_uri(&id), None).await;
        assert_eq!(
            deleted.status, 204,
            "case {:?}: expected 204 from DeleteDiscoverer, got {} {:?}",
            case.case, deleted.status, deleted.body
        );
        let again = emulator
            .call(
                "POST",
                DISCOVERERS_URI,
                Some(json!({"SourceArn": case.source_arn})),
            )
            .await;
        assert_eq!(
            again.status, 201,
            "case {:?}: the freed source must take a discoverer again, got {} {:?}",
            case.case, again.status, again.body
        );
        assert_eq!(
            again.str_at("DiscovererId"),
            id,
            "case {:?}: a re-created discoverer carries the id its source derives, got \
             {:?}",
            case.case,
            again.body.get("DiscovererId")
        );
    }
}

/// RULE: a source this store already discovers is the same resource asked for twice, and
/// the conflict names the `DiscovererId` of the discoverer already held for it.
#[tokio::test]
async fn a_second_create_names_the_id_already_held_for_that_source() {
    for case in source_cases() {
        // One store per row, so the refusal is about this row's source alone.
        let emulator = Emulator::new();
        let created = emulator
            .call(
                "POST",
                DISCOVERERS_URI,
                Some(json!({"SourceArn": case.source_arn})),
            )
            .await;
        assert_eq!(
            created.status, 201,
            "case {:?}: expected 201, got {} {:?}",
            case.case, created.status, created.body
        );
        let held = created.str_at("DiscovererId");

        // The second create differs from the first in everything but the source: what
        // makes it a conflict is the identity it asks for.
        let again = emulator
            .call(
                "POST",
                DISCOVERERS_URI,
                Some(json!({
                    "SourceArn": case.source_arn,
                    "Description": "a second try",
                    "CrossAccount": false,
                    "tags": {"owner": "someone"},
                })),
            )
            .await;
        assert_eq!(
            again.status, 409,
            "case {:?}: expected 409, got {} {:?}",
            case.case, again.status, again.body
        );
        assert_eq!(
            again.error_type(),
            "ConflictException",
            "case {:?}: expected the modelled ConflictException, got {:?}",
            case.case,
            again.header("x-amzn-errortype")
        );
        assert_eq!(
            again.str_at("Code"),
            CODE_RESOURCE_ALREADY_EXIST,
            "case {:?}: expected Code {CODE_RESOURCE_ALREADY_EXIST}, got {:?}",
            case.case,
            again.body
        );
        assert_eq!(
            again.str_at("Message"),
            format!("Discoverer {held} already exists."),
            "case {:?}: the conflict names the id already held for the source, got {:?}",
            case.case,
            again.body.get("Message")
        );
    }
}

/// One discoverer operation addressed by a `DiscovererId` `@httpLabel`, with the method
/// and the body it is called with.
#[derive(Debug)]
struct IdAddressed {
    operation: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
}

/// Every discoverer operation addressed by a `DiscovererId` `@httpLabel`.
///
/// Source: `smithy_query(query="digest")` on this service's model — `DescribeDiscoverer`
/// `GET /v1/discoverers/id/{DiscovererId}`, `UpdateDiscoverer` `PUT` on the same path,
/// `StartDiscoverer` and `StopDiscoverer` `POST` on its `/start` and `/stop` children,
/// and `DeleteDiscoverer` `DELETE` on it; each models `NotFoundException` (HTTP 404).
fn id_addressed_operations(discoverer_id: &str) -> Vec<IdAddressed> {
    vec![
        IdAddressed {
            operation: "DescribeDiscoverer",
            method: "GET",
            uri: discoverer_uri(discoverer_id),
            body: None,
        },
        IdAddressed {
            operation: "UpdateDiscoverer",
            method: "PUT",
            uri: discoverer_uri(discoverer_id),
            body: Some(json!({"Description": "written nowhere"})),
        },
        IdAddressed {
            operation: "StartDiscoverer",
            method: "POST",
            uri: format!("{}/start", discoverer_uri(discoverer_id)),
            body: None,
        },
        IdAddressed {
            operation: "StopDiscoverer",
            method: "POST",
            uri: stop_uri(discoverer_id),
            body: None,
        },
        IdAddressed {
            operation: "DeleteDiscoverer",
            method: "DELETE",
            uri: discoverer_uri(discoverer_id),
            body: None,
        },
    ]
}

/// RULE: an id this store holds no discoverer for names no resource, so every
/// id-addressed discoverer operation refuses it as a missing resource (HTTP 404) with
/// the one sentence "Discoverer «id» does not exist.", built from the id the request
/// sent and from nothing else.
#[tokio::test]
async fn every_id_addressed_operation_refuses_an_absent_id_with_one_sentence() {
    let emulator = Emulator::new();

    // A discoverer that DOES exist, so the refusals are about the id named rather than
    // about an empty store.
    let present = source_arn("present");
    let created = emulator
        .call("POST", DISCOVERERS_URI, Some(json!({"SourceArn": present})))
        .await;
    assert_eq!(
        created.status, 201,
        "expected 201 from CreateDiscoverer, got {} {:?}",
        created.status, created.body
    );
    let existing = created.str_at("DiscovererId");

    // Rows: ids of every shape a caller can send that names nothing here — one of the
    // derived form for a source nothing subscribed, one for a source in this store but
    // under another service token, a 36-character opaque id, 32 hex digits, a bare word,
    // the existing id with a character added, with its first character dropped, and
    // upper-cased.
    let absent_ids = vec![
        expected_discoverer_id(&source_arn("never-created")),
        expected_discoverer_id("arn:aws:sqs:::present"),
        "00000000-0000-0000-0000-000000000000".to_string(),
        "0".repeat(32),
        "no-such-discoverer".to_string(),
        format!("{existing}x"),
        existing[1..].to_string(),
        existing.to_ascii_uppercase(),
    ];

    for absent in &absent_ids {
        for call in id_addressed_operations(absent) {
            let row = format!("{} on {absent:?}", call.operation);
            let answer = emulator.call(call.method, &call.uri, call.body).await;
            assert_eq!(
                answer.status, 404,
                "case {row:?}: expected 404, got {} {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_type(),
                "NotFoundException",
                "case {row:?}: expected the modelled NotFoundException, got {:?}",
                answer.header("x-amzn-errortype")
            );
            assert_eq!(
                answer.str_at("Code"),
                CODE_RESOURCE_NOT_FOUND,
                "case {row:?}: expected Code {CODE_RESOURCE_NOT_FOUND}, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.str_at("Message"),
                format!("Discoverer {absent} does not exist."),
                "case {row:?}: wrong not-found sentence, got {:?}",
                answer.body.get("Message")
            );
        }
    }

    // And none of those refusals touched the discoverer that does exist.
    let described = emulator.call("GET", &discoverer_uri(&existing), None).await;
    assert_eq!(
        described.status, 200,
        "a refused request must leave the stored discoverer readable, got {} {:?}",
        described.status, described.body
    );
    assert_eq!(
        described.str_at("SourceArn"),
        present,
        "a refused request must leave the stored discoverer's source alone, got {:?}",
        described.body
    );
}
