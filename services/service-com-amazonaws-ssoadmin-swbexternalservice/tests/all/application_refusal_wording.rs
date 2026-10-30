// SPDX-License-Identifier: CC0-1.0

//! How the APPLICATION surface words its refusals, end to end over the `awsJson1_1` wire.
//!
//! The rules under test:
//!
//! 1. a not-found on the application surface says WHAT was not found and the application it
//!    was looked for under, as `No <what> found for application arn: <arn>` — the
//!    application itself is the `resource`, and each singleton child a caller configures on
//!    it (an access scope, an authentication method, a grant) is named with the value the
//!    request asked for. One condition reads ONE way whichever operation of the surface the
//!    client called;
//! 2. a principal that holds no DIRECT assignment to an application that DOES exist is
//!    refused with the error class alone, carrying no detail text;
//! 3. the TAGGING surface reaches four kinds of resource through one `ResourceArn`, so its
//!    writes name the kind the ARN declares and the id asked for
//!    (`Could not find <Kind> with id <arn>.`), while its REPORT words a missing application
//!    as the application's own reads do (rule 1);
//! 4. a tagging write that would leave a resource carrying more tags than the published
//!    limit is refused with the modelled `ServiceQuotaExceededException`, whose sentence
//!    states that limit and nothing of the request.
//!
//! The table spans the class each rule generalises over: for rule 1, every operation of the
//! surface that can answer a not-found and every kind of missing thing (the application, an
//! access scope, an authentication method, a grant), under a known application and under an
//! ARN nothing was created for; for rule 3, each of the three tag operations.
//!
//! `application_describe.rs`, `application_access_scopes.rs`,
//! `application_authentication_methods.rs`, `application_grants.rs` and
//! `tag_resource_resolution.rs` own the behaviour of those operations; this suite owns the
//! WORDING they all share. Every call goes through `MultiServiceRouter::dispatch`, and the
//! harness is the one `ssoadmin_permission_sets.rs` owns.

use serde_json::{json, Value};
use service_com_amazonaws_ssoadmin_swbexternalservice::state;

use crate::application_authentication_methods::{create_application, IAM};
use crate::ssoadmin_permission_sets::{call, error_code, router, seeded_instance_arn};

/// An `ApplicationArn` nothing was ever created under, shaped by the model's
/// `ApplicationArn` pattern
/// `^arn:aws(-[a-z]{1,5}){0,3}:sso::\d{12}:application/(sso)?ins-[a-zA-Z0-9-.]{16}/apl-[a-zA-Z0-9]{16}$`.
fn never_created_application_arn(instance_arn: &str) -> String {
    let instance_id = instance_arn
        .rsplit('/')
        .next()
        .unwrap_or_else(|| panic!("the instance ARN carries no id: {instance_arn}"));
    format!(
        "arn:aws:sso::{}:application/{instance_id}/apl-0123456789abcdef",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// A `Scope` the model's `Scope` shape admits (`length 1..255`), configured on no
/// application here.
const UNSET_SCOPE: &str = "sso:account:access";

/// The `GrantType` every row below asks for: one value of the model's `GrantType` enum
/// (`smithy_query(query="shape",shape="GrantType")`).
const UNSET_GRANT_TYPE: &str = "refresh_token";

/// The detail text of a refusal, whichever casing the envelope used for the member.
fn message(body: &Value) -> Option<String> {
    body["Message"]
        .as_str()
        .or_else(|| body["message"].as_str())
        .map(str::to_string)
}

/// RULE 1 — every not-found of the application surface is worded
/// `No <what> found for application arn: <arn>`.
///
/// The rows span the operations that can answer one and the kinds of missing thing: the
/// application (under an ARN nothing was created for) and each singleton child a caller
/// configures (under an application that exists and carries none).
#[tokio::test]
async fn every_application_not_found_names_what_was_missing_and_the_application() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;
    let application_arn = create_application(&router, &instance_arn, "refusal-wording").await;
    let missing_arn = never_created_application_arn(&instance_arn);

    // `(label, operation, request, the sentence the refusal must carry)`.
    let rows: Vec<(&str, &str, Value, String)> = vec![
        (
            "DescribeApplication of an ARN nothing was created for",
            "DescribeApplication",
            json!({"ApplicationArn": missing_arn}),
            format!("No resource found for application arn: {missing_arn}"),
        ),
        (
            "GetApplicationAssignmentConfiguration of a missing application",
            "GetApplicationAssignmentConfiguration",
            json!({"ApplicationArn": missing_arn}),
            format!("No resource found for application arn: {missing_arn}"),
        ),
        (
            "ListApplicationAccessScopes of a missing application",
            "ListApplicationAccessScopes",
            json!({"ApplicationArn": missing_arn}),
            format!("No resource found for application arn: {missing_arn}"),
        ),
        (
            "DeleteApplication of a missing application",
            "DeleteApplication",
            json!({"ApplicationArn": missing_arn}),
            format!("No resource found for application arn: {missing_arn}"),
        ),
        (
            "GetApplicationAccessScope for a scope the application does not carry",
            "GetApplicationAccessScope",
            json!({"ApplicationArn": application_arn, "Scope": UNSET_SCOPE}),
            format!("No access scope {UNSET_SCOPE} found for application arn: {application_arn}"),
        ),
        (
            "DeleteApplicationAccessScope for a scope the application does not carry",
            "DeleteApplicationAccessScope",
            json!({"ApplicationArn": application_arn, "Scope": UNSET_SCOPE}),
            format!("No access scope {UNSET_SCOPE} found for application arn: {application_arn}"),
        ),
        (
            "GetApplicationAuthenticationMethod for an unconfigured method type",
            "GetApplicationAuthenticationMethod",
            json!({"ApplicationArn": application_arn, "AuthenticationMethodType": IAM}),
            format!(
                "No authentication method {IAM} found for application arn: {application_arn}"
            ),
        ),
        (
            "DeleteApplicationAuthenticationMethod for an unconfigured method type",
            "DeleteApplicationAuthenticationMethod",
            json!({"ApplicationArn": application_arn, "AuthenticationMethodType": IAM}),
            format!(
                "No authentication method {IAM} found for application arn: {application_arn}"
            ),
        ),
        (
            "GetApplicationGrant for a grant type the application does not carry",
            "GetApplicationGrant",
            json!({"ApplicationArn": application_arn, "GrantType": UNSET_GRANT_TYPE}),
            format!(
                "No grant {UNSET_GRANT_TYPE} found for application arn: {application_arn}"
            ),
        ),
        (
            "DeleteApplicationGrant for a grant type the application does not carry",
            "DeleteApplicationGrant",
            json!({"ApplicationArn": application_arn, "GrantType": UNSET_GRANT_TYPE}),
            format!(
                "No grant {UNSET_GRANT_TYPE} found for application arn: {application_arn}"
            ),
        ),
    ];

    for (case, operation, request, expected) in rows {
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: {operation} must be refused, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body}"
        );
        assert_eq!(
            message(&body).as_deref(),
            Some(expected.as_str()),
            "case {case:?}: expected the message {expected:?}, got {body}"
        );
    }
}

/// RULE 2 — a principal that holds no direct assignment to an application that EXISTS is
/// refused with the error class alone: no detail text at all. The rows are the two
/// operations that resolve an assignment, and a principal type of each kind the model's
/// `PrincipalType` enum admits.
#[tokio::test]
async fn a_missing_assignment_on_a_known_application_carries_no_detail_text() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;
    let application_arn = create_application(&router, &instance_arn, "assignment-wording").await;

    // `PrincipalType` is `USER | GROUP`; a `PrincipalId` is 1..47 of `[a-zA-Z0-9-]`.
    for (principal_type, principal_id) in [
        ("USER", "11111111-2222-3333-4444-555555555555"),
        ("GROUP", "66666666-7777-8888-9999-aaaaaaaaaaaa"),
    ] {
        for operation in [
            "DescribeApplicationAssignment",
            "DeleteApplicationAssignment",
        ] {
            let case = format!("{operation} of an unassigned {principal_type}");
            let (status, body) = call(
                &router,
                operation,
                &json!({
                    "ApplicationArn": application_arn,
                    "PrincipalType": principal_type,
                    "PrincipalId": principal_id,
                }),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: must be refused, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {body}"
            );
            assert_eq!(
                message(&body),
                None,
                "case {case:?}: the refusal carries no detail text, got {body}"
            );
        }
    }
}

/// RULE 3 — the tagging surface's refusals: a WRITE names the resource kind the ARN
/// declares and the id asked for, the REPORT words a missing application as the
/// application's own reads do. The rows are the three tag operations, over an application
/// that was deleted and an ARN nothing was ever created for.
#[tokio::test]
async fn the_tagging_surface_names_the_resource_kind_on_a_write_and_the_application_on_a_read() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;
    let deleted_arn = create_application(&router, &instance_arn, "tag-wording").await;
    let (status, body) = call(
        &router,
        "DeleteApplication",
        &json!({"ApplicationArn": deleted_arn}),
    )
    .await;
    assert_eq!(status, 200, "DeleteApplication expected 200, got {status} {body}");
    let missing_arn = never_created_application_arn(&instance_arn);

    for resource_arn in [deleted_arn.as_str(), missing_arn.as_str()] {
        let write_expected = format!("Could not find Application with id {resource_arn}.");
        let report_expected = format!("No resource found for application arn: {resource_arn}");

        let rows: Vec<(&str, Value, &String)> = vec![
            (
                "TagResource",
                json!({
                    "ResourceArn": resource_arn,
                    "Tags": [{"Key": "Owner", "Value": "platform"}],
                }),
                &write_expected,
            ),
            (
                "UntagResource",
                json!({"ResourceArn": resource_arn, "TagKeys": ["Owner"]}),
                &write_expected,
            ),
            (
                "ListTagsForResource",
                json!({"ResourceArn": resource_arn}),
                &report_expected,
            ),
        ];

        for (operation, request, expected) in rows {
            let case = format!("{operation} on {resource_arn}");
            let (status, body) = call(&router, operation, &request).await;
            assert_eq!(
                status, 400,
                "case {case:?}: must be refused, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {body}"
            );
            assert_eq!(
                message(&body).as_deref(),
                Some(expected.as_str()),
                "case {case:?}: expected the message {expected:?}, got {body}"
            );
        }
    }
}

/// RULE 4 — a tagging write past the published per-resource tag limit is refused with the
/// modelled `ServiceQuotaExceededException`, whose sentence states the limit twice and
/// nothing of the request. The rows differ along how the limit is crossed: in one call, and
/// one tag at a time from a resource already at the limit.
#[tokio::test]
async fn a_write_past_the_published_tag_limit_states_the_limit_and_nothing_else() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;
    let limit = state::MAX_TAGS_PER_RESOURCE;
    // The sentence AWS answers this refusal with, stating the limit twice.
    let expected = format!(
        "{limit} user tags are allowed for a resource at maximum, The resultant tag set \
         must not have more than {limit} user tags."
    );

    let tags = |count: usize, prefix: &str| -> Value {
        json!((0..count)
            .map(|index| json!({"Key": format!("{prefix}-{index:03}"), "Value": "v"}))
            .collect::<Vec<Value>>())
    };

    // Row 1: one call that would store one tag more than the limit admits.
    let one_call_arn = create_application(&router, &instance_arn, "tag-limit-one-call").await;
    // Row 2: the limit reached first, then one more tag in a second call.
    let two_call_arn = create_application(&router, &instance_arn, "tag-limit-two-calls").await;
    let (status, body) = call(
        &router,
        "TagResource",
        &json!({"ResourceArn": two_call_arn, "Tags": tags(limit, "held")}),
    )
    .await;
    assert_eq!(
        status, 200,
        "a tag set at the limit must answer 200, got {status} {body}"
    );

    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "one call past the limit",
            one_call_arn.as_str(),
            tags(limit + 1, "once"),
        ),
        (
            "one tag past a resource already at the limit",
            two_call_arn.as_str(),
            tags(1, "extra"),
        ),
    ];

    for (case, resource_arn, requested) in rows {
        let (status, body) = call(
            &router,
            "TagResource",
            &json!({"ResourceArn": resource_arn, "Tags": requested}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: the write must be refused, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ServiceQuotaExceededException",
            "case {case:?}: expected ServiceQuotaExceededException, got {body}"
        );
        assert_eq!(
            message(&body).as_deref(),
            Some(expected.as_str()),
            "case {case:?}: expected the message {expected:?}, got {body}"
        );
    }

    // A refused write stored nothing: the resource still carries exactly what it held.
    let (status, listed) = call(
        &router,
        "ListTagsForResource",
        &json!({"ResourceArn": two_call_arn}),
    )
    .await;
    assert_eq!(status, 200, "ListTagsForResource expected 200, got {status} {listed}");
    assert_eq!(
        listed["Tags"].as_array().map(Vec::len),
        Some(limit),
        "a refused write must leave the stored set at the limit, got {listed}"
    );
    let (status, listed) = call(
        &router,
        "ListTagsForResource",
        &json!({"ResourceArn": one_call_arn}),
    )
    .await;
    assert_eq!(status, 200, "ListTagsForResource expected 200, got {status} {listed}");
    assert_eq!(
        listed["Tags"].as_array().map(Vec::len),
        Some(0),
        "a refused write must store nothing at all, got {listed}"
    );
}
