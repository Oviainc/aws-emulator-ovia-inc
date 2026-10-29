// SPDX-License-Identifier: CC0-1.0

//! The wire form of an ExperimentDefinition document: the keys this service mints for a
//! definition's treatments, and the form the definition's write instants take on the wire.
//!
//! Two rules are under test, each one level more general than the recorded request that
//! exposed it:
//!
//! 1. A treatment's `Key` is the SERVICE's to mint — `TreatmentInput` carries no `Key` and
//!    `Treatment` does (`smithy_query(query="shape", shape="TreatmentInput")` /
//!    `shape="Treatment"`) — and AppConfig composes it from the treatment's place in the
//!    definition in LOWER case: the control is `c` and the treatment at the Nth position of
//!    the request's list is `t<N>`, each within the modelled `TreatmentKey` pattern
//!    `^[a-zA-Z0-9]{1,8}$`. Those are the keys every operation that reports the definition
//!    answers, and the keys a run's snapshot of it carries, so a key a client reads off one
//!    response is a key it can assign an entity to in `TreatmentOverrides`.
//! 2. Every member of the document that targets `Iso8601DateTime`
//!    (`@timestampFormat("date-time")`) — `CreatedAt` and `UpdatedAt` — goes onto the
//!    restJson1 wire as an RFC 3339 / ISO-8601 STRING, never as an epoch number, on every
//!    path that answers one: the create, the read, the update, and the summary a listing
//!    reports the same definition in.
//!
//! The table spans what those rules generalise over: HOW MANY treatments the definition
//! holds, across the published `TreatmentInputList` bounds (1..5); whether a treatment
//! carries a `Description`, and whether that text is non-ASCII; whether the flag value is
//! enabled, and whether it carries attribute values; the weights; and how an update changes
//! the list's LENGTH (shrunk to the modelled minimum, grown to the modelled maximum, left
//! the same size). The recorded single-treatment request is one row of it. Every assertion
//! names its case and prints the actual value.

use aws_smithy_types::DateTime;
use serde_json::{json, Map, Value};

use crate::appconfig_workflow::router;
use crate::deployment_wire_fidelity::{iso_instant, member};
use crate::experiment_definitions::{create_definition, flag, get_definition, scope, treatment, Scope};
use crate::list_experiment_definitions::list;
use crate::start_experiment_run::start_run;
use crate::update_experiment_definition::update_definition;

/// The key AppConfig gives the control treatment of a definition.
const CONTROL_KEY: &str = "c";

/// The key AppConfig gives the treatment at `index` of a definition's list, counting from
/// `0`.
fn treatment_key(index: usize) -> String {
    format!("t{}", index + 1)
}

/// The keys one definition's treatments must carry: the control's, then one per treatment
/// in the order the request listed them.
fn expected_keys(treatments: usize) -> Vec<String> {
    std::iter::once(CONTROL_KEY.to_string())
        .chain((0..treatments).map(treatment_key))
        .collect()
}

/// One row: what a failing assertion calls it, the `Treatments` and `Control` the create
/// writes, and the `Treatments` list a later update REPLACES them with.
type Row = (&'static str, Vec<Value>, Value, Vec<Value>);

/// A flag value carrying attribute values, under keys of the shape `AttributeKey` allows
/// (`^(?!enabled$)[a-z][a-zA-Z0-9_-]{0,63}$`).
fn rich_flag(enabled: bool) -> Value {
    json!({
        "Enabled": enabled,
        "AttributeValues": {
            "greeting": { "StringValue": "hóla" },
            "ratio": { "NumberValue": 0.125 },
            "beta": { "BooleanValue": true },
            "regions": { "StringArray": ["us-east-1", "eu-west-1"] },
            "thresholds": { "NumberArray": [1.0, 2.5] },
        },
    })
}

/// `n` treatments, each with its own weight, description and flag value, so the position a
/// key is minted from is the only thing a row varies along.
fn treatments(n: usize, described: bool) -> Vec<Value> {
    (0..n)
        .map(|index| {
            let description = format!("variation {}", index + 1);
            treatment(
                100.0 / (n as f64 + 1.0),
                described.then_some(description.as_str()),
                if index % 2 == 0 {
                    flag(true)
                } else {
                    rich_flag(false)
                },
            )
        })
        .collect()
}

/// The rows. `TreatmentInputList` is `length 1..5`
/// (`smithy_query(query="operation", operation="CreateExperimentDefinition")`), so the
/// counts below are the published minimum, the maximum, and sizes between — written and
/// then replaced in both directions.
fn rows() -> Vec<Row> {
    vec![
        (
            "one treatment, replaced by one",
            treatments(1, true),
            treatment(50.0, Some("today's experience"), flag(false)),
            treatments(1, true),
        ),
        (
            "one treatment, grown to the modelled maximum of five",
            treatments(1, true),
            treatment(50.0, Some("today's experience"), flag(false)),
            treatments(5, true),
        ),
        (
            "five treatments, shrunk to the modelled minimum of one",
            treatments(5, true),
            treatment(0.0, None, rich_flag(false)),
            treatments(1, false),
        ),
        (
            "two treatments with no descriptions, grown to three",
            treatments(2, false),
            treatment(33.5, None, flag(true)),
            treatments(3, false),
        ),
        (
            "three treatments with attribute-bearing flag values",
            treatments(3, true),
            treatment(25.0, Some("対照"), rich_flag(true)),
            treatments(4, true),
        ),
        (
            "four treatments with non-ASCII descriptions",
            vec![
                treatment(10.0, Some("変化"), flag(true)),
                treatment(20.0, Some("βeta — \"quoted\" & <escaped>"), rich_flag(true)),
                treatment(30.0, Some("🚀 ship it"), flag(false)),
                treatment(40.0, Some(""), rich_flag(false)),
            ],
            treatment(0.5, Some("control — año"), flag(false)),
            treatments(2, true),
        ),
    ]
}

/// The request body one row's definition is created from.
fn create_body(scope: &Scope, name: &str, treatments: &[Value], control: &Value) -> Value {
    json!({
        "Name": name,
        "ConfigurationProfileIdentifier": scope.profile,
        "EnvironmentIdentifier": scope.environment,
        "FlagKey": "checkout-flow",
        "AudienceRule": "attribute(\"tier\") == \"beta\"",
        "Treatments": treatments,
        "Control": control,
    })
}

/// The `Key` of every treatment a document reports: the control's first, then one per entry
/// of `Treatments` in the order the document lists them.
fn keys_of(case: &str, document: &str, body: &Value) -> Vec<String> {
    let key_of = |owner: &str, treatment: &Value| -> String {
        member(treatment, "Key")
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {case:?}: {document}'s {owner} must carry a Key, got {treatment}")
            })
            .to_string()
    };
    let control = member(body, "Control");
    let treatments = member(body, "Treatments");
    let treatments = treatments.as_array().cloned().unwrap_or_else(|| {
        panic!("case {case:?}: {document} must report the definition's Treatments, got {body}")
    });
    std::iter::once(key_of("Control", &control))
        .chain(
            treatments
                .iter()
                .enumerate()
                .map(|(index, entry)| key_of(&format!("Treatments[{index}]"), entry)),
        )
        .collect()
}

/// Everything the two rules require of the keys ONE document reports, whichever operation
/// answered it.
fn assert_keys(case: &str, document: &str, body: &Value, treatments: usize) {
    let keys = keys_of(case, document, body);
    assert_eq!(
        keys,
        expected_keys(treatments),
        "case {case:?}: {document} must key the control {CONTROL_KEY:?} and each of its \
         {treatments} treatment(s) from its place in the list, got {keys:?}"
    );
    for key in &keys {
        assert!(
            (1..=8).contains(&key.chars().count())
                && key.chars().all(|c| c.is_ascii_alphanumeric()),
            "case {case:?}: {document}'s key {key:?} must match the modelled TreatmentKey \
             pattern ^[a-zA-Z0-9]{{1,8}}$"
        );
        assert_eq!(
            key.as_str(),
            key.to_lowercase().as_str(),
            "case {case:?}: {document}'s minted keys are lower case, got {key:?}"
        );
    }
    let mut unique = keys.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        keys.len(),
        "case {case:?}: {document} must key every treatment of the definition distinctly, \
         got {keys:?}"
    );
}

/// The write instants ONE document reports, each asserted to be on the wire as the modelled
/// `date-time` STRING, with the record's last write at or after its creation.
fn assert_instants(case: &str, document: &str, body: &Value) -> (DateTime, DateTime) {
    let created_at = iso_instant(case, document, "CreatedAt", &member(body, "CreatedAt"));
    let updated_at = iso_instant(case, document, "UpdatedAt", &member(body, "UpdatedAt"));
    assert!(
        updated_at.as_secs_f64() >= created_at.as_secs_f64(),
        "case {case:?}: {document} cannot have been written before it was created, got \
         {updated_at:?} before {created_at:?}"
    );
    (created_at, updated_at)
}

/// Every path that reports an experiment definition reports the service's minted treatment
/// keys and ISO-8601 write instants: the create, the read, the update, the listing's
/// summary, and the snapshot a run takes of it — and a key read back off one of them is a
/// key a run can assign an entity to.
#[tokio::test]
async fn an_experiment_definition_reports_minted_keys_and_iso_8601_instants() {
    let router = router();
    let scope = scope(&router, "definition-wire").await;

    for (index, (case, written, control, replacement)) in rows().into_iter().enumerate() {
        let name = format!("definition-{index}");
        let created = create_definition(
            &router,
            &scope.application,
            &create_body(&scope, &name, &written, &control),
        )
        .await;
        assert_eq!(
            created.status,
            201,
            "case {case:?}: CreateExperimentDefinition must answer 201, got {} / {}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );
        let created_body = created.json();
        assert_keys(case, "CreateExperimentDefinition", &created_body, written.len());
        let (created_at, updated_at) = assert_instants(case, "CreateExperimentDefinition", &created_body);
        assert_eq!(
            updated_at.as_secs_f64(),
            created_at.as_secs_f64(),
            "case {case:?}: nothing has updated the definition, so its UpdatedAt is its \
             CreatedAt, got {updated_at:?} and {created_at:?}"
        );
        let id = created
            .string("Id")
            .unwrap_or_else(|| panic!("case {case:?}: the create answered no Id: {created_body}"));

        // The read answers the stored record, so it reports the same keys and the same
        // instants, in the same wire form.
        let fetched = get_definition(&router, &scope.application, &id).await;
        assert_eq!(
            fetched.status,
            200,
            "case {case:?}: GetExperimentDefinition must answer 200, got {} / {}",
            fetched.status,
            String::from_utf8_lossy(&fetched.body)
        );
        let fetched_body = fetched.json();
        assert_keys(case, "GetExperimentDefinition", &fetched_body, written.len());
        assert_instants(case, "GetExperimentDefinition", &fetched_body);
        for name in ["CreatedAt", "UpdatedAt"] {
            assert_eq!(
                member(&fetched_body, name),
                member(&created_body, name),
                "case {case:?}: GetExperimentDefinition must report the {name} the create \
                 stored, got {} then {}",
                member(&created_body, name),
                member(&fetched_body, name)
            );
        }

        // Replacing the treatment list re-mints the keys from each treatment's place in the
        // new list, and writes the record — so `UpdatedAt` moves while `CreatedAt` stays.
        let patched = update_definition(
            &router,
            &scope.application,
            &id,
            &json!({ "Treatments": replacement, "Control": control }),
        )
        .await;
        assert_eq!(
            patched.status,
            200,
            "case {case:?}: UpdateExperimentDefinition must answer 200, got {} / {}",
            patched.status,
            String::from_utf8_lossy(&patched.body)
        );
        let patched_body = patched.json();
        assert_keys(
            case,
            "UpdateExperimentDefinition",
            &patched_body,
            replacement.len(),
        );
        let (patched_created_at, _) = assert_instants(case, "UpdateExperimentDefinition", &patched_body);
        assert_eq!(
            patched_created_at.as_secs_f64(),
            created_at.as_secs_f64(),
            "case {case:?}: an update does not move CreatedAt, got {patched_created_at:?} \
             for {created_at:?}"
        );

        // The listing reports the same definition as a summary, whose instants are the
        // definition's own and in the same wire form.
        let page = list(
            &router,
            &[("application_identifier", scope.application.clone())],
        )
        .await;
        assert_eq!(
            page.status,
            200,
            "case {case:?}: ListExperimentDefinitions must answer 200, got {} / {}",
            page.status,
            String::from_utf8_lossy(&page.body)
        );
        let page_body = page.json();
        let summary = member(&page_body, "Items")
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no Items array in {page_body}"))
            .iter()
            .find(|summary| member(summary, "Id").as_str() == Some(id.as_str()))
            .cloned()
            .unwrap_or_else(|| {
                panic!("case {case:?}: the listing must report definition {id:?}: {page_body}")
            });
        assert_instants(case, "ListExperimentDefinitions", &summary);
        for name in ["CreatedAt", "UpdatedAt"] {
            assert_eq!(
                member(&summary, name),
                member(&patched_body, name),
                "case {case:?}: a summary reports the definition's own {name}, got {} for {}",
                member(&summary, name),
                member(&patched_body, name)
            );
        }

        // A key read off the definition is a key a run can assign an entity to, and the run
        // snapshots the definition's keys as they stand.
        let assignments: Map<String, Value> = keys_of(case, "UpdateExperimentDefinition", &patched_body)
            .into_iter()
            .enumerate()
            .map(|(entity, key)| (format!("entity-{entity}"), Value::from(key)))
            .collect();
        let started = start_run(
            &router,
            &scope.application,
            &id,
            &json!({ "TreatmentOverrides": { "Inline": Value::Object(assignments.clone()) } }),
        )
        .await;
        assert_eq!(
            started.status,
            201,
            "case {case:?}: a run assigned to the keys the definition reports must be \
             accepted, got {} / {}",
            started.status,
            String::from_utf8_lossy(&started.body)
        );
        let run_body = started.json();
        assert_eq!(
            member(&run_body, "TreatmentOverrides"),
            json!({ "Inline": Value::Object(assignments) }),
            "case {case:?}: the run must report the assignments it stored, got {}",
            member(&run_body, "TreatmentOverrides")
        );
        assert_keys(
            case,
            "StartExperimentRun's ExperimentDefinitionSnapshot",
            &member(&run_body, "ExperimentDefinitionSnapshot"),
            replacement.len(),
        );
    }
}
