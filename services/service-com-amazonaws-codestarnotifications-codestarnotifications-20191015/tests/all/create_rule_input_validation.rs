// SPDX-License-Identifier: CC0-1.0

//! The two input rules the MODEL cannot express but the SERVICE enforces, over the wire.
//!
//! * `Resource` must name a resource of a type CodeStar Notifications supports. The
//!   modelled pattern (`NotificationRuleResource`:
//!   `^arn:aws[^:\s]*:[^:\s]*:[^:\s]*:[0-9]{12}:[^\s]+$`) accepts ANY service's ARN, so
//!   the type restriction is the service's: an ARN of any other service is refused with
//!   400 `ValidationException` over the service's own FIXED text, which names the four
//!   resource types that are valid and interpolates nothing.
//! * every `EventTypeIds` entry must be an event type the service publishes — the same
//!   catalog `ListEventTypes` reports — so a rule can never be wired to an event that
//!   does not exist. An unresolvable id is refused with 400 `ValidationException`.
//!
//! Both rules hold on the CREATE path and, for the member it carries, on the UPDATE path;
//! and a refusal stores nothing, which each suite checks by listing the rules the
//! resource would have.
//!
//! Each suite asserts its rule over a TABLE whose rows differ in what the rule
//! generalises over — the service the ARN names, where the unknown id sits in the list
//! and what form it takes — rather than pinning one recorded request.
//!
//! The harness (`Emulator`, `error_code`, `message`, `string`, …) is the one
//! `notification_rule_lifecycle` already builds: one shared engine behind the same
//! `MultiServiceRouter` dispatch the emulator serves production requests with.

use serde_json::{json, Value};
use service_com_amazonaws_codestarnotifications_codestarnotifications_20191015 as codestar;

use crate::notification_rule_lifecycle::{error_code, message, string, Emulator};

/// The service's fixed refusal for a `Resource` of an unsupported type: 100% fixed text,
/// with no part of the request in it.
const UNSUPPORTED_RESOURCE: &str = "The resource ARN is not valid. The ARN must be a \
                                    supported AWS CodeStar Notifications resource type. \
                                    Valid resource types include AWS CodeBuild, AWS \
                                    CodeCommit, AWS CodeDeploy and AWS CodePipeline \
                                    resources.";

/// A catalogued event type id, for a request whose `Resource` is what is under test.
const KNOWN_EVENT_TYPE: &str = "codecommit-repository-comments-on-commits";

/// The service's refusal for an event type id it does not publish: fixed text either side
/// of ONE slot, the id that did not resolve.
fn unresolvable_event_type(id: &str) -> String {
    format!(
        "The notification rule cannot be created or modified because the following \
         notification event type ID is not valid: {id}. For more information, see the \
         AWS CodeStar Notifications API Reference."
    )
}

/// A body that differs from the well-formed one in exactly one place.
fn body(name: &str, resource: &str, event_type_ids: &[&str]) -> Value {
    json!({
        "Name": name,
        "Resource": resource,
        "DetailType": "BASIC",
        "EventTypeIds": event_type_ids,
        "Targets": [],
    })
}

/// How many rules the account holds for `resource`, through the RESOURCE filter.
async fn rules_for(emulator: &Emulator, resource: &str) -> usize {
    let listed = emulator
        .ok(
            resource,
            "/listNotificationRules",
            &json!({ "Filters": [{ "Name": "RESOURCE", "Value": resource }] }),
        )
        .await;
    listed
        .get("NotificationRules")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or_default()
}

/// RULE: a `Resource` whose ARN names a service CodeStar Notifications does not create
/// notification rules for is refused with 400 `ValidationException` and the service's
/// fixed text, and no rule is created for it.
///
/// The rows differ in the service the ARN names and in the shape of its resource segment
/// (slash-separated, colon-separated, bare), including services a caller might plausibly
/// confuse for a supported one (the SNS topic a rule's TARGET is) — none of which the
/// refusal's text mentions.
#[tokio::test]
async fn a_resource_of_an_unsupported_type_is_refused_with_the_services_fixed_text() {
    // Each is a well-formed `NotificationRuleResource` (the modelled pattern passes), so
    // only the type restriction can refuse it.
    let rows: &[(&str, &str)] = &[
        (
            "an SNS topic (what a rule's target is, not its resource)",
            "arn:aws:sns:us-east-1:123456789012:a-topic",
        ),
        (
            "a chat configuration",
            "arn:aws:chatbot:us-east-1:123456789012:chat-configuration/slack-channel/eng",
        ),
        ("an S3 bucket", "arn:aws:s3:us-east-1:123456789012:my-bucket"),
        (
            "a Lambda function",
            "arn:aws:lambda:us-east-1:123456789012:function:handler",
        ),
        (
            "an SQS queue",
            "arn:aws:sqs:us-east-1:123456789012:a-queue",
        ),
        (
            "an IAM role",
            "arn:aws:iam:us-east-1:123456789012:role/some-role",
        ),
        (
            "a CodeStar Notifications rule (not a resource a rule watches)",
            "arn:aws:codestar-notifications:us-east-1:123456789012:notificationrule/abc",
        ),
        (
            "a service whose name merely starts like a supported one",
            "arn:aws:codeartifact:us-east-1:123456789012:repository/pkgs",
        ),
    ];

    let emulator = Emulator::new();

    for (index, (case, resource)) in rows.iter().enumerate() {
        let request = body(&format!("unsupported-{index}"), resource, &[KNOWN_EVENT_TYPE]);
        let (status, answered) = emulator.post("/createNotificationRule", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {answered}"
        );
        assert_eq!(
            error_code(&answered),
            "ValidationException",
            "case {case:?}: got {answered}"
        );
        let stated = message(&answered);
        assert_eq!(
            stated, UNSUPPORTED_RESOURCE,
            "case {case:?}: expected the service's fixed text, got {stated:?}"
        );
        assert!(
            !stated.contains(resource),
            "case {case:?}: the submitted ARN must not be interpolated, got {stated:?}"
        );
        assert_eq!(
            rules_for(&emulator, resource).await,
            0,
            "case {case:?}: a refused create must store nothing"
        );
    }
}

/// RULE (the other side of it): a `Resource` of each type the service DOES support is
/// accepted and read back, so the refusal above is the type restriction and not a blanket
/// one.
///
/// One row per service the refusal's text names, each with that service's own resource
/// form.
#[tokio::test]
async fn a_resource_of_every_supported_type_is_accepted_and_read_back() {
    // The four services the refusal names, as an ARN spells them.
    let rows: &[(&str, &str)] = &[
        (
            "a CodeCommit repository",
            "arn:aws:codecommit:us-east-1:123456789012:MyRepo",
        ),
        (
            "a CodeBuild project",
            "arn:aws:codebuild:us-east-1:123456789012:project/widget",
        ),
        (
            "a CodeDeploy application",
            "arn:aws:codedeploy:us-east-1:123456789012:application:app",
        ),
        (
            "a CodePipeline pipeline",
            "arn:aws:codepipeline:us-east-1:123456789012:release",
        ),
    ];

    let emulator = Emulator::new();

    for (index, (case, resource)) in rows.iter().enumerate() {
        let name = format!("supported-{index}");
        let created = emulator
            .ok(
                case,
                "/createNotificationRule",
                &body(&name, resource, &[KNOWN_EVENT_TYPE]),
            )
            .await;
        let arn = string(&created, "Arn");
        let described = emulator
            .ok(case, "/describeNotificationRule", &json!({ "Arn": arn }))
            .await;
        assert_eq!(
            string(&described, "Resource"),
            *resource,
            "case {case:?}: expected the Resource it was created with, got {described}"
        );
        assert_eq!(
            rules_for(&emulator, resource).await,
            1,
            "case {case:?}: the created rule is listed for its resource"
        );
    }

    // And the set the service supports is exactly the four the refusal names.
    assert_eq!(
        codestar::state::SUPPORTED_RESOURCE_SERVICES,
        ["codebuild", "codecommit", "codedeploy", "codepipeline"],
        "the supported services are the four the refusal's text names"
    );
}

/// RULE: an `EventTypeIds` entry the service does not publish is refused with 400
/// `ValidationException`, over the service's own text for the class — fixed either side of
/// ONE slot, the first submitted id that did not resolve — and no rule is created.
///
/// The rows differ in where the unresolvable id sits (alone, first, last, two of them
/// among published ones), and in its form — a wholly foreign id, a published id with a
/// typo, a well-formed id of a supported service the catalog does not list, and one that
/// differs from a published id only in case.
#[tokio::test]
async fn an_event_type_id_the_service_does_not_publish_is_refused() {
    let resource = "arn:aws:codecommit:us-east-1:123456789012:EventTypeRepo";

    // (case, the ids submitted, the id the refusal must name)
    let rows: &[(&str, &[&str], &str)] = &[
        (
            "a wholly foreign id, alone",
            &["an-event-type-that-does-not-exist"],
            "an-event-type-that-does-not-exist",
        ),
        (
            "a published id with a typo",
            &["codecommit-repository-comments-on-commit"],
            "codecommit-repository-comments-on-commit",
        ),
        (
            "an id of a supported service the catalog does not list",
            &["codebuild-project-build-state-unknown"],
            "codebuild-project-build-state-unknown",
        ),
        (
            "a published id in the wrong case",
            &["CodeCommit-Repository-Comments-On-Commits"],
            "CodeCommit-Repository-Comments-On-Commits",
        ),
        (
            "the first of several, the rest published",
            &[
                "not-published-first",
                "codecommit-repository-pull-request-created",
                "codebuild-project-build-state-failed",
            ],
            "not-published-first",
        ),
        (
            "the last of several, the rest published",
            &[
                "codecommit-repository-pull-request-created",
                "codebuild-project-build-state-failed",
                "not-published-last",
            ],
            "not-published-last",
        ),
        (
            "two of them among published ones: the first is named",
            &[
                "codecommit-repository-pull-request-created",
                "not-published-a",
                "not-published-b",
            ],
            "not-published-a",
        ),
    ];

    let emulator = Emulator::new();

    for (index, (case, submitted, named)) in rows.iter().enumerate() {
        let request = body(&format!("unknown-event-{index}"), resource, submitted);
        let (status, answered) = emulator.post("/createNotificationRule", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {answered}"
        );
        assert_eq!(
            error_code(&answered),
            "ValidationException",
            "case {case:?}: got {answered}"
        );
        let stated = message(&answered);
        let expected = unresolvable_event_type(named);
        assert_eq!(
            stated, expected,
            "case {case:?}: expected {expected:?}, got {stated:?}"
        );
        for id in submitted.iter().filter(|id| *id != named) {
            assert!(
                !stated.contains(id),
                "case {case:?}: only the first unresolvable id is named, got {stated:?}"
            );
        }
        assert_eq!(
            rules_for(&emulator, resource).await,
            0,
            "case {case:?}: a refused create must store nothing"
        );
    }

    // The same list, with every id published, is accepted — so what the rows above refuse
    // is the unresolvable entry and nothing else about them.
    let accepted = emulator
        .ok(
            "every id published",
            "/createNotificationRule",
            &body(
                "known-events",
                resource,
                &[
                    "codecommit-repository-pull-request-created",
                    "codebuild-project-build-state-failed",
                ],
            ),
        )
        .await;
    assert!(
        !string(&accepted, "Arn").is_empty(),
        "a list of published ids must create a rule, got {accepted}"
    );
}

/// RULE: the UPDATE path resolves `EventTypeIds` against the same catalog — a rule cannot
/// be re-pointed at an event the service does not publish, and a refused update leaves the
/// stored list alone.
///
/// The rows differ in the form of the unresolvable id and in how many published ids travel
/// with it, and each is followed by an update that only uses published ids to show the
/// path itself works.
#[tokio::test]
async fn an_update_cannot_repoint_a_rule_at_an_event_the_service_does_not_publish() {
    let emulator = Emulator::new();
    let resource = "arn:aws:codebuild:us-east-1:123456789012:project/updates";
    let created = emulator
        .ok(
            "seed",
            "/createNotificationRule",
            &body(
                "update-events",
                resource,
                &["codebuild-project-build-state-failed"],
            ),
        )
        .await;
    let arn = string(&created, "Arn");

    // (case, the ids submitted, the id the refusal must name)
    let rows: &[(&str, &[&str], &str)] = &[
        (
            "one unpublished id",
            &["not-published-at-all"],
            "not-published-at-all",
        ),
        (
            "an unpublished id among published ones",
            &[
                "codebuild-project-build-state-succeeded",
                "codebuild-project-build-state-nope",
            ],
            "codebuild-project-build-state-nope",
        ),
        (
            "a published id with a typo",
            &["codebuild-project-build-state-succeede"],
            "codebuild-project-build-state-succeede",
        ),
    ];

    for (case, ids, named) in rows {
        let (status, answered) = emulator
            .post(
                "/updateNotificationRule",
                &json!({ "Arn": arn, "EventTypeIds": ids }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {answered}"
        );
        assert_eq!(
            error_code(&answered),
            "ValidationException",
            "case {case:?}: got {answered}"
        );
        // The same text the create path states for the class: it is the service's one
        // wording for a rule that "cannot be created or modified".
        let stated = message(&answered);
        let expected = unresolvable_event_type(named);
        assert_eq!(
            stated, expected,
            "case {case:?}: expected {expected:?}, got {stated:?}"
        );

        let described = emulator
            .ok(case, "/describeNotificationRule", &json!({ "Arn": arn }))
            .await;
        let stored: Vec<String> = described
            .get("EventTypes")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .map(|summary| string(summary, "EventTypeId"))
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            stored,
            vec!["codebuild-project-build-state-failed".to_string()],
            "case {case:?}: a refused update must leave the stored ids alone, got {described}"
        );
    }

    // A list of published ids does replace the stored one.
    emulator
        .ok(
            "published ids",
            "/updateNotificationRule",
            &json!({
                "Arn": arn,
                "EventTypeIds": [
                    "codebuild-project-build-state-succeeded",
                    "codebuild-project-build-phase-success",
                ],
            }),
        )
        .await;
    let described = emulator
        .ok("published ids", "/describeNotificationRule", &json!({ "Arn": arn }))
        .await;
    let stored: Vec<String> = described
        .get("EventTypes")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .map(|summary| string(summary, "EventTypeId"))
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        stored,
        vec![
            "codebuild-project-build-state-succeeded".to_string(),
            "codebuild-project-build-phase-success".to_string(),
        ],
        "an update naming published ids replaces the stored list, got {described}"
    );
}

/// RULE: what the create and update paths accept is exactly what `ListEventTypes`
/// publishes — every id the listing reports is accepted, and nothing else is.
///
/// This walks the whole published catalog rather than a sample, so a catalog entry added
/// without being accepted (or accepted without being published) fails here.
#[tokio::test]
async fn every_published_event_type_is_accepted_and_nothing_else_is() {
    let emulator = Emulator::new();
    let resource = "arn:aws:codecommit:us-east-1:123456789012:CatalogRepo";

    let mut published: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut request = json!({ "MaxResults": 100 });
        if let Some(value) = &token {
            request["NextToken"] = json!(value);
        }
        let listed = emulator.ok("catalog", "/listEventTypes", &request).await;
        published.extend(
            listed
                .get("EventTypes")
                .and_then(Value::as_array)
                .map(|list| {
                    list.iter()
                        .map(|summary| string(summary, "EventTypeId"))
                        .collect::<Vec<String>>()
                })
                .unwrap_or_default(),
        );
        match listed.get("NextToken").and_then(Value::as_str) {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
    }
    assert!(
        !published.is_empty(),
        "ListEventTypes must publish at least one event type"
    );

    for (index, id) in published.iter().enumerate() {
        let created = emulator
            .ok(
                id,
                "/createNotificationRule",
                &body(&format!("published-{index}"), resource, &[id.as_str()]),
            )
            .await;
        assert!(
            !string(&created, "Arn").is_empty(),
            "case {id:?}: a published event type must be accepted, got {created}"
        );
    }

    // An id that is not in the listing is refused, whichever published id it sits beside.
    for (index, published_id) in published.iter().take(3).enumerate() {
        let (status, answered) = emulator
            .post(
                "/createNotificationRule",
                &body(
                    &format!("unpublished-{index}"),
                    resource,
                    &[published_id.as_str(), "an-id-the-listing-does-not-report"],
                ),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {published_id:?}: an id outside the listing is refused, got {status} with \
             {answered}"
        );
        assert_eq!(
            error_code(&answered),
            "ValidationException",
            "case {published_id:?}: got {answered}"
        );
    }
}
