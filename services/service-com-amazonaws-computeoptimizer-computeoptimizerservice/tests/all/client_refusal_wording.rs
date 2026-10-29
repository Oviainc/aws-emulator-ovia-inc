// SPDX-License-Identifier: CC0-1.0

//! The wording of this service's client refusals, end to end over the wire.
//!
//! Every refusal of a request this API will not accept carries the service's OWN constant
//! sentence for that refusal class: one text for every request in the class, naming the input
//! member or the rejected value only where that sentence itself does. So no refusal reflects
//! caller-supplied input back into the error body, and none of them varies with the resource
//! type, the Region, the in-flight job or the value that arrived.
//!
//! Each suite below drives one class over a TABLE of inputs that differ along what the class
//! generalises over — the rejected value's form, the operation it arrives at, the scope name,
//! the exported resource type — and asserts the SAME sentence every time. The sentences
//! themselves are the crate's constants, so a drift between what one operation answers and
//! what another does fails here.
//!
//! Every call goes through the shared wire harness (`tests/all/harness.rs`), the same
//! `MultiServiceRouter::dispatch` the emulator's front door uses, so what is asserted is what
//! a client sees.

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::state::export::EXPORT_JOB_LIMIT_MESSAGE;
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::state::{
    AUTO_SCALING_GROUP_SCOPE_MESSAGE, INVALID_ENROLLMENT_STATUS_MESSAGE,
    INVALID_RESOURCE_ARN_MESSAGE, ORGANIZATION_SCOPE_VALUE, RESOURCE_TYPE_AUTO_SCALING_GROUP,
    RESOURCE_TYPE_EBS_VOLUME, RESOURCE_TYPE_EC2_INSTANCE, RESOURCE_TYPE_LAMBDA_FUNCTION,
    SCOPE_ACCOUNT_ID, SCOPE_ORGANIZATION, SCOPE_RESOURCE_ARN,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    auto_scaling_group_arn, call, create_bucket, emulator, error_code, instance_arn, opt_in,
    ACCOUNT,
};

/// The generic wording this service must never answer: a refusal built from a member name and
/// the value the request carried. Asserted absent on every row below, so a refusal that falls
/// back to a parameter template fails whatever its remaining text says.
const GENERIC_PARAMETER_PREFIX: &str = "The value supplied for parameter";

/// Assert `request` is refused `400 InvalidParameterValueException` carrying exactly
/// `expected`, and that the body neither falls back to the generic parameter template nor
/// echoes any of `absent` — the values the request supplied, which the service's sentence does
/// not contain.
async fn assert_refusal(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    request: Value,
    expected: &str,
    absent: &[&str],
) {
    let (status, body) = call(router, operation, request).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected the request to be refused with 400, got {status}: {body}"
    );
    assert!(
        error_code(&body).contains("InvalidParameterValueException"),
        "case {case:?}: expected InvalidParameterValueException, got {body}"
    );
    let message = body["message"].as_str().unwrap_or_default();
    assert_eq!(
        message, expected,
        "case {case:?}: expected the service's sentence {expected:?}, got {message:?}"
    );
    assert!(
        !message.contains(GENERIC_PARAMETER_PREFIX),
        "case {case:?}: the refusal must not be built from a parameter template, got {message:?}"
    );
    for echoed in absent {
        assert!(
            echoed.is_empty() || !message.contains(echoed),
            "case {case:?}: the refusal must not echo the request value {echoed:?}, got \
             {message:?}"
        );
    }
}

/// The enrollment statuses a client cannot set — "The Pending and Failed options cannot be
/// used to update the enrollment status of an account" (`UpdateEnrollmentStatus$status`) — are
/// each refused with the service's one status sentence, whichever of them arrived and whatever
/// else the request carried, and the stored enrollment is left exactly as it was.
#[tokio::test]
async fn a_status_a_client_cannot_set_is_refused_with_the_services_status_sentence() {
    let router = emulator();
    opt_in(&router).await;

    // The two output-only members of the model's `Status` enum (Active | Inactive | Pending |
    // Failed — `smithy_query(query="shape",shape="Status")`), each crossed with
    // `includeMemberAccounts`, so the rows differ in the rejected value AND in the rest of
    // the request.
    for status in ["Pending", "Failed"] {
        for include_member_accounts in [false, true] {
            let case = format!("status={status} includeMemberAccounts={include_member_accounts}");
            assert_refusal(
                &router,
                &case,
                "UpdateEnrollmentStatus",
                json!({"status": status, "includeMemberAccounts": include_member_accounts}),
                INVALID_ENROLLMENT_STATUS_MESSAGE,
                &[status],
            )
            .await;

            // The refusal is about the request, so it wrote nothing: the enrollment the
            // opt-in stored is still in force.
            let (read_status, enrollment) = call(&router, "GetEnrollmentStatus", json!({})).await;
            assert_eq!(
                read_status, 200,
                "case {case:?}: reading the enrollment back: got {read_status}: {enrollment}"
            );
            assert_eq!(
                enrollment["status"], "Active",
                "case {case:?}: a refused update leaves the stored status as it was, got \
                 {enrollment}"
            );
        }
    }
}

/// An `Active` or `Inactive` status is accepted, so the suite above pins the WORDING of a
/// refusal rather than a blanket rejection.
#[tokio::test]
async fn a_status_a_client_can_set_is_still_accepted() {
    let router = emulator();

    for status in ["Active", "Inactive"] {
        let (code, body) = call(
            &router,
            "UpdateEnrollmentStatus",
            json!({"status": status, "includeMemberAccounts": false}),
        )
        .await;
        assert_eq!(
            code, 200,
            "case {status:?}: a settable status is accepted, got {code}: {body}"
        );
        assert_eq!(
            body["status"], status,
            "case {status:?}: the response carries the stored status, got {body}"
        );
    }
}

/// Every string the preference API cannot resolve a resource from — "Only EC2 instance and
/// Auto Scaling group ARNs are currently supported" (`Scope$value`,
/// `GetEffectiveRecommendationPreferences$resourceArn`) — is refused with the service's one
/// ARN sentence, on every operation that takes such an ARN, and the rejected value is never
/// echoed.
#[tokio::test]
async fn an_arn_the_preference_api_cannot_resolve_is_refused_with_the_services_arn_sentence() {
    let router = emulator();
    opt_in(&router).await;

    // The forms an unusable ARN takes: not an ARN at all, an empty value, a truncated one, and
    // well-formed ARNs of services and resource kinds this API does not accept. `ResourceArn`
    // carries no length constraint (`smithy_query(query="shape",shape="ResourceArn")`), so the
    // empty value reaches the same validation as the rest.
    let rejected: Vec<String> = vec![
        "not-an-arn".to_string(),
        String::new(),
        "arn:aws".to_string(),
        "arn:aws:ec2:us-east-1".to_string(),
        format!("arn:aws:ec2:us-east-1:{ACCOUNT}:volume/vol-0123456789abcdef0"),
        format!("arn:aws:s3:::compute-optimizer-{ACCOUNT}"),
        format!("arn:aws:autoscaling:us-east-1:{ACCOUNT}:scalingPolicy:policy/scale-out"),
        format!("arn:aws:lambda:us-east-1:{ACCOUNT}:function:summarise"),
    ];

    for value in &rejected {
        // One unusable ARN, on each operation that takes one: the reader that resolves a
        // resource's effective preferences, and the three operations that take a
        // `ResourceArn` scope.
        let requests = [
            (
                "GetEffectiveRecommendationPreferences",
                json!({"resourceArn": value}),
            ),
            (
                "PutRecommendationPreferences",
                json!({
                    "resourceType": RESOURCE_TYPE_EC2_INSTANCE,
                    "scope": {"name": SCOPE_RESOURCE_ARN, "value": value},
                    "lookBackPeriod": "DAYS_32",
                }),
            ),
            (
                "GetRecommendationPreferences",
                json!({
                    "resourceType": RESOURCE_TYPE_EC2_INSTANCE,
                    "scope": {"name": SCOPE_RESOURCE_ARN, "value": value},
                }),
            ),
            (
                "DeleteRecommendationPreferences",
                json!({
                    "resourceType": RESOURCE_TYPE_EC2_INSTANCE,
                    "scope": {"name": SCOPE_RESOURCE_ARN, "value": value},
                    "recommendationPreferenceNames": ["LookBackPeriodPreference"],
                }),
            ),
        ];
        for (operation, request) in requests {
            let case = format!("{operation} with {value:?}");
            assert_refusal(
                &router,
                &case,
                operation,
                request,
                INVALID_RESOURCE_ARN_MESSAGE,
                &[value.as_str()],
            )
            .await;
        }
    }

    // The ARNs this API does accept still are, so the rows above pin the wording of a refusal
    // rather than a reader that refuses everything.
    for arn in [
        instance_arn("i-0123456789abcdef0"),
        auto_scaling_group_arn("batch-tier"),
    ] {
        let (status, body) = call(
            &router,
            "GetEffectiveRecommendationPreferences",
            json!({"resourceArn": arn}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {arn:?}: an accepted ARN form is answered, got {status}: {body}"
        );
    }
}

/// An Auto Scaling group preference asked for anywhere but the resource level — "You cannot
/// create recommendation preferences for Auto Scaling groups at the organization and account
/// levels" (`PutRecommendationPreferences$scope`) — is refused with the service's one sentence
/// for the condition, whichever scope the request named and whichever preference it carried,
/// and nothing is stored.
#[tokio::test]
async fn an_auto_scaling_group_preference_off_the_resource_scope_is_refused_with_one_sentence() {
    let router = emulator();
    opt_in(&router).await;

    // The scopes that are not the resource level: the two the model's `ScopeName` enum names
    // besides `ResourceArn` (`smithy_query(query="shape",shape="ScopeName")`), each with the
    // value that scope requires, plus a request that names no scope at all — which is the
    // account level too.
    let scopes = [
        (
            SCOPE_ORGANIZATION,
            Some(json!({"name": SCOPE_ORGANIZATION, "value": ORGANIZATION_SCOPE_VALUE})),
        ),
        (
            SCOPE_ACCOUNT_ID,
            Some(json!({"name": SCOPE_ACCOUNT_ID, "value": ACCOUNT})),
        ),
        ("no scope", None),
    ];
    // Rows differ in the preference the request carries too, so the refusal cannot depend on
    // which member was being set.
    let preferences = [
        ("lookBackPeriod", json!("DAYS_93")),
        ("enhancedInfrastructureMetrics", json!("Active")),
        ("inferredWorkloadTypes", json!("Inactive")),
    ];

    for (scope_case, scope) in &scopes {
        for (member, value) in &preferences {
            let case = format!("{member} on an AutoScalingGroup at {scope_case}");
            let mut request = json!({"resourceType": RESOURCE_TYPE_AUTO_SCALING_GROUP});
            if let Some(scope) = scope {
                request["scope"] = scope.clone();
            }
            request[*member] = value.clone();
            assert_refusal(
                &router,
                &case,
                "PutRecommendationPreferences",
                request,
                AUTO_SCALING_GROUP_SCOPE_MESSAGE,
                &[RESOURCE_TYPE_AUTO_SCALING_GROUP, member],
            )
            .await;

            // A refused request stores nothing: no record of this resource type is held at
            // any scope.
            let (status, read) = call(
                &router,
                "GetRecommendationPreferences",
                json!({"resourceType": RESOURCE_TYPE_AUTO_SCALING_GROUP}),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: reading the preferences back: got {status}: {read}"
            );
            assert_eq!(
                read["recommendationPreferencesDetails"],
                json!([]),
                "case {case:?}: a refused request must store nothing, got {read}"
            );
        }
    }

    // The resource-level scope the sentence points the caller at is accepted, so the rows
    // above pin a wording rather than a writer that refuses every Auto Scaling group.
    let (status, body) = call(
        &router,
        "PutRecommendationPreferences",
        json!({
            "resourceType": RESOURCE_TYPE_AUTO_SCALING_GROUP,
            "scope": {
                "name": SCOPE_RESOURCE_ARN,
                "value": auto_scaling_group_arn("batch-tier"),
            },
            "lookBackPeriod": "DAYS_93",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the resource-level scope is accepted, got {status}: {body}"
    );
}

/// The destination bucket the export rows below write to.
const BUCKET: &str = "refusal-wording-exports";

/// A second export of a resource type whose job is still in flight is refused with the
/// service's one export-limit sentence: the SAME text on every export surface, carrying
/// neither the resource type nor the id of the job that still holds the slot.
#[tokio::test]
async fn a_concurrent_export_is_refused_with_the_services_one_limit_sentence() {
    let router = emulator();
    create_bucket(&router, BUCKET).await;
    opt_in(&router).await;

    // Four surfaces spanning resource types, whose own documentation words the limit four
    // different ways, so the rows would differ if the refusal were worded per operation.
    let surfaces = [
        (
            "ExportEC2InstanceRecommendations",
            RESOURCE_TYPE_EC2_INSTANCE,
        ),
        (
            "ExportAutoScalingGroupRecommendations",
            RESOURCE_TYPE_AUTO_SCALING_GROUP,
        ),
        ("ExportEBSVolumeRecommendations", RESOURCE_TYPE_EBS_VOLUME),
        (
            "ExportLambdaFunctionRecommendations",
            RESOURCE_TYPE_LAMBDA_FUNCTION,
        ),
    ];

    let mut messages: Vec<String> = Vec::new();
    for (operation, resource_type) in surfaces {
        let case = format!("{operation} ({resource_type})");
        let request = json!({"s3DestinationConfig": {"bucket": BUCKET}});
        let (status, body) = call(&router, operation, request.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the first export of this resource type is admitted, got \
             {status}: {body}"
        );
        let job_id = body["jobId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: an admitted export answers a jobId, got {body}"))
            .to_string();

        let (status, body) = call(&router, operation, request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a second export while one is in flight is refused, got \
             {status}: {body}"
        );
        assert!(
            error_code(&body).contains("LimitExceededException"),
            "case {case:?}: expected LimitExceededException, got {body}"
        );
        let message = body["message"].as_str().unwrap_or_default().to_string();
        assert_eq!(
            message, EXPORT_JOB_LIMIT_MESSAGE,
            "case {case:?}: expected the service's sentence {EXPORT_JOB_LIMIT_MESSAGE:?}, got \
             {message:?}"
        );
        assert!(
            !message.contains(&job_id) && !message.contains(resource_type),
            "case {case:?}: the refusal carries neither the job id nor the resource type, got \
             {message:?}"
        );
        messages.push(message);
    }

    // One sentence for every surface: the refusals are byte-identical.
    assert!(
        messages.windows(2).all(|pair| pair[0] == pair[1]),
        "every export surface answers ONE limit sentence, got {messages:?}"
    );
}
