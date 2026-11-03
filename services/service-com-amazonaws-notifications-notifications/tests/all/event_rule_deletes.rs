// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteEventRule`, driven over HTTP through the same
//! multi-service router the emulator boots in production.
//!
//! Every case asserts the RULE the operation follows — a delete keyed by an arn
//! removes exactly the record that arn names, or refuses with the modelled
//! not-found error when the store holds none — rather than one recorded request.
//! The table rows vary along what that rule generalises over: how many rules the
//! configuration owns when the delete runs, which of them is deleted (oldest,
//! middle, newest), how many Regions the rule covers, whether it carries an
//! `eventPattern`, and whether the rule is still in the transitional status the
//! create minted it in or has already settled. The catalogs the rows draw from
//! are the model's own constraints and this service's own bound
//! (`MAX_EVENT_RULES_PER_CONFIGURATION`), so a model change moves the test with
//! it. The refusal's wording is asserted whole, and the resource it names is the
//! name this service's refusal of a MUTATION of an EventRule uses, which is not
//! the one its refusal of a READ uses.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_notifications_notifications::state::{
    not_found_message, ARN_IDENTIFIER_LABEL, CONFIGURATION_STATUS_ACTIVE,
    CONFIGURATION_STATUS_INACTIVE, EVENT_RULE_MUTATION_RESOURCE_LABEL, EVENT_RULE_RESOURCE_LABEL,
    EVENT_RULE_TRANSITION_SPAN, MAX_EVENT_RULES_PER_CONFIGURATION,
};

use crate::harness::{
    encode_path_label, error_code, get_event_rule, is_event_rule_arn, stored_event_rule_count,
    Emulator,
};

/// Long enough that the settling the create made due has certainly fallen due,
/// without depending on how slow the machine is.
const PAST_THE_TRANSITION: Duration =
    EVENT_RULE_TRANSITION_SPAN.saturating_add(Duration::from_millis(400));

/// Regions the rows draw from. Catalog source: the `Region` shape the model
/// declares for the members of `Regions` — `@length(2..25)` and
/// `@pattern("^([a-z]{1,4})-([a-z]{1,15}-)+([0-9])$")` — so the rows differ in
/// prefix length, middle-segment length, segment count and total length.
const COMMONPLACE_REGION: &str = "us-east-1";
const SECOND_REGION: &str = "eu-west-1";
const FOUR_SEGMENTS: &str = "us-gov-east-1";

/// A well-formed `NotificationConfigurationArn` this emulator never minted — the
/// arn pattern is satisfied, so a refusal comes from the handler rather than
/// from the boundary validator.
const UNMINTED_CONFIGURATION_ARN: &str =
    "arn:aws:notifications::123456789012:configuration/abcdefghijklmnopqrstuvwxyz0";

/// One row of the delete table.
struct DeleteCase {
    case: &'static str,
    /// `source` of the rule the row deletes.
    source: String,
    /// `eventType` of the rule the row deletes.
    event_type: String,
    /// The `eventPattern` the deleted rule carries, or `None` when the create
    /// omitted the member entirely.
    event_pattern: Option<String>,
    /// The Regions the deleted rule covers.
    regions: Vec<String>,
    /// How many rules the configuration owns when the delete runs.
    rules: usize,
    /// Which of those rules the row deletes, in creation order (0 is the
    /// oldest).
    delete_index: usize,
    /// Whether the row waits past the transition span before deleting, so the
    /// rule is settled rather than still in the status the create minted it in.
    settled: bool,
}

/// The table. Rows differ in the deleted rule's Region count and Region shape,
/// in whether it carries an `eventPattern`, in how many rules its configuration
/// owns, in which of them is deleted, and in whether the rule had settled.
fn cases() -> Vec<DeleteCase> {
    // Catalog source: `Source`'s `@length(1..36)` and
    // `@pattern("^aws.([a-z0-9\\-])+$")`, and `EventType`'s `@length(1..128)`
    // and `@pattern("^([a-zA-Z0-9 \\-\\(\\)])+$")`.
    let longest_source = format!("aws.{}", "a".repeat(32));
    let longest_event_type = format!("Event Type {}", "E".repeat(117));

    vec![
        DeleteCase {
            case: "the only rule of its configuration, still transitional",
            source: "aws.ec2".to_string(),
            event_type: "EC2 Instance State-change Notification".to_string(),
            event_pattern: None,
            regions: vec![COMMONPLACE_REGION.to_string()],
            rules: 1,
            delete_index: 0,
            settled: false,
        },
        DeleteCase {
            case: "the only rule of its configuration, settled",
            source: "aws.cloudwatch".to_string(),
            event_type: "CloudWatch Alarm State Change".to_string(),
            event_pattern: Some(r#"{"detail":{"state":{"value":["ALARM"]}}}"#.to_string()),
            regions: vec![COMMONPLACE_REGION.to_string(), SECOND_REGION.to_string()],
            rules: 1,
            delete_index: 0,
            settled: true,
        },
        DeleteCase {
            case: "the oldest of three rules",
            source: "aws.health".to_string(),
            event_type: "AWS Health Event".to_string(),
            event_pattern: Some(r#"{"detail":{"eventTypeCategory":["issue"]}}"#.to_string()),
            regions: vec![FOUR_SEGMENTS.to_string()],
            rules: 3,
            delete_index: 0,
            settled: false,
        },
        DeleteCase {
            case: "the middle of three rules, settled",
            source: longest_source,
            event_type: "Tag Change on Resource".to_string(),
            event_pattern: None,
            regions: vec![
                COMMONPLACE_REGION.to_string(),
                SECOND_REGION.to_string(),
                FOUR_SEGMENTS.to_string(),
            ],
            rules: 3,
            delete_index: 1,
            settled: true,
        },
        DeleteCase {
            case: "the newest of four rules, with the longest eventType",
            source: "aws.s3".to_string(),
            event_type: longest_event_type,
            event_pattern: Some(String::new()),
            regions: vec![SECOND_REGION.to_string()],
            rules: 4,
            delete_index: 3,
            settled: false,
        },
    ]
}

/// Create a NotificationConfiguration for rules to hang off, and return its arn.
async fn create_delete_host(emulator: &Emulator, name: &str) -> String {
    let (status, response) = emulator
        .send(
            "POST",
            "/notification-configurations",
            Some(json!({ "name": name, "description": "a configuration under test" })),
        )
        .await;
    assert_eq!(
        status, 201,
        "the configuration a rule hangs off must be created, got {status} {response:?}"
    );
    response["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("a create must answer an arn, got {response:?}"))
        .to_string()
}

/// `CreateEventRule` with the body the caller passes, answering its status and
/// body.
async fn create_rule(emulator: &Emulator, body: Value) -> (u16, Value) {
    emulator.send("POST", "/event-rules", Some(body)).await
}

/// Create one rule and return its arn, asserting the create succeeded.
async fn create_rule_arn(emulator: &Emulator, body: Value) -> String {
    let (status, created) = create_rule(emulator, body).await;
    assert_eq!(
        status, 201,
        "the rule under test must be created, got {status} {created:?}"
    );
    let arn = created["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("a create must answer an arn, got {created:?}"))
        .to_string();
    assert!(
        is_event_rule_arn(&arn),
        "a created arn matches the modelled EventRuleArn pattern, got {arn:?}"
    );
    arn
}

/// `DeleteEventRule` for one rule arn.
async fn delete_event_rule(emulator: &Emulator, arn: &str) -> (u16, Value) {
    let uri = format!("/event-rules/{}", encode_path_label(arn));
    emulator.send("DELETE", &uri, None).await
}

/// The arns `ListEventRules` reports for one configuration, newest first.
async fn listed_rule_arns(emulator: &Emulator, case: &str, configuration: &str) -> Vec<String> {
    let uri = format!(
        "/event-rules?notificationConfigurationArn={}",
        encode_path_label(configuration)
    );
    let (status, listed) = emulator.send("GET", &uri, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: a listing answers 200, got {status} {listed:?}"
    );
    listed["eventRules"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: a listing answers an eventRules list, got {listed:?}")
        })
        .iter()
        .map(|entry| entry["arn"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// `GetNotificationConfiguration` for one configuration arn.
async fn get_configuration(emulator: &Emulator, arn: &str) -> (u16, Value) {
    let uri = format!("/notification-configurations/{}", encode_path_label(arn));
    emulator.send("GET", &uri, None).await
}

/// The configuration arns `ListNotificationConfigurations` selects for one
/// `eventRuleSource` filter value.
async fn configurations_with_source(emulator: &Emulator, case: &str, source: &str) -> Vec<String> {
    let uri = format!("/notification-configurations?eventRuleSource={source}");
    let (status, listed) = emulator.send("GET", &uri, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: a filtered listing answers 200, got {status} {listed:?}"
    );
    listed["notificationConfigurations"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: a listing answers a notificationConfigurations list, got {listed:?}")
        })
        .iter()
        .map(|entry| entry["arn"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The request body for one row's own rule, omitting `eventPattern` entirely
/// when the row supplies none.
fn target_body(case: &DeleteCase, configuration_arn: &str) -> Value {
    let mut body = json!({
        "notificationConfigurationArn": configuration_arn,
        "source": case.source,
        "eventType": case.event_type,
        "regions": case.regions,
    });
    if let Some(pattern) = &case.event_pattern {
        body["eventPattern"] = json!(pattern);
    }
    body
}

/// The request body for a filler rule: a pair no row's own rule covers, so every
/// rule of a configuration asks for a rule of its own.
fn filler_body(configuration_arn: &str, index: usize) -> Value {
    json!({
        "notificationConfigurationArn": configuration_arn,
        "source": "aws.config",
        "eventType": format!("Filler Event {index}"),
        "regions": [COMMONPLACE_REGION],
    })
}

/// RULE: a delete keyed by a resource's arn removes exactly the record that arn
/// names — so a later read of it is the modelled 404, its parent stops listing
/// it, and nothing else in the store moves — whatever that record carries and
/// whatever else the store holds.
#[tokio::test]
async fn a_delete_removes_exactly_the_rule_its_arn_names() {
    for case in cases() {
        let label = case.case;
        let emulator = Emulator::new();
        let configuration = create_delete_host(&emulator, "delete-host").await;

        // A second configuration carrying a rule of its own: a delete must not
        // reach it.
        let other = create_delete_host(&emulator, "other-delete-host").await;
        let foreign = create_rule_arn(&emulator, filler_body(&other, 99)).await;

        // The configuration's rules in creation order, the row's own rule at
        // its chosen index.
        let mut arns: Vec<String> = Vec::new();
        for index in 0..case.rules {
            let body = if index == case.delete_index {
                target_body(&case, &configuration)
            } else {
                filler_body(&configuration, index)
            };
            arns.push(create_rule_arn(&emulator, body).await);
        }
        let target = arns[case.delete_index].clone();
        let survivors: Vec<String> = arns
            .iter()
            .filter(|arn| **arn != target)
            .cloned()
            .collect();

        if case.settled {
            tokio::time::sleep(PAST_THE_TRANSITION).await;
        }

        let before = stored_event_rule_count(&emulator);
        assert_eq!(
            before,
            case.rules + 1,
            "case {label:?}: the store holds every created rule before the delete"
        );

        let (status, deleted) = delete_event_rule(&emulator, &target).await;
        assert_eq!(
            status, 200,
            "case {label:?}: a delete of a stored rule answers the modelled 200, got {status} {deleted:?}"
        );
        assert!(
            deleted.as_object().is_some_and(serde_json::Map::is_empty) || deleted == Value::Null,
            "case {label:?}: the delete answers no members, got {deleted:?}"
        );

        // The record the arn named is gone.
        let (status, gone) = get_event_rule(&emulator, &target).await;
        assert_eq!(
            status, 404,
            "case {label:?}: a Get of the deleted arn answers the modelled 404, got {status} {gone:?}"
        );
        assert!(
            error_code(&gone).contains("ResourceNotFoundException"),
            "case {label:?}: that refusal is a ResourceNotFoundException, got {gone:?}"
        );
        assert_eq!(
            gone["message"],
            json!(not_found_message(
                EVENT_RULE_RESOURCE_LABEL,
                ARN_IDENTIFIER_LABEL,
                &target
            )),
            "case {label:?}: the read's refusal states the resource that was looked up, got {gone:?}"
        );

        // Nothing else moved: every other rule is still readable, the listing
        // reports exactly the survivors newest first, and the store shrank by
        // one.
        for survivor in &survivors {
            let (status, kept) = get_event_rule(&emulator, survivor).await;
            assert_eq!(
                status, 200,
                "case {label:?}: the sibling {survivor:?} is untouched, got {status} {kept:?}"
            );
        }
        let (status, kept) = get_event_rule(&emulator, &foreign).await;
        assert_eq!(
            status, 200,
            "case {label:?}: another configuration's rule is untouched, got {status} {kept:?}"
        );
        let expected_listing: Vec<String> = survivors.iter().rev().cloned().collect();
        assert_eq!(
            listed_rule_arns(&emulator, label, &configuration).await,
            expected_listing,
            "case {label:?}: the listing reports exactly the surviving rules, newest first"
        );
        assert_eq!(
            stored_event_rule_count(&emulator),
            before - 1,
            "case {label:?}: a delete removes exactly one record"
        );

        // The configuration the rule belonged to is not deleted with it.
        let (status, host) = get_configuration(&emulator, &configuration).await;
        assert_eq!(
            status, 200,
            "case {label:?}: deleting a rule leaves its configuration, got {status} {host:?}"
        );

        // The record is gone, so repeating the delete has nothing to remove.
        let (status, again) = delete_event_rule(&emulator, &target).await;
        assert_eq!(
            status, 404,
            "case {label:?}: a second delete of the same arn answers the modelled 404, got {status} {again:?}"
        );
        assert!(
            error_code(&again).contains("ResourceNotFoundException"),
            "case {label:?}: that refusal is a ResourceNotFoundException, got {again:?}"
        );
        assert_eq!(
            again["message"],
            json!(not_found_message(
                EVENT_RULE_MUTATION_RESOURCE_LABEL,
                ARN_IDENTIFIER_LABEL,
                &target
            )),
            "case {label:?}: the mutation's refusal states the resource that was looked up, got {again:?}"
        );
        assert_eq!(
            stored_event_rule_count(&emulator),
            before - 1,
            "case {label:?}: a refused delete removes nothing further"
        );
    }
}

/// RULE: a deleted record holds nothing any more — neither the room it occupied
/// under its container's ceiling nor the identity it reserved there — so a
/// create the stored record would have conflicted with is served once it is
/// gone.
#[tokio::test]
async fn a_delete_frees_the_pair_and_the_room_the_rule_held() {
    // Rows differ in the pair re-created after the delete, and in whether the
    // re-create asks for the same Regions and pattern as the rule that is gone.
    let rows = [
        (
            "the same pair with the same members",
            json!({
                "source": "aws.ec2",
                "eventType": "EC2 Instance State-change Notification",
                "regions": [COMMONPLACE_REGION],
            }),
        ),
        (
            "the same pair with different Regions",
            json!({
                "source": "aws.cloudwatch",
                "eventType": "CloudWatch Alarm State Change",
                "regions": [COMMONPLACE_REGION, FOUR_SEGMENTS],
            }),
        ),
    ];

    for (label, members) in rows {
        let emulator = Emulator::new();
        let configuration = create_delete_host(&emulator, "freed-pair-host").await;
        let mut body = members.clone();
        body["notificationConfigurationArn"] = json!(configuration);

        let first = create_rule_arn(&emulator, body.clone()).await;

        // While the rule is stored, the pair it covers is taken.
        let (status, conflicted) = create_rule(&emulator, body.clone()).await;
        assert_eq!(
            status, 409,
            "case {label:?}: a create for a covered pair conflicts, got {status} {conflicted:?}"
        );
        assert!(
            error_code(&conflicted).contains("ConflictException"),
            "case {label:?}: that refusal is a ConflictException, got {conflicted:?}"
        );

        let (status, deleted) = delete_event_rule(&emulator, &first).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the delete must succeed, got {status} {deleted:?}"
        );

        // With the rule gone the pair is free again, and the create mints a new
        // record rather than reporting the old one.
        let second = create_rule_arn(&emulator, body).await;
        assert_ne!(
            second, first,
            "case {label:?}: the re-create mints a rule of its own, got {second:?}"
        );
        let (status, fetched) = get_event_rule(&emulator, &second).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the re-created rule is readable, got {status} {fetched:?}"
        );
        assert_eq!(
            fetched["source"], members["source"],
            "case {label:?}: the re-created rule covers the pair that was asked for, got {fetched:?}"
        );
        assert_eq!(
            fetched["eventType"], members["eventType"],
            "case {label:?}: the re-created rule covers the pair that was asked for, got {fetched:?}"
        );
        assert_eq!(
            stored_event_rule_count(&emulator),
            1,
            "case {label:?}: the store holds the re-created rule only"
        );
    }
}

/// RULE: the same freeing holds at the container's ceiling — a configuration
/// holds at most `MAX_EVENT_RULES_PER_CONFIGURATION` rules, so a create refused
/// for the ceiling is served once a delete has made room.
#[tokio::test]
async fn a_delete_makes_room_under_the_per_configuration_ceiling() {
    let emulator = Emulator::new();
    let configuration = create_delete_host(&emulator, "ceiling-host").await;

    let mut arns: Vec<String> = Vec::new();
    for index in 0..MAX_EVENT_RULES_PER_CONFIGURATION {
        arns.push(create_rule_arn(&emulator, filler_body(&configuration, index)).await);
    }

    let past = MAX_EVENT_RULES_PER_CONFIGURATION;
    let (status, refused) = create_rule(&emulator, filler_body(&configuration, past)).await;
    assert_eq!(
        status, 402,
        "case at the ceiling: a create past it answers the modelled 402, got {status} {refused:?}"
    );
    assert!(
        error_code(&refused).contains("ServiceQuotaExceededException"),
        "case at the ceiling: that refusal is a ServiceQuotaExceededException, got {refused:?}"
    );

    let (status, deleted) = delete_event_rule(&emulator, &arns[0]).await;
    assert_eq!(
        status, 200,
        "case at the ceiling: the delete must succeed, got {status} {deleted:?}"
    );
    let (status, created) = create_rule(&emulator, filler_body(&configuration, past)).await;
    assert_eq!(
        status, 201,
        "case after a delete: the freed room admits one create, got {status} {created:?}"
    );
    assert_eq!(
        stored_event_rule_count(&emulator),
        MAX_EVENT_RULES_PER_CONFIGURATION,
        "case after a delete: the store is back at the ceiling, not past it"
    );
}

/// RULE: a delete keyed by an arn the store holds no record under is the
/// modelled 404 naming what was looked up, and removes nothing — whatever the
/// shape of that arn.
#[tokio::test]
async fn a_delete_of_an_arn_no_rule_is_stored_under_is_not_found() {
    let emulator = Emulator::new();
    let configuration = create_delete_host(&emulator, "delete-host").await;
    let kept = create_rule_arn(&emulator, filler_body(&configuration, 0)).await;
    let rule_id = kept.rsplit('/').next().unwrap_or_default().to_string();

    // A rule created and then deleted, so its arn is one this emulator really
    // minted and no longer holds.
    let removed = create_rule_arn(&emulator, filler_body(&configuration, 1)).await;
    let (status, response) = delete_event_rule(&emulator, &removed).await;
    assert_eq!(
        status, 200,
        "the rule under test must be deleted first, got {status} {response:?}"
    );

    // A rule under a configuration that was deleted with its rules: the arn is
    // minted, the configuration is gone.
    let doomed = create_delete_host(&emulator, "doomed-host").await;
    let orphan = create_rule_arn(&emulator, filler_body(&doomed, 2)).await;
    let uri = format!("/notification-configurations/{}", encode_path_label(&doomed));
    let (status, response) = emulator.send("DELETE", &uri, None).await;
    assert_eq!(
        status, 200,
        "the configuration under test must be deleted, got {status} {response:?}"
    );

    // Rows differ in HOW the arn misses: a rule id never minted under a known
    // configuration, a known rule id under a configuration never minted, a rule
    // this emulator minted and deleted, and a rule whose configuration was
    // deleted. Every row satisfies the modelled `EventRuleArn` pattern, so the
    // refusal comes from the handler.
    let rows = [
        (
            "a rule id never minted under a known configuration",
            format!("{configuration}/rule/{}", "z".repeat(27)),
        ),
        (
            "a known rule id under a configuration never minted",
            format!("{UNMINTED_CONFIGURATION_ARN}/rule/{rule_id}"),
        ),
        ("a rule this store minted and deleted", removed),
        ("a rule whose configuration was deleted", orphan),
    ];

    let before = stored_event_rule_count(&emulator);
    for (label, arn) in rows {
        assert!(
            is_event_rule_arn(&arn),
            "case {label:?}: the arn under test must be well formed, got {arn:?}"
        );
        let (status, refused) = delete_event_rule(&emulator, &arn).await;
        assert_eq!(
            status, 404,
            "case {label:?}: a delete of an unknown arn answers the modelled 404, got {status} {refused:?}"
        );
        assert!(
            error_code(&refused).contains("ResourceNotFoundException"),
            "case {label:?}: that refusal is a ResourceNotFoundException, got {refused:?}"
        );
        assert_eq!(
            refused["message"],
            json!(not_found_message(
                EVENT_RULE_MUTATION_RESOURCE_LABEL,
                ARN_IDENTIFIER_LABEL,
                &arn
            )),
            "case {label:?}: the refusal states the resource that was looked up, got {refused:?}"
        );
        assert_eq!(
            stored_event_rule_count(&emulator),
            before,
            "case {label:?}: a refused delete removes nothing"
        );
    }

    // The rule that does exist is still readable afterwards.
    let (status, still_there) = get_event_rule(&emulator, &kept).await;
    assert_eq!(
        status, 200,
        "case a surviving rule: a refused delete changes nothing, got {status} {still_there:?}"
    );
}

/// RULE: everything a configuration derives from the rules it owns is a view of
/// the rules the store holds for it, so a delete moves the derived status and
/// the `eventRuleSource` the listing filter selects on with it.
#[tokio::test]
async fn a_delete_moves_what_the_configuration_derives_from_its_rules() {
    let emulator = Emulator::new();
    let configuration = create_delete_host(&emulator, "derived-host").await;

    // Two rules of the configuration, each with a source of its own so the
    // filter has something to tell apart.
    let first_source = "aws.ec2";
    let second_source = "aws.cloudwatch";
    let first = create_rule_arn(
        &emulator,
        json!({
            "notificationConfigurationArn": configuration,
            "source": first_source,
            "eventType": "EC2 Instance State-change Notification",
            "regions": [COMMONPLACE_REGION],
        }),
    )
    .await;
    let second = create_rule_arn(
        &emulator,
        json!({
            "notificationConfigurationArn": configuration,
            "source": second_source,
            "eventType": "CloudWatch Alarm State Change",
            "regions": [SECOND_REGION],
        }),
    )
    .await;

    // Settled, so the derived status is the one every rule being ACTIVE gives
    // and a later delete cannot be confused with the clock settling a rule.
    tokio::time::sleep(PAST_THE_TRANSITION).await;
    let (status, host) = get_configuration(&emulator, &configuration).await;
    assert_eq!(
        status, 200,
        "case two settled rules: a read of the configuration answers 200, got {status} {host:?}"
    );
    assert_eq!(
        host["status"],
        json!(CONFIGURATION_STATUS_ACTIVE),
        "case two settled rules: every rule ACTIVE is reported ACTIVE, got {host:?}"
    );
    assert_eq!(
        configurations_with_source(&emulator, "two settled rules", first_source).await,
        vec![configuration.clone()],
        "case two settled rules: the filter selects the configuration on each of its rules' sources"
    );

    // Deleting one rule takes its source out of what the filter selects on, and
    // leaves the other rule's where it was.
    let (status, deleted) = delete_event_rule(&emulator, &first).await;
    assert_eq!(
        status, 200,
        "case one rule deleted: the delete must succeed, got {status} {deleted:?}"
    );
    assert_eq!(
        configurations_with_source(&emulator, "one rule deleted", first_source).await,
        Vec::<String>::new(),
        "case one rule deleted: a source no surviving rule has selects no configuration"
    );
    assert_eq!(
        configurations_with_source(&emulator, "one rule deleted", second_source).await,
        vec![configuration.clone()],
        "case one rule deleted: the surviving rule's source still selects the configuration"
    );
    let (_, host) = get_configuration(&emulator, &configuration).await;
    assert_eq!(
        host["status"],
        json!(CONFIGURATION_STATUS_ACTIVE),
        "case one rule deleted: the remaining rule is ACTIVE, so the configuration is, got {host:?}"
    );

    // Deleting the last rule leaves a configuration that owns none, which is
    // the status no rule being ACTIVE gives.
    let (status, deleted) = delete_event_rule(&emulator, &second).await;
    assert_eq!(
        status, 200,
        "case every rule deleted: the delete must succeed, got {status} {deleted:?}"
    );
    let (status, host) = get_configuration(&emulator, &configuration).await;
    assert_eq!(
        status, 200,
        "case every rule deleted: the configuration itself survives, got {status} {host:?}"
    );
    assert_eq!(
        host["status"],
        json!(CONFIGURATION_STATUS_INACTIVE),
        "case every rule deleted: a configuration owning no rule is INACTIVE, got {host:?}"
    );
    assert_eq!(
        listed_rule_arns(&emulator, "every rule deleted", &configuration).await,
        Vec::<String>::new(),
        "case every rule deleted: the listing reports no rule"
    );
    assert_eq!(
        stored_event_rule_count(&emulator),
        0,
        "case every rule deleted: the store holds no rule"
    );
}
