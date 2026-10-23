// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for this service's ERROR WIRE CONTRACT — the three rules a client
//! branches on, each asserted over a table of inputs rather than one recorded case:
//!
//! 1. The ARN-addressed tagging operations (`TagResource`, `UntagResource`,
//!    `ListTagsForResources`) answer a `ResourceArn` this account has no record of with
//!    the error code `NotFoundException` at HTTP **400** — the status and code this
//!    service puts on the wire for them, neither of which is the modelled
//!    `ResourceNotFoundException` (404) those operations list. The NAME-addressed
//!    Get/Delete operations are the ones that keep the modelled 404, which rule 1's
//!    table asserts side by side so the two are not confused.
//! 2. The 403 denial of a create carries the error code the recording pins for THAT
//!    operation: `CreateReadinessCheck` answers `AccessDenied`, while `CreateCell`,
//!    `CreateRecoveryGroup` and `CreateResourceSet` answer the `AccessDeniedException`
//!    shape name. On restJson1 the `x-amzn-errortype` header IS the code an SDK
//!    resolves the modelled exception from, so it belongs to the operation rather than
//!    being a rendering of the shape's name. The status is the shape's modelled 403 for
//!    all four.
//! 3. An unsupported `ListRules` `resourceType` is refused by the OPERATION's own
//!    unknown-type check: HTTP 400 `ValidationException` with a constant sentence that
//!    neither echoes the rejected value nor enumerates the supported types. The member
//!    targets a plain `__string` (model: `smithy_query(query="operation",
//!    operation="ListRules")` — no enum, no pattern, no value list), so there is no
//!    modelled constraint for a generated "Member must be one of …" report to describe,
//!    and the emulator's catalog is not part of the wire contract.
//!
//! Every assertion goes over the wire through the same [`MultiServiceRouter`] the
//! production emulator mounts, so only the HTTP status, the `x-amzn-errortype` header
//! and the JSON body are inspected.
//!
//! The rows differ along what each rule generalises over: which of the four resource
//! kinds an ARN names, which tagging operation addresses it, how far the ARN is from one
//! of this service's (another kind, another service, not an ARN at all, empty), which
//! create is denied and which optional members it carries, and the form of the rejected
//! resource type (case, pluralisation, an ARN, empty, a type of no AWS service).

use serde_json::{json, Value};

use crate::readiness_check_lifecycle::{Emulator, Reply};

/// The constant sentence the tagging operations' not-found answer carries. Written out
/// rather than imported, so the test pins the string.
const TAGGING_NOT_FOUND_MESSAGE: &str = "Resource was not found";

/// The constant sentence the name-addressed operations' modelled not-found carries.
const RESOURCE_NOT_FOUND_MESSAGE: &str = "Resource not found";

/// The sentence the account-eligibility gate denies a create with.
const ACCOUNT_DENIED_MESSAGE: &str = "This operation is currently unavailable for new accounts.";

/// The sentence an unsupported `ListRules` resource type is refused with.
const UNKNOWN_RESOURCE_TYPE_MESSAGE: &str = "Unknown resource type.";

/// The resource types this API supports, written out from the `ResourceSetType` member's
/// own documentation (model: `smithy_query(query="doc",
/// shape="CreateResourceSetRequest", member="ResourceSetType")`) rather than imported
/// from the implementation. Used here to assert the refusal message leaks NONE of them,
/// and to check that a supported value — including entries the issue did not cite — is
/// not refused.
const DOCUMENTED_RESOURCE_TYPES: &[&str] = &[
    "AWS::ApiGateway::Stage",
    "AWS::ApiGatewayV2::Stage",
    "AWS::AutoScaling::AutoScalingGroup",
    "AWS::CloudWatch::Alarm",
    "AWS::DynamoDB::Table",
    "AWS::EC2::CustomerGateway",
    "AWS::EC2::VPC",
    "AWS::EC2::VPNConnection",
    "AWS::EC2::VPNGateway",
    "AWS::EC2::Volume",
    "AWS::ElasticLoadBalancing::LoadBalancer",
    "AWS::ElasticLoadBalancingV2::LoadBalancer",
    "AWS::Lambda::Function",
    "AWS::MSK::Cluster",
    "AWS::RDS::DBCluster",
    "AWS::Route53::HealthCheck",
    "AWS::Route53RecoveryReadiness::DNSTargetResource",
    "AWS::SNS::Subscription",
    "AWS::SNS::Topic",
    "AWS::SQS::Queue",
];

/// The ARN form these resources carry. Route 53 Application Recovery Controller
/// readiness is a GLOBAL service, so the region segment is empty. Written out rather
/// than taken from the handler's helper, so the test pins the format.
fn arn(resource_type: &str, name: &str) -> String {
    format!(
        "arn:aws:route53-recovery-readiness::{account}:{resource_type}/{name}",
        account = engine::EMULATOR_ACCOUNT_ID
    )
}

/// A value as ONE URI path segment: `{ResourceArn}` is a non-greedy `@httpLabel`, so an
/// SDK percent-encodes the ARN's `:` and `/` before sending it.
fn label(value: &str) -> String {
    value
        .replace('%', "%25")
        .replace(':', "%3A")
        .replace('/', "%2F")
}

/// Which of the four readiness resources a row addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Cell,
    RecoveryGroup,
    ReadinessCheck,
    ResourceSet,
}

impl Kind {
    const ALL: [Kind; 4] = [
        Kind::Cell,
        Kind::RecoveryGroup,
        Kind::ReadinessCheck,
        Kind::ResourceSet,
    ];

    /// The collection URI, from the model's `@http` bindings.
    fn collection(self) -> &'static str {
        match self {
            Kind::Cell => "/cells",
            Kind::RecoveryGroup => "/recoverygroups",
            Kind::ReadinessCheck => "/readinesschecks",
            Kind::ResourceSet => "/resourcesets",
        }
    }

    /// The resource-type segment of this kind's ARN.
    fn arn_type(self) -> &'static str {
        match self {
            Kind::Cell => "cell",
            Kind::RecoveryGroup => "recovery-group",
            Kind::ReadinessCheck => "readiness-check",
            Kind::ResourceSet => "resource-set",
        }
    }

    /// The wire error code this kind's CREATE answers its 403 denial with, as recorded
    /// per operation: `CreateReadinessCheck`'s denial carries `AccessDenied`, the other
    /// three carry the `AccessDeniedException` shape name.
    fn denied_error_code(self) -> &'static str {
        match self {
            Kind::ReadinessCheck => "AccessDenied",
            Kind::Cell | Kind::RecoveryGroup | Kind::ResourceSet => "AccessDeniedException",
        }
    }

    /// The smallest create request that stores a record under `name`.
    fn create_body(self, name: &str) -> Value {
        match self {
            Kind::Cell => json!({"cellName": name}),
            Kind::RecoveryGroup => json!({"recoveryGroupName": name}),
            Kind::ReadinessCheck => {
                json!({"readinessCheckName": name, "resourceSetName": "watched_set"})
            }
            Kind::ResourceSet => json!({
                "resourceSetName": name,
                "resourceSetType": "AWS::SQS::Queue",
                "resources": [],
            }),
        }
    }
}

/// Assert `reply` is the refusal `status` / `code` / `message` names.
fn assert_refused(case: &str, reply: &Reply, status: u16, code: &str, message: &str) {
    assert_eq!(
        reply.status, status,
        "case {case:?}: expected HTTP {status}, got {} / {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_code(),
        code,
        "case {case:?}: expected error code {code:?}, got {:?} / {}",
        reply.error_code(),
        reply.body
    );
    let actual = reply.string("message").unwrap_or_default();
    assert_eq!(
        actual, message,
        "case {case:?}: expected the message {message:?}, got {actual:?}"
    );
}

/// The three calls that address a resource by ARN: `(operation, method, uri, body)`.
fn tagging_calls(resource_arn: &str) -> Vec<(&'static str, &'static str, String, Option<Value>)> {
    let encoded = label(resource_arn);
    vec![
        (
            "ListTagsForResources",
            "GET",
            format!("/tags/{encoded}"),
            None,
        ),
        (
            "TagResource",
            "POST",
            format!("/tags/{encoded}"),
            Some(json!({"tags": {"k": "v"}})),
        ),
        (
            "UntagResource",
            "DELETE",
            format!("/tags/{encoded}?tagKeys=k"),
            None,
        ),
    ]
}

/// RULE 1: every ARN-addressed tagging call for a `ResourceArn` this account has no
/// record of answers HTTP 400 with the error code `NotFoundException` and this service's
/// constant sentence — whatever the ARN is, and on every one of the three operations.
///
/// Rows differ along which kind the unresolvable ARN names, whether the same NAME exists
/// under a different kind, and how far from one of this service's ARNs the value is: a
/// foreign service's ARN, a string that is not an ARN at all, and the empty value.
#[tokio::test]
async fn the_tagging_operations_answer_an_unresolvable_arn_with_400_not_found() {
    let emulator = Emulator::new();
    // A cell named `twin` exists; the resource-set ARN for `twin` does not. One stored
    // resource of each kind, so a refusal is about the addressed ARN rather than an
    // empty store.
    for kind in Kind::ALL {
        let reply = emulator
            .send("POST", kind.collection(), Some(kind.create_body("twin")))
            .await;
        assert_eq!(
            reply.status, 200,
            "an eligible account's create must answer HTTP 200, got {} / {}",
            reply.status, reply.body
        );
    }

    let mut rows: Vec<(String, String)> = Vec::new();
    for kind in Kind::ALL {
        rows.push((
            format!("a {} ARN nobody created", kind.arn_type()),
            arn(kind.arn_type(), "never_created"),
        ));
    }
    rows.push((
        "a cell ARN with a 64-character name".to_string(),
        arn("cell", &"n".repeat(64)),
    ));
    rows.push((
        "an ARN of another service".to_string(),
        "arn:aws:sqs:us-west-2:000000000000:q1".to_string(),
    ));
    rows.push((
        "an ARN of this service in another account".to_string(),
        "arn:aws:route53-recovery-readiness::000000000001:cell/twin".to_string(),
    ));
    rows.push((
        "a value that is not an ARN at all".to_string(),
        "not-an-arn".to_string(),
    ));
    rows.push((
        "an ARN whose resource type this service has none of".to_string(),
        arn("made-up-type", "twin"),
    ));

    for (row, resource_arn) in rows {
        for (operation, method, uri, body) in tagging_calls(&resource_arn) {
            let case = format!("{operation} / {row}");
            let reply = emulator.send(method, &uri, body).await;
            assert_refused(
                &case,
                &reply,
                400,
                "NotFoundException",
                TAGGING_NOT_FOUND_MESSAGE,
            );
            let message = reply.string("message").unwrap_or_default();
            assert!(
                !message.contains(&resource_arn),
                "case {case:?}: the constant message must not echo the rejected ARN, got {message:?}"
            );
        }
    }

    // The empty label is a different URI, so it is sent on its own: whatever the router
    // makes of it, it must not be a success.
    for (operation, method, uri, body) in tagging_calls("") {
        let case = format!("{operation} / an empty resource ARN");
        let reply = emulator.send(method, &uri, body).await;
        assert!(
            reply.status >= 400,
            "case {case:?}: an empty resource ARN must be refused, got {} / {}",
            reply.status,
            reply.body
        );
    }

    // THE OTHER HALF of the rule: the NAME-addressed operations keep the modelled
    // `ResourceNotFoundException` and its 404, so the two not-found answers of this
    // service stay distinguishable.
    for (operation, method, uri) in [
        ("GetCell", "GET", "/cells/never_created"),
        ("DeleteCell", "DELETE", "/cells/never_created"),
        ("GetRecoveryGroup", "GET", "/recoverygroups/never_created"),
        ("GetReadinessCheck", "GET", "/readinesschecks/never_created"),
        ("GetResourceSet", "GET", "/resourcesets/never_created"),
        ("DeleteResourceSet", "DELETE", "/resourcesets/never_created"),
    ] {
        let case = format!("{operation} / a name nobody created");
        let reply = emulator.send(method, uri, None).await;
        assert_refused(
            &case,
            &reply,
            404,
            "ResourceNotFoundException",
            RESOURCE_NOT_FOUND_MESSAGE,
        );
    }

    // And an ARN that DOES resolve is answered, so the 400 above is about the ARN not
    // resolving rather than about the tagging surface being refused wholesale.
    for kind in Kind::ALL {
        let resource_arn = arn(kind.arn_type(), "twin");
        let tagged = emulator
            .send(
                "POST",
                &format!("/tags/{}", label(&resource_arn)),
                Some(json!({"tags": {"k": "v"}})),
            )
            .await;
        assert_eq!(
            tagged.status, 200,
            "case {kind:?}: tagging a stored resource must succeed, got {} / {}",
            tagged.status, tagged.body
        );
        let listed = emulator
            .send("GET", &format!("/tags/{}", label(&resource_arn)), None)
            .await;
        assert_eq!(
            listed.field("tags"),
            Some(&json!({"k": "v"})),
            "case {kind:?}: expected the written tag to be reported, got {}",
            listed.body
        );
    }
}

/// RULE 2: the 403 denial of a create carries the error code the recording pins for
/// THAT operation — `AccessDenied` for `CreateReadinessCheck`, the `AccessDeniedException`
/// shape name for the other three — with the status the shape's `@httpError` declares
/// and the gate's constant sentence.
///
/// Rows differ along which create is denied (all four, each with its own expected code),
/// name form and length, and which optional members the request carries: the code
/// depends on the operation and on none of the rest.
#[tokio::test]
async fn a_denial_carries_the_wire_error_code_of_the_operation_that_answers_it() {
    let emulator = Emulator::with_account_eligibility(false);

    for kind in Kind::ALL {
        let expected_code = kind.denied_error_code();
        let mut rows: Vec<(String, Value)> = Vec::new();
        for (row, name) in [
            ("the shortest name the pattern admits", "a".to_string()),
            ("a plain name", "denied_1".to_string()),
            ("a 64-character name, the longest reported", "n".repeat(64)),
        ] {
            rows.push((row.to_string(), kind.create_body(&name)));
        }
        // The same request carrying every optional member it has.
        let mut full = kind.create_body("with_every_member");
        full["tags"] = json!({"env": "prod", "team": "arc"});
        if matches!(kind, Kind::Cell | Kind::RecoveryGroup) {
            full["cells"] = json!([arn("cell", "some_other_cell")]);
        }
        rows.push(("every optional member present".to_string(), full));

        for (row, body) in rows {
            let case = format!("{kind:?} / {row}");
            let denied = emulator.send("POST", kind.collection(), Some(body)).await;
            assert_refused(
                &case,
                &denied,
                403,
                expected_code,
                ACCOUNT_DENIED_MESSAGE,
            );
        }
    }
}

/// RULE 3: an unsupported `ListRules` `resourceType` is refused by the operation's own
/// unknown-type check — HTTP 400 `ValidationException` with a CONSTANT sentence that
/// neither echoes the rejected value nor enumerates the supported types — while every
/// documented type is answered.
///
/// Rows differ along the form of the rejected value: case, pluralisation, a type of no
/// AWS service, an ARN, a one-segment value, and a very long value.
#[tokio::test]
async fn list_rules_refuses_an_unknown_resource_type_with_a_constant_message() {
    let emulator = Emulator::new();

    let long_value = format!("AWS::{}::Thing", "X".repeat(200));
    let rejected: Vec<(&str, String)> = vec![
        ("the value the issue recorded", "Not::A::RealType".to_string()),
        (
            "a supported type with a trailing s",
            "AWS::SQS::Queues".to_string(),
        ),
        (
            "a supported type in the wrong case",
            "aws::sqs::queue".to_string(),
        ),
        (
            "a well-formed type of no AWS service",
            "AWS::Fake::Thing".to_string(),
        ),
        ("a one-segment value", "Queue".to_string()),
        (
            "an ARN rather than a type",
            "arn:aws:sqs:::queue".to_string(),
        ),
        ("a value far longer than any type", long_value),
    ];

    for (row, value) in rejected {
        let case = format!("unknown resourceType / {row}");
        let reply = emulator
            .send("GET", &format!("/rules?resourceType={}", label(&value)), None)
            .await;
        assert_refused(
            &case,
            &reply,
            400,
            "ValidationException",
            UNKNOWN_RESOURCE_TYPE_MESSAGE,
        );
        let message = reply.string("message").unwrap_or_default();
        assert!(
            !message.contains(&value),
            "case {case:?}: the constant message must not echo the rejected value, got {message:?}"
        );
        for supported in DOCUMENTED_RESOURCE_TYPES {
            assert!(
                !message.contains(supported),
                "case {case:?}: the message must not publish the supported-type catalog \
                 ({supported}), got {message:?}"
            );
        }
    }

    // Every documented type is answered instead of refused — including entries the issue
    // did not cite — so the refusal is about the value being unknown.
    for value in DOCUMENTED_RESOURCE_TYPES {
        let case = format!("supported resourceType / {value}");
        let reply = emulator
            .send("GET", &format!("/rules?resourceType={value}"), None)
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: a documented resource type must be answered, got {} / {}",
            reply.status, reply.body
        );
        let rules = reply
            .field("rules")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert!(
            !rules.is_empty(),
            "case {case:?}: a documented resource type has readiness rules, got {}",
            reply.body
        );
        for rule in &rules {
            assert_eq!(
                rule.get("resourceType").and_then(Value::as_str),
                Some(*value),
                "case {case:?}: the filter must admit only that type, got {rule}"
            );
        }
    }
}
