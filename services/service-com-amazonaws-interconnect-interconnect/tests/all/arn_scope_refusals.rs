// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for how the ARN-identified operations refuse an
//! `AmazonResourceName` they cannot resolve, driven over the wire through the
//! shared [`crate::emulator`] harness.
//!
//! RULE under test: an operation that identifies its resource by ARN splits the two
//! ways an ARN can name no resource and reports BOTH with HTTP 400 — an ARN whose
//! partition, Region or account is not the one the request addresses is a fault in
//! the request (`InterconnectValidationException`), while an ARN inside that scope
//! that names no stored resource is this service's own client fault
//! (`InterconnectClientException`) — so a client can tell "wrong account" from "no
//! such resource", and neither is answered with the connection-scoped 404.
//!
//! The operations covered are every one whose input identifies a resource by
//! `AmazonResourceName`: `ListTagsForResource`, `TagResource` and `UntagResource`
//! (`smithy_query(query="operation",operation="ListTagsForResource,TagResource,UntagResource")`
//! — each takes `arn` -> `AmazonResourceName`, `@length 59..150`, `@pattern
//! ^arn:aws[a-z-]*:interconnect:[^:]+:[0-9]{12}:connection/(mcc|lmcc)-[a-z0-9]{8}$`,
//! and each models `InterconnectClientException` (400) and
//! `InterconnectValidationException` (400)).
//!
//! Every assertion is about that rule over a TABLE of ARNs whose rows differ in
//! WHICH field puts the ARN out of the request's scope (partition, Region, account,
//! several at once), in the value of that field, in the id prefix (`mcc`/`lmcc`), and
//! in the ARN's overall length (both ends of the modelled 59..150 bound) — crossed
//! with all three operations.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{json, Value};

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
type ArnOperation = (&'static str, fn(&str) -> Value);

/// Every operation that identifies its resource by `AmazonResourceName`, with the
/// request body each one takes.
fn arn_operations() -> Vec<ArnOperation> {
    vec![
        ("ListTagsForResource", list_tags_body as fn(&str) -> Value),
        ("TagResource", tag_body as fn(&str) -> Value),
        ("UntagResource", untag_body as fn(&str) -> Value),
    ]
}

// ---------------------------------------------------------------------------
// The two negative classes
// ---------------------------------------------------------------------------

/// The two ways an ARN the model's `@pattern` admits can name no resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Fault {
    /// Its partition, Region or account is not the one the request addresses.
    OutOfScope,
    /// Inside the request's scope, naming no stored connection.
    NoSuchConnection,
}

impl Fault {
    /// The modelled error code this class is reported with.
    fn code(self) -> &'static str {
        match self {
            Fault::OutOfScope => "InterconnectValidationException",
            Fault::NoSuchConnection => "InterconnectClientException",
        }
    }
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
/// its value, rows inside the scope that name nothing, and rows at both ends of the
/// modelled ARN length.
fn table(live_arn: &str) -> Vec<Case> {
    // A Region long enough to take the ARN to its modelled maximum length, and one
    // short enough to take it to the minimum: everything around the Region field is
    // fixed, so the ARN's length is that Region's length plus the rest.
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
            "another region",
            live_arn.replace(REGION, "eu-west-3"),
            Fault::OutOfScope,
        ),
        Case::new(
            "another region, on the other side of the world",
            live_arn.replace(REGION, "ap-southeast-2"),
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
        // Out of scope by several fields at once.
        Case::new(
            "another account in another region",
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
        // Out of scope at each end of the modelled ARN length.
        Case::new(
            "an out-of-scope region at the ARN length floor",
            connection_arn(PARTITION, &shortest_region, ACCOUNT_ID, floor_id),
            Fault::OutOfScope,
        ),
        Case::new(
            "an out-of-scope region at the ARN length ceiling",
            connection_arn(PARTITION, &longest_region, ACCOUNT_ID, ceiling_id),
            Fault::OutOfScope,
        ),
        // In the request's scope, naming nothing: both id prefixes, and ids whose
        // characters span the `[a-z0-9]` alphabet the id's pattern allows.
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

/// RULE: every ARN-identified operation refuses an unresolvable `arn` with HTTP 400,
/// and with the code of the class the ARN belongs to — the request fault for one
/// outside the scope the request addresses, this service's client fault for one
/// inside it that names nothing.
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
            assert_eq!(
                status, 400,
                "case {:?} on {operation}: expected HTTP 400 for an ARN naming no resource, got {status} with {answer:?}",
                case.name
            );
            let answered = code(&answer);
            assert!(
                answered.contains(case.fault.code()),
                "case {:?} on {operation}: expected {}, got {answered:?} with {answer:?}",
                case.name,
                case.fault.code()
            );
            assert!(
                message(&answer).contains(&case.arn),
                "case {:?} on {operation}: expected the refusal to name the requested ARN, got {:?}",
                case.name,
                message(&answer)
            );
            seen.entry(case.fault)
                .or_default()
                .insert(format!("{operation}/{}", case.name), answered);
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
/// `deleted` state its ARN is in scope and names nothing, which is the client fault
/// and never a 404.
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
    let (status, answer) = emulator.ic("ListTagsForResource", list_tags_body(&arn)).await;
    assert_eq!(
        status, 200,
        "case \"while deleting\": expected the resource to still be served, got {status} with {answer:?}"
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
        assert_eq!(
            status, 400,
            "case \"after the delete settles\" on {operation}: expected HTTP 400, got {status} with {answer:?}"
        );
        assert!(
            code(&answer).contains(Fault::NoSuchConnection.code()),
            "case \"after the delete settles\" on {operation}: expected {}, got {answer:?}",
            Fault::NoSuchConnection.code()
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
