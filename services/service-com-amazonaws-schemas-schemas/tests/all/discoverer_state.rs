// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the discoverer lifecycle: the rule that a discoverer's
//! `State` is a STORED property of the resource, which `StartDiscoverer` sets to
//! `STARTED` and `StopDiscoverer` sets to `STOPPED`, so every later read of that
//! discoverer — the start/stop answer itself and `DescribeDiscoverer` — reports what
//! the last write left, and a write to one discoverer leaves every other alone.
//!
//! The rule is asserted over a TABLE of start/stop SEQUENCES (their length, their
//! order, and a repeat of the state the discoverer is already in), and the create
//! round-trip over a table of request bodies — so a handler that answered a constant,
//! or answered from its input instead of from the store, would have to make a later
//! read agree.
//!
//! Every case drives the real wire path: a signed `restJson1` request through
//! `MultiServiceRouter::dispatch`, exactly as the production router serves it. The HTTP
//! harness is the crate's one harness, imported from `registry_schema_lifecycle`.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use service_com_amazonaws_schemas_schemas::ops::types::DISCOVERER_STATE_VALUES;
use service_com_amazonaws_schemas_schemas::state::{
    CODE_MALFORMED_REQUEST, CODE_RESOURCE_ALREADY_EXIST, CODE_RESOURCE_NOT_FOUND,
};

use crate::registry_schema_lifecycle::{Answer, Emulator, ACCOUNT, REGION};

/// `CreateDiscoverer`'s route (`POST /v1/discoverers -> 201`).
pub(crate) const DISCOVERERS_URI: &str = "/v1/discoverers";

/// `DescribeDiscoverer`'s route.
pub(crate) fn discoverer_uri(discoverer_id: &str) -> String {
    format!("/v1/discoverers/id/{discoverer_id}")
}

/// `StartDiscoverer`'s route.
fn start_uri(discoverer_id: &str) -> String {
    format!("{}/start", discoverer_uri(discoverer_id))
}

/// `StopDiscoverer`'s route.
pub(crate) fn stop_uri(discoverer_id: &str) -> String {
    format!("{}/stop", discoverer_uri(discoverer_id))
}

/// An event-bus ARN to discover from. One per case, because a source may be discovered
/// by at most one discoverer.
pub(crate) fn source_arn(bus: &str) -> String {
    format!("arn:aws:events:{REGION}:{ACCOUNT}:event-bus/{bus}")
}

/// Create a discoverer for `bus`, asserting the create succeeded, and return its
/// minted `DiscovererId`.
pub(crate) async fn create_discoverer(emulator: &Emulator, case: &str, bus: &str) -> String {
    let created = emulator
        .call(
            "POST",
            DISCOVERERS_URI,
            Some(json!({"SourceArn": source_arn(bus)})),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "case {case:?}: expected 201 from CreateDiscoverer, got {} {:?}",
        created.status, created.body
    );
    created.str_at("DiscovererId")
}

/// The `State` a response reports.
pub(crate) fn state_of(answer: &Answer, case: &str) -> String {
    let state = answer.str_at("State");
    assert!(
        DISCOVERER_STATE_VALUES.contains(&state.as_str()),
        "case {case:?}: State must be one of {DISCOVERER_STATE_VALUES:?}, got {state:?}"
    );
    state
}

/// One call of the start/stop pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Move {
    Start,
    Stop,
}

impl Move {
    /// The route this call is served on.
    pub(crate) fn uri(self, discoverer_id: &str) -> String {
        match self {
            Self::Start => start_uri(discoverer_id),
            Self::Stop => stop_uri(discoverer_id),
        }
    }

    /// The state the discoverer is in once this call has been answered — one of
    /// [`DISCOVERER_STATE_VALUES`], whatever state it was in before.
    pub(crate) fn leaves(self) -> &'static str {
        match self {
            Self::Start => "STARTED",
            Self::Stop => "STOPPED",
        }
    }
}

/// One sequence of start/stop calls applied to a freshly created discoverer.
#[derive(Debug)]
struct SequenceCase {
    case: &'static str,
    /// The bus whose discoverer this row drives; distinct per row, since one source
    /// carries one discoverer.
    bus: &'static str,
    moves: Vec<Move>,
}

/// The table. Rows differ along what the rule generalises over: how many calls are
/// made, which call they are, the order they come in, and whether a call repeats the
/// state the discoverer is already in.
fn sequence_cases() -> Vec<SequenceCase> {
    use Move::{Start, Stop};
    vec![
        SequenceCase {
            case: "no call at all: a created discoverer is already discovering",
            bus: "untouched",
            moves: vec![],
        },
        SequenceCase {
            case: "start on a discoverer that is already started",
            bus: "start-once",
            moves: vec![Start],
        },
        SequenceCase {
            case: "stop once",
            bus: "stop-once",
            moves: vec![Stop],
        },
        SequenceCase {
            case: "stop then start",
            bus: "stop-then-start",
            moves: vec![Stop, Start],
        },
        SequenceCase {
            case: "start repeated three times",
            bus: "start-thrice",
            moves: vec![Start, Start, Start],
        },
        SequenceCase {
            case: "stop repeated twice",
            bus: "stop-twice",
            moves: vec![Stop, Stop],
        },
        SequenceCase {
            case: "alternating, ending started",
            bus: "alternating-started",
            moves: vec![Stop, Start, Stop, Start],
        },
        SequenceCase {
            case: "alternating, ending stopped",
            bus: "alternating-stopped",
            moves: vec![Start, Stop, Start, Stop],
        },
        SequenceCase {
            case: "stopped after a run of starts",
            bus: "starts-then-stop",
            moves: vec![Start, Start, Stop],
        },
    ]
}

/// RULE: a discoverer's `State` is a stored property of the resource. `StartDiscoverer`
/// sets it to `STARTED` and `StopDiscoverer` to `STOPPED` — whatever state the
/// discoverer was in, and however often the call is repeated — and every later read of
/// that discoverer reports the state the last write left.
#[tokio::test]
async fn start_and_stop_store_the_state_every_later_read_reports() {
    let emulator = Emulator::new();

    for case in sequence_cases() {
        let discoverer_id = create_discoverer(&emulator, case.case, case.bus).await;

        // A created discoverer discovers from the moment it exists.
        let mut expected = "STARTED";
        let described = emulator
            .call("GET", &discoverer_uri(&discoverer_id), None)
            .await;
        assert_eq!(
            state_of(&described, case.case),
            expected,
            "case {:?}: a created discoverer reads back {expected}, got {:?}",
            case.case,
            described.body
        );

        for (step, movement) in case.moves.iter().enumerate() {
            let row = format!("{} [step {step}: {movement:?}]", case.case);
            let answer = emulator
                .call("POST", &movement.uri(&discoverer_id), None)
                .await;
            assert_eq!(
                answer.status, 200,
                "case {row:?}: expected 200, got {} {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.str_at("DiscovererId"),
                discoverer_id,
                "case {row:?}: the answer names the discoverer it acted on, got {:?}",
                answer.body
            );
            expected = movement.leaves();
            assert_eq!(
                state_of(&answer, &row),
                expected,
                "case {row:?}: expected the answer to report {expected}, got {:?}",
                answer.body
            );

            // And the stored resource agrees with the answer, at every step.
            let described = emulator
                .call("GET", &discoverer_uri(&discoverer_id), None)
                .await;
            assert_eq!(
                state_of(&described, &row),
                expected,
                "case {row:?}: DescribeDiscoverer must report the stored {expected}, got {:?}",
                described.body
            );
        }

        // A read does not move the state: reading twice more reports the same value.
        for repeat in 0..2 {
            let described = emulator
                .call("GET", &discoverer_uri(&discoverer_id), None)
                .await;
            assert_eq!(
                state_of(&described, case.case),
                expected,
                "case {:?}: read {repeat} after the sequence must still report \
                 {expected}, got {:?}",
                case.case,
                described.body
            );
        }
    }
}

/// RULE: the state is a property of ONE discoverer, so starting or stopping one leaves
/// every other discoverer's state exactly as it was.
#[tokio::test]
async fn starting_one_discoverer_leaves_the_others_alone() {
    let emulator = Emulator::new();
    let case = "three discoverers, one started";

    let mut ids = Vec::new();
    for bus in ["bus-a", "bus-b", "bus-c"] {
        ids.push((bus, create_discoverer(&emulator, case, bus).await));
    }

    // Stop them all, then start the middle one only.
    for (bus, id) in &ids {
        let stopped = emulator.call("POST", &stop_uri(id), None).await;
        assert_eq!(
            state_of(&stopped, bus),
            "STOPPED",
            "case {case:?}: {bus} must stop, got {:?}",
            stopped.body
        );
    }
    let (_, middle) = &ids[1];
    let started = emulator.call("POST", &start_uri(middle), None).await;
    assert_eq!(
        state_of(&started, case),
        "STARTED",
        "case {case:?}: the started discoverer reports STARTED, got {:?}",
        started.body
    );

    for (index, (bus, id)) in ids.iter().enumerate() {
        let expected = if index == 1 { "STARTED" } else { "STOPPED" };
        let described = emulator.call("GET", &discoverer_uri(id), None).await;
        assert_eq!(
            state_of(&described, bus),
            expected,
            "case {case:?}: {bus} must still read {expected}, got {:?}",
            described.body
        );
    }
}

/// One `CreateDiscoverer` request body and what the stored discoverer must then be.
#[derive(Debug)]
struct CreateCase {
    case: &'static str,
    /// The `SourceArn` the request names; distinct per row.
    source_arn: String,
    description: Option<String>,
    /// The `CrossAccount` the request sends, or `None` to omit the member.
    cross_account: Option<bool>,
    tags: BTreeMap<&'static str, &'static str>,
}

impl CreateCase {
    /// The request body this row sends. `Tags` travels as `tags`
    /// (`smithy_query(query="operation", operation="CreateDiscoverer")`: the member
    /// carries `jsonName: "tags"`).
    fn body(&self) -> Value {
        let mut body = json!({"SourceArn": self.source_arn});
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

    /// The `CrossAccount` the stored discoverer carries: what the request sent, or
    /// `true` where it sent nothing (`smithy_query(query="doc",
    /// shape="CreateDiscovererRequest", member="CrossAccount")`: "(default: true)").
    fn expected_cross_account(&self) -> bool {
        self.cross_account.unwrap_or(true)
    }
}

/// The table. Rows differ along what the round-trip rule generalises over: the form of
/// the source ARN (partition, region and account fields, and the resource path), the
/// presence and size of `Description` at the published bounds of `__stringMin0Max256`
/// (len 0..256 — source: `smithy_query(query="operation",
/// operation="CreateDiscoverer")`), the presence and value of `CrossAccount`, and how
/// many tags the request carries.
fn create_cases() -> Vec<CreateCase> {
    vec![
        CreateCase {
            case: "nothing but the required source",
            source_arn: source_arn("plain"),
            description: None,
            cross_account: None,
            tags: BTreeMap::new(),
        },
        CreateCase {
            case: "described, cross-account off, tagged",
            source_arn: source_arn("described"),
            description: Some("orders bus discoverer".to_string()),
            cross_account: Some(false),
            tags: BTreeMap::from([("env", "test"), ("team", "schemas")]),
        },
        CreateCase {
            case: "empty description at the modelled lower bound",
            source_arn: source_arn("empty-description"),
            description: Some(String::new()),
            cross_account: Some(true),
            tags: BTreeMap::from([("only", "one")]),
        },
        CreateCase {
            case: "description at the modelled upper bound",
            source_arn: source_arn("long-description"),
            description: Some("d".repeat(256)),
            cross_account: None,
            tags: BTreeMap::new(),
        },
        CreateCase {
            case: "a source in another partition, region and account",
            source_arn: "arn:aws-cn:events:cn-north-1:210987654321:event-bus/other".to_string(),
            description: Some("caf\u{e9} \u{2014} \u{30b9}\u{30ad}\u{30fc}\u{30de}".to_string()),
            cross_account: Some(true),
            tags: BTreeMap::new(),
        },
        CreateCase {
            case: "a source ARN whose region and account fields are empty",
            source_arn: "arn:aws:events:::event-bus/global-events-bus".to_string(),
            description: None,
            cross_account: Some(false),
            tags: BTreeMap::from([("a", ""), ("b", "2"), ("c", "3")]),
        },
    ]
}

/// RULE: a create answers from the resource it stored, so every member of the created
/// discoverer reads back unchanged through `DescribeDiscoverer`; the id and the ARN are
/// minted by the service (never taken from the request), and a created discoverer is
/// `STARTED`.
#[tokio::test]
async fn a_created_discoverer_reads_back_exactly_as_it_was_stored() {
    let emulator = Emulator::new();
    let mut minted: BTreeSet<String> = BTreeSet::new();

    for case in create_cases() {
        let created = emulator
            .call("POST", DISCOVERERS_URI, Some(case.body()))
            .await;
        assert_eq!(
            created.status, 201,
            "case {:?}: expected 201, got {} {:?}",
            case.case, created.status, created.body
        );

        let discoverer_id = created.str_at("DiscovererId");
        assert!(
            discoverer_id.len() == 32 && discoverer_id.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "case {:?}: the service mints an opaque id, got {discoverer_id:?}",
            case.case
        );
        assert!(
            minted.insert(discoverer_id.clone()),
            "case {:?}: no two discoverers share an id, got {discoverer_id:?}",
            case.case
        );
        assert_eq!(
            created.str_at("DiscovererArn"),
            format!("arn:aws:schemas:{REGION}:{ACCOUNT}:discoverer/{discoverer_id}"),
            "case {:?}: the ARN names this service, the signed region and the minted id",
            case.case
        );
        assert_eq!(
            state_of(&created, case.case),
            "STARTED",
            "case {:?}: a created discoverer is discovering, got {:?}",
            case.case,
            created.body
        );

        let described = emulator
            .call("GET", &discoverer_uri(&discoverer_id), None)
            .await;
        assert_eq!(
            described.status, 200,
            "case {:?}: expected 200 from DescribeDiscoverer, got {} {:?}",
            case.case, described.status, described.body
        );
        for member in ["DiscovererId", "DiscovererArn", "SourceArn", "State"] {
            assert_eq!(
                described.str_at(member),
                created.str_at(member),
                "case {:?}: {member} must read back as the create stored it",
                case.case
            );
        }
        assert_eq!(
            described.str_at("SourceArn"),
            case.source_arn,
            "case {:?}: SourceArn must be the source the request named",
            case.case
        );
        assert_eq!(
            described.body.get("Description").and_then(Value::as_str),
            case.description.as_deref(),
            "case {:?}: expected Description {:?}, got {:?}",
            case.case,
            case.description,
            described.body.get("Description")
        );
        assert_eq!(
            described.body.get("CrossAccount").and_then(Value::as_bool),
            Some(case.expected_cross_account()),
            "case {:?}: expected CrossAccount {}, got {:?}",
            case.case,
            case.expected_cross_account(),
            described.body.get("CrossAccount")
        );
        let expected_tags: Value = json!(case.tags);
        assert_eq!(
            described.value_at("tags"),
            &expected_tags,
            "case {:?}: expected the stored tags {expected_tags:?}, got {:?}",
            case.case,
            described.body.get("tags")
        );
    }
}

/// RULE: every discoverer operation names its discoverer by the id the service minted,
/// so an id this store holds no discoverer for is refused as a missing resource (HTTP
/// 404, the service's one not-found code) by each of them.
#[tokio::test]
async fn an_id_nothing_created_is_refused_as_a_missing_resource() {
    let emulator = Emulator::new();

    // A discoverer that DOES exist, so the refusals below are about the id named and
    // not about an empty store.
    let existing = create_discoverer(&emulator, "a discoverer that exists", "present").await;

    // Rows: ids of every shape a caller can send — a well-formed id nothing minted, a
    // value that is not of the minted form at all, and the existing id perturbed.
    for absent in [
        "0".repeat(32),
        "ffffffffffffffffffffffffffffffff".to_string(),
        "no-such-discoverer".to_string(),
        format!("{existing}x"),
        existing[1..].to_string(),
        existing.to_ascii_uppercase(),
    ] {
        for (operation, method, uri) in [
            ("StartDiscoverer", "POST", start_uri(&absent)),
            ("StopDiscoverer", "POST", stop_uri(&absent)),
            ("DescribeDiscoverer", "GET", discoverer_uri(&absent)),
        ] {
            let row = format!("{operation} on {absent:?}");
            let answer = emulator.call(method, &uri, None).await;
            assert_eq!(
                answer.status, 404,
                "case {row:?}: expected 404, got {} {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.str_at("Code"),
                CODE_RESOURCE_NOT_FOUND,
                "case {row:?}: expected Code {CODE_RESOURCE_NOT_FOUND}, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.error_type(),
                "NotFoundException",
                "case {row:?}: expected the modelled NotFoundException, got {:?}",
                answer.header("x-amzn-errortype")
            );
        }
    }

    // And the discoverer that does exist was untouched by any of it.
    let described = emulator.call("GET", &discoverer_uri(&existing), None).await;
    assert_eq!(
        state_of(&described, "a discoverer that exists"),
        "STARTED",
        "a refused request must not move another discoverer's state, got {:?}",
        described.body
    );
}

/// RULE: a discoverer IS the subscription to one event source, named by that source's
/// ARN — so a source that is not an ARN names no source to discover (HTTP 400), and a
/// source this store already discovers is a resource that already exists (HTTP 409).
#[tokio::test]
async fn a_source_must_be_an_arn_and_may_carry_one_discoverer() {
    let emulator = Emulator::new();

    // Rows: each way a value of a length the model accepts (`__stringMin20Max1600`)
    // still fails to name an event source.
    for not_an_arn in [
        "not-an-arn-at-all-but-long-enough",
        "https://events.example.invalid/bus/default",
        "arn:aws:events:us-west-2:210987654321",
        "arn:aws::us-west-2:210987654321:event-bus/no-service",
        "arn:aws:events:us-west-2:210987654321:",
    ] {
        let answer = emulator
            .call(
                "POST",
                DISCOVERERS_URI,
                Some(json!({"SourceArn": not_an_arn})),
            )
            .await;
        assert_eq!(
            answer.status, 400,
            "case {not_an_arn:?}: expected 400, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.str_at("Code"),
            CODE_MALFORMED_REQUEST,
            "case {not_an_arn:?}: expected Code {CODE_MALFORMED_REQUEST}, got {:?}",
            answer.body
        );
        let message = answer.str_at("Message");
        assert!(
            message.contains("SourceArn") && !message.contains(not_an_arn),
            "case {not_an_arn:?}: the refusal names the member, never the value sent, \
             got {message:?}"
        );
    }

    // The same source asked for twice, in every state the first discoverer can be in:
    // the second create is a conflict either way, and the first discoverer is left as
    // it was.
    for (case, stop_first) in [("while started", false), ("while stopped", true)] {
        let bus = if stop_first { "twice-stopped" } else { "twice" };
        let first = create_discoverer(&emulator, case, bus).await;
        let expected = if stop_first {
            emulator.call("POST", &stop_uri(&first), None).await;
            "STOPPED"
        } else {
            "STARTED"
        };

        let again = emulator
            .call(
                "POST",
                DISCOVERERS_URI,
                Some(json!({"SourceArn": source_arn(bus), "Description": "a second try"})),
            )
            .await;
        assert_eq!(
            again.status, 409,
            "case {case:?}: expected 409, got {} {:?}",
            again.status, again.body
        );
        assert_eq!(
            again.str_at("Code"),
            CODE_RESOURCE_ALREADY_EXIST,
            "case {case:?}: expected Code {CODE_RESOURCE_ALREADY_EXIST}, got {:?}",
            again.body
        );

        let described = emulator.call("GET", &discoverer_uri(&first), None).await;
        assert_eq!(
            state_of(&described, case),
            expected,
            "case {case:?}: the refused create must leave the discoverer at {expected}, \
             got {:?}",
            described.body
        );
        assert_eq!(
            described.body.get("Description").and_then(Value::as_str),
            None,
            "case {case:?}: the refused create must not write its Description, got {:?}",
            described.body.get("Description")
        );
    }
}
