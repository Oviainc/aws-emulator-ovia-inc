// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHICH destinations an `UpdateFunctionEventInvokeConfig` leaves
//! behind on the asynchronous-invocation configuration of a function, version or alias.
//!
//! The rule under test: an update changes only the settings its request STATES — "To set one
//! option without affecting existing settings for other options, use
//! UpdateFunctionEventInvokeConfig" — and the two destination slots of `DestinationConfig`
//! (`OnSuccess` and `OnFailure`, each a structure of its own carrying one `DestinationArn`,
//! from `smithy_query(query="shape", shape="DestinationConfig")`) are settings in that same
//! sense: the slot a request states replaces the destination the configuration holds, and the
//! slot it states nothing about keeps the one it holds. A slot stated with no destination —
//! the empty string `DestinationArn`'s pattern admits (`^$|kafka://…|arn:…`), or no
//! `Destination` member at all — is how a caller takes that destination away, so one slot can
//! be cleared while the other stands. A put is the other half of the same sentence and is not
//! under test here: its request states the configuration whole.
//!
//! Every row drives real HTTP requests through the router: it puts a seed configuration, sends
//! one update, and then asserts the pair of destinations BOTH on the update's own 200 answer
//! and on the `GetFunctionEventInvokeConfig` read-back, so a destination that was echoed but
//! never stored — and one that was stored but dropped from the echo — fails here. The rows
//! differ along what the rule generalises over: WHICH slot the request states (neither, one,
//! the other, both), what it states in it (each kind of destination the model documents, the
//! empty string, a slot with no `Destination` member at all, an empty destination
//! configuration), and what the configuration held before (both slots, one, neither) — and the
//! whole table runs on two qualifiers, the unpublished version and a published one, because
//! the configuration is a resource of ONE qualifier.

use std::sync::Arc;

use serde_json::{json, Value};

// How this resource is addressed, how its `FunctionArn` is formed and how a version is
// published are the same here as in the suite that covers the resource as a whole, so the
// two modules cannot drift on them: they are its helpers, imported rather than restated.
use crate::event_invoke_config::{config_uri, configured_arn, publish_version};
use crate::event_source_mappings::{call, create_function, router_on, Answer};

/// One destination per KIND the model documents for an asynchronous-invocation
/// configuration, from `FunctionEventInvokeConfig$DestinationConfig`: "Function - The Amazon
/// Resource Name (ARN) of a Lambda function. Queue - The ARN of a standard SQS queue.
/// Bucket - The ARN of an Amazon S3 bucket. Topic - The ARN of a standard SNS topic.
/// Event Bus - The ARN of an Amazon EventBridge event bus."
const QUEUE: &str = "arn:aws:sqs:us-east-1:000000000000:slot-records";
/// See [`QUEUE`].
const TOPIC: &str = "arn:aws:sns:us-east-1:000000000000:slot-records";
/// See [`QUEUE`]. The default bus, which is a bus name no row below configures as a seed.
const BUS: &str = "arn:aws:events:us-east-1:000000000000:event-bus/default";
/// See [`QUEUE`].
const FUNCTION: &str = "arn:aws:lambda:us-east-1:000000000000:function:slot-records";
/// See [`QUEUE`].
const BUCKET: &str = "arn:aws:s3:::slot-records";

/// The retry and age settings every seed carries, so a row that states neither shows them
/// surviving the update untouched. Both are inside the published ranges
/// (`MaximumRetryAttempts` `@range(min: 0, max: 2)`, `MaximumEventAgeInSeconds`
/// `@range(min: 60, max: 21600)`).
const SEED_RETRIES: i64 = 1;
/// See [`SEED_RETRIES`].
const SEED_AGE: i64 = 300;

/// The retry count a row that states one asks for, which is the other end of that range.
const STATED_RETRIES: i64 = 0;

/// The destinations one configuration holds, slot by slot: `None` for a slot holding none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Slots {
    on_success: Option<&'static str>,
    on_failure: Option<&'static str>,
}

impl Slots {
    /// The `DestinationConfig` a request states to ask for exactly these destinations, with
    /// a slot holding none left out of the request altogether.
    fn stated(&self) -> Value {
        let mut config = json!({});
        if let Some(destination) = self.on_success {
            config["OnSuccess"] = json!({ "Destination": destination });
        }
        if let Some(destination) = self.on_failure {
            config["OnFailure"] = json!({ "Destination": destination });
        }
        config
    }
}

/// One row: what the configuration held, what one update states, and what it holds after.
struct Row {
    case: &'static str,
    /// The destinations the seed put stores.
    seeded: Slots,
    /// The body of the one update under test.
    update: Value,
    /// The destinations the configuration holds afterwards.
    expected: Slots,
    /// The `MaximumRetryAttempts` the configuration holds afterwards, which is the seed's
    /// unless the row's update states one of its own: the destination slots are settings
    /// beside it, so moving one must not move the other.
    retries: i64,
}

/// The table: one row per way the rule can be exercised.
fn rows() -> Vec<Row> {
    let both = Slots {
        on_success: Some(TOPIC),
        on_failure: Some(QUEUE),
    };
    let neither = Slots {
        on_success: None,
        on_failure: None,
    };
    vec![
        Row {
            case: "an update stating no destination configuration at all keeps both slots",
            seeded: both,
            update: json!({ "MaximumRetryAttempts": STATED_RETRIES }),
            expected: both,
            retries: STATED_RETRIES,
        },
        Row {
            case: "an update stating only the on-failure slot keeps the on-success one",
            seeded: both,
            update: json!({ "DestinationConfig": { "OnFailure": { "Destination": BUS } } }),
            expected: Slots {
                on_failure: Some(BUS),
                ..both
            },
            retries: SEED_RETRIES,
        },
        Row {
            case: "an update stating only the on-success slot keeps the on-failure one",
            seeded: both,
            update: json!({ "DestinationConfig": { "OnSuccess": { "Destination": FUNCTION } } }),
            expected: Slots {
                on_success: Some(FUNCTION),
                ..both
            },
            retries: SEED_RETRIES,
        },
        Row {
            case: "an update stating both slots replaces both",
            seeded: both,
            update: json!({
                "DestinationConfig": {
                    "OnSuccess": { "Destination": BUS },
                    "OnFailure": { "Destination": BUCKET },
                }
            }),
            expected: Slots {
                on_success: Some(BUS),
                on_failure: Some(BUCKET),
            },
            retries: SEED_RETRIES,
        },
        Row {
            case: "an update stating the empty destination the pattern admits clears that \
                   slot alone",
            seeded: both,
            update: json!({ "DestinationConfig": { "OnFailure": { "Destination": "" } } }),
            expected: Slots {
                on_failure: None,
                ..both
            },
            retries: SEED_RETRIES,
        },
        Row {
            case: "an update stating a slot with no destination member clears that slot alone",
            seeded: both,
            update: json!({ "DestinationConfig": { "OnSuccess": {} } }),
            expected: Slots {
                on_success: None,
                ..both
            },
            retries: SEED_RETRIES,
        },
        Row {
            case: "an update stating an empty destination configuration states no slot and \
                   keeps both",
            seeded: both,
            update: json!({ "DestinationConfig": {} }),
            expected: both,
            retries: SEED_RETRIES,
        },
        Row {
            case: "an update fills the slot a configuration holding neither left empty",
            seeded: neither,
            update: json!({ "DestinationConfig": { "OnFailure": { "Destination": TOPIC } } }),
            expected: Slots {
                on_success: None,
                on_failure: Some(TOPIC),
            },
            retries: SEED_RETRIES,
        },
        Row {
            case: "an update fills the other slot of a configuration holding one",
            seeded: Slots {
                on_success: None,
                on_failure: Some(BUCKET),
            },
            update: json!({ "DestinationConfig": { "OnSuccess": { "Destination": QUEUE } } }),
            expected: Slots {
                on_success: Some(QUEUE),
                on_failure: Some(BUCKET),
            },
            retries: SEED_RETRIES,
        },
        Row {
            case: "an update of a slot already holding that destination leaves the pair as \
                   it stands",
            seeded: both,
            update: json!({ "DestinationConfig": { "OnFailure": { "Destination": QUEUE } } }),
            expected: both,
            retries: SEED_RETRIES,
        },
    ]
}

/// The destinations `answer` reports, slot by slot, with the slot that is absent from the
/// answer altogether reported as absent so a dropped slot cannot pass as an empty one.
fn reported_slots(case: &str, operation: &str, answer: &Answer) -> (Option<String>, Option<String>) {
    let body = answer.json();
    let config = body.get("DestinationConfig").unwrap_or_else(|| {
        panic!("case {case:?}: expected {operation} to report a DestinationConfig, got {body}")
    });
    let read = |slot: &str| {
        config
            .get(slot)
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected {operation} to report the {slot} slot, got {body}")
            })
            .get("Destination")
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    (read("OnSuccess"), read("OnFailure"))
}

/// Assert that `answer` is a 200 reporting exactly `expected` on `arn`, with `retries` as
/// the retry setting and the seed's event age still in force — the destination slots are
/// settings beside those two, so a row must move only what it states.
fn assert_holds(
    case: &str,
    operation: &str,
    answer: &Answer,
    expected: &Slots,
    retries: i64,
    arn: &str,
) {
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: expected 200 from {operation}, got {} {}",
        answer.status,
        answer.text()
    );
    let body = answer.json();
    assert_eq!(
        body.get("FunctionArn").and_then(Value::as_str),
        Some(arn),
        "case {case:?}: expected {operation} to report FunctionArn {arn:?}, got {body}"
    );
    assert_eq!(
        body.get("MaximumRetryAttempts").and_then(Value::as_i64),
        Some(retries),
        "case {case:?}: expected {operation} to report MaximumRetryAttempts {retries}, got \
         {body}"
    );
    assert_eq!(
        body.get("MaximumEventAgeInSeconds").and_then(Value::as_i64),
        Some(SEED_AGE),
        "case {case:?}: expected {operation} to keep MaximumEventAgeInSeconds {SEED_AGE}, got \
         {body}"
    );
    let (on_success, on_failure) = reported_slots(case, operation, answer);
    assert_eq!(
        on_success.as_deref(),
        expected.on_success,
        "case {case:?}: expected {operation} to report OnSuccess {:?}, got {on_success:?}",
        expected.on_success
    );
    assert_eq!(
        on_failure.as_deref(),
        expected.on_failure,
        "case {case:?}: expected {operation} to report OnFailure {:?}, got {on_failure:?}",
        expected.on_failure
    );
}

#[tokio::test]
async fn an_update_states_each_destination_slot_on_its_own() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    for (index, row) in rows().into_iter().enumerate() {
        let function = format!("event-invoke-slots-{index}");
        create_function(&router, &function).await;
        let published = publish_version(&router, &function).await;

        // The configuration is a resource of ONE qualifier, so every row is driven on both
        // the unpublished version (addressed with no qualifier at all) and a published one.
        for qualifier in [None, Some(published.as_str())] {
            let case = format!("{} (qualifier {:?})", row.case, qualifier);
            let arn = configured_arn(&function, qualifier);

            let mut seed = json!({
                "MaximumRetryAttempts": SEED_RETRIES,
                "MaximumEventAgeInSeconds": SEED_AGE,
            });
            let stated = row.seeded.stated();
            if stated.as_object().is_some_and(|slots| !slots.is_empty()) {
                seed["DestinationConfig"] = stated;
            }
            let seeded = call(
                &router,
                "PUT",
                &config_uri(&function, qualifier),
                &seed,
            )
            .await;
            assert_holds(
                &format!("{case} (seed)"),
                "PutFunctionEventInvokeConfig",
                &seeded,
                &row.seeded,
                SEED_RETRIES,
                &arn,
            );

            let updated = call(
                &router,
                "POST",
                &config_uri(&function, qualifier),
                &row.update,
            )
            .await;
            assert_holds(
                &case,
                "UpdateFunctionEventInvokeConfig",
                &updated,
                &row.expected,
                row.retries,
                &arn,
            );

            // The echo is only half of it: the read-back is what a client, and an auditor,
            // uses to see which destinations the function actually has.
            let fetched = call(
                &router,
                "GET",
                &config_uri(&function, qualifier),
                &Value::Null,
            )
            .await;
            assert_holds(
                &case,
                "GetFunctionEventInvokeConfig",
                &fetched,
                &row.expected,
                row.retries,
                &arn,
            );
        }
    }
}
