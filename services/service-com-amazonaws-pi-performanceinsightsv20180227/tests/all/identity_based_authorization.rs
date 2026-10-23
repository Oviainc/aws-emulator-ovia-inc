// SPDX-License-Identifier: CC0-1.0

//! Identity-based authorization on the Performance Insights surface, end to end
//! over the wire.
//!
//! RULE under test: a request signed by a credential this account minted for an
//! IAM identity is served only when that identity's own identity-based policies
//! allow the `pi:<Action>` the request performs on the resource it names; an
//! identity nothing allows it for is refused with the error code
//! `AccessDeniedException` and a message built from the request — the caller's
//! principal, the action attempted and the resource named — and the operation
//! performs nothing. A credential belonging to no IAM identity is the ambient
//! account identity, which no identity-based policy constrains.
//!
//! Each test is a TABLE whose rows differ along what the rule generalises over:
//! the kind of identity the signing key belongs to (an IAM user's access key, an
//! assumed-role session, the ambient credential), the operation performed, the
//! form the policy's `Action` and `Resource` take (one string or a list, an
//! exact action or a wildcard, every resource or one), and the effect
//! (`Allow`, `Deny`, nothing at all).

use serde_json::{json, Value};

use service_com_amazonaws_pi_performanceinsightsv20180227::state;

use crate::pi_harness::{
    call, call_signed_by, emulator_with_identity_services, iam_call, sts_call, xml_element, REGION,
    DEFAULT_ACCESS_KEY_ID,
};

/// 2026-01-01T00:00:00Z. `awsJson1_1` carries an `ISOTimestamp` as epoch
/// seconds.
const T0: f64 = 1_767_225_600.0;

/// A monitored data source: an immutable DB resource id of the form the model
/// documents on every `Identifier` member.
const IDENTIFIER: &str = "db-ABCDEFGHIJKLMNOPQRSTU1VW2X";

/// A second monitored data source, for a policy scoped to the first.
const OTHER_IDENTIFIER: &str = "db-ZYXWVUTSRQPONMLKJIHGFE987";

/// A pattern-valid `AnalysisReportId` (`report-` + 17 lowercase hex characters).
const REPORT_ID: &str = "report-0123456789abcdef0";

/// Every operation of this service a caller can reach — the rule holds over the
/// whole implemented surface, so the tables below are built from this list
/// rather than from the operations the issue happened to cite.
const ALL_OPERATIONS: &[&str] = &[
    "GetResourceMetrics",
    "DescribeDimensionKeys",
    "CreatePerformanceAnalysisReport",
    "ListPerformanceAnalysisReports",
    "ListAvailableResourceMetrics",
    "ListAvailableResourceDimensions",
    "GetResourceMetadata",
    "GetPerformanceAnalysisReport",
    "DeletePerformanceAnalysisReport",
    "ListTagsForResource",
    "TagResource",
    "UntagResource",
];

/// One otherwise-valid request per implemented operation, with what a refusal
/// names its resource as: the `Identifier` for an operation that addresses a
/// data source, and the `ResourceARN` for a tag operation, which names its
/// resource by ARN.
///
/// Authorization is the only reason any of them could be refused, and each
/// performs something different (a read, a write, a delete), so the rule is
/// exercised over the whole surface rather than over one call.
fn requests(identifier: &str) -> Vec<(&'static str, Value, String)> {
    let report_arn = state::report_arn(REGION, "RDS", identifier, REPORT_ID);
    let rows = vec![
        (
            "GetResourceMetrics",
            json!({
                "ServiceType": "RDS",
                "Identifier": identifier,
                "MetricQueries": [{"Metric": "db.load.avg"}],
                "StartTime": T0,
                "EndTime": T0 + 3_600.0,
                "PeriodInSeconds": 300,
            }),
            identifier.to_string(),
        ),
        (
            "DescribeDimensionKeys",
            json!({
                "ServiceType": "RDS",
                "Identifier": identifier,
                "StartTime": T0,
                "EndTime": T0 + 3_600.0,
                "Metric": "db.load.avg",
                "GroupBy": {"Group": "db.sql"},
            }),
            identifier.to_string(),
        ),
        (
            "CreatePerformanceAnalysisReport",
            json!({
                "ServiceType": "RDS",
                "Identifier": identifier,
                "StartTime": T0,
                "EndTime": T0 + 3_600.0,
            }),
            identifier.to_string(),
        ),
        (
            "ListPerformanceAnalysisReports",
            json!({"ServiceType": "RDS", "Identifier": identifier}),
            identifier.to_string(),
        ),
        (
            // `os` is the one metric type documented for "All engines".
            "ListAvailableResourceMetrics",
            json!({
                "ServiceType": "RDS",
                "Identifier": identifier,
                "MetricTypes": ["os"],
            }),
            identifier.to_string(),
        ),
        (
            // `db.load` is one of the metric types the documented dimension
            // catalog publishes groups for.
            "ListAvailableResourceDimensions",
            json!({
                "ServiceType": "RDS",
                "Identifier": identifier,
                "Metrics": ["db.load"],
            }),
            identifier.to_string(),
        ),
        (
            "GetResourceMetadata",
            json!({"ServiceType": "RDS", "Identifier": identifier}),
            identifier.to_string(),
        ),
        (
            "GetPerformanceAnalysisReport",
            json!({
                "ServiceType": "RDS",
                "Identifier": identifier,
                "AnalysisReportId": REPORT_ID,
            }),
            identifier.to_string(),
        ),
        (
            "DeletePerformanceAnalysisReport",
            json!({
                "ServiceType": "RDS",
                "Identifier": identifier,
                "AnalysisReportId": REPORT_ID,
            }),
            identifier.to_string(),
        ),
        (
            "ListTagsForResource",
            json!({"ServiceType": "RDS", "ResourceARN": report_arn}),
            report_arn.clone(),
        ),
        (
            "TagResource",
            json!({
                "ServiceType": "RDS",
                "ResourceARN": report_arn,
                "Tags": [{"Key": "env", "Value": "test"}],
            }),
            report_arn.clone(),
        ),
        (
            "UntagResource",
            json!({
                "ServiceType": "RDS",
                "ResourceARN": report_arn,
                "TagKeys": ["env"],
            }),
            report_arn.clone(),
        ),
    ];
    // The table and the operation list are one surface: a new operation must be
    // added to both, or the rule would stop being asserted over all of them.
    assert_eq!(
        rows.iter().map(|(op, _, _)| *op).collect::<Vec<_>>(),
        ALL_OPERATIONS.to_vec(),
        "the request table must carry exactly one row per implemented operation"
    );
    rows
}

/// Create an IAM user and return the access key id minted for it.
async fn user_with_key(
    emulator: &smithy_server_runtime::MultiServiceRouter,
    user_name: &str,
) -> String {
    let (status, body) = iam_call(emulator, "CreateUser", &[("UserName", user_name)]).await;
    assert_eq!(status, 200, "CreateUser {user_name} must succeed: {body}");
    let (status, body) = iam_call(emulator, "CreateAccessKey", &[("UserName", user_name)]).await;
    assert_eq!(
        status, 200,
        "CreateAccessKey for {user_name} must succeed: {body}"
    );
    xml_element(&body, "AccessKeyId")
}

/// Embed `document` in the user as an inline policy.
async fn put_user_policy(
    emulator: &smithy_server_runtime::MultiServiceRouter,
    user_name: &str,
    document: &str,
) {
    let (status, body) = iam_call(
        emulator,
        "PutUserPolicy",
        &[
            ("UserName", user_name),
            ("PolicyName", "pi-access"),
            ("PolicyDocument", document),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "PutUserPolicy for {user_name} must succeed: {body}"
    );
}

/// Assert that `answer` is the implicit-deny refusal AWS builds for
/// `principal_suffix` performing `operation` on `resource`.
fn assert_implicit_deny(
    case: &str,
    answer: &crate::pi_harness::Answer,
    principal_suffix: &str,
    operation: &str,
    resource: &str,
) {
    assert_eq!(
        answer.error_code(),
        "AccessDeniedException",
        "case {case}: expected AccessDeniedException, got {} {:?}",
        answer.status,
        answer.body
    );
    assert_eq!(
        answer.status, 403,
        "case {case}: expected the authorization layer's 403, got {}",
        answer.status
    );
    let message = answer.error_message();
    assert!(
        message.starts_with("User: ") && message.contains(principal_suffix),
        "case {case}: expected the refusal to name the caller's principal \
         ({principal_suffix}), got {message:?}"
    );
    let expected = format!(
        "is not authorized to perform: pi:{operation} on resource: {resource} because no \
         identity-based policy allows the pi:{operation} action"
    );
    assert!(
        message.contains(&expected),
        "case {case}: expected a message containing {expected:?}, got {message:?}"
    );
    // A refused call performs nothing: no report is minted and no payload is
    // answered beside the error.
    assert!(
        answer.member("AnalysisReportId").is_null() && answer.member("MetricList").is_null(),
        "case {case}: expected no payload in a refusal, got {:?}",
        answer.body
    );
}

/// RULE: an IAM user with no identity-based policy is refused every operation
/// of this service, with the message IAM builds from the request.
#[tokio::test]
async fn an_identity_with_no_policy_is_refused_every_operation() {
    let emulator = emulator_with_identity_services();
    let access_key_id = user_with_key(&emulator, "no-policy").await;

    for (operation, input, resource) in requests(IDENTIFIER) {
        let answer = call_signed_by(&emulator, &access_key_id, operation, input).await;
        assert_implicit_deny(
            &format!("an unprivileged user / {operation}"),
            &answer,
            ":user/no-policy",
            operation,
            &resource,
        );
    }

    // Nothing the refused calls asked for happened: the ambient identity, which
    // IS authorized, sees an empty report collection for that data source.
    let listed = call(
        &emulator,
        "ListPerformanceAnalysisReports",
        json!({"ServiceType": "RDS", "Identifier": IDENTIFIER}),
    )
    .await;
    assert_eq!(
        listed.member("AnalysisReports").as_array().map(Vec::len),
        Some(0),
        "case after the refusals: expected no report to have been created, got {:?}",
        listed.body
    );
}

/// RULE: the decision is the policy's, per action and per resource — so the
/// same identity is served exactly the calls its policy allows and refused
/// every other one.
#[tokio::test]
async fn a_policy_authorizes_exactly_the_actions_and_resources_it_names() {
    let emulator = emulator_with_identity_services();

    // Each row is one policy document, differing in the form `Action` and
    // `Resource` take (a string or a list, a wildcard or an exact value), and
    // in which of the service's actions it therefore authorizes.
    struct Case {
        label: &'static str,
        user: &'static str,
        document: String,
        /// The operations this policy allows on [`IDENTIFIER`].
        allowed: &'static [&'static str],
    }

    let cases = vec![
        Case {
            label: "every action of this service on every resource",
            user: "pi-all",
            document: json!({
                "Version": "2012-10-17",
                "Statement": [{"Effect": "Allow", "Action": "pi:*", "Resource": "*"}],
            })
            .to_string(),
            allowed: ALL_OPERATIONS,
        },
        Case {
            label: "every action of every service",
            user: "admin",
            document: json!({
                "Version": "2012-10-17",
                "Statement": {"Effect": "Allow", "Action": "*", "Resource": "*"},
            })
            .to_string(),
            allowed: ALL_OPERATIONS,
        },
        Case {
            label: "one action, named exactly",
            user: "metrics-only",
            document: json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Action": "pi:GetResourceMetrics",
                    "Resource": "*",
                }],
            })
            .to_string(),
            allowed: &["GetResourceMetrics"],
        },
        Case {
            label: "an Action list mixing this service with another",
            user: "dimension-reader",
            document: json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Action": ["rds:Describe*", "pi:DescribeDimensionKeys"],
                    "Resource": "*",
                }],
            })
            .to_string(),
            allowed: &["DescribeDimensionKeys"],
        },
        Case {
            label: "another service's actions only",
            user: "rds-only",
            document: json!({
                "Version": "2012-10-17",
                "Statement": [{"Effect": "Allow", "Action": "rds:*", "Resource": "*"}],
            })
            .to_string(),
            allowed: &[],
        },
        Case {
            label: "an Allow overridden by an explicit Deny",
            user: "denied",
            document: json!({
                "Version": "2012-10-17",
                "Statement": [
                    {"Effect": "Allow", "Action": "pi:*", "Resource": "*"},
                    {"Effect": "Deny", "Action": "pi:*", "Resource": "*"},
                ],
            })
            .to_string(),
            allowed: &[],
        },
        Case {
            // A Deny this emulator cannot evaluate fails CLOSED: it may forbid
            // the call, so the call is not answered as if it permitted it.
            label: "an Allow and a Deny carrying a Condition",
            user: "conditionally-denied",
            document: json!({
                "Version": "2012-10-17",
                "Statement": [
                    {"Effect": "Allow", "Action": "pi:*", "Resource": "*"},
                    {
                        "Effect": "Deny",
                        "Action": "pi:*",
                        "Resource": "*",
                        "Condition": {"Bool": {"aws:SecureTransport": "false"}},
                    },
                ],
            })
            .to_string(),
            allowed: &[],
        },
        Case {
            // And an Allow this emulator cannot evaluate grants NOTHING: the
            // implicit deny stands rather than the statement being read as a
            // grant it may not be.
            label: "the only Allow carries a Condition",
            user: "conditionally-allowed",
            document: json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Action": "pi:*",
                    "Resource": "*",
                    "Condition": {"Bool": {"aws:MultiFactorAuthPresent": "true"}},
                }],
            })
            .to_string(),
            allowed: &[],
        },
        Case {
            // The complement of this service's own actions excludes every pi
            // action, so this Allow cannot be the grant either.
            label: "the only Allow is on everything but this service",
            user: "not-action-allowed",
            document: json!({
                "Version": "2012-10-17",
                "Statement": [{"Effect": "Allow", "NotAction": "pi:*", "Resource": "*"}],
            })
            .to_string(),
            allowed: &[],
        },
        Case {
            // The complement of another service's actions covers every pi
            // action, so this Deny denies them all.
            label: "a Deny on everything but another service",
            user: "not-action-denied",
            document: json!({
                "Version": "2012-10-17",
                "Statement": [
                    {"Effect": "Allow", "Action": "pi:*", "Resource": "*"},
                    {"Effect": "Deny", "NotAction": "s3:*", "Resource": "*"},
                ],
            })
            .to_string(),
            allowed: &[],
        },
        Case {
            // The complement of ANOTHER service's actions covers every pi
            // action, so this Allow IS the grant: IAM reads `NotAction` as
            // "every action but these".
            label: "an Allow on everything but another service",
            user: "not-action-allowed-elsewhere",
            document: json!({
                "Version": "2012-10-17",
                "Statement": [{"Effect": "Allow", "NotAction": "s3:*", "Resource": "*"}],
            })
            .to_string(),
            allowed: ALL_OPERATIONS,
        },
        Case {
            // `NotResource` excludes one other data source's `metrics` ARN, and
            // every resource this table addresses is a different one, so the
            // grant covers them all.
            label: "an Allow on every resource but another data source's",
            user: "not-resource-allowed",
            document: json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Action": "pi:*",
                    "NotResource": format!(
                        "arn:aws:pi:*:{}:metrics/RDS/{OTHER_IDENTIFIER}",
                        engine::emulator_account_id()
                    ),
                }],
            })
            .to_string(),
            allowed: ALL_OPERATIONS,
        },
        Case {
            // The complement of THIS service's actions covers no pi action, so
            // this Deny applies to nothing here and the Allow stands.
            label: "a Deny on everything but this service",
            user: "not-action-denied-elsewhere",
            document: json!({
                "Version": "2012-10-17",
                "Statement": [
                    {"Effect": "Allow", "Action": "pi:*", "Resource": "*"},
                    {"Effect": "Deny", "NotAction": "pi:*", "Resource": "*"},
                ],
            })
            .to_string(),
            allowed: ALL_OPERATIONS,
        },
        Case {
            // `NotResource` on the DENY covers every resource but the data
            // source's own `metrics` ARN, so the operations that address that
            // ARN survive the Deny and the ones addressing a `perf-reports` ARN
            // (the report lifecycle and the tag operations) do not.
            label: "a Deny on every resource but the data source's metrics ARN",
            user: "not-resource-denied",
            document: json!({
                "Version": "2012-10-17",
                "Statement": [
                    {"Effect": "Allow", "Action": "pi:*", "Resource": "*"},
                    {
                        "Effect": "Deny",
                        "Action": "pi:*",
                        "NotResource": format!(
                            "arn:aws:pi:*:{}:metrics/RDS/{IDENTIFIER}",
                            engine::emulator_account_id()
                        ),
                    },
                ],
            })
            .to_string(),
            allowed: &[
                "GetResourceMetrics",
                "DescribeDimensionKeys",
                "ListAvailableResourceMetrics",
                "ListAvailableResourceDimensions",
                "GetResourceMetadata",
            ],
        },
        Case {
            // An IAM ACTION name is compared case-insensitively, so a statement
            // spelling it in another case names the same action.
            label: "the action named in another case",
            user: "lowercase-action",
            document: json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Action": "pi:getresourcemetrics",
                    "Resource": "*",
                }],
            })
            .to_string(),
            allowed: &["GetResourceMetrics"],
        },
        Case {
            // A resource ARN, by contrast, is compared case-SENSITIVELY, so a
            // statement naming the data source in another case names a
            // different resource and grants nothing here.
            label: "a resource ARN differing from the request's only in case",
            user: "wrong-case-resource",
            document: json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Action": "pi:*",
                    "Resource": format!(
                        "arn:aws:pi:*:{}:metrics/RDS/{}",
                        engine::emulator_account_id(),
                        IDENTIFIER.to_lowercase()
                    ),
                }],
            })
            .to_string(),
            allowed: &[],
        },
    ];

    for case in cases {
        let access_key_id = user_with_key(&emulator, case.user).await;
        put_user_policy(&emulator, case.user, &case.document).await;

        for (operation, input, _resource) in requests(IDENTIFIER) {
            let label = format!("{} / {operation}", case.label);
            let answer = call_signed_by(&emulator, &access_key_id, operation, input).await;
            if case.allowed.contains(&operation) {
                // Get and Delete name a report no Create minted, which is this
                // service's own uniform `NotAuthorizedException` — an answer
                // about the RESOURCE, reached only because the CALLER was
                // authorized.
                assert_ne!(
                    answer.error_code(),
                    "AccessDeniedException",
                    "case {label}: expected the policy to authorize the call, got {:?}",
                    answer.body
                );
            } else {
                assert_eq!(
                    answer.error_code(),
                    "AccessDeniedException",
                    "case {label}: expected AccessDeniedException, got {} {:?}",
                    answer.status,
                    answer.body
                );
                assert_eq!(
                    answer.status, 403,
                    "case {label}: expected 403, got {}",
                    answer.status
                );
            }
        }
    }
}

/// RULE: a policy scoped to one resource authorizes the action on that resource
/// and on no other — the `Resource` of a `pi` statement is the data source's own
/// `metrics` ARN.
#[tokio::test]
async fn a_resource_scoped_policy_authorizes_only_that_resource() {
    let emulator = emulator_with_identity_services();
    let access_key_id = user_with_key(&emulator, "one-instance").await;
    let scoped = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Action": "pi:GetResourceMetrics",
            // The `metrics` resource type Performance Insights documents:
            // arn:aws:pi:<region>:<account>:metrics/<ServiceType>/<Identifier>.
            "Resource": format!(
                "arn:aws:pi:*:{}:metrics/RDS/{IDENTIFIER}",
                engine::emulator_account_id()
            ),
        }],
    })
    .to_string();
    put_user_policy(&emulator, "one-instance", &scoped).await;

    for (identifier, expected_denial) in [(IDENTIFIER, false), (OTHER_IDENTIFIER, true)] {
        let input = json!({
            "ServiceType": "RDS",
            "Identifier": identifier,
            "MetricQueries": [{"Metric": "db.load.avg"}],
            "StartTime": T0,
            "EndTime": T0 + 3_600.0,
            "PeriodInSeconds": 300,
        });
        let answer = call_signed_by(&emulator, &access_key_id, "GetResourceMetrics", input).await;
        if expected_denial {
            assert_implicit_deny(
                &format!("a policy scoped elsewhere / {identifier}"),
                &answer,
                ":user/one-instance",
                "GetResourceMetrics",
                identifier,
            );
        } else {
            assert_eq!(
                answer.status, 200,
                "case the resource the policy names ({identifier}): expected 200, got {} {:?}",
                answer.status, answer.body
            );
        }
    }
}

/// RULE: the identity evaluated is the one the signing key belongs to, whatever
/// kind it is — an assumed-role session is decided by its ROLE's policies, and
/// the refusal names the session's own principal.
#[tokio::test]
async fn an_assumed_role_session_is_decided_by_the_roles_policies() {
    let emulator = emulator_with_identity_services();
    let trust = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"Service": "ec2.amazonaws.com"},
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string();
    let (status, body) = iam_call(
        &emulator,
        "CreateRole",
        &[
            ("RoleName", "pi-reader"),
            ("AssumeRolePolicyDocument", &trust),
        ],
    )
    .await;
    assert_eq!(status, 200, "CreateRole must succeed: {body}");

    let role_arn = format!(
        "arn:aws:iam::{}:role/pi-reader",
        engine::emulator_account_id()
    );
    let (status, body) = sts_call(
        &emulator,
        "AssumeRole",
        &[("RoleArn", &role_arn), ("RoleSessionName", "charts")],
    )
    .await;
    assert_eq!(status, 200, "AssumeRole must succeed: {body}");
    let session_key_id = xml_element(&body, "AccessKeyId");

    let metrics = json!({
        "ServiceType": "RDS",
        "Identifier": IDENTIFIER,
        "MetricQueries": [{"Metric": "db.load.avg"}],
        "StartTime": T0,
        "EndTime": T0 + 3_600.0,
        "PeriodInSeconds": 300,
    });

    // The role has no policy yet: the session is refused, and the principal the
    // refusal names is the assumed-role one STS minted.
    let answer = call_signed_by(
        &emulator,
        &session_key_id,
        "GetResourceMetrics",
        metrics.clone(),
    )
    .await;
    assert_implicit_deny(
        "a session of a role with no policy",
        &answer,
        ":assumed-role/pi-reader/charts",
        "GetResourceMetrics",
        IDENTIFIER,
    );

    // The role's own inline policy is what authorizes the session.
    let document = json!({
        "Version": "2012-10-17",
        "Statement": [{"Effect": "Allow", "Action": "pi:GetResourceMetrics", "Resource": "*"}],
    })
    .to_string();
    let (status, body) = iam_call(
        &emulator,
        "PutRolePolicy",
        &[
            ("RoleName", "pi-reader"),
            ("PolicyName", "pi-read"),
            ("PolicyDocument", &document),
        ],
    )
    .await;
    assert_eq!(status, 200, "PutRolePolicy must succeed: {body}");

    let answer = call_signed_by(&emulator, &session_key_id, "GetResourceMetrics", metrics).await;
    assert_eq!(
        answer.status, 200,
        "case a session of a role the policy allows: expected 200, got {} {:?}",
        answer.status, answer.body
    );
}

/// RULE: a credential belonging to no IAM identity is the ambient account
/// identity, which no identity-based policy constrains — so the emulator's own
/// callers keep being served, and the gate refuses nothing it was not asked to.
#[tokio::test]
async fn a_credential_of_no_iam_identity_is_the_ambient_account_identity() {
    let emulator = emulator_with_identity_services();

    // One IAM user exists with no policy, so the store is populated: the
    // ambient key is served because it belongs to no identity, not because
    // there are no identities.
    let denied_key = user_with_key(&emulator, "no-policy").await;

    for (label, access_key_id) in [
        ("the emulator's own credential", DEFAULT_ACCESS_KEY_ID),
        ("a key id no identity holds", "AKIANOSUCHKEY0000000"),
    ] {
        let answer = call_signed_by(
            &emulator,
            access_key_id,
            "GetResourceMetrics",
            json!({
                "ServiceType": "RDS",
                "Identifier": IDENTIFIER,
                "MetricQueries": [{"Metric": "db.load.avg"}],
                "StartTime": T0,
                "EndTime": T0 + 3_600.0,
                "PeriodInSeconds": 300,
            }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {label}: expected 200, got {} {:?}",
            answer.status, answer.body
        );
    }

    // The same call on the same emulator IS refused for the IAM user, so the
    // two answers differ by the identity alone.
    let answer = call_signed_by(
        &emulator,
        &denied_key,
        "GetResourceMetrics",
        json!({
            "ServiceType": "RDS",
            "Identifier": IDENTIFIER,
            "MetricQueries": [{"Metric": "db.load.avg"}],
            "StartTime": T0,
            "EndTime": T0 + 3_600.0,
            "PeriodInSeconds": 300,
        }),
    )
    .await;
    assert_implicit_deny(
        "the unprivileged user on the same emulator",
        &answer,
        ":user/no-policy",
        "GetResourceMetrics",
        IDENTIFIER,
    );
}

/// RULE: authorization is decided BEFORE anything about the request's values,
/// so an unauthorized caller learns nothing from the refusal about whether what
/// it named was even well formed.
#[tokio::test]
async fn authorization_precedes_the_services_own_argument_checks() {
    let emulator = emulator_with_identity_services();
    let access_key_id = user_with_key(&emulator, "no-policy").await;

    // Each row is a request the SERVICE would refuse with an argument error
    // (an inverted window, a metric with no aggregate function, an undocumented
    // period) or with its own resource gate (an unmonitored data source).
    let cases: Vec<(&str, Value)> = vec![
        (
            "an inverted window",
            json!({
                "ServiceType": "RDS",
                "Identifier": IDENTIFIER,
                "MetricQueries": [{"Metric": "db.load.avg"}],
                "StartTime": T0,
                "EndTime": T0 - 1.0,
            }),
        ),
        (
            "a metric with no aggregate function",
            json!({
                "ServiceType": "RDS",
                "Identifier": IDENTIFIER,
                "MetricQueries": [{"Metric": "db.load"}],
                "StartTime": T0,
                "EndTime": T0 + 3_600.0,
            }),
        ),
        (
            "an undocumented PeriodInSeconds",
            json!({
                "ServiceType": "RDS",
                "Identifier": IDENTIFIER,
                "MetricQueries": [{"Metric": "db.load.avg"}],
                "StartTime": T0,
                "EndTime": T0 + 3_600.0,
                "PeriodInSeconds": 30,
            }),
        ),
        (
            "an unmonitored data source",
            json!({
                "ServiceType": "RDS",
                "Identifier": "my-production-database",
                "MetricQueries": [{"Metric": "db.load.avg"}],
                "StartTime": T0,
                "EndTime": T0 + 3_600.0,
            }),
        ),
    ];

    for (label, input) in cases {
        let answer = call_signed_by(&emulator, &access_key_id, "GetResourceMetrics", input).await;
        assert_eq!(
            answer.error_code(),
            "AccessDeniedException",
            "case {label}: expected the authorization refusal to come first, got {} {:?}",
            answer.status,
            answer.body
        );
    }
}
