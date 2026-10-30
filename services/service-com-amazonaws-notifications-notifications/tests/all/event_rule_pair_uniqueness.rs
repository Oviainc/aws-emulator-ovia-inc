// SPDX-License-Identifier: CC0-1.0

//! What makes an `EventRule` unique inside a `NotificationConfiguration`, driven
//! over HTTP through the same multi-service router the emulator boots in
//! production.
//!
//! The rule under test: a configuration holds at most ONE EventRule per
//! `(source, eventType)` pair, so a create naming a pair one of its rules
//! already covers is refused with the modelled `ConflictException` (HTTP 409)
//! and persists nothing, whatever the rest of the request carries, while a pair
//! no rule of that configuration covers mints a rule of its own.
//!
//! The rows vary along what that rule generalises over: which pair is created
//! (both `@length` bounds of `Source` and of `EventType`, and sources and event
//! types no issue cited), how the duplicate request differs from the create that
//! won (not at all, in `eventPattern`, in the Regions it covers, in the order it
//! lists them), how MANY creates race for one pair, and which configuration the
//! pair belongs to. Catalog source:
//! `smithy_query(query="operation",operation="CreateEventRule",model_path="services/service-com-amazonaws-notifications-notifications/smithy-model/com-amazonaws-notifications-notifications.json")`
//! — `source` `@length(1..36)` `@pattern("^aws.([a-z0-9\\-])+$")`, `eventType`
//! `@length(1..128)` `@pattern("^([a-zA-Z0-9 \\-\\(\\)])+$")`, and the
//! `ConflictException` it declares with `httpError` 409.

use std::sync::Arc;

use serde_json::{json, Value};

// The HTTP harness this suite drives the service through, shared with every
// other suite in this crate, including the `GetEventRule` reader a created rule
// is read back through and the store-depth probe a refusal is checked against.
use crate::harness::{
    encode_path_label, error_code, get_event_rule, is_event_rule_arn, stored_event_rule_count,
    Emulator,
};

/// A Region every row's rule covers. Catalog source: the `Region` shape the
/// model declares for the members of `Regions`.
const A_REGION: &str = "us-east-1";
/// A second Region, so a duplicate can widen the Region set of the rule that
/// won.
const ANOTHER_REGION: &str = "eu-west-1";

/// One `(source, eventType)` pair a rule is created for. Rows differ in the
/// length and shape of each member of the pair, at both `@length` bounds of
/// each.
struct Pair {
    case: &'static str,
    source: String,
    event_type: String,
}

/// The pairs the rows draw from.
fn pairs() -> Vec<Pair> {
    vec![
        Pair {
            case: "a commonplace pair",
            source: "aws.ec2".to_string(),
            event_type: "EC2 Instance State-change Notification".to_string(),
        },
        Pair {
            case: "a pair no issue cited",
            source: "aws.s3".to_string(),
            event_type: "Object Access Tier Changed".to_string(),
        },
        Pair {
            // Catalog source: `Source`'s `@length(1..36)` floor and
            // `EventType`'s `@length(1..128)` floor.
            case: "the shortest source and event type the constraints admit",
            source: "aws.s".to_string(),
            event_type: "a".to_string(),
        },
        Pair {
            // Catalog source: the same two members' `@length` ceilings.
            case: "the longest source and event type the length bounds admit",
            source: format!("aws.{}", "b".repeat(32)),
            event_type: format!("Event Type {}", "E".repeat(117)),
        },
        Pair {
            case: "an event type carrying the parentheses and hyphens the pattern permits",
            source: "aws.cloudwatch".to_string(),
            event_type: "CloudWatch Alarm State Change (ALARM)".to_string(),
        },
    ]
}

/// The whole sentence a create for an already-covered pair must state, spelled
/// here rather than read from the service so the test asserts the wording
/// instead of echoing it.
fn conflict_message(configuration: &str, pair: &Pair) -> String {
    format!(
        "an event rule with source {} and event type {} already exists for notification \
         configuration {configuration}",
        pair.source, pair.event_type
    )
}

/// Create a NotificationConfiguration for rules to hang off, and return its arn.
async fn create_configuration_for_rules(emulator: &Emulator, name: &str) -> String {
    let (status, response) = emulator
        .send(
            "POST",
            "/notification-configurations",
            Some(json!({ "name": name, "description": "a configuration under test" })),
        )
        .await;
    assert_eq!(
        status, 201,
        "case {name:?}: the configuration a rule hangs off must be created, got {response:?}"
    );
    response["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: a create must answer an arn, got {response:?}"))
        .to_string()
}

/// The `CreateEventRule` body for one pair, with whatever extra members a row
/// asks for.
fn create_body(configuration: &str, pair: &Pair, regions: &[&str], event_pattern: Option<&str>) -> Value {
    let mut body = json!({
        "notificationConfigurationArn": configuration,
        "source": pair.source,
        "eventType": pair.event_type,
        "regions": regions,
    });
    if let Some(pattern) = event_pattern {
        body["eventPattern"] = json!(pattern);
    }
    body
}

async fn create_event_rule(emulator: &Emulator, body: Value) -> (u16, Value) {
    emulator.send("POST", "/event-rules", Some(body)).await
}

/// The arns `ListEventRules` reports for one configuration.
async fn listed_rule_arns(emulator: &Emulator, case: &str, configuration: &str) -> Vec<String> {
    let uri = format!(
        "/event-rules?notificationConfigurationArn={}&maxResults=100",
        encode_path_label(configuration)
    );
    let (status, listed) = emulator.send("GET", &uri, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: a listing of the configuration's rules answers 200, got {listed:?}"
    );
    listed["eventRules"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: a listing answers a list, got {listed:?}"))
        .iter()
        .map(|entry| entry["arn"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// RULE: a configuration holds one EventRule per `(source, eventType)` pair, so
/// a burst of creates racing for one pair resolves to exactly ONE created rule
/// carrying a freshly minted arn and one `ConflictException` (HTTP 409) per
/// loser — and the winner is the only rule the configuration reports for that
/// pair.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_burst_of_creates_for_one_pair_resolves_to_exactly_one_rule() {
    // Rows differ in how many creates race for the pair, and in whether the
    // racers carry identical bodies or differ in the members a rule holds
    // rather than is named by.
    for size in [2usize, 5, 9] {
        for pair in pairs() {
            let case = format!("{} in a burst of {size}", pair.case);
            let emulator = Arc::new(Emulator::new());
            let configuration =
                create_configuration_for_rules(&emulator, "burst-host").await;

            let mut running = Vec::with_capacity(size);
            for index in 0..size {
                // Every racer names the same pair; the Regions and the
                // eventPattern differ per racer, which the pair rule makes no
                // difference to.
                let regions: Vec<&str> = if index % 2 == 0 {
                    vec![A_REGION]
                } else {
                    vec![A_REGION, ANOTHER_REGION]
                };
                let pattern = (index % 3 == 0).then(|| format!(r#"{{"detail":{{"racer":[{index}]}}}}"#));
                let body = create_body(&configuration, &pair, &regions, pattern.as_deref());
                let emulator = Arc::clone(&emulator);
                running.push(tokio::spawn(async move {
                    create_event_rule(&emulator, body).await
                }));
            }

            let mut winners: Vec<String> = Vec::new();
            let mut losers: Vec<Value> = Vec::new();
            for task in running {
                let (status, response) = task.await.expect("a create task runs to completion");
                match status {
                    201 => winners.push(response["arn"].as_str().unwrap_or_default().to_string()),
                    409 => losers.push(response),
                    other => panic!(
                        "case {case:?}: a create answers either the modelled 201 or the modelled 409, got {other} {response:?}"
                    ),
                }
            }

            assert_eq!(
                winners.len(),
                1,
                "case {case:?}: exactly one create of a pair wins, got winners {winners:?}"
            );
            assert_eq!(
                losers.len(),
                size - 1,
                "case {case:?}: every other create is refused, got {} refusals",
                losers.len()
            );
            let winner = &winners[0];
            assert!(
                is_event_rule_arn(winner),
                "case {case:?}: the winner carries a freshly minted EventRuleArn, got {winner:?}"
            );
            assert!(
                winner.starts_with(&format!("{configuration}/rule/")),
                "case {case:?}: the winner's arn nests under its configuration {configuration:?}, got {winner:?}"
            );
            for refused in &losers {
                assert!(
                    error_code(refused).contains("ConflictException"),
                    "case {case:?}: a loser is refused with a ConflictException, got {refused:?}"
                );
                assert_eq!(
                    refused["message"].as_str().unwrap_or_default(),
                    conflict_message(&configuration, &pair),
                    "case {case:?}: the refusal states the covered pair, got {refused:?}"
                );
                assert_eq!(
                    refused["resourceId"].as_str().unwrap_or_default(),
                    winner.as_str(),
                    "case {case:?}: the refusal names the rule that covers the pair, got {refused:?}"
                );
            }

            assert_eq!(
                stored_event_rule_count(&emulator),
                1,
                "case {case:?}: the burst persists exactly one rule"
            );
            assert_eq!(
                listed_rule_arns(&emulator, &case, &configuration).await,
                vec![winner.clone()],
                "case {case:?}: the winner is the only rule the configuration reports"
            );
            let (status, read_back) = get_event_rule(&emulator, winner).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the created rule is readable, got {read_back:?}"
            );
            assert_eq!(
                read_back["source"].as_str().unwrap_or_default(),
                pair.source,
                "case {case:?}: the stored rule carries the source it was created with, got {read_back:?}"
            );
            assert_eq!(
                read_back["eventType"].as_str().unwrap_or_default(),
                pair.event_type,
                "case {case:?}: the stored rule carries the event type it was created with, got {read_back:?}"
            );
        }
    }
}

/// RULE: the uniqueness of a pair is scoped to ONE NotificationConfiguration, so
/// the same pair is created freely on a configuration no rule of which covers it
/// — and a configuration that no longer exists holds no pair at all.
#[tokio::test]
async fn one_configurations_covered_pair_leaves_another_configuration_free() {
    let emulator = Emulator::new();
    let pair = &pairs()[0];

    // Rows differ in which configuration the pair is created on: two that exist
    // side by side, and a third created after a configuration covering the pair
    // was deleted.
    let first = create_configuration_for_rules(&emulator, "first-host").await;
    let second = create_configuration_for_rules(&emulator, "second-host").await;

    let mut arns = Vec::new();
    for configuration in [&first, &second] {
        let (status, created) =
            create_event_rule(&emulator, create_body(configuration, pair, &[A_REGION], None)).await;
        assert_eq!(
            status, 201,
            "case {configuration:?}: a pair no rule of this configuration covers is created, got {created:?}"
        );
        arns.push(created["arn"].as_str().unwrap_or_default().to_string());
    }
    assert_ne!(
        arns[0], arns[1],
        "two configurations' rules for one pair are distinct rules, got {arns:?}"
    );

    // On either configuration the pair is now covered.
    for configuration in [&first, &second] {
        let (status, refused) = create_event_rule(
            &emulator,
            create_body(configuration, pair, &[ANOTHER_REGION], None),
        )
        .await;
        assert_eq!(
            status, 409,
            "case {configuration:?}: the pair is covered here now, got {refused:?}"
        );
        assert!(
            error_code(&refused).contains("ConflictException"),
            "case {configuration:?}: that refusal is a ConflictException, got {refused:?}"
        );
    }
    assert_eq!(
        stored_event_rule_count(&emulator),
        2,
        "the refusals persisted nothing"
    );

    // Deleting a configuration takes its rules with it, so the pair is free
    // again on a configuration created afterwards.
    let (status, deleted) = emulator
        .send(
            "DELETE",
            &format!("/notification-configurations/{}", encode_path_label(&first)),
            None,
        )
        .await;
    assert_eq!(status, 200, "the delete must succeed, got {deleted:?}");
    let third = create_configuration_for_rules(&emulator, "third-host").await;
    let (status, created) =
        create_event_rule(&emulator, create_body(&third, pair, &[A_REGION], None)).await;
    assert_eq!(
        status, 201,
        "a fresh configuration covers no pair, got {created:?}"
    );
    assert!(
        !arns.contains(&created["arn"].as_str().unwrap_or_default().to_string()),
        "the new rule is its own rule, got {created:?}"
    );
}

/// RULE: a create for a covered pair is refused whatever else the request
/// carries, and the refusal changes nothing a later reader sees — the rule that
/// covers the pair keeps the `eventPattern` and the Regions it was created with.
#[tokio::test]
async fn a_refused_duplicate_leaves_the_covering_rule_as_it_was() {
    for pair in pairs() {
        let label = pair.case;
        let emulator = Emulator::new();
        let configuration = create_configuration_for_rules(&emulator, "duplicate-host").await;

        let original_pattern = r#"{"detail":{"state":{"value":["ALARM"]}}}"#;
        let (status, created) = create_event_rule(
            &emulator,
            create_body(&configuration, &pair, &[A_REGION], Some(original_pattern)),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {label:?}: the covering rule must be created, got {created:?}"
        );
        let covering = created["arn"].as_str().unwrap_or_default().to_string();
        let (status, before) = get_event_rule(&emulator, &covering).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the covering rule must be readable, got {before:?}"
        );

        // Rows differ in how the duplicate varies from the create that won: not
        // at all, in `eventPattern` (absent, empty, another pattern) and in the
        // Regions it covers (widened, reordered).
        for (variation, regions, event_pattern) in [
            ("the identical request", vec![A_REGION], Some(original_pattern)),
            ("no eventPattern at all", vec![A_REGION], None),
            ("an empty eventPattern", vec![A_REGION], Some("")),
            (
                "another eventPattern",
                vec![A_REGION],
                Some(r#"{"detail":{"state":{"value":["OK"]}}}"#),
            ),
            (
                "one more Region",
                vec![A_REGION, ANOTHER_REGION],
                Some(original_pattern),
            ),
            (
                "the Regions in another order",
                vec![ANOTHER_REGION, A_REGION],
                Some(original_pattern),
            ),
        ] {
            let (status, refused) = create_event_rule(
                &emulator,
                create_body(&configuration, &pair, &regions, event_pattern),
            )
            .await;
            assert_eq!(
                status, 409,
                "case {label:?} / {variation}: a create for a covered pair answers the modelled 409, got {refused:?}"
            );
            assert!(
                error_code(&refused).contains("ConflictException"),
                "case {label:?} / {variation}: that refusal is a ConflictException, got {refused:?}"
            );
            assert_eq!(
                refused["message"].as_str().unwrap_or_default(),
                conflict_message(&configuration, &pair),
                "case {label:?} / {variation}: the refusal states the covered pair, got {refused:?}"
            );
            assert_eq!(
                stored_event_rule_count(&emulator),
                1,
                "case {label:?} / {variation}: a refused create persists nothing"
            );
            let (status, after) = get_event_rule(&emulator, &covering).await;
            assert_eq!(
                status, 200,
                "case {label:?} / {variation}: the covering rule is still readable, got {after:?}"
            );
            for member in ["eventPattern", "regions", "source", "eventType", "creationTime"] {
                assert_eq!(
                    after[member], before[member],
                    "case {label:?} / {variation}: a refused duplicate leaves {member} as it was, got {after:?}"
                );
            }
        }
    }
}
