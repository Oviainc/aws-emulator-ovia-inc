// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the WORDING of the two refusals an ARN-addressed operation
//! answers, driven over the wire through the shared [`crate::emulator`] harness.
//!
//! RULE under test: an operation that identifies its resource by
//! `AmazonResourceName` refuses an ARN that does not address the caller's scope by
//! naming the ARN FIELD that differs, the value the ARN carried for it, the value the
//! CALLER is (its own account), and the name of the INPUT SHAPE the ARN arrived in —
//! and nothing else: neither this service's own refusal envelope nor the ARN text. An
//! ARN that DOES address the caller's scope but resolves to no resource is refused
//! with a sentence that is entirely fixed, carrying no ARN, no id and no resource
//! type, which is a different sentence from the one an identifier-addressed read
//! (`GetConnection`, `DeleteConnection`) words for an identifier that names nothing.
//!
//! The operations covered are every one whose input addresses its resource by
//! `AmazonResourceName` — `ListTagsForResource`, `TagResource` and `UntagResource`
//! (`smithy_query(query="operation",operation="ListTagsForResource,TagResource,UntagResource")`:
//! each takes `arn` -> `AmazonResourceName`, `@length 59..150`, `@pattern
//! ^arn:aws[a-z-]*:interconnect:[^:]+:[0-9]{12}:connection/(mcc|lmcc)-[a-z0-9]{8}$`,
//! and their `input_shape`s are `ListTagsForResourceRequest`, `TagResourceRequest` and
//! `UntagResourceRequest`) — crossed with a TABLE of ARNs whose rows differ in which
//! field is out of scope (account, partition, both at once), in the VALUE of that
//! field, in the Region segment (which puts nothing out of scope), in the id prefix
//! (`mcc`/`lmcc`) and in the ARN's overall length (both ends of the modelled 59..150
//! bound).

use serde_json::{json, Value};

use crate::arn_scope_refusals::{
    arn_operations, request_shape, scope_mismatch_message, ARN_ACCOUNT_FIELD,
    ARN_PARTITION_FIELD, TAG_READER,
};
use crate::emulator::{code, message, tags_of, Emulator, SIGNING_NAME};

/// The account every request this emulator serves is for, and so the account a
/// refusal reports as the caller's.
const CALLER_ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The partition every ARN this emulator mints carries.
const CALLER_PARTITION: &str = "aws";

/// The Region the shared harness signs its requests for.
const REGION: &str = "us-east-1";

/// `AmazonResourceName` is modelled `@length(min: 59, max: 150)`
/// (`smithy_query(query="shape",shape="AmazonResourceName")`), so a row at either end
/// of that span is still an ARN the boundary validator hands to the handler.
const ARN_LENGTH: (usize, usize) = (59, 150);

/// The envelope this service puts on the refusals it words ITSELF, which an ARN-scope
/// mismatch does not carry.
const SERVICE_REFUSAL_ENVELOPE: &str = "Unable to process request: ";

/// A connection ARN with the fields a row wants, built the way the member's pattern
/// writes them: `arn:<partition>:interconnect:<region>:<account>:connection/<id>`.
fn connection_arn(partition: &str, region: &str, account: &str, id: &str) -> String {
    format!("arn:{partition}:{SIGNING_NAME}:{region}:{account}:connection/{id}")
}

/// An ARN of exactly `length` characters, reached by padding the REGION segment: the
/// member's pattern leaves that segment unconstrained (`[^:]+`), so padding it is how
/// a row reaches a length without changing which field is out of scope.
fn arn_of_length(length: usize, partition: &str, account: &str, id: &str) -> String {
    let fixed = connection_arn(partition, "", account, id).len();
    let region = "r".repeat(length.saturating_sub(fixed).max(1));
    connection_arn(partition, &region, account, id)
}

/// One row: a name, the `arn` to send, and the field the refusal must name together
/// with the value the ARN carried for it and the value the caller is.
#[derive(Debug, Clone)]
struct Row {
    name: String,
    arn: String,
    field: &'static str,
    carried: String,
    caller: String,
}

impl Row {
    /// A row whose ACCOUNT is not the caller's.
    fn account(name: &str, arn: String, carried: &str) -> Self {
        Self {
            name: name.to_string(),
            arn,
            field: ARN_ACCOUNT_FIELD,
            carried: carried.to_string(),
            caller: CALLER_ACCOUNT.to_string(),
        }
    }

    /// A row whose PARTITION is not the caller's, its account being the caller's.
    fn partition(name: &str, arn: String, carried: &str) -> Self {
        Self {
            name: name.to_string(),
            arn,
            field: ARN_PARTITION_FIELD,
            carried: carried.to_string(),
            caller: CALLER_PARTITION.to_string(),
        }
    }

    /// The message `operation` must answer this row with.
    fn expected(&self, operation: &str) -> String {
        scope_mismatch_message(self.field, &self.carried, &self.caller, operation)
    }
}

/// The table: ARNs out of the caller's scope by each field, by several values of each
/// field, by both fields at once, over both id prefixes, over Region segments the
/// request did and did not address, and at both ends of the modelled ARN length.
fn table() -> Vec<Row> {
    let rows = vec![
        // Out of scope by the ACCOUNT, over several values of it.
        Row::account(
            "another account",
            connection_arn(CALLER_PARTITION, REGION, "123456789012", "mcc-abcd1234"),
            "123456789012",
        ),
        Row::account(
            "a repeated-digit account",
            connection_arn(CALLER_PARTITION, REGION, "111111111111", "mcc-abcd1234"),
            "111111111111",
        ),
        Row::account(
            "an account differing in one digit",
            connection_arn(CALLER_PARTITION, REGION, "000000000001", "lmcc-abcd1234"),
            "000000000001",
        ),
        Row::account(
            "the highest account id",
            connection_arn(CALLER_PARTITION, REGION, "999999999999", "mcc-00000000"),
            "999999999999",
        ),
        // Out of scope by the PARTITION, over several values of it: the account is
        // the caller's, so the partition is what the refusal names.
        Row::partition(
            "the China partition",
            connection_arn("aws-cn", REGION, CALLER_ACCOUNT, "mcc-abcd1234"),
            "aws-cn",
        ),
        Row::partition(
            "the government partition",
            connection_arn("aws-us-gov", REGION, CALLER_ACCOUNT, "lmcc-zzzzzzzz"),
            "aws-us-gov",
        ),
        Row::partition(
            "an isolated partition",
            connection_arn("aws-iso-b", REGION, CALLER_ACCOUNT, "mcc-a1b2c3d4"),
            "aws-iso-b",
        ),
        // Out of scope by BOTH at once: the account is judged first, so a caller that
        // addressed another account always reads the account mismatch.
        Row::account(
            "another account in another partition",
            connection_arn("aws-cn", REGION, "123456789012", "mcc-abcd1234"),
            "123456789012",
        ),
        // The Region segment is not a scope of its own, so it changes the value of
        // nothing the refusal names.
        Row::account(
            "another account spelled with another Region",
            connection_arn(CALLER_PARTITION, "eu-west-3", "123456789012", "mcc-abcd1234"),
            "123456789012",
        ),
        Row::partition(
            "another partition spelled with another Region",
            connection_arn("aws-cn", "ap-southeast-2", CALLER_ACCOUNT, "lmcc-abcd1234"),
            "aws-cn",
        ),
        // Both ends of the modelled ARN length, reached through the Region segment.
        Row::account(
            "another account at the ARN length floor",
            arn_of_length(
                ARN_LENGTH.0,
                CALLER_PARTITION,
                "123456789012",
                "mcc-00000000",
            ),
            "123456789012",
        ),
        Row::account(
            "another account at the ARN length ceiling",
            arn_of_length(
                ARN_LENGTH.1,
                CALLER_PARTITION,
                "123456789012",
                "lmcc-zzzzzzzz",
            ),
            "123456789012",
        ),
        Row::partition(
            "another partition at the ARN length ceiling",
            arn_of_length(ARN_LENGTH.1, "aws-cn", CALLER_ACCOUNT, "mcc-00000000"),
            "aws-cn",
        ),
    ];
    for row in &rows {
        assert!(
            row.arn.len() >= ARN_LENGTH.0 && row.arn.len() <= ARN_LENGTH.1,
            "case {:?}: expected an ARN inside the modelled {ARN_LENGTH:?} length, got {} characters",
            row.name,
            row.arn.len()
        );
    }
    rows
}

/// Assert `(status, body)` is the refusal `row` must get from `operation`.
fn assert_scope_refusal(row: &Row, operation: &str, status: u16, body: &Value) {
    let case = format!("{} on {operation}", row.name);
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400 for an ARN outside the caller's scope, got {status} with {body:?}"
    );
    let answered = code(body);
    assert!(
        answered.contains("InterconnectValidationException"),
        "case {case:?}: expected InterconnectValidationException, got {answered:?} with {body:?}"
    );
    let reported = message(body);
    assert_eq!(
        reported,
        row.expected(operation),
        "case {case:?}: expected the scope-mismatch wording, got {reported:?}"
    );
    assert!(
        !reported.contains(SERVICE_REFUSAL_ENVELOPE),
        "case {case:?}: expected no service refusal envelope in the message, got {reported:?}"
    );
    assert!(
        !reported.contains(&row.arn),
        "case {case:?}: expected the ARN not to be reproduced in the message, got {reported:?}"
    );
    assert!(
        reported.ends_with(&format!(" for {}", request_shape(operation))),
        "case {case:?}: expected the message to name this request's input shape, got {reported:?}"
    );
}

// ---------------------------------------------------------------------------
// The rule
// ---------------------------------------------------------------------------

/// RULE: every ARN-addressed operation refuses an ARN outside the caller's scope with
/// the field that differs, the value the ARN carried, the caller's own value and the
/// input shape the ARN arrived in.
#[tokio::test]
async fn an_out_of_scope_arn_is_refused_with_the_field_the_values_and_the_input_shape() {
    let emulator = Emulator::new();
    let rows = table();
    assert!(
        rows.iter().any(|row| row.field == ARN_ACCOUNT_FIELD)
            && rows.iter().any(|row| row.field == ARN_PARTITION_FIELD),
        "the table must span both scope fields, got {rows:?}"
    );

    for (operation, body) in arn_operations() {
        for row in &rows {
            let (status, answered) = emulator.ic(operation, body(&row.arn)).await;
            assert_scope_refusal(row, operation, status, &answered);
        }
    }
}

/// RULE: the input-shape name is what says WHICH request carried the ARN, so the SAME
/// out-of-scope ARN is refused with a different suffix on each operation.
#[tokio::test]
async fn the_refusal_names_the_input_shape_of_the_request_that_carried_the_arn() {
    let emulator = Emulator::new();
    let arn = connection_arn(CALLER_PARTITION, REGION, "123456789012", "mcc-abcd1234");

    let mut reported: Vec<(String, String)> = Vec::new();
    for (operation, body) in arn_operations() {
        let (status, answered) = emulator.ic(operation, body(&arn)).await;
        assert_eq!(
            status, 400,
            "case {operation:?}: expected HTTP 400 for an ARN of another account, got {status} with {answered:?}"
        );
        reported.push((operation.to_string(), message(&answered)));
    }

    for (operation, answered) in &reported {
        assert_eq!(
            *answered,
            scope_mismatch_message(ARN_ACCOUNT_FIELD, "123456789012", CALLER_ACCOUNT, operation),
            "case {operation:?}: expected the account-mismatch wording for this operation, got {answered:?}"
        );
    }
    let distinct: std::collections::BTreeSet<&String> =
        reported.iter().map(|(_, answered)| answered).collect();
    assert_eq!(
        distinct.len(),
        reported.len(),
        "case \"one ARN, three operations\": expected one message per input shape, got {reported:?}"
    );
}

/// RULE: an ARN that DOES address the caller's scope but resolves to no resource is
/// refused by the two tag WRITES with an entirely fixed sentence — no ARN, no id, no
/// resource type — while the READ answers the empty tag set, and nothing is stored.
#[tokio::test]
async fn an_in_scope_arn_that_resolves_to_nothing_is_refused_value_free() {
    let emulator = Emulator::new();
    // The sentence this class carries, spelled out rather than taken from the crate,
    // so a change to it fails here.
    let fixed = "Unable to process request: Resource does not exist or is deleted";

    let unresolvable = [
        (
            "an id that was never allocated",
            connection_arn(CALLER_PARTITION, REGION, CALLER_ACCOUNT, "mcc-00000000"),
        ),
        (
            "the other id prefix, never allocated",
            connection_arn(CALLER_PARTITION, REGION, CALLER_ACCOUNT, "lmcc-zzzzzzzz"),
        ),
        (
            "an unallocated id mixing letters and digits",
            connection_arn(CALLER_PARTITION, REGION, CALLER_ACCOUNT, "mcc-a1b2c3d4"),
        ),
        (
            "an unallocated id spelled with another Region",
            connection_arn(CALLER_PARTITION, "eu-west-3", CALLER_ACCOUNT, "mcc-a1b2c3d4"),
        ),
        (
            "an unallocated id at the ARN length ceiling",
            arn_of_length(
                ARN_LENGTH.1,
                CALLER_PARTITION,
                CALLER_ACCOUNT,
                "lmcc-zzzzzzzz",
            ),
        ),
    ];

    for (operation, body) in arn_operations() {
        for (name, arn) in &unresolvable {
            let case = format!("{name} on {operation}");
            let (status, answered) = emulator.ic(operation, body(arn)).await;
            if operation == TAG_READER {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected HTTP 200 from the read, got {status} with {answered:?}"
                );
                assert!(
                    tags_of(&answered).is_empty(),
                    "case {case:?}: expected the empty tag set, got {:?}",
                    answered["tags"]
                );
                continue;
            }
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 from the write, got {status} with {answered:?}"
            );
            let reported_code = code(&answered);
            assert!(
                reported_code.contains("InterconnectClientException"),
                "case {case:?}: expected InterconnectClientException, got {reported_code:?} with {answered:?}"
            );
            let reported = message(&answered);
            assert_eq!(
                reported, fixed,
                "case {case:?}: expected the fixed not-found sentence, got {reported:?}"
            );
        }
    }

    // The refused writes applied no tag: the same ARNs still read back as carrying
    // none, which is what proves the refusal stored nothing.
    for (name, arn) in &unresolvable {
        let (status, listed) = emulator.ic(TAG_READER, json!({ "arn": arn })).await;
        assert_eq!(
            status, 200,
            "case {name:?} after the refused writes: expected HTTP 200, got {status} with {listed:?}"
        );
        assert!(
            tags_of(&listed).is_empty(),
            "case {name:?} after the refused writes: expected no tag to have been applied, got {:?}",
            listed["tags"]
        );
    }
}

/// RULE: the ARN-addressed not-found sentence and the IDENTIFIER-addressed one are
/// different sentences — a read addressed by `ConnectionId` names the connection it
/// could not retrieve, so the value-free ARN wording never replaces it.
#[tokio::test]
async fn an_identifier_addressed_read_keeps_its_own_not_found_wording() {
    let emulator = Emulator::new();

    for identifier in ["mcc-00000000", "lmcc-00000000", "mcc-a1b2c3d4"] {
        let expected = format!(
            "{SERVICE_REFUSAL_ENVELOPE}Unable to retrieve connection {identifier}, as it does not exist"
        );
        for operation in ["GetConnection", "DeleteConnection"] {
            let case = format!("{identifier} on {operation}");
            let (status, answered) = emulator
                .ic(operation, json!({ "identifier": identifier }))
                .await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 for an identifier that names no connection, got {status} with {answered:?}"
            );
            assert!(
                code(&answered).contains("ResourceNotFoundException"),
                "case {case:?}: expected the modelled ResourceNotFoundException, got {:?}",
                code(&answered)
            );
            let reported = message(&answered);
            assert_eq!(
                reported, expected,
                "case {case:?}: expected the identifier-naming wording, got {reported:?}"
            );
            assert!(
                reported.contains(identifier),
                "case {case:?}: expected the identifier the request carried, got {reported:?}"
            );
        }
    }
}
