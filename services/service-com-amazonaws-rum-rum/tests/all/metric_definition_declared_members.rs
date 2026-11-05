// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the form a metric definition is STORED in, and therefore for
//! the identity `BatchCreateRumMetricDefinitions` resolves a requested definition against.
//!
//! The rule under test: an optional member of a `MetricDefinitionRequest` whose value
//! declares nothing — an `EventPattern` that is empty or carries only whitespace, a
//! `DimensionKeys` map with no entry — is the same input as a member left out, and a
//! declared `EventPattern` is the pattern that is left once the whitespace around it is
//! gone. The rules that judge a definition already read those members that way, so the
//! stored canonical form reads them the same way: otherwise two spellings of "nothing"
//! that the service admits as ONE definition would be stored as two, each minting another
//! `MetricDefinitionId`, taking another slot of the destination's capacity and needing its
//! own delete.
//!
//! `EventPattern` and `DimensionKeys` are the two members such a value can reach a handler
//! through: they are modelled `@length(min: 0, …)`, while `ValueKey`, `UnitLabel` and
//! `Namespace` are each `@length(min: 1, …)`, so a value that declares nothing is refused
//! at the boundary for those.
//! Source: `smithy_query(query="shape", shape="MetricDefinitionRequest")`.
//!
//! The rows span what the rule generalises over: which member declares nothing, which
//! spelling of nothing it uses (the empty string, one space, several spaces and tabs, the
//! empty map), whether a DECLARED value carries whitespace around it, whether the metric
//! is an extended or a custom one, which `MetricDestination` it lives under, and which of
//! the two spellings reaches the service first. Every row drives the real wire path — a
//! signed restJson1 request at a `MultiServiceRouter` — and reads the stored definitions
//! back through the paginated `BatchGetRumMetricDefinitions`, never out of the store.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use crate::metric_definitions::{
    custom, dimension_keys, event_pattern, extended, with, EXTENDED_DIMENSION_KEYS,
    EXTENDED_METRICS,
};
use crate::support::{
    all_metric_definitions, assert_metric_definition, conflicting_metric_definitions, create,
    created_metric_definitions, emulator, experiment_arn, metric_definition_id,
    metric_definition_without, put_metrics_destination, update_metric_definition,
};

/// Both values `MetricDestination` admits, as the `(Destination, experiment slug)` pair a
/// row names its destination by — an `Evidently` destination is identified by an experiment
/// ARN and a `CloudWatch` one carries none.
/// Source: `smithy_query(query="shape", shape="MetricDestination")`.
const CLOUD_WATCH: &str = "CloudWatch";
const EVIDENTLY: &str = "Evidently";

/// One row: two spellings of ONE definition, the canonical one being the form the service
/// has to store and report whichever spelling it was sent.
struct Row {
    case: &'static str,
    /// The spelling whose member declares nothing, or pads a declared value.
    spelled: Value,
    /// The same definition with that member left out, or with the padding gone.
    canonical: Value,
    /// The `MetricDestination` this row's definitions live under.
    destination: &'static str,
}

/// `definition` with the string value of `member` wrapped in whitespace, which is not part
/// of the value it declares.
fn padded(definition: &Value, member: &str) -> Value {
    let declared = definition[member]
        .as_str()
        .unwrap_or_else(|| panic!("{member} carries a string in {definition}"));
    with(
        definition.clone(),
        member,
        json!(format!("  {declared}\t\n")),
    )
}

/// The experiment ARN an `Evidently` destination of `monitor` names, and `None` for a
/// `CloudWatch` one, which carries no ARN.
fn destination_arn(destination: &str, monitor: &str) -> Option<String> {
    (destination == EVIDENTLY).then(|| experiment_arn(monitor))
}

/// One app monitor with one extended-metrics destination, ready to hold definitions.
async fn destination_holding_definitions(
    router: &MultiServiceRouter,
    case: &str,
    monitor: &str,
    destination: &str,
    arn: Option<&str>,
) {
    create(
        router,
        case,
        json!({"Name": monitor, "Domain": "example.com"}),
    )
    .await;
    put_metrics_destination(router, case, monitor, destination, arn).await;
}

/// Every definition the destination holds, keyed by the id it is stored under.
async fn stored(
    router: &MultiServiceRouter,
    case: &str,
    monitor: &str,
    destination: &str,
    arn: Option<&str>,
) -> BTreeMap<String, Value> {
    all_metric_definitions(router, case, monitor, destination, arn, None).await
}

/// Two spellings of one definition, where one of them declares nothing through a member the
/// other leaves out — or pads a value the other carries bare.
fn rows() -> Vec<Row> {
    // The pattern of a custom metric that filters on an event type and names the field one
    // dimension is read from, so a row can pad a pattern that really does declare
    // something.
    let naming_dimension = event_pattern("com.example.checkout", &[EXTENDED_DIMENSION_KEYS[0].0]);
    let dimensioned = with(
        with(
            custom("Checkouts", "MyApp/Funnel"),
            "EventPattern",
            json!(naming_dimension),
        ),
        "DimensionKeys",
        dimension_keys(&EXTENDED_DIMENSION_KEYS[..1]),
    );
    let extended_metric = extended(EXTENDED_METRICS[0].name);
    vec![
        Row {
            case: "an EventPattern that is the empty string",
            spelled: with(
                custom("Signups", "MyApp/Funnel"),
                "EventPattern",
                json!(""),
            ),
            canonical: custom("Signups", "MyApp/Funnel"),
            destination: CLOUD_WATCH,
        },
        Row {
            case: "an EventPattern that is one space",
            spelled: with(
                custom("Signups", "MyApp/Funnel"),
                "EventPattern",
                json!(" "),
            ),
            canonical: custom("Signups", "MyApp/Funnel"),
            destination: EVIDENTLY,
        },
        Row {
            case: "an EventPattern of spaces, tabs and newlines",
            spelled: with(
                custom("Abandonments", "MyApp/Checkout"),
                "EventPattern",
                json!("  \t\n "),
            ),
            canonical: custom("Abandonments", "MyApp/Checkout"),
            destination: CLOUD_WATCH,
        },
        Row {
            case: "a DimensionKeys map with no entry",
            spelled: with(
                custom("Conversions", "MyApp/Signup"),
                "DimensionKeys",
                json!({}),
            ),
            canonical: custom("Conversions", "MyApp/Signup"),
            destination: EVIDENTLY,
        },
        Row {
            case: "both members declaring nothing at once",
            spelled: with(
                with(
                    custom("Logins", "MyApp/Account"),
                    "EventPattern",
                    json!("   "),
                ),
                "DimensionKeys",
                json!({}),
            ),
            canonical: custom("Logins", "MyApp/Account"),
            destination: CLOUD_WATCH,
        },
        Row {
            case: "an extended metric's EventPattern padded with whitespace",
            spelled: padded(&extended_metric, "EventPattern"),
            canonical: extended_metric.clone(),
            destination: CLOUD_WATCH,
        },
        Row {
            case: "a custom metric's padded EventPattern beside the dimension it names",
            spelled: padded(&dimensioned, "EventPattern"),
            canonical: dimensioned.clone(),
            destination: EVIDENTLY,
        },
        Row {
            case: "an EventPattern declaring nothing where a dimensioned one was dropped",
            spelled: with(
                metric_definition_without(&dimensioned, "DimensionKeys"),
                "EventPattern",
                json!("\t"),
            ),
            canonical: metric_definition_without(
                &metric_definition_without(&dimensioned, "DimensionKeys"),
                "EventPattern",
            ),
            destination: CLOUD_WATCH,
        },
    ]
}

#[tokio::test]
async fn a_definition_member_declaring_nothing_is_stored_as_an_absent_one() {
    let (router, _engine) = emulator();

    for (index, row) in rows().into_iter().enumerate() {
        let case = row.case;
        // Both orders: the spelling that declares nothing can be the one that CREATES the
        // definition or the one that re-sends it, and either way the pair is one definition.
        let orders: [(&str, &Value, &Value); 2] = [
            ("nothing-declared spelling first", &row.spelled, &row.canonical),
            ("canonical spelling first", &row.canonical, &row.spelled),
        ];
        for (which, (order, first, second)) in orders.into_iter().enumerate() {
            let case = format!("{case} ({order})");
            let case = case.as_str();
            let monitor = format!("declared-{index}-{which}");
            let arn = destination_arn(row.destination, &monitor);
            destination_holding_definitions(
                &router,
                case,
                &monitor,
                row.destination,
                arn.as_deref(),
            )
            .await;

            // Whichever spelling creates the definition, it is stored and reported in the
            // canonical form.
            let reported = created_metric_definitions(
                &router,
                case,
                &monitor,
                row.destination,
                arn.as_deref(),
                std::slice::from_ref(first),
            )
            .await;
            assert_eq!(
                reported.len(),
                1,
                "case {case:?}: expected one reported definition, got {reported:?}"
            );
            let id = metric_definition_id(case, &reported[0]);
            assert_metric_definition(case, &reported[0], &row.canonical, &id);
            let held = stored(&router, case, &monitor, row.destination, arn.as_deref()).await;
            assert_eq!(
                held.len(),
                1,
                "case {case:?}: expected the destination to hold one definition, got {held:?}"
            );
            assert_metric_definition(
                case,
                held.get(&id)
                    .unwrap_or_else(|| panic!("case {case:?}: {id:?} is not stored: {held:?}")),
                &row.canonical,
                &id,
            );

            // The OTHER spelling names that same definition, which the destination already
            // holds: the call reports it as that entry's own conflict, the stored
            // definition keeps the id the first call minted and nothing is stored a second
            // time.
            let again = conflicting_metric_definitions(
                &router,
                case,
                &monitor,
                row.destination,
                arn.as_deref(),
                std::slice::from_ref(second),
            )
            .await;
            assert_eq!(
                again.len(),
                1,
                "case {case:?}: expected one reported definition, got {again:?}"
            );
            assert_metric_definition(case, &again[0], &row.canonical, &id);
            let after = stored(&router, case, &monitor, row.destination, arn.as_deref()).await;
            assert_eq!(
                after, held,
                "case {case:?}: the second spelling changed the stored definitions, got \
                 {after:?}"
            );

            // Both spellings in ONE batch are one definition too: each entry names the
            // definition the destination already holds, so each is answered as its own
            // conflict.
            let batched = conflicting_metric_definitions(
                &router,
                case,
                &monitor,
                row.destination,
                arn.as_deref(),
                &[row.spelled.clone(), row.canonical.clone()],
            )
            .await;
            assert_eq!(
                batched.len(),
                2,
                "case {case:?}: expected one report per requested definition, got {batched:?}"
            );
            for entry in &batched {
                assert_metric_definition(case, entry, &row.canonical, &id);
            }
            let after_batch =
                stored(&router, case, &monitor, row.destination, arn.as_deref()).await;
            assert_eq!(
                after_batch, held,
                "case {case:?}: a batch of both spellings stored a second definition, got \
                 {after_batch:?}"
            );

            // A member that DOES declare something is another definition, so the identity
            // is still the definition itself rather than everything being folded together.
            let differing = with(row.canonical.clone(), "UnitLabel", json!("Count"));
            let other = created_metric_definitions(
                &router,
                case,
                &monitor,
                row.destination,
                arn.as_deref(),
                std::slice::from_ref(&differing),
            )
            .await;
            let other_id = metric_definition_id(case, &other[0]);
            assert_ne!(
                other_id, id,
                "case {case:?}: expected a definition differing in a declared member to get \
                 an id of its own, got {other_id:?}"
            );
            assert_metric_definition(case, &other[0], &differing, &other_id);
            let finally = stored(&router, case, &monitor, row.destination, arn.as_deref()).await;
            assert_eq!(
                finally.len(),
                2,
                "case {case:?}: expected the differing definition beside the first one, got \
                 {finally:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_update_stores_a_member_declaring_nothing_as_an_absent_one() {
    let (router, _engine) = emulator();

    // The definition the update replaces: a custom metric whose pattern names the field its
    // one dimension is read from, so the replacement has both members to drop.
    let dimensioned = with(
        with(
            custom("Checkouts", "MyApp/Funnel"),
            "EventPattern",
            json!(event_pattern(
                "com.example.checkout",
                &[EXTENDED_DIMENSION_KEYS[0].0]
            )),
        ),
        "DimensionKeys",
        dimension_keys(&EXTENDED_DIMENSION_KEYS[..1]),
    );

    // One row per replacement: what the new definition spells, and the definition the
    // service has to store for it.
    let rows: [(&str, Value, Value); 3] = [
        (
            "a replacement whose EventPattern is the empty string",
            with(custom("Checkouts", "MyApp/Funnel"), "EventPattern", json!("")),
            custom("Checkouts", "MyApp/Funnel"),
        ),
        (
            "a replacement whose members both declare nothing",
            with(
                with(
                    custom("Checkouts", "MyApp/Funnel"),
                    "EventPattern",
                    json!(" \t "),
                ),
                "DimensionKeys",
                json!({}),
            ),
            custom("Checkouts", "MyApp/Funnel"),
        ),
        (
            "a replacement whose declared EventPattern is padded",
            padded(&dimensioned, "EventPattern"),
            dimensioned.clone(),
        ),
    ];

    for (index, (case, replacement, expected)) in rows.into_iter().enumerate() {
        let monitor = format!("declared-update-{index}");
        destination_holding_definitions(&router, case, &monitor, CLOUD_WATCH, None).await;
        let reported = created_metric_definitions(
            &router,
            case,
            &monitor,
            CLOUD_WATCH,
            None,
            std::slice::from_ref(&dimensioned),
        )
        .await;
        let id = metric_definition_id(case, &reported[0]);

        update_metric_definition(
            &router,
            case,
            &monitor,
            CLOUD_WATCH,
            None,
            &id,
            &replacement,
        )
        .await;
        let held = stored(&router, case, &monitor, CLOUD_WATCH, None).await;
        assert_eq!(
            held.len(),
            1,
            "case {case:?}: expected the update to replace the one definition, got {held:?}"
        );
        assert_metric_definition(
            case,
            held.get(&id)
                .unwrap_or_else(|| panic!("case {case:?}: {id:?} is gone: {held:?}")),
            &expected,
            &id,
        );

        // And the definition the update stored is the one a create of the canonical
        // spelling names: the create finds that definition already there, so it answers the
        // conflict for it rather than storing a second copy — which is the evidence that
        // the update wrote the same canonical form a create writes.
        let again = conflicting_metric_definitions(
            &router,
            case,
            &monitor,
            CLOUD_WATCH,
            None,
            std::slice::from_ref(&expected),
        )
        .await;
        assert_metric_definition(case, &again[0], &expected, &id);
        let after = stored(&router, case, &monitor, CLOUD_WATCH, None).await;
        assert_eq!(
            after, held,
            "case {case:?}: the create stored a second copy of the updated definition, got \
             {after:?}"
        );
    }
}
