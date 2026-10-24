// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the `EventRule` lifecycle: `CreateEventRule` and the
//! `GetEventRule` that reads a created rule back, driven over HTTP through the
//! same multi-service router the emulator boots in production.
//!
//! Every case asserts the RULE the operations follow, not one recorded request.
//! The table rows vary along what those rules generalise over — how many Regions
//! the rule covers, which Regions (segment count and both `@length` bounds of the
//! modelled `Region`), whether an `eventPattern` is supplied, empty or absent,
//! and how long the `source` and `eventType` are (both `@length` bounds of each)
//! — and the catalogs the rows draw from are the model's own constraints and the
//! service's own modelled value set (`EVENT_RULE_STATUS_VALUES`) and bound
//! (`MAX_EVENT_RULES_PER_CONFIGURATION`), so a model change moves the test with
//! it.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_notifications_notifications::ops::types::EVENT_RULE_STATUS_VALUES;
use service_com_amazonaws_notifications_notifications::state::{
    EVENT_RULE_REASON_ACTIVE, EVENT_RULE_REASON_CREATING, EVENT_RULE_STATUS_ACTIVE,
    EVENT_RULE_STATUS_CREATING, EVENT_RULE_TRANSITION_SPAN, MAX_EVENT_RULES_PER_CONFIGURATION,
    MAX_REGIONS_PER_EVENT_RULE,
};
use service_com_amazonaws_notifications_notifications::ComAmazonawsNotificationsNotificationsState;

// The HTTP harness this suite drives the service through, shared with every
// other suite in this crate.
use crate::harness::{encode_path_label, error_code, is_an_instant, is_event_rule_arn, Emulator};

/// Long enough that the settling the create made due has certainly fallen due,
/// without depending on how slow the machine is.
const PAST_THE_TRANSITION: Duration =
    EVENT_RULE_TRANSITION_SPAN.saturating_add(Duration::from_millis(400));

/// Regions the rows draw from. Catalog source: the `Region` shape the model
/// declares for the members of `Regions` — `@length(2..25)` and
/// `@pattern("^([a-z]{1,4})-([a-z]{1,15}-)+([0-9])$")` — so the rows differ in
/// prefix length, middle-segment length, segment count and total length, and
/// include Regions no issue cited.
const COMMONPLACE_REGION: &str = "us-east-1";
const SECOND_REGION: &str = "eu-west-1";
const LONGER_MIDDLE_SEGMENT: &str = "ap-southeast-2";
const FOUR_SEGMENTS: &str = "us-gov-east-1";
/// The shortest Region the pattern and `@length` floor admit.
const SHORTEST_REGION: &str = "a-b-1";
/// The longest Region the `@length(..25)` ceiling admits, at 25 characters.
const LONGEST_REGION: &str = "abcd-efghijklmnop-opqrs-1";
/// A Region no row lists, so adding it to a row always widens that row's Region
/// set into a different request.
const UNLISTED_REGION: &str = "ca-central-1";

/// A well-formed `NotificationConfigurationArn` this emulator never minted —
/// the arn pattern is satisfied, so the refusal comes from the handler rather
/// than from the boundary validator.
const UNMINTED_CONFIGURATION_ARN: &str =
    "arn:aws:notifications::123456789012:configuration/abcdefghijklmnopqrstuvwxyz0";

/// One row of the create/get table.
struct RuleCase {
    case: &'static str,
    source: String,
    event_type: String,
    /// The `eventPattern` the request carries, or `None` when it omits the
    /// member entirely.
    event_pattern: Option<String>,
    /// The `regions` the request carries, in the order it lists them.
    regions: Vec<String>,
    /// The Regions the rule covers: the request's Regions in order, without
    /// repeats — `statusSummaryByRegion` is keyed by Region, so a Region named
    /// twice in one request is one Region.
    distinct_regions: Vec<String>,
}

/// The table. Rows differ in Region count, Region shape, whether an
/// `eventPattern` is absent / empty / a real pattern, and the length of `source`
/// and `eventType` at both of each member's `@length` bounds.
fn cases() -> Vec<RuleCase> {
    // Catalog source: `Source`'s `@length(1..36)` and
    // `@pattern("^aws.([a-z0-9\\-])+$")`.
    let longest_source = format!("aws.{}", "a".repeat(32));
    // Catalog source: `EventType`'s `@length(1..128)` and
    // `@pattern("^([a-zA-Z0-9 \\-\\(\\)])+$")`.
    let longest_event_type = format!("Event Type {}", "E".repeat(117));

    vec![
        RuleCase {
            case: "a single Region and no eventPattern",
            source: "aws.ec2".to_string(),
            event_type: "EC2 Instance State-change Notification".to_string(),
            event_pattern: None,
            regions: vec![COMMONPLACE_REGION.to_string()],
            distinct_regions: vec![COMMONPLACE_REGION.to_string()],
        },
        RuleCase {
            case: "two Regions and an eventPattern",
            source: "aws.cloudwatch".to_string(),
            event_type: "CloudWatch Alarm State Change".to_string(),
            event_pattern: Some(
                r#"{"detail":{"state":{"value":["ALARM"]}}}"#.to_string(),
            ),
            regions: vec![COMMONPLACE_REGION.to_string(), SECOND_REGION.to_string()],
            distinct_regions: vec![COMMONPLACE_REGION.to_string(), SECOND_REGION.to_string()],
        },
        RuleCase {
            case: "four Regions, including the shortest and the longest the Region constraints admit",
            source: "aws.health".to_string(),
            event_type: "AWS Health Event".to_string(),
            event_pattern: None,
            regions: vec![
                LONGER_MIDDLE_SEGMENT.to_string(),
                FOUR_SEGMENTS.to_string(),
                SHORTEST_REGION.to_string(),
                LONGEST_REGION.to_string(),
            ],
            distinct_regions: vec![
                LONGER_MIDDLE_SEGMENT.to_string(),
                FOUR_SEGMENTS.to_string(),
                SHORTEST_REGION.to_string(),
                LONGEST_REGION.to_string(),
            ],
        },
        RuleCase {
            case: "the shortest source and eventType the constraints admit",
            source: "aws.s".to_string(),
            event_type: "a".to_string(),
            event_pattern: None,
            regions: vec![COMMONPLACE_REGION.to_string()],
            distinct_regions: vec![COMMONPLACE_REGION.to_string()],
        },
        RuleCase {
            case: "the longest source and eventType the length bounds admit",
            source: longest_source,
            event_type: longest_event_type,
            event_pattern: None,
            regions: vec![SECOND_REGION.to_string(), COMMONPLACE_REGION.to_string()],
            distinct_regions: vec![SECOND_REGION.to_string(), COMMONPLACE_REGION.to_string()],
        },
        RuleCase {
            case: "an empty eventPattern, which the member's 0 length floor admits",
            source: "aws.s3".to_string(),
            event_type: "Object Created".to_string(),
            event_pattern: Some(String::new()),
            regions: vec![COMMONPLACE_REGION.to_string()],
            distinct_regions: vec![COMMONPLACE_REGION.to_string()],
        },
        RuleCase {
            case: "a Region named twice, which the status map keys once",
            source: "aws.ec2".to_string(),
            event_type: "EC2 Instance State-change Notification".to_string(),
            event_pattern: None,
            regions: vec![
                COMMONPLACE_REGION.to_string(),
                SECOND_REGION.to_string(),
                COMMONPLACE_REGION.to_string(),
            ],
            distinct_regions: vec![COMMONPLACE_REGION.to_string(), SECOND_REGION.to_string()],
        },
    ]
}

/// The EventRule-specific helpers this suite adds to the shared harness.
impl Emulator {
    /// Create a NotificationConfiguration for a rule to hang off, and return its
    /// arn.
    async fn create_configuration(&self, name: &str) -> String {
        let (status, response) = self
            .send(
                "POST",
                "/notification-configurations",
                Some(json!({ "name": name, "description": "a configuration under test" })),
            )
            .await;
        assert_eq!(
            status, 201,
            "the configuration a rule hangs off must be created, got {response:?}"
        );
        response["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("a create must answer an arn, got {response:?}"))
            .to_string()
    }

    async fn create_event_rule(&self, body: Value) -> (u16, Value) {
        self.send("POST", "/event-rules", Some(body)).await
    }

    async fn get_event_rule(&self, arn: &str) -> (u16, Value) {
        let uri = format!("/event-rules/{}", encode_path_label(arn));
        self.send("GET", &uri, None).await
    }

    async fn delete_configuration_by_arn(&self, arn: &str) -> (u16, Value) {
        let uri = format!("/notification-configurations/{}", encode_path_label(arn));
        self.send("DELETE", &uri, None).await
    }

    async fn list_configurations_by_event_rule_source(&self, source: &str) -> (u16, Value) {
        let uri = format!("/notification-configurations?eventRuleSource={source}");
        self.send("GET", &uri, None).await
    }

    /// `ListEventRules` for one configuration, with a raw extra query string
    /// (empty for an unpaged listing).
    async fn list_event_rules(&self, configuration_arn: &str, extra: &str) -> (u16, Value) {
        let mut uri = format!(
            "/event-rules?notificationConfigurationArn={}",
            encode_path_label(configuration_arn)
        );
        if !extra.is_empty() {
            uri.push('&');
            uri.push_str(extra);
        }
        self.send("GET", &uri, None).await
    }

    /// How many EventRules the store holds, read straight out of it — what a
    /// refused create must not have changed.
    fn stored_event_rule_count(&self) -> usize {
        let state = self
            .engine
            .state::<ComAmazonawsNotificationsNotificationsState>();
        let store = state.lock();
        store.event_rules.len()
    }
}

/// The request body for one row, omitting `eventPattern` entirely when the row
/// supplies none.
fn request_body(case: &RuleCase, configuration_arn: &str) -> Value {
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

/// `statusSummaryByRegion` as a Region -> (status, reason) map, with every
/// status checked against the modelled value set.
fn status_map(case: &str, response: &Value) -> BTreeMap<String, (String, String)> {
    let map = response["statusSummaryByRegion"]
        .as_object()
        .unwrap_or_else(|| {
            panic!("case {case:?}: every report of a rule carries statusSummaryByRegion, got {response:?}")
        });
    map.iter()
        .map(|(region, summary)| {
            let status = summary["status"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: every Region summary carries a status, got {summary:?}")
                })
                .to_string();
            assert!(
                // Catalog source: the `EventRuleStatus` enum shape, re-exported
                // by the service crate.
                EVENT_RULE_STATUS_VALUES.contains(&status.as_str()),
                "case {case:?}: status must be one of the modelled EventRuleStatus values {EVENT_RULE_STATUS_VALUES:?}, got {status:?}"
            );
            let reason = summary["reason"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: every Region summary carries a reason, got {summary:?}")
                })
                .to_string();
            (region.clone(), (status, reason))
        })
        .collect()
}

/// The Regions `statusSummaryByRegion` is keyed by, sorted so the assertion does
/// not depend on a JSON object's member order.
fn sorted(regions: &[String]) -> Vec<String> {
    let mut sorted = regions.to_vec();
    sorted.sort();
    sorted
}

/// RULE: a create persists one EventRule under the named NotificationConfiguration,
/// keyed by an arn nested under that configuration's, with one
/// `statusSummaryByRegion` entry per distinct Region asked for in the
/// transitional status the model documents (`CREATING`); a Get reports that
/// stored record member for member, and the clock — not a read — settles every
/// Region to `ACTIVE`.
#[tokio::test]
async fn create_persists_an_event_rule_the_get_reports_and_the_clock_settles() {
    for case in cases() {
        let label = case.case;
        let emulator = Emulator::new();
        let configuration = emulator.create_configuration("rule-host").await;

        let (status, created) = emulator
            .create_event_rule(request_body(&case, &configuration))
            .await;
        assert_eq!(
            status, 201,
            "case {label:?}: a create answers the modelled 201, got {created:?}"
        );
        let arn = created["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: a create answers an arn, got {created:?}"))
            .to_string();
        assert!(
            is_event_rule_arn(&arn),
            "case {label:?}: the arn must match the modelled EventRuleArn pattern, got {arn:?}"
        );
        assert!(
            arn.starts_with(&format!("{configuration}/rule/")),
            "case {label:?}: the rule's arn nests under its configuration's {configuration:?}, got {arn:?}"
        );
        assert_eq!(
            created["notificationConfigurationArn"],
            json!(configuration),
            "case {label:?}: the response names the configuration the rule belongs to, got {created:?}"
        );

        let summaries = status_map(label, &created);
        assert_eq!(
            summaries.keys().cloned().collect::<Vec<String>>(),
            sorted(&case.distinct_regions),
            "case {label:?}: statusSummaryByRegion is keyed by the distinct Regions asked for, got {created:?}"
        );
        for (region, (status, reason)) in &summaries {
            assert_eq!(
                status, EVENT_RULE_STATUS_CREATING,
                "case {label:?}: Region {region:?} of a fresh rule is CREATING, got {created:?}"
            );
            assert_eq!(
                reason, EVENT_RULE_REASON_CREATING,
                "case {label:?}: Region {region:?} carries the reason for its status, got {created:?}"
            );
        }

        // The Get reports the stored record: the transitional status is
        // observable to a caller reading straight after the create, and every
        // member the create was given round-trips.
        let (status, fetched) = emulator.get_event_rule(&arn).await;
        assert_eq!(
            status, 200,
            "case {label:?}: a Get of a created rule answers 200, got {fetched:?}"
        );
        assert_eq!(
            fetched["arn"],
            json!(arn),
            "case {label:?}: the Get reports the arn it was asked for, got {fetched:?}"
        );
        assert_eq!(
            fetched["notificationConfigurationArn"],
            json!(configuration),
            "case {label:?}: the Get reports the stored configuration arn, got {fetched:?}"
        );
        assert_eq!(
            fetched["source"],
            json!(case.source),
            "case {label:?}: the Get reports the stored source, got {fetched:?}"
        );
        assert_eq!(
            fetched["eventType"],
            json!(case.event_type),
            "case {label:?}: the Get reports the stored eventType, got {fetched:?}"
        );
        assert_eq!(
            fetched["eventPattern"],
            json!(case.event_pattern.clone().unwrap_or_default()),
            "case {label:?}: the Get reports the stored eventPattern, the empty string when the create supplied none, got {fetched:?}"
        );
        assert_eq!(
            fetched["regions"],
            json!(case.distinct_regions),
            "case {label:?}: the Get reports the Regions asked for, in order and without repeats, got {fetched:?}"
        );
        assert_eq!(
            fetched["managedRules"],
            json!([]),
            "case {label:?}: managedRules is present and empty — this emulator creates no EventBridge rule, so there is none to report, got {fetched:?}"
        );
        assert!(
            is_an_instant(&fetched["creationTime"]),
            "case {label:?}: creationTime is the instant the rule was created, got {fetched:?}"
        );
        assert_eq!(
            status_map(label, &fetched),
            summaries,
            "case {label:?}: the Get reports the stored status summaries, got {fetched:?}"
        );

        // Only the clock moves the rule on.
        tokio::time::sleep(PAST_THE_TRANSITION).await;
        let (status, settled) = emulator.get_event_rule(&arn).await;
        assert_eq!(
            status, 200,
            "case {label:?}: a Get of a settled rule answers 200, got {settled:?}"
        );
        for (region, (status, reason)) in status_map(label, &settled) {
            assert_eq!(
                status, EVENT_RULE_STATUS_ACTIVE,
                "case {label:?}: the clock activates Region {region:?}, got {settled:?}"
            );
            assert_eq!(
                reason, EVENT_RULE_REASON_ACTIVE,
                "case {label:?}: the reason moves with the status in Region {region:?}, got {settled:?}"
            );
        }
        assert_eq!(
            settled["creationTime"], fetched["creationTime"],
            "case {label:?}: creationTime is stamped once and never rewritten, got {settled:?}"
        );

        // A read changes nothing a later reader sees.
        let (_, again) = emulator.get_event_rule(&arn).await;
        assert_eq!(
            again, settled,
            "case {label:?}: a Get must not change what the next Get reports, got {again:?}"
        );
        assert_eq!(
            emulator.stored_event_rule_count(),
            1,
            "case {label:?}: one create makes exactly one rule"
        );
    }
}

/// RULE: an EventRule is created "associated with a specified
/// NotificationConfiguration", so a create naming a configuration the account
/// does not hold is refused with the modelled 404 and persists nothing —
/// whatever the reason the configuration is absent.
#[tokio::test]
async fn creating_a_rule_for_a_configuration_that_does_not_exist_is_not_found() {
    let case = &cases()[0];

    // Rows differ in WHY the configuration is absent: an arn this emulator never
    // minted, and the arn of a configuration that was deleted.
    let emulator = Emulator::new();
    let deleted = emulator.create_configuration("to-be-deleted").await;
    let (status, response) = emulator.delete_configuration_by_arn(&deleted).await;
    assert_eq!(status, 200, "the configuration must be deleted, got {response:?}");

    for (label, configuration) in [
        ("an arn this emulator never minted", UNMINTED_CONFIGURATION_ARN.to_string()),
        ("the arn of a deleted configuration", deleted),
    ] {
        let (status, refused) = emulator
            .create_event_rule(request_body(case, &configuration))
            .await;
        assert_eq!(
            status, 404,
            "case {label:?}: a create for an absent configuration answers the modelled 404, got {refused:?}"
        );
        assert!(
            error_code(&refused).contains("ResourceNotFoundException"),
            "case {label:?}: that refusal is a ResourceNotFoundException, got {refused:?}"
        );
        assert_eq!(
            emulator.stored_event_rule_count(),
            0,
            "case {label:?}: a refused create persists nothing"
        );
    }
}

/// RULE: `CreateEventRule` is `@idempotent` — repeating a request already served
/// reports the rule it made instead of minting a second one, and Region order is
/// not part of what was asked for; a request that differs in any member is a
/// different request and makes its own rule.
#[tokio::test]
async fn a_repeated_create_reports_the_same_rule_and_a_changed_one_makes_another() {
    for case in cases() {
        let label = case.case;
        let emulator = Emulator::new();
        let configuration = emulator.create_configuration("rule-host").await;
        let body = request_body(&case, &configuration);

        let (_, first) = emulator.create_event_rule(body.clone()).await;
        let (status, repeated) = emulator.create_event_rule(body.clone()).await;
        assert_eq!(
            status, 201,
            "case {label:?}: a repeated create stays successful, got {repeated:?}"
        );
        assert_eq!(
            repeated["arn"], first["arn"],
            "case {label:?}: a repeated create reports the rule it already made, got {repeated:?}"
        );
        assert_eq!(
            emulator.stored_event_rule_count(),
            1,
            "case {label:?}: a repeated create does not mint a second rule"
        );

        // The same Regions in another order are the same request: the status map
        // is keyed by Region, so their order carries nothing.
        let mut reordered = body.clone();
        let mut regions = case.regions.clone();
        regions.reverse();
        reordered["regions"] = json!(regions);
        let (status, same) = emulator.create_event_rule(reordered).await;
        assert_eq!(
            status, 201,
            "case {label:?}: the same Regions reordered stays successful, got {same:?}"
        );
        assert_eq!(
            same["arn"], first["arn"],
            "case {label:?}: Region order is not part of the request, got {same:?}"
        );
        assert_eq!(
            emulator.stored_event_rule_count(),
            1,
            "case {label:?}: reordering the Regions does not mint a second rule"
        );

        // A request that differs in a member is a different request. One row
        // per member that identifies a rule.
        let mut different_pattern = body.clone();
        different_pattern["eventPattern"] = json!(r#"{"detail":{"other":["value"]}}"#);
        let mut different_type = body.clone();
        different_type["eventType"] = json!("Another Event Type");
        let mut different_source = body.clone();
        different_source["source"] = json!("aws.other");
        let mut extra_region = body.clone();
        let mut widened = case.distinct_regions.clone();
        assert!(
            !widened.iter().any(|region| region == UNLISTED_REGION),
            "case {label:?}: the widening Region must not already be in the row, got {widened:?}"
        );
        widened.push(UNLISTED_REGION.to_string());
        extra_region["regions"] = json!(widened);

        let mut expected = 1;
        for (member, changed) in [
            ("eventPattern", different_pattern),
            ("eventType", different_type),
            ("source", different_source),
            ("regions", extra_region),
        ] {
            let (status, other) = emulator.create_event_rule(changed).await;
            assert_eq!(
                status, 201,
                "case {label:?}: a create differing in {member} succeeds, got {other:?}"
            );
            assert_ne!(
                other["arn"], first["arn"],
                "case {label:?}: a create differing in {member} is its own rule, got {other:?}"
            );
            expected += 1;
            assert_eq!(
                emulator.stored_event_rule_count(),
                expected,
                "case {label:?}: a create differing in {member} adds one rule"
            );
        }
    }
}

/// RULE: a configuration's EventRules are part of it — their sources are what
/// `ListNotificationConfigurations`'s `eventRuleSource` filter selects a
/// configuration on, and deleting the configuration removes the rules
/// associated with it.
#[tokio::test]
async fn a_created_rule_joins_its_configuration_and_goes_when_that_configuration_does() {
    let emulator = Emulator::new();
    let first = emulator.create_configuration("first-host").await;
    let second = emulator.create_configuration("second-host").await;

    // Two configurations, each carrying a rule with its own source, so the
    // filter has something to tell apart.
    let mut arns = Vec::new();
    for (configuration, case) in [(&first, &cases()[0]), (&second, &cases()[1])] {
        let (status, created) = emulator
            .create_event_rule(request_body(case, configuration))
            .await;
        assert_eq!(
            status, 201,
            "case {:?}: the rule under test must be created, got {created:?}",
            case.case
        );
        arns.push(created["arn"].as_str().unwrap_or_default().to_string());
    }

    for (configuration, case) in [(&first, &cases()[0]), (&second, &cases()[1])] {
        let label = case.case;
        let (status, listed) = emulator
            .list_configurations_by_event_rule_source(&case.source)
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: a filtered listing answers 200, got {listed:?}"
        );
        let selected: Vec<String> = listed["notificationConfigurations"]
            .as_array()
            .unwrap_or_else(|| panic!("case {label:?}: a listing answers a list, got {listed:?}"))
            .iter()
            .map(|entry| entry["arn"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            selected,
            vec![configuration.clone()],
            "case {label:?}: the eventRuleSource filter selects exactly the configuration carrying a rule with that source, got {listed:?}"
        );
    }

    // A source no rule carries selects nothing.
    let (status, none) = emulator
        .list_configurations_by_event_rule_source("aws.unused")
        .await;
    assert_eq!(status, 200, "an unmatched filter answers 200, got {none:?}");
    assert_eq!(
        none["notificationConfigurations"],
        json!([]),
        "a source no rule carries selects no configuration, got {none:?}"
    );

    // Deleting the configuration takes its rules with it, and leaves the other
    // configuration's alone.
    let (status, deleted) = emulator.delete_configuration_by_arn(&first).await;
    assert_eq!(status, 200, "the delete must succeed, got {deleted:?}");
    let (status, gone) = emulator.get_event_rule(&arns[0]).await;
    assert_eq!(
        status, 404,
        "a rule whose configuration was deleted is reported by nobody, got {gone:?}"
    );
    assert!(
        error_code(&gone).contains("ResourceNotFoundException"),
        "that refusal is a ResourceNotFoundException, got {gone:?}"
    );
    let (status, kept) = emulator.get_event_rule(&arns[1]).await;
    assert_eq!(
        status, 200,
        "another configuration's rule is untouched, got {kept:?}"
    );
    assert_eq!(
        emulator.stored_event_rule_count(),
        1,
        "exactly the deleted configuration's rules are gone"
    );
}

/// RULE: a Get keyed by an arn the store holds no rule under answers the
/// modelled 404, whatever the shape of that arn.
#[tokio::test]
async fn a_get_of_an_arn_no_rule_is_stored_under_is_not_found() {
    let emulator = Emulator::new();
    let configuration = emulator.create_configuration("rule-host").await;
    let (_, created) = emulator
        .create_event_rule(request_body(&cases()[0], &configuration))
        .await;
    let created_arn = created["arn"].as_str().unwrap_or_default().to_string();

    // Rows differ in how the arn misses: a rule id this emulator never minted
    // under a configuration it did, and a rule id it did mint under a
    // configuration it did not. Both satisfy the modelled `EventRuleArn`
    // pattern, so the refusal comes from the handler.
    let unminted_rule = format!("{configuration}/rule/{}", "z".repeat(27));
    let foreign_configuration = format!(
        "{UNMINTED_CONFIGURATION_ARN}/rule/{}",
        created_arn.rsplit('/').next().unwrap_or_default()
    );
    for (label, arn) in [
        ("a rule id never minted under a known configuration", unminted_rule),
        ("a known rule id under a configuration never minted", foreign_configuration),
    ] {
        assert!(
            is_event_rule_arn(&arn),
            "case {label:?}: the arn under test must be well formed, got {arn:?}"
        );
        let (status, refused) = emulator.get_event_rule(&arn).await;
        assert_eq!(
            status, 404,
            "case {label:?}: a Get of an unknown arn answers the modelled 404, got {refused:?}"
        );
        assert!(
            error_code(&refused).contains("ResourceNotFoundException"),
            "case {label:?}: that refusal is a ResourceNotFoundException, got {refused:?}"
        );
    }

    // The rule that does exist is still readable afterwards.
    let (status, kept) = emulator.get_event_rule(&created_arn).await;
    assert_eq!(status, 200, "a refused Get changes nothing, got {kept:?}");
}

/// RULE: a configuration holds at most `MAX_EVENT_RULES_PER_CONFIGURATION`
/// EventRules, so a create past that ceiling is refused with the modelled quota
/// error instead of growing the store — and another configuration's capacity is
/// its own.
#[tokio::test]
async fn a_create_past_the_cap_is_refused_with_the_quota_error() {
    let emulator = Emulator::new();
    let configuration = emulator.create_configuration("rule-host").await;

    // Each create differs in `eventType`, so none of them is a repeat of
    // another and every one asks for a new rule.
    let body_for = |index: usize| {
        json!({
            "notificationConfigurationArn": configuration,
            "source": "aws.ec2",
            "eventType": format!("Event Type {index}"),
            "regions": [COMMONPLACE_REGION],
        })
    };
    for index in 0..MAX_EVENT_RULES_PER_CONFIGURATION {
        let (status, created) = emulator.create_event_rule(body_for(index)).await;
        assert_eq!(
            status, 201,
            "case {index}: a create below the cap succeeds, got {created:?}"
        );
    }

    let past = MAX_EVENT_RULES_PER_CONFIGURATION;
    let (status, refused) = emulator.create_event_rule(body_for(past)).await;
    assert_eq!(
        status, 402,
        "case {past}: a create past the cap answers the modelled 402, got {refused:?}"
    );
    assert!(
        error_code(&refused).contains("ServiceQuotaExceededException"),
        "case {past}: a create past the cap is a ServiceQuotaExceededException, got {refused:?}"
    );
    assert_eq!(
        emulator.stored_event_rule_count(),
        MAX_EVENT_RULES_PER_CONFIGURATION,
        "a refused create does not grow the store"
    );

    // A repeat of a create already served is still served at the cap: it asks
    // for no new rule.
    let (status, repeated) = emulator.create_event_rule(body_for(0)).await;
    assert_eq!(
        status, 201,
        "a repeat of an existing rule is served at the cap, got {repeated:?}"
    );

    // The cap is per configuration.
    let other = emulator.create_configuration("other-host").await;
    let (status, created) = emulator
        .create_event_rule(request_body(&cases()[0], &other))
        .await;
    assert_eq!(
        status, 201,
        "another configuration's capacity is its own, got {created:?}"
    );
}

/// RULE: one EventRule covers at most `MAX_REGIONS_PER_EVENT_RULE` Regions — the
/// modelled `Regions` list declares a floor and no ceiling — so a create whose
/// list is longer is refused with the modelled 400 naming the member, and
/// persists nothing; a list at the ceiling is served and reports one status
/// entry per Region.
#[tokio::test]
async fn a_create_naming_more_regions_than_one_rule_covers_is_refused() {
    let emulator = Emulator::new();
    let configuration = emulator.create_configuration("rule-host").await;
    let body_for = |regions: Vec<String>| {
        json!({
            "notificationConfigurationArn": configuration,
            "source": "aws.ec2",
            "eventType": "EC2 Instance State-change Notification",
            "regions": regions,
        })
    };

    // A list exactly at the ceiling is served, and every Region in it is
    // reported.
    let at_the_cap = generated_regions(MAX_REGIONS_PER_EVENT_RULE);
    let (status, created) = emulator.create_event_rule(body_for(at_the_cap.clone())).await;
    assert_eq!(
        status, 201,
        "case {MAX_REGIONS_PER_EVENT_RULE}: a list at the ceiling is served, got {created:?}"
    );
    assert_eq!(
        status_map("at the ceiling", &created)
            .keys()
            .cloned()
            .collect::<Vec<String>>(),
        sorted(&at_the_cap),
        "case {MAX_REGIONS_PER_EVENT_RULE}: every Region asked for is reported, got {created:?}"
    );

    // Rows differ in how far past the ceiling the list reaches: one Region past
    // it, and far past it.
    for over in [
        MAX_REGIONS_PER_EVENT_RULE + 1,
        MAX_REGIONS_PER_EVENT_RULE * 4,
    ] {
        let (status, refused) = emulator.create_event_rule(body_for(generated_regions(over))).await;
        assert_eq!(
            status, 400,
            "case {over}: a list past the ceiling answers the modelled 400, got {refused:?}"
        );
        assert!(
            error_code(&refused).contains("ValidationException"),
            "case {over}: that refusal is a ValidationException, got {refused:?}"
        );
        assert!(
            refused["message"].as_str().unwrap_or_default().contains("regions"),
            "case {over}: the refusal names the member that is out of bounds, got {refused:?}"
        );
        assert_eq!(
            emulator.stored_event_rule_count(),
            1,
            "case {over}: a refused create persists nothing"
        );
    }
}

/// `count` distinct Regions, each satisfying the modelled `Region`
/// `@pattern("^([a-z]{1,4})-([a-z]{1,15}-)+([0-9])$")` and `@length(2..25)`, so
/// a list longer than any catalog written by hand still reaches the handler
/// rather than the boundary validator.
fn generated_regions(count: usize) -> Vec<String> {
    (0..count)
        .map(|index| {
            let first = char::from(b'a' + (index / 26) as u8);
            let second = char::from(b'a' + (index % 26) as u8);
            format!("ab-{first}{second}-{}", index % 10)
        })
        .collect()
}

/// RULE: `ListEventRules` reports the EventRules OF the configuration it names, newest
/// first, as the same records a Get reports — and nothing of another configuration's.
/// A page carries at most `maxResults` entries and the pages of one listing concatenate
/// to the whole set; an arn no configuration is stored under is the modelled 404.
#[tokio::test]
async fn the_listing_reports_a_configurations_own_rules_newest_first() {
    let emulator = Emulator::new();
    let configuration = emulator.create_configuration("listed-host").await;
    let other = emulator.create_configuration("other-listed-host").await;

    // Rows differ in how many rules the configuration owns, so the listing is
    // exercised empty, with one rule and with several.
    let (status, empty) = emulator.list_event_rules(&configuration, "").await;
    assert_eq!(status, 200, "case empty: a listing answers 200, got {empty:?}");
    assert_eq!(
        empty["eventRules"],
        json!([]),
        "case empty: a configuration with no rule lists none, got {empty:?}"
    );

    // Five rules, each differing in `eventType` so none is a repeat of another, in
    // creation order.
    let mut created_oldest_first = Vec::new();
    for index in 0..5usize {
        let (status, created) = emulator
            .create_event_rule(json!({
                "notificationConfigurationArn": configuration,
                "source": "aws.ec2",
                "eventType": format!("Listed Event {index}"),
                "regions": [COMMONPLACE_REGION],
            }))
            .await;
        assert_eq!(
            status, 201,
            "case {index}: the rule under test must be created, got {created:?}"
        );
        created_oldest_first.push(created["arn"].as_str().unwrap_or_default().to_string());
    }
    // The other configuration carries one rule, which must never be listed here.
    let (_, foreign) = emulator
        .create_event_rule(request_body(&cases()[1], &other))
        .await;
    let foreign_arn = foreign["arn"].as_str().unwrap_or_default().to_string();

    let newest_first: Vec<String> = created_oldest_first.iter().rev().cloned().collect();

    for page_size in [1usize, 2, 4, 5, 1000] {
        let mut collected: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            pages += 1;
            assert!(
                pages <= newest_first.len() + 1,
                "case maxResults={page_size}: paging did not terminate after {pages} pages"
            );
            let extra = match &token {
                Some(value) => format!("maxResults={page_size}&nextToken={value}"),
                None => format!("maxResults={page_size}"),
            };
            let (status, page) = emulator.list_event_rules(&configuration, &extra).await;
            assert_eq!(
                status, 200,
                "case maxResults={page_size}: a listing answers 200, got {page:?}"
            );
            let entries = page["eventRules"]
                .as_array()
                .unwrap_or_else(|| {
                    panic!("case maxResults={page_size}: eventRules is required, got {page:?}")
                })
                .clone();
            assert!(
                entries.len() <= page_size,
                "case maxResults={page_size}: a page must not exceed maxResults, got {} entries",
                entries.len()
            );
            for entry in &entries {
                let arn = entry["arn"].as_str().unwrap_or_default().to_string();
                assert!(
                    is_event_rule_arn(&arn),
                    "case maxResults={page_size}: a listed arn matches the modelled pattern, got {arn:?}"
                );
                assert_ne!(
                    arn, foreign_arn,
                    "case maxResults={page_size}: another configuration's rule must not be listed, got {entry:?}"
                );
                assert_eq!(
                    entry["notificationConfigurationArn"].as_str(),
                    Some(configuration.as_str()),
                    "case maxResults={page_size}: a listed entry names the configuration it belongs to, got {entry:?}"
                );
                // The listing reports the same record the Get reports.
                let (status, got) = emulator.get_event_rule(&arn).await;
                assert_eq!(
                    status, 200,
                    "case maxResults={page_size}: a listed rule is readable, got {got:?}"
                );
                for member in [
                    "arn",
                    "notificationConfigurationArn",
                    "source",
                    "eventType",
                    "eventPattern",
                    "regions",
                    "creationTime",
                    "statusSummaryByRegion",
                ] {
                    assert_eq!(
                        entry[member], got[member],
                        "case maxResults={page_size}: the listing and the Get must agree on {member}, got {entry:?} and {got:?}"
                    );
                }
                collected.push(arn);
            }
            match page.get("nextToken").and_then(Value::as_str) {
                Some(next) => {
                    assert!(
                        !entries.is_empty(),
                        "case maxResults={page_size}: an empty page must not carry a token"
                    );
                    token = Some(next.to_string());
                }
                None => break,
            }
        }
        assert_eq!(
            collected, newest_first,
            "case maxResults={page_size}: the pages concatenate to every rule of the configuration, newest first"
        );
    }

    // An arn no configuration is stored under is the modelled 404, not an empty page.
    let (status, refused) = emulator
        .list_event_rules(UNMINTED_CONFIGURATION_ARN, "")
        .await;
    assert_eq!(
        status, 404,
        "case an unminted configuration: the listing answers the modelled 404, got {refused:?}"
    );
    assert!(
        error_code(&refused).contains("ResourceNotFoundException"),
        "case an unminted configuration: that refusal is a ResourceNotFoundException, got {refused:?}"
    );
}
