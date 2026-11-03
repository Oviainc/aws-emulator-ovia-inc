// SPDX-License-Identifier: CC0-1.0

//! Which `@pattern` expression a refusal NAMES, and what it still ENFORCES, end to end over
//! the wire (the same `MultiServiceRouter` the emulator boots, through the harness in
//! `canary_lifecycle`).
//!
//! Smithy's `@pattern` is a FIND — the expression has to match somewhere in the value — so a
//! model that wants the whole value to match writes `^` and `$` around the expression it
//! actually requires. Two properties follow, and this suite pins both over the same rows,
//! because a change that satisfied one by breaking the other would be a worse divergence
//! than the one it fixed:
//!
//! 1. **The requirement names the expression its OPERATION's dialect states.** The recorded
//!    service words this clause two ways, and the model says which way an operation takes:
//!    an operation that declares `ValidationException` in its own error set states the
//!    expression as the model declares it, pair included; one that does not — the three
//!    tagging operations, which declare the `BadRequestException` family and are still
//!    recorded answering `ValidationException` for a constraint failure, i.e. from the
//!    generic request validator ahead of them — states the expression that validator
//!    publishes, with the pair dropped. The dialect is the operation's: the rows below
//!    reach the same ARN form, through the same kind of URI-label binding, from both
//!    families, and each family states it its own way. (No pattern-constrained SHAPE is
//!    reached from both families in this model; that the dialect is read off the error set
//!    rather than off a shape or a member is pinned by `src/service.rs`'s own test.)
//! 2. **The total match is still what is enforced, in both dialects.** A value that
//!    satisfies the declared expression only as a find — the expression matches a part of it
//!    — is still refused, even where that is exactly the expression the requirement quoted.
//!    So the set of values this service accepts is the model's, whichever way the refusal is
//!    worded.
//!
//! Every row names the SHAPE whose `@pattern` it exercises and reads that expression out of
//! the model this crate ships (`canary_lifecycle::declared_pattern`), so nothing here
//! restates a rule the model does not carry, and the published form comes from
//! `service_common::validation::unanchored` — the tree's one rendering of that drop — rather
//! than from a strip written again here. The shapes are the pattern-constrained ones
//! reachable from this service's operations, read with
//! `smithy_query(query="shape",shape="ResourceArn,CanaryArn,CanaryName,RoleArn,KmsKeyArn,UUID",model_path="services/service-com-amazonaws-synthetics-synthetics/smithy-model/com-amazonaws-synthetics-synthetics.json")`,
//! and each operation's error set with
//! `smithy_query(query="digest",model_path="…")`. The rows differ along what the rule
//! generalises over: the dialect, the shape and its expression, the operation, and the
//! member's binding (URI label, query string, request body).

use serde_json::json;

use crate::canary_lifecycle::{
    create_body, declared_pattern, one_violation, pattern_requirement, Answer, Synthetics,
};

/// The Region every request below is signed for.
const REGION: &str = "us-east-1";

/// A twelve-digit account id, the form every ARN pattern in this model requires
/// (`…:\d{12}:…`). Which account it is does not matter to a constraint check.
const ACCOUNT: &str = "123456789012";

/// Characters outside the alphabet of every expression below, prepended to a value that
/// matches in full: the result breaks the total match while leaving the expression itself
/// matching a part of the value.
const PREFIX: &str = "ZZ";

/// The same, appended rather than prepended — the other end the `$` anchors.
const SUFFIX: &str = "!!";

/// Which dialect the operation a row calls states a `@pattern` requirement in.
#[derive(Debug, Clone, Copy)]
enum Dialect {
    /// The operation declares `ValidationException` itself: the expression as the model
    /// declares it.
    AsDeclared,
    /// The operation declares no `ValidationException`: the expression the generic request
    /// validator publishes.
    AsPublished,
}

/// Where a row puts its value, which is also the operation that refuses it.
#[derive(Debug, Clone, Copy)]
enum Binding {
    /// `POST /tags/{ResourceArn}` — the value is a URI label.
    TagResourceArn,
    /// `DELETE /tags/{ResourceArn}?tagKeys=…` — the value is a URI label.
    UntagResourceArn,
    /// `POST /resource/{ResourceArn}/groups` — the value is a URI label on an operation of
    /// the OTHER family, bound the same way as the two above.
    ListAssociatedGroupsArn,
    /// `PATCH /group/{GroupIdentifier}/associate` — the value is a body member.
    AssociateResourceArn,
    /// `GET /canary/{Name}` — the value is a URI label.
    GetCanaryName,
    /// `GET /canary/{Name}?dryRunId=…` — the value is a query-string parameter.
    GetCanaryDryRunId,
    /// `POST /canary` — the value is the named member of an otherwise valid body.
    CreateCanaryMember(&'static str),
}

/// One pattern-constrained member: where it is bound, which dialect its operation states,
/// how the message names it, and a value that satisfies its declared expression in full.
#[derive(Debug)]
struct Row {
    case: &'static str,
    /// The shape whose `@pattern` this row exercises; its expression is read from the
    /// shipped model rather than written out here.
    shape: &'static str,
    /// The member path the constraint envelope states, which is the wire spelling.
    member_path: &'static str,
    binding: Binding,
    dialect: Dialect,
    /// A value the declared expression matches in FULL.
    matching: String,
}

/// Send `value` through `binding`. `canary` names the canary a `CreateCanary` row creates,
/// so two requests from one row never collide on a name.
async fn send(aws: &Synthetics, binding: Binding, value: &str, canary: &str) -> Answer {
    match binding {
        Binding::TagResourceArn => {
            aws.call(
                "POST",
                &format!("/tags/{value}"),
                Some(json!({ "Tags": { "team": "synthetics" } })),
            )
            .await
        }
        Binding::UntagResourceArn => {
            aws.call("DELETE", &format!("/tags/{value}?tagKeys=env"), None)
                .await
        }
        Binding::ListAssociatedGroupsArn => {
            aws.call("POST", &format!("/resource/{value}/groups"), None)
                .await
        }
        Binding::AssociateResourceArn => {
            aws.call(
                "PATCH",
                "/group/some-group/associate",
                Some(json!({ "ResourceArn": value })),
            )
            .await
        }
        Binding::GetCanaryName => aws.call("GET", &format!("/canary/{value}"), None).await,
        Binding::GetCanaryDryRunId => {
            aws.call("GET", &format!("/canary/probe-canary?dryRunId={value}"), None)
                .await
        }
        Binding::CreateCanaryMember(member) => {
            let mut body = create_body(canary);
            body[member] = json!(value);
            aws.create(body).await
        }
    }
}

fn rows() -> Vec<Row> {
    let canary_arn = format!("arn:aws:synthetics:{REGION}:{ACCOUNT}:canary:probe-canary");
    vec![
        // The recorded instance: a URI-label ARN on the tag write path, in the dialect of
        // the family that declares no `ValidationException`.
        Row {
            case: "tag-resource-arn",
            shape: "ResourceArn",
            member_path: "resourceArn",
            binding: Binding::TagResourceArn,
            dialect: Dialect::AsPublished,
            matching: canary_arn.clone(),
        },
        // Another operation of the same family, reached with a different method.
        Row {
            case: "untag-resource-arn",
            shape: "ResourceArn",
            member_path: "resourceArn",
            binding: Binding::UntagResourceArn,
            dialect: Dialect::AsPublished,
            matching: canary_arn.clone(),
        },
        // A URI label bound the same way, on an operation of the OTHER family: same
        // binding, same ARN form, and the dialect still differs — which is what shows it
        // belongs to the operation rather than to the binding. (No pattern-constrained
        // SHAPE is reached from both families: that the dialect is read off the error set
        // and not off the shape is pinned in `src/service.rs`'s own test.) The member here
        // targets `CanaryArn`, the canary-only ARN shape.
        Row {
            case: "list-associated-groups-arn",
            shape: "CanaryArn",
            member_path: "resourceArn",
            binding: Binding::ListAssociatedGroupsArn,
            dialect: Dialect::AsDeclared,
            matching: canary_arn.clone(),
        },
        // The canary-only ARN shape, as a BODY member.
        Row {
            case: "associate-resource-arn",
            shape: "CanaryArn",
            member_path: "resourceArn",
            binding: Binding::AssociateResourceArn,
            dialect: Dialect::AsDeclared,
            matching: canary_arn,
        },
        // A URI label on a read operation, whose expression is a bare character class.
        Row {
            case: "get-canary-name",
            shape: "CanaryName",
            member_path: "name",
            binding: Binding::GetCanaryName,
            dialect: Dialect::AsDeclared,
            matching: "probe-canary".to_string(),
        },
        // A query-string parameter, whose expression is a fixed-width form.
        Row {
            case: "get-canary-dry-run-id",
            shape: "UUID",
            member_path: "dryRunId",
            binding: Binding::GetCanaryDryRunId,
            dialect: Dialect::AsDeclared,
            matching: "0123abcd-4567-89ab-cdef-0123456789ab".to_string(),
        },
        // A body member whose expression carries an alternation and a slash.
        Row {
            case: "create-canary-execution-role-arn",
            shape: "RoleArn",
            member_path: "executionRoleArn",
            binding: Binding::CreateCanaryMember("ExecutionRoleArn"),
            dialect: Dialect::AsDeclared,
            matching: format!("arn:aws:iam::{ACCOUNT}:role/CanaryRole"),
        },
        // A body member whose expression carries escapes (`\w`, `\-`, `\/`).
        Row {
            case: "create-canary-kms-key-arn",
            shape: "KmsKeyArn",
            member_path: "kmsKeyArn",
            binding: Binding::CreateCanaryMember("KmsKeyArn"),
            dialect: Dialect::AsDeclared,
            matching: format!(
                "arn:aws:kms:{REGION}:{ACCOUNT}:key/0123abcd-4567-89ab-cdef-0123456789ab"
            ),
        },
    ]
}

/// Rule 1 over every row: a refused value names the expression its operation's dialect
/// states. Rule 2 over the same rows: the value that breaks only the whole-value pair is
/// among the refused ones either way, and the value that matches in full is not refused at
/// all.
#[tokio::test]
async fn a_pattern_requirement_follows_its_operations_dialect_and_still_enforces_the_total_match() {
    let aws = Synthetics::new(REGION);

    for row in rows() {
        let declared = declared_pattern(row.shape);
        // The row proves something only if the model's own expression CARRIES the
        // whole-value pair: that pair is the only difference between the two dialects, and
        // it is what the prefixed and suffixed values below break.
        assert!(
            declared.starts_with('^') && declared.ends_with('$'),
            "case {}: the modelled pattern {declared:?} is not anchored, so this row \
             asserts nothing",
            row.case
        );
        let stated = match row.dialect {
            Dialect::AsDeclared => declared.clone(),
            Dialect::AsPublished => service_common::validation::unanchored(&declared),
        };

        // The value that matches in full: accepted by the constraint walk, so whatever the
        // operation answers, it is not a refusal of the request as invalid.
        let answer = send(
            &aws,
            row.binding,
            &row.matching,
            &format!("{}-matching", row.case),
        )
        .await;
        assert_ne!(
            answer.error_code.as_deref(),
            Some("ValidationException"),
            "case {}/matches-in-full: expected {:?} to satisfy {declared:?}, got {} ({:?})",
            row.case,
            row.matching,
            answer.status,
            answer.body
        );

        // Values the declared expression does NOT match in full. The first two match it as
        // a find — for an `AsPublished` row they are exactly the values that would be
        // accepted if the quoted expression were the one enforced — and the third matches it
        // nowhere.
        let refused = [
            ("prefixed", format!("{PREFIX}{}", row.matching)),
            ("suffixed", format!("{}{SUFFIX}", row.matching)),
            ("unrelated", "zz.not.a.value".to_string()),
        ];
        for (variant, value) in refused {
            let case = format!("{}/{variant}", row.case);
            let answer = send(&aws, row.binding, &value, &format!("{}-{variant}", row.case)).await;
            assert_eq!(
                answer.status, 400,
                "case {case}: expected 400 for {value:?}, got {} ({:?})",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code.as_deref(),
                Some("ValidationException"),
                "case {case}: expected x-amzn-errortype: ValidationException, got {:?}",
                answer.error_code
            );
            let expected = one_violation(&value, row.member_path, &pattern_requirement(&stated));
            assert_eq!(
                answer.at("Message").as_str(),
                Some(expected.as_str()),
                "case {case}: expected Message {expected:?}, got {:?}",
                answer.body
            );
        }
    }
}
