// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `UpdateDiscoverer`.
//!
//! RULE: an update writes the members its request carries into the stored resource the
//! id names and answers from the store, so the update's own answer and every later read
//! report one value: a member that carries a modelled default takes that default where
//! the request omits it, a member that carries none is left as it was, and the members
//! the request cannot name (the minted id, the minted ARN, the source and the state) are
//! left exactly as they were found. An id the store holds no resource under is refused
//! as a missing resource, and a request outside a modelled bound is refused and writes
//! nothing.
//!
//! The rule is asserted over TABLES whose rows differ along what it generalises over:
//! which members the request sends and which it omits, the value and length of
//! `Description` at each published bound of `__stringMin0Max256` and past it, the value
//! of `CrossAccount`, the number and order of updates applied to one discoverer, and the
//! state the discoverer is in when the update arrives (sources for the bounds and the
//! defaults: `smithy_query(query="operation", operation="UpdateDiscoverer")` and
//! `smithy_query(query="doc", shape="UpdateDiscovererRequest", member="CrossAccount")`).
//!
//! Every case drives the real wire path: a signed `restJson1` request through
//! `MultiServiceRouter::dispatch`, exactly as the production router serves it. The HTTP
//! harness is the crate's one harness, imported from `registry_schema_lifecycle`; the
//! discoverer routes come from `discoverer_state`.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_schemas_schemas::state::CODE_RESOURCE_NOT_FOUND;

use crate::discoverer_state::{discoverer_uri, source_arn, state_of, stop_uri, DISCOVERERS_URI};
use crate::registry_schema_lifecycle::{Answer, Emulator, ACCOUNT, REGION};

/// One `UpdateDiscoverer` request body: each member is sent, or omitted where the row
/// leaves it `None`.
#[derive(Debug, Clone)]
struct Update {
    description: Option<String>,
    cross_account: Option<bool>,
}

impl Update {
    fn body(&self) -> Value {
        let mut body = json!({});
        if let Some(description) = &self.description {
            body["Description"] = json!(description);
        }
        if let Some(cross_account) = self.cross_account {
            body["CrossAccount"] = json!(cross_account);
        }
        body
    }
}

/// Shorthand for a row's update, so the table reads as the members it sends.
fn update(description: Option<&str>, cross_account: Option<bool>) -> Update {
    Update {
        description: description.map(str::to_string),
        cross_account,
    }
}

/// One discoverer, and the sequence of updates applied to it.
#[derive(Debug)]
struct SequenceCase {
    case: &'static str,
    /// The bus this row's discoverer discovers; distinct per row, since one source
    /// carries at most one discoverer.
    bus: &'static str,
    /// The `Description` the create sends, or `None` to omit it.
    created_description: Option<String>,
    /// The `CrossAccount` the create sends, or `None` to omit it.
    created_cross_account: Option<bool>,
    /// Whether the discoverer is stopped before the updates arrive: an update is about
    /// the members it carries, so it must not move the state either way.
    stop_first: bool,
    updates: Vec<Update>,
}

/// The table. Rows differ along which members the update sends, the value and length of
/// `Description` (at both published bounds of `__stringMin0Max256`, len 0..256, and in
/// another alphabet), the value of `CrossAccount`, how many updates are applied and in
/// what order, whether an update repeats the value already stored, and the state the
/// discoverer is in when the update arrives.
fn sequence_cases() -> Vec<SequenceCase> {
    vec![
        SequenceCase {
            case: "both members sent onto a bare discoverer",
            bus: "update-both",
            created_description: None,
            created_cross_account: None,
            stop_first: false,
            updates: vec![update(Some("orders discoverer v2"), Some(false))],
        },
        SequenceCase {
            case: "only the description sent, so CrossAccount takes its default",
            bus: "update-description-only",
            created_description: Some("v1".to_string()),
            created_cross_account: Some(false),
            stop_first: false,
            updates: vec![update(Some("v2"), None)],
        },
        SequenceCase {
            case: "only CrossAccount sent, so the stored description is left alone",
            bus: "update-cross-account-only",
            created_description: Some("kept across the update".to_string()),
            created_cross_account: Some(true),
            stop_first: false,
            updates: vec![update(None, Some(false))],
        },
        SequenceCase {
            case: "an empty request body: no description change, CrossAccount defaulted",
            bus: "update-empty",
            created_description: Some("still here afterwards".to_string()),
            created_cross_account: Some(false),
            stop_first: false,
            updates: vec![update(None, None)],
        },
        SequenceCase {
            case: "a description at the modelled lower bound of 0",
            bus: "update-empty-description",
            created_description: Some("about to be emptied".to_string()),
            created_cross_account: None,
            stop_first: false,
            updates: vec![update(Some(""), Some(true))],
        },
        SequenceCase {
            case: "a description at the modelled upper bound of 256",
            bus: "update-long-description",
            created_description: None,
            created_cross_account: Some(false),
            stop_first: false,
            updates: vec![Update {
                description: Some("u".repeat(256)),
                cross_account: Some(false),
            }],
        },
        SequenceCase {
            case: "a description in another alphabet",
            bus: "update-non-ascii",
            created_description: Some("ascii".to_string()),
            created_cross_account: None,
            stop_first: false,
            updates: vec![update(
                Some("caf\u{e9} \u{2014} \u{30b9}\u{30ad}\u{30fc}\u{30de} \"v3\" 100%"),
                None,
            )],
        },
        SequenceCase {
            case: "the same update sent three times running",
            bus: "update-repeated",
            created_description: None,
            created_cross_account: None,
            stop_first: false,
            updates: vec![
                update(Some("idempotent"), Some(false)),
                update(Some("idempotent"), Some(false)),
                update(Some("idempotent"), Some(false)),
            ],
        },
        SequenceCase {
            case: "four updates in a row, each changing a different member",
            bus: "update-sequence",
            created_description: Some("first".to_string()),
            created_cross_account: Some(true),
            stop_first: false,
            updates: vec![
                update(Some("second"), Some(false)),
                update(None, Some(false)),
                update(Some("third"), None),
                update(None, None),
            ],
        },
        SequenceCase {
            case: "an update of a stopped discoverer does not restart it",
            bus: "update-stopped",
            created_description: None,
            created_cross_account: Some(false),
            stop_first: true,
            updates: vec![
                update(Some("updated while stopped"), Some(false)),
                update(None, None),
            ],
        },
    ]
}

/// The tags every discoverer in this suite is created with, so each case also proves an
/// update leaves the resource's tags where they were.
fn created_tags() -> BTreeMap<&'static str, &'static str> {
    BTreeMap::from([("env", "test"), ("team", "schemas")])
}

/// The `Description` a response reports, or `None` where the member is absent or null.
fn description_of(answer: &Answer) -> Option<&str> {
    answer.body.get("Description").and_then(Value::as_str)
}

/// The `CrossAccount` a response reports, or `None` where the member is absent.
fn cross_account_of(answer: &Answer) -> Option<bool> {
    answer.body.get("CrossAccount").and_then(Value::as_bool)
}

/// RULE: `UpdateDiscoverer` writes the members its request carries into the stored
/// discoverer and answers from the store — a sent `Description` replaces the stored one
/// and an omitted one changes nothing, an omitted `CrossAccount` takes its documented
/// default — so the update's answer and every later `DescribeDiscoverer` agree, while the
/// id, the ARN, the source, the state and the tags are left as they were.
#[tokio::test]
async fn an_update_stores_what_it_sends_and_answers_what_a_later_read_reports() {
    let emulator = Emulator::new();

    for case in sequence_cases() {
        let mut create_body = json!({"SourceArn": source_arn(case.bus), "tags": created_tags()});
        if let Some(description) = &case.created_description {
            create_body["Description"] = json!(description);
        }
        if let Some(cross_account) = case.created_cross_account {
            create_body["CrossAccount"] = json!(cross_account);
        }
        let created = emulator
            .call("POST", DISCOVERERS_URI, Some(create_body))
            .await;
        assert_eq!(
            created.status, 201,
            "case {:?}: expected 201 from CreateDiscoverer, got {} {:?}",
            case.case, created.status, created.body
        );
        let discoverer_id = created.str_at("DiscovererId");
        let expected_arn = format!("arn:aws:schemas:{REGION}:{ACCOUNT}:discoverer/{discoverer_id}");

        // The state the discoverer is in when the updates arrive — and the state every
        // one of them must leave it in, since an update names no state.
        let expected_state = if case.stop_first {
            let stopped = emulator.call("POST", &stop_uri(&discoverer_id), None).await;
            assert_eq!(
                stopped.status, 200,
                "case {:?}: expected 200 from StopDiscoverer, got {} {:?}",
                case.case, stopped.status, stopped.body
            );
            "STOPPED"
        } else {
            "STARTED"
        };

        // What the rule says the stored discoverer carries, carried forward across the
        // sequence: the create's values to begin with.
        let mut expected_description = case.created_description.clone();
        let mut expected_cross_account = case.created_cross_account.unwrap_or(true);

        // Where the sequence starts: what the create stored, so a later step's
        // expectation is a change from a value this read pinned.
        let before = emulator
            .call("GET", &discoverer_uri(&discoverer_id), None)
            .await;
        assert_eq!(
            description_of(&before),
            expected_description.as_deref(),
            "case {:?}: before any update the stored Description is {:?}, got {:?}",
            case.case,
            expected_description,
            before.body.get("Description")
        );
        assert_eq!(
            cross_account_of(&before),
            Some(expected_cross_account),
            "case {:?}: before any update the stored CrossAccount is {expected_cross_account}, \
             got {:?}",
            case.case,
            before.body.get("CrossAccount")
        );

        for (step, sent) in case.updates.iter().enumerate() {
            let row = format!("{} [step {step}]", case.case);
            let updated = emulator
                .call("PUT", &discoverer_uri(&discoverer_id), Some(sent.body()))
                .await;
            assert_eq!(
                updated.status, 200,
                "case {row:?}: expected 200 from UpdateDiscoverer, got {} {:?}",
                updated.status, updated.body
            );

            // The rule, applied to the expectation before the answer is read.
            if let Some(description) = &sent.description {
                expected_description = Some(description.clone());
            }
            expected_cross_account = sent.cross_account.unwrap_or(true);

            // The update's own answer, and the stored resource a later read reports,
            // must be the same discoverer.
            for (source, answer) in [
                ("the update's answer", &updated),
                (
                    "DescribeDiscoverer",
                    &emulator
                        .call("GET", &discoverer_uri(&discoverer_id), None)
                        .await,
                ),
            ] {
                assert_eq!(
                    description_of(answer),
                    expected_description.as_deref(),
                    "case {row:?}: {source} must report Description {:?}, got {:?}",
                    expected_description,
                    answer.body.get("Description")
                );
                assert_eq!(
                    cross_account_of(answer),
                    Some(expected_cross_account),
                    "case {row:?}: {source} must report CrossAccount {expected_cross_account}, \
                     got {:?}",
                    answer.body.get("CrossAccount")
                );
                assert_eq!(
                    answer.str_at("DiscovererId"),
                    discoverer_id,
                    "case {row:?}: {source} must name the discoverer the id named, got {:?}",
                    answer.body
                );
                assert_eq!(
                    answer.str_at("DiscovererArn"),
                    expected_arn,
                    "case {row:?}: {source} must report the ARN the create minted, got {:?}",
                    answer.body.get("DiscovererArn")
                );
                assert_eq!(
                    answer.str_at("SourceArn"),
                    source_arn(case.bus),
                    "case {row:?}: {source} must report the source the create named, got {:?}",
                    answer.body.get("SourceArn")
                );
                assert_eq!(
                    state_of(answer, &row),
                    expected_state,
                    "case {row:?}: {source} must report the state the update left \
                     ({expected_state}), got {:?}",
                    answer.body
                );
                let expected_tags: Value = json!(created_tags());
                assert_eq!(
                    answer.value_at("tags"),
                    &expected_tags,
                    "case {row:?}: {source} must report the tags the create stored \
                     {expected_tags:?}, got {:?}",
                    answer.body.get("tags")
                );
            }
        }
    }
}

/// RULE: an update names its resource by the id the service minted, so an id this store
/// holds no discoverer under names nothing to update and is refused as a missing resource
/// (HTTP 404, the service's one not-found code), writing nothing.
#[tokio::test]
async fn an_id_nothing_created_is_refused_as_a_missing_resource() {
    let emulator = Emulator::new();

    // A discoverer that DOES exist, so each refusal below is about the id named and not
    // about an empty store — and so the "writing nothing" half of the rule has something
    // to be asserted against.
    let existing = emulator
        .call(
            "POST",
            DISCOVERERS_URI,
            Some(json!({"SourceArn": source_arn("present"), "Description": "untouched"})),
        )
        .await;
    assert_eq!(
        existing.status, 201,
        "expected 201 from CreateDiscoverer, got {} {:?}",
        existing.status, existing.body
    );
    let existing_id = existing.str_at("DiscovererId");

    // Rows: ids of every shape a caller can send — a well-formed id nothing minted, a
    // value not of the minted form at all, and the existing id perturbed.
    for absent in [
        "0".repeat(32),
        "ffffffffffffffffffffffffffffffff".to_string(),
        "no-such-discoverer".to_string(),
        format!("{existing_id}x"),
        existing_id[1..].to_string(),
        existing_id.to_ascii_uppercase(),
    ] {
        let answer = emulator
            .call(
                "PUT",
                &discoverer_uri(&absent),
                Some(json!({"Description": "written nowhere", "CrossAccount": false})),
            )
            .await;
        assert_eq!(
            answer.status, 404,
            "case {absent:?}: expected 404, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.str_at("Code"),
            CODE_RESOURCE_NOT_FOUND,
            "case {absent:?}: expected Code {CODE_RESOURCE_NOT_FOUND}, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.error_type(),
            "NotFoundException",
            "case {absent:?}: expected the modelled NotFoundException, got {:?}",
            answer.header("x-amzn-errortype")
        );
    }

    // And the discoverer that does exist was written by none of it.
    let described = emulator.call("GET", &discoverer_uri(&existing_id), None).await;
    assert_eq!(
        description_of(&described),
        Some("untouched"),
        "a refused update must not write another discoverer's Description, got {:?}",
        described.body.get("Description")
    );
}

/// One update whose `Description` sits at, or past, the modelled bound.
#[derive(Debug)]
struct BoundCase {
    case: &'static str,
    description: String,
    /// 200 for a length the model admits, 400 for one it does not.
    expected_status: u16,
}

/// The table. Rows differ along the length of `Description` — at each published bound of
/// `__stringMin0Max256` (len 0..256, per `smithy_query(query="operation",
/// operation="UpdateDiscoverer")`), one character past the upper one, and far past it.
fn bound_cases() -> Vec<BoundCase> {
    vec![
        BoundCase {
            case: "at the modelled lower bound of 0",
            description: String::new(),
            expected_status: 200,
        },
        BoundCase {
            case: "one character above the lower bound",
            description: "x".to_string(),
            expected_status: 200,
        },
        BoundCase {
            case: "one character below the modelled upper bound",
            description: "u".repeat(255),
            expected_status: 200,
        },
        BoundCase {
            case: "at the modelled upper bound of 256",
            description: "m".repeat(256),
            expected_status: 200,
        },
        BoundCase {
            case: "one character past the upper bound",
            description: "o".repeat(257),
            expected_status: 400,
        },
        BoundCase {
            case: "far past the upper bound",
            description: "f".repeat(4_096),
            expected_status: 400,
        },
    ]
}

/// RULE: a request outside a modelled bound is refused with HTTP 400 before anything is
/// written, so the stored discoverer is left exactly as the last admitted write left it —
/// its description AND the `CrossAccount` an admitted request would have defaulted.
#[tokio::test]
async fn a_description_past_the_modelled_bound_is_refused_and_writes_nothing() {
    let emulator = Emulator::new();
    let created = emulator
        .call(
            "POST",
            DISCOVERERS_URI,
            Some(json!({
                "SourceArn": source_arn("bounds"),
                "Description": "the last admitted value",
                "CrossAccount": false,
            })),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "expected 201 from CreateDiscoverer, got {} {:?}",
        created.status, created.body
    );
    let discoverer_id = created.str_at("DiscovererId");

    // What the store holds, carried forward: only an admitted update moves it.
    let mut stored_description = "the last admitted value".to_string();
    let mut stored_cross_account = false;

    for case in bound_cases() {
        let answer = emulator
            .call(
                "PUT",
                &discoverer_uri(&discoverer_id),
                Some(json!({"Description": case.description})),
            )
            .await;
        assert_eq!(
            answer.status, case.expected_status,
            "case {:?}: expected {}, got {} {:?}",
            case.case, case.expected_status, answer.status, answer.body
        );
        if case.expected_status == 200 {
            stored_description = case.description.clone();
            // The admitted request omits `CrossAccount`, which therefore takes its
            // documented default.
            stored_cross_account = true;
        } else {
            assert_eq!(
                answer.error_type(),
                "ValidationException",
                "case {:?}: a value outside a modelled bound is refused by the boundary \
                 validator, got {:?}",
                case.case,
                answer.header("x-amzn-errortype")
            );
        }

        let described = emulator
            .call("GET", &discoverer_uri(&discoverer_id), None)
            .await;
        assert_eq!(
            description_of(&described),
            Some(stored_description.as_str()),
            "case {:?}: expected the stored Description to be {} characters, got {:?}",
            case.case,
            stored_description.len(),
            description_of(&described).map(str::len)
        );
        assert_eq!(
            cross_account_of(&described),
            Some(stored_cross_account),
            "case {:?}: expected the stored CrossAccount {stored_cross_account}, got {:?}",
            case.case,
            described.body.get("CrossAccount")
        );
    }
}

/// RULE: an update is about ONE discoverer, so updating one leaves every other
/// discoverer's members exactly as they were.
#[tokio::test]
async fn updating_one_discoverer_leaves_the_others_alone() {
    let emulator = Emulator::new();
    let case = "three discoverers, one updated";

    let mut ids = Vec::new();
    for bus in ["neighbour-a", "neighbour-b", "neighbour-c"] {
        let created = emulator
            .call(
                "POST",
                DISCOVERERS_URI,
                Some(json!({
                    "SourceArn": source_arn(bus),
                    "Description": bus,
                    "CrossAccount": false,
                })),
            )
            .await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected 201 for {bus}, got {} {:?}",
            created.status, created.body
        );
        ids.push((bus, created.str_at("DiscovererId")));
    }

    let (_, middle) = &ids[1];
    let updated = emulator
        .call(
            "PUT",
            &discoverer_uri(middle),
            Some(json!({"Description": "only this one moved"})),
        )
        .await;
    assert_eq!(
        updated.status, 200,
        "case {case:?}: expected 200, got {} {:?}",
        updated.status, updated.body
    );

    for (index, (bus, id)) in ids.iter().enumerate() {
        let described = emulator.call("GET", &discoverer_uri(id), None).await;
        let (expected_description, expected_cross_account) = if index == 1 {
            // The updated one: the description it was sent, and the default its update
            // omitted.
            ("only this one moved", true)
        } else {
            (*bus, false)
        };
        assert_eq!(
            description_of(&described),
            Some(expected_description),
            "case {case:?}: {bus} must read Description {expected_description:?}, got {:?}",
            described.body.get("Description")
        );
        assert_eq!(
            cross_account_of(&described),
            Some(expected_cross_account),
            "case {case:?}: {bus} must read CrossAccount {expected_cross_account}, got {:?}",
            described.body.get("CrossAccount")
        );
    }
}
