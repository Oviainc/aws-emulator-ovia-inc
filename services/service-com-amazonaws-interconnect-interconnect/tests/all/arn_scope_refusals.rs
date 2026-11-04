// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for how the ARN-ADDRESSED operations answer an
//! `AmazonResourceName` they cannot resolve, driven over the wire through the
//! shared [`crate::emulator`] harness.
//!
//! RULE under test: an operation whose `arn` member IS the resource it acts on splits
//! the two ways that ARN can fail to address one. An ARN whose PARTITION or ACCOUNT is
//! not the one the request addresses names a resource the caller is not talking to at
//! all: a fault in the request, HTTP 400 `InterconnectValidationException`, whichever
//! of the three operations carried it, worded with the ARN field that differs, the
//! value the ARN carried for it, the caller's own value and the name of the input
//! shape the ARN arrived in. An ARN of that scope which resolves to no
//! resource is not a malformed request at all: the two tag WRITES refuse it as this
//! service's own client fault (HTTP 400 `InterconnectClientException`, worded with a
//! fixed sentence carrying nothing of the ARN), and the `@readonly` LISTING answers
//! HTTP 200 with the empty tag set. Neither class is ever the connection-scoped 404.
//!
//! The operations covered are every one whose input addresses its resource by
//! `AmazonResourceName`: `ListTagsForResource`, `TagResource` and `UntagResource`
//! (`smithy_query(query="operation",operation="ListTagsForResource,TagResource,UntagResource")`
//! — each takes `arn` -> `AmazonResourceName`, `@length 59..150`, `@pattern
//! ^arn:aws[a-z-]*:interconnect:[^:]+:[0-9]{12}:connection/(mcc|lmcc)-[a-z0-9]{8}$`,
//! and each models `InterconnectClientException` (400) and
//! `InterconnectValidationException` (400)).
//!
//! Every assertion is about that rule over a TABLE of ARNs whose rows differ in
//! WHICH field puts the ARN out of the request's scope (partition, account, both at
//! once), in the value of that field, in the Region segment the ARN carries (which
//! puts nothing out of scope, since the member leaves it unconstrained), in the id
//! prefix (`mcc`/`lmcc`), and in the ARN's overall length (both ends of the modelled
//! 59..150 bound) — crossed with all three operations.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_interconnect_interconnect::state::{
    arn_resource_absent_message, parse_connection_arn,
};

use crate::emulator::{
    code, create_with_tags, message, placements, tags_of, text, Emulator, SIGNING_NAME,
};

/// The account id every ARN this emulator mints carries, and so the account every
/// request it serves is for.
const ACCOUNT_ID: &str = engine::EMULATOR_ACCOUNT_ID;

/// The Region the shared harness signs its requests for, so the Region every ARN it
/// mints carries.
const REGION: &str = "us-east-1";

/// The partition every ARN this emulator mints carries.
const PARTITION: &str = "aws";

/// `AmazonResourceName` is modelled `@length(min: 59, max: 150)`
/// (`smithy_query(query="shape",shape="AmazonResourceName")`), so a row at either
/// end of that span is still an ARN the boundary validator passes to the handler.
const ARN_LENGTH: (usize, usize) = (59, 150);

/// Long enough to cover the provisioning span `state.rs` documents (400 ms) with
/// margin; the transition is due at a stored instant, so any wait past it settles.
const SETTLE_WAIT: Duration = Duration::from_millis(700);

// ---------------------------------------------------------------------------
// The operations under test
// ---------------------------------------------------------------------------

/// `ListTagsForResource`'s request for `arn`.
fn list_tags_body(arn: &str) -> Value {
    json!({ "arn": arn })
}

/// `TagResource`'s request for `arn` (a write, so the refusal must also store
/// nothing).
fn tag_body(arn: &str) -> Value {
    json!({ "arn": arn, "tags": { "env": "test" } })
}

/// `UntagResource`'s request for `arn`.
fn untag_body(arn: &str) -> Value {
    json!({ "arn": arn, "tagKeys": ["env"] })
}

/// How one ARN-identified operation is called: its name and the request body it
/// takes for an `arn`.
pub(crate) type ArnOperation = (&'static str, fn(&str) -> Value);

/// Every operation that identifies its resource by `AmazonResourceName`, with the
/// request body each one takes.
pub(crate) fn arn_operations() -> Vec<ArnOperation> {
    vec![
        ("ListTagsForResource", list_tags_body as fn(&str) -> Value),
        ("TagResource", tag_body as fn(&str) -> Value),
        ("UntagResource", untag_body as fn(&str) -> Value),
    ]
}

// ---------------------------------------------------------------------------
// The two negative classes
// ---------------------------------------------------------------------------

/// The operation among [`arn_operations`] that only READS a tag set; the other two
/// write one, and what the rule answers for an ARN resolving to no resource differs
/// between the two kinds.
pub(crate) const TAG_READER: &str = "ListTagsForResource";

/// The two ways an ARN the model's `@pattern` admits can fail to address a resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Fault {
    /// Its partition or account is not the one the request addresses, so it names a
    /// resource outside the scope the caller is talking to.
    OutOfScope,
    /// Inside the request's scope, resolving to no stored connection.
    NoSuchConnection,
}

impl Fault {
    /// The modelled error code this class is reported with, where it is reported as an
    /// error at all.
    pub(crate) fn code(self) -> &'static str {
        match self {
            Fault::OutOfScope => "InterconnectValidationException",
            Fault::NoSuchConnection => "InterconnectClientException",
        }
    }

    /// How this service words the refusal of `arn` in this class, on `operation`.
    pub(crate) fn message(self, operation: &str, arn: &str) -> String {
        match self {
            Fault::OutOfScope => {
                let parsed = parse_connection_arn(arn)
                    .expect("the row's ARN is a well-formed Interconnect connection ARN");
                if parsed.account != ACCOUNT_ID {
                    scope_mismatch_message(
                        ARN_ACCOUNT_FIELD,
                        parsed.account,
                        ACCOUNT_ID,
                        operation,
                    )
                } else {
                    scope_mismatch_message(
                        ARN_PARTITION_FIELD,
                        parsed.partition,
                        PARTITION,
                        operation,
                    )
                }
            }
            Fault::NoSuchConnection => arn_resource_absent_message(),
        }
    }
}

/// How the ACCOUNT field of an ARN is named in a scope-mismatch refusal.
pub(crate) const ARN_ACCOUNT_FIELD: &str = "account ID";

/// How the PARTITION field of an ARN is named in that same refusal.
pub(crate) const ARN_PARTITION_FIELD: &str = "partition";

/// The name the model gives `operation`'s input shape: each ARN-addressed operation's
/// `input_shape` is its own name with `Request` appended
/// (`smithy_query(query="operation",operation="ListTagsForResource,TagResource,UntagResource")`
/// -> `input_shape` `ListTagsForResourceRequest`, `TagResourceRequest`,
/// `UntagResourceRequest`).
pub(crate) fn request_shape(operation: &str) -> String {
    format!("{operation}Request")
}

/// How an ARN that does not address the caller's scope is refused: the ARN FIELD that
/// differs, the value the ARN carried for it, the value the CALLER is, and the input
/// shape the ARN arrived in — with neither this service's refusal envelope nor the ARN
/// text.
pub(crate) fn scope_mismatch_message(
    field: &str,
    carried: &str,
    caller: &str,
    operation: &str,
) -> String {
    format!(
        "ARN {field} ({carried}) does not match caller {field} ({caller}) for {}",
        request_shape(operation)
    )
}

/// Assert `(status, body)` is what `operation` answers an `arn` of class `fault`: the
/// empty tag set from the READ for an ARN that resolves to no resource, and otherwise
/// the modelled 400 under that class's code and wording.
pub(crate) fn assert_answers_fault(
    case: &str,
    operation: &str,
    fault: Fault,
    arn: &str,
    status: u16,
    body: &Value,
) {
    if fault == Fault::NoSuchConnection && operation == TAG_READER {
        assert_eq!(
            status, 200,
            "case {case:?} on {operation}: expected HTTP 200 for an ARN that resolves to no resource, got {status} with {body:?}"
        );
        assert_eq!(
            body["tags"],
            json!({}),
            "case {case:?} on {operation}: expected the empty tag set, got {:?}",
            body["tags"]
        );
        return;
    }
    assert_eq!(
        status, 400,
        "case {case:?} on {operation}: expected HTTP 400 for an ARN that addresses no resource, got {status} with {body:?}"
    );
    let answered = code(body);
    assert!(
        answered.contains(fault.code()),
        "case {case:?} on {operation}: expected {}, got {answered:?} with {body:?}",
        fault.code()
    );
    assert_eq!(
        message(body),
        fault.message(operation, arn),
        "case {case:?} on {operation}: expected this service's own wording for the class, got {:?}",
        message(body)
    );
}

/// One row: a name, the `arn` to send and the class it belongs to.
#[derive(Debug, Clone)]
struct Case {
    name: String,
    arn: String,
    fault: Fault,
}

impl Case {
    fn new(name: &str, arn: String, fault: Fault) -> Self {
        Self {
            name: name.to_string(),
            arn,
            fault,
        }
    }
}

/// A connection ARN with the fields the row wants, built the way the ARN's pattern
/// writes them: `arn:<partition>:interconnect:<region>:<account>:connection/<id>`.
fn connection_arn(partition: &str, region: &str, account: &str, id: &str) -> String {
    format!("arn:{partition}:{SIGNING_NAME}:{region}:{account}:connection/{id}")
}

/// The table: rows that differ in which field is out of the request's scope and in
/// its value, rows of that scope that resolve to nothing (including ones whose Region
/// segment is not the called Region, which puts nothing out of scope), and rows at
/// both ends of the modelled ARN length.
fn table(live_arn: &str) -> Vec<Case> {
    // A Region segment long enough to take the ARN to its modelled maximum length, and
    // one short enough to take it to the minimum: everything around that segment is
    // fixed, so the ARN's length is the segment's length plus the rest.
    let floor_id = "mcc-00000000";
    let ceiling_id = "lmcc-00000000";
    let shortest_region = "r".repeat(
        ARN_LENGTH
            .0
            .saturating_sub(connection_arn(PARTITION, "", ACCOUNT_ID, floor_id).len())
            .max(1),
    );
    let longest_region =
        "r".repeat(ARN_LENGTH.1 - connection_arn(PARTITION, "", ACCOUNT_ID, ceiling_id).len());

    let rows = vec![
        // Out of scope by exactly one field.
        Case::new(
            "another account",
            live_arn.replace(ACCOUNT_ID, "123456789012"),
            Fault::OutOfScope,
        ),
        Case::new(
            "an account differing in one digit",
            live_arn.replace(ACCOUNT_ID, "000000000001"),
            Fault::OutOfScope,
        ),
        Case::new(
            "another partition",
            live_arn.replace(&format!("arn:{PARTITION}:"), "arn:aws-cn:"),
            Fault::OutOfScope,
        ),
        Case::new(
            "the government partition",
            live_arn.replace(&format!("arn:{PARTITION}:"), "arn:aws-us-gov:"),
            Fault::OutOfScope,
        ),
        // Out of scope by several fields at once, and with a Region segment that is
        // not the called one either: the account and the partition still decide.
        Case::new(
            "another account, spelled with another Region",
            live_arn
                .replace(ACCOUNT_ID, "123456789012")
                .replace(REGION, "eu-west-3"),
            Fault::OutOfScope,
        ),
        Case::new(
            "another account in another partition",
            live_arn
                .replace(ACCOUNT_ID, "123456789012")
                .replace(&format!("arn:{PARTITION}:"), "arn:aws-cn:"),
            Fault::OutOfScope,
        ),
        // In the request's scope, resolving to nothing: an id this endpoint never
        // allocated, carried by a Region segment at each end of the modelled ARN
        // length.
        Case::new(
            "an unallocated id at the ARN length floor",
            connection_arn(PARTITION, &shortest_region, ACCOUNT_ID, floor_id),
            Fault::NoSuchConnection,
        ),
        Case::new(
            "an unallocated id at the ARN length ceiling",
            connection_arn(PARTITION, &longest_region, ACCOUNT_ID, ceiling_id),
            Fault::NoSuchConnection,
        ),
        // In the request's scope, resolving to nothing: both id prefixes, ids whose
        // characters span the `[a-z0-9]` alphabet the id's pattern allows, and Region
        // segments both matching and not matching the called Region.
        Case::new(
            "an id that was never allocated",
            connection_arn(PARTITION, REGION, ACCOUNT_ID, "mcc-00000000"),
            Fault::NoSuchConnection,
        ),
        Case::new(
            "the other id prefix, never allocated",
            connection_arn(PARTITION, REGION, ACCOUNT_ID, "lmcc-zzzzzzzz"),
            Fault::NoSuchConnection,
        ),
        Case::new(
            "an unallocated id mixing letters and digits",
            connection_arn(PARTITION, REGION, ACCOUNT_ID, "mcc-a1b2c3d4"),
            Fault::NoSuchConnection,
        ),
        Case::new(
            "an unallocated id spelled with another Region",
            connection_arn(PARTITION, "eu-west-3", ACCOUNT_ID, "mcc-a1b2c3d4"),
            Fault::NoSuchConnection,
        ),
        Case::new(
            "an unallocated id spelled with a Region on the other side of the world",
            connection_arn(PARTITION, "ap-southeast-2", ACCOUNT_ID, "lmcc-zzzzzzzz"),
            Fault::NoSuchConnection,
        ),
        Case::new(
            "an unallocated id one character off a live one",
            {
                let live_id = live_arn
                    .rsplit_once('/')
                    .map(|(_, id)| id.to_string())
                    .unwrap_or_default();
                let mut flipped = live_id.clone();
                flipped.pop();
                flipped.push(if live_id.ends_with('0') { '1' } else { '0' });
                connection_arn(PARTITION, REGION, ACCOUNT_ID, &flipped)
            },
            Fault::NoSuchConnection,
        ),
    ];
    // Every row must stay inside the modelled length, or the boundary validator —
    // not the rule under test — would be what refuses it.
    for case in &rows {
        assert!(
            case.arn.len() >= ARN_LENGTH.0 && case.arn.len() <= ARN_LENGTH.1,
            "case {:?}: expected an ARN inside the modelled {ARN_LENGTH:?} length, got {} characters",
            case.name,
            case.arn.len()
        );
    }
    rows
}

// ---------------------------------------------------------------------------
// The rule
// ---------------------------------------------------------------------------

/// RULE: every ARN-addressed operation answers an `arn` it cannot resolve by the class
/// that ARN belongs to — the request fault for one outside the scope the request
/// addresses, this service's client fault for one of that scope resolving to nothing,
/// and the empty tag set where that one reaches the READ.
#[tokio::test]
async fn arn_refusals_split_the_two_negative_classes() {
    let emulator = Emulator::new();
    let places = placements(&emulator).await;
    let live_tags: BTreeMap<String, String> = [("env".to_string(), "test".to_string())]
        .into_iter()
        .collect();
    let live = create_with_tags(&emulator, &places[0], 1_000, &live_tags).await;
    let live_arn = text(&live, "arn");
    let live_id = text(&live, "id");

    let rows = table(&live_arn);
    assert!(
        rows.len() >= 10,
        "the table must span both classes over several fields and values, got {} rows",
        rows.len()
    );
    assert!(
        rows.iter().any(|case| case.fault == Fault::OutOfScope)
            && rows.iter().any(|case| case.fault == Fault::NoSuchConnection),
        "the table must carry both negative classes, got {rows:?}"
    );

    let mut seen: BTreeMap<Fault, BTreeMap<String, String>> = BTreeMap::new();
    for (operation, body) in arn_operations() {
        for case in &rows {
            let (status, answer) = emulator.ic(operation, body(&case.arn)).await;
            assert_answers_fault(
                &case.name,
                operation,
                case.fault,
                &case.arn,
                status,
                &answer,
            );
            if status == 400 {
                seen.entry(case.fault)
                    .or_default()
                    .insert(format!("{operation}/{}", case.name), code(&answer));
            }
        }
    }

    // The two classes really are distinguishable: no code answered for one class is
    // ever answered for the other.
    let out_of_scope: Vec<String> = seen
        .get(&Fault::OutOfScope)
        .map(|cases| cases.values().cloned().collect())
        .unwrap_or_default();
    let no_connection: Vec<String> = seen
        .get(&Fault::NoSuchConnection)
        .map(|cases| cases.values().cloned().collect())
        .unwrap_or_default();
    assert!(
        out_of_scope
            .iter()
            .all(|answered| !no_connection.contains(answered)),
        "case \"two classes, two codes\": expected disjoint codes, got {out_of_scope:?} against {no_connection:?}"
    );

    // Every refusal above stored nothing: the live connection still holds exactly
    // the tags it was created with, and is still readable.
    let (get_status, fetched) = emulator.get_connection(&live_id).await;
    assert_eq!(
        get_status, 200,
        "case \"after every refusal\": expected the live connection to be readable, got {get_status} with {fetched:?}"
    );
    assert_eq!(
        tags_of(&fetched["connection"]),
        live_tags,
        "case \"after every refusal\": expected the live connection's tags untouched, got {:?}",
        fetched["connection"]["tags"]
    );
}

/// RULE: the refusals are about the ARN, not about the operation — the SAME
/// operations serve the live resource's own ARN, so a 400 above is the class of the
/// ARN and not a blanket rejection.
#[tokio::test]
async fn the_same_operations_serve_an_in_scope_arn() {
    let emulator = Emulator::new();
    let places = placements(&emulator).await;

    for (seed, (operation, body)) in arn_operations().into_iter().enumerate() {
        let tags: BTreeMap<String, String> = [("env".to_string(), "test".to_string())]
            .into_iter()
            .collect();
        let live = create_with_tags(&emulator, &places[seed % places.len()], 1_100 + seed, &tags).await;
        let arn = text(&live, "arn");
        let (status, answer) = emulator.ic(operation, body(&arn)).await;
        assert_eq!(
            status, 200,
            "case {operation:?}: expected HTTP 200 for the live resource's own ARN, got {status} with {answer:?}"
        );
    }
}

/// RULE: the class an ARN falls into follows the resource — a connection still
/// `deleting` is served, and once the clock has settled it into the terminal
/// `deleted` state its ARN resolves to nothing, which is the client fault for the two
/// writes and the empty tag set for the read, and never a 404.
#[tokio::test]
async fn a_settled_delete_moves_its_arn_into_the_client_fault_class() {
    let emulator = Emulator::new();
    let places = placements(&emulator).await;
    let tags: BTreeMap<String, String> = [("env".to_string(), "test".to_string())]
        .into_iter()
        .collect();
    let connection = create_with_tags(&emulator, &places[0], 1_200, &tags).await;
    let arn = text(&connection, "arn");
    let id = text(&connection, "id");

    let deleted = emulator
        .ic_ok("DeleteConnection", json!({ "identifier": id }))
        .await;
    assert_eq!(
        text(&deleted["connection"], "state"),
        "deleting",
        "case \"straight after the delete\": expected the transient state, got {:?}",
        deleted["connection"]["state"]
    );
    let (status, answer) = emulator.ic(TAG_READER, list_tags_body(&arn)).await;
    assert_eq!(
        status, 200,
        "case \"while deleting\": expected the resource to still be served, got {status} with {answer:?}"
    );
    assert_eq!(
        tags_of(&answer),
        tags,
        "case \"while deleting\": expected the tags of a resource that still exists, got {:?}",
        answer["tags"]
    );

    tokio::time::sleep(SETTLE_WAIT).await;
    let settled = emulator.get_connection(&id).await.1["connection"].clone();
    assert_eq!(
        text(&settled, "state"),
        "deleted",
        "case \"after the delete settles\": expected the terminal state, got {:?}",
        settled["state"]
    );

    for (operation, body) in arn_operations() {
        let (status, answer) = emulator.ic(operation, body(&arn)).await;
        assert_answers_fault(
            "after the delete settles",
            operation,
            Fault::NoSuchConnection,
            &arn,
            status,
            &answer,
        );
    }
}

/// RULE: an `arn` that is not an Interconnect connection ARN at all is a fault in
/// the request too — invalid input (HTTP 400), whether the modelled
/// `@pattern`/`@length` catches it at the boundary or the handler does, and never a
/// 404 or a 501.
#[tokio::test]
async fn an_arn_that_is_not_a_connection_arn_is_invalid_input() {
    let emulator = Emulator::new();
    let malformed = vec![
        ("not an ARN at all", "give-me-the-tags".to_string()),
        (
            "another service's resource",
            connection_arn(PARTITION, REGION, ACCOUNT_ID, "mcc-aaaaaaaa")
                .replace("interconnect", "directconnect"),
        ),
        (
            "another resource type",
            connection_arn(PARTITION, REGION, ACCOUNT_ID, "mcc-aaaaaaaa")
                .replace("connection/", "environment/"),
        ),
        (
            "no account id",
            connection_arn(PARTITION, REGION, "", "mcc-aaaaaaaa"),
        ),
        (
            "an account id that is not twelve digits",
            connection_arn(PARTITION, REGION, "12345", "mcc-aaaaaaaa"),
        ),
    ];

    for (operation, body) in arn_operations() {
        for (name, arn) in &malformed {
            let (status, answer) = emulator.ic(operation, body(arn)).await;
            assert_eq!(
                status, 400,
                "case {name:?} on {operation}: expected HTTP 400 for an ARN that is not a connection ARN, got {status} with {answer:?}"
            );
        }
    }
}
