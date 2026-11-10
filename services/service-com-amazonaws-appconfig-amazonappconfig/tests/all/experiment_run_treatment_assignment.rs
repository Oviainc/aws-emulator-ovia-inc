// SPDX-License-Identifier: CC0-1.0

//! WHICH treatment an inline override may assign an entity to, on both of the operations
//! that write `TreatmentOverrides` — `StartExperimentRun` and `UpdateExperimentRun`.
//!
//! The rule under test, one level more general than any single request: an entry of
//! `TreatmentOverrides$Inline` "assigns the specified entity to the specified treatment"
//! (`smithy_query(query="doc", shape="TreatmentOverrides", member="Inline")`), so its value
//! must be a treatment of the experiment the request addresses — the modelled `TreatmentKey`
//! pattern says what such a name may LOOK like, not which names exist. A value no treatment
//! of that experiment carries is refused HTTP 400 `BadRequestException`, stated as the name
//! the request sent followed by the names it could have sent — the keys the service minted
//! for that experiment, the control's first and then each compared treatment in declaration
//! order — and the refusal writes nothing: no run is created, no run number is issued, the
//! definition stays `IDLE` and an update leaves the stored assignment exactly as it was.
//! The same call naming a key the experiment DOES define is accepted.
//!
//! The tables span what the rule generalises over rather than the one recorded request: how
//! many treatments the experiment defines (1 through the modelled `TreatmentInputList`
//! maximum of 5, from `smithy_query(query="operation",
//! operation="CreateExperimentDefinition")`), WHICH name the request assigns (a name no
//! experiment defines, the next index past the list, the wrong case of a defined key, a
//! single character, a name at the `TreatmentKey` 8-character bound, an index-zero key), how
//! many entries the map carries and whether the offending one is the first by entity id, and
//! WHICH of the two writing operations made the call.

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::appconfig_workflow::router;
use crate::experiment_definitions::{
    body, create_definition, flag, get_definition, scope, treatment, Scope,
};
use crate::list_experiment_runs::list;
use crate::start_experiment_run::{assert_refused, assert_running, get_run, id_of, start_run};
use crate::update_experiment_run::update_run;

/// A definition with `treatments` compared treatments beside its control, so a row can vary
/// how many names the experiment defines. `TreatmentInputList` is `length 1..5`.
async fn definition_with(
    router: &MultiServiceRouter,
    scope: &Scope,
    name: &str,
    treatments: usize,
) -> Value {
    let listed: Vec<Value> = (0..treatments)
        .map(|index| {
            treatment(
                50.0 / treatments as f64,
                Some(&format!("variation {index}")),
                flag(index % 2 == 0),
            )
        })
        .collect();
    let answer = create_definition(
        router,
        &scope.application,
        &body(scope, name, listed, treatment(50.0, None, flag(false))),
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "case {name:?}: the definition under test must be created, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    answer.json()
}

/// The treatment names `definition` defines, read off the document the service answered:
/// the control's `Key` first, then one per entry of `Treatments` in the order the document
/// lists them. These are the names an override may assign, in the order a refusal states
/// them.
fn defined_keys(case: &str, definition: &Value) -> Vec<String> {
    let key_of = |owner: &str, treatment: &Value| -> String {
        treatment
            .get("Key")
            .and_then(Value::as_str)
            .unwrap_or_else(|| {
                panic!("case {case:?}: the definition's {owner} carries no Key: {treatment}")
            })
            .to_string()
    };
    let control = definition.get("Control").unwrap_or_else(|| {
        panic!("case {case:?}: the definition reports no Control: {definition}")
    });
    let treatments = definition
        .get("Treatments")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!("case {case:?}: the definition reports no Treatments: {definition}")
        });
    std::iter::once(key_of("Control", control))
        .chain(
            treatments
                .iter()
                .enumerate()
                .map(|(index, entry)| key_of(&format!("Treatments[{index}]"), entry)),
        )
        .collect()
}

/// How AppConfig states the refusal: the name the request assigned, then the names the
/// experiment defines, in the order it defines them.
fn expected_message(assigned: &str, defined: &[String]) -> String {
    format!(
        "Invalid treatment name: {assigned}. Valid treatment names are: [{}]",
        defined.join(", ")
    )
}

/// An inline `TreatmentOverrides` payload from `(entity, treatment)` pairs.
fn overrides<'pair>(pairs: impl IntoIterator<Item = (&'pair str, String)>) -> Value {
    let assignments: Map<String, Value> = pairs
        .into_iter()
        .map(|(entity, treatment)| (entity.to_string(), Value::from(treatment)))
        .collect();
    json!({ "Inline": Value::Object(assignments) })
}

/// Assert a refusal's whole message.
fn assert_message(case: &str, answer: &crate::appconfig_workflow::Answer, expected: &str) {
    let actual = answer.string("Message").unwrap_or_else(|| {
        panic!(
            "case {case:?}: the refusal carried no Message: {}",
            String::from_utf8_lossy(&answer.body)
        )
    });
    assert_eq!(
        actual, expected,
        "case {case:?}: expected message {expected:?}, got {actual:?}"
    );
}

/// The run numbers a definition's history reports.
async fn listed_runs(
    router: &MultiServiceRouter,
    case: &str,
    application: &str,
    definition: &str,
) -> Vec<i64> {
    let answer = list(router, application, definition, &[]).await;
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: ListExperimentRuns expected 200, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    answer
        .json()
        .get("Items")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    item.get("Run").and_then(Value::as_i64).unwrap_or_else(|| {
                        panic!("case {case:?}: a listed run carries no Run: {item}")
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The `Status` a definition reports.
async fn definition_status(
    router: &MultiServiceRouter,
    case: &str,
    application: &str,
    definition: &str,
) -> String {
    let answer = get_definition(router, application, definition).await;
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: GetExperimentDefinition expected 200, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    answer
        .string("Status")
        .unwrap_or_else(|| panic!("case {case:?}: the definition reports no Status"))
}

/// One row: how many treatments the experiment defines, and the assignment the row sends —
/// the entity/name pairs and which of those names the experiment does not define.
struct Row {
    case: &'static str,
    treatments: usize,
    /// `(entity, name)` pairs, where a name is either a defined key by INDEX into the
    /// experiment's own key list (`None` for an undefined literal) or the literal itself.
    assignment: &'static [(&'static str, Assigned)],
    /// The undefined name the refusal must state: the one the rule reports for this map.
    offending: &'static str,
}

/// What a row assigns an entity to: a name the experiment defines, taken from its own key
/// list by position, or a literal name the experiment does not define.
enum Assigned {
    Defined(usize),
    Undefined(&'static str),
}

/// The rows' assignments, resolved against the experiment's own minted keys.
fn resolve(case: &str, assignment: &[(&str, Assigned)], defined: &[String]) -> Value {
    overrides(assignment.iter().map(|(entity, assigned)| {
        let name = match assigned {
            Assigned::Defined(index) => defined
                .get(*index)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: the experiment defines no key at {index}: {defined:?}")
                })
                .clone(),
            Assigned::Undefined(literal) => (*literal).to_string(),
        };
        (*entity, name)
    }))
}

/// The table: every row assigns at least one name the experiment does not define, while
/// staying inside every modelled constraint on the map (`length 0..32`, keys `EntityId` len
/// 1..2048, values `TreatmentKey` `^[a-zA-Z0-9]{1,8}$`) — so the refusal under test is the
/// treatment-name rule and nothing else.
const ROWS: &[Row] = &[
    Row {
        case: "a name no experiment defines, against one treatment",
        treatments: 1,
        assignment: &[("entity-1", Assigned::Undefined("nosuchtr"))],
        offending: "nosuchtr",
    },
    Row {
        case: "the next index past a one-treatment list",
        treatments: 1,
        assignment: &[("entity-1", Assigned::Undefined("t2"))],
        offending: "t2",
    },
    Row {
        case: "the next index past the modelled maximum of five treatments",
        treatments: 5,
        assignment: &[("entity-1", Assigned::Undefined("t6"))],
        offending: "t6",
    },
    Row {
        case: "the wrong case of a defined treatment key",
        treatments: 2,
        assignment: &[("entity-1", Assigned::Undefined("T1"))],
        offending: "T1",
    },
    Row {
        case: "the wrong case of the control's key",
        treatments: 3,
        assignment: &[("entity-1", Assigned::Undefined("C"))],
        offending: "C",
    },
    Row {
        case: "a single-character name the experiment does not define",
        treatments: 2,
        assignment: &[("entity-1", Assigned::Undefined("x"))],
        offending: "x",
    },
    Row {
        case: "an index-zero key, which no experiment mints",
        treatments: 4,
        assignment: &[("entity-1", Assigned::Undefined("t0"))],
        offending: "t0",
    },
    Row {
        case: "a name at the TreatmentKey 8-character bound",
        treatments: 1,
        assignment: &[("entity-1", Assigned::Undefined("abcdefgh"))],
        offending: "abcdefgh",
    },
    Row {
        case: "one undefined name beside two defined ones, assigned last by entity id",
        treatments: 2,
        assignment: &[
            ("entity-1", Assigned::Defined(0)),
            ("entity-2", Assigned::Defined(2)),
            ("entity-3", Assigned::Undefined("t3")),
        ],
        offending: "t3",
    },
    Row {
        case: "one undefined name beside a defined one, assigned first by entity id",
        treatments: 5,
        assignment: &[
            ("entity-1", Assigned::Undefined("9")),
            ("entity-2", Assigned::Defined(5)),
        ],
        offending: "9",
    },
];

/// A start that assigns a name the experiment does not define is refused with the
/// treatment-name template and creates no run, while the same start naming every key the
/// experiment DOES define is accepted.
#[tokio::test]
async fn a_start_assigning_an_undefined_treatment_is_refused_and_writes_nothing() {
    let router = router();
    let scope = scope(&router, "assignment-start").await;

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        // One definition per row: a definition delivers one experiment at a time.
        let definition =
            definition_with(&router, &scope, &format!("start-{index}"), row.treatments).await;
        let definition_id = id_of(case, &definition);
        let defined = defined_keys(case, &definition);
        let refused_body =
            json!({ "TreatmentOverrides": resolve(case, row.assignment, &defined) });

        let refused = start_run(&router, &scope.application, &definition_id, &refused_body).await;
        assert_refused(case, &refused, 400, "BadRequestException");
        assert_message(case, &refused, &expected_message(row.offending, &defined));

        // The refusal wrote nothing: no run, no number issued, and the definition has not
        // moved out of the status a definition nothing has run holds.
        let runs = listed_runs(&router, case, &scope.application, &definition_id).await;
        assert!(
            runs.is_empty(),
            "case {case:?}: a refused start must list no run, got {runs:?}"
        );
        let status = definition_status(&router, case, &scope.application, &definition_id).await;
        assert_eq!(
            status, "IDLE",
            "case {case:?}: a refused start must leave the definition IDLE, got {status:?}"
        );
        let unissued = get_run(&router, &scope.application, &definition_id, 1).await;
        assert_refused(case, &unissued, 404, "ResourceNotFoundException");

        // The positive control: the same call assigning one entity to each name the
        // experiment defines starts the run, and the run it started is the only one listed.
        let accepted_assignment = overrides(
            defined
                .iter()
                .enumerate()
                .map(|(position, key)| (ENTITIES[position], key.clone())),
        );
        let accepted = start_run(
            &router,
            &scope.application,
            &definition_id,
            &json!({ "TreatmentOverrides": accepted_assignment.clone() }),
        )
        .await;
        let started = assert_running(case, &accepted, 1, &definition);
        assert_eq!(
            started.get("TreatmentOverrides"),
            Some(&accepted_assignment),
            "case {case:?}: expected the assignment {accepted_assignment:?} echoed, got {:?}",
            started.get("TreatmentOverrides")
        );
        let runs = listed_runs(&router, case, &scope.application, &definition_id).await;
        assert_eq!(
            runs,
            vec![1],
            "case {case:?}: expected exactly the started run listed, got {runs:?}"
        );
    }
}

/// Entity ids the positive control assigns, one per name an experiment can define: the
/// control's and up to the modelled maximum of five treatments.
const ENTITIES: &[&str] = &[
    "entity-a", "entity-b", "entity-c", "entity-d", "entity-e", "entity-f",
];

/// The same rule on the update side: an update that assigns a name the experiment does not
/// define is refused with the same template and leaves the stored assignment untouched,
/// while an update naming a defined key replaces it.
#[tokio::test]
async fn an_update_assigning_an_undefined_treatment_is_refused_and_writes_nothing() {
    let router = router();
    let scope = scope(&router, "assignment-update").await;

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let definition =
            definition_with(&router, &scope, &format!("update-{index}"), row.treatments).await;
        let definition_id = id_of(case, &definition);
        let defined = defined_keys(case, &definition);

        // The run the update addresses, started with one assignment the experiment defines.
        let seeded = overrides([("entity-seed", defined[0].clone())]);
        let created = start_run(
            &router,
            &scope.application,
            &definition_id,
            &json!({ "TreatmentOverrides": seeded.clone() }),
        )
        .await;
        let started = assert_running(case, &created, 1, &definition);

        let refused = update_run(
            &router,
            &scope.application,
            &definition_id,
            1,
            &json!({ "TreatmentOverrides": resolve(case, row.assignment, &defined) }),
        )
        .await;
        assert_refused(case, &refused, 400, "BadRequestException");
        assert_message(case, &refused, &expected_message(row.offending, &defined));

        // The refused update wrote nothing: the run still reports what the start assigned.
        let read = get_run(&router, &scope.application, &definition_id, 1).await;
        assert_eq!(
            read.json(),
            started,
            "case {case:?}: a refused update must leave the run as it was {started:?}, got {:?}",
            read.json()
        );

        // The positive control: the same update naming the LAST key the experiment defines
        // replaces the assignment, and the record reports the replacement.
        let replacement = overrides([(
            "entity-seed",
            defined
                .last()
                .unwrap_or_else(|| panic!("case {case:?}: the experiment defines no key"))
                .clone(),
        )]);
        let accepted = update_run(
            &router,
            &scope.application,
            &definition_id,
            1,
            &json!({ "TreatmentOverrides": replacement.clone() }),
        )
        .await;
        assert_eq!(
            accepted.status,
            200,
            "case {case:?}: an update naming a defined treatment must be accepted, got {} / {}",
            accepted.status,
            String::from_utf8_lossy(&accepted.body)
        );
        assert_eq!(
            accepted.json().get("TreatmentOverrides"),
            Some(&replacement),
            "case {case:?}: expected the assignment {replacement:?} stored, got {:?}",
            accepted.json().get("TreatmentOverrides")
        );
    }
}

/// Every name an experiment defines is assignable: one start per key, each on its own
/// definition, across the modelled bounds of the treatment list.
#[tokio::test]
async fn every_treatment_the_experiment_defines_can_be_assigned() {
    let router = router();
    let scope = scope(&router, "assignment-keys").await;

    for treatments in [1_usize, 2, 5] {
        let case = format!("{treatments} treatment(s)");
        let probe = definition_with(
            &router,
            &scope,
            &format!("keys-probe-{treatments}"),
            treatments,
        )
        .await;
        let defined = defined_keys(&case, &probe);
        assert_eq!(
            defined.len(),
            treatments + 1,
            "case {case:?}: an experiment defines its control and each treatment, got \
             {defined:?}"
        );

        for (position, key) in defined.iter().enumerate() {
            let case = format!("{case}, assigning {key:?}");
            let definition = definition_with(
                &router,
                &scope,
                &format!("keys-{treatments}-{position}"),
                treatments,
            )
            .await;
            let definition_id = id_of(&case, &definition);
            let assignment = overrides([("entity-1", key.clone())]);
            let accepted = start_run(
                &router,
                &scope.application,
                &definition_id,
                &json!({ "TreatmentOverrides": assignment.clone() }),
            )
            .await;
            let started = assert_running(&case, &accepted, 1, &definition);
            assert_eq!(
                started.get("TreatmentOverrides"),
                Some(&assignment),
                "case {case:?}: expected the assignment {assignment:?} echoed, got {:?}",
                started.get("TreatmentOverrides")
            );
        }
    }
}
