// SPDX-License-Identifier: CC0-1.0

//! What `NotificationConfigurationStatus` a configuration reports, driven over HTTP
//! through the same multi-service router the emulator boots in production.
//!
//! The rule under test: a configuration's status is DERIVED from the `EventRule`s it
//! owns, which is how the model defines each value — `ACTIVE` is "All of the EventRules
//! are in ACTIVE Status", `PARTIALLY_ACTIVE` is "Some EventRules are in ACTIVE Status and
//! some are INACTIVE", `INACTIVE` is "All of the EventRules are in INACTIVE Status" — so
//! a configuration with no rule at all has none of them ACTIVE and reports `INACTIVE`,
//! and the same derived value is what `CreateNotificationConfiguration` answers,
//! `GetNotificationConfiguration` reports and `ListNotificationConfigurations` both
//! filters on and reports.
//!
//! The table rows vary along what the rule generalises over: how many rules the
//! configuration owns (none, one, several), how many Regions each rule covers, and which
//! `EventRuleStatus` each of those Regions is in. Catalog source:
//! `smithy_query(query="shape",shape="NotificationConfigurationStatus,EventRuleStatus",
//! model_path="services/service-com-amazonaws-notifications-notifications/smithy-model/com-amazonaws-notifications-notifications.json")`,
//! re-exported as `NOTIFICATION_CONFIGURATION_STATUS_VALUES` and
//! `EVENT_RULE_STATUS_VALUES`.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_notifications_notifications::ops::types::NOTIFICATION_CONFIGURATION_STATUS_VALUES;
use service_com_amazonaws_notifications_notifications::state::{
    CONFIGURATION_STATUS_ACTIVE, CONFIGURATION_STATUS_INACTIVE,
    CONFIGURATION_STATUS_PARTIALLY_ACTIVE, EVENT_RULE_STATUS_ACTIVE, EVENT_RULE_STATUS_CREATING,
    EVENT_RULE_TRANSITION_SPAN,
};
use service_com_amazonaws_notifications_notifications::ComAmazonawsNotificationsNotificationsState;

// The HTTP harness this suite drives the service through, shared with every other suite
// in this crate.
use crate::harness::{encode_path_label, Emulator};

/// Long enough that an EventRule activation the create made due has certainly fallen
/// due, without depending on how slow the machine is.
const PAST_THE_TRANSITION: Duration =
    EVENT_RULE_TRANSITION_SPAN.saturating_add(Duration::from_millis(400));

/// `INACTIVE`, one of `EventRuleStatus`'s modelled values ("EventRule is in a bad state
/// and may not be processing events"). Spelled here because no handler puts a rule in it,
/// so the service crate declares no constant for it.
const EVENT_RULE_STATUS_INACTIVE: &str = "INACTIVE";

/// Regions a rule under test covers, one per Region slot a row asks for. Catalog source:
/// the `Region` shape the model declares for the members of `Regions`, `@length(2..25)`
/// and `@pattern("^([a-z]{1,4})-([a-z]{1,15}-)+([0-9])$")`.
const REGIONS: &[&str] = &["us-east-1", "eu-west-1", "ap-southeast-2"];

/// The status-derivation table.
struct StatusCase {
    case: &'static str,
    /// One entry per `EventRule` the configuration owns; each entry is the
    /// `EventRuleStatus` of that rule's Regions, in Region order.
    rules: &'static [&'static [&'static str]],
    /// The `NotificationConfigurationStatus` the configuration must then report.
    expected: &'static str,
}

/// Rows differ in rule count, Region count per rule and which `EventRuleStatus` each
/// Region is in — including rule sets no issue cited (a rule split across Regions, a
/// rule still coming up beside settled ones).
const STATUS_CASES: &[StatusCase] = &[
    StatusCase {
        case: "no EventRule at all, as a freshly created configuration",
        rules: &[],
        expected: CONFIGURATION_STATUS_INACTIVE,
    },
    StatusCase {
        case: "one rule, its single Region ACTIVE",
        rules: &[&[EVENT_RULE_STATUS_ACTIVE]],
        expected: CONFIGURATION_STATUS_ACTIVE,
    },
    StatusCase {
        case: "three rules, every Region of each ACTIVE",
        rules: &[
            &[EVENT_RULE_STATUS_ACTIVE],
            &[EVENT_RULE_STATUS_ACTIVE, EVENT_RULE_STATUS_ACTIVE],
            &[
                EVENT_RULE_STATUS_ACTIVE,
                EVENT_RULE_STATUS_ACTIVE,
                EVENT_RULE_STATUS_ACTIVE,
            ],
        ],
        expected: CONFIGURATION_STATUS_ACTIVE,
    },
    StatusCase {
        case: "one rule, its single Region INACTIVE",
        rules: &[&[EVENT_RULE_STATUS_INACTIVE]],
        expected: CONFIGURATION_STATUS_INACTIVE,
    },
    StatusCase {
        case: "two rules, every Region of each INACTIVE",
        rules: &[
            &[EVENT_RULE_STATUS_INACTIVE],
            &[EVENT_RULE_STATUS_INACTIVE, EVENT_RULE_STATUS_INACTIVE],
        ],
        expected: CONFIGURATION_STATUS_INACTIVE,
    },
    StatusCase {
        case: "one ACTIVE rule beside one INACTIVE rule",
        rules: &[&[EVENT_RULE_STATUS_ACTIVE], &[EVENT_RULE_STATUS_INACTIVE]],
        expected: CONFIGURATION_STATUS_PARTIALLY_ACTIVE,
    },
    StatusCase {
        case: "two ACTIVE rules beside one still coming up",
        rules: &[
            &[EVENT_RULE_STATUS_ACTIVE],
            &[EVENT_RULE_STATUS_ACTIVE],
            &[EVENT_RULE_STATUS_CREATING],
        ],
        expected: CONFIGURATION_STATUS_PARTIALLY_ACTIVE,
    },
    StatusCase {
        case: "one rule ACTIVE in one Region and INACTIVE in another, so not ACTIVE",
        rules: &[&[EVENT_RULE_STATUS_ACTIVE, EVENT_RULE_STATUS_INACTIVE]],
        expected: CONFIGURATION_STATUS_INACTIVE,
    },
    StatusCase {
        case: "a fully ACTIVE rule beside one ACTIVE in only one of its Regions",
        rules: &[
            &[EVENT_RULE_STATUS_ACTIVE, EVENT_RULE_STATUS_ACTIVE],
            &[EVENT_RULE_STATUS_ACTIVE, EVENT_RULE_STATUS_CREATING],
        ],
        expected: CONFIGURATION_STATUS_PARTIALLY_ACTIVE,
    },
];

/// The status-specific helpers this suite adds to the shared harness.
impl Emulator {
    /// Create a configuration and return its arn.
    async fn create_status_host(&self, name: &str) -> String {
        let (status, created) = self
            .send(
                "POST",
                "/notification-configurations",
                Some(json!({ "name": name, "description": "a configuration under test" })),
            )
            .await;
        assert_eq!(
            status, 201,
            "case {name:?}: create must answer 201, got {status} {created:?}"
        );
        created["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {name:?}: create must answer an arn, got {created:?}"))
            .to_string()
    }

    /// Create one `EventRule` on `configuration_arn` covering the first `regions`
    /// Regions, and return its arn.
    async fn create_status_rule(&self, configuration_arn: &str, source: &str, regions: usize) -> String {
        let (status, created) = self
            .send(
                "POST",
                "/event-rules",
                Some(json!({
                    "notificationConfigurationArn": configuration_arn,
                    "source": source,
                    "eventType": "Demo Event",
                    "regions": &REGIONS[..regions],
                })),
            )
            .await;
        assert_eq!(
            status, 201,
            "case {source:?}: CreateEventRule must answer 201, got {status} {created:?}"
        );
        created["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {source:?}: a created rule carries an arn, got {created:?}"))
            .to_string()
    }

    async fn read_configuration(&self, arn: &str) -> (u16, Value) {
        let uri = format!("/notification-configurations/{}", encode_path_label(arn));
        self.send("GET", &uri, None).await
    }

    async fn list_configurations_by_status(&self, status: &str) -> (u16, Value) {
        let uri = format!("/notification-configurations?status={status}&maxResults=100");
        self.send("GET", &uri, None).await
    }

    /// Put each of `configuration_arn`'s rules, in creation order, into the per-Region
    /// statuses a row asks for.
    ///
    /// Written straight into the store because no operation moves a rule to `INACTIVE`:
    /// the row is the STATE the derivation is a function of, and driving it through the
    /// clock could only reach the subset of states the clock produces.
    fn put_rule_statuses(&self, configuration_arn: &str, rules: &[&[&str]]) {
        let state = self
            .engine
            .state::<ComAmazonawsNotificationsNotificationsState>();
        let mut store = state.lock();
        let mut owned: Vec<(u64, String)> = store
            .event_rules
            .values()
            .filter(|rule| rule.notification_configuration_arn == configuration_arn)
            .map(|rule| (rule.sequence, rule.arn.clone()))
            .collect();
        owned.sort_by_key(|(sequence, _)| *sequence);
        assert_eq!(
            owned.len(),
            rules.len(),
            "the store must hold one rule per row entry for {configuration_arn}"
        );
        for ((_, arn), statuses) in owned.into_iter().zip(rules) {
            let rule = store
                .event_rules
                .get_mut(&arn)
                .unwrap_or_else(|| panic!("the rule {arn} was just read out of the store"));
            // The clock owes this rule nothing once the test has placed it.
            rule.due_at = None;
            let regions = rule.regions.clone();
            assert_eq!(
                regions.len(),
                statuses.len(),
                "the rule {arn} must cover one Region per status the row lists"
            );
            for (region, status) in regions.into_iter().zip(*statuses) {
                if let Some(summary) = rule.status_by_region.get_mut(&region) {
                    summary.status = (*status).to_string();
                }
            }
        }
    }
}

/// The `status` of a configuration (from a create, a get or a listing entry), which the
/// model marks required and constrains to `NotificationConfigurationStatus`.
fn status_of(case: &str, configuration: &Value) -> String {
    let status = configuration["status"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case:?}: every configuration carries a status, got {configuration:?}")
        })
        .to_string();
    assert!(
        NOTIFICATION_CONFIGURATION_STATUS_VALUES.contains(&status.as_str()),
        "case {case:?}: status must be one of the modelled NotificationConfigurationStatus \
         values {NOTIFICATION_CONFIGURATION_STATUS_VALUES:?}, got {status:?}"
    );
    status
}

/// Whether a listing reports `arn`.
fn lists_arn(case: &str, listed: &Value, arn: &str) -> bool {
    listed["notificationConfigurations"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: a listing must answer a notificationConfigurations list, got {listed:?}")
        })
        .iter()
        .any(|entry| entry["arn"] == json!(arn))
}

/// RULE: the status a configuration reports is derived from the `EventRule`s it owns —
/// every rule ACTIVE is `ACTIVE`, no rule ACTIVE (including owning none at all) is
/// `INACTIVE`, and some-but-not-all is `PARTIALLY_ACTIVE` — and a rule counts as ACTIVE
/// only when every Region it covers is.
#[tokio::test]
async fn the_reported_status_is_derived_from_the_configurations_event_rules() {
    let emulator = Emulator::new();

    for (index, row) in STATUS_CASES.iter().enumerate() {
        let arn = emulator
            .create_status_host(&format!("derived-status-{index}"))
            .await;
        for (rule_index, statuses) in row.rules.iter().enumerate() {
            emulator
                .create_status_rule(
                    &arn,
                    &format!("aws.svc-{index}-{rule_index}"),
                    statuses.len(),
                )
                .await;
        }
        emulator.put_rule_statuses(&arn, row.rules);

        let case = row.case;
        let (status, got) = emulator.read_configuration(&arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a get of a created configuration answers 200, got {got:?}"
        );
        assert_eq!(
            status_of(case, &got),
            row.expected,
            "case {case:?}: expected the derived status {:?}, got {got:?}",
            row.expected
        );

        // The listing reports the same derived value, and its `status` filter selects on
        // it: the configuration appears under exactly the value it reports.
        for candidate in NOTIFICATION_CONFIGURATION_STATUS_VALUES {
            let (code, listed) = emulator.list_configurations_by_status(candidate).await;
            assert_eq!(
                code, 200,
                "case {case:?} / status={candidate}: a listing answers 200, got {listed:?}"
            );
            let selected = lists_arn(case, &listed, &arn);
            assert_eq!(
                selected,
                *candidate == row.expected,
                "case {case:?} / status={candidate}: expected selected == {}, got {selected} \
                 ({listed:?})",
                *candidate == row.expected
            );
            if selected {
                let entry = listed["notificationConfigurations"]
                    .as_array()
                    .and_then(|entries| entries.iter().find(|entry| entry["arn"] == json!(arn)))
                    .expect("the entry just found in the listing")
                    .clone();
                assert_eq!(
                    status_of(case, &entry),
                    row.expected,
                    "case {case:?}: a listed entry reports the derived status, got {entry:?}"
                );
            }
        }
    }
}

/// RULE: a configuration `CreateNotificationConfiguration` mints owns no `EventRule`, so
/// the create response, a later read and the `INACTIVE` listing filter all report
/// `INACTIVE` — and the `ACTIVE` filter does not select it.
#[tokio::test]
async fn a_freshly_created_configuration_reports_inactive_everywhere() {
    let emulator = Emulator::new();

    // Rows differ in the name submitted, which the status does not depend on.
    for name in ["fresh-one", "Fresh_Two-3", "f"] {
        let (status, created) = emulator
            .send(
                "POST",
                "/notification-configurations",
                Some(json!({ "name": name, "description": "no rules yet" })),
            )
            .await;
        assert_eq!(
            status, 201,
            "case {name:?}: create answers the modelled 201, got {created:?}"
        );
        assert_eq!(
            status_of(name, &created),
            CONFIGURATION_STATUS_INACTIVE,
            "case {name:?}: a rule-less configuration is INACTIVE, got {created:?}"
        );
        let arn = created["arn"].as_str().unwrap_or_default().to_string();

        let (status, got) = emulator.read_configuration(&arn).await;
        assert_eq!(
            status, 200,
            "case {name:?}: a get of the created arn answers 200, got {got:?}"
        );
        assert_eq!(
            status_of(name, &got),
            CONFIGURATION_STATUS_INACTIVE,
            "case {name:?}: the read reports the same derived status, got {got:?}"
        );

        let (_, inactive) = emulator
            .list_configurations_by_status(CONFIGURATION_STATUS_INACTIVE)
            .await;
        assert!(
            lists_arn(name, &inactive, &arn),
            "case {name:?}: the INACTIVE filter selects a rule-less configuration, got {inactive:?}"
        );
        let (_, active) = emulator
            .list_configurations_by_status(CONFIGURATION_STATUS_ACTIVE)
            .await;
        assert!(
            !lists_arn(name, &active, &arn),
            "case {name:?}: the ACTIVE filter must not select it, got {active:?}"
        );
    }
}

/// RULE: it is the CLOCK, not a read, that moves a configuration's status: a rule created
/// on it is still coming up straight after the create — so the configuration is not
/// ACTIVE yet — and once the rule's activation has fallen due the same read reports
/// `ACTIVE`, with repeated reads reporting it identically.
#[tokio::test]
async fn the_clock_moves_a_configuration_to_active_when_its_rule_settles() {
    let emulator = Emulator::new();
    let case = "one rule created on a fresh configuration";

    let arn = emulator.create_status_host("clock-driven").await;
    emulator.create_status_rule(&arn, "aws.ec2", 2).await;

    // Straight after the create the rule's Regions are still CREATING, so no rule of the
    // configuration is ACTIVE.
    let (status, got) = emulator.read_configuration(&arn).await;
    assert_eq!(
        status, 200,
        "case {case:?}: a get answers 200, got {status} {got:?}"
    );
    assert_eq!(
        status_of(case, &got),
        CONFIGURATION_STATUS_INACTIVE,
        "case {case:?}: a configuration whose only rule is still coming up is not ACTIVE, got {got:?}"
    );

    tokio::time::sleep(PAST_THE_TRANSITION).await;

    let (status, settled) = emulator.read_configuration(&arn).await;
    assert_eq!(
        status, 200,
        "case {case:?}: a get after the transition answers 200, got {settled:?}"
    );
    assert_eq!(
        status_of(case, &settled),
        CONFIGURATION_STATUS_ACTIVE,
        "case {case:?}: every Region of the one rule is now ACTIVE, got {settled:?}"
    );
    let (_, again) = emulator.read_configuration(&arn).await;
    assert_eq!(
        settled, again,
        "case {case:?}: a read must not change what the next read reports, got {again:?}"
    );
    let (_, listed) = emulator
        .list_configurations_by_status(CONFIGURATION_STATUS_ACTIVE)
        .await;
    assert!(
        lists_arn(case, &listed, &arn),
        "case {case:?}: the ACTIVE filter now selects it, got {listed:?}"
    );
}
