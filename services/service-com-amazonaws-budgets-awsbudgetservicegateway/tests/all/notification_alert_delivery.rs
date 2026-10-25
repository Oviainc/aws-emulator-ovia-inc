// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the ALERT a budget notification delivers: a notification that
//! the service holds in the `ALARM` state publishes a budget alert to each of its `SNS`
//! subscribers' topics, driven over the wire through the same `MultiServiceRouter` the
//! production router serves from, with SNS mounted beside budgets on ONE engine so the
//! alert is asserted where a client sees it — on the topic the subscriber named.
//!
//! The RULE asserted here: a notification in `ALARM` has passed the threshold it alerts
//! on (`Notification$NotificationState`: "specifies whether this notification is in
//! alarm. If a budget notification is in the `ALARM` state, you passed the set threshold
//! for the budget"), so every write that leaves such a notification holding an `SNS`
//! subscriber publishes ONE alert, naming the budget and what the notification alerts
//! on, to the topic that subscriber's `Address` names ("the address that Amazon Web
//! Services sends budget notifications to"). A notification that is not in alarm owes
//! nothing, and an address that names no topic of this emulator is published to nothing.
//!
//! The table spans the class rather than one recorded request: its rows differ along the
//! write that establishes the pairing (`CreateBudget`, `CreateNotification`,
//! `CreateSubscriber`), the `NotificationState` (absent, `OK`, `ALARM`), the subscriber
//! types in the set, the Region of the topic, the budget type and the limit the alert
//! names, and what the notification alerts on — the notification type, the comparison
//! operator, both published `Threshold` bounds and a fractional value in between, and
//! whether `ThresholdType` was supplied.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::{EmbeddedEmulator, MultiServiceRouter};

use service_com_amazonaws_budgets_awsbudgetservicegateway::state::{
    PERCENTAGE_LIMIT_AMOUNT, PERCENTAGE_LIMIT_UNIT, THRESHOLD_TYPE_PERCENTAGE,
};
use service_com_amazonaws_sns_amazonsimplenotificationservice::ComAmazonawsSnsAmazonsimplenotificationserviceState;

use super::budget_lifecycle::{caller_account, call, same};

/// The Region the budgets requests below are signed for, and the Region the topic a row
/// names lives in unless the row says otherwise.
const REGION: &str = "us-east-1";

/// A second Region, for the row whose topic is NOT in the Region the budgets request was
/// signed for: a budget is global, so the Region an alert is published in can only come
/// from the topic's own ARN.
const OTHER_REGION: &str = "eu-west-2";

/// The account every budget below belongs to: the one account this emulator answers
/// for, which is the only `AccountId` a budgets operation serves.
fn account_id() -> String {
    caller_account()
}

/// The emulator: budgets and SNS on ONE engine — the shape `core/router/src/lib.rs`
/// mounts them in — because a budget alert is published through the SNS API and read
/// back where SNS delivered it.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_budgets_awsbudgetservicegateway::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_sns_amazonsimplenotificationservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    Emulator { router, engine }
}

/// An SNS client for `region` over the in-process callback into this emulator — the same
/// seam the service itself publishes an alert through, so the suite's setup reaches SNS
/// exactly the way the handler does.
fn sns(emulator: &Emulator, region: &str) -> ::aws_sdk_sns::Client {
    let callback = EmbeddedEmulator::from_router(emulator.router.clone());
    let config = ::aws_sdk_sns::Config::builder()
        .behavior_version(::aws_sdk_sns::config::BehaviorVersion::latest())
        .region(::aws_sdk_sns::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://sns.{region}.amazonaws.com"))
        .credentials_provider(callback.aws_credentials_provider())
        .http_client(callback.http_client())
        .retry_config(::aws_sdk_sns::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_sns::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_sns::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_sns::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_sns::Client::from_conf(config)
}

/// The ARN of the topic `name` in `region`, as SNS mints it for this emulator's account.
/// Every setup below asserts the ARN SNS answered against this, so the rows can name
/// their subscriber addresses up front.
fn topic_arn(region: &str, name: &str) -> String {
    format!(
        "arn:aws:sns:{region}:{}:{name}",
        ::engine::emulator_account_id()
    )
}

/// The topic `name` in `region`, with one confirmed subscription on it so a publish onto
/// it is observable — SNS "delivers the message to each endpoint that is subscribed to
/// the topic", and a topic with no subscription holds no delivery to read back.
async fn given_topic(emulator: &Emulator, region: &str, name: &str) {
    let client = sns(emulator, region);
    let created = client
        .create_topic()
        .name(name)
        .send()
        .await
        .unwrap_or_else(|error| panic!("setup: CreateTopic({name}) failed: {error:?}"));
    let arn = created.topic_arn().unwrap_or_default();
    assert_eq!(
        arn,
        topic_arn(region, name),
        "setup: CreateTopic({name}) answered an ARN the rows do not name"
    );
    let queue = format!(
        "arn:aws:sqs:{region}:{}:{name}-inbox",
        ::engine::emulator_account_id()
    );
    client
        .subscribe()
        .topic_arn(arn)
        .protocol("sqs")
        .endpoint(&queue)
        .send()
        .await
        .unwrap_or_else(|error| panic!("setup: Subscribe({name}) failed: {error:?}"));
}

/// Every alert SNS delivered, as `(topic ARN, message)` pairs with the message parsed,
/// sorted so a comparison does not depend on which subscription SNS recorded first.
fn delivered(emulator: &Emulator) -> Vec<(String, Value)> {
    let mut found: Vec<(String, Value)> = emulator
        .engine
        .state::<ComAmazonawsSnsAmazonsimplenotificationserviceState>()
        .lock()
        .delivered
        .values()
        .flatten()
        .map(|published| {
            let parsed = serde_json::from_str::<Value>(&published.message).unwrap_or_else(|error| {
                panic!(
                    "a published alert is not JSON ({error}): {:?}",
                    published.message
                )
            });
            (published.topic_arn.clone(), parsed)
        })
        .collect();
    found.sort_by_key(|(topic, message)| (topic.clone(), message.to_string()));
    found
}

/// An email subscriber: "either an Amazon SNS topic or an email address", on the half
/// this emulator hosts no transport for.
fn email_subscriber(n: usize) -> Value {
    json!({"SubscriptionType": "EMAIL", "Address": format!("ops{n}@example.com")})
}

/// An SNS subscriber whose `Address` is `address`.
fn sns_subscriber(address: &str) -> Value {
    json!({"SubscriptionType": "SNS", "Address": address})
}

/// Which write attaches the row's notification to its budget.
#[derive(Debug, Clone, Copy)]
enum Attach {
    /// `CreateBudget`, which "creates a budget and, if included, notifications and
    /// subscribers".
    WithBudget,
    /// `CreateNotification`, against a budget that already exists.
    AfterBudget,
}

/// One row: the budget, the notification and subscribers attached to it, a subscriber
/// attached afterwards with `CreateSubscriber`, and the alerts that must have been
/// published once all of that has run.
struct Row {
    case: &'static str,
    attach: Attach,
    /// The `Budget` shape, minus its name (the runner gives each row its own).
    budget: Value,
    notification: Value,
    subscribers: Value,
    /// A subscriber attached with `CreateSubscriber` after the notification exists.
    then_subscribe: Option<Value>,
    /// `(topic ARN, alert)` for every alert expected, in any order.
    expected: Vec<(String, Value)>,
}

/// The topic every row but the cross-Region one names.
fn primary_topic() -> String {
    topic_arn(REGION, "budget-alerts")
}

/// The topic in [`OTHER_REGION`].
fn other_topic() -> String {
    topic_arn(OTHER_REGION, "budget-alerts")
}

/// The alert a notification of `budget_name` publishes, as the rule describes it: the
/// budget it is about, what that budget tracks and over what period, and what the
/// notification alerts on.
#[allow(clippy::too_many_arguments)]
fn alert(
    budget_name: &str,
    budget_type: &str,
    time_unit: &str,
    limit: Option<(&str, &str)>,
    notification_type: &str,
    comparison_operator: &str,
    threshold: f64,
    threshold_type: &str,
) -> Value {
    let mut expected = json!({
        "accountId": account_id(),
        "budgetName": budget_name,
        "budgetType": budget_type,
        "timeUnit": time_unit,
        "notificationType": notification_type,
        "comparisonOperator": comparison_operator,
        "threshold": threshold,
        "thresholdType": threshold_type,
        "notificationState": "ALARM",
    });
    if let Some((amount, unit)) = limit {
        expected["budgetLimit"] = json!({"amount": amount, "unit": unit});
    }
    expected
}

/// A `COST` budget on a monthly period with a 100 USD limit — the budget shape the rows
/// that vary only the notification use.
fn cost_budget() -> Value {
    json!({
        "TimeUnit": "MONTHLY",
        "BudgetType": "COST",
        "BudgetLimit": {"Amount": "100", "Unit": "USD"}
    })
}

/// The table. Thresholds and value sets come from the model
/// (`smithy_query(query="shape",shape="Notification,NotificationThreshold,NotificationState,SubscriptionType")`):
/// `NotificationThreshold` is `@range(min: 0, max: 15000000000000)`, `NotificationState`
/// is `OK | ALARM` and `SubscriptionType` is `SNS | EMAIL`.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a notification created in alarm by CreateNotification, with an SNS subscriber",
            attach: Attach::AfterBudget,
            budget: cost_budget(),
            notification: json!({
                "NotificationType": "ACTUAL",
                "ComparisonOperator": "GREATER_THAN",
                "Threshold": 80,
                "NotificationState": "ALARM"
            }),
            subscribers: json!([sns_subscriber(&primary_topic())]),
            then_subscribe: None,
            expected: vec![(
                primary_topic(),
                alert(
                    "alert-row-0",
                    "COST",
                    "MONTHLY",
                    Some(("100", "USD")),
                    "ACTUAL",
                    "GREATER_THAN",
                    80.0,
                    // Supplied by neither the row nor the budget: "thresholds are always
                    // a percentage", so a notification naming no `ThresholdType` alerts
                    // as the percentage it is.
                    THRESHOLD_TYPE_PERCENTAGE,
                ),
            )],
        },
        Row {
            case: "a notification attached in alarm by CreateBudget, beside email subscribers \
                   that have no transport",
            attach: Attach::WithBudget,
            budget: json!({
                "TimeUnit": "ANNUALLY",
                "BudgetType": "USAGE",
                "BudgetLimit": {"Amount": "2500.5", "Unit": "GB"}
            }),
            notification: json!({
                "NotificationType": "FORECASTED",
                "ComparisonOperator": "LESS_THAN",
                "Threshold": 42.5,
                "ThresholdType": "ABSOLUTE_VALUE",
                "NotificationState": "ALARM"
            }),
            subscribers: json!([
                email_subscriber(1),
                sns_subscriber(&primary_topic()),
                email_subscriber(2)
            ]),
            then_subscribe: None,
            expected: vec![(
                primary_topic(),
                alert(
                    "alert-row-1",
                    "USAGE",
                    "ANNUALLY",
                    Some(("2500.5", "GB")),
                    "FORECASTED",
                    "LESS_THAN",
                    42.5,
                    "ABSOLUTE_VALUE",
                ),
            )],
        },
        Row {
            case: "an SNS subscriber attached by CreateSubscriber to a notification already in \
                   alarm, at the published minimum threshold",
            attach: Attach::AfterBudget,
            budget: cost_budget(),
            notification: json!({
                "NotificationType": "ACTUAL",
                "ComparisonOperator": "EQUAL_TO",
                "Threshold": 0,
                "NotificationState": "ALARM"
            }),
            subscribers: json!([email_subscriber(1)]),
            then_subscribe: Some(sns_subscriber(&primary_topic())),
            expected: vec![(
                primary_topic(),
                alert(
                    "alert-row-2",
                    "COST",
                    "MONTHLY",
                    Some(("100", "USD")),
                    "ACTUAL",
                    "EQUAL_TO",
                    0.0,
                    THRESHOLD_TYPE_PERCENTAGE,
                ),
            )],
        },
        Row {
            case: "a topic in another Region, at the published maximum threshold",
            attach: Attach::AfterBudget,
            budget: cost_budget(),
            notification: json!({
                "NotificationType": "FORECASTED",
                "ComparisonOperator": "GREATER_THAN",
                "Threshold": 15_000_000_000_000_i64,
                "ThresholdType": "PERCENTAGE",
                "NotificationState": "ALARM"
            }),
            subscribers: json!([sns_subscriber(&other_topic())]),
            then_subscribe: None,
            expected: vec![(
                other_topic(),
                alert(
                    "alert-row-3",
                    "COST",
                    "MONTHLY",
                    Some(("100", "USD")),
                    "FORECASTED",
                    "GREATER_THAN",
                    15_000_000_000_000.0,
                    "PERCENTAGE",
                ),
            )],
        },
        Row {
            case: "a budget whose limit the SERVICE defaulted, so the alert names the stored \
                   limit rather than the request's",
            attach: Attach::WithBudget,
            budget: json!({
                "TimeUnit": "QUARTERLY",
                "BudgetType": "RI_UTILIZATION"
            }),
            notification: json!({
                "NotificationType": "ACTUAL",
                "ComparisonOperator": "LESS_THAN",
                "Threshold": 75,
                "NotificationState": "ALARM"
            }),
            subscribers: json!([sns_subscriber(&primary_topic())]),
            then_subscribe: None,
            expected: vec![(
                primary_topic(),
                alert(
                    "alert-row-4",
                    "RI_UTILIZATION",
                    "QUARTERLY",
                    Some((PERCENTAGE_LIMIT_AMOUNT, PERCENTAGE_LIMIT_UNIT)),
                    "ACTUAL",
                    "LESS_THAN",
                    75.0,
                    THRESHOLD_TYPE_PERCENTAGE,
                ),
            )],
        },
        Row {
            case: "a notification in alarm whose only subscribers are email addresses",
            attach: Attach::AfterBudget,
            budget: cost_budget(),
            notification: json!({
                "NotificationType": "ACTUAL",
                "ComparisonOperator": "GREATER_THAN",
                "Threshold": 80,
                "NotificationState": "ALARM"
            }),
            subscribers: json!([email_subscriber(1), email_subscriber(2)]),
            then_subscribe: None,
            expected: Vec::new(),
        },
        Row {
            case: "a notification created with no NotificationState, so it is not in alarm",
            attach: Attach::AfterBudget,
            budget: cost_budget(),
            notification: json!({
                "NotificationType": "ACTUAL",
                "ComparisonOperator": "GREATER_THAN",
                "Threshold": 80
            }),
            subscribers: json!([sns_subscriber(&primary_topic())]),
            then_subscribe: None,
            expected: Vec::new(),
        },
        Row {
            case: "a notification created explicitly OK",
            attach: Attach::AfterBudget,
            budget: cost_budget(),
            notification: json!({
                "NotificationType": "ACTUAL",
                "ComparisonOperator": "GREATER_THAN",
                "Threshold": 80,
                "NotificationState": "OK"
            }),
            subscribers: json!([sns_subscriber(&primary_topic())]),
            then_subscribe: None,
            expected: Vec::new(),
        },
        Row {
            case: "a notification attached OK by CreateBudget",
            attach: Attach::WithBudget,
            budget: cost_budget(),
            notification: json!({
                "NotificationType": "FORECASTED",
                "ComparisonOperator": "GREATER_THAN",
                "Threshold": 120,
                "NotificationState": "OK"
            }),
            subscribers: json!([sns_subscriber(&primary_topic())]),
            then_subscribe: None,
            expected: Vec::new(),
        },
        Row {
            case: "an EMAIL subscriber attached by CreateSubscriber to a notification in alarm",
            attach: Attach::AfterBudget,
            budget: cost_budget(),
            notification: json!({
                "NotificationType": "ACTUAL",
                "ComparisonOperator": "GREATER_THAN",
                "Threshold": 90,
                "NotificationState": "ALARM"
            }),
            subscribers: json!([email_subscriber(1)]),
            then_subscribe: Some(email_subscriber(2)),
            expected: Vec::new(),
        },
        Row {
            case: "an SNS subscriber whose address names no topic at all",
            attach: Attach::AfterBudget,
            budget: cost_budget(),
            notification: json!({
                "NotificationType": "ACTUAL",
                "ComparisonOperator": "GREATER_THAN",
                "Threshold": 80,
                "NotificationState": "ALARM"
            }),
            subscribers: json!([sns_subscriber("budget-alerts")]),
            then_subscribe: None,
            expected: Vec::new(),
        },
        Row {
            case: "an SNS subscriber naming a topic in a Region nothing can be addressed in",
            attach: Attach::AfterBudget,
            budget: cost_budget(),
            notification: json!({
                "NotificationType": "ACTUAL",
                "ComparisonOperator": "GREATER_THAN",
                "Threshold": 80,
                "NotificationState": "ALARM"
            }),
            subscribers: json!([sns_subscriber(
                "arn:aws:sns:us-east-1/evil:123456789012:budget-alerts"
            )]),
            then_subscribe: None,
            expected: Vec::new(),
        },
        Row {
            case: "an SNS subscriber naming a topic this emulator does not hold",
            attach: Attach::AfterBudget,
            budget: cost_budget(),
            notification: json!({
                "NotificationType": "ACTUAL",
                "ComparisonOperator": "GREATER_THAN",
                "Threshold": 80,
                "NotificationState": "ALARM"
            }),
            subscribers: json!([sns_subscriber(&topic_arn(REGION, "no-such-topic"))]),
            then_subscribe: None,
            expected: Vec::new(),
        },
    ]
}

/// Run one row's writes against a fresh emulator and answer what was delivered.
async fn run(row: &Row, name: &str) -> Vec<(String, Value)> {
    let emulator = emulator();
    given_topic(&emulator, REGION, "budget-alerts").await;
    given_topic(&emulator, OTHER_REGION, "budget-alerts").await;

    let mut budget = row.budget.clone();
    budget["BudgetName"] = json!(name);
    let notification_with_subscribers = json!([{
        "Notification": row.notification,
        "Subscribers": row.subscribers
    }]);
    let create = match row.attach {
        Attach::WithBudget => json!({
            "AccountId": account_id(),
            "Budget": budget,
            "NotificationsWithSubscribers": notification_with_subscribers
        }),
        Attach::AfterBudget => json!({"AccountId": account_id(), "Budget": budget}),
    };
    let (status, body) = call(&emulator.router, "CreateBudget", &create).await;
    assert_eq!(
        status, 200,
        "case {:?}: CreateBudget answered {status} {body}",
        row.case
    );

    if matches!(row.attach, Attach::AfterBudget) {
        let (status, body) = call(
            &emulator.router,
            "CreateNotification",
            &json!({
                "AccountId": account_id(),
                "BudgetName": name,
                "Notification": row.notification,
                "Subscribers": row.subscribers
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: CreateNotification answered {status} {body}",
            row.case
        );
    }

    if let Some(subscriber) = &row.then_subscribe {
        let (status, body) = call(
            &emulator.router,
            "CreateSubscriber",
            &json!({
                "AccountId": account_id(),
                "BudgetName": name,
                "Notification": row.notification,
                "Subscriber": subscriber
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: CreateSubscriber answered {status} {body}",
            row.case
        );
    }

    // The notification is read back through the implemented reader, so every row also
    // proves the write it asserts on actually persisted.
    let (status, read) = call(
        &emulator.router,
        "DescribeNotificationsForBudget",
        &json!({"AccountId": account_id(), "BudgetName": name}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: DescribeNotificationsForBudget answered {status} {read}",
        row.case
    );
    let held = read["Notifications"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        held.len(),
        1,
        "case {:?}: expected the budget to hold one notification, got {held:?}",
        row.case
    );

    delivered(&emulator)
}

/// Rule: a notification in `ALARM` publishes one budget alert — naming the budget, what
/// it tracks and what the notification alerts on — to each of its `SNS` subscribers'
/// topics, and a notification that is not in alarm, or whose address names no topic this
/// emulator holds, publishes nothing.
#[tokio::test]
async fn a_notification_in_alarm_publishes_a_budget_alert_to_each_sns_subscribers_topic() {
    for (index, row) in rows().iter().enumerate() {
        let name = format!("alert-row-{index}");
        let found = run(row, &name).await;
        let mut expected = row.expected.clone();
        expected.sort_by_key(|(topic, message)| (topic.clone(), message.to_string()));
        assert_eq!(
            found.len(),
            expected.len(),
            "case {:?}: expected {} alert(s), got {found:?}",
            row.case,
            expected.len()
        );
        for (found, expected) in found.iter().zip(&expected) {
            assert_eq!(
                found.0, expected.0,
                "case {:?}: expected the alert on {}, got it on {}",
                row.case, expected.0, found.0
            );
            assert!(
                same(&found.1, &expected.1),
                "case {:?}: expected the alert {}, got {}",
                row.case,
                expected.1,
                found.1
            );
        }
    }
}

/// Rule: reading a notification back reports what is stored and delivers nothing — an
/// alert is owed by the WRITE that put the notification in alarm, so no number of reads
/// adds another, and a second notification in alarm on the same budget publishes its own.
#[tokio::test]
async fn alerts_are_owed_by_writes_only_and_one_per_notification_in_alarm() {
    let emulator = emulator();
    given_topic(&emulator, REGION, "budget-alerts").await;
    let topic = primary_topic();
    let name = "alert-reads";

    let (status, body) = call(
        &emulator.router,
        "CreateBudget",
        &json!({
            "AccountId": account_id(),
            "Budget": {
                "BudgetName": name,
                "TimeUnit": "MONTHLY",
                "BudgetType": "COST",
                "BudgetLimit": {"Amount": "100", "Unit": "USD"}
            }
        }),
    )
    .await;
    assert_eq!(status, 200, "setup: CreateBudget answered {status} {body}");

    // Two notifications of the same budget, each in alarm with the same SNS subscriber:
    // what a notification alerts on identifies it, so these are two notifications and
    // each owes its own alert.
    for threshold in [50, 90] {
        let notification = json!({
            "NotificationType": "ACTUAL",
            "ComparisonOperator": "GREATER_THAN",
            "Threshold": threshold,
            "NotificationState": "ALARM"
        });
        let (status, body) = call(
            &emulator.router,
            "CreateNotification",
            &json!({
                "AccountId": account_id(),
                "BudgetName": name,
                "Notification": notification,
                "Subscribers": [sns_subscriber(&topic)]
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case threshold {threshold}: CreateNotification answered {status} {body}"
        );
    }

    let after_writes = delivered(&emulator);
    assert_eq!(
        after_writes.len(),
        2,
        "case two notifications in alarm: expected one alert each, got {after_writes:?}"
    );
    let thresholds: Vec<Value> = after_writes
        .iter()
        .map(|(_, message)| message["threshold"].clone())
        .collect();
    for threshold in [json!(50.0), json!(90.0)] {
        assert!(
            thresholds.iter().any(|found| same(found, &threshold)),
            "case two notifications in alarm: expected an alert for threshold {threshold}, got \
             {thresholds:?}"
        );
    }

    // Every read of the notifications and their subscribers, run twice over.
    for round in 0..2 {
        for (operation, body) in [
            (
                "DescribeNotificationsForBudget",
                json!({"AccountId": account_id(), "BudgetName": name}),
            ),
            (
                "DescribeSubscribersForNotification",
                json!({
                    "AccountId": account_id(),
                    "BudgetName": name,
                    "Notification": {
                        "NotificationType": "ACTUAL",
                        "ComparisonOperator": "GREATER_THAN",
                        "Threshold": 50,
                        "NotificationState": "ALARM"
                    }
                }),
            ),
            (
                "DescribeBudget",
                json!({"AccountId": account_id(), "BudgetName": name}),
            ),
        ] {
            let (status, answered) = call(&emulator.router, operation, &body).await;
            assert_eq!(
                status, 200,
                "case read {operation} round {round}: answered {status} {answered}"
            );
        }
    }

    let after_reads = delivered(&emulator);
    assert_eq!(
        after_reads, after_writes,
        "case reads deliver nothing: expected the alerts to be unchanged by reads, got \
         {after_reads:?}"
    );
}
