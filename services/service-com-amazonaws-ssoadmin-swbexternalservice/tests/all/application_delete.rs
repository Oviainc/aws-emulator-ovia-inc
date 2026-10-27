// SPDX-License-Identifier: CC0-1.0

//! `DeleteApplication` end to end over the `awsJson1_1` wire — the operation the stub
//! issue asked for.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than over one
//! example:
//!
//! 1. a delete REMOVES exactly the application its `ApplicationArn` names: that ARN stops
//!    resolving through `DescribeApplication`, `ListApplications` stops carrying it, the
//!    stored record is gone, and every other stored application still resolves — whatever
//!    optional members, status, portal options, tags or client token the application was
//!    created with;
//! 2. everything held INSIDE the application goes with it: its access scopes, its
//!    authentication methods, its grants, its assignment and session configuration, its
//!    tags and its direct assignments all stop resolving, while the same resources of
//!    another application are untouched;
//! 3. a delete is NOT idempotent on an application nothing holds: an `ApplicationArn` no
//!    create ever minted, one a delete already removed, and one naming an instance or an
//!    account this emulator holds nothing under are each refused
//!    `ResourceNotFoundException` — never answered 200 — with the message naming the ARN
//!    that resolved to nothing, and they change no stored record;
//! 4. the `ClientToken` of the create that minted a deleted application is free again,
//!    because it was held ON the record: the same token can mint a DIFFERENT application
//!    afterwards instead of folding into the deleted one.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, so the modelled constraints,
//! the `awsJson1_1` envelope and the `X-Amz-Target` routing are exercised rather than
//! bypassed. The harness is the one `ssoadmin_permission_sets.rs` owns.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ssoadmin_permission_sets::{
    assert_refused, call, create_application_assignment, describe_application_assignment,
    router_over, seeded_instance_arn, with_store,
};

/// The OAuth 2.0 application provider every application below is created under.
///
/// SOURCE: `state::BUILT_IN_APPLICATION_PROVIDERS`, the catalogue
/// `ListApplicationProviders` reports, whose `custom` entry is the one provider a customer
/// managed application is created under.
const CUSTOM_PROVIDER_ARN: &str = "arn:aws:sso::aws:applicationProvider/custom";

/// An access-scope name of the three-segment form the model's `Scope` pattern admits.
///
/// SOURCE: `smithy_query(query="shape",shape="Scope")` —
/// `^([A-Za-z0-9_]{1,50})(:[A-Za-z0-9_]{1,50}){0,1}(:[A-Za-z0-9_]{1,50}){0,1}$`.
const SCOPE: &str = "sso:account:access";

/// The one value the model's `AuthenticationMethodType` enum admits.
///
/// SOURCE: `smithy_query(query="shape",shape="AuthenticationMethodType")`.
const IAM_METHOD: &str = "IAM";

/// A `GrantType` the model's enum admits, with the `Grant` union variant it selects.
///
/// SOURCE: `smithy_query(query="shape",shape="GrantType,Grant")`.
const GRANT_TYPE: &str = "refresh_token";

/// A principal of each `PrincipalType` the model admits, in the two forms its
/// `PrincipalId` pattern accepts (a bare GUID and the `[0-9a-f]{10}-` prefixed form).
///
/// SOURCE: `smithy_query(query="operation",operation="CreateApplicationAssignment")`.
const PRINCIPALS: &[(&str, &str, &str)] = &[
    (
        "a USER GUID",
        "USER",
        "0f1e2d3c-4b5a-4697-8869-7a6b5c4d3e2f",
    ),
    (
        "a prefixed GROUP id",
        "GROUP",
        "02468ace13-0a1b2c3d-4e5f-4061-8273-8495a6b7c8d9",
    ),
];

/// One row of the application table: a label and the create members beyond the two the
/// operation always needs (`InstanceArn`, `ApplicationProviderArn`).
struct ApplicationCase {
    label: &'static str,
    members: Value,
}

/// The applications rule 1 is asserted over.
///
/// The rows differ along every axis the create generalises over, so the removal cannot be
/// keyed to one request's shape: which optional members are present, `Name` at both
/// published bounds (`length 1..100`), `Description` at its upper bound (`length 1..128`),
/// both values of `ApplicationStatus`, both `SignInOrigin` forms and both values of
/// `ApplicationVisibility`, a tagged application and one created under a `ClientToken`.
/// The bounds and value sets come from
/// `smithy_query(query="operation",operation="CreateApplication")` and
/// `smithy_query(query="shape",shape="ApplicationStatus,ApplicationVisibility,SignInOrigin")`.
fn application_cases() -> Vec<ApplicationCase> {
    vec![
        ApplicationCase {
            label: "only the required members, Name at its 1-character lower bound",
            members: json!({"Name": "a"}),
        },
        ApplicationCase {
            label: "a Name at its 100-character upper bound",
            members: json!({"Name": "n".repeat(100)}),
        },
        ApplicationCase {
            label: "a Description at its 128-character upper bound, DISABLED",
            members: json!({
                "Name": "described and disabled",
                "Description": "d".repeat(128),
                "Status": "DISABLED",
            }),
        },
        ApplicationCase {
            label: "portal sign-in from the APPLICATION origin, portal visibility DISABLED",
            members: json!({
                "Name": "app-origin sign-in",
                "Status": "ENABLED",
                "PortalOptions": {
                    "SignInOptions": {
                        "Origin": "APPLICATION",
                        "ApplicationUrl": "https://app.example.com/start",
                    },
                    "Visibility": "DISABLED",
                },
            }),
        },
        ApplicationCase {
            label: "portal sign-in from the IDENTITY_CENTER origin, tagged",
            members: json!({
                "Name": "identity-center sign-in",
                "PortalOptions": {
                    "SignInOptions": {"Origin": "IDENTITY_CENTER"},
                    "Visibility": "ENABLED",
                },
                "Tags": [
                    {"Key": "Owner", "Value": "platform"},
                    {"Key": "Tier", "Value": "test"},
                ],
            }),
        },
        ApplicationCase {
            label: "created under a ClientToken",
            members: json!({"Name": "token-minted", "ClientToken": "delete-case-token"}),
        },
    ]
}

/// A router over a fresh engine the test also holds, so the stored record count can be
/// read the way a handler reads it.
fn fixture() -> (Arc<engine::InMemoryEngine>, MultiServiceRouter) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_over(engine.clone());
    (engine, router)
}

/// Create one application from a table row and return the `ApplicationArn` it minted.
async fn create_application(
    router: &MultiServiceRouter,
    instance_arn: &str,
    case: &str,
    members: &Value,
) -> String {
    let mut request = json!({
        "InstanceArn": instance_arn,
        "ApplicationProviderArn": CUSTOM_PROVIDER_ARN,
    });
    for (member, value) in members.as_object().expect("the row's members are an object") {
        request[member] = value.clone();
    }
    let (status, body) = call(router, "CreateApplication", &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateApplication must answer 200, got {status} {body}"
    );
    body["ApplicationArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: CreateApplication minted no ARN: {body}"))
        .to_string()
}

async fn delete_application(router: &MultiServiceRouter, application_arn: &str) -> (u16, Value) {
    call(
        router,
        "DeleteApplication",
        &json!({"ApplicationArn": application_arn}),
    )
    .await
}

async fn describe_application(router: &MultiServiceRouter, application_arn: &str) -> (u16, Value) {
    call(
        router,
        "DescribeApplication",
        &json!({"ApplicationArn": application_arn}),
    )
    .await
}

/// Every `ApplicationArn` the instance enumerates, walked the way a client walks it.
async fn listed_arns(router: &MultiServiceRouter, instance_arn: &str, case: &str) -> Vec<String> {
    let mut seen = Vec::new();
    let mut next_token: Option<String> = None;
    loop {
        let mut request = json!({"InstanceArn": instance_arn});
        if let Some(token) = &next_token {
            request["NextToken"] = json!(token);
        }
        let (status, body) = call(router, "ListApplications", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListApplications must answer 200, got {status} {body}"
        );
        let page = body["Applications"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: the answer carries no Applications: {body}"))
            .clone();
        for item in page {
            seen.push(
                item["ApplicationArn"]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: a listed item carries no ApplicationArn: {item}")
                    })
                    .to_string(),
            );
        }
        next_token = body["NextToken"].as_str().map(str::to_string);
        if next_token.is_none() {
            return seen;
        }
    }
}

/// The refusal an `ApplicationArn` this account holds no application for earns, as the
/// service words it for every operation that resolves one.
fn not_found_message(application_arn: &str) -> String {
    format!("Could not find Application with arn {application_arn}.")
}

/// Assert one call was refused `ResourceNotFoundException` naming `application_arn`.
fn assert_application_not_found(case: &str, what: &str, application_arn: &str, answer: &(u16, Value)) {
    let (status, body) = answer;
    assert_refused(case, what, "ResourceNotFoundException", *status, body);
    assert_eq!(
        body["Message"].as_str().unwrap_or_default(),
        not_found_message(application_arn),
        "case {case:?}: {what} must name the ARN that resolved to nothing, got {body}"
    );
}

/// How many applications and how many direct application assignments the store holds.
fn stored_counts(engine: &Arc<engine::InMemoryEngine>) -> (usize, usize) {
    with_store(engine, |store| {
        (store.applications.len(), store.application_assignments.len())
    })
}

/// Rule 1: every row of the table is removed by its own delete, and only that row.
#[tokio::test]
async fn a_delete_removes_exactly_the_application_it_names() {
    let (engine, router) = fixture();
    let instance_arn = seeded_instance_arn(&router).await;

    let cases = application_cases();
    let mut arns: Vec<(&'static str, String)> = Vec::new();
    for case in &cases {
        let arn = create_application(&router, &instance_arn, case.label, &case.members).await;
        arns.push((case.label, arn));
    }
    let (stored, _) = stored_counts(&engine);
    assert_eq!(
        stored,
        cases.len(),
        "the table's creates must store one application each, got {stored}"
    );

    for (index, (label, arn)) in arns.iter().enumerate() {
        let (status, body) = delete_application(&router, arn).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the delete must answer 200, got {status} {body}"
        );
        assert_eq!(
            body,
            json!({}),
            "case {label:?}: the model declares an empty output, so the delete must answer \
             an empty JSON object, got {body}"
        );

        let (stored, _) = stored_counts(&engine);
        assert_eq!(
            stored,
            cases.len() - index - 1,
            "case {label:?}: the delete must remove exactly one stored record, got {stored}"
        );

        // Every application already deleted stops resolving, through the describe and
        // through the enumeration alike.
        let listed = listed_arns(&router, &instance_arn, label).await;
        for (gone_label, gone_arn) in arns.iter().take(index + 1) {
            let answer = describe_application(&router, gone_arn).await;
            assert_application_not_found(
                gone_label,
                "a describe of a deleted application",
                gone_arn,
                &answer,
            );
            assert!(
                !listed.contains(gone_arn),
                "case {gone_label:?}: a deleted application must not be listed, got {listed:?}"
            );
        }
        // …and every application no delete has named yet still resolves, through both.
        for (kept_label, kept_arn) in arns.iter().skip(index + 1) {
            let (status, body) = describe_application(&router, kept_arn).await;
            assert_eq!(
                status, 200,
                "case {kept_label:?}: an application no delete named must still describe, \
                 got {status} {body}"
            );
            assert!(
                listed.contains(kept_arn),
                "case {kept_label:?}: an application no delete named must still be listed, \
                 got {listed:?}"
            );
        }
    }

    let (stored, _) = stored_counts(&engine);
    assert_eq!(
        stored, 0,
        "every application of the table was deleted, got {stored} stored"
    );
}

/// Every resource held inside an application, as the writes that store it and the reads
/// that resolve it: `(label, write operation, write body, read operation, read body)`.
fn held_inside(application_arn: &str) -> Vec<(&'static str, &'static str, Value, &'static str, Value)> {
    vec![
        (
            "an access scope",
            "PutApplicationAccessScope",
            json!({
                "ApplicationArn": application_arn,
                "Scope": SCOPE,
                "AuthorizedTargets": [application_arn],
            }),
            "GetApplicationAccessScope",
            json!({"ApplicationArn": application_arn, "Scope": SCOPE}),
        ),
        (
            "an authentication method",
            "PutApplicationAuthenticationMethod",
            json!({
                "ApplicationArn": application_arn,
                "AuthenticationMethodType": IAM_METHOD,
                "AuthenticationMethod": {
                    "Iam": {
                        "ActorPolicy": {
                            "Version": "2012-10-17",
                            "Statement": [{
                                "Effect": "Allow",
                                "Action": "sso-oauth:CreateTokenWithIAM",
                                "Resource": "*",
                            }],
                        },
                    },
                },
            }),
            "GetApplicationAuthenticationMethod",
            json!({"ApplicationArn": application_arn, "AuthenticationMethodType": IAM_METHOD}),
        ),
        (
            "an OAuth 2.0 grant",
            "PutApplicationGrant",
            json!({
                "ApplicationArn": application_arn,
                "GrantType": GRANT_TYPE,
                "Grant": {"RefreshToken": {}},
            }),
            "GetApplicationGrant",
            json!({"ApplicationArn": application_arn, "GrantType": GRANT_TYPE}),
        ),
        (
            "the assignment configuration",
            "PutApplicationAssignmentConfiguration",
            json!({"ApplicationArn": application_arn, "AssignmentRequired": false}),
            "GetApplicationAssignmentConfiguration",
            json!({"ApplicationArn": application_arn}),
        ),
        (
            "the session configuration",
            "PutApplicationSessionConfiguration",
            json!({
                "ApplicationArn": application_arn,
                "UserBackgroundSessionApplicationStatus": "ENABLED",
            }),
            "GetApplicationSessionConfiguration",
            json!({"ApplicationArn": application_arn}),
        ),
        (
            "the tag set",
            "TagResource",
            json!({
                "ResourceArn": application_arn,
                "Tags": [{"Key": "Held", "Value": "inside"}],
            }),
            "ListTagsForResource",
            json!({"ResourceArn": application_arn}),
        ),
    ]
}

/// Rule 2: the application's own resources go with it, and another application's do not.
#[tokio::test]
async fn deleting_an_application_removes_everything_held_inside_it() {
    let (engine, router) = fixture();
    let instance_arn = seeded_instance_arn(&router).await;

    let deleted =
        create_application(&router, &instance_arn, "the deleted one", &json!({"Name": "deleted"}))
            .await;
    let kept =
        create_application(&router, &instance_arn, "the kept one", &json!({"Name": "kept"})).await;

    // Both applications hold the same resources, so a reader that stops answering for one
    // of them can only have stopped because of the delete.
    for (owner, application_arn) in [("the deleted one", &deleted), ("the kept one", &kept)] {
        for (label, write, body, read, read_body) in held_inside(application_arn) {
            let case = format!("{label} of {owner}");
            let (status, answer) = call(&router, write, &body).await;
            assert_eq!(
                status, 200,
                "case {case:?}: {write} must answer 200, got {status} {answer}"
            );
            let (status, answer) = call(&router, read, &read_body).await;
            assert_eq!(
                status, 200,
                "case {case:?}: {read} must answer 200 before the delete, got {status} \
                 {answer}"
            );
        }
        for (label, principal_type, principal_id) in PRINCIPALS {
            let case = format!("{label} assigned to {owner}");
            let (status, answer) =
                create_application_assignment(&router, application_arn, principal_type, principal_id)
                    .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the grant must answer 200, got {status} {answer}"
            );
        }
    }
    let (stored, assignments) = stored_counts(&engine);
    assert_eq!(
        (stored, assignments),
        (2, 2 * PRINCIPALS.len()),
        "the fixture must hold two applications and their assignments, got {stored} \
         applications and {assignments} assignments"
    );

    // What the kept application answers before the delete, so it can be compared with
    // what it answers after.
    let mut before: Vec<(String, Value)> = Vec::new();
    for (label, _, _, read, read_body) in held_inside(&kept) {
        let (_, answer) = call(&router, read, &read_body).await;
        before.push((label.to_string(), answer));
    }

    let (status, body) = delete_application(&router, &deleted).await;
    assert_eq!(
        status, 200,
        "the delete must answer 200, got {status} {body}"
    );

    // Every resource of the deleted application stops resolving: the record that held it
    // is gone, so there is no application to resolve the read against.
    for (label, _, _, read, read_body) in held_inside(&deleted) {
        let case = format!("{label} of the deleted application");
        let answer = call(&router, read, &read_body).await;
        assert_refused(&case, read, "ResourceNotFoundException", answer.0, &answer.1);
    }
    // Its enumerations too — those of them that are implemented; the stub census
    // (`grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' */src/ops/`) still carries
    // `ListApplicationAssignments`, which is its own issue.
    for (list, request) in [
        ("ListApplicationAccessScopes", json!({"ApplicationArn": deleted})),
        ("ListApplicationAuthenticationMethods", json!({"ApplicationArn": deleted})),
        ("ListApplicationGrants", json!({"ApplicationArn": deleted})),
    ] {
        let answer = call(&router, list, &request).await;
        assert_application_not_found(list, list, &deleted, &answer);
    }
    // And its direct assignments, which are stored beside the record rather than on it.
    for (label, principal_type, principal_id) in PRINCIPALS {
        let case = format!("{label} assigned to the deleted application");
        let answer =
            describe_application_assignment(&router, &deleted, principal_type, principal_id).await;
        assert_refused(
            &case,
            "a describe of an assignment of the deleted application",
            "ResourceNotFoundException",
            answer.0,
            &answer.1,
        );
    }

    // The kept application answers exactly what it answered before, and keeps its own
    // assignments.
    for (index, (label, _, _, read, read_body)) in held_inside(&kept).into_iter().enumerate() {
        let case = format!("{label} of the kept application");
        let (status, answer) = call(&router, read, &read_body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: {read} must still answer 200, got {status} {answer}"
        );
        assert_eq!(
            answer, before[index].1,
            "case {case:?}: the delete of another application must change nothing {read} \
             reports, got {answer}"
        );
    }
    for (label, principal_type, principal_id) in PRINCIPALS {
        let case = format!("{label} assigned to the kept application");
        let (status, answer) =
            describe_application_assignment(&router, &kept, principal_type, principal_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: an assignment of another application must survive, got \
             {status} {answer}"
        );
    }

    let (stored, assignments) = stored_counts(&engine);
    assert_eq!(
        (stored, assignments),
        (1, PRINCIPALS.len()),
        "only the deleted application and its assignments may be gone, got {stored} \
         applications and {assignments} assignments"
    );
}

/// Rule 3: an application nothing holds has nothing to delete, for every way of not
/// being held.
#[tokio::test]
async fn a_delete_of_an_application_nothing_holds_is_refused_and_stores_nothing() {
    let (engine, router) = fixture();
    let instance_arn = seeded_instance_arn(&router).await;

    let kept =
        create_application(&router, &instance_arn, "the surviving one", &json!({"Name": "kept"}))
            .await;
    let removed = create_application(
        &router,
        &instance_arn,
        "the already-deleted one",
        &json!({"Name": "removed"}),
    )
    .await;
    let (status, body) = delete_application(&router, &removed).await;
    assert_eq!(
        status, 200,
        "the first delete must answer 200, got {status} {body}"
    );

    // The account and instance segments of a minted ARN, so an unknown application can be
    // named in each form the model's `ApplicationArn` pattern admits
    // (`^arn:aws(-[a-z]{1,5}){0,3}:sso::\d{12}:application/(sso)?ins-[a-zA-Z0-9-.]{16}/apl-[a-zA-Z0-9]{16}$`).
    let account = kept.split(':').nth(4).unwrap_or_default().to_string();
    let instance_id = instance_arn.rsplit('/').next().unwrap_or_default().to_string();

    let cases: Vec<(String, String)> = vec![
        (
            "an application id no create ever minted".to_string(),
            format!("arn:aws:sso::{account}:application/{instance_id}/apl-0000000000000000"),
        ),
        (
            "the same ARN a delete already removed".to_string(),
            removed.clone(),
        ),
        (
            "an instance this account holds nothing under".to_string(),
            format!(
                "arn:aws:sso::{account}:application/ssoins-0000000000000000/\
                 apl-1111111111111111"
            ),
        ),
        (
            "the unprefixed instance-id form the pattern admits".to_string(),
            format!("arn:aws:sso::{account}:application/ins-0a1b2c3d4e5f6071/apl-2222222222222222"),
        ),
        (
            "another partition the pattern admits".to_string(),
            format!("arn:aws-cn:sso::{account}:application/{instance_id}/apl-3333333333333333"),
        ),
        (
            "another account".to_string(),
            format!("arn:aws:sso::000000000000:application/{instance_id}/apl-4444444444444444"),
        ),
    ];

    for (case, application_arn) in &cases {
        let answer = delete_application(&router, application_arn).await;
        assert_application_not_found(case, "the delete", application_arn, &answer);
        let (stored, _) = stored_counts(&engine);
        assert_eq!(
            stored, 1,
            "case {case:?}: a refused delete must remove no stored record, got {stored}"
        );
    }

    // The one application nothing deleted survived every refusal above.
    let (status, body) = describe_application(&router, &kept).await;
    assert_eq!(
        status, 200,
        "the surviving application must still describe, got {status} {body}"
    );
}

/// Rule 4: the client token a deleted application was minted under is free again.
#[tokio::test]
async fn the_client_token_of_a_deleted_application_is_free_again() {
    let (_engine, router) = fixture();
    let instance_arn = seeded_instance_arn(&router).await;

    // `ClientToken` is `^[!-~]+$`, `length 1..64`
    // (`smithy_query(query="operation",operation="CreateApplication")`).
    let token = "reused-after-a-delete";
    let first = create_application(
        &router,
        &instance_arn,
        "the first create under the token",
        &json!({"Name": "first under the token", "ClientToken": token}),
    )
    .await;
    let (status, body) = delete_application(&router, &first).await;
    assert_eq!(
        status, 200,
        "the delete must answer 200, got {status} {body}"
    );

    // A create carrying the SAME token and a DIFFERENT request would be a conflict while
    // the first application existed; with that application gone the token names nothing,
    // so this is a new application rather than a refusal or a fold onto the deleted one.
    let second = create_application(
        &router,
        &instance_arn,
        "a different request under the same token",
        &json!({"Name": "second under the token", "ClientToken": token}),
    )
    .await;
    assert_ne!(
        second, first,
        "the second create must mint an application of its own, got {second}"
    );
    let (status, body) = describe_application(&router, &second).await;
    assert_eq!(
        status, 200,
        "the second application must describe, got {status} {body}"
    );
    assert_eq!(
        body["Name"].as_str().unwrap_or_default(),
        "second under the token",
        "the second application must report its OWN name, got {body}"
    );
    let answer = describe_application(&router, &first).await;
    assert_application_not_found(
        "the first create under the token",
        "a describe of the deleted application",
        &first,
        &answer,
    );
}
