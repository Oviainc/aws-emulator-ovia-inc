// SPDX-License-Identifier: CC0-1.0

//! What `DescribeNotificationRule` reports about a stored rule, over the wire.
//!
//! Each suite asserts a RULE over a TABLE of inputs rather than one recorded request:
//!
//! * an `EventTypeSummary` reports the event-type catalog's own published values
//!   (`EventTypeName`, `ResourceType`, `ServiceName`) verbatim, for every catalogued id;
//! * `Tags` is the rule's own tag map and is therefore present with ZERO entries for an
//!   untagged rule, never an absent member;
//! * a target this service subscribed is `ACTIVE` \u2014 a modelled `TargetStatus` \u2014 from the
//!   first read, whatever the rule's targets are;
//! * `Unsubscribe` detaches the address from the rule, so the next describe no longer
//!   reports it.
//!
//! The harness (`Emulator`, `string`, `tags_of`, ...) is the one
//! `notification_rule_lifecycle` already builds: one shared engine behind the same
//! `MultiServiceRouter` dispatch the emulator serves production requests with.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_codestarnotifications_codestarnotifications_20191015 as codestar;

use crate::notification_rule_lifecycle::{
    error_code, string, tags_of, target_addresses_of, target_statuses_of, Emulator,
};

/// Create a rule and return the ARN it was minted with.
async fn create_rule(emulator: &Emulator, case: &str, body: &Value) -> String {
    let created = emulator.ok(case, "/createNotificationRule", body).await;
    let arn = string(&created, "Arn");
    assert!(
        !arn.is_empty(),
        "case {case:?}: create must answer with an ARN, got {created}"
    );
    arn
}

/// Describe `arn` and return the body.
async fn describe(emulator: &Emulator, case: &str, arn: &str) -> Value {
    emulator
        .ok(case, "/describeNotificationRule", &json!({ "Arn": arn }))
        .await
}

/// One event-type summary as the API reports it.
fn summary_of(body: &Value, index: usize) -> Value {
    body.get("EventTypes")
        .and_then(Value::as_array)
        .and_then(|list| list.get(index))
        .cloned()
        .unwrap_or(Value::Null)
}

/// An `EventTypeSummary` carries the catalog's published name, resource type and
/// service name for the id the rule stores \u2014 nothing rendered from the id itself.
///
/// The rows below are LITERAL expectations taken from the event-type list in the
/// Developer Tools Console User Guide that `EventTypeSummary$EventTypeId`'s own
/// documentation links to
/// (<https://docs.aws.amazon.com/codestar-notifications/latest/userguide/concepts.html#concepts-api>),
/// one per service in the catalog and several the issue that prompted this suite never
/// cited. They differ along what the rule generalises over: the owning service, the
/// resource type, how many words the category has, and whether the category is one word
/// or several.
#[tokio::test]
async fn an_event_type_summary_reports_the_catalogs_published_values() {
    // (EventTypeId, ServiceName, ResourceType, EventTypeName)
    let rows: &[(&str, &str, &str, &str)] = &[
        (
            "codecommit-repository-comments-on-commits",
            "CodeCommit",
            "Repository",
            "Comments: On commits",
        ),
        (
            "codecommit-repository-pull-request-source-updated",
            "CodeCommit",
            "Repository",
            "Pull request: Source updated",
        ),
        (
            "codecommit-repository-branches-and-tags-created",
            "CodeCommit",
            "Repository",
            "Branches and tags: Created",
        ),
        (
            "codebuild-project-build-state-in-progress",
            "CodeBuild",
            "Project",
            "Build state: In-progress",
        ),
        (
            "codedeploy-application-deployment-succeeded",
            "CodeDeploy",
            "Application",
            "Deployment: Succeeded",
        ),
        (
            "codepipeline-pipeline-stage-execution-resumed",
            "CodePipeline",
            "Pipeline",
            "Stage execution: Resumed",
        ),
    ];

    let emulator = Emulator::new();

    // One rule carrying every row, so the describe reports them in the stored order.
    let ids: Vec<&str> = rows.iter().map(|(id, _, _, _)| *id).collect();
    let arn = create_rule(
        &emulator,
        "catalog",
        &json!({
            "Name": "catalog-fidelity",
            "Resource": "arn:aws:codecommit:us-east-1:123456789012:MyRepo",
            "DetailType": "FULL",
            "EventTypeIds": ids,
            "Targets": [],
        }),
    )
    .await;
    let described = describe(&emulator, "catalog", &arn).await;

    for (index, (id, service_name, resource_type, event_type_name)) in rows.iter().enumerate() {
        let summary = summary_of(&described, index);
        let actual = (
            string(&summary, "EventTypeId"),
            string(&summary, "ServiceName"),
            string(&summary, "ResourceType"),
            string(&summary, "EventTypeName"),
        );
        let expected = (
            (*id).to_string(),
            (*service_name).to_string(),
            (*resource_type).to_string(),
            (*event_type_name).to_string(),
        );
        assert_eq!(
            actual, expected,
            "case {id:?}: expected {expected:?}, got {actual:?} from {summary}"
        );
    }
}

/// Every entry of the seeded catalog carries data the model accepts, and no id is
/// catalogued twice: `ResourceType` is `@length(min: 1)` with pattern
/// `^([a-zA-Z0-9-])+$`, and the display name AWS publishes is a capitalised label.
#[tokio::test]
async fn the_catalog_data_satisfies_the_modelled_constraints() {
    let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
    for entry in codestar::state::EVENT_TYPE_CATALOG {
        let count = seen.entry(entry.event_type_id).or_default();
        *count += 1;
        assert_eq!(
            *count, 1,
            "case {:?}: an event type id is catalogued once, got {count} entries",
            entry.event_type_id
        );
        let resource_type = entry.resource_type;
        assert!(
            !resource_type.is_empty()
                && resource_type
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-'),
            "case {:?}: ResourceType is len 1.. matching ^([a-zA-Z0-9-])+$, got {resource_type:?}",
            entry.event_type_id
        );
        assert!(
            resource_type
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_uppercase()),
            "case {:?}: ResourceType is the service's capitalised noun, got {resource_type:?}",
            entry.event_type_id
        );
        let name = entry.event_type_name;
        assert!(
            name.chars().next().is_some_and(|c| c.is_ascii_uppercase()),
            "case {:?}: EventTypeName is a capitalised label, got {name:?}",
            entry.event_type_id
        );
        assert!(
            name.contains(": "),
            "case {:?}: EventTypeName is the published label (a category, then the state it \
             reports), not the id slug, got {name:?}",
            entry.event_type_id
        );
    }
    assert!(
        seen.len() == codestar::state::EVENT_TYPE_CATALOG.len() && !seen.is_empty(),
        "the catalog holds {} distinct ids out of {} entries",
        seen.len(),
        codestar::state::EVENT_TYPE_CATALOG.len()
    );
}

/// `Tags` is the rule's own map: it is PRESENT in every describe, holding exactly the
/// tags the rule stores \u2014 zero of them for an untagged rule, which is an empty map and
/// not an absent member.
///
/// The rows differ in tag count (including none at all) and in key and value shape.
#[tokio::test]
async fn tags_are_reported_as_a_map_even_when_the_rule_has_none() {
    let rows: &[(&str, &[(&str, &str)])] = &[
        ("no tags at all", &[]),
        ("one tag", &[("Team", "dev")]),
        ("two tags", &[("Team", "platform"), ("env", "prod")]),
        (
            "a tag with an empty value (TagValue is len 0..256)",
            &[("marker", "")],
        ),
        (
            "three tags",
            &[("a", "1"), ("b", "2"), ("c", "3")],
        ),
    ];

    let emulator = Emulator::new();

    for (index, (label, tags)) in rows.iter().enumerate() {
        let mut body = json!({
            "Name": format!("tags-case-{index}"),
            "Resource": "arn:aws:codecommit:us-east-1:123456789012:MyRepo",
            "DetailType": "BASIC",
            "EventTypeIds": ["codecommit-repository-pull-request-created"],
            "Targets": [],
        });
        if !tags.is_empty() {
            body["Tags"] = Value::Object(
                tags.iter()
                    .map(|(k, v)| ((*k).to_string(), json!(v)))
                    .collect(),
            );
        }
        let arn = create_rule(&emulator, label, &body).await;
        let described = describe(&emulator, label, &arn).await;

        assert!(
            described.get("Tags").is_some_and(Value::is_object),
            "case {label:?}: Tags is a map on every describe, got {described}"
        );
        let expected: BTreeMap<String, String> = tags
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        assert_eq!(
            tags_of(&described),
            expected,
            "case {label:?}: expected Tags {expected:?}, got {described}"
        );

        // The map stays a map across an update that does not touch the tags.
        emulator
            .ok(
                label,
                "/updateNotificationRule",
                &json!({ "Arn": arn, "DetailType": "FULL" }),
            )
            .await;
        let after = describe(&emulator, label, &arn).await;
        assert!(
            after.get("Tags").is_some_and(Value::is_object),
            "case {label:?}: Tags is still a map after an update, got {after}"
        );
        assert_eq!(
            tags_of(&after),
            expected,
            "case {label:?}: an update that names no tags leaves them alone, got {after}"
        );
    }
}

/// A target this service subscribed reports a modelled `TargetStatus`, and the status it
/// reports is `ACTIVE` from the first read: the association is usable when the call that
/// created it returns, so no read has to settle it.
///
/// The rows differ in target count (none, one, several, the modelled maximum) and type.
#[tokio::test]
async fn every_subscribed_target_is_reported_active() {
    let emulator = Emulator::new();

    let max = codestar::state::MAX_TARGETS_PER_RULE;
    let full_house: Vec<(String, String)> = (0..max)
        .map(|index| {
            (
                "SNS".to_string(),
                format!("arn:aws:sns:us-east-1:123456789012:bulk-{index}"),
            )
        })
        .collect();
    let rows: Vec<(String, Vec<(String, String)>)> = vec![
        ("no targets (the modelled minimum)".to_string(), vec![]),
        (
            "one SNS topic".to_string(),
            vec![(
                "SNS".to_string(),
                "arn:aws:sns:us-east-1:123456789012:one".to_string(),
            )],
        ),
        (
            "an SNS topic and a chat client".to_string(),
            vec![
                (
                    "SNS".to_string(),
                    "arn:aws:sns:us-east-1:123456789012:two".to_string(),
                ),
                (
                    "AWSChatbotSlack".to_string(),
                    "arn:aws:chatbot::123456789012:chat-configuration/slack-channel/eng".to_string(),
                ),
            ],
        ),
        (format!("{max} targets (the modelled maximum)"), full_house),
    ];

    for (index, (label, targets)) in rows.iter().enumerate() {
        let arn = create_rule(
            &emulator,
            label,
            &json!({
                "Name": format!("status-case-{index}"),
                "Resource": "arn:aws:codebuild:us-east-1:123456789012:project/widget",
                "DetailType": "BASIC",
                "EventTypeIds": ["codebuild-project-build-state-failed"],
                "Targets": targets
                    .iter()
                    .map(|(target_type, address)| json!({
                        "TargetType": target_type,
                        "TargetAddress": address,
                    }))
                    .collect::<Vec<_>>(),
            }),
        )
        .await;
        let described = describe(&emulator, label, &arn).await;

        let statuses = target_statuses_of(&described);
        assert_eq!(
            statuses.len(),
            targets.len(),
            "case {label:?}: expected {} target summaries, got {described}",
            targets.len()
        );
        for status in &statuses {
            assert!(
                codestar::ops::types::TARGET_STATUS_VALUES.contains(&status.as_str()),
                "case {label:?}: TargetStatus must be a modelled value, got {status:?}"
            );
            assert_eq!(
                status, "ACTIVE",
                "case {label:?}: a subscribed target is ACTIVE, got {described}"
            );
        }
    }
}

/// `Unsubscribe` removes the association between the rule and one target address: the
/// next describe reports exactly the addresses that are left, in their stored order.
///
/// The rows differ in how many targets the rule holds and which one is removed
/// (including the only one, and one the rule never held).
#[tokio::test]
async fn unsubscribe_detaches_the_target_from_the_rule() {
    let emulator = Emulator::new();
    let address = |index: usize| format!("arn:aws:sns:us-east-1:123456789012:detach-{index}");

    // (label, how many targets the rule starts with, the address to unsubscribe)
    let rows: &[(&str, usize, Option<usize>)] = &[
        ("the only target", 1, Some(0)),
        ("the first of three", 3, Some(0)),
        ("the last of three", 3, Some(2)),
        ("a middle one of four", 4, Some(1)),
        ("an address the rule never held", 2, None),
    ];

    for (case_index, (label, count, removed)) in rows.iter().enumerate() {
        let addresses: Vec<String> = (0..*count)
            .map(|index| address(case_index * 10 + index))
            .collect();
        let arn = create_rule(
            &emulator,
            label,
            &json!({
                "Name": format!("detach-case-{case_index}"),
                "Resource": "arn:aws:codecommit:us-east-1:123456789012:MyRepo",
                "DetailType": "BASIC",
                "EventTypeIds": ["codecommit-repository-comments-on-commits"],
                "Targets": addresses
                    .iter()
                    .map(|a| json!({ "TargetType": "SNS", "TargetAddress": a }))
                    .collect::<Vec<_>>(),
            }),
        )
        .await;
        assert_eq!(
            target_addresses_of(&describe(&emulator, label, &arn).await),
            addresses,
            "case {label:?}: the rule must start with every target it was created with"
        );

        let target_address = match removed {
            Some(index) => addresses[*index].clone(),
            None => address(900 + case_index),
        };
        let unsubscribed = emulator
            .ok(
                label,
                "/unsubscribe",
                &json!({ "Arn": arn, "TargetAddress": target_address }),
            )
            .await;
        assert_eq!(
            string(&unsubscribed, "Arn"),
            arn,
            "case {label:?}: unsubscribe answers for the rule it was given, got {unsubscribed}"
        );

        let expected: Vec<String> = addresses
            .iter()
            .filter(|a| **a != target_address)
            .cloned()
            .collect();
        let described = describe(&emulator, label, &arn).await;
        assert_eq!(
            target_addresses_of(&described),
            expected,
            "case {label:?}: expected {expected:?} left, got {described}"
        );
        // Everything still reported keeps its own status.
        for status in target_statuses_of(&described) {
            assert_eq!(
                status, "ACTIVE",
                "case {label:?}: a target that is still attached stays ACTIVE, got {described}"
            );
        }

        // Removal is idempotent: the association is already gone the second time.
        let repeated = emulator
            .ok(
                label,
                "/unsubscribe",
                &json!({ "Arn": arn, "TargetAddress": target_address }),
            )
            .await;
        assert_eq!(
            string(&repeated, "Arn"),
            arn,
            "case {label:?}: a repeated unsubscribe is a success, got {repeated}"
        );
        assert_eq!(
            target_addresses_of(&describe(&emulator, label, &arn).await),
            expected,
            "case {label:?}: a repeated unsubscribe changes nothing"
        );
    }
}

/// An address that was unsubscribed can be subscribed again, and is reported once.
#[tokio::test]
async fn an_unsubscribed_address_can_be_subscribed_again() {
    let emulator = Emulator::new();
    let address = "arn:aws:sns:us-east-1:123456789012:resubscribe";
    let arn = create_rule(
        &emulator,
        "resubscribe",
        &json!({
            "Name": "resubscriber",
            "Resource": "arn:aws:codecommit:us-east-1:123456789012:MyRepo",
            "DetailType": "BASIC",
            "EventTypeIds": ["codecommit-repository-comments-on-commits"],
            "Targets": [{ "TargetType": "SNS", "TargetAddress": address }],
        }),
    )
    .await;

    emulator
        .ok(
            "resubscribe",
            "/unsubscribe",
            &json!({ "Arn": arn, "TargetAddress": address }),
        )
        .await;
    assert!(
        target_addresses_of(&describe(&emulator, "resubscribe", &arn).await).is_empty(),
        "case \"after unsubscribe\": the rule holds no targets"
    );

    emulator
        .ok(
            "resubscribe",
            "/subscribe",
            &json!({
                "Arn": arn,
                "Target": { "TargetType": "SNS", "TargetAddress": address },
            }),
        )
        .await;
    let described = describe(&emulator, "resubscribe", &arn).await;
    assert_eq!(
        target_addresses_of(&described),
        vec![address.to_string()],
        "case \"after re-subscribing\": the address is attached once, got {described}"
    );
    assert_eq!(
        target_statuses_of(&described),
        vec!["ACTIVE".to_string()],
        "case \"after re-subscribing\": expected ACTIVE, got {described}"
    );
}

/// Unsubscribing on an ARN no rule in this region holds is refused with the one client
/// error `Unsubscribe` models, rather than reported as a removal that happened.
#[tokio::test]
async fn unsubscribing_an_unknown_rule_is_refused() {
    let emulator = Emulator::new();
    let address = "arn:aws:sns:us-east-1:123456789012:ghost";

    // A well-formed ARN (the member's pattern is checked before the handler runs) that
    // names no rule, and a rule that exists but in another region.
    let created = create_rule(
        &emulator,
        "other region",
        &json!({
            "Name": "elsewhere",
            "Resource": "arn:aws:codecommit:us-east-1:123456789012:MyRepo",
            "DetailType": "BASIC",
            "EventTypeIds": ["codecommit-repository-comments-on-commits"],
            "Targets": [{ "TargetType": "SNS", "TargetAddress": address }],
        }),
    )
    .await;
    let cases: &[(&str, &str, &str)] = &[
        (
            "no such rule",
            "us-east-1",
            "arn:aws:codestar-notifications:us-east-1:123456789012:notificationrule/11111111-2222-4333-8444-555555555555",
        ),
        ("a rule from another region", "eu-west-1", &created),
    ];

    for (label, region, arn) in cases {
        let (status, parsed) = emulator
            .post_in(
                region,
                "/unsubscribe",
                &json!({ "Arn": arn, "TargetAddress": address }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected 400, got {status} with {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ValidationException",
            "case {label:?}: got {parsed}"
        );
    }

    // The rule that does exist still holds its target.
    assert_eq!(
        target_addresses_of(&describe(&emulator, "other region", &created).await),
        vec![address.to_string()],
        "a refused unsubscribe must not have detached anything"
    );
}
