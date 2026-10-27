// SPDX-License-Identifier: CC0-1.0

//! `GetApplicationGrant` against the rest of the application record, end to end over the
//! `awsJson1_1` wire.
//!
//! The rule under test: a grant is ONE member of the application record, keyed by its
//! `GrantType`, and `GetApplicationGrant` is a report that answers from the grant stored
//! under the type it names and from nothing else. It is asserted in BOTH directions, each
//! over a TABLE of calls rather than one example:
//!
//! 1. a write to any OTHER member of the same application — its name, description, status
//!    and portal sign-in options, its authentication method, its access scopes, its
//!    assignment configuration and its tags, each added, replaced and removed — leaves
//!    every stored grant exactly as the put of that type left it, for every `GrantType`
//!    the model admits;
//! 2. a write to a grant — a put of each type, a replacing put, a delete — leaves every
//!    other reader of the same application (`DescribeApplication`,
//!    `ListApplicationAuthenticationMethods`, `ListApplicationAccessScopes`,
//!    `GetApplicationAssignmentConfiguration`, `ListTagsForResource`) answering exactly
//!    what it answered before, and a deleted grant stops resolving while the application
//!    and its remaining grants survive.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, so the modelled constraints,
//! the `awsJson1_1` envelope and the `X-Amz-Target` routing are exercised rather than
//! bypassed. The harness is the one `ssoadmin_permission_sets.rs` owns and the grant table
//! is the one `application_grants.rs` owns.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::application_authentication_methods::IAM;
use crate::application_grants::{
    create_application, delete_grant, get_grant, one_grant_per_type, put_grant, GrantCase,
};
use crate::ssoadmin_permission_sets::{assert_refused, call, router, seeded_instance_arn};

/// An access-scope name of the three-segment form the model's `Scope` pattern admits.
///
/// SOURCE: `smithy_query(query="shape",shape="Scope")` —
/// `^([A-Za-z0-9_]{1,50})(:[A-Za-z0-9_]{1,50}){0,1}(:[A-Za-z0-9_]{1,50}){0,1}$`.
const SCOPE: &str = "s3:access_grants:read_only";

/// An IAM policy document for `IamAuthenticationMethod$ActorPolicy`, documented as "An
/// IAM policy document in JSON".
fn actor_policy(action: &str) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{"Effect": "Allow", "Action": action, "Resource": "*"}],
    })
}

/// Every write that touches a member of an application OTHER than its grants, in an order
/// that exercises each one ADDED, REPLACED and REMOVED.
///
/// The rows span the application members the service stores beside the grant map:
/// `UpdateApplication`'s own members, the authentication method
/// (`application.authentication_methods`), the access scopes
/// (`application.access_scopes`), the assignment configuration and the tag set.
/// `AuthorizedTargets` entries are an application ARN and an instance ARN, the two forms
/// `smithy_query(query="shape",shape="ScopeTargets")` admits for `ScopeTarget`.
fn unrelated_writes(
    application_arn: &str,
    instance_arn: &str,
) -> Vec<(&'static str, &'static str, Value)> {
    vec![
        (
            "renaming the application",
            "UpdateApplication",
            json!({"ApplicationArn": application_arn, "Name": "renamed by an unrelated write"}),
        ),
        (
            "storing a description and disabling the application",
            "UpdateApplication",
            json!({
                "ApplicationArn": application_arn,
                "Description": "an unrelated write to the same record",
                "Status": "DISABLED",
            }),
        ),
        (
            "replacing the portal sign-in options",
            "UpdateApplication",
            json!({
                "ApplicationArn": application_arn,
                "PortalOptions": {
                    "SignInOptions": {
                        "Origin": "APPLICATION",
                        "ApplicationUrl": "https://unrelated.example.com/start",
                    },
                },
            }),
        ),
        (
            "adding an IAM authentication method",
            "PutApplicationAuthenticationMethod",
            json!({
                "ApplicationArn": application_arn,
                "AuthenticationMethodType": IAM,
                "AuthenticationMethod": {
                    "Iam": {"ActorPolicy": actor_policy("sso-oauth:CreateTokenWithIAM")},
                },
            }),
        ),
        (
            "replacing that authentication method",
            "PutApplicationAuthenticationMethod",
            json!({
                "ApplicationArn": application_arn,
                "AuthenticationMethodType": IAM,
                "AuthenticationMethod": {
                    "Iam": {"ActorPolicy": actor_policy("sso-oauth:*")},
                },
            }),
        ),
        (
            "removing the authentication method",
            "DeleteApplicationAuthenticationMethod",
            json!({"ApplicationArn": application_arn, "AuthenticationMethodType": IAM}),
        ),
        (
            "adding an access scope with authorized targets",
            "PutApplicationAccessScope",
            json!({
                "ApplicationArn": application_arn,
                "Scope": SCOPE,
                "AuthorizedTargets": [application_arn],
            }),
        ),
        (
            "replacing that scope's authorized targets",
            "PutApplicationAccessScope",
            json!({
                "ApplicationArn": application_arn,
                "Scope": SCOPE,
                "AuthorizedTargets": [application_arn, instance_arn],
            }),
        ),
        (
            "removing the access scope",
            "DeleteApplicationAccessScope",
            json!({"ApplicationArn": application_arn, "Scope": SCOPE}),
        ),
        (
            "making assignments optional",
            "PutApplicationAssignmentConfiguration",
            json!({"ApplicationArn": application_arn, "AssignmentRequired": false}),
        ),
        (
            "requiring assignments again",
            "PutApplicationAssignmentConfiguration",
            json!({"ApplicationArn": application_arn, "AssignmentRequired": true}),
        ),
        (
            "tagging the application",
            "TagResource",
            json!({
                "ResourceArn": application_arn,
                "Tags": [{"Key": "env", "Value": "prod"}, {"Key": "owner", "Value": "platform"}],
            }),
        ),
        (
            "untagging the application",
            "UntagResource",
            json!({"ResourceArn": application_arn, "TagKeys": ["env", "owner"]}),
        ),
    ]
}

/// Every stored grant of `application_arn`, read one `GrantType` at a time through the
/// operation under test.
async fn grant_snapshot(
    router: &MultiServiceRouter,
    application_arn: &str,
    rows: &[GrantCase],
) -> Vec<(&'static str, Value)> {
    let mut snapshot = Vec::new();
    for row in rows {
        let (status, body) = get_grant(router, application_arn, row.grant_type).await;
        assert_eq!(
            status, 200,
            "case {:?}: the get of a stored grant must answer 200, got {status} {body}",
            row.label
        );
        snapshot.push((row.grant_type, body));
    }
    snapshot
}

/// What every reader of `application_arn` OTHER than the grant readers answers.
async fn other_members_snapshot(router: &MultiServiceRouter, application_arn: &str) -> Vec<Value> {
    let readers: Vec<(&str, Value)> = vec![
        (
            "DescribeApplication",
            json!({"ApplicationArn": application_arn}),
        ),
        (
            "ListApplicationAuthenticationMethods",
            json!({"ApplicationArn": application_arn}),
        ),
        (
            "ListApplicationAccessScopes",
            json!({"ApplicationArn": application_arn}),
        ),
        (
            "GetApplicationAssignmentConfiguration",
            json!({"ApplicationArn": application_arn}),
        ),
        (
            "ListTagsForResource",
            json!({"ResourceArn": application_arn}),
        ),
    ];
    let mut snapshot = Vec::new();
    for (target, request) in readers {
        let (status, body) = call(router, target, &request).await;
        assert_eq!(
            status, 200,
            "case {target:?}: the reader must answer 200, got {status} {body}"
        );
        snapshot.push(json!({"reader": target, "answer": body}));
    }
    snapshot
}

/// Rule 1: whatever else is written to the application, every stored grant reads back
/// exactly as its put left it.
#[tokio::test]
async fn a_write_to_another_member_leaves_every_grant_untouched() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;
    let application_arn = create_application(&router, &instance_arn, "grant-independence").await;

    // One grant of EVERY modelled `GrantType`, so the claim is about the whole grant map
    // rather than one type.
    let rows = one_grant_per_type();
    for row in &rows {
        let (status, body) = put_grant(&router, &application_arn, row.grant_type, &row.grant).await;
        assert_eq!(
            status, 200,
            "case {:?}: the put must answer 200, got {status} {body}",
            row.label
        );
    }

    let expected: Vec<(&str, Value)> = rows
        .iter()
        .map(|row| (row.grant_type, json!({"Grant": row.grant})))
        .collect();
    let baseline = grant_snapshot(&router, &application_arn, &rows).await;
    assert_eq!(
        baseline, expected,
        "before any unrelated write, each get must report the grant its put stored, got \
         {baseline:?}"
    );

    for (case, target, request) in unrelated_writes(&application_arn, &instance_arn) {
        let (status, body) = call(&router, target, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: {target} must answer 200, got {status} {body}"
        );

        let after = grant_snapshot(&router, &application_arn, &rows).await;
        assert_eq!(
            after, expected,
            "case {case:?}: {target} writes another member of the record, so every grant must \
             still read back as its put stored it, got {after:?}"
        );
    }
}

/// Rule 2: a grant write is confined to the grant map — every other reader of the same
/// application answers exactly what it answered before — and a delete removes exactly the
/// grant it names.
#[tokio::test]
async fn a_grant_write_leaves_every_other_member_untouched() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;
    let application_arn = create_application(&router, &instance_arn, "grant-confinement").await;

    // Give the application something to lose on every other member, so "unchanged" is a
    // claim about stored values rather than about absent ones.
    for (case, target, request) in unrelated_writes(&application_arn, &instance_arn)
        .into_iter()
        .filter(|(case, _, _)| !case.starts_with("removing") && !case.starts_with("untagging"))
    {
        let (status, body) = call(&router, target, &request).await;
        assert_eq!(
            status, 200,
            "setting up {case:?}: {target} must answer 200, got {status} {body}"
        );
    }
    let baseline = other_members_snapshot(&router, &application_arn).await;

    let rows = one_grant_per_type();
    for row in &rows {
        let label = row.label;
        // The put of a type the application does not carry, then the put of one it does.
        for pass in ["a first put", "a repeated put"] {
            let (status, body) =
                put_grant(&router, &application_arn, row.grant_type, &row.grant).await;
            assert_eq!(
                status, 200,
                "case {label:?}: {pass} must answer 200, got {status} {body}"
            );
            let after = other_members_snapshot(&router, &application_arn).await;
            assert_eq!(
                after, baseline,
                "case {label:?}: {pass} touches the grant map only, so every other reader must \
                 still answer {baseline:?}, got {after:?}"
            );
        }

        let (status, read) = get_grant(&router, &application_arn, row.grant_type).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the get after the puts must answer 200, got {status} {read}"
        );
        assert_eq!(
            read,
            json!({"Grant": row.grant}),
            "case {label:?}: the get must report exactly the grant the put stored, got {read}"
        );

        let (status, body) = delete_grant(&router, &application_arn, row.grant_type).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the delete must answer 200, got {status} {body}"
        );
        let (status, body) = get_grant(&router, &application_arn, row.grant_type).await;
        assert_refused(
            label,
            "a get of the deleted grant type",
            "ResourceNotFoundException",
            status,
            &body,
        );
        let after = other_members_snapshot(&router, &application_arn).await;
        assert_eq!(
            after, baseline,
            "case {label:?}: the delete touches the grant map only, so every other reader must \
             still answer {baseline:?}, got {after:?}"
        );
    }
}
