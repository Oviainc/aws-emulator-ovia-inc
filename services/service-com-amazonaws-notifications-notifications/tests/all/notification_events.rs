// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListNotificationEvents`, driven over HTTP through the
//! same multi-service router the emulator boots in production.
//!
//! The RULE these cases pin: the listing reports one `NotificationEventOverview`
//! per notification event the store holds, in reverse chronological order
//! (newest first), keeping only the events every filter the request carries
//! selects, cut into pages of `maxResults` (defaulting to 20) with an opaque
//! resume token — and it never changes what a later caller sees.
//!
//! A `NotificationEvent` is not created by a request: User Notifications stores
//! one when an event from a source service matches an `EventRule`. So the
//! fixture these cases read back is seeded through the store's own delivery path
//! (`record_notification_event`), the way the shared harness reads the store
//! directly for state no operation reports.
//!
//! Every row varies the input along what the rule generalises over — the source,
//! the window, the aggregation role, the organizational unit, the page size —
//! rather than repeating one request.

use aws_smithy_types::DateTime;
use serde_json::Value;
use service_com_amazonaws_notifications_notifications::ops::types::{
    AggregationKey, AggregationSummary, SummarizationDimensionOverview, EVENT_STATUS_VALUES,
    NOTIFICATION_TYPE_VALUES,
};
use service_com_amazonaws_notifications_notifications::state::{
    NotificationEventDelivery, AGGREGATION_EVENT_TYPE_AGGREGATE, AGGREGATION_EVENT_TYPE_CHILD,
    AGGREGATION_EVENT_TYPE_NONE, NOTIFICATION_EVENT_SCHEMA_VERSION,
};
use service_com_amazonaws_notifications_notifications::ComAmazonawsNotificationsNotificationsState;

use crate::harness::{encode_path_label, error_code, is_an_instant, Emulator};

/// The Region the seeded events are stored in: "ListNotificationEvents only
/// returns notifications stored in the same Region in which the action is
/// called", and the harness calls the service as `us-east-1`.
const REGION: &str = "us-east-1";

/// The instant the oldest seeded event was delivered, in epoch seconds. Each
/// later event is delivered [`DELIVERY_GAP`] after the one before it, so a
/// window bound can name one event exactly.
const FIRST_DELIVERY_SECS: i64 = 1_760_000_000;

/// The gap between two seeded deliveries, in seconds.
const DELIVERY_GAP: i64 = 60;

/// One seeded delivery.
struct Seed {
    /// What this event is, named in every assertion that reports it.
    label: &'static str,
    source: &'static str,
    event_type: &'static str,
    headline: &'static str,
    event_status: &'static str,
    notification_type: &'static str,
    /// `AggregationEventType`: `NONE`, `AGGREGATE` (this event aggregates the
    /// `CHILD` rows that follow it) or `CHILD`.
    aggregation: &'static str,
    organizational_unit_id: Option<&'static str>,
}

/// The seeded fixture, oldest delivery first.
///
/// The rows differ in source, event type, status, notification type,
/// aggregation role and organizational unit: the dimensions the listing's
/// filters narrow on. The statuses and notification types are members of the
/// service's own modelled value sets (`EVENT_STATUS_VALUES`,
/// `NOTIFICATION_TYPE_VALUES`), checked below.
fn seeds() -> Vec<Seed> {
    vec![
        Seed {
            label: "oldest: a CloudWatch alarm state change",
            source: "aws.cloudwatch",
            event_type: "CloudWatch Alarm State Change",
            headline: "Alarm checkout-latency changed state to ALARM",
            event_status: "UNHEALTHY",
            notification_type: "ALERT",
            aggregation: AGGREGATION_EVENT_TYPE_NONE,
            organizational_unit_id: None,
        },
        Seed {
            label: "an AWS Health announcement",
            source: "aws.health",
            event_type: "AWS Health Event",
            headline: "Scheduled maintenance for one of your resources",
            event_status: "HEALTHY",
            notification_type: "ANNOUNCEMENT",
            aggregation: AGGREGATION_EVENT_TYPE_NONE,
            organizational_unit_id: None,
        },
        Seed {
            label: "an EC2 event inside an organizational unit",
            source: "aws.ec2",
            event_type: "EC2 Instance State-change Notification",
            headline: "Instance i-0123456789abcdef0 is stopping",
            event_status: "HEALTHY",
            notification_type: "INFORMATIONAL",
            aggregation: AGGREGATION_EVENT_TYPE_NONE,
            organizational_unit_id: Some("ou-abc1-1a2b3c4d"),
        },
        Seed {
            label: "an aggregate of CloudWatch alarm state changes",
            source: "aws.cloudwatch",
            event_type: "CloudWatch Alarm State Change",
            headline: "2 alarms changed state in the last hour",
            event_status: "UNHEALTHY",
            notification_type: "ALERT",
            aggregation: AGGREGATION_EVENT_TYPE_AGGREGATE,
            organizational_unit_id: None,
        },
        Seed {
            label: "the aggregate's first child",
            source: "aws.cloudwatch",
            event_type: "CloudWatch Alarm State Change",
            headline: "Alarm queue-depth changed state to ALARM",
            event_status: "UNHEALTHY",
            notification_type: "ALERT",
            aggregation: AGGREGATION_EVENT_TYPE_CHILD,
            organizational_unit_id: None,
        },
        Seed {
            label: "the aggregate's second child",
            source: "aws.cloudwatch",
            event_type: "CloudWatch Alarm State Change",
            headline: "Alarm error-rate changed state to ALARM",
            event_status: "UNHEALTHY",
            notification_type: "ALERT",
            aggregation: AGGREGATION_EVENT_TYPE_CHILD,
            organizational_unit_id: None,
        },
        Seed {
            label: "newest: a Config compliance change at the organization root",
            source: "aws.config",
            event_type: "Config Rules Compliance Change",
            headline: "Rule required-tags is now NON_COMPLIANT",
            event_status: "HEALTHY",
            notification_type: "WARNING",
            aggregation: AGGREGATION_EVENT_TYPE_NONE,
            organizational_unit_id: Some("Root"),
        },
    ]
}

/// The fixture as it was stored: one entry per seed, in delivery order.
struct Delivered {
    label: &'static str,
    arn: String,
    at: DateTime,
}

impl Emulator {
    /// Create a notification configuration and return its arn — the parent every
    /// delivered event nests under.
    async fn create_configuration_for_events(&self, name: &str) -> String {
        let body = serde_json::json!({"name": name, "description": "events under test"});
        let (status, response) = self
            .send("POST", "/notification-configurations", Some(body))
            .await;
        assert_eq!(
            status, 201,
            "creating configuration {name:?} must answer 201, got {response:?}"
        );
        response["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("create must answer an arn, got {response:?}"))
            .to_string()
    }

    /// Record one delivery in the store and return the arn it was given.
    fn deliver(&self, delivery: NotificationEventDelivery) -> String {
        let state = self
            .engine
            .state::<ComAmazonawsNotificationsNotificationsState>();
        let mut store = state.lock();
        store
            .record_notification_event(delivery)
            .map(|record| record.arn.clone())
            .expect("a delivery naming a stored configuration is recorded")
    }

    /// How many notification events the store holds — what a reporting call must
    /// not change.
    fn stored_event_count(&self) -> usize {
        let state = self
            .engine
            .state::<ComAmazonawsNotificationsNotificationsState>();
        let held = state.lock().notification_events.len();
        held
    }

    /// `ListNotificationEvents` with a raw query string (empty for an unfiltered,
    /// unpaged listing).
    async fn list_events(&self, query: &str) -> (u16, Value) {
        let uri = if query.is_empty() {
            "/notification-events".to_string()
        } else {
            format!("/notification-events?{query}")
        };
        self.send("GET", &uri, None).await
    }
}

/// Seed the whole fixture into one configuration and report what was stored.
async fn seeded_emulator() -> (Emulator, Vec<Delivered>) {
    let emulator = Emulator::new();
    let configuration = emulator
        .create_configuration_for_events("events-under-test")
        .await;

    let mut delivered: Vec<Delivered> = Vec::new();
    let mut aggregate: Option<String> = None;
    for (index, seed) in seeds().iter().enumerate() {
        let at = DateTime::from_secs(FIRST_DELIVERY_SECS + DELIVERY_GAP * index as i64);
        let parent = if seed.aggregation == AGGREGATION_EVENT_TYPE_CHILD {
            aggregate.clone()
        } else {
            None
        };
        let arn = emulator.deliver(NotificationEventDelivery {
            notification_configuration_arn: configuration.clone(),
            region: REGION.to_string(),
            creation_time: at,
            source: seed.source.to_string(),
            event_type: seed.event_type.to_string(),
            event_origin_region: Some(REGION.to_string()),
            headline: seed.headline.to_string(),
            event_status: seed.event_status.to_string(),
            notification_type: seed.notification_type.to_string(),
            related_account: None,
            aggregation_event_type: seed.aggregation.to_string(),
            aggregate_notification_event_arn: parent,
            aggregation_summary: (seed.aggregation == AGGREGATION_EVENT_TYPE_AGGREGATE)
                .then(seeded_aggregation_summary),
            organizational_unit_id: seed.organizational_unit_id.map(ToString::to_string),
        });
        if seed.aggregation == AGGREGATION_EVENT_TYPE_AGGREGATE {
            aggregate = Some(arn.clone());
        }
        delivered.push(Delivered {
            label: seed.label,
            arn,
            at,
        });
    }
    (emulator, delivered)
}

/// The `aggregationSummary` the seeded `AGGREGATE` delivery carries, so the
/// listing is asserted to report the summary it was given rather than one it
/// invented.
fn seeded_aggregation_summary() -> AggregationSummary {
    AggregationSummary {
        event_count: 2,
        aggregated_by: vec![AggregationKey {
            name: "eventType".to_string(),
            value: "CloudWatch Alarm State Change".to_string(),
        }],
        aggregated_accounts: SummarizationDimensionOverview {
            name: "accountId".to_string(),
            count: 1,
            sample_values: Some(vec![engine::emulator_account_id().to_string()]),
        },
        aggregated_regions: SummarizationDimensionOverview {
            name: "region".to_string(),
            count: 1,
            sample_values: Some(vec![REGION.to_string()]),
        },
        aggregated_organizational_units: None,
        additional_summarization_dimensions: None,
    }
}

/// The arns a listing response reports, in the order it reported them.
fn reported_arns(response: &Value) -> Vec<String> {
    response["notificationEvents"]
        .as_array()
        .unwrap_or_else(|| panic!("a listing must answer a notificationEvents list, got {response:?}"))
        .iter()
        .map(|event| {
            event["arn"]
                .as_str()
                .unwrap_or_else(|| panic!("every event must carry an arn, got {event:?}"))
                .to_string()
        })
        .collect()
}

/// One instant as the `date-time` string the `startTime`/`endTime` query members
/// are modelled in, percent-encoded for a query value.
fn query_instant(at: DateTime) -> String {
    let rendered = at
        .fmt(aws_smithy_types::date_time::Format::DateTime)
        .expect("a stored instant renders as date-time");
    encode_path_label(&rendered)
}

/// Whether `value` matches the `NotificationEventArn` pattern the model
/// declares,
/// `^arn:[a-z-]{3,10}:notifications:[-.a-z0-9]{1,63}:[0-9]{12}:configuration/[a-z0-9]{27}/event/[a-z0-9]{27}$`
/// — checked by hand, like the arn checkers in the shared harness, so this suite
/// needs no regex dependency.
fn is_notification_event_arn(value: &str) -> bool {
    let Some(rest) = value.strip_prefix("arn:") else {
        return false;
    };
    let Some((partition, rest)) = rest.split_once(':') else {
        return false;
    };
    if !(3..=10).contains(&partition.len())
        || !partition
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte == b'-')
    {
        return false;
    }
    let Some(rest) = rest.strip_prefix("notifications:") else {
        return false;
    };
    let Some((region, rest)) = rest.split_once(':') else {
        return false;
    };
    if !(1..=63).contains(&region.len())
        || !region
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'.'))
    {
        return false;
    }
    let Some((account, resource)) = rest.split_once(':') else {
        return false;
    };
    if account.len() != 12 || !account.bytes().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    let Some(path) = resource.strip_prefix("configuration/") else {
        return false;
    };
    let Some((configuration_id, event_id)) = path.split_once("/event/") else {
        return false;
    };
    is_arn_id(configuration_id) && is_arn_id(event_id)
}

/// One arn id segment as this service's arn patterns declare it: `[a-z0-9]{27}`.
fn is_arn_id(id: &str) -> bool {
    id.len() == 27
        && id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

/// RULE: an account nothing has been delivered to reports no notification event
/// — a 200 with an empty list and no resume token, not a refusal.
#[tokio::test]
async fn an_empty_store_lists_no_events() {
    let emulator = Emulator::new();
    let (status, response) = emulator.list_events("").await;
    assert_eq!(status, 200, "an empty listing must answer 200, got {response:?}");
    assert_eq!(
        reported_arns(&response),
        Vec::<String>::new(),
        "an empty store must report no event, got {response:?}"
    );
    assert!(
        response.get("nextToken").is_none_or(Value::is_null),
        "an empty listing must carry no nextToken, got {response:?}"
    );
}

/// RULE: every stored event that is not aggregated under another is reported
/// newest first, with every member read back from the record that was stored.
#[tokio::test]
async fn reports_stored_events_newest_first() {
    let (emulator, delivered) = seeded_emulator().await;
    let (status, response) = emulator.list_events("").await;
    assert_eq!(status, 200, "the listing must answer 200, got {response:?}");

    let expected: Vec<String> = delivered
        .iter()
        .zip(seeds())
        .filter(|(_, seed)| seed.aggregation != AGGREGATION_EVENT_TYPE_CHILD)
        .map(|(record, _)| record.arn.clone())
        .rev()
        .collect();
    assert_eq!(
        reported_arns(&response),
        expected,
        "the unfiltered listing must report every non-child event newest first, got {response:?}"
    );

    let events = response["notificationEvents"]
        .as_array()
        .expect("a notificationEvents list");
    for event in events {
        let arn = event["arn"].as_str().unwrap_or_default();
        let (position, _) = delivered
            .iter()
            .enumerate()
            .find(|(_, record)| record.arn == arn)
            .unwrap_or_else(|| panic!("the listing reported an arn nothing delivered: {event:?}"));
        let seed = &seeds()[position];
        let case = seed.label;

        assert!(
            is_notification_event_arn(arn),
            "case {case:?}: the arn must match NotificationEventArn, got {arn:?}"
        );
        assert_eq!(
            event["notificationEvent"]["sourceEventMetadata"]["source"].as_str(),
            Some(seed.source),
            "case {case:?}: expected source {:?}, got {event:?}",
            seed.source
        );
        assert_eq!(
            event["notificationEvent"]["sourceEventMetadata"]["eventType"].as_str(),
            Some(seed.event_type),
            "case {case:?}: expected eventType {:?}, got {event:?}",
            seed.event_type
        );
        assert_eq!(
            event["notificationEvent"]["sourceEventMetadata"]["eventOriginRegion"].as_str(),
            Some(REGION),
            "case {case:?}: expected eventOriginRegion {REGION:?}, got {event:?}"
        );
        assert_eq!(
            event["notificationEvent"]["messageComponents"]["headline"].as_str(),
            Some(seed.headline),
            "case {case:?}: expected headline {:?}, got {event:?}",
            seed.headline
        );
        assert_eq!(
            event["notificationEvent"]["eventStatus"].as_str(),
            Some(seed.event_status),
            "case {case:?}: expected eventStatus {:?}, got {event:?}",
            seed.event_status
        );
        assert!(
            EVENT_STATUS_VALUES.contains(&seed.event_status),
            "case {case:?}: the fixture must use a modelled EventStatus, got {:?}",
            seed.event_status
        );
        assert_eq!(
            event["notificationEvent"]["notificationType"].as_str(),
            Some(seed.notification_type),
            "case {case:?}: expected notificationType {:?}, got {event:?}",
            seed.notification_type
        );
        assert!(
            NOTIFICATION_TYPE_VALUES.contains(&seed.notification_type),
            "case {case:?}: the fixture must use a modelled NotificationType, got {:?}",
            seed.notification_type
        );
        assert_eq!(
            event["notificationEvent"]["schemaVersion"].as_str(),
            Some(NOTIFICATION_EVENT_SCHEMA_VERSION),
            "case {case:?}: expected schemaVersion {NOTIFICATION_EVENT_SCHEMA_VERSION:?}, got {event:?}"
        );
        assert_eq!(
            event["aggregationEventType"].as_str(),
            Some(seed.aggregation),
            "case {case:?}: expected aggregationEventType {:?}, got {event:?}",
            seed.aggregation
        );
        assert_eq!(
            event["organizationalUnitId"].as_str(),
            seed.organizational_unit_id,
            "case {case:?}: expected organizationalUnitId {:?}, got {event:?}",
            seed.organizational_unit_id
        );
        assert_eq!(
            event["relatedAccount"].as_str(),
            Some(engine::emulator_account_id()),
            "case {case:?}: expected relatedAccount {:?}, got {event:?}",
            engine::emulator_account_id()
        );
        assert!(
            is_an_instant(&event["creationTime"]),
            "case {case:?}: creationTime must be a real instant, got {event:?}"
        );

        // Only the AGGREGATE event carries the summary its delivery computed.
        if seed.aggregation == AGGREGATION_EVENT_TYPE_AGGREGATE {
            assert_eq!(
                event["aggregationSummary"]["eventCount"].as_i64(),
                Some(2),
                "case {case:?}: expected the stored aggregationSummary, got {event:?}"
            );
            assert_eq!(
                event["aggregationSummary"]["aggregatedRegions"]["name"].as_str(),
                Some("region"),
                "case {case:?}: expected the stored aggregated region dimension, got {event:?}"
            );
        } else {
            assert!(
                event.get("aggregationSummary").is_none_or(Value::is_null),
                "case {case:?}: an event that aggregates nothing must carry no aggregationSummary, got {event:?}"
            );
        }
    }
}

/// RULE: each filter the request carries narrows the listing on the matching
/// member of the stored event, and the filters narrow independently.
#[tokio::test]
async fn filters_narrow_the_listing() {
    let (emulator, delivered) = seeded_emulator().await;
    let arn = |label: &str| {
        delivered
            .iter()
            .find(|record| record.label == label)
            .map(|record| record.arn.clone())
            .unwrap_or_else(|| panic!("the fixture must hold {label:?}"))
    };
    let at = |label: &str| {
        delivered
            .iter()
            .find(|record| record.label == label)
            .map(|record| record.at)
            .unwrap_or_else(|| panic!("the fixture must hold {label:?}"))
    };

    let oldest = "oldest: a CloudWatch alarm state change";
    let health = "an AWS Health announcement";
    let ec2 = "an EC2 event inside an organizational unit";
    let aggregate = "an aggregate of CloudWatch alarm state changes";
    let first_child = "the aggregate's first child";
    let second_child = "the aggregate's second child";
    let newest = "newest: a Config compliance change at the organization root";

    let cases: Vec<(&str, String, Vec<String>)> = vec![
        (
            "source names a service with two non-child events",
            "source=aws.cloudwatch".to_string(),
            vec![arn(aggregate), arn(oldest)],
        ),
        (
            "source names a service with one event",
            "source=aws.health".to_string(),
            vec![arn(health)],
        ),
        (
            "source names the newest event's service",
            "source=aws.config".to_string(),
            vec![arn(newest)],
        ),
        (
            "source names a service nothing was delivered from",
            "source=aws.lambda".to_string(),
            Vec::new(),
        ),
        (
            "includeChildEvents admits the aggregated children",
            "includeChildEvents=true".to_string(),
            vec![
                arn(newest),
                arn(second_child),
                arn(first_child),
                arn(aggregate),
                arn(ec2),
                arn(health),
                arn(oldest),
            ],
        ),
        (
            "includeChildEvents=false reports the same set as omitting it",
            "includeChildEvents=false".to_string(),
            vec![arn(newest), arn(aggregate), arn(ec2), arn(health), arn(oldest)],
        ),
        (
            "source with includeChildEvents admits that source's children",
            "source=aws.cloudwatch&includeChildEvents=true".to_string(),
            vec![
                arn(second_child),
                arn(first_child),
                arn(aggregate),
                arn(oldest),
            ],
        ),
        (
            "startTime is inclusive, so the event delivered at it is reported",
            format!("startTime={}", query_instant(at(ec2))),
            vec![arn(newest), arn(aggregate), arn(ec2)],
        ),
        (
            "endTime is inclusive, so the event delivered at it is reported",
            format!("endTime={}", query_instant(at(health))),
            vec![arn(health), arn(oldest)],
        ),
        (
            "both bounds select the window between them",
            format!(
                "startTime={}&endTime={}",
                query_instant(at(health)),
                query_instant(at(aggregate))
            ),
            vec![arn(aggregate), arn(ec2), arn(health)],
        ),
        (
            "a window whose earliest is after its latest selects nothing",
            format!(
                "startTime={}&endTime={}",
                query_instant(at(aggregate)),
                query_instant(at(health))
            ),
            Vec::new(),
        ),
        (
            "a window that ends before the oldest delivery selects nothing",
            format!(
                "endTime={}",
                query_instant(DateTime::from_secs(FIRST_DELIVERY_SECS - 1))
            ),
            Vec::new(),
        ),
        (
            "organizationalUnitId names an organizational unit",
            "organizationalUnitId=ou-abc1-1a2b3c4d".to_string(),
            vec![arn(ec2)],
        ),
        (
            "organizationalUnitId names the organization root",
            "organizationalUnitId=Root".to_string(),
            vec![arn(newest)],
        ),
        (
            "organizationalUnitId names a unit nothing was delivered for",
            "organizationalUnitId=r-abcd".to_string(),
            Vec::new(),
        ),
        (
            "aggregateNotificationEventArn with includeChildEvents reports that aggregate's children",
            format!(
                "aggregateNotificationEventArn={}&includeChildEvents=true",
                encode_path_label(&arn(aggregate))
            ),
            vec![arn(second_child), arn(first_child)],
        ),
        (
            "aggregateNotificationEventArn alone admits no child",
            format!(
                "aggregateNotificationEventArn={}",
                encode_path_label(&arn(aggregate))
            ),
            Vec::new(),
        ),
        (
            "filters that disagree select nothing",
            "source=aws.health&organizationalUnitId=Root".to_string(),
            Vec::new(),
        ),
    ];

    for (case, query, expected) in cases {
        let (status, response) = emulator.list_events(&query).await;
        assert_eq!(
            status, 200,
            "case {case:?}: {query} must answer 200, got {response:?}"
        );
        assert_eq!(
            reported_arns(&response),
            expected,
            "case {case:?}: {query} reported the wrong events, got {response:?}"
        );
    }
}

/// RULE: a listing is cut into pages of at most `maxResults`, each page carrying
/// a resume token exactly while events remain behind it, and the pages
/// concatenate into the unpaged listing.
#[tokio::test]
async fn pages_walk_the_whole_listing() {
    let (emulator, _) = seeded_emulator().await;
    let (_, whole) = emulator.list_events("includeChildEvents=true").await;
    let expected = reported_arns(&whole);
    assert_eq!(
        expected.len(),
        seeds().len(),
        "the fixture must list every seeded event, got {whole:?}"
    );

    // Every page size from one event to one past the whole listing, plus the
    // modelled `maxResults` maximum.
    for page_size in [1_usize, 2, 3, 6, 7, 8, 100] {
        let case = format!("maxResults={page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0_usize;
        loop {
            let mut query = format!("includeChildEvents=true&maxResults={page_size}");
            if let Some(cursor) = &token {
                query.push_str(&format!("&nextToken={}", encode_path_label(cursor)));
            }
            let (status, response) = emulator.list_events(&query).await;
            assert_eq!(
                status, 200,
                "case {case:?}: page {pages} must answer 200, got {response:?}"
            );
            let page = reported_arns(&response);
            assert!(
                page.len() <= page_size,
                "case {case:?}: page {pages} must hold at most {page_size} events, got {page:?}"
            );
            walked.extend(page.iter().cloned());
            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "case {case:?}: paging must terminate, walked {walked:?}"
            );
            match response["nextToken"].as_str() {
                Some(next) => {
                    assert!(
                        walked.len() < expected.len(),
                        "case {case:?}: a token must be answered only while events remain, got {response:?}"
                    );
                    token = Some(next.to_string());
                }
                None => break,
            }
        }
        assert_eq!(
            walked, expected,
            "case {case:?}: the pages must concatenate into the unpaged listing"
        );
    }
}

/// RULE: a `nextToken` this service did not issue is refused as a validation
/// failure naming the member, rather than silently restarting the listing.
#[tokio::test]
async fn a_foreign_page_token_is_refused() {
    let (emulator, _) = seeded_emulator().await;
    let cases = [
        ("a token of the right shape this service never issued", "0".repeat(32)),
        ("a token that is not hex at all", "not-a-token".to_string()),
        ("a token of the wrong length", "abcdef".to_string()),
    ];
    for (case, token) in cases {
        let query = format!("nextToken={}", encode_path_label(&token));
        let (status, response) = emulator.list_events(&query).await;
        assert_eq!(
            status, 400,
            "case {case:?}: {token:?} must be refused with 400, got {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {response:?}"
        );
        assert_eq!(
            response["fieldList"][0]["name"].as_str(),
            Some("nextToken"),
            "case {case:?}: the refusal must name nextToken, got {response:?}"
        );
    }
}

/// RULE: reporting is a read — listing events, with or without filters, changes
/// nothing a later caller sees.
#[tokio::test]
async fn listing_does_not_change_what_a_later_caller_sees() {
    let (emulator, _) = seeded_emulator().await;
    let before = emulator.stored_event_count();
    let queries = [
        "",
        "includeChildEvents=true",
        "source=aws.cloudwatch",
        "maxResults=1",
    ];
    let mut first: Vec<Vec<String>> = Vec::new();
    for query in queries {
        let (_, response) = emulator.list_events(query).await;
        first.push(reported_arns(&response));
    }
    for (index, query) in queries.iter().enumerate() {
        let (status, response) = emulator.list_events(query).await;
        assert_eq!(
            status, 200,
            "case {query:?}: the repeated listing must answer 200, got {response:?}"
        );
        assert_eq!(
            reported_arns(&response),
            first[index],
            "case {query:?}: a second listing must report the same events"
        );
    }
    assert_eq!(
        emulator.stored_event_count(),
        before,
        "listing must not change how many events the store holds"
    );
}

/// RULE: a notification event exists because an `EventRule` of a configuration
/// matched, so a delivery naming a configuration the store does not hold records
/// nothing and the listing stays as it was.
#[tokio::test]
async fn a_delivery_for_an_unknown_configuration_records_nothing() {
    let (emulator, _) = seeded_emulator().await;
    let before = emulator.stored_event_count();
    let unknown = format!(
        "arn:aws:notifications::{}:configuration/{}",
        engine::emulator_account_id(),
        "z".repeat(27)
    );
    let recorded = {
        let state = emulator
            .engine
            .state::<ComAmazonawsNotificationsNotificationsState>();
        let mut store = state.lock();
        store
            .record_notification_event(NotificationEventDelivery {
                notification_configuration_arn: unknown.clone(),
                region: REGION.to_string(),
                creation_time: DateTime::from_secs(FIRST_DELIVERY_SECS),
                source: "aws.cloudwatch".to_string(),
                event_type: "CloudWatch Alarm State Change".to_string(),
                event_origin_region: Some(REGION.to_string()),
                headline: "an event for a configuration that does not exist".to_string(),
                event_status: "UNHEALTHY".to_string(),
                notification_type: "ALERT".to_string(),
                related_account: None,
                aggregation_event_type: AGGREGATION_EVENT_TYPE_NONE.to_string(),
                aggregate_notification_event_arn: None,
                aggregation_summary: None,
                organizational_unit_id: None,
            })
            .map(|record| record.arn.clone())
    };
    assert!(
        recorded.is_none(),
        "a delivery for {unknown:?} must record nothing, got {recorded:?}"
    );
    assert_eq!(
        emulator.stored_event_count(),
        before,
        "a refused delivery must leave the store as it was"
    );
}
