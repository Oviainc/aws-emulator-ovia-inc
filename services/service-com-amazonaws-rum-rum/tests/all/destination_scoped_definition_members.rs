// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the metric-definition members the model scopes to ONE metrics
//! destination — `DimensionKeys` and `UnitLabel` — across every destination a definition can
//! be created under.
//!
//! The rule under test: a member the model documents as taking effect only under one
//! destination is not a constraint on the REQUEST, so which `MetricDestination` a call names
//! never decides whether a definition carrying that member is stored — the definition is
//! accepted, persisted and reported back exactly as it was sent under every destination, and
//! the rules that DO bound a definition are applied identically under each of them.
//!
//! `MetricDefinition$DimensionKeys` — the member a read reports the stored map back through
//! — states that scoping as a consequence rather than a prohibition: "The value of this
//! field is used only if the metric destination is `CloudWatch`. If the metric destination is
//! `Evidently`, the value of `DimensionKeys` is ignored", which is a stored definition
//! carrying the map under either destination; `MetricDefinition$UnitLabel` is worded the same
//! way ("Use this field only if you are sending this metric to CloudWatch"), and
//! `BatchCreateRumMetricDefinitions` itself has extended metrics — the metrics those
//! additional dimensions belong to — going "to both Evidently and CloudWatch". The
//! `Destination` pairings this service refuses are the ones the model prohibits outright
//! instead ("If `Destination` is `CloudWatch`, do not use this parameter", for
//! `DestinationArn` and `IamRoleArn`), which `shared_destination_arn_rule.rs` and
//! `metrics_destination_pass_role.rs` pin.
//! Source: `smithy_query(query="shape", shape="MetricDefinition,MetricDefinitionRequest")`
//! and `smithy_query(query="operation", operation="BatchCreateRumMetricDefinitions")`.
//!
//! The tables span the rule rather than one request: the targets walk both
//! `MetricDestination` values and two distinct Evidently experiments; the accepted rows walk
//! every dimension entry the model lists, one entry and every entry at once, a unit label
//! with and without dimensions, extended metrics whose entries the model pins and custom
//! metrics whose dimension names are their own, and a definition carrying neither scoped
//! member; and the refused rows walk the destination-independent reasons a dimension map is
//! rejected, each run against every target on both operations that accept a definition.

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use crate::metric_definitions::{
    custom, dimension_keys, event_pattern, extended, extended_with_dimensions, with,
    EXTENDED_DIMENSION_KEYS, EXTENDED_METRICS,
};
use crate::support::{
    all_metric_definitions, assert_metric_definition, batch_create_metric_definitions, create,
    created_metric_definitions, emulator, experiment_arn, message, metric_definition_id,
    put_metrics_destination, reported_metric_definitions, sole_metric_definition,
    update_metric_definition, update_metric_definition_reply,
};

/// Both values `MetricDestination` admits.
/// Source: `smithy_query(query="shape", shape="MetricDestination")`.
const METRIC_DESTINATIONS: [&str; 2] = ["CloudWatch", "Evidently"];

/// The error a definition the service will not store is reported with: the `ErrorCode` of the
/// batch create's per-definition error, and the modelled error of the update.
const VALIDATION_CODE: &str = "ValidationException";

/// The event type the custom metrics below match.
const CUSTOM_EVENT_TYPE: &str = "com.example.probe";

/// A namespace of the caller's own, which is what makes a definition a CUSTOM metric.
const CUSTOM_NAMESPACE: &str = "MyApp/Scoped";

/// One extended-metrics destination a definition can be created under: the
/// `(Destination, DestinationArn)` pair that identifies it — an `Evidently` destination names
/// the experiment that receives the metrics, a `CloudWatch` one carries no ARN.
struct Target {
    case: &'static str,
    destination: &'static str,
    destination_arn: Option<String>,
}

impl Target {
    fn arn(&self) -> Option<&str> {
        self.destination_arn.as_deref()
    }
}

/// Every destination kind a definition can live under, with a second Evidently experiment so
/// the rule is checked on more than one instance of the destination that ignores the
/// dimensions.
fn targets() -> Vec<Target> {
    vec![
        Target {
            case: "the CloudWatch destination",
            destination: METRIC_DESTINATIONS[0],
            destination_arn: None,
        },
        Target {
            case: "an Evidently experiment",
            destination: METRIC_DESTINATIONS[1],
            destination_arn: Some(experiment_arn("alpha")),
        },
        Target {
            case: "another Evidently experiment",
            destination: METRIC_DESTINATIONS[1],
            destination_arn: Some(experiment_arn("beta")),
        },
    ]
}

/// A custom metric carrying `entries` as its `DimensionKeys`, with a pattern naming each
/// entry's field path — the one rule the model does state for the map, whichever destination
/// the definition is sent to.
fn custom_dimensioned(name: &str, entries: &[(&str, &str)]) -> Value {
    let paths: Vec<&str> = entries.iter().map(|(field, _)| *field).collect();
    json!({
        "Name": name,
        "Namespace": CUSTOM_NAMESPACE,
        "EventPattern": event_pattern(CUSTOM_EVENT_TYPE, &paths),
        "DimensionKeys": dimension_keys(entries),
    })
}

/// One app monitor holding one extended-metrics destination, ready to hold definitions.
async fn monitor_holding(router: &MultiServiceRouter, case: &str, monitor: &str, target: &Target) {
    create(
        router,
        case,
        json!({"Name": monitor, "Domain": "example.com"}),
    )
    .await;
    put_metrics_destination(router, case, monitor, target.destination, target.arn()).await;
}

/// Definitions carrying a destination-scoped member, each with the reason it is a row of its
/// own. Every row is a definition of a DIFFERENT metric, so one batch can carry them all.
fn scoped_definitions() -> Vec<(String, Value)> {
    // One extended metric per entry the model lists, each carrying that one entry: the rule is
    // about the map, so every listed entry is a row.
    let mut rows: Vec<(String, Value)> = EXTENDED_DIMENSION_KEYS
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            (
                format!("the listed entry {:?}: {:?}", entry.0, entry.1),
                extended_with_dimensions(EXTENDED_METRICS[index].name, &[*entry]),
            )
        })
        .collect();
    rows.extend([
        (
            "every listed entry at once, beside a unit label".to_string(),
            with(
                extended_with_dimensions(EXTENDED_METRICS[8].name, &EXTENDED_DIMENSION_KEYS),
                "UnitLabel",
                json!("Count"),
            ),
        ),
        (
            // `UnitLabel` is scoped to CloudWatch the same way, so a row carries it alone.
            "a unit label and no dimension at all".to_string(),
            with(
                extended(EXTENDED_METRICS[9].name),
                "UnitLabel",
                json!("Milliseconds"),
            ),
        ),
        (
            // A custom metric computes its own dimensions, so its names are its own.
            "a custom metric's two dimensions of its own, beside a unit label".to_string(),
            with(
                custom_dimensioned(
                    "CheckoutStarts",
                    &[
                        ("metadata.browserName", "Browser"),
                        ("metadata.countryCode", "Country"),
                    ],
                ),
                "UnitLabel",
                json!("Count"),
            ),
        ),
        (
            "a custom metric keyed on an event-details field".to_string(),
            custom_dimensioned("Downloads", &[("event_details.fileType", "Kind")]),
        ),
        (
            // The control: the rule is about the members a definition HAS.
            "a definition carrying neither scoped member".to_string(),
            extended(EXTENDED_METRICS[10].name),
        ),
    ]);
    rows
}

/// Definitions the model declares invalid for a reason that is not about the destination,
/// each with a string the refusal has to name.
fn refused_definitions() -> Vec<(String, Value, String)> {
    vec![
        (
            // "For extended metrics, valid values for the entries in this field are the
            // following": a field path the model does not list.
            "an extended metric's dimension the model does not list".to_string(),
            extended_with_dimensions(EXTENDED_METRICS[2].name, &[("metadata.title", "PageTitle")]),
            "metadata.title".to_string(),
        ),
        (
            "a listed path mapped to another entry's dimension name".to_string(),
            extended_with_dimensions(
                EXTENDED_METRICS[3].name,
                &[("metadata.browserName", "PageId")],
            ),
            "BrowserName".to_string(),
        ),
        (
            // "all dimensions listed in this field must also be included in EventPattern":
            // the catalog pattern alone names no dimension.
            "an extended metric whose EventPattern omits its dimension".to_string(),
            with(
                extended_with_dimensions(EXTENDED_METRICS[4].name, &[EXTENDED_DIMENSION_KEYS[1]]),
                "EventPattern",
                json!(EXTENDED_METRICS[4].event_pattern),
            ),
            "metadata.browserName".to_string(),
        ),
        (
            "a custom metric whose EventPattern omits its dimension".to_string(),
            with(
                custom_dimensioned("Signups", &[("metadata.browserName", "Browser")]),
                "EventPattern",
                json!(event_pattern(CUSTOM_EVENT_TYPE, &[])),
            ),
            "metadata.browserName".to_string(),
        ),
        (
            // "all first-level keys in these fields must be one of the keys in the list": a
            // dimension read from a field no RUM event carries.
            "a custom metric's dimension keyed on no RUM event field".to_string(),
            with(
                custom("Probes", CUSTOM_NAMESPACE),
                "DimensionKeys",
                dimension_keys(&[("not_a_rum_field.x", "Whatever")]),
            ),
            "not_a_rum_field".to_string(),
        ),
    ]
}

#[tokio::test]
async fn destination_scoped_members_are_created_and_read_back_under_every_destination() {
    let (router, _engine) = emulator();

    let rows = scoped_definitions();
    let requested: Vec<Value> = rows
        .iter()
        .map(|(_, definition)| definition.clone())
        .collect();

    for (index, target) in targets().into_iter().enumerate() {
        let monitor = format!("scoped-created-{index}");
        monitor_holding(&router, target.case, &monitor, &target).await;

        let created = created_metric_definitions(
            &router,
            target.case,
            &monitor,
            target.destination,
            target.arn(),
            &requested,
        )
        .await;
        assert_eq!(
            created.len(),
            requested.len(),
            "case {:?}: expected {} created definitions, got {created:?}",
            target.case,
            requested.len()
        );
        let held = all_metric_definitions(
            &router,
            target.case,
            &monitor,
            target.destination,
            target.arn(),
            None,
        )
        .await;
        assert_eq!(
            held.len(),
            requested.len(),
            "case {:?}: expected the destination to hold {} definitions, got {held:?}",
            target.case,
            requested.len()
        );

        // Each definition is reported, and read back, carrying the scoped members it was sent
        // with — whichever destination it was sent to.
        for ((row, wanted), reported) in rows.iter().zip(created.iter()) {
            let case = format!("{row} under {}", target.case);
            let case = case.as_str();
            let id = metric_definition_id(case, reported);
            assert_metric_definition(case, reported, wanted, &id);
            let read_back = held.get(&id).unwrap_or_else(|| {
                panic!("case {case:?}: no definition {id:?} was stored: {held:?}")
            });
            assert_metric_definition(case, read_back, wanted, &id);
        }
    }
}

#[tokio::test]
async fn an_update_stores_destination_scoped_members_under_every_destination() {
    let (router, _engine) = emulator();

    let rows = scoped_definitions();

    for (index, target) in targets().into_iter().enumerate() {
        let monitor = format!("scoped-updated-{index}");
        monitor_holding(&router, target.case, &monitor, &target).await;
        // The subject each row replaces carries NEITHER scoped member, so a row that stores
        // one is visible in the read-back. It is the destination's only definition, so no row
        // can be refused as a copy of a sibling.
        let subject = sole_metric_definition(
            &router,
            target.case,
            &monitor,
            target.destination,
            target.arn(),
            &custom("Seeded", CUSTOM_NAMESPACE),
        )
        .await;

        for (row, definition) in &rows {
            let case = format!("{row} replacing a definition under {}", target.case);
            let case = case.as_str();
            update_metric_definition(
                &router,
                case,
                &monitor,
                target.destination,
                target.arn(),
                &subject,
                definition,
            )
            .await;
            let held = all_metric_definitions(
                &router,
                case,
                &monitor,
                target.destination,
                target.arn(),
                None,
            )
            .await;
            assert_eq!(
                held.len(),
                1,
                "case {case:?}: expected the update to replace the one definition, got {held:?}"
            );
            let read_back = held
                .get(&subject)
                .unwrap_or_else(|| panic!("case {case:?}: {subject:?} is gone: {held:?}"));
            assert_metric_definition(case, read_back, definition, &subject);
        }
    }
}

#[tokio::test]
async fn the_dimension_rules_are_applied_under_every_destination() {
    let (router, _engine) = emulator();

    let rows = refused_definitions();
    let requested: Vec<Value> = rows
        .iter()
        .map(|(_, definition, _)| definition.clone())
        .collect();

    for (index, target) in targets().into_iter().enumerate() {
        let monitor = format!("scoped-refused-{index}");
        monitor_holding(&router, target.case, &monitor, &target).await;
        // One definition the model declares valid, so the destination has something to show
        // unchanged — and something for the refused updates to name.
        let valid = custom_dimensioned("Kept", &[EXTENDED_DIMENSION_KEYS[0]]);
        let subject = sole_metric_definition(
            &router,
            target.case,
            &monitor,
            target.destination,
            target.arn(),
            &valid,
        )
        .await;

        // The batch create refuses each of them per definition, whichever destination it is
        // sent to.
        let reply = batch_create_metric_definitions(
            &router,
            &monitor,
            target.destination,
            target.arn(),
            &requested,
        )
        .await;
        assert_eq!(
            reply.status,
            200,
            "case {:?}: expected the batch create to answer 200, got {} {:?}",
            target.case,
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        let answered = reply.json(target.case);
        assert_eq!(
            reported_metric_definitions(&answered),
            Vec::<Value>::new(),
            "case {:?}: expected no definition to be created, got {answered}",
            target.case
        );
        let errors = answered["Errors"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            errors.len(),
            rows.len(),
            "case {:?}: expected {} per-definition errors, got {errors:?}",
            target.case,
            rows.len()
        );
        for ((row, definition, named), error) in rows.iter().zip(errors.iter()) {
            let case = format!("{row} under {}", target.case);
            let case = case.as_str();
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
                    .is_some_and(|text| text.contains(named)),
                "case {case:?}: expected the error message to name {named:?}, got {error}"
            );
        }

        // And the single-definition update refuses each of them outright, with the modelled
        // error, under that same destination.
        for (row, definition, named) in &rows {
            let case = format!("{row} as an update under {}", target.case);
            let case = case.as_str();
            let reply = update_metric_definition_reply(
                &router,
                &monitor,
                target.destination,
                target.arn(),
                &subject,
                definition,
            )
            .await;
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
                message(&body).is_some_and(|text| text.contains(named)),
                "case {case:?}: expected the refusal to name {named:?}, got {body}"
            );
        }

        // Nothing either operation refused reached the store: the destination still holds the
        // one valid definition, exactly as it was created.
        let held = all_metric_definitions(
            &router,
            target.case,
            &monitor,
            target.destination,
            target.arn(),
            None,
        )
        .await;
        assert_eq!(
            held.len(),
            1,
            "case {:?}: expected the destination to hold only the valid definition, got {held:?}",
            target.case
        );
        assert_metric_definition(
            target.case,
            held.get(&subject)
                .unwrap_or_else(|| panic!("case {:?}: {subject:?} is gone: {held:?}", target.case)),
            &valid,
            &subject,
        );
    }
}
