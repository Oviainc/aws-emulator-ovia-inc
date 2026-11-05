// SPDX-License-Identifier: CC0-1.0

//! The BODY every operation on the tagging path answers a `resourceArn` that names
//! no Aurora DSQL resource with.
//!
//! RULE under test: a `resourceArn` that names no resource of this service AT ALL is
//! one logical failure, so every operation bound to `/tags/{resourceArn}`
//! (`TagResource`, `ListTagsForResource`, `UntagResource`) answers it with the SAME
//! modelled body — HTTP 400, wire code `ValidationException`, a non-empty `message`,
//! the `@required` `reason` `fieldValidationFailed` and a `fieldList` naming
//! `resourceArn` — and with no member the modelled shape does not declare. The
//! synthetic `smithy.framework#ValidationException`, whose only member is `message`,
//! is never that body: a client branching on `reason` would find nothing to branch
//! on, and two operations refusing the same value would disagree about the failure.
//!
//! Source for the shape: `smithy_query(query="shape",
//! shape="ValidationException,ValidationExceptionReason,ValidationExceptionField")` —
//! `message` (`@required`), `reason` (`@required`, one of `unknownOperation`,
//! `cannotParse`, `fieldValidationFailed`, `deletionProtectionEnabled`, `other`) and
//! `fieldList`, each field naming a member and why it failed. `ValidationException`
//! is declared for all three operations (`smithy_query(query="operation",
//! operation="TagResource,UntagResource,ListTagsForResource")`), at HTTP 400.
//!
//! The table spans the class the rule generalises over: the OPERATION that answers
//! (all three bound to the path, across the three HTTP methods), and the FORM an ARN
//! takes that names nothing here — another service's resource, another partition,
//! the service field cased differently, every field empty, a resource field that is
//! empty or carries no type or no id, too few fields, and an ARN at `Arn`'s published
//! maximum length. The reported case — tagging an S3 ARN — is one row.
//!
//! A value the MODEL itself constrains (an ARN failing `Arn`'s `^arn:.+$` or its
//! length bounds) never reaches a handler: that refusal is the boundary validator's,
//! a different code path asserted in `tests/all/boundary_validation_exception.rs`.
//! The control test below pins the other side of this rule: an ARN that does name a
//! resource of this service is resolved rather than refused, so the 400 is about the
//! ARN naming nothing here and not about the route.
//!
//! The client is the shared `Dsql` harness in `harness.rs`, and `UntagResource`'s
//! query encoding is the one the `resource_tags` suite already spells.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_dsql_dsql::ops::types::VALIDATION_EXCEPTION_REASON_VALUES;

use crate::harness::{absent_identifier, expected_arn, Answer, Dsql};
use crate::resource_tags::untag;

/// The `ValidationExceptionReason` a refusal about one member's value carries, from
/// the modelled enum (`smithy_query(query="shape",
/// shape="ValidationExceptionReason")`).
const REASON_FIELD_VALIDATION_FAILED: &str = "fieldValidationFailed";

/// The input member every operation on this path carries the ARN in, as the model
/// spells it on the wire.
const MEMBER: &str = "resourceArn";

/// The members a refusal body may carry: the modelled shape's three, plus the
/// restJson1 envelope's own discriminator and the alternate `message` casing the
/// protocol allows. A member outside this set would mean some other shape was
/// serialized.
const MODELED_BODY_MEMBERS: &[&str] = &["__type", "message", "Message", "reason", "fieldList"];

/// `Arn`'s published maximum length (`smithy_query(query="operation",
/// operation="TagResource")`: `resourceArn` targets `Arn`, `@length(1, 1011)`), so the
/// longest ARN a caller can send still reaches the handler rather than the boundary.
const ARN_MAX: usize = 1011;

/// ARNs that satisfy `Arn`'s modelled `^arn:.+$` and its length bounds — so they
/// reach a handler — and name no Aurora DSQL resource at all.
fn arns_naming_no_dsql_resource() -> Vec<(String, String)> {
    let account = engine::EMULATOR_ACCOUNT_ID;
    // An ARN exactly at the published maximum: still a well-formed request, and still
    // another service's resource.
    let at_max_length = {
        let prefix = "arn:aws:s3:::";
        format!("{prefix}{}", "b".repeat(ARN_MAX - prefix.len()))
    };
    let cases = vec![
        (
            "another service's resource".to_string(),
            format!("arn:aws:kinesis:us-east-1:{account}:stream/orders"),
        ),
        (
            "an S3 ARN, with its empty region and account fields".to_string(),
            "arn:aws:s3:::my-bucket/key".to_string(),
        ),
        (
            "another service's resource in another partition".to_string(),
            format!("arn:aws-cn:kinesis:cn-north-1:{account}:stream/orders"),
        ),
        (
            "the service field cased differently".to_string(),
            format!(
                "arn:aws:DSQL:us-east-1:{account}:cluster/{}",
                absent_identifier()
            ),
        ),
        ("every field empty".to_string(), "arn::::::".to_string()),
        (
            "a dsql ARN with an empty resource field".to_string(),
            format!("arn:aws:dsql:us-east-1:{account}:"),
        ),
        (
            "a dsql ARN whose resource field has no type".to_string(),
            format!("arn:aws:dsql:us-east-1:{account}:cluster"),
        ),
        (
            "a dsql ARN whose resource field has no id".to_string(),
            format!("arn:aws:dsql:us-east-1:{account}:cluster/"),
        ),
        (
            "a dsql ARN whose resource field has an empty type".to_string(),
            format!("arn:aws:dsql:us-east-1:{account}:/{}", absent_identifier()),
        ),
        (
            "too few ARN fields".to_string(),
            "arn:aws:dsql:us-east-1".to_string(),
        ),
        (
            "an ARN at Arn's published maximum length".to_string(),
            at_max_length,
        ),
    ];
    for (name, arn) in &cases {
        assert!(
            arn.starts_with("arn:") && arn.len() > 4 && arn.len() <= ARN_MAX,
            "case {name:?}: the row must satisfy Arn's own pattern and length so it \
             reaches a handler, got {} characters: {arn:?}",
            arn.len()
        );
    }
    cases
}

/// The same ARN put to every operation bound to `/tags/{resourceArn}`, each with a
/// request that is otherwise complete, so the only thing wrong with any of them is
/// the ARN.
async fn every_tagging_operation(dsql: &Dsql, arn: &str) -> Vec<(&'static str, Answer)> {
    vec![
        (
            "TagResource",
            dsql.tag_resource(arn, json!({"tags": {"env": "dev"}})).await,
        ),
        ("ListTagsForResource", dsql.list_tags_for_resource(arn).await),
        (
            "UntagResource",
            untag(dsql, arn, &["env".to_string()]).await,
        ),
    ]
}

/// The members a body carries, so a body can be compared against the modelled set.
fn body_members(answer: &Answer) -> BTreeSet<String> {
    answer
        .body
        .as_object()
        .map(|members| members.keys().cloned().collect())
        .unwrap_or_default()
}

/// The member names a refusal's `fieldList` names.
fn field_names(answer: &Answer) -> BTreeSet<String> {
    answer
        .get("fieldList")
        .as_array()
        .map(|fields| {
            fields
                .iter()
                .filter_map(|field| field.get("name").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Assert one answer IS the service's own modelled `ValidationException` about
/// [`MEMBER`]: the status and code the model declares, and every modelled member a
/// client reads — not the synthetic framework shape, which carries `message` alone.
fn assert_modeled_arn_refusal(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: an ARN naming no resource of this service is a 400, got {} {:?}",
        answer.status, answer.body
    );
    assert!(
        answer
            .error
            .as_deref()
            .is_some_and(|code| code.contains("ValidationException")),
        "case {case:?}: the wire code must be ValidationException, got {:?}",
        answer.error
    );
    assert!(
        !answer.message().is_empty(),
        "case {case:?}: the modelled @required message must be filled, got {:?}",
        answer.body
    );

    let reason = answer.str("reason");
    assert!(
        VALIDATION_EXCEPTION_REASON_VALUES.contains(&reason.as_str()),
        "case {case:?}: the modelled @required reason must be one of \
         {VALIDATION_EXCEPTION_REASON_VALUES:?}, got {reason:?} in {:?}",
        answer.body
    );
    assert_eq!(
        reason, REASON_FIELD_VALIDATION_FAILED,
        "case {case:?}: a refusal about one member's value reports \
         {REASON_FIELD_VALIDATION_FAILED}, got {reason:?} in {:?}",
        answer.body
    );

    let fields = answer.get("fieldList");
    let named = fields.as_array().map(Vec::as_slice).unwrap_or_default();
    assert!(
        named.iter().any(|field| {
            field.get("name").and_then(Value::as_str) == Some(MEMBER)
                && field
                    .get("message")
                    .and_then(Value::as_str)
                    .is_some_and(|message| !message.is_empty())
        }),
        "case {case:?}: fieldList must name {MEMBER} with a message saying why, got {fields:?}"
    );

    let unexpected: Vec<String> = body_members(answer)
        .into_iter()
        .filter(|member| !MODELED_BODY_MEMBERS.contains(&member.as_str()))
        .collect();
    assert!(
        unexpected.is_empty(),
        "case {case:?}: the body must carry only the modelled shape's members \
         {MODELED_BODY_MEMBERS:?}, got the extra {unexpected:?} in {:?}",
        answer.body
    );
}

/// RULE: every operation bound to `/tags/{resourceArn}` refuses an ARN that names no
/// Aurora DSQL resource with the service's own modelled `ValidationException` — and
/// the three answer one such ARN identically, rather than one of them answering the
/// synthetic framework shape that carries `message` alone.
#[tokio::test]
async fn every_tagging_operation_refuses_an_arn_naming_nothing_with_one_modeled_body() {
    let dsql = Dsql::new();
    let rows = arns_naming_no_dsql_resource();
    assert!(
        rows.len() >= 8,
        "the table must span the forms such an ARN takes, got {} rows",
        rows.len()
    );

    for (name, arn) in rows {
        let answers = every_tagging_operation(&dsql, &arn).await;
        for (operation, answer) in &answers {
            assert_modeled_arn_refusal(&format!("{operation} with {name}"), answer);
        }

        // One logical failure, so the three answers agree on everything a client
        // reads but the sentence: same status, same code, same reason, same named
        // member.
        let (first_operation, first) = &answers[0];
        for (operation, answer) in &answers[1..] {
            let case = format!("{operation} vs {first_operation} with {name}");
            assert_eq!(
                answer.status, first.status,
                "case {case:?}: the same refusal must carry the same status, got {} and {}",
                answer.status, first.status
            );
            assert_eq!(
                answer.error, first.error,
                "case {case:?}: the same refusal must carry the same wire code, got {:?} and {:?}",
                answer.error, first.error
            );
            assert_eq!(
                answer.str("reason"),
                first.str("reason"),
                "case {case:?}: the same refusal must carry the same reason, got {:?} and {:?}",
                answer.str("reason"),
                first.str("reason")
            );
            assert_eq!(
                field_names(answer),
                field_names(first),
                "case {case:?}: the same refusal must name the same members, got {:?} and {:?}",
                field_names(answer),
                field_names(first)
            );
            assert_eq!(
                body_members(answer),
                body_members(first),
                "case {case:?}: the same refusal must carry the same members, got {:?} and {:?}",
                body_members(answer),
                body_members(first)
            );
        }
    }
}

/// RULE (the other side): an ARN this service DOES read as naming one of its
/// resources is resolved rather than refused — served when the resource is there,
/// and answered with the tagging surface's own 404 when it is not — so the 400 above
/// is about the ARN naming nothing here, not about the path or the request body.
#[tokio::test]
async fn an_arn_this_service_reads_is_resolved_rather_than_refused() {
    let dsql = Dsql::new();
    let created = dsql.create(json!({"deletionProtectionEnabled": false})).await;
    assert_eq!(
        created.status, 200,
        "CreateCluster must answer 200, got {} {:?}",
        created.status, created.body
    );
    let live = created.str("arn");
    assert_eq!(
        live,
        expected_arn(dsql.region(), &created.str("identifier")),
        "the created cluster's ARN must be the one ClusterArn's pattern requires"
    );

    for (operation, answer) in every_tagging_operation(&dsql, &live).await {
        assert_eq!(
            answer.status, 200,
            "case {operation:?} with the live cluster's own ARN: got {} {:?}",
            answer.status, answer.body
        );
    }

    // Well-formed, read as naming a cluster of this service, and naming nothing the
    // store holds: the tagging surface's own 404, never the 400 above.
    let absent = expected_arn(dsql.region(), &absent_identifier());
    for (operation, answer) in every_tagging_operation(&dsql, &absent).await {
        assert_eq!(
            answer.status, 404,
            "case {operation:?} with an ARN naming no stored cluster: got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error.as_deref(),
            Some("NotFoundException"),
            "case {operation:?} with an ARN naming no stored cluster: the code must be \
             NotFoundException, got {:?}",
            answer.error
        );
    }
}
