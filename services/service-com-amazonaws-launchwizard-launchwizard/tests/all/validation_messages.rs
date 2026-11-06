// SPDX-License-Identifier: CC0-1.0

//! The rules this service builds a `ValidationException` message by, driven over HTTP
//! through the same router the production emulator serves with.
//!
//! Three rules, each asserted over a TABLE of inputs rather than one recorded call:
//!
//! 1. A member that failed one of its own modelled constraints is reported as
//!    `Value at '<memberPath>' failed to satisfy constraint: Member must <requirement>`
//!    — the rejected value is NOT echoed, and a `@pattern` is stated without the
//!    `^`/`$` the model spells "whole value" with.
//! 2. A scalar ELEMENT of a list that failed the element shape's constraints is
//!    reported at the COLLECTION member's path, with that shape's complete constraint
//!    set composed into one requirement — or, where the report names none of the
//!    element shapes a member path covers, with what those shapes declare alike
//!    (`tests/all/filter_value_element_expressions.rs` owns that rule).
//! 3. A `nextToken` this service cannot use is refused with one fixed sentence, because
//!    the token is opaque.
//!
//! The rows differ along what each rule generalises over: the member, the constraint
//! kind (length floor, length ceiling, range bound, pattern, absence), the position and
//! count of the offending element, and which of several element constraints it broke.

use serde_json::{json, Value};

use crate::deployment_lifecycle::create;
use crate::harness::Harness;
// The tagging suite owns the `/tags/{resourceArn}` request shape; reached from here
// rather than rebuilt, so the two suites cannot disagree about how a tagging call is
// addressed.
use crate::resource_tagging::{deployment_arn, encode_component};

/// The wire error code and HTTP status every one of these refusals carries
/// (`ValidationException`, `errorKind: client`, `httpError: 400` —
/// `smithy_query(query="shape",shape="ValidationException")`).
const CODE: &str = "ValidationException";
/// See [`CODE`].
const STATUS: u16 = 400;

/// A well-formed `specifications` map for the rows below, which are about a member
/// OTHER than `specifications`: each one is refused at the BOUNDARY, before the handler
/// resolves the deployment pattern and judges the settings against the schema that
/// pattern publishes, so what this map holds is not what those rows assert. A row that
/// has to get PAST the boundary fills in the pattern's own published schema instead
/// (`deployment_lifecycle::required_specifications`).
fn conforming_specifications() -> Value {
    json!({"KeyPairName": "a-key-pair", "VpcId": "vpc-0123456789abcdef0"})
}

/// The `Message` of a refusal, after requiring the modelled status and code.
///
/// `pub(crate)` so a suite about one of these rules drives the refusal through the same
/// two requirements rather than restating them.
pub(crate) async fn refusal_message(
    harness: &Harness,
    case: &str,
    path: &str,
    body: Value,
) -> String {
    let reply = harness.refused(case, path, body, STATUS, CODE).await;
    message_of(case, &reply)
}

/// The `message` member of a `ValidationException` body, whichever casing the envelope
/// spells it with.
fn message_of(case: &str, body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no message in the refusal body {body}"))
        .to_string()
}

/// A forged pagination token in the shape `crate::state::encode_token` issues them in,
/// through the same `hex` crate that function encodes with, so a test token cannot drift
/// from the real encoding.
fn forged_token(cursor: &str) -> String {
    hex::encode(cursor)
}

/// One row of the scalar-member table.
struct Scalar {
    case: &'static str,
    path: &'static str,
    body: Value,
    /// The whole message the rule produces for this row.
    expected: String,
    /// The rejected value as the request rendered it, which must NOT appear in the
    /// message. `None` where the value is empty and so trivially a substring.
    rejected: Option<String>,
}

#[tokio::test]
async fn a_member_constraint_names_the_member_and_the_requirement_without_the_value() {
    let harness = Harness::new();
    let too_long_workload = "A".repeat(101);
    // Every member below is from this service's model
    // (`smithy_query(query="closure",operation="GetWorkload,GetDeployment,ListDeployments,CreateDeployment")`):
    // WorkloadName @length 1..100 @pattern ^[A-Za-z][a-zA-Z0-9-_]*$, MaxDeploymentResults
    // @range 1..100, NextToken @length 1..2048, DeploymentName @length 1..50 @pattern
    // ^[A-Za-z0-9_\.-]+$, DeploymentPatternName @length 1..256 @pattern
    // ^[A-Za-z0-9][a-zA-Z0-9-]*$, GetDeploymentInput$deploymentId @required.
    let rows = vec![
        Scalar {
            case: "workloadName breaks its pattern",
            path: "/getWorkload",
            body: json!({"workloadName": "1InvalidLeadingDigit"}),
            expected: scalar_clause(
                "workloadName",
                r"Member must satisfy regular expression pattern: [A-Za-z][a-zA-Z0-9-_]*",
            ),
            rejected: Some("1InvalidLeadingDigit".to_string()),
        },
        Scalar {
            case: "workloadName one past its length ceiling",
            path: "/getWorkload",
            body: json!({"workloadName": too_long_workload.clone()}),
            expected: scalar_clause(
                "workloadName",
                "Member must have length less than or equal to 100",
            ),
            rejected: Some(too_long_workload),
        },
        Scalar {
            case: "maxResults one past its range ceiling",
            path: "/listDeployments",
            body: json!({"maxResults": 101}),
            expected: scalar_clause(
                "maxResults",
                "Member must have value less than or equal to 100",
            ),
            rejected: Some("101".to_string()),
        },
        Scalar {
            case: "maxResults one below its range floor",
            path: "/listDeployments",
            body: json!({"maxResults": 0}),
            expected: scalar_clause(
                "maxResults",
                "Member must have value greater than or equal to 1",
            ),
            rejected: None,
        },
        Scalar {
            case: "nextToken below its length floor",
            path: "/listWorkloads",
            body: json!({"nextToken": ""}),
            expected: scalar_clause(
                "nextToken",
                "Member must have length greater than or equal to 1",
            ),
            rejected: None,
        },
        Scalar {
            case: "a deployment name breaks its pattern",
            path: "/createDeployment",
            body: json!({
                "workloadName": "SAP",
                "deploymentPatternName": "SapHanaSingle",
                "name": "bad name",
                "specifications": conforming_specifications(),
                "dryRun": true,
            }),
            expected: scalar_clause(
                "name",
                r"Member must satisfy regular expression pattern: [A-Za-z0-9_\.-]+",
            ),
            rejected: Some("bad name".to_string()),
        },
        Scalar {
            case: "a deployment pattern name breaks its pattern",
            path: "/createDeployment",
            body: json!({
                "workloadName": "SAP",
                "deploymentPatternName": "-SapHanaSingle",
                "name": "valid-name",
                "specifications": conforming_specifications(),
                "dryRun": true,
            }),
            expected: scalar_clause(
                "deploymentPatternName",
                "Member must satisfy regular expression pattern: [A-Za-z0-9][a-zA-Z0-9-]*",
            ),
            rejected: Some("-SapHanaSingle".to_string()),
        },
        Scalar {
            // The one "value" AWS does state, because there was none to withhold.
            case: "a required member is absent",
            path: "/getDeployment",
            body: json!({}),
            expected: "1 validation error detected: Value null at 'deploymentId' failed to \
                       satisfy constraint: Member must not be null"
                .to_string(),
            rejected: None,
        },
    ];

    for row in rows {
        let case = row.case;
        let message = refusal_message(&harness, case, row.path, row.body).await;
        assert_eq!(
            message, row.expected,
            "case {case:?}: expected message {:?}, got {message:?}",
            row.expected
        );
        if let Some(rejected) = &row.rejected {
            assert!(
                !message.contains(rejected.as_str()),
                "case {case:?}: the message echoes the rejected value {rejected:?}: {message:?}"
            );
        }
        assert!(
            !message.contains('^') && !message.contains('$'),
            "case {case:?}: a scalar member's requirement states no anchors: {message:?}"
        );
    }
}

/// The whole message for one member that failed one of its own constraints.
fn scalar_clause(member: &str, requirement: &str) -> String {
    format!(
        "1 validation error detected: Value at '{member}' failed to satisfy constraint: \
         {requirement}"
    )
}

/// The composed requirement for an element of `DeploymentFilter$values`
/// (`DeploymentFilterValues` of `DeploymentFilterValue`: @length 1..100, @pattern
/// `(^(([A-Za-z])+([0-9A-Za-z\-\_])+)$)`).
///
/// `pub(crate)` so the one test-side statement of this constraint set is reachable from
/// the other suites in this crate rather than restated in each of them.
pub(crate) const FILTER_VALUE_CONSTRAINTS: &str =
    "[Member must have length less than or equal to 100, Member must have length greater \
     than or equal to 1, Member must satisfy regular expression pattern: \
     .*(^(([A-Za-z])+([0-9A-Za-z\\-\\_])+)$).*]";

/// The composed requirement for an element of `UntagResourceInput$tagKeys` (`TagKeyList`
/// of `TagKey`: @length 1..128, @pattern `^[a-zA-Z0-9 _.:/=+@-]+$`) — an entry the issue
/// did not cite.
const TAG_KEY_CONSTRAINTS: &str = "[Member must have length less than or equal to 128, \
     Member must have length greater than or equal to 1, Member must satisfy regular expression \
     pattern: .*^[a-zA-Z0-9 _.:/=+@-]+$.*]";

/// The composed requirement for an element of a `filters[].values` list the report does
/// not name the element SHAPE of: the `@length` bounds `DeploymentFilterValue` and
/// `DeploymentPatternVersionFilterValue` BOTH declare, and no expression, because the
/// two shapes declare different ones and only one of them judged the request.
///
/// Reached by a value that breaks the length bounds alone: `ListDeployments$filters[]
/// .values` and `ListDeploymentPatternVersions$filters[].values` are the same member
/// path, and a length failure names neither list. `tests/all/
/// filter_value_element_expressions.rs` is the suite about that rule; this constant is
/// the row of this table that reaches it.
pub(crate) const SHARED_FILTER_VALUE_CONSTRAINTS: &str =
    "[Member must have length less than or equal to 100, Member must have length greater \
     than or equal to 1]";

/// The whole message for an element of the list at `collection`.
///
/// `pub(crate)` so the one test-side statement of this wording is reachable from the
/// other suites in this crate rather than restated in each of them.
pub(crate) fn composed_clause(collection: &str, constraints: &str) -> String {
    format!(
        "1 validation error detected: Value at '{collection}' failed to satisfy constraint: \
         Member must satisfy constraint: {constraints}"
    )
}

/// A stored deployment, so a tagging row names a resource that exists — a tagging call
/// decides the resource before it judges the tags or keys it was given.
async fn tagged_deployment(harness: &Harness) -> String {
    create(
        harness,
        "a deployment to untag",
        "SAP",
        "SapHanaSingle",
        "untag-message-subject",
    )
    .await
}

#[tokio::test]
async fn a_list_element_is_reported_at_its_collection_with_the_composed_element_constraints() {
    let harness = Harness::new();
    let too_long_value = "A".repeat(101);
    // Rows differ in which filter key carries the offending value, in the position of
    // the element inside its list, in the position of the list's own filter, and in
    // WHICH of the element shape's three constraints the value broke. Each row states
    // the constraints the REPORT names — the element shape's whole declared set where
    // the value broke the expression the walk judged it by (which names that shape), and
    // the set the two lists sharing this member path declare alike where it broke the
    // length bounds alone (which names neither of them, since both declare 1..100 and
    // different expressions). `tests/all/filter_value_element_expressions.rs` is the
    // suite about that rule.
    let rows: Vec<(&str, Value, &str, &str)> = vec![
        (
            "a DEPLOYMENT_STATUS value breaks the element pattern",
            json!({"filters": [{"name": "DEPLOYMENT_STATUS", "values": ["not a valid status!!"]}],
                   "maxResults": 10}),
            "filters.1.member.values",
            FILTER_VALUE_CONSTRAINTS,
        ),
        (
            "a WORKLOAD_NAME value breaks the element pattern",
            json!({"filters": [{"name": "WORKLOAD_NAME", "values": ["not a valid name!!"]}]}),
            "filters.1.member.values",
            FILTER_VALUE_CONSTRAINTS,
        ),
        (
            "the offending value is the second of two, in the second of two filters",
            json!({"filters": [
                {"name": "WORKLOAD_NAME", "values": ["SAP"]},
                {"name": "DEPLOYMENT_STATUS", "values": ["COMPLETED", "not a valid status!!"]},
            ]}),
            "filters.2.member.values",
            FILTER_VALUE_CONSTRAINTS,
        ),
        (
            // An empty element breaks the element EXPRESSION as well as the length
            // floor, so the report names the element shape and the whole set is stated.
            "an element below the element length floor",
            json!({"filters": [{"name": "WORKLOAD_NAME", "values": [""]}]}),
            "filters.1.member.values",
            FILTER_VALUE_CONSTRAINTS,
        ),
        (
            // This one is inside both expressions, so only the ceiling is reported and
            // the report names neither of the two lists.
            "an element one past the element length ceiling",
            json!({"filters": [{"name": "WORKLOAD_NAME", "values": [too_long_value.clone()]}]}),
            "filters.1.member.values",
            SHARED_FILTER_VALUE_CONSTRAINTS,
        ),
    ];

    for (case, body, collection, constraints) in rows {
        let expected = composed_clause(collection, constraints);
        let message = refusal_message(&harness, case, "/listDeployments", body).await;
        assert_eq!(
            message, expected,
            "case {case:?}: expected message {expected:?}, got {message:?}"
        );
        assert!(
            !message.contains(&format!("{collection}.1.member")),
            "case {case:?}: the message names an element position: {message:?}"
        );
    }

    // The value is never echoed, whichever element constraint it broke.
    for (case, value) in [
        ("pattern", "not a valid status!!"),
        ("length ceiling", too_long_value.as_str()),
    ] {
        let message = refusal_message(
            &harness,
            case,
            "/listDeployments",
            json!({"filters": [{"name": "DEPLOYMENT_STATUS", "values": [value]}]}),
        )
        .await;
        assert!(
            !message.contains(value),
            "case {case:?}: the message echoes the rejected element {value:?}: {message:?}"
        );
    }
}

#[tokio::test]
async fn the_composed_rendering_covers_every_constrained_list_the_model_declares() {
    let harness = Harness::new();
    // `UntagResource` binds `tagKeys` to the query string of a DELETE, so this list is
    // reached by a different protocol binding than `filters` — and the same rule holds.
    //
    // The resource has to EXIST for the rule to be reached at all: a tagging call reads
    // its `resourceArn` and looks the resource up before it judges the shape of the keys
    // it was given (an unparseable ARN is a `BadRequestException`, and an ARN naming
    // nothing a `NotFoundException`, whatever the key list looks like). So these rows
    // name a real deployment, and the refusal below is the key list's own.
    let arn = deployment_arn(&tagged_deployment(&harness).await);
    let expected = composed_clause("tagKeys", TAG_KEY_CONSTRAINTS);
    for (case, query) in [
        ("a tag key breaks the element pattern", "bad%20key%21"),
        ("a tag key below the element length floor", ""),
    ] {
        let reply = harness
            .send(
                "DELETE",
                &format!("/tags/{}?tagKeys={query}", encode_component(&arn)),
                None,
            )
            .await;
        assert_eq!(
            reply.status, STATUS,
            "case {case:?}: expected HTTP {STATUS}, got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code.as_deref(),
            Some(CODE),
            "case {case:?}: expected error code {CODE:?}, got {:?}",
            reply.error_code
        );
        let message = message_of(case, &reply.body);
        assert_eq!(
            message, expected,
            "case {case:?}: expected message {expected:?}, got {message:?}"
        );
    }

    // A tag key inside the element bounds passes the validation layer: whatever the
    // handler then answers, it is not this refusal.
    let admitted = harness
        .send(
            "DELETE",
            &format!("/tags/{}?tagKeys=Owner", encode_component(&arn)),
            None,
        )
        .await;
    assert_ne!(
        admitted.status, STATUS,
        "case \"a valid tag key\": a key inside the element bounds must not be refused, got body {}",
        admitted.body
    );
}

#[tokio::test]
async fn an_unusable_next_token_is_refused_with_the_fixed_pagination_message() {
    let harness = Harness::new();
    // One deployment, so `ListDeploymentEvents` has a resource to page the events of.
    // Created through the shared helper, so its settings are the ones the pattern's own
    // schema asks for.
    let deployment = create(&harness, "seed", "SAP", "SapHanaSingle", "token-suite").await;

    // A real token, so one row can tamper with a token this service really issued.
    let first_page = harness
        .ok(
            "first page",
            "/listWorkloads",
            json!({"maxResults": 1}),
        )
        .await;
    let issued = first_page["nextToken"]
        .as_str()
        .expect("a catalogue paged at one hands back a token")
        .to_string();
    let tampered = tamper(&issued);

    // Every operation the model paginates, against every way a token can be unusable.
    let operations: Vec<(&str, Value)> = vec![
        ("/listWorkloads", json!({})),
        ("/listDeployments", json!({})),
        (
            "/listDeploymentEvents",
            json!({"deploymentId": deployment.clone()}),
        ),
    ];
    let tokens = vec![
        ("not a token this service ever issued", "notatokenatall".to_string()),
        (
            "a token whose decoding carries no cursor",
            forged_token("v2:SAP"),
        ),
        (
            "a well-formed token naming nothing in the result set",
            forged_token("v1:NoSuchPositionAtAll"),
        ),
        ("a token issued for another result set", issued.clone()),
        ("a token with its last character flipped", tampered),
    ];

    for (path, base) in &operations {
        for (why, token) in &tokens {
            // A token issued by `ListWorkloads` is a usable token THERE, so that one
            // row belongs to the other operations only.
            if *path == "/listWorkloads" && token == &issued {
                continue;
            }
            let case = format!("{path}: {why}");
            let mut body = base.as_object().cloned().expect("a request body is an object");
            body.insert("nextToken".to_string(), json!(token));
            let message =
                refusal_message(&harness, &case, path, Value::Object(body)).await;
            assert_eq!(
                message, "NextToken is not valid.",
                "case {case:?}: expected the fixed pagination message, got {message:?}"
            );
            assert!(
                !message.contains(token.as_str()),
                "case {case:?}: the message echoes the rejected token {token:?}"
            );
        }
    }

    // And the token the service did issue still works, so the refusal is about the
    // token being unusable rather than about tokens.
    let resumed = harness
        .ok(
            "resumed",
            "/listWorkloads",
            json!({"maxResults": 1, "nextToken": issued}),
        )
        .await;
    assert!(
        resumed["workloads"]
            .as_array()
            .is_some_and(|page| page.len() == 1),
        "an issued token must still name the next page, got {}",
        resumed["workloads"]
    );
}

/// `token` with its last character replaced by a different one — a token the caller
/// mangled, which this service never issued.
fn tamper(token: &str) -> String {
    let mut flipped: String = token.chars().take(token.chars().count() - 1).collect();
    let last = token.chars().next_back().expect("a token is not empty");
    flipped.push(if last == 'a' { 'b' } else { 'a' });
    flipped
}
