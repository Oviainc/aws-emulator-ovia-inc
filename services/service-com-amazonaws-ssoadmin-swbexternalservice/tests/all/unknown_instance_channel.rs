// SPDX-License-Identifier: CC0-1.0

//! WHICH channel an `InstanceArn` this account holds no instance for is refused on, over
//! the operations of this service that resolve one, end to end over the `awsJson1_1` wire.
//!
//! The rule under test: an `InstanceArn` that satisfies the modelled pattern but resolves
//! to no instance of this account is a MISSING RESOURCE, not a syntax error, so every
//! operation that resolves one refuses it on the not-found channel it MODELS —
//! `ResourceNotFoundException` where the operation declares one, otherwise the
//! `AccessDeniedException` (HTTP 403) this service declares for every operation — and
//! never on the client-input `ValidationException` channel, whose modelled meaning is "the
//! request failed because it contains a syntax error". One condition therefore earns ONE
//! status and ONE error code from every enumeration, read and write that resolves an
//! instance, so a client branching on the error code (credential handling versus
//! request-fixing) classifies it the same way whichever operation it called.
//!
//! The error set of each row is the operation's own, from `smithy_query(query="digest")`:
//! the service's common errors are `AccessDeniedException` / `InternalServerException` /
//! `ThrottlingException` / `ValidationException`, so only an operation that declares
//! `ResourceNotFoundException` on top of them has a not-found channel of its own, and the
//! rest have exactly one channel left for a resource they cannot resolve.
//!
//! The table spans what the rule generalises over: both groups of operations (those
//! declaring `ResourceNotFoundException` and those not), reads and writes, enumerations and
//! single-resource calls, each crossed with the `InstanceArn` spellings the model's pattern
//! admits (`unknown_instance_message::UNKNOWN_INSTANCE_ARNS`). The detail TEXT these
//! refusals share, and that a refused call leaves the account exactly as it was, are
//! `unknown_instance_message.rs`, whose table spans the whole instance-resolving surface;
//! this suite is about the CHANNEL, which that suite's rows deliberately leave open, and it
//! reuses that suite's ARN rows and request constants rather than restating them.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, the one path a socket-served
//! request takes. The harness is the one `ssoadmin_permission_sets.rs` owns.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ssoadmin_permission_sets::{
    assert_refused, call, error_code, router, UNKNOWN_PERMISSION_SET_ARN,
};
use crate::unknown_instance_message::{PRINCIPAL_ID, PROVIDER_ARN, UNKNOWN_INSTANCE_ARNS};

/// The channel an operation's modelled error set leaves for an instance it cannot resolve.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Channel {
    /// The operation declares NO `ResourceNotFoundException`, so the one channel left for a
    /// resource the caller may not act on is the modelled `AccessDeniedException`, HTTP
    /// 403.
    AccessDenied,
    /// The operation declares `ResourceNotFoundException` — the not-found channel every
    /// missing resource of this service is reported on, with the status the crate's shared
    /// [`assert_refused`] pins for it.
    NotFound,
}

/// One row: the operation, the request it takes, and the channel its modelled error set
/// leaves for a missing instance.
struct Row {
    operation: &'static str,
    body: Value,
    channel: Channel,
}

/// The table, for one unknown `InstanceArn`.
///
/// Each body satisfies its operation's modelled constraints, so the only thing wrong with
/// it is the instance it names. The expected channel of each row is read off that
/// operation's modelled error set, never off what its handler happens to answer.
fn rows(instance_arn: &str) -> Vec<Row> {
    // Operations whose modelled errors are the service's common set ALONE — no
    // `ResourceNotFoundException` — so the rule leaves them `AccessDeniedException`: two
    // single-resource reads, three enumerations (one of them filtered and paged), a write
    // that mints a child, a write that changes the instance's Regions and a delete.
    let without_not_found: Vec<(&'static str, Value)> = vec![
        ("DescribeInstance", json!({"InstanceArn": instance_arn})),
        ("ListRegions", json!({"InstanceArn": instance_arn})),
        ("ListApplications", json!({"InstanceArn": instance_arn})),
        (
            "ListApplications",
            json!({
                "InstanceArn": instance_arn,
                "MaxResults": 1,
                "Filter": {"ApplicationProvider": PROVIDER_ARN},
            }),
        ),
        (
            "ListTrustedTokenIssuers",
            json!({"InstanceArn": instance_arn, "MaxResults": 10}),
        ),
        (
            "CreateTrustedTokenIssuer",
            json!({
                "InstanceArn": instance_arn,
                "Name": "AnIssuer",
                "TrustedTokenIssuerType": "OIDC_JWT",
                "TrustedTokenIssuerConfiguration": {
                    "OidcJwtConfiguration": {
                        "IssuerUrl": "https://example.com/oidc",
                        "ClaimAttributePath": "email",
                        "IdentityStoreAttributePath": "email",
                        "JwksRetrievalOption": "OPEN_ID_DISCOVERY",
                    },
                },
            }),
        ),
        (
            "AddRegion",
            json!({"InstanceArn": instance_arn, "RegionName": "us-west-2"}),
        ),
        ("DeleteInstance", json!({"InstanceArn": instance_arn})),
    ];

    // Operations that declare `ResourceNotFoundException` on top of the common set, so the
    // same condition goes on their own not-found channel — the contrast that makes this a
    // rule about the MODELLED error set rather than a blanket "always `AccessDenied`".
    let with_not_found: Vec<(&'static str, Value)> = vec![
        (
            "UpdateInstance",
            json!({"InstanceArn": instance_arn, "Name": "Renamed"}),
        ),
        (
            "DescribeRegion",
            json!({"InstanceArn": instance_arn, "RegionName": "us-east-1"}),
        ),
        (
            "RemoveRegion",
            json!({"InstanceArn": instance_arn, "RegionName": "us-west-2"}),
        ),
        (
            "CreateApplication",
            json!({
                "InstanceArn": instance_arn,
                "ApplicationProviderArn": PROVIDER_ARN,
                "Name": "AnApplication",
            }),
        ),
        (
            "ListApplicationAssignmentsForPrincipal",
            json!({
                "InstanceArn": instance_arn,
                "PrincipalId": PRINCIPAL_ID,
                "PrincipalType": "USER",
            }),
        ),
        (
            "ProvisionPermissionSet",
            json!({
                "InstanceArn": instance_arn,
                "PermissionSetArn": UNKNOWN_PERMISSION_SET_ARN,
                "TargetType": "ALL_PROVISIONED_ACCOUNTS",
            }),
        ),
    ];

    without_not_found
        .into_iter()
        .map(|(operation, body)| Row {
            operation,
            body,
            channel: Channel::AccessDenied,
        })
        .chain(with_not_found.into_iter().map(|(operation, body)| Row {
            operation,
            body,
            channel: Channel::NotFound,
        }))
        .collect()
}

/// Call one row and assert the channel its modelled error set leaves for a missing
/// instance.
async fn assert_channel(router: &MultiServiceRouter, instance_arn: &str, row: &Row) {
    let case = format!("{} on {instance_arn}", row.operation);
    let (status, body) = call(router, row.operation, &row.body).await;
    match row.channel {
        Channel::AccessDenied => {
            assert_eq!(
                status, 403,
                "case {case:?}: an InstanceArn this account holds no instance for expected \
                 HTTP 403, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "AccessDeniedException",
                "case {case:?}: expected AccessDeniedException, got {body}"
            );
        }
        Channel::NotFound => assert_refused(
            &case,
            "an InstanceArn this account holds no instance for",
            "ResourceNotFoundException",
            status,
            &body,
        ),
    }
}

/// THE RULE, over the whole table: a missing instance is refused on the not-found channel
/// the operation models — `ResourceNotFoundException` where the operation declares one, 403
/// `AccessDeniedException` where it does not — and the channel is a property of the
/// request, so the same row answers the same way when it is repeated.
#[tokio::test]
async fn a_missing_instance_is_refused_on_the_channel_the_operation_models() {
    for instance_arn in UNKNOWN_INSTANCE_ARNS {
        // A fresh store per ARN, so no row is answered out of what another row wrote.
        let router = router();
        let table = rows(instance_arn);
        assert!(
            !table.is_empty(),
            "case {instance_arn:?}: the table must drive at least one operation"
        );
        for row in &table {
            assert_channel(&router, instance_arn, row).await;
            assert_channel(&router, instance_arn, row).await;
        }
        // The instance the account DOES hold is still served, so no row of the table was
        // answered by a store a refusal had emptied.
        let (status, body) = call(&router, "ListInstances", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {instance_arn:?}: ListInstances expected 200 after the refusals, got \
             {status} {body}"
        );
        let held = body["Instances"][0]["InstanceArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let (status, body) = call(&router, "ListApplications", &json!({"InstanceArn": held})).await;
        assert_eq!(
            status, 200,
            "case {instance_arn:?}: ListApplications on the instance the account holds \
             expected 200, got {status} {body}"
        );
    }
}

/// The two groups answer the SAME condition, so operations that share one modelled error
/// set cannot disagree about its channel: a missing instance reads identically from each of
/// the three instance-scoped enumerations that model no `ResourceNotFoundException` and from
/// the single-resource read of the same instance — and from none of them as a syntax error.
#[tokio::test]
async fn the_operations_sharing_one_error_set_report_a_missing_instance_alike() {
    for instance_arn in UNKNOWN_INSTANCE_ARNS {
        let router = router();
        let mut answers: Vec<(&str, u16, String)> = Vec::new();
        for operation in [
            "ListApplications",
            "ListRegions",
            "ListTrustedTokenIssuers",
            "DescribeInstance",
        ] {
            let (status, body) =
                call(&router, operation, &json!({"InstanceArn": instance_arn})).await;
            answers.push((operation, status, error_code(&body)));
        }

        let (first_operation, first_status, first_code) = answers[0].clone();
        for (operation, status, code) in &answers {
            assert_eq!(
                (*status, code.as_str()),
                (first_status, first_code.as_str()),
                "case {operation:?} on {instance_arn}: operations sharing one modelled error \
                 set must report a missing instance alike, got {status} {code} instead of the \
                 {first_status} {first_code} of {first_operation:?}"
            );
            assert_ne!(
                code, "ValidationException",
                "case {operation:?} on {instance_arn}: an ARN the modelled pattern admits \
                 that resolves to nothing is not a syntax error, got {status} {code}"
            );
        }
    }
}
