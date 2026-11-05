// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the IDENTITY of a CloudWatch RUM metric definition on the
//! write path: one extended-metrics destination holds at most ONE definition of a given
//! metric, whichever operation writes it.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a signed
//! restJson1 request — so routing, the `@httpLabel` app-monitor name, the `@httpQuery`
//! bindings of the read, the modelled constraints, the handler and the error envelope are
//! all exercised rather than just a handler body. An update is asserted through
//! `BatchGetRumMetricDefinitions`, never through its own answer:
//! `UpdateRumMetricDefinitionResponse` models no member at all, so the PERSISTED
//! definitions are the only evidence of what a call did.
//!
//! The rule under test: a definition IS the metric it defines — the members a request
//! carries — and the `MetricDefinitionId` is only the handle the service minted for it, so
//! a write that would leave one destination holding two definitions of one metric is
//! refused — the single-definition update with the modelled `ConflictException` (HTTP 409)
//! naming the definition that already exists, and the batch create with that error as the
//! offending entry's own per-item error — while a write that leaves one definition per
//! metric is accepted. That is the identity
//! `BatchCreateRumMetricDefinitions` resolves a requested definition against, and
//! `UpdateRumMetricDefinition` is `@idempotent`
//! on the same identity: replacing a definition by the one it already is changes nothing
//! and is accepted.
//!
//! The table spans the rule rather than one request: its rows are pairs of definitions that
//! define DIFFERENT metrics, differing along each thing the identity is made of — the
//! metric's name, whether it carries a dimension at all, which dimension entry it carries,
//! the dimension's name, the unit label, the namespace, the value key and the event pattern
//! — over extended and custom metrics, and each row runs against both `MetricDestination`
//! values. Per row the suite checks both directions of the conflict, that the refusal
//! stored nothing, that the self-update is accepted, that a replacement defining a third
//! metric is accepted and read back, that the create still resolves the untouched
//! definition to its own id, and that no destination ever holds two definitions
//! that differ only in their id.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::metric_definitions::{
    custom, dimension_keys, event_pattern, extended, extended_with_dimensions, with,
    EXTENDED_DIMENSION_KEYS,
};
use crate::support::{
    all_metric_definitions, assert_metric_definition, conflicting_metric_definitions, create,
    created_metric_definitions, emulator, experiment_arn, message, metric_definition_id,
    put_metrics_destination, update_metric_definition, update_metric_definition_reply,
    METRIC_DEFINITION_MEMBERS,
};

/// The modelled error a write that would duplicate a stored definition is refused with, and
/// the status the model binds to it.
/// Source: `smithy_query(query="operation", operation="UpdateRumMetricDefinition")` —
/// `ConflictException`, `httpError` 409.
const CONFLICT_CODE: &str = "ConflictException";
const CONFLICT_STATUS: u16 = 409;

/// The literals this service's conflict message brackets the name of the resource that
/// already exists with — the wording the duplicate-name conflict of `CreateAppMonitor` is
/// recorded with (`not_found_and_conflict_bodies.rs`).
const CONFLICT_PREFIX: &str = "Resource ";
const CONFLICT_SUFFIX: &str = " already exists";

/// A namespace of the caller's own, which is what makes a definition a CUSTOM metric.
const CUSTOM_NAMESPACE: &str = "MyApp/Checkout";

/// A unit label no row carries, so a definition carrying it defines a THIRD metric whatever
/// the row's pair is. `UnitLabel` is an unconstrained string, so any value is admissible.
const THIRD_UNIT_LABEL: &str = "Milliseconds";

/// One extended-metrics destination, as the `(Destination, DestinationArn)` pair every
/// metric-definition call addresses one by.
struct Target {
    destination: &'static str,
    destination_arn: Option<String>,
}

/// Both values `MetricDestination` admits, each with the `DestinationArn` the model pairs
/// it with: an Evidently destination names the experiment that receives the metrics and a
/// CloudWatch one carries no ARN.
/// Source: `smithy_query(query="shape", shape="MetricDestination")`.
fn targets() -> Vec<Target> {
    vec![
        Target {
            destination: "CloudWatch",
            destination_arn: None,
        },
        Target {
            destination: "Evidently",
            destination_arn: Some(experiment_arn("identity")),
        },
    ]
}

/// A custom metric named `name`, carrying the members `members` adds beside the namespace
/// that makes it a custom metric.
fn custom_metric(name: &str, members: Value) -> Value {
    let mut definition = custom(name, CUSTOM_NAMESPACE);
    for (member, value) in members.as_object().expect("the members are a JSON object") {
        definition[member] = value.clone();
    }
    definition
}

/// Pairs of definitions that define DIFFERENT metrics, so one destination may hold both —
/// each row differing in exactly one of the things a definition is made of.
fn distinct_pairs() -> Vec<(&'static str, Value, Value)> {
    let browser = EXTENDED_DIMENSION_KEYS[1];
    let country = EXTENDED_DIMENSION_KEYS[4];
    vec![
        (
            "two extended metrics with different names",
            extended("PageViewCount"),
            extended("JsErrorCount"),
        ),
        (
            "one extended metric, with and without a dimension",
            extended("SessionCount"),
            extended_with_dimensions("SessionCount", &[browser]),
        ),
        (
            "one extended metric carrying two different dimension entries",
            extended_with_dimensions("Http4xxCount", &[browser]),
            extended_with_dimensions("Http4xxCount", &[country]),
        ),
        (
            "one extended metric under two unit labels",
            with(extended("JsErrorCount"), "UnitLabel", json!("Count")),
            with(extended("JsErrorCount"), "UnitLabel", json!("None")),
        ),
        (
            "one custom metric in two namespaces",
            custom("CartDepth", CUSTOM_NAMESPACE),
            custom("CartDepth", "MyApp/Cart"),
        ),
        (
            "one custom metric reading two different value keys",
            custom_metric(
                "CartTotal",
                json!({
                    "ValueKey": "event_details.total",
                    "EventPattern": event_pattern("com.example.cart", &[]),
                }),
            ),
            custom_metric(
                "CartTotal",
                json!({
                    "ValueKey": "event_details.count",
                    "EventPattern": event_pattern("com.example.cart", &[]),
                }),
            ),
        ),
        (
            "one custom metric matching two different patterns",
            custom_metric(
                "Conversions",
                json!({"EventPattern": event_pattern("com.example.checkout", &[])}),
            ),
            custom_metric(
                "Conversions",
                json!({"EventPattern": event_pattern("com.example.signup", &[])}),
            ),
        ),
        (
            "one custom metric mapping one field to two dimension names",
            custom_metric(
                "PageViews",
                json!({
                    "EventPattern": event_pattern("com.example.page", &[browser.0]),
                    "DimensionKeys": dimension_keys(&[(browser.0, "Browser")]),
                }),
            ),
            custom_metric(
                "PageViews",
                json!({
                    "EventPattern": event_pattern("com.example.page", &[browser.0]),
                    "DimensionKeys": dimension_keys(&[(browser.0, browser.1)]),
                }),
            ),
        ),
    ]
}

/// The members of a reported definition WITHOUT the service-minted id — what two stored
/// definitions are compared by, since the id is the handle and not the metric.
fn defined_metric(reported: &Value) -> Value {
    Value::Object(
        METRIC_DEFINITION_MEMBERS
            .iter()
            .map(|member| {
                (
                    (*member).to_string(),
                    reported.get(member).cloned().unwrap_or(Value::Null),
                )
            })
            .collect(),
    )
}

/// The invariant the identity rule keeps: no two definitions one destination holds define
/// the same metric, i.e. no two of them differ only in their `MetricDefinitionId`.
fn assert_no_two_define_the_same_metric(case: &str, held: &BTreeMap<String, Value>) {
    let defined: Vec<(&String, Value)> = held
        .iter()
        .map(|(id, reported)| (id, defined_metric(reported)))
        .collect();
    for (index, (left_id, left)) in defined.iter().enumerate() {
        for (right_id, right) in defined.iter().skip(index + 1) {
            assert_ne!(
                left, right,
                "case {case:?}: definitions {left_id:?} and {right_id:?} define the same \
                 metric: {left}"
            );
        }
    }
}

/// Assert the conflict `reply` is the modelled refusal naming `existing`: the status the
/// model binds to `ConflictException`, that error's code, the service's one conflict
/// wording around the id of the definition that already exists, and that id as
/// `resourceName`.
fn assert_conflict(case: &str, reply: &crate::support::Reply, existing: &str) {
    assert_eq!(
        reply.status, CONFLICT_STATUS,
        "case {case:?}: expected the update to answer {CONFLICT_STATUS}, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    assert_eq!(
        reply.error_code(case),
        CONFLICT_CODE,
        "case {case:?}: expected the refusal to be {CONFLICT_CODE}, got {:?}",
        String::from_utf8_lossy(&reply.body)
    );
    let body = reply.json(case);
    let expected = format!("{CONFLICT_PREFIX}{existing}{CONFLICT_SUFFIX}");
    assert_eq!(
        message(&body),
        Some(expected.as_str()),
        "case {case:?}: expected the message {expected:?}, got {body}"
    );
    assert_eq!(
        body["resourceName"].as_str(),
        Some(existing),
        "case {case:?}: expected resourceName to name the definition that already exists \
         ({existing:?}), got {body}"
    );
}

#[tokio::test]
async fn no_write_leaves_one_destination_holding_two_definitions_of_one_metric() {
    let (router, _engine) = emulator();

    for target in targets() {
        let destination = target.destination;
        let arn = target.destination_arn.as_deref();
        for (index, (row, first, second)) in distinct_pairs().into_iter().enumerate() {
            let case = format!("{destination}: {row}");
            let case = case.as_str();
            let monitor = format!("identity-{}-{index}", destination.to_lowercase());
            let monitor = monitor.as_str();
            create(
                &router,
                case,
                json!({"Name": monitor, "Domain": "example.com"}),
            )
            .await;
            put_metrics_destination(&router, case, monitor, destination, arn).await;

            // Two definitions of two metrics: the create stores both, under ids of its own.
            let created = created_metric_definitions(
                &router,
                case,
                monitor,
                destination,
                arn,
                &[first.clone(), second.clone()],
            )
            .await;
            assert_eq!(
                created.len(),
                2,
                "case {case:?}: expected the pair to be created as two definitions, got \
                 {created:?}"
            );
            let first_id = metric_definition_id(case, &created[0]);
            let second_id = metric_definition_id(case, &created[1]);

            // Updating either definition INTO the other is the condition
            // `ConflictException` names: the metric the request defines already has a
            // definition, under the other id.
            for (addressed, duplicated, existing) in [
                (&second_id, &first, &first_id),
                (&first_id, &second, &second_id),
            ] {
                let reply = update_metric_definition_reply(
                    &router, monitor, destination, arn, addressed, duplicated,
                )
                .await;
                assert_conflict(case, &reply, existing);
            }

            // And the refusals stored nothing: both definitions are still the ones the
            // create wrote, under the ids it minted.
            let held =
                all_metric_definitions(&router, case, monitor, destination, arn, None).await;
            assert_eq!(
                held.len(),
                2,
                "case {case:?}: expected the destination to still hold two definitions, got \
                 {held:?}"
            );
            assert_metric_definition(case, &held[&first_id], &first, &first_id);
            assert_metric_definition(case, &held[&second_id], &second, &second_id);
            assert_no_two_define_the_same_metric(case, &held);

            // Replacing a definition by the one it ALREADY is leaves one definition of that
            // metric, so it is accepted — the update is `@idempotent` on the same identity
            // the create is.
            for (addressed, unchanged) in [(&first_id, &first), (&second_id, &second)] {
                update_metric_definition(
                    &router, case, monitor, destination, arn, addressed, unchanged,
                )
                .await;
            }
            let held =
                all_metric_definitions(&router, case, monitor, destination, arn, None).await;
            assert_eq!(
                held.len(),
                2,
                "case {case:?}: a self-update changed how many definitions the destination \
                 holds: {held:?}"
            );
            assert_metric_definition(case, &held[&first_id], &first, &first_id);
            assert_metric_definition(case, &held[&second_id], &second, &second_id);

            // A replacement defining a THIRD metric is what the update is for: it is
            // accepted and read back under the id it was addressed by.
            let third = with(second.clone(), "UnitLabel", json!(THIRD_UNIT_LABEL));
            update_metric_definition(
                &router, case, monitor, destination, arn, &second_id, &third,
            )
            .await;
            let held =
                all_metric_definitions(&router, case, monitor, destination, arn, None).await;
            assert_eq!(
                held.len(),
                2,
                "case {case:?}: an accepted update changed how many definitions the \
                 destination holds: {held:?}"
            );
            assert_metric_definition(case, &held[&second_id], &third, &second_id);
            assert_metric_definition(case, &held[&first_id], &first, &first_id);
            assert_no_two_define_the_same_metric(case, &held);

            // The definition the writes left alone is still the one the create resolves for
            // its metric: the create finds it already defined and answers that entry's own
            // conflict under the id it already carries — so neither stored definition has
            // become unreachable through the create.
            let recreated = conflicting_metric_definitions(
                &router,
                case,
                monitor,
                destination,
                arn,
                std::slice::from_ref(&first),
            )
            .await;
            assert_eq!(
                recreated.len(),
                1,
                "case {case:?}: expected the repeated create to report one definition, got \
                 {recreated:?}"
            );
            assert_metric_definition(case, &recreated[0], &first, &first_id);
            let held =
                all_metric_definitions(&router, case, monitor, destination, arn, None).await;
            assert_eq!(
                held.len(),
                2,
                "case {case:?}: the repeated create stored a second copy of a definition it \
                 already held: {held:?}"
            );
        }
    }
}

#[tokio::test]
async fn one_metric_may_be_defined_once_per_destination() {
    let (router, _engine) = emulator();
    let case = "the same definition under two destinations";
    let monitor = "identity-per-destination";
    let experiment = experiment_arn("per-destination");
    create(
        &router,
        case,
        json!({"Name": monitor, "Domain": "example.com"}),
    )
    .await;
    put_metrics_destination(&router, case, monitor, "CloudWatch", None).await;
    put_metrics_destination(&router, case, monitor, "Evidently", Some(&experiment)).await;

    // One metric's definition under CloudWatch, another metric's under Evidently.
    let shared = extended("PageViewCount");
    let other = extended("JsErrorCount");
    let cloud_watch = created_metric_definitions(
        &router,
        case,
        monitor,
        "CloudWatch",
        None,
        std::slice::from_ref(&shared),
    )
    .await;
    let cloud_watch_id = metric_definition_id(case, &cloud_watch[0]);
    let evidently = created_metric_definitions(
        &router,
        case,
        monitor,
        "Evidently",
        Some(&experiment),
        std::slice::from_ref(&other),
    )
    .await;
    let evidently_id = metric_definition_id(case, &evidently[0]);

    // A definition belongs to the destination it was created under, so the one CloudWatch
    // holds is not in the way of the Evidently destination defining that same metric: the
    // update is accepted and both destinations then hold ONE definition of it.
    update_metric_definition(
        &router,
        case,
        monitor,
        "Evidently",
        Some(&experiment),
        &evidently_id,
        &shared,
    )
    .await;
    let held_by_evidently =
        all_metric_definitions(&router, case, monitor, "Evidently", Some(&experiment), None).await;
    assert_metric_definition(
        case,
        &held_by_evidently[&evidently_id],
        &shared,
        &evidently_id,
    );
    let held_by_cloud_watch =
        all_metric_definitions(&router, case, monitor, "CloudWatch", None, None).await;
    assert_metric_definition(
        case,
        &held_by_cloud_watch[&cloud_watch_id],
        &shared,
        &cloud_watch_id,
    );
    assert_eq!(
        (held_by_cloud_watch.len(), held_by_evidently.len()),
        (1, 1),
        "case {case:?}: expected one definition per destination, got {held_by_cloud_watch:?} \
         and {held_by_evidently:?}"
    );
}

#[tokio::test]
async fn an_id_that_names_no_definition_is_not_the_conflict_whatever_the_request_defines() {
    let (router, _engine) = emulator();
    let case = "a duplicate definition under an id that names nothing";
    let monitor = "identity-absent-id";
    create(
        &router,
        case,
        json!({"Name": monitor, "Domain": "example.com"}),
    )
    .await;
    put_metrics_destination(&router, case, monitor, "CloudWatch", None).await;

    let stored = extended("PageViewCount");
    let created = created_metric_definitions(
        &router,
        case,
        monitor,
        "CloudWatch",
        None,
        std::slice::from_ref(&stored),
    )
    .await;
    let stored_id = metric_definition_id(case, &created[0]);

    // The request defines a metric the destination already has a definition for, but the id
    // it addresses names no definition at all: there is nothing to replace, so the add
    // fails as the service's own `InternalServerException` (500, modelled `retryable`)
    // rather than as the conflict a duplicate under a STORED id would be.
    let reply = update_metric_definition_reply(
        &router,
        monitor,
        "CloudWatch",
        None,
        "no-such-metric-definition-id",
        &stored,
    )
    .await;
    assert_eq!(
        reply.status,
        500,
        "case {case:?}: expected 500 for an id that names no definition, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    assert_eq!(
        reply.error_code(case),
        "InternalServerException",
        "case {case:?}: expected the refusal to be InternalServerException, got {:?}",
        String::from_utf8_lossy(&reply.body)
    );

    // And nothing was written under the absent id.
    let held = all_metric_definitions(&router, case, monitor, "CloudWatch", None, None).await;
    assert_eq!(
        held.len(),
        1,
        "case {case:?}: a refused update changed what the destination holds: {held:?}"
    );
    assert_metric_definition(case, &held[&stored_id], &stored, &stored_id);
}
