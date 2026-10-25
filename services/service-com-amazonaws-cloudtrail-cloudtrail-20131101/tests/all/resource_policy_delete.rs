// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteResourcePolicy`.
//!
//! Every case goes over the wire through the shared [`super::emulator::Emulator`], so
//! what is asserted is what a client sees — the HTTP status, the error code and the
//! body.
//!
//! The rules under test, each grounded in this service's Smithy model:
//!
//! * the detach removes the policy from the resource its `ResourceArn` names and
//!   leaves the resource itself in place, so a later read reports that the resource
//!   carries none and a later put attaches a new one;
//! * a resource that exists and carries no policy has none to detach, which is
//!   `ResourcePolicyNotFoundException` (HTTP 404);
//! * a `ResourceArn` is refused by the case it is, the way every resource-policy
//!   operation refuses it: not a CloudTrail resource ARN
//!   (`ResourceARNNotValidException`), a resource type that carries no
//!   resource-based policy (`ResourceTypeNotSupportedException`), or a supported
//!   type naming nothing stored (`ResourceNotFoundException`), all HTTP 400;
//! * detaching a policy WRITES to the resource, so a resource CloudTrail is still
//!   loading refuses it with `ConflictException` (HTTP 409) until the clock settles
//!   it;
//! * a `ResourceArn` outside the modelled `@length`/`@pattern` is refused before the
//!   handler runs.
//!
//! The resources under test come from the shared seam in `super::resource_policy`: a
//! dashboard created over the wire, and a channel and an event data store seeded in
//! the registries the handlers read (`CreateChannel` and `CreateEventDataStore` are
//! still scaffold stubs with issues of their own).

use serde_json::{json, Value};
use std::thread::sleep;

use service_com_amazonaws_cloudtrail_cloudtrail_20131101::state::{self, ResourceKind};

use super::emulator::{Emulator, Reply, HOME_REGION};
use super::resource_policy::{
    addressed_resources, attach_each, channel, dashboard, document, event_data_store, expect_policy,
    get, put, OTHER_REGION, SHARED_ID,
};

/// A real-looking account id a client may quote in an ARN, which is NOT the account
/// the emulator mints ARNs for.
const OTHER_ACCOUNT: &str = "123456789012";

/// Detach the policy of `resource_arn`.
async fn delete(emulator: &Emulator, resource_arn: &str) -> Reply {
    emulator
        .call("DeleteResourcePolicy", &json!({"ResourceArn": resource_arn}))
        .await
}

/// The ARN of the CloudTrail resource of type `resource_type` called `id` in
/// `region`, in the account the emulator mints ARNs for.
fn arn(region: &str, resource_type: &str, id: &str) -> String {
    format!(
        "arn:aws:cloudtrail:{region}:{}:{resource_type}/{id}",
        engine::emulator_account_id()
    )
}

/// The policy the service holds for `resource_arn`, read straight from the registry
/// the handlers write, so the assertion is about stored state and not only about the
/// answer a read gives.
fn stored_policy(emulator: &Emulator, resource_arn: &str) -> Option<String> {
    emulator
        .state()
        .lock()
        .resource_policies
        .get(resource_arn)
        .cloned()
}

/// RULE: the detach removes the policy from the resource the ARN names and leaves the
/// resource in place — so a read then reports none, a second detach has none to take,
/// and a new policy attaches to the same resource.
///
/// The rows span the resource TYPES a policy attaches to and the ARN's Region (the
/// shared table in `super::resource_policy`), with every row carrying its own
/// document, so a detach that took the wrong resource's policy is visible.
#[tokio::test]
async fn detaching_a_policy_leaves_the_resource_carrying_none() {
    let emulator = Emulator::new();
    let resources = addressed_resources(&emulator).await;
    attach_each(&emulator, &resources).await;

    for (index, resource) in resources.iter().enumerate() {
        let case = resource.case;
        assert_eq!(
            stored_policy(&emulator, &resource.arn),
            Some(resource.policy.clone()),
            "case {case:?}: the attached policy must be stored before the detach"
        );

        let detached = delete(&emulator, &resource.arn).await;
        assert_eq!(
            detached.expect_ok(case),
            &json!({}),
            "case {case:?}: DeleteResourcePolicy models an empty response, got {detached:?}"
        );
        assert_eq!(
            stored_policy(&emulator, &resource.arn),
            None,
            "case {case:?}: the detached policy must be gone from the registry"
        );
        get(&emulator, &resource.arn).await.expect_error(
            &format!("{case}: after the detach"),
            404,
            "ResourcePolicyNotFoundException",
        );

        // Nothing is left to detach, so the second call is the same 404 a resource
        // that never carried a policy answers.
        delete(&emulator, &resource.arn).await.expect_error(
            &format!("{case}: detached twice"),
            404,
            "ResourcePolicyNotFoundException",
        );

        // Every row NOT yet detached still carries exactly its own document: the
        // detach reached one resource.
        for later in &resources[index + 1..] {
            expect_policy(
                &format!("{} after {case:?} was detached", later.case),
                &get(&emulator, &later.arn).await,
                &later.arn,
                &later.policy,
            );
        }

        // The resource survived its policy, so a new one attaches to it.
        let reattached = document("attached-after-the-detach");
        expect_policy(
            &format!("{case}: a new policy after the detach"),
            &put(&emulator, &resource.arn, &reattached).await,
            &resource.arn,
            &reattached,
        );
        expect_policy(
            &format!("{case}: reading the new policy"),
            &get(&emulator, &resource.arn).await,
            &resource.arn,
            &reattached,
        );
    }
}

/// RULE: a resource that exists and carries no resource-based policy has none to
/// detach, which is the one HTTP 404 this surface models — and the failed detach
/// changes nothing, so the resource is still there to attach one to.
#[tokio::test]
async fn a_resource_carrying_no_policy_has_none_to_detach() {
    let emulator = Emulator::new();
    // One resource per kind, none of them carrying a policy.
    let rows = vec![
        (
            "a dashboard",
            dashboard(&emulator, "no-policy-dash", HOME_REGION).await,
        ),
        (
            "a channel",
            channel(&emulator, "no-policy-channel", HOME_REGION),
        ),
        (
            "an event data store",
            event_data_store(&emulator, "no-policy-store", OTHER_REGION),
        ),
    ];

    for (case, resource_arn) in &rows {
        delete(&emulator, resource_arn)
            .await
            .expect_error(case, 404, "ResourcePolicyNotFoundException");
        assert_eq!(
            state::resource_policy_not_found_message(resource_arn),
            format!("No resource policy is attached to {resource_arn}"),
            "case {case:?}: the refusal names the resource it found no policy for"
        );

        let attached = document(case);
        expect_policy(
            &format!("{case}: attaches after the refused detach"),
            &put(&emulator, resource_arn, &attached).await,
            resource_arn,
            &attached,
        );
    }
}

/// RULE: a `ResourceArn` is refused by the case it is, and the refusal is built from
/// the request — the value that is no resource ARN, or the kind of resource that was
/// not found, as this surface words that kind.
///
/// The rows span every way an ARN can fail to name a resource carrying a policy, over
/// each resource type, and are measured against a resource that DOES exist.
#[tokio::test]
async fn an_arn_naming_no_policy_carrying_resource_is_refused_by_its_case() {
    let emulator = Emulator::new();
    let existing = dashboard(&emulator, "detach-existing", HOME_REGION).await;
    let document = document("the-resource-that-exists");
    put(&emulator, &existing, &document)
        .await
        .expect_ok("attach to the resource the rows are measured against");

    // (case, ResourceArn, expected HTTP status, expected error code)
    let rows: Vec<(&str, String, u16, &str)> = vec![
        (
            "a bare resource name",
            "detach-existing".to_string(),
            400,
            "ResourceARNNotValidException",
        ),
        (
            "an ARN of another service",
            format!("arn:aws:s3:{HOME_REGION}:{OTHER_ACCOUNT}:bucket/logs"),
            400,
            "ResourceARNNotValidException",
        ),
        (
            "an ARN with no resource field",
            format!("arn:aws:cloudtrail:{HOME_REGION}:{OTHER_ACCOUNT}"),
            400,
            "ResourceARNNotValidException",
        ),
        (
            "an ARN with an empty Region",
            format!("arn:aws:cloudtrail::{OTHER_ACCOUNT}:dashboard/detach-existing"),
            400,
            "ResourceARNNotValidException",
        ),
        (
            "an ARN with an empty account",
            format!("arn:aws:cloudtrail:{HOME_REGION}::dashboard/detach-existing"),
            400,
            "ResourceARNNotValidException",
        ),
        (
            "an ARN naming no CloudTrail resource type",
            arn(HOME_REGION, "query", "0d7b0b5a-2e2a-4f57-9f7f-1f5a6f8f0c11"),
            400,
            "ResourceARNNotValidException",
        ),
        (
            "an ARN whose resource id is empty",
            arn(HOME_REGION, "channel", ""),
            400,
            "ResourceARNNotValidException",
        ),
        (
            "a trail ARN: a CloudTrail resource that carries no resource-based policy",
            arn(HOME_REGION, "trail", "org-trail"),
            400,
            "ResourceTypeNotSupportedException",
        ),
        (
            "a dashboard that was never created",
            arn(HOME_REGION, "dashboard", "no-such-dash"),
            400,
            "ResourceNotFoundException",
        ),
        (
            "a dashboard that exists in another Region",
            arn(OTHER_REGION, "dashboard", "detach-existing"),
            400,
            "ResourceNotFoundException",
        ),
        (
            "a channel that was never created",
            arn(HOME_REGION, "channel", "01234567890"),
            400,
            "ResourceNotFoundException",
        ),
        (
            "an event data store that was never created",
            arn(
                HOME_REGION,
                "eventdatastore",
                "EXAMPLE-f852-4e8f-8bd1-bcf6cEXAMPLE",
            ),
            400,
            "ResourceNotFoundException",
        ),
    ];

    for (case, resource_arn, status, code) in &rows {
        let answer = delete(&emulator, resource_arn).await;
        answer.expect_error(case, *status, code);

        // The sentences are written out HERE rather than read back from the handler's
        // own helper, so a change to either side is a failure: a value that is no
        // CloudTrail resource ARN is named back, an unsupported type names the type
        // the ARN carried, a dashboard that was not found names no identifier and
        // every other kind names the ARN the request supplied.
        let expected = match *code {
            "ResourceARNNotValidException" => {
                format!("The format of the resource ARN {resource_arn} is not valid.")
            }
            "ResourceTypeNotSupportedException" => {
                "Resource policies are not supported for the resource: trail.".to_string()
            }
            _ => {
                let kind = ResourceKind::ALL
                    .into_iter()
                    .find(|kind| resource_arn.contains(&format!(":{}", kind.resource_prefix())))
                    .unwrap_or_else(|| panic!("case {case:?}: {resource_arn} names no type"));
                match kind {
                    ResourceKind::Dashboard => "The dashboard was not found.".to_string(),
                    ResourceKind::EventDataStore => {
                        format!("The event data store {resource_arn} was not found.")
                    }
                    ResourceKind::Channel => format!("The channel {resource_arn} was not found."),
                    ResourceKind::Trail => panic!(
                        "case {case:?}: a trail ARN is refused as an unsupported resource type"
                    ),
                }
            }
        };
        assert_eq!(
            answer.message(),
            expected,
            "case {case:?}: expected {expected:?}, got {answer:?}"
        );
    }

    // Every row above was refused before any policy was touched: the resource that
    // does exist still carries the document attached at the top.
    expect_policy(
        "the resource that does exist",
        &get(&emulator, &existing).await,
        &existing,
        &document,
    );
}

/// RULE: detaching a policy WRITES to the resource, so a resource CloudTrail has not
/// finished loading refuses it with `ConflictException` until the clock settles the
/// resource — and the refusal is decided before the policy is looked up, so a
/// resource still loading answers the conflict rather than a 404.
#[tokio::test]
async fn detaching_from_a_resource_that_is_still_loading_is_refused() {
    let emulator = Emulator::new();
    let created = emulator
        .call("CreateDashboard", &json!({"Name": "still-loading"}))
        .await;
    let resource_arn = created
        .expect_ok("create a dashboard to detach from")
        .get("DashboardArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("CreateDashboard reported no ARN: {created:?}"))
        .to_string();

    delete(&emulator, &resource_arn).await.expect_error(
        "a dashboard still loading",
        409,
        "ConflictException",
    );

    // Once the load the clock owes is due, the same resource takes a policy and gives
    // it up again.
    sleep(state::DASHBOARD_SETTLE_AFTER + std::time::Duration::from_millis(250));
    let attached = document("detached-after-the-load");
    expect_policy(
        "the settled dashboard",
        &put(&emulator, &resource_arn, &attached).await,
        &resource_arn,
        &attached,
    );
    delete(&emulator, &resource_arn)
        .await
        .expect_ok("detach from the settled dashboard");
    assert_eq!(
        stored_policy(&emulator, &resource_arn),
        None,
        "case {:?}: the settled dashboard must carry no policy after the detach",
        "the settled dashboard"
    );
}

/// RULE: a `ResourceArn` outside the modelled constraints is refused by the request
/// walk, before the handler runs — `ResourceArn` is `@length(min: 3, max: 256)` with
/// `@pattern("^[a-zA-Z0-9._/\\-:]+$")`.
///
/// The rows walk each published bound and one value past it, plus a character the
/// pattern does not admit.
#[tokio::test]
async fn an_arn_outside_the_modelled_constraints_is_a_validation_error() {
    let emulator = Emulator::new();
    let channel_arn = channel(&emulator, SHARED_ID, HOME_REGION);

    let refused = [
        ("one character short of the minimum length", "ab".to_string()),
        (
            "one character past the maximum length",
            "c".repeat(257),
        ),
        ("a space, outside the pattern", format!("{channel_arn} ")),
        ("a star, outside the pattern", format!("{channel_arn}*")),
    ];
    for (case, resource_arn) in &refused {
        delete(&emulator, resource_arn)
            .await
            .expect_error(case, 400, "ValidationException");
    }

    // Each published bound itself is INSIDE the model, so it reaches the handler and
    // is answered by this service's own rules instead.
    let admitted = [
        ("the shortest value the model admits", "abc".to_string()),
        ("the longest value the model admits", "c".repeat(256)),
    ];
    for (case, resource_arn) in &admitted {
        delete(&emulator, resource_arn).await.expect_error(
            case,
            400,
            "ResourceARNNotValidException",
        );
    }
}
