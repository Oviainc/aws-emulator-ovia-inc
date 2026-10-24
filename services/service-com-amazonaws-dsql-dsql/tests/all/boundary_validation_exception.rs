// SPDX-License-Identifier: CC0-1.0

//! The BODY of every refusal this service answers at the MODEL BOUNDARY — a request
//! refused before any handler ran.
//!
//! RULE under test: a request refused for a constraint the model declares on one of
//! its input members — a value outside a `@pattern`, a `@length` or a `@range`, a
//! value outside an enumerated type's value set, or a `@required` member absent — is
//! answered HTTP 400 with the service's OWN modelled `ValidationException`: the wire
//! code `ValidationException` and a body carrying that shape's members, the
//! `@required` `message`, the `@required` `reason` drawn from
//! `ValidationExceptionReason` (`fieldValidationFailed` for a member that did not
//! validate), and the `fieldList` naming each member that failed — never the
//! framework's pre-dispatch envelope, which serializes `message` ALONE and so carries
//! no `reason` for a client to branch on.
//!
//! Source for the member set and the enum: the service's own model,
//! `smithy_query(query="shape",
//! shape="ValidationException,ValidationExceptionReason,ValidationExceptionField")` —
//! `message` (`@required`), `reason` (`@required`, `ValidationExceptionReason`),
//! `fieldList`; and `smithy_query(query="digest")`, which reports
//! `ValidationException` among the errors bound to EVERY operation.
//!
//! The tables span the class rather than the one recorded request: EVERY operation the
//! model declares (so an operation that answers in some other envelope is caught), and
//! separately every constraint class crossed with where the member sits — a URI label,
//! a query parameter, a body member, a member nested inside a union.
//!
//! The complements keep the refusal honest: a request that breaks nothing is served,
//! the published bounds of a `@range` are accepted, and the one refusal this service
//! does NOT state as a `ValidationException` — a tag map over `TagMap`'s modelled
//! maximum, which Aurora DSQL answers as a bad request BODY — still answers that way.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_dsql_dsql::ops::types::VALIDATION_EXCEPTION_REASON_VALUES;

use crate::harness::{absent_identifier, expected_arn, percent_encode_label, Answer, Dsql};

/// The `ValidationExceptionReason` a member that did not validate carries. Source:
/// `smithy_query(query="shape", shape="ValidationExceptionReason")` — one of
/// `unknownOperation`, `cannotParse`, `fieldValidationFailed`,
/// `deletionProtectionEnabled`, `other`.
const REASON_FIELD_VALIDATION_FAILED: &str = "fieldValidationFailed";

/// The members an error body may carry: the modelled `ValidationException`'s three,
/// plus the restJson1 envelope's own `__type` discriminator and the `Message`
/// spelling the envelope is allowed to use for `message`. Anything else means the
/// serialized shape is not the modelled one.
const MODELED_BODY_MEMBERS: &[&str] = &["__type", "message", "Message", "reason", "fieldList"];

/// Every operation this service's model declares. Source:
/// `smithy_query(query="digest")` for `com.amazonaws.dsql#DSQL`, which reports these
/// 16 operations. The per-operation table below has to cover all of them: the boundary
/// refusal is the same refusal for every one, so an operation left out of this
/// service's refusal layer would answer it in the framework's `message`-only envelope.
const MODELED_OPERATIONS: &[&str] = &[
    "CreateCluster",
    "CreateStream",
    "DeleteCluster",
    "DeleteClusterPolicy",
    "DeleteStream",
    "GetCluster",
    "GetClusterPolicy",
    "GetStream",
    "GetVpcEndpointServiceName",
    "ListClusters",
    "ListStreams",
    "ListTagsForResource",
    "PutClusterPolicy",
    "TagResource",
    "UntagResource",
    "UpdateCluster",
];

/// A cluster identifier outside `ClusterId`'s `^[a-z0-9]{26}$`
/// (`smithy_query(query="shape", shape="ClusterId")`), used wherever a case needs a
/// URI label the boundary refuses.
const MALFORMED_IDENTIFIER: &str = "NOT-A-CLUSTER-ID";

/// A value outside `Arn`'s `^arn:.+$` (`smithy_query(query="shape", shape="Arn")`),
/// used wherever a case needs a `resourceArn` the boundary refuses.
const MALFORMED_ARN: &str = "not-an-arn-at-all";

/// What a refusal must name about the member that failed.
#[derive(Debug, Clone, Copy)]
enum Names {
    /// The refusal is about this member's value, so `fieldList` must name it.
    Member(&'static str),
    /// The refusal is about one member whose path this table does not fix — a member
    /// nested inside a union, or whichever of several absent members the codec reports
    /// first — so `fieldList` must name SOME member.
    SomeMember,
    /// The value could not be read into its modelled type at all: nothing was parsed,
    /// so there is no member whose constraint failed — only a reason to report.
    NoMember,
}

/// One refused request: the operation it was addressed to, how the case is named, what
/// came back, and what the refusal must name.
struct Refusal {
    operation: &'static str,
    case: String,
    answer: Answer,
    names: Names,
}

/// A `targetDefinition` this account may pass: a Kinesis stream and a role of this
/// emulator's own account, so a `CreateStream` case is refused for the member under
/// test and not for the pass-role decision that precedes it.
fn passable_target() -> Value {
    let account = engine::EMULATOR_ACCOUNT_ID;
    json!({"kinesis": {
        "streamArn": format!("arn:aws:kinesis:us-east-1:{account}:stream/changes"),
        "roleArn": format!("arn:aws:iam::{account}:role/dsql-cdc"),
    }})
}

/// A `CreateStream` body with every `@required` member filled.
fn stream_request() -> Value {
    json!({
        "format": "JSON",
        "ordering": "UNORDERED",
        "targetDefinition": passable_target(),
    })
}

/// `DELETE /tags/{resourceArn}?tagKeys=env` — `UntagResource` is told from the other
/// operations bound to that path by its `@required` `@httpQuery` `tagKeys`.
async fn untag(dsql: &Dsql, arn: &str) -> Answer {
    dsql.call(
        "DELETE",
        &format!("/tags/{}?tagKeys=env", percent_encode_label(arn)),
        None,
    )
    .await
}

/// One boundary refusal per operation the model declares, each breaking a constraint
/// the model puts on one of that operation's own members.
async fn one_refusal_per_operation(dsql: &Dsql) -> Vec<Refusal> {
    let absent = absent_identifier();
    let mut refusals = Vec::new();
    let mut push = |operation: &'static str, case: &str, answer: Answer, names: Names| {
        refusals.push(Refusal {
            operation,
            case: format!("{operation} {case}"),
            answer,
            names,
        });
    };

    push(
        "CreateCluster",
        "clientToken outside ClientToken's pattern",
        dsql.create(json!({"deletionProtectionEnabled": false, "clientToken": "has a space"}))
            .await,
        Names::Member("clientToken"),
    );
    push(
        "CreateStream",
        "clusterIdentifier outside ClusterId's pattern",
        dsql.create_stream(MALFORMED_IDENTIFIER, stream_request())
            .await,
        Names::Member("clusterIdentifier"),
    );
    push(
        "DeleteCluster",
        "identifier outside ClusterId's pattern",
        dsql.delete(MALFORMED_IDENTIFIER).await,
        Names::Member("identifier"),
    );
    push(
        "DeleteClusterPolicy",
        "identifier outside ClusterId's pattern",
        dsql.delete_policy(MALFORMED_IDENTIFIER, "").await,
        Names::Member("identifier"),
    );
    push(
        "DeleteStream",
        "clusterIdentifier outside ClusterId's pattern",
        dsql.call(
            "DELETE",
            &format!("/stream/{MALFORMED_IDENTIFIER}/{absent}"),
            None,
        )
        .await,
        Names::Member("clusterIdentifier"),
    );
    push(
        "GetCluster",
        "identifier outside ClusterId's pattern",
        dsql.get(MALFORMED_IDENTIFIER).await,
        Names::Member("identifier"),
    );
    push(
        "GetClusterPolicy",
        "identifier outside ClusterId's pattern",
        dsql.get_policy(MALFORMED_IDENTIFIER).await,
        Names::Member("identifier"),
    );
    push(
        "GetStream",
        "clusterIdentifier outside ClusterId's pattern",
        dsql.get_stream(MALFORMED_IDENTIFIER, &absent).await,
        Names::Member("clusterIdentifier"),
    );
    push(
        "GetVpcEndpointServiceName",
        "identifier outside ClusterId's pattern",
        dsql.call(
            "GET",
            &format!("/clusters/{MALFORMED_IDENTIFIER}/vpc-endpoint-service-name"),
            None,
        )
        .await,
        Names::Member("identifier"),
    );
    push(
        "ListClusters",
        "maxResults below its range",
        dsql.list("?max-results=0").await,
        Names::Member("maxResults"),
    );
    push(
        "ListStreams",
        "maxResults above its range",
        dsql.list_streams(&absent, "?max-results=101").await,
        Names::Member("maxResults"),
    );
    push(
        "ListTagsForResource",
        "resourceArn outside Arn's pattern",
        dsql.list_tags_for_resource(MALFORMED_ARN).await,
        Names::Member("resourceArn"),
    );
    push(
        "PutClusterPolicy",
        "policy under its length minimum",
        dsql.put_policy(&absent, json!({"policy": ""})).await,
        Names::Member("policy"),
    );
    push(
        "TagResource",
        "resourceArn outside Arn's pattern",
        dsql.tag_resource(MALFORMED_ARN, json!({"tags": {"env": "dev"}}))
            .await,
        Names::Member("resourceArn"),
    );
    push(
        "UntagResource",
        "resourceArn outside Arn's pattern",
        untag(dsql, MALFORMED_ARN).await,
        Names::Member("resourceArn"),
    );
    push(
        "UpdateCluster",
        "identifier outside ClusterId's pattern",
        dsql.update(MALFORMED_IDENTIFIER, json!({})).await,
        Names::Member("identifier"),
    );

    refusals
}

/// Every constraint class the model declares, crossed with where the refused member
/// sits: a URI label, a query parameter, a body member, a member nested in a union —
/// plus the two layers that are not a declared bound at all, an absent `@required`
/// member and a value no modelled type can be read from.
async fn one_refusal_per_constraint_class(dsql: &Dsql) -> Vec<Refusal> {
    let absent = absent_identifier();
    let cluster_arn = expected_arn(dsql.region(), &absent);
    let mut refusals = Vec::new();
    let mut push = |operation: &'static str, case: &str, answer: Answer, names: Names| {
        refusals.push(Refusal {
            operation,
            case: format!("{operation} {case}"),
            answer,
            names,
        });
    };

    // `@pattern`, on a URI label and on a body member.
    push(
        "GetCluster",
        "a label with punctuation",
        dsql.get("not-a-valid-identifier!!").await,
        Names::Member("identifier"),
    );
    push(
        "CreateCluster",
        "a body member outside its pattern",
        dsql.create(json!({"clientToken": "tab\tseparated"})).await,
        Names::Member("clientToken"),
    );

    // `@range`, on a query parameter: both bounds and far outside.
    for (case, query) in [
        ("maxResults at zero", "?max-results=0"),
        ("maxResults negative", "?max-results=-5"),
        ("maxResults one past its maximum", "?max-results=101"),
        ("maxResults far past its maximum", "?max-results=100000"),
    ] {
        push(
            "ListClusters",
            case,
            dsql.list(query).await,
            Names::Member("maxResults"),
        );
    }

    // `@length`, under the minimum of a body member.
    push(
        "PutClusterPolicy",
        "an empty policy document",
        dsql.put_policy(&absent, json!({"policy": ""})).await,
        Names::Member("policy"),
    );

    // An enumerated type's value set, on each enumerated member of one operation.
    for (member, outside) in [("ordering", "SEQUENTIAL"), ("format", "CSV")] {
        let mut body = stream_request();
        body[member] = Value::String(outside.to_string());
        push(
            "CreateStream",
            &format!("{member}={outside}, outside its value set"),
            dsql.create_stream(&absent, body).await,
            Names::Member(member),
        );
    }

    // A member nested inside the target union: the refusal is about one member, whose
    // path from the request root this table does not spell.
    let mut nested = stream_request();
    nested["targetDefinition"]["kinesis"]["streamArn"] = json!("not-a-kinesis-arn");
    push(
        "CreateStream",
        "a streamArn nested in the target union",
        dsql.create_stream(&absent, nested).await,
        Names::SomeMember,
    );

    // A `@required` member absent — each of the three `CreateStream` body members
    // omitted on its own, so the one the codec reports is the one under test.
    for member in ["format", "ordering", "targetDefinition"] {
        let mut body = stream_request();
        body.as_object_mut()
            .expect("the stream request is an object")
            .remove(member);
        push(
            "CreateStream",
            &format!("the @required {member} absent"),
            dsql.create_stream(&absent, body).await,
            Names::Member(member),
        );
    }
    push(
        "TagResource",
        "the @required tags absent",
        dsql.tag_resource(&cluster_arn, json!({})).await,
        Names::Member("tags"),
    );
    push(
        "PutClusterPolicy",
        "the @required policy absent",
        dsql.put_policy(&absent, json!({})).await,
        Names::Member("policy"),
    );

    // A value the member's modelled TYPE cannot be read from: nothing is parsed, so
    // the refusal names no member — but it still carries a reason.
    for (case, query) in [
        ("maxResults that is not a number", "?max-results=not-a-number"),
        ("maxResults that is empty", "?max-results="),
    ] {
        push(
            "ListClusters",
            case,
            dsql.list(query).await,
            Names::NoMember,
        );
    }
    push(
        "CreateCluster",
        "deletionProtectionEnabled that is not a boolean",
        dsql.create(json!({"deletionProtectionEnabled": "maybe"}))
            .await,
        Names::NoMember,
    );

    refusals
}

/// Assert one refusal against the rule: 400, the `ValidationException` wire code, and
/// exactly the modelled shape's members with `reason` filled — and, where the refusal
/// is about a member, `fieldValidationFailed` with that member in `fieldList`.
fn assert_modeled_validation_exception(case: &str, answer: &Answer, names: Names) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: a boundary refusal is HTTP 400, got {} {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error.as_deref(),
        Some("ValidationException"),
        "case {case:?}: the wire error code must be ValidationException, got {:?}",
        answer.error
    );
    assert!(
        !answer.message().is_empty(),
        "case {case:?}: the modelled @required message must be filled, got {:?}",
        answer.body
    );

    // The discriminator between the modelled shape and the framework's own: the
    // framework envelope serializes `message` and nothing else, so a body without
    // `reason` — or one carrying a member the modelled shape does not declare — is
    // not the modelled shape.
    let reason = answer.str("reason");
    assert!(
        VALIDATION_EXCEPTION_REASON_VALUES.contains(&reason.as_str()),
        "case {case:?}: the modelled @required reason must be one of \
         {VALIDATION_EXCEPTION_REASON_VALUES:?}, got {reason:?} in {:?}",
        answer.body
    );
    let members: Vec<String> = answer
        .body
        .as_object()
        .map(|object| object.keys().cloned().collect())
        .unwrap_or_default();
    let unmodeled: Vec<&String> = members
        .iter()
        .filter(|member| !MODELED_BODY_MEMBERS.contains(&member.as_str()))
        .collect();
    assert!(
        unmodeled.is_empty(),
        "case {case:?}: the body may carry only the modelled ValidationException \
         members {MODELED_BODY_MEMBERS:?}, got the extra {unmodeled:?} in {:?}",
        answer.body
    );

    let expected_member = match names {
        Names::NoMember => return,
        Names::SomeMember => None,
        Names::Member(member) => Some(member),
    };
    assert_eq!(
        reason, REASON_FIELD_VALIDATION_FAILED,
        "case {case:?}: a member that did not validate reports \
         {REASON_FIELD_VALIDATION_FAILED}, got {reason:?} in {:?}",
        answer.body
    );
    let field_list = answer.get("fieldList");
    let fields = field_list.as_array().map(Vec::as_slice).unwrap_or_default();
    assert!(
        fields.iter().any(|field| {
            let name = field.get("name").and_then(Value::as_str);
            let names_the_member = match expected_member {
                // The path is spelled from the request root, so a nested member's
                // entry ENDS with the member's own name.
                Some(member) => name.is_some_and(|name| {
                    name == member || name.ends_with(&format!(".{member}"))
                }),
                None => name.is_some_and(|name| !name.is_empty()),
            };
            names_the_member
                && field
                    .get("message")
                    .and_then(Value::as_str)
                    .is_some_and(|message| !message.is_empty())
        }),
        "case {case:?}: fieldList must name {} with a message saying why, got {field_list:?}",
        expected_member.unwrap_or("the member that failed")
    );
}

/// RULE: every operation the model declares answers a boundary refusal with the
/// service's own modelled `ValidationException` body — so no operation is left
/// answering the framework's `message`-only envelope.
#[tokio::test]
async fn every_operation_refuses_with_the_modeled_validation_exception_body() {
    let dsql = Dsql::new();
    let refusals = one_refusal_per_operation(&dsql).await;

    let covered: BTreeSet<&str> = refusals.iter().map(|refusal| refusal.operation).collect();
    let declared: BTreeSet<&str> = MODELED_OPERATIONS.iter().copied().collect();
    assert_eq!(
        covered, declared,
        "the table must refuse one request per modelled operation, missing {:?}",
        declared.difference(&covered).collect::<Vec<_>>()
    );

    for Refusal {
        case, answer, names, ..
    } in refusals
    {
        assert_modeled_validation_exception(&case, &answer, names);
    }
}

/// RULE: the same body answers every constraint class the model declares, wherever
/// the refused member sits — a URI label, a query parameter, a body member, a member
/// nested in a union — and an absent `@required` member is reported the same way.
#[tokio::test]
async fn every_constraint_class_refuses_with_the_modeled_validation_exception_body() {
    let dsql = Dsql::new();
    let refusals = one_refusal_per_constraint_class(&dsql).await;
    assert!(
        refusals.len() > 15,
        "the table must span the constraint classes, got {} cases",
        refusals.len()
    );

    for Refusal {
        case, answer, names, ..
    } in refusals
    {
        assert_modeled_validation_exception(&case, &answer, names);
    }
}

/// RULE (the complement that keeps the refusal honest): a request that breaks no
/// modelled constraint is served, and the published bounds of a `@range` are inside
/// it — so the refusal above is about the constraint and not about the request shape.
#[tokio::test]
async fn a_request_inside_every_constraint_is_served() {
    let dsql = Dsql::new();
    let cluster = dsql.create_settled().await;

    let served = dsql.get(&cluster).await;
    assert_eq!(
        served.status, 200,
        "case GetCluster on a created cluster: must be served, got {} {:?}",
        served.status, served.body
    );
    assert_eq!(
        served.str("identifier"),
        cluster,
        "case GetCluster on a created cluster: must report the cluster read back, got {:?}",
        served.body
    );

    // The published bounds of `maxResults`' `@range`, and one inside it.
    for accepted in [1, 50, 100] {
        let answer = dsql.list(&format!("?max-results={accepted}")).await;
        assert_eq!(
            answer.status, 200,
            "case maxResults={accepted}: a value inside the range must be served, got {} {:?}",
            answer.status, answer.body
        );
    }

    // A `clientToken` inside `ClientToken`'s pattern, and a policy at its length
    // minimum: the members the table above refuses, filled legally.
    let created = dsql
        .create(json!({"deletionProtectionEnabled": false, "clientToken": "a-legal-token"}))
        .await;
    assert_eq!(
        created.status, 200,
        "case CreateCluster with a legal clientToken: must be served, got {} {:?}",
        created.status, created.body
    );
    let policy = dsql
        .put_policy(&cluster, json!({"policy": "{\"Version\":\"2012-10-17\",\"Statement\":[]}"}))
        .await;
    assert_eq!(
        policy.status, 200,
        "case PutClusterPolicy with a policy document: must be served, got {} {:?}",
        policy.status, policy.body
    );
}

/// RULE: the one refusal this service does NOT state as a `ValidationException` is
/// unchanged — a tag map carrying more entries than `TagMap` admits is a bad request
/// BODY, so it keeps its own code and sentence and carries no `reason`.
#[tokio::test]
async fn a_tag_map_over_its_maximum_is_still_a_bad_request_body() {
    let dsql = Dsql::new();
    let cluster = dsql.create_settled().await;
    let cluster_arn = expected_arn(dsql.region(), &cluster);

    // One entry past `TagMap`'s modelled maximum (`smithy_query(query="shape",
    // shape="TagMap")`: `@length(0, 200)`), as `crate::harness::TAG_MAP_MAX` spells it.
    let mut tags = serde_json::Map::new();
    for n in 0..=crate::harness::TAG_MAP_MAX {
        tags.insert(format!("key{n:04}"), Value::String(format!("value{n:04}")));
    }
    let tags = Value::Object(tags);

    let answers = [
        (
            "CreateCluster",
            dsql.create(json!({"deletionProtectionEnabled": false, "tags": tags.clone()}))
                .await,
        ),
        (
            "TagResource",
            dsql.tag_resource(&cluster_arn, json!({"tags": tags})).await,
        ),
    ];
    for (operation, answer) in answers {
        assert_eq!(
            answer.status, 400,
            "case {operation} with an oversized tag map: must answer 400, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error.as_deref(),
            Some("BadRequestException"),
            "case {operation} with an oversized tag map: the refusal must be a \
             BadRequestException, got {:?}",
            answer.error
        );
        assert_eq!(
            answer.message(),
            "Invalid request body",
            "case {operation} with an oversized tag map: got {:?}",
            answer.message()
        );
        assert_eq!(
            answer.get("reason"),
            Value::Null,
            "case {operation} with an oversized tag map: a bad request body is not the \
             modelled ValidationException, got {:?}",
            answer.body
        );
    }
}
