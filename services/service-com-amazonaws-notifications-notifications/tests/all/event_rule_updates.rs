// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateEventRule`, driven over HTTP through the same
//! multi-service router the emulator boots in production and read back through
//! the `GetEventRule` that reports the stored rule.
//!
//! Every case asserts the RULE the operation follows, not one recorded request:
//! an update writes the members the request carries onto the stored rule, leaves
//! every member it omits as it is, covers exactly the distinct Regions it now
//! asks for, and — when that changed the rule — moves those Regions into the
//! transitional `UPDATING` status the model documents, which the CLOCK (not a
//! read) settles to `ACTIVE`.
//!
//! The table rows vary along what that rule generalises over: which members the
//! request carries (`regions`, `eventPattern`, both, neither), whether the Region
//! set widens, narrows, moves to a disjoint set or only changes order, whether a
//! Region is named twice, whether the pattern is a real pattern or the empty
//! string, and whether the request asks for anything the rule does not already
//! say. The catalogs the rows draw from are the model's own constraints
//! (`Region`'s `@length(2..25)` and `@pattern`, `EventRuleEventPattern`'s
//! `@length(0..4096)`) and this service's own modelled value set
//! (`EVENT_RULE_STATUS_VALUES`) and bound (`MAX_REGIONS_PER_EVENT_RULE`), so a
//! model change moves the test with it.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_notifications_notifications::ops::types::EVENT_RULE_STATUS_VALUES;
use service_com_amazonaws_notifications_notifications::state::{
    EVENT_RULE_MUTATION_RESOURCE_LABEL, EVENT_RULE_REASON_ACTIVE, EVENT_RULE_REASON_UPDATING,
    EVENT_RULE_STATUS_ACTIVE, EVENT_RULE_STATUS_UPDATING, EVENT_RULE_TRANSITION_SPAN,
    MAX_REGIONS_PER_EVENT_RULE,
};

// The HTTP harness this suite drives the service through, shared with every
// other suite in this crate — including the `GetEventRule` reader and the
// store-depth probe both EventRule suites use.
use crate::harness::{
    encode_path_label, error_code, get_event_rule, is_event_rule_arn, stored_event_rule_count,
    Emulator,
};

/// Long enough that a settling the service made due has certainly fallen due,
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
/// The shortest Region the pattern and the `@length` floor admit.
const SHORTEST_REGION: &str = "a-b-1";
/// The longest Region the `@length(..25)` ceiling admits, at 25 characters.
const LONGEST_REGION: &str = "abcd-efghijklmnop-opqrs-1";
/// A Region the rule is never created with, so an update naming it always moves
/// the rule to a Region it did not cover.
const UNUSED_REGION: &str = "ca-central-1";

/// The `source` and `eventType` the rule under test is created with. Neither is
/// a member of `UpdateEventRule`'s input, so no update can change them.
const BASE_SOURCE: &str = "aws.ec2";
const BASE_EVENT_TYPE: &str = "EC2 Instance State-change Notification";

/// The `eventPattern` the rule under test is created with, and a second one an
/// update can move it to. Catalog source: `EventRuleEventPattern`'s
/// `@length(0..4096)`, whose floor of 0 also admits the empty string a row below
/// updates to.
const BASE_EVENT_PATTERN: &str = r#"{"detail":{"state":["running"]}}"#;
const OTHER_EVENT_PATTERN: &str = r#"{"detail":{"state":["stopped","terminated"]}}"#;

/// A well-formed `EventRuleArn` this emulator never minted — the arn pattern is
/// satisfied, so a refusal comes from the handler rather than from the boundary
/// validator.
const UNMINTED_EVENT_RULE_ARN: &str = "arn:aws:notifications::123456789012:configuration/abcdefghijklmnopqrstuvwxyz0/rule/0zyxwvutsrqponmlkjihgfedcba";

/// The Regions the rule under test is created with.
fn base_regions() -> Vec<String> {
    vec![COMMONPLACE_REGION.to_string(), SECOND_REGION.to_string()]
}

/// `count` distinct Regions, every one satisfying the modelled `Region` pattern
/// `^([a-z]{1,4})-([a-z]{1,15}-)+([0-9])$` and both of its `@length` bounds: a
/// two-letter prefix walked through the alphabet, so the only thing that differs
/// between the bound rows is HOW MANY Regions the list names.
fn many_regions(count: usize) -> Vec<String> {
    (0..count)
        .map(|index| {
            let first = (b'a' + (index / 26) as u8) as char;
            let second = (b'a' + (index % 26) as u8) as char;
            format!("{first}{second}-region-1")
        })
        .collect()
}

/// One row of the update table.
struct UpdateCase {
    case: &'static str,
    /// The `regions` the update request carries, or `None` when it omits the
    /// member entirely.
    regions: Option<Vec<String>>,
    /// The `eventPattern` the update request carries, or `None` when it omits
    /// the member entirely.
    event_pattern: Option<String>,
    /// The Regions the rule covers afterwards, in the order a read reports them.
    expected_regions: Vec<String>,
    /// The `eventPattern` a read reports afterwards.
    expected_pattern: String,
    /// Whether the request asks for anything the rule does not already say, so
    /// the Regions it covers re-enter the transitional status.
    changes_the_rule: bool,
}

/// The table. Rows differ in which members the request carries, and in whether
/// the Region set widens, narrows, moves elsewhere, repeats a Region, only
/// changes order, or stays exactly as it was.
fn cases() -> Vec<UpdateCase> {
    vec![
        UpdateCase {
            case: "a wider Region set",
            regions: Some(vec![
                COMMONPLACE_REGION.to_string(),
                SECOND_REGION.to_string(),
                LONGER_MIDDLE_SEGMENT.to_string(),
            ]),
            event_pattern: None,
            expected_regions: vec![
                COMMONPLACE_REGION.to_string(),
                SECOND_REGION.to_string(),
                LONGER_MIDDLE_SEGMENT.to_string(),
            ],
            expected_pattern: BASE_EVENT_PATTERN.to_string(),
            changes_the_rule: true,
        },
        UpdateCase {
            case: "a narrower Region set, dropping a Region the rule covered",
            regions: Some(vec![SECOND_REGION.to_string()]),
            event_pattern: None,
            expected_regions: vec![SECOND_REGION.to_string()],
            expected_pattern: BASE_EVENT_PATTERN.to_string(),
            changes_the_rule: true,
        },
        UpdateCase {
            case: "a disjoint Region set, at both bounds the Region constraints admit",
            regions: Some(vec![
                SHORTEST_REGION.to_string(),
                LONGEST_REGION.to_string(),
                FOUR_SEGMENTS.to_string(),
            ]),
            event_pattern: None,
            expected_regions: vec![
                SHORTEST_REGION.to_string(),
                LONGEST_REGION.to_string(),
                FOUR_SEGMENTS.to_string(),
            ],
            expected_pattern: BASE_EVENT_PATTERN.to_string(),
            changes_the_rule: true,
        },
        UpdateCase {
            case: "only an eventPattern, which leaves the Regions as they were",
            regions: None,
            event_pattern: Some(OTHER_EVENT_PATTERN.to_string()),
            expected_regions: base_regions(),
            expected_pattern: OTHER_EVENT_PATTERN.to_string(),
            changes_the_rule: true,
        },
        UpdateCase {
            case: "an empty eventPattern, which the member's 0 length floor admits",
            regions: None,
            event_pattern: Some(String::new()),
            expected_regions: base_regions(),
            expected_pattern: String::new(),
            changes_the_rule: true,
        },
        UpdateCase {
            case: "both members at once",
            regions: Some(vec![UNUSED_REGION.to_string()]),
            event_pattern: Some(OTHER_EVENT_PATTERN.to_string()),
            expected_regions: vec![UNUSED_REGION.to_string()],
            expected_pattern: OTHER_EVENT_PATTERN.to_string(),
            changes_the_rule: true,
        },
        UpdateCase {
            case: "a Region named twice, which the status map keys once",
            regions: Some(vec![
                FOUR_SEGMENTS.to_string(),
                UNUSED_REGION.to_string(),
                FOUR_SEGMENTS.to_string(),
            ]),
            event_pattern: None,
            expected_regions: vec![FOUR_SEGMENTS.to_string(), UNUSED_REGION.to_string()],
            expected_pattern: BASE_EVENT_PATTERN.to_string(),
            changes_the_rule: true,
        },
        UpdateCase {
            case: "the same Regions in another order, which a read reports in the order asked for",
            regions: Some(vec![SECOND_REGION.to_string(), COMMONPLACE_REGION.to_string()]),
            event_pattern: None,
            expected_regions: vec![SECOND_REGION.to_string(), COMMONPLACE_REGION.to_string()],
            expected_pattern: BASE_EVENT_PATTERN.to_string(),
            changes_the_rule: true,
        },
        UpdateCase {
            case: "neither member, which leaves the whole rule as it is",
            regions: None,
            event_pattern: None,
            expected_regions: base_regions(),
            expected_pattern: BASE_EVENT_PATTERN.to_string(),
            changes_the_rule: false,
        },
        UpdateCase {
            case: "exactly what the rule already says, which changes nothing",
            regions: Some(base_regions()),
            event_pattern: Some(BASE_EVENT_PATTERN.to_string()),
            expected_regions: base_regions(),
            expected_pattern: BASE_EVENT_PATTERN.to_string(),
            changes_the_rule: false,
        },
    ]
}

/// Create a NotificationConfiguration for a rule to hang off, and return its arn.
async fn create_configuration(emulator: &Emulator, name: &str) -> String {
    let (status, response) = emulator
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

/// Create the rule every row updates, and return its configuration's arn and its
/// own.
async fn create_base_rule(emulator: &Emulator) -> (String, String) {
    let configuration = create_configuration(emulator, "rule-host").await;
    let (status, created) = emulator
        .send(
            "POST",
            "/event-rules",
            Some(json!({
                "notificationConfigurationArn": configuration,
                "source": BASE_SOURCE,
                "eventType": BASE_EVENT_TYPE,
                "eventPattern": BASE_EVENT_PATTERN,
                "regions": base_regions(),
            })),
        )
        .await;
    assert_eq!(
        status, 201,
        "the rule under test must be created, got {created:?}"
    );
    let arn = created["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("a create must answer an arn, got {created:?}"))
        .to_string();
    (configuration, arn)
}

/// `UpdateEventRule` with the body a row asks for, omitting a member the row
/// does not carry.
async fn update_event_rule(
    emulator: &Emulator,
    arn: &str,
    regions: Option<&[String]>,
    event_pattern: Option<&str>,
) -> (u16, Value) {
    let mut body = json!({});
    if let Some(regions) = regions {
        body["regions"] = json!(regions);
    }
    if let Some(event_pattern) = event_pattern {
        body["eventPattern"] = json!(event_pattern);
    }
    let uri = format!("/event-rules/{}", encode_path_label(arn));
    emulator.send("PUT", &uri, Some(body)).await
}

/// `statusSummaryByRegion` as a Region -> (status, reason) map, with every status
/// checked against the modelled value set.
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

/// The Regions a status map is keyed by, sorted so an assertion does not depend
/// on a JSON object's member order.
fn sorted(regions: &[String]) -> Vec<String> {
    let mut sorted = regions.to_vec();
    sorted.sort();
    sorted
}

/// RULE: an update writes the members the request carries onto the stored
/// EventRule and leaves every member it omits as it is; the rule then covers
/// exactly the distinct Regions it was last asked for, one `statusSummaryByRegion`
/// entry each, and an update that changed the rule leaves those Regions in the
/// transitional `UPDATING` status the model documents, which the clock — not a
/// read — settles to `ACTIVE`. A Get reports that same stored record.
#[tokio::test]
async fn an_update_writes_the_members_it_carries_and_the_clock_settles_the_rule() {
    for case in cases() {
        let label = case.case;
        let emulator = Emulator::new();
        let (configuration, arn) = create_base_rule(&emulator).await;

        // Start from a settled rule, so a transitional status after the update
        // can only have been put there by the update.
        tokio::time::sleep(PAST_THE_TRANSITION).await;
        let (status, before) = get_event_rule(&emulator, &arn).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the rule under test must be readable, got {before:?}"
        );
        for (region, (status, _)) in status_map(label, &before) {
            assert_eq!(
                status, EVENT_RULE_STATUS_ACTIVE,
                "case {label:?}: the rule starts settled in Region {region:?}, got {before:?}"
            );
        }

        let (status, updated) = update_event_rule(
            &emulator,
            &arn,
            case.regions.as_deref(),
            case.event_pattern.as_deref(),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: an update answers the modelled 200, got {updated:?}"
        );
        assert_eq!(
            updated["arn"],
            json!(arn),
            "case {label:?}: the update reports the arn it was asked for, got {updated:?}"
        );
        assert!(
            is_event_rule_arn(updated["arn"].as_str().unwrap_or_default()),
            "case {label:?}: that arn matches the modelled EventRuleArn pattern, got {updated:?}"
        );
        assert_eq!(
            updated["notificationConfigurationArn"],
            json!(configuration),
            "case {label:?}: the update reports the configuration the rule belongs to, got {updated:?}"
        );

        let summaries = status_map(label, &updated);
        assert_eq!(
            summaries.keys().cloned().collect::<Vec<String>>(),
            sorted(&case.expected_regions),
            "case {label:?}: statusSummaryByRegion is keyed by the distinct Regions the rule now covers, got {updated:?}"
        );
        let (expected_status, expected_reason) = if case.changes_the_rule {
            (EVENT_RULE_STATUS_UPDATING, EVENT_RULE_REASON_UPDATING)
        } else {
            (EVENT_RULE_STATUS_ACTIVE, EVENT_RULE_REASON_ACTIVE)
        };
        for (region, (status, reason)) in &summaries {
            assert_eq!(
                status, expected_status,
                "case {label:?}: Region {region:?} reports the status this update put it in, got {updated:?}"
            );
            assert_eq!(
                reason, expected_reason,
                "case {label:?}: Region {region:?} carries the reason for its status, got {updated:?}"
            );
        }

        // The Get reports the stored record: every member the update wrote, every
        // member it left alone, and the same status summaries the update answered.
        let (status, fetched) = get_event_rule(&emulator, &arn).await;
        assert_eq!(
            status, 200,
            "case {label:?}: a Get of an updated rule answers 200, got {fetched:?}"
        );
        assert_eq!(
            fetched["regions"],
            json!(case.expected_regions),
            "case {label:?}: the Get reports the Regions the rule now covers, in order and without repeats, got {fetched:?}"
        );
        assert_eq!(
            fetched["eventPattern"],
            json!(case.expected_pattern),
            "case {label:?}: the Get reports the eventPattern the rule now carries, got {fetched:?}"
        );
        assert_eq!(
            fetched["source"], before["source"],
            "case {label:?}: source is not a member of this operation, so an update leaves it, got {fetched:?}"
        );
        assert_eq!(
            fetched["eventType"], before["eventType"],
            "case {label:?}: eventType is not a member of this operation, so an update leaves it, got {fetched:?}"
        );
        assert_eq!(
            fetched["creationTime"], before["creationTime"],
            "case {label:?}: creationTime is stamped once and never rewritten, got {fetched:?}"
        );
        assert_eq!(
            fetched["notificationConfigurationArn"],
            json!(configuration),
            "case {label:?}: an update never moves a rule to another configuration, got {fetched:?}"
        );
        assert_eq!(
            status_map(label, &fetched),
            summaries,
            "case {label:?}: the Get reports the stored status summaries, got {fetched:?}"
        );

        // Only the clock moves the rule on.
        tokio::time::sleep(PAST_THE_TRANSITION).await;
        let (status, settled) = get_event_rule(&emulator, &arn).await;
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

        // `@idempotent`: asking again for what the rule now says changes nothing
        // a later reader sees — not the members, and not the settled status.
        let (status, repeated) = update_event_rule(
            &emulator,
            &arn,
            case.regions.as_deref(),
            case.event_pattern.as_deref(),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: a repeated update stays successful, got {repeated:?}"
        );
        let (_, again) = get_event_rule(&emulator, &arn).await;
        assert_eq!(
            again, settled,
            "case {label:?}: a repeated update leaves the rule exactly as it was, got {again:?}"
        );
        assert_eq!(
            stored_event_rule_count(&emulator),
            1,
            "case {label:?}: an update never mints a second rule"
        );
    }
}

/// RULE: the arn is the only key an EventRule is reached by, so an update naming
/// an arn the store holds no rule under is refused with the modelled
/// `ResourceNotFoundException` (HTTP 404), whose message names the resource, the
/// member that identified it and the value the request carried — whatever the
/// reason the rule is absent — and persists nothing.
#[tokio::test]
async fn updating_a_rule_that_does_not_exist_is_not_found() {
    let emulator = Emulator::new();

    // Rows differ in WHY the rule is absent: an arn this emulator never minted,
    // and the arn of a rule whose configuration was deleted under it.
    let (configuration, orphaned) = create_base_rule(&emulator).await;
    let (status, deleted) = emulator
        .send(
            "DELETE",
            &format!(
                "/notification-configurations/{}",
                encode_path_label(&configuration)
            ),
            None,
        )
        .await;
    assert_eq!(
        status, 200,
        "the configuration must be deleted, got {deleted:?}"
    );

    for (label, arn) in [
        ("an arn this emulator never minted", UNMINTED_EVENT_RULE_ARN.to_string()),
        ("the arn of a rule whose configuration was deleted", orphaned),
    ] {
        let regions = vec![UNUSED_REGION.to_string()];
        let (status, refused) =
            update_event_rule(&emulator, &arn, Some(&regions), None).await;
        assert_eq!(
            status, 404,
            "case {label:?}: an update of an absent rule answers the modelled 404, got {refused:?}"
        );
        assert!(
            error_code(&refused).contains("ResourceNotFoundException"),
            "case {label:?}: that refusal is a ResourceNotFoundException, got {refused:?}"
        );
        let stated = refused["message"].as_str().unwrap_or_default();
        let expected = format!("{EVENT_RULE_MUTATION_RESOURCE_LABEL} with arn {arn} not found");
        assert_eq!(
            stated, expected,
            "case {label:?}: expected message {expected:?}, got {stated:?}"
        );
        assert_eq!(
            stored_event_rule_count(&emulator),
            0,
            "case {label:?}: a refused update persists nothing"
        );
    }
}

/// RULE: the Regions one EventRule covers are bounded, so an update is served at
/// the bound and refused past it with the modelled `ValidationException`
/// (HTTP 400) naming the member and the bound — and a refused update leaves the
/// rule exactly as it was.
#[tokio::test]
async fn an_update_is_served_at_the_region_bound_and_refused_past_it() {
    for (label, count, expected_status) in [
        ("one Region, the modelled length floor", 1, 200),
        ("one Region short of the bound", MAX_REGIONS_PER_EVENT_RULE - 1, 200),
        ("exactly the bound", MAX_REGIONS_PER_EVENT_RULE, 200),
        ("one Region past the bound", MAX_REGIONS_PER_EVENT_RULE + 1, 400),
    ] {
        let emulator = Emulator::new();
        let (_, arn) = create_base_rule(&emulator).await;
        let regions = many_regions(count);

        let (status, response) = update_event_rule(&emulator, &arn, Some(&regions), None).await;
        assert_eq!(
            status, expected_status,
            "case {label:?}: an update naming {count} Regions answers {expected_status}, got {response:?}"
        );

        let (_, fetched) = get_event_rule(&emulator, &arn).await;
        if expected_status == 200 {
            assert_eq!(
                fetched["regions"],
                json!(regions),
                "case {label:?}: a served update covers every Region it named, got {fetched:?}"
            );
            assert_eq!(
                status_map(label, &fetched).len(),
                count,
                "case {label:?}: one status summary per Region, got {fetched:?}"
            );
        } else {
            assert!(
                error_code(&response).contains("ValidationException"),
                "case {label:?}: that refusal is a ValidationException, got {response:?}"
            );
            let stated = response["message"].as_str().unwrap_or_default();
            assert!(
                stated.contains("regions") && stated.contains(&MAX_REGIONS_PER_EVENT_RULE.to_string()),
                "case {label:?}: the message names the member and the bound it exceeded, got {stated:?}"
            );
            assert_eq!(
                fetched["regions"],
                json!(base_regions()),
                "case {label:?}: a refused update leaves the rule as it was, got {fetched:?}"
            );
        }
    }
}
