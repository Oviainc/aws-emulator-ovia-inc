// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the rules a `MetricDefinitionRequest` is judged by, on both
//! operations that accept one: `BatchCreateRumMetricDefinitions`, where a definition the
//! service will not store fails on its own and the rest of the list still succeeds, and
//! `UpdateRumMetricDefinition`, where it fails the call.
//!
//! The rule under test is the one the model states for the shape: the `Namespace` decides
//! which KIND of metric a definition is, and the kind decides the rest. A definition whose
//! `Namespace` is omitted or `AWS/RUM` is an EXTENDED metric, so its `Name` has to be one
//! the model pins a combination for and its `ValueKey` and `EventPattern` have to be that
//! combination ("Only certain combinations of values for `Name`, `ValueKey`, and
//! `EventPattern` are valid"); every other definition is a CUSTOM metric, whose namespace
//! "can't be omitted and can't be `AWS/RUM`".
//!
//! The tables span the rule rather than one request: the acceptance table carries the
//! pinned combination of EVERY extended metric the model lists, a pattern that includes
//! the required object and more, both namespaces an extended metric may name, and custom
//! metrics from a bare one to one carrying every member; the refusal table varies which
//! part of the combination is wrong (a `ValueKey` where the model wants none, none where it
//! wants one, another's value, another metric's pattern, a pattern missing a nested
//! matcher, no pattern at all) and which namespace rule a custom metric breaks, including
//! names just outside the catalog. Every refused definition is then shown to have been
//! stored nowhere, read back through `BatchGetRumMetricDefinitions`.

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use crate::metric_definitions::{
    custom, dimension_keys, event_pattern, extended, extended_with_dimensions, with,
    EXTENDED_DIMENSION_KEYS, EXTENDED_METRICS,
};
use crate::support::{call, create, emulator, message, Reply};

/// The `ErrorCode` a per-definition error carries for a definition the service will not
/// store — the modelled error the single-definition update answers with.
const VALIDATION_CODE: &str = "ValidationException";

/// The namespace RUM publishes extended metrics to, the one value an extended metric's
/// `Namespace` may carry.
/// Source: `smithy_query(query="doc", shape="MetricDefinitionRequest")` — "The `Namespace`
/// parameter must be omitted or set to `AWS/RUM`".
const EXTENDED_NAMESPACE: &str = "AWS/RUM";

/// `definition` without `member`, so a row can state the ABSENCE of a member as its one
/// difference from a definition the model declares valid.
fn without(mut definition: Value, member: &str) -> Value {
    if let Some(members) = definition.as_object_mut() {
        members.remove(member);
    }
    definition
}

/// The catalog row the model pins for the extended metric `name`.
fn row(name: &str) -> &'static crate::metric_definitions::Extended {
    EXTENDED_METRICS
        .iter()
        .find(|entry| entry.name == name)
        .unwrap_or_else(|| panic!("case {name:?}: the model lists no extended metric by that name"))
}

/// One app monitor with a CloudWatch extended-metrics destination — the destination a
/// definition has to be created under.
async fn seeded_monitor(router: &MultiServiceRouter, case: &str, monitor: &str) {
    create(
        router,
        case,
        json!({"Name": monitor, "Domain": "example.com"}),
    )
    .await;
    let reply = call(
        router,
        "POST",
        &format!("/rummetrics/{monitor}/metricsdestination"),
        Some(json!({"Destination": "CloudWatch"})),
    )
    .await;
    assert_eq!(
        reply.status,
        200,
        "case {case:?}: seeding the destination must succeed, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
}

/// `BatchCreateRumMetricDefinitions` for the CloudWatch destination of `monitor`.
async fn batch_create(
    router: &MultiServiceRouter,
    monitor: &str,
    definitions: &[Value],
) -> Reply {
    call(
        router,
        "POST",
        &format!("/rummetrics/{monitor}/metrics"),
        Some(json!({
            "Destination": "CloudWatch",
            "MetricDefinitions": Value::Array(definitions.to_vec()),
        })),
    )
    .await
}

/// `UpdateRumMetricDefinition` replacing the definition `id` of `monitor` with
/// `definition`.
async fn update(
    router: &MultiServiceRouter,
    monitor: &str,
    id: &str,
    definition: &Value,
) -> Reply {
    call(
        router,
        "PATCH",
        &format!("/rummetrics/{monitor}/metrics"),
        Some(json!({
            "Destination": "CloudWatch",
            "MetricDefinitionId": id,
            "MetricDefinition": definition.clone(),
        })),
    )
    .await
}

/// Every definition the CloudWatch destination of `monitor` holds, read back through the
/// paginated `BatchGetRumMetricDefinitions` and following its tokens to the end.
async fn stored(router: &MultiServiceRouter, case: &str, monitor: &str) -> Vec<Value> {
    let mut held = Vec::new();
    let mut extra = String::new();
    loop {
        let reply = call(
            router,
            "GET",
            &format!("/rummetrics/{monitor}/metrics?destination=CloudWatch{extra}"),
            None,
        )
        .await;
        assert_eq!(
            reply.status,
            200,
            "case {case:?}: expected the read-back to answer 200, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        let page = reply.json(case);
        held.extend(
            page["MetricDefinitions"]
                .as_array()
                .cloned()
                .unwrap_or_default(),
        );
        match page["NextToken"].as_str() {
            Some(token) => extra = format!("&nextToken={token}"),
            None => return held,
        }
    }
}

/// Every member of a definition, as a stored definition reports them beside its id.
/// Source: `smithy_query(query="shape", shape="MetricDefinitionRequest,MetricDefinition")`.
const DEFINITION_MEMBERS: [&str; 6] = [
    "Name",
    "ValueKey",
    "UnitLabel",
    "DimensionKeys",
    "EventPattern",
    "Namespace",
];

/// Assert `reported` carries exactly the members `requested` asked for.
fn assert_definition(case: &str, reported: &Value, requested: &Value) {
    for member in DEFINITION_MEMBERS {
        let expected = requested.get(member).cloned().unwrap_or(Value::Null);
        assert_eq!(
            reported[member], expected,
            "case {case:?}: expected {member} to be {expected:?}, got {:?}",
            reported[member]
        );
    }
}

/// Definitions the model declares VALID, each with the reason it is one.
fn accepted_definitions() -> Vec<(String, Value)> {
    // Every extended metric, each with the combination its own catalog row pins.
    let mut rows: Vec<(String, Value)> = EXTENDED_METRICS
        .iter()
        .map(|entry| {
            (
                format!("the combination pinned for {}", entry.name),
                extended(entry.name),
            )
        })
        .collect();
    rows.extend([
        (
            // "The Namespace parameter must be omitted or set to AWS/RUM."
            "an extended metric naming the extended-metrics namespace".to_string(),
            with(
                extended(EXTENDED_METRICS[8].name),
                "Namespace",
                json!(EXTENDED_NAMESPACE),
            ),
        ),
        (
            // "In addition to what is displayed in the following list, the EventPattern
            // can also include information used by the DimensionKeys field."
            "an extended metric whose pattern names its dimensions too".to_string(),
            extended_with_dimensions(EXTENDED_METRICS[0].name, &EXTENDED_DIMENSION_KEYS[0..2]),
        ),
        (
            // The model's own example pattern for this metric, which carries a member
            // beyond the required object.
            "an extended metric whose pattern includes more than the required object"
                .to_string(),
            with(
                extended(EXTENDED_METRICS[8].name),
                "EventPattern",
                json!(
                    r#"{"event_type":["com.amazon.rum.js_error_event"],"metadata":{"browserName":["Chrome","Safari"]}}"#
                ),
            ),
        ),
        (
            "a custom metric carrying nothing but its name and namespace".to_string(),
            custom("CartCheckout", "MyApp/Checkout"),
        ),
        (
            "a custom metric carrying every member".to_string(),
            json!({
                "Name": "CheckoutLatency",
                "Namespace": "MyApp/Checkout",
                "ValueKey": "event_details.latency",
                "UnitLabel": "Milliseconds",
                "EventPattern": event_pattern("com.example.checkout", &["metadata.pageId"]),
                "DimensionKeys": dimension_keys(&EXTENDED_DIMENSION_KEYS[0..1]),
            }),
        ),
        (
            // "You cannot use any string that starts with AWS/ for your namespace": the
            // prefix is matched as written, so a namespace that merely resembles it is a
            // namespace of the caller's own.
            "a custom metric under a namespace resembling the reserved prefix".to_string(),
            custom("Resembling", "AWSCustom"),
        ),
        (
            // "For custom metrics, you can specify any name that you like" — a name the
            // extended catalog lists is a custom metric's too, once it names a namespace
            // of its own.
            "a custom metric named like an extended one, under its own namespace".to_string(),
            custom(EXTENDED_METRICS[8].name, "MyApp/Errors"),
        ),
    ]);
    rows
}

/// Definitions the model declares INVALID, each with the reason it is one.
fn refused_definitions() -> Vec<(String, Value)> {
    vec![
        (
            // "The namespace can't be omitted."
            "a custom metric with no namespace at all".to_string(),
            json!({"Name": "MyCustomMetric"}),
        ),
        (
            "a custom metric carrying every member but a namespace".to_string(),
            json!({
                "Name": "CartSize",
                "ValueKey": "event_details.items",
                "UnitLabel": "Count",
                "EventPattern": event_pattern("com.example.cart", &[]),
            }),
        ),
        (
            // A name is matched as written, so one differing only in case from a catalog
            // name is a custom metric and needs a namespace.
            "a name differing from an extended one only in case, with no namespace"
                .to_string(),
            json!({"Name": "jserrorcount"}),
        ),
        (
            "a name an extended one is a prefix of, with no namespace".to_string(),
            json!({"Name": format!("{}Extra", EXTENDED_METRICS[8].name)}),
        ),
        (
            // "The namespace … can't be AWS/RUM. You can use the AWS/RUM namespace only
            // for extended metrics."
            "a custom metric published to the extended-metrics namespace".to_string(),
            custom("MyCustomMetric", EXTENDED_NAMESPACE),
        ),
        (
            "a name differing from an extended one only in case, under AWS/RUM".to_string(),
            custom("jserrorcount", EXTENDED_NAMESPACE),
        ),
        (
            // "If Name is JsErrorCount, then ValueKey must be null."
            "an extended metric carrying a ValueKey the model wants null".to_string(),
            with(
                extended(EXTENDED_METRICS[8].name),
                "ValueKey",
                json!("event_details.duration"),
            ),
        ),
        (
            // "If Name is PerformanceNavigationDuration, then ValueKey must be
            // event_details.duration."
            "an extended metric with no ValueKey where the model requires one".to_string(),
            without(extended(EXTENDED_METRICS[0].name), "ValueKey"),
        ),
        (
            "an extended metric carrying another metric's ValueKey".to_string(),
            with(
                extended(EXTENDED_METRICS[6].name),
                "ValueKey",
                json!(row(EXTENDED_METRICS[0].name).value_key),
            ),
        ),
        (
            // The instance this rule was filed on: a catalog name with another metric's
            // combination.
            "an extended metric with another metric's ValueKey and pattern".to_string(),
            json!({
                "Name": EXTENDED_METRICS[8].name,
                "ValueKey": "event_details.duration",
                "EventPattern": row(EXTENDED_METRICS[0].name).event_pattern,
            }),
        ),
        (
            // "When you define extended metrics, the metric definition is not valid if
            // EventPattern is omitted."
            "an extended metric with no EventPattern at all".to_string(),
            without(extended(EXTENDED_METRICS[10].name), "EventPattern"),
        ),
        (
            "an extended metric carrying another metric's pattern".to_string(),
            with(
                extended(EXTENDED_METRICS[8].name),
                "EventPattern",
                json!(row(EXTENDED_METRICS[9].name).event_pattern),
            ),
        ),
        (
            // The pattern matches the right event type and still leaves out the nested
            // matcher this metric's row requires.
            "an extended metric whose pattern leaves out a nested matcher".to_string(),
            with(
                extended(EXTENDED_METRICS[2].name),
                "EventPattern",
                json!(row(EXTENDED_METRICS[0].name).event_pattern),
            ),
        ),
        (
            "an extended metric carrying the neighbouring threshold's pattern".to_string(),
            with(
                extended(EXTENDED_METRICS[3].name),
                "EventPattern",
                json!(row(EXTENDED_METRICS[4].name).event_pattern),
            ),
        ),
        (
            "an extended metric carrying the other status range's pattern".to_string(),
            with(
                extended(EXTENDED_METRICS[12].name),
                "EventPattern",
                json!(row(EXTENDED_METRICS[13].name).event_pattern),
            ),
        ),
    ]
}

#[tokio::test]
async fn every_definition_the_model_declares_valid_is_created_and_read_back() {
    let (router, _engine) = emulator();
    let monitor = "validation-accepted";
    seeded_monitor(&router, "accepted", monitor).await;

    let rows = accepted_definitions();
    let requested: Vec<Value> = rows.iter().map(|(_, definition)| definition.clone()).collect();
    let reply = batch_create(&router, monitor, &requested).await;
    assert_eq!(
        reply.status,
        200,
        "case {:?}: expected the batch create to answer 200, got {} {:?}",
        "accepted",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    let answered = reply.json("accepted");
    assert_eq!(
        answered["Errors"],
        json!([]),
        "case {:?}: expected every definition to be accepted, got {:?}",
        "accepted",
        answered["Errors"]
    );
    let created = answered["MetricDefinitions"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        created.len(),
        rows.len(),
        "case {:?}: expected {} created definitions, got {created:?}",
        "accepted",
        rows.len()
    );

    // Each is reported, and read back, as the definition that was asked for.
    let held = stored(&router, "accepted", monitor).await;
    assert_eq!(
        held.len(),
        rows.len(),
        "case {:?}: expected the destination to hold {} definitions, got {held:?}",
        "accepted",
        rows.len()
    );
    for ((case, requested), reported) in rows.iter().zip(created.iter()) {
        assert_definition(case, reported, requested);
        let id = reported["MetricDefinitionId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the definition carries no id: {reported}"));
        let read_back = held
            .iter()
            .find(|definition| definition["MetricDefinitionId"].as_str() == Some(id))
            .unwrap_or_else(|| panic!("case {case:?}: no definition {id:?} was stored"));
        assert_definition(case, read_back, requested);
    }

    // The same definitions are accepted by the update, which judges one the same way.
    let subject = created[0]["MetricDefinitionId"]
        .as_str()
        .expect("the first created definition carries an id")
        .to_string();
    for (case, definition) in &rows {
        let reply = update(&router, monitor, &subject, definition).await;
        assert_eq!(
            reply.status,
            200,
            "case {case:?}: expected the update to answer 200, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
    }
}

#[tokio::test]
async fn no_definition_the_model_declares_invalid_is_stored_by_either_operation() {
    let (router, _engine) = emulator();
    let monitor = "validation-refused";
    seeded_monitor(&router, "refused", monitor).await;

    // One valid definition beside the invalid ones, so the batch shows what the model asks
    // for: "those metric definitions fail and return errors, but all valid metric
    // definitions in the same operation still succeed."
    let valid = extended(EXTENDED_METRICS[8].name);
    let rows = refused_definitions();
    let mut requested = vec![valid.clone()];
    requested.extend(rows.iter().map(|(_, definition)| definition.clone()));

    let reply = batch_create(&router, monitor, &requested).await;
    assert_eq!(
        reply.status,
        200,
        "case {:?}: expected the batch create to answer 200, got {} {:?}",
        "refused",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    let answered = reply.json("refused");
    let created = answered["MetricDefinitions"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        created.len(),
        1,
        "case {:?}: expected only the valid definition to be created, got {created:?}",
        "refused"
    );
    assert_definition("refused", &created[0], &valid);

    // One error per refused definition, in the order the request named them, each carrying
    // the definition that caused it and a message of its own.
    let errors = answered["Errors"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        errors.len(),
        rows.len(),
        "case {:?}: expected {} per-definition errors, got {errors:?}",
        "refused",
        rows.len()
    );
    for ((case, definition), error) in rows.iter().zip(errors.iter()) {
        assert_eq!(
            &error["MetricDefinition"], definition,
            "case {case:?}: expected the error to carry the definition that caused it, got \
             {error}"
        );
        assert_eq!(
            error["ErrorCode"].as_str(),
            Some(VALIDATION_CODE),
            "case {case:?}: expected the error code {VALIDATION_CODE}, got {error}"
        );
        assert!(
            error["ErrorMessage"]
                .as_str()
                .is_some_and(|text| !text.is_empty()),
            "case {case:?}: the error carries no ErrorMessage: {error}"
        );
    }

    // And the single-definition update refuses each of them outright, with the modelled
    // error, leaving the definition it named as it was.
    let subject = created[0]["MetricDefinitionId"]
        .as_str()
        .expect("the created definition carries an id")
        .to_string();
    for (case, definition) in &rows {
        let reply = update(&router, monitor, &subject, definition).await;
        assert_eq!(
            reply.status,
            400,
            "case {case:?}: expected the update to answer 400, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        assert_eq!(
            reply.error_code(case),
            VALIDATION_CODE,
            "case {case:?}: expected a {VALIDATION_CODE}, got {:?}",
            String::from_utf8_lossy(&reply.body)
        );
        let body = reply.json(case);
        assert!(
            message(&body).is_some_and(|text| !text.is_empty()),
            "case {case:?}: the refusal carries no message: {body}"
        );
    }

    // Nothing either operation refused was stored: the destination holds the one valid
    // definition, exactly as it was created.
    let held = stored(&router, "refused", monitor).await;
    assert_eq!(
        held.len(),
        1,
        "case {:?}: expected the destination to hold only the valid definition, got {held:?}",
        "refused"
    );
    assert_definition("refused", &held[0], &valid);
}
