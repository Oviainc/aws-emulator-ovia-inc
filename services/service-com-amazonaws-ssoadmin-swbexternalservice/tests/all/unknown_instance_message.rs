// SPDX-License-Identifier: CC0-1.0

//! How an `InstanceArn` this account holds no instance for is REPORTED, over the whole
//! API surface, end to end over the `awsJson1_1` wire.
//!
//! The rule under test: an `InstanceArn` that resolves to no instance of this account is
//! one condition, so every operation that resolves one states it with the service's single
//! detail text for that condition — the text naming the ARN the request sent — whichever
//! operation a client called. WHICH error shape carries it is each operation's own
//! modelled business (`ResourceNotFoundException` where the operation declares one,
//! otherwise the `AccessDeniedException` every operation of this service declares, which
//! `smithy_query(query="operation",operation="DescribeInstance,AddRegion,ListRegions,ListApplications,ListTrustedTokenIssuers,CreateTrustedTokenIssuer")`
//! shows for those, and which `unknown_instance_channel.rs` asserts row by row); the TEXT
//! is one. A client that branches on the detail text — the only
//! member these error shapes carry — therefore reads one ARN the same way everywhere.
//!
//! The table spans what the rule generalises over: EVERY operation of this service that
//! resolves an `InstanceArn` (the permission-set / account-assignment / ABAC surface
//! `account_instance_restriction.rs` tabulates, plus the instance, Region, application,
//! trusted-token-issuer, provisioning and tag operations), crossed with several forms the
//! `InstanceArn` pattern admits — both the `ssoins-` and the bare `ins-` spelling, a
//! second partition, and an instance id spanning the id charset. The recorded case (one
//! `CreatePermissionSet` on `arn:aws:sso:::instance/ssoins-…`) is one row of it.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, the one path a socket-served
//! request takes. The harness is the one `ssoadmin_permission_sets.rs` owns.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::account_instance_restriction::{surface_calls, VARIANTS};
use crate::ssoadmin_permission_sets::{
    call, error_code, router, UNKNOWN_INSTANCE_ARN, UNKNOWN_PERMISSION_SET_ARN,
};

/// The `InstanceArn` forms the rows are asserted over. SOURCE: the model's `InstanceArn`
/// pattern `^arn:aws(-[a-z]{1,5}){0,3}:sso:::instance/(sso)?ins-[a-zA-Z0-9-.]{16}$` —
/// the rows span the optional `sso` prefix of the id, a partition suffix the pattern
/// admits, and an id built from the whole `[a-zA-Z0-9-.]` class. None of them is held by
/// this account, and none is the id the store seeds (`state::SEED_INSTANCE_ID`).
pub(crate) const UNKNOWN_INSTANCE_ARNS: [&str; 4] = [
    // The recorded shape of the request.
    UNKNOWN_INSTANCE_ARN,
    "arn:aws:sso:::instance/ins-1111111111111111",
    "arn:aws-us-gov:sso:::instance/ssoins-a.b-C9d8e7f6g5h4",
    "arn:aws-cn:sso:::instance/ins-ZZ99.88--aa77bbc",
];

/// A `UUId` (`length 36..36`, lowercase hex groups) for the status reads, which no
/// workflow of this store ever issued.
const UNISSUED_REQUEST_ID: &str = "11111111-2222-4333-8444-555555555555";

/// A `PrincipalId` the model's pattern admits.
pub(crate) const PRINCIPAL_ID: &str = "f81d4fae-7dec-11d0-a765-00a0c91e6bf6";

/// A 12-digit account id (`^\d{12}$`).
const ACCOUNT_ID: &str = "123456789012";

/// An `ApplicationProviderArn` this service holds a provider for, so a row of
/// `CreateApplication` fails on the instance rather than on its provider.
pub(crate) const PROVIDER_ARN: &str = "arn:aws:sso::aws:applicationProvider/custom";

/// One call that names an `InstanceArn`: the operation and the body it takes.
struct Call {
    label: String,
    operation: &'static str,
    body: Value,
}

/// Every operation OUTSIDE the permission-set / account-assignment / ABAC surface that
/// resolves an `InstanceArn`, as the one request each takes for `instance_arn`.
///
/// Each body satisfies the operation's modelled constraints, so the only thing wrong with
/// it is the instance it names.
fn instance_arn_calls(instance_arn: &str) -> Vec<Call> {
    let rows: Vec<(&'static str, Value)> = vec![
        ("DescribeInstance", json!({"InstanceArn": instance_arn})),
        ("DeleteInstance", json!({"InstanceArn": instance_arn})),
        (
            "UpdateInstance",
            json!({"InstanceArn": instance_arn, "Name": "Renamed"}),
        ),
        (
            "AddRegion",
            json!({"InstanceArn": instance_arn, "RegionName": "us-west-2"}),
        ),
        (
            "RemoveRegion",
            json!({"InstanceArn": instance_arn, "RegionName": "us-west-2"}),
        ),
        (
            "DescribeRegion",
            json!({"InstanceArn": instance_arn, "RegionName": "us-east-1"}),
        ),
        ("ListRegions", json!({"InstanceArn": instance_arn})),
        (
            "CreateApplication",
            json!({
                "InstanceArn": instance_arn,
                "ApplicationProviderArn": PROVIDER_ARN,
                "Name": "AnApplication",
            }),
        ),
        ("ListApplications", json!({"InstanceArn": instance_arn})),
        (
            "ListApplicationAssignmentsForPrincipal",
            json!({
                "InstanceArn": instance_arn,
                "PrincipalId": PRINCIPAL_ID,
                "PrincipalType": "USER",
            }),
        ),
        (
            "ListAccountAssignmentsForPrincipal",
            json!({
                "InstanceArn": instance_arn,
                "PrincipalId": PRINCIPAL_ID,
                "PrincipalType": "GROUP",
            }),
        ),
        (
            "ListPermissionSetsProvisionedToAccount",
            json!({"InstanceArn": instance_arn, "AccountId": ACCOUNT_ID}),
        ),
        (
            "ProvisionPermissionSet",
            json!({
                "InstanceArn": instance_arn,
                "PermissionSetArn": UNKNOWN_PERMISSION_SET_ARN,
                "TargetType": "ALL_PROVISIONED_ACCOUNTS",
            }),
        ),
        (
            "DescribePermissionSetProvisioningStatus",
            json!({
                "InstanceArn": instance_arn,
                "ProvisionPermissionSetRequestId": UNISSUED_REQUEST_ID,
            }),
        ),
        (
            "ListPermissionSetProvisioningStatus",
            json!({"InstanceArn": instance_arn}),
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
            "ListTrustedTokenIssuers",
            json!({"InstanceArn": instance_arn}),
        ),
        // The tag surface takes the instance as an OPTIONAL member beside the resource it
        // tags; an instance ARN is itself a `TaggableResourceArn`, so these rows name the
        // same unknown instance twice.
        (
            "TagResource",
            json!({
                "InstanceArn": instance_arn,
                "ResourceArn": instance_arn,
                "Tags": [{"Key": "owner", "Value": "platform"}],
            }),
        ),
        (
            "UntagResource",
            json!({
                "InstanceArn": instance_arn,
                "ResourceArn": instance_arn,
                "TagKeys": ["owner"],
            }),
        ),
        (
            "ListTagsForResource",
            json!({"InstanceArn": instance_arn, "ResourceArn": instance_arn}),
        ),
    ];
    rows.into_iter()
        .map(|(operation, body)| Call {
            label: format!("{operation} on {instance_arn}"),
            operation,
            body,
        })
        .collect()
}

/// The same table, with the permission-set / account-assignment / ABAC surface added: one
/// row per operation of this service that resolves an `InstanceArn`.
fn every_call(instance_arn: &str) -> Vec<Call> {
    let mut calls = instance_arn_calls(instance_arn);
    // One variant of the surface table is enough here: the rule is about the INSTANCE, and
    // `account_instance_restriction.rs` already crosses that surface with every other
    // input. `VARIANTS` is never empty.
    let variant = &VARIANTS[0];
    calls.extend(surface_calls(instance_arn, variant).into_iter().map(
        |surface_call| Call {
            label: format!("{} on {instance_arn}", surface_call.operation),
            operation: surface_call.operation,
            body: surface_call.body,
        },
    ));
    calls
}

/// The detail text of a refusal, from the member this `awsJson1_1` model carries it in.
fn message(body: &Value) -> String {
    body["Message"]
        .as_str()
        .or_else(|| body["message"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// Call `case` and answer the detail text it was refused with, having asserted the refusal
/// is a modelled not-found rather than anything else.
async fn refusal_text(router: &MultiServiceRouter, case: &Call) -> String {
    let (status, body) = call(router, case.operation, &case.body).await;
    assert!(
        status >= 400,
        "case {:?}: an InstanceArn this account holds no instance for must be refused, got \
         {status} {body}",
        case.label
    );
    let code = error_code(&body);
    assert!(
        matches!(
            code.as_str(),
            "ResourceNotFoundException" | "AccessDeniedException" | "ValidationException"
        ),
        "case {:?}: a missing instance is reported on a channel the operation MODELS, got \
         {code} {body}",
        case.label
    );
    let text = message(&body);
    assert!(
        !text.is_empty(),
        "case {:?}: the refusal must carry its detail text, got {body}",
        case.label
    );
    text
}

/// THE RULE: for one unknown `InstanceArn`, every operation that resolves it answers with
/// the SAME detail text — so the text cannot depend on the operation or on the channel it
/// answers on — and that text names the ARN the request sent.
#[tokio::test]
async fn one_unknown_instance_arn_reads_the_same_whichever_operation_is_called() {
    for instance_arn in UNKNOWN_INSTANCE_ARNS {
        let router = router();
        let cases = every_call(instance_arn);
        let mut agreed: Option<(String, String)> = None;

        for case in &cases {
            let text = refusal_text(&router, case).await;
            assert!(
                text.contains(instance_arn),
                "case {:?}: the refusal must name the ARN that resolved to nothing, got \
                 {text:?}",
                case.label
            );
            match &agreed {
                None => agreed = Some((case.label.clone(), text)),
                Some((first_label, first_text)) => assert_eq!(
                    &text, first_text,
                    "case {:?}: one unknown InstanceArn must read the same as it does for \
                     {first_label:?}, got {text:?} instead of {first_text:?}",
                    case.label
                ),
            }
        }

        assert!(
            agreed.is_some(),
            "case {instance_arn:?}: the table must drive at least one operation"
        );
    }
}

/// The text is BUILT from the request: two unknown ARNs earn two refusals that differ only
/// in the ARN they name, so nothing about the reporting is a constant that happens to
/// contain one recorded ARN.
#[tokio::test]
async fn the_refusal_names_the_arn_the_request_sent() {
    let router = router();
    let mut texts: Vec<(&str, String)> = Vec::new();

    for instance_arn in UNKNOWN_INSTANCE_ARNS {
        let case = Call {
            label: format!("DescribeInstance on {instance_arn}"),
            operation: "DescribeInstance",
            body: json!({"InstanceArn": instance_arn}),
        };
        let text = refusal_text(&router, &case).await;
        texts.push((instance_arn, text));
    }

    for (instance_arn, text) in &texts {
        assert!(
            text.contains(instance_arn),
            "case {instance_arn:?}: the refusal must name this ARN, got {text:?}"
        );
        // The template is the same sentence for every row: replacing the ARN with another
        // row's ARN yields exactly that row's text.
        for (other_arn, other_text) in &texts {
            let rewritten = text.replace(instance_arn, other_arn);
            assert_eq!(
                &rewritten, other_text,
                "case {instance_arn:?}: the ARN is the only part of the refusal that \
                 varies, got {rewritten:?} instead of {other_text:?}"
            );
        }
    }
}

/// A REFUSED call changes nothing: the unknown instance is reported, no instance is minted
/// under the ARN it named, and the account's own instance is untouched — so the same text
/// is the answer to the same call made again.
#[tokio::test]
async fn a_refused_call_leaves_the_account_as_it_was() {
    let router = router();
    let (status, before) = call(&router, "ListInstances", &json!({})).await;
    assert_eq!(status, 200, "ListInstances expected 200, got {status} {before}");

    for instance_arn in UNKNOWN_INSTANCE_ARNS {
        for case in every_call(instance_arn) {
            let first = refusal_text(&router, &case).await;
            let again = refusal_text(&router, &case).await;
            assert_eq!(
                first, again,
                "case {:?}: the same call must read the same way twice, got {first:?} then \
                 {again:?}",
                case.label
            );
        }
    }

    let (status, after) = call(&router, "ListInstances", &json!({})).await;
    assert_eq!(status, 200, "ListInstances expected 200, got {status} {after}");
    assert_eq!(
        before, after,
        "a refused call must change nothing a later caller sees, got {before} then {after}"
    );
}
