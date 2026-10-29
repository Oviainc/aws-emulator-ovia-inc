// SPDX-License-Identifier: CC0-1.0

//! One service, ONE refusal for a string that is no ARN of this service.
//!
//! The rule under test: an operation addressed by an ARN member refuses a string that does
//! not read as an ARN of this service with HTTP 400 `ValidationException`, `Reason` =
//! `FieldValidationFailed` and one `Fields` entry naming that member — the SAME answer
//! whichever operation the string arrived on, because every one of them answers it out of
//! the single statement of the refusal in `crate::state`.
//!
//! The rows span the ways a string fails to read as an ARN of this service (the leading
//! literal, the partition, the service namespace, the account, the resource type, the
//! resource id, the depth of the resource path) crossed with the operations that take such
//! a member: the three tagging operations, which reach the resource only through its ARN,
//! and `UpdateNetworkResourceMetadata`, which carries the ARN beside a global network id.
//! A well-formed ARN of a stored resource is driven through the same four operations, so
//! each refusal turns on the form of the string and nothing else.
//!
//! Every call goes through the real `MultiServiceRouter` and nothing reaches into
//! `crate::state`: the resource under test is created and read back through operations.

use serde_json::{json, Value};

use crate::global_network_lifecycle::{error_code, expected_scoped_arn, Emulator};
use crate::resource_tagging::{escape, tag, tags_uri, untag_uri};

/// The top-level `Message` this service answers for the input class.
const MALFORMED_ARN_MESSAGE: &str = "ResourceArn is not a networkmanager resource ARN";

/// The `Fields` entry's `Message`: the form an ARN of this global (region-less) service
/// reads in.
const MALFORMED_ARN_FIELD_MESSAGE: &str =
    "must read arn:<partition>:networkmanager::<account>:<type>/<id>";

/// The input member every one of these operations carries the ARN in
/// (`smithy_query query="operation" operation="ListTagsForResource,TagResource,\
/// UntagResource,UpdateNetworkResourceMetadata"`).
const FIELD_RESOURCE_ARN: &str = "ResourceArn";

/// The `UpdateNetworkResourceMetadata` URI of one resource of `network`.
///
/// `/global-networks/{GlobalNetworkId}/network-resources/{ResourceArn}/metadata` binds a
/// NON-greedy `@httpLabel` for the ARN (`smithy_query query="operation"
/// operation="UpdateNetworkResourceMetadata"`), so a client sends its `:` and `/` encoded.
fn metadata_uri(network: &str, arn: &str) -> String {
    format!(
        "/global-networks/{network}/network-resources/{}/metadata",
        escape(arn)
    )
}

/// One call: the operation that answers it, and the request a client sends.
struct Call {
    operation: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
}

/// The four operations of this service that address a resource by an ARN member, each
/// carrying `arn` the way its own binding takes it.
fn calls_for(network: &str, arn: &str) -> Vec<Call> {
    vec![
        Call {
            operation: "ListTagsForResource",
            method: "GET",
            uri: tags_uri(arn),
            body: None,
        },
        Call {
            operation: "TagResource",
            method: "POST",
            uri: tags_uri(arn),
            body: Some(json!({ "Tags": [tag("env", "test")] })),
        },
        Call {
            operation: "UntagResource",
            method: "DELETE",
            uri: untag_uri(arn, &["env"]),
            body: None,
        },
        Call {
            operation: "UpdateNetworkResourceMetadata",
            method: "PATCH",
            uri: metadata_uri(network, arn),
            body: Some(json!({ "Metadata": { "owner": "net-eng" } })),
        },
    ]
}

/// The `(Message, Reason, Fields)` triple of a refusal, which is what the rule fixes.
fn refusal_of(body: &Value) -> (&Value, &Value, &Value) {
    (&body["Message"], &body["Reason"], &body["Fields"])
}

#[tokio::test]
async fn a_string_that_is_no_arn_of_this_service_is_refused_identically_by_every_operation() {
    let emulator = Emulator::new();
    // A live global network and a site in it, so the ARN member is the only thing wrong
    // with each request below: the path the metadata operation travels resolves, and the
    // control row at the end is answered.
    let network = emulator.create_global_network("malformed ARN refusals").await;
    let site = emulator.create_site(&network, "hq").await;
    let site_arn = expected_scoped_arn("site", &network, &site);
    let account = engine::emulator_account_id();

    // Each row breaks ONE part of the ARN form: a different reason the string names no
    // resource of this service.
    let malformed: Vec<(&str, String)> = vec![
        ("no ARN at all", "not-an-arn".to_string()),
        ("only the leading literal", "arn".to_string()),
        (
            "an ARN of another service",
            format!("arn:aws:ec2:us-west-2:{account}:vpc/vpc-0123456789abcdef0"),
        ),
        (
            "an empty partition",
            format!("arn::networkmanager::{account}:site/{network}/{site}"),
        ),
        (
            "an empty account",
            format!("arn:aws:networkmanager:::site/{network}/{site}"),
        ),
        (
            "no resource path at all",
            format!("arn:aws:networkmanager::{account}:site"),
        ),
        (
            "an empty resource type",
            format!("arn:aws:networkmanager::{account}:/{site}"),
        ),
        (
            "an empty resource id",
            format!("arn:aws:networkmanager::{account}:site/"),
        ),
        (
            "an empty owner segment ahead of the id",
            format!("arn:aws:networkmanager::{account}:site//{site}"),
        ),
        (
            "a resource path one level too deep",
            format!("arn:aws:networkmanager::{account}:site/{network}/{site}/{site}"),
        ),
    ];

    let mut answers: Vec<(String, Value)> = Vec::new();
    for (form, arn) in &malformed {
        for call in calls_for(&network, arn) {
            let case = format!("{} on an ARN member that is {form}", call.operation);
            let (status, answer) = emulator.call(call.method, &call.uri, call.body).await;
            assert_eq!(
                status, 400,
                "case {case}: expected 400, got {status} with {answer}"
            );
            assert_eq!(
                error_code(&answer),
                "ValidationException",
                "case {case}: expected ValidationException, got {answer}"
            );
            assert_eq!(
                answer["Reason"].as_str(),
                Some("FieldValidationFailed"),
                "case {case}: expected Reason FieldValidationFailed, got {answer}"
            );
            assert_eq!(
                answer["Message"].as_str(),
                Some(MALFORMED_ARN_MESSAGE),
                "case {case}: expected Message {MALFORMED_ARN_MESSAGE:?}, got {answer}"
            );
            assert_eq!(
                answer["Fields"],
                json!([{
                    "Name": FIELD_RESOURCE_ARN,
                    "Message": MALFORMED_ARN_FIELD_MESSAGE,
                }]),
                "case {case}: expected one Fields entry naming {FIELD_RESOURCE_ARN:?}, got {answer}"
            );
            answers.push((case, answer));
        }
    }

    // And every one of them is the SAME refusal: the operations that reach a resource only
    // through its ARN and the one that carries it beside a global network id agree on the
    // prose, the reason and the member they name, whatever broke the string.
    let (first_case, first) = &answers[0];
    for (case, answer) in &answers[1..] {
        assert_eq!(
            refusal_of(answer),
            refusal_of(first),
            "case {case}: its Message, Reason and Fields must match {first_case}, got {answer}"
        );
    }

    // The control row: the same four operations answer the well-formed ARN of the stored
    // site, so each refusal above turns on the form of the string and nothing else.
    for call in calls_for(&network, &site_arn) {
        let case = format!("{} on the stored site's own ARN", call.operation);
        let (status, answer) = emulator.call(call.method, &call.uri, call.body).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200, got {status} with {answer}"
        );
    }

    // No refused call stored a tag on the site: the tag the refused `TagResource` rows
    // carried reached no resource, and the accepted one above was removed again by the
    // accepted `UntagResource`.
    let (status, listed) = emulator.get(&tags_uri(&site_arn)).await;
    assert_eq!(
        status, 200,
        "case ListTagsForResource after the refusals: expected 200, got {status} with {listed}"
    );
    assert_eq!(
        listed["TagList"],
        json!([]),
        "case ListTagsForResource after the refusals: expected no stored tags, got {listed}"
    );
}
