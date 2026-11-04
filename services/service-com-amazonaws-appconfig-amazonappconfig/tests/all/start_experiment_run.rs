// SPDX-License-Identifier: CC0-1.0

//! `StartExperimentRun`, and the read that proves what it wrote — `GetExperimentRun` —
//! end to end over HTTP.
//!
//! The rules under test:
//!
//! * a definition delivers ONE experiment at a time, so a start creates a run only while
//!   no run of that definition is running: of any number of starts aimed at one
//!   definition exactly one is created (HTTP 201, a `Run` number, `Status` `RUNNING`) and
//!   every other is refused HTTP 409 `ConflictException` without writing anything —
//!   whether those starts arrive simultaneously or one after the other;
//! * the guard is per DEFINITION: a run in flight on one definition never refuses a start
//!   on another, under the same application or another one;
//! * runs are numbered within the definition from 1, and a refused start issues no
//!   number;
//! * a start persists the run it answers — the members the request carried, the status a
//!   just-started run holds, the instants it was started and last written, and a snapshot
//!   of the definition as it stood — so `GetExperimentRun` reports that same record, and
//!   the definition itself then reports `ACTIVE`;
//! * an identifier or a run number that addresses nothing is HTTP 404
//!   `ResourceNotFoundException`;
//! * a value outside a published modelled bound is refused HTTP 400
//!   `BadRequestException` and writes nothing, while the same member AT its bound is
//!   accepted;
//! * the run document reports the members `ExperimentRun` models and no others — the
//!   `Tags` and `DeploymentParameters` a start accepts are not members of it — and the
//!   instant it was last written is the instant it was started, while nothing has written
//!   the record since;
//! * reading a run reports it without changing it: repeated reads answer the same
//!   document, and neither they nor a refused duplicate start move the run or the
//!   definition.
//!
//! The tables span what those rules generalise over rather than the one recorded request:
//! HOW MANY starts race for a definition (two through eight), whether they race or follow
//! one another, which application holds the definition and whether it is addressed by id
//! or by name, which optional members the start carries and their values at the published
//! bounds (`Description` len 0..1024, `NullablePercentage` range 0..100, from
//! `smithy_query(query="operation", operation="StartExperimentRun")`), how many treatment
//! overrides are assigned, and WHICH part of the path — application, definition or run
//! number — resolves to nothing.

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::appconfig_workflow::{create_application, is_modelled_id, json_call, router, Answer};
use crate::experiment_definitions::{
    create_definition, flag, get_definition, scope, treatment, Scope,
};
use crate::resource_tags::{filler, pct};

/// The members `ExperimentDefinitionSnapshot` carries
/// (`smithy_query(query="shape", shape="ExperimentDefinitionSnapshot")`). A run's snapshot
/// must report each of them exactly as the definition's own create reported it, and must
/// report none the definition does not hold.
const SNAPSHOT_MEMBERS: &[&str] = &[
    "ApplicationId",
    "Id",
    "Name",
    "Hypothesis",
    "ConfigurationProfileId",
    "EnvironmentId",
    "FlagKey",
    "AudienceRule",
    "AudienceDescription",
    "LaunchCriteria",
    "Treatments",
    "Control",
];

/// Every member of the output shape both `StartExperimentRun` and `GetExperimentRun`
/// answer — `ExperimentRun`
/// (`smithy_query(query="operation", operation="StartExperimentRun")`). A run document
/// reports members from this list and nothing else: the `Tags` and `DeploymentParameters`
/// the start ACCEPTS are input members with no counterpart here.
const EXPERIMENT_RUN_MEMBERS: &[&str] = &[
    "ApplicationId",
    "ExperimentDefinitionId",
    "Run",
    "Description",
    "Status",
    "ExposurePercentage",
    "TreatmentOverrides",
    "Result",
    "StartedAt",
    "UpdatedAt",
    "EndedAt",
    "ExperimentDefinitionSnapshot",
];

/// `POST /applications/{ApplicationIdentifier}/experimentdefinitions/{ExperimentDefinitionIdentifier}/experimentruns`.
pub(crate) async fn start_run(
    router: &MultiServiceRouter,
    application: &str,
    definition: &str,
    body: &Value,
) -> Answer {
    json_call(
        router,
        "POST",
        &format!(
            "/applications/{}/experimentdefinitions/{}/experimentruns",
            pct(application),
            pct(definition)
        ),
        body,
    )
    .await
}

/// `GET .../experimentruns/{Run}`.
pub(crate) async fn get_run(
    router: &MultiServiceRouter,
    application: &str,
    definition: &str,
    run: i64,
) -> Answer {
    json_call(
        router,
        "GET",
        &format!(
            "/applications/{}/experimentdefinitions/{}/experimentruns/{run}",
            pct(application),
            pct(definition)
        ),
        &Value::Null,
    )
    .await
}

/// Create one experiment definition under `scope` and answer the create's document, which
/// carries both its minted `Id` and the record every run's snapshot is taken from.
pub(crate) async fn create_test_definition(
    router: &MultiServiceRouter,
    scope: &Scope,
    name: &str,
) -> Value {
    let answer = create_definition(
        router,
        &scope.application,
        &json!({
            "Name": name,
            "ConfigurationProfileIdentifier": scope.profile,
            "EnvironmentIdentifier": scope.environment,
            "FlagKey": "checkout-flow",
            "AudienceRule": "attribute(\"tier\") == \"beta\"",
            "Hypothesis": "the new flow converts better",
            "Treatments": [treatment(50.0, Some("the new flow"), flag(true))],
            "Control": treatment(50.0, Some("today's experience"), flag(false)),
        }),
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

/// A definition document's minted id.
pub(crate) fn id_of(case: &str, definition: &Value) -> String {
    definition
        .get("Id")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: the definition answered no Id: {definition:?}"))
        .to_string()
}

/// A JSON object from `(key, value)` pairs — the wire form of `TagMap` and of the inline
/// `TreatmentOverrides`.
pub(crate) fn string_map<'pair>(pairs: impl IntoIterator<Item = (&'pair str, &'pair str)>) -> Value {
    Value::Object(
        pairs
            .into_iter()
            .map(|(key, value)| (key.to_string(), Value::from(value)))
            .collect::<Map<String, Value>>(),
    )
}

/// Assert a started run: 201, the expected run number, `Status` `RUNNING`, no end instant
/// while it runs, and a snapshot of the definition it was started from.
pub(crate) fn assert_running(
    case: &str,
    created: &Answer,
    expected_run: i64,
    definition: &Value,
) -> Value {
    assert_eq!(
        created.status,
        201,
        "case {case:?}: expected 201, got {} / {}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    let body = created.json();
    assert_eq!(
        body.get("Run").and_then(Value::as_i64),
        Some(expected_run),
        "case {case:?}: expected Run {expected_run}, got {:?}",
        body.get("Run")
    );
    assert_eq!(
        body.get("Status").and_then(Value::as_str),
        Some("RUNNING"),
        "case {case:?}: expected Status RUNNING, got {:?}",
        body.get("Status")
    );
    assert_eq!(
        body.get("EndedAt"),
        None,
        "case {case:?}: a run that is still running has no EndedAt, got {:?}",
        body.get("EndedAt")
    );
    assert_eq!(
        body.get("Result"),
        None,
        "case {case:?}: a run that has just started has no Result, got {:?}",
        body.get("Result")
    );
    assert!(
        body.get("StartedAt").is_some() && body.get("UpdatedAt").is_some(),
        "case {case:?}: expected StartedAt and UpdatedAt, got {body:?}"
    );
    assert_eq!(
        body.get("ExperimentDefinitionId").and_then(Value::as_str),
        definition.get("Id").and_then(Value::as_str),
        "case {case:?}: expected the resolved definition id {:?}, got {:?}",
        definition.get("Id"),
        body.get("ExperimentDefinitionId")
    );
    let snapshot = body
        .get("ExperimentDefinitionSnapshot")
        .unwrap_or_else(|| panic!("case {case:?}: expected a snapshot, got {body:?}"));
    for member in SNAPSHOT_MEMBERS {
        assert_eq!(
            snapshot.get(member),
            definition.get(member),
            "case {case:?}: snapshot {member} must be the definition's {:?}, got {:?}",
            definition.get(member),
            snapshot.get(member)
        );
    }
    body
}

/// Assert that reading the run back reports byte for byte what the start answered.
pub(crate) async fn assert_reads_back(
    router: &MultiServiceRouter,
    case: &str,
    application: &str,
    definition: &str,
    started: &Value,
) {
    let run = started
        .get("Run")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("case {case:?}: the start answered no Run: {started:?}"));
    let read = get_run(router, application, definition, run).await;
    assert_eq!(
        read.status,
        200,
        "case {case:?}: expected GetExperimentRun 200, got {} / {}",
        read.status,
        String::from_utf8_lossy(&read.body)
    );
    assert_eq!(
        read.json(),
        *started,
        "case {case:?}: expected GetExperimentRun to report the started run {started:?}, got {:?}",
        read.json()
    );
}

/// Assert a refusal: the status the operation models for it and its wire error code.
pub(crate) fn assert_refused(
    case: &str,
    refused: &Answer,
    expected_status: u16,
    expected_code: &str,
) {
    assert_eq!(
        refused.status,
        expected_status,
        "case {case:?}: expected {expected_status}, got {} / {}",
        refused.status,
        String::from_utf8_lossy(&refused.body)
    );
    assert!(
        refused.error_code().contains(expected_code),
        "case {case:?}: expected {expected_code}, got {:?}",
        refused.error_code()
    );
}

/// Of any number of simultaneous starts on ONE definition, exactly one creates the run
/// and every other is told `ConflictException`; the run the winner created is the run
/// `GetExperimentRun` reports, and no second run was written.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn exactly_one_of_any_number_of_simultaneous_starts_creates_the_run() {
    let router = router();
    let scope = scope(&router, "race").await;

    // Rows differ along HOW MANY starters race for the same definition: the rule is
    // "exactly one run is created", not "the second caller loses".
    for starters in [2_usize, 3, 5, 8] {
        let case = format!("{starters} simultaneous starts on one definition");
        let definition = create_test_definition(&router, &scope, &format!("race-{starters}")).await;
        let definition_id = id_of(&case, &definition);

        let mut tasks = Vec::new();
        for starter in 0..starters {
            let router = router.clone();
            let application = scope.application.clone();
            let definition_id = definition_id.clone();
            tasks.push(tokio::spawn(async move {
                start_run(
                    &router,
                    &application,
                    &definition_id,
                    &json!({ "Description": format!("starter {starter}") }),
                )
                .await
            }));
        }

        let mut created = Vec::new();
        let mut conflicted = Vec::new();
        for task in tasks {
            let answer = task
                .await
                .unwrap_or_else(|error| panic!("case {case:?}: a starter did not finish: {error}"));
            match answer.status {
                201 => created.push(answer),
                409 => conflicted.push(answer),
                other => panic!(
                    "case {case:?}: a start answered neither 201 nor 409 but {other}: {}",
                    String::from_utf8_lossy(&answer.body)
                ),
            }
        }

        assert_eq!(
            created.len(),
            1,
            "case {case:?}: expected exactly one start to succeed, got {}",
            created.len()
        );
        assert_eq!(
            conflicted.len(),
            starters - 1,
            "case {case:?}: expected {} starts to be refused, got {}",
            starters - 1,
            conflicted.len()
        );
        for refusal in &conflicted {
            assert_refused(&case, refusal, 409, "ConflictException");
        }

        // The winner is run 1 of a definition nothing had run before, and it reads back.
        let started = assert_running(&case, &created[0], 1, &definition);
        assert_reads_back(&router, &case, &scope.application, &definition_id, &started).await;

        // The refused starts issued no run number: the definition holds run 1 and nothing
        // else, so a read of the next number misses.
        let absent = get_run(&router, &scope.application, &definition_id, 2).await;
        assert_refused(&case, &absent, 404, "ResourceNotFoundException");

        // A definition with a run in flight is the one that reports ACTIVE.
        let read_definition = get_definition(&router, &scope.application, &definition_id).await;
        assert_eq!(
            read_definition.json().get("Status").and_then(Value::as_str),
            Some("ACTIVE"),
            "case {case:?}: expected the definition to report ACTIVE, got {:?}",
            read_definition.json().get("Status")
        );
    }
}

/// A start that follows an earlier one is refused for the same reason a simultaneous one
/// is — the rule is about a run being in flight, not about the two calls overlapping —
/// and the refusal writes nothing.
#[tokio::test]
async fn a_later_start_while_a_run_is_in_flight_is_refused_without_writing() {
    let router = router();
    let scope = scope(&router, "sequential").await;
    let case = "a second start after the first";
    let definition = create_test_definition(&router, &scope, "sequential").await;
    let definition_id = id_of(case, &definition);

    let first = start_run(&router, &scope.application, &definition_id, &json!({})).await;
    let started = assert_running(case, &first, 1, &definition);

    // Rows differ along what the duplicate start carries: none of them is written while a
    // run is in flight.
    for (row, body) in [
        ("an empty duplicate start", json!({})),
        (
            "a duplicate start with a description",
            json!({ "Description": "must not be stored" }),
        ),
        (
            "a duplicate start with every optional member",
            json!({
                "Description": "must not be stored either",
                "ExposurePercentage": 25.0,
                "TreatmentOverrides": { "Inline": { "entity-1": "T1" } },
                "Tags": { "owner": "experiments" },
            }),
        ),
    ] {
        let refused = start_run(&router, &scope.application, &definition_id, &body).await;
        assert_refused(row, &refused, 409, "ConflictException");
        // Still exactly one run, still the one the first start wrote.
        assert_reads_back(&router, row, &scope.application, &definition_id, &started).await;
        let absent = get_run(&router, &scope.application, &definition_id, 2).await;
        assert_refused(row, &absent, 404, "ResourceNotFoundException");
    }
}

/// The guard is per definition: a run in flight on one definition refuses nothing on
/// another, under the same application or a different one, however each is addressed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_run_in_flight_on_one_definition_does_not_refuse_a_start_on_another() {
    let router = router();
    let here = scope(&router, "neighbours").await;
    let elsewhere = scope(&router, "elsewhere").await;

    let first = create_test_definition(&router, &here, "neighbour-one").await;
    let second = create_test_definition(&router, &here, "neighbour-two").await;
    let third = create_test_definition(&router, &elsewhere, "far-away").await;

    // Rows differ along which application holds the definition and whether the pair of
    // identifiers is written as ids or as names.
    let rows: Vec<(&str, String, String, Value)> = vec![
        (
            "the first definition of its application, by id",
            here.application.clone(),
            id_of("the first definition of its application, by id", &first),
            first.clone(),
        ),
        (
            "a second definition of the same application, by name",
            "app-neighbours".to_string(),
            "neighbour-two".to_string(),
            second.clone(),
        ),
        (
            "a definition of another application, by id",
            elsewhere.application.clone(),
            id_of("a definition of another application, by id", &third),
            third.clone(),
        ),
    ];

    // Every definition is started at once: each is its own definition, so each start wins.
    let mut tasks = Vec::new();
    for (case, application, definition, _) in &rows {
        let router = router.clone();
        let case = case.to_string();
        let application = application.clone();
        let definition = definition.clone();
        tasks.push(tokio::spawn(async move {
            (
                case,
                start_run(&router, &application, &definition, &json!({})).await,
            )
        }));
    }
    let mut answers = Vec::new();
    for task in tasks {
        answers.push(
            task.await
                .unwrap_or_else(|error| panic!("a starter did not finish: {error}")),
        );
    }

    for (case, answer) in &answers {
        let (_, application, definition, document) = rows
            .iter()
            .find(|(row, _, _, _)| row == case)
            .unwrap_or_else(|| panic!("case {case:?}: the row must be in the table"));
        // Each definition numbers its own runs from 1, independently of its neighbours.
        let started = assert_running(case, answer, 1, document);
        assert_reads_back(&router, case, application, definition, &started).await;
        // And each definition is now the one guarding itself: its own second start loses.
        let refused = start_run(&router, application, definition, &json!({})).await;
        assert_refused(case, &refused, 409, "ConflictException");
    }
}

/// A start persists the members the request carried and nothing it did not, and the run it
/// wrote is what a later read reports.
#[tokio::test]
async fn a_started_run_reports_the_members_the_request_carried() {
    let router = router();
    let scope = scope(&router, "members").await;

    // Rows differ along WHICH optional members the start carries and their values at the
    // published bounds: `Description` len 0..1024, `NullablePercentage` range 0..100, and
    // the inline `TreatmentOverrides` from none to several assignments.
    let longest_description = filler(1024, 5);
    let rows: Vec<(&str, Value)> = vec![
        ("nothing optional", json!({})),
        (
            "a description at the lower length bound",
            json!({ "Description": "" }),
        ),
        (
            "a description at the upper length bound",
            json!({ "Description": longest_description.clone() }),
        ),
        (
            "an exposure of 0, which validates without exposing anyone",
            json!({ "ExposurePercentage": 0.0 }),
        ),
        (
            "a fractional exposure",
            json!({ "ExposurePercentage": 0.5 }),
        ),
        (
            "an exposure at the upper range bound",
            json!({ "ExposurePercentage": 100.0 }),
        ),
        (
            "one treatment override",
            json!({ "TreatmentOverrides": { "Inline": string_map([("entity-1", "T1")]) } }),
        ),
        (
            "several treatment overrides and tags",
            json!({
                "Description": "overridden assignment",
                "ExposurePercentage": 12.5,
                "TreatmentOverrides": { "Inline": string_map([
                    ("entity-1", "T1"),
                    ("entity-2", "C"),
                    ("entity-3", "T1"),
                ]) },
                "Tags": string_map([("owner", "experiments"), ("stage", "beta")]),
            }),
        ),
    ];

    for (index, (case, body)) in rows.iter().enumerate() {
        // One definition per row: a definition serves one run at a time.
        let definition = create_test_definition(&router, &scope, &format!("members-{index}")).await;
        let definition_id = id_of(case, &definition);
        let created = start_run(&router, &scope.application, &definition_id, body).await;
        let started = assert_running(case, &created, 1, &definition);

        // Each member the request carried is reported as written; one it did not carry is
        // not invented — except the audience exposure, which a run always delivers to, so a
        // start that named none reports the zero exposure it is running at.
        for member in ["Description", "TreatmentOverrides"] {
            assert_eq!(
                started.get(member),
                body.get(member),
                "case {case:?}: expected {member} {:?}, got {:?}",
                body.get(member),
                started.get(member)
            );
        }
        let expected_exposure = body
            .get("ExposurePercentage")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        let reported_exposure = started.get("ExposurePercentage").and_then(Value::as_f64);
        assert!(
            reported_exposure.is_some_and(|got| (got - expected_exposure).abs() < 1e-6),
            "case {case:?}: expected ExposurePercentage {expected_exposure}, got {:?}",
            started.get("ExposurePercentage")
        );
        assert_reads_back(&router, case, &scope.application, &definition_id, &started).await;
    }
}

/// An identifier or a run number that addresses nothing is HTTP 404, and a run number
/// outside the modelled range is refused before any handler runs.
#[tokio::test]
async fn a_start_or_a_read_that_addresses_nothing_is_not_found() {
    let router = router();
    let scope = scope(&router, "misses").await;
    let other_application = create_application(&router, "app-misses-other").await;
    let definition = create_test_definition(&router, &scope, "misses").await;
    let definition_id = id_of("misses", &definition);

    // Rows differ along WHICH level of the path resolves to nothing.
    for (case, application, addressed) in [
        (
            "an application no resource answers to",
            "nosuchapp".to_string(),
            definition_id.clone(),
        ),
        (
            "a definition name nothing holds",
            scope.application.clone(),
            "no-such-definition".to_string(),
        ),
        (
            "a definition addressed under the wrong application",
            other_application.clone(),
            definition_id.clone(),
        ),
    ] {
        let refused = start_run(&router, &application, &addressed, &json!({})).await;
        assert_refused(case, &refused, 404, "ResourceNotFoundException");
        let read = get_run(&router, &application, &addressed, 1).await;
        assert_refused(case, &read, 404, "ResourceNotFoundException");
    }

    // A definition that exists but has never been started holds no run at all, and neither
    // the first number nor a far one addresses anything.
    for (case, run) in [
        ("the first run number before any start", 1_i64),
        ("a run number never issued", 7),
        ("the largest run number the model's Integer holds", 2_147_483_647),
    ] {
        let read = get_run(&router, &scope.application, &definition_id, run).await;
        assert_refused(case, &read, 404, "ResourceNotFoundException");
    }

    // `Run` is a `PositiveInteger` (range 1..), so one below that bound is refused as a
    // bad request rather than looked up.
    let case = "a run number below the modelled range";
    let refused = get_run(&router, &scope.application, &definition_id, 0).await;
    assert_eq!(
        refused.status,
        400,
        "case {case:?}: expected 400, got {} / {}",
        refused.status,
        String::from_utf8_lossy(&refused.body)
    );

    // One started run, and every refusal above left it the only one.
    let created = start_run(&router, &scope.application, &definition_id, &json!({})).await;
    let started = assert_running("after the misses", &created, 1, &definition);
    assert_reads_back(
        &router,
        "after the misses",
        &scope.application,
        &definition_id,
        &started,
    )
    .await;
}

/// `count` inline treatment overrides, each assigning one entity to a treatment key the
/// definition under test holds (`TreatmentOverrideMap`: keys `EntityId` len 1..2048,
/// values `TreatmentKey` `^[a-zA-Z0-9]{1,8}$`).
pub(crate) fn inline_overrides(count: usize) -> Value {
    Value::Object(
        (0..count)
            .map(|index| {
                (
                    format!("entity-{index}"),
                    Value::from(if index % 2 == 0 { "T1" } else { "C" }),
                )
            })
            .collect::<Map<String, Value>>(),
    )
}

/// `count` tags, each pair well within `TagKey` len 1..128 and `TagValue` len 0..256, so a
/// row that spans the `TagMap` length bound varies the COUNT alone.
fn tag_map(count: usize) -> Value {
    Value::Object(
        (0..count)
            .map(|index| (format!("key-{index}"), Value::from(format!("value-{index}"))))
            .collect::<Map<String, Value>>(),
    )
}

/// One pair of a string-keyed map, for a key a literal cannot spell.
pub(crate) fn one_pair(key: &str, value: &str) -> Value {
    Value::Object(
        [(key.to_string(), Value::from(value))]
            .into_iter()
            .collect::<Map<String, Value>>(),
    )
}

/// One row of the bounds table: a start that breaks exactly one constraint the model
/// declares, and the clause the refusal must state for it.
struct PastTheBound {
    case: String,
    body: Value,
    /// The dotted member path and the requirement the message must name, both as the
    /// framework's one validation envelope spells them. `None` where the member's own rule
    /// owns the wording (a tag key, refused by the rule every tagging path shares).
    clause: Option<(&'static str, String)>,
}

/// A start carrying a value outside a published modelled bound is refused HTTP 400
/// `BadRequestException`, states AWS's validation envelope for the member that broke, and
/// writes nothing — while the very same members AT their bounds are accepted, so each
/// refusal differs from an accepted start by one character or one map entry alone.
#[tokio::test]
async fn a_start_past_a_modelled_bound_is_refused_without_writing() {
    let router = router();
    let scope = scope(&router, "bounds").await;
    let definition = create_test_definition(&router, &scope, "bounds").await;
    let definition_id = id_of("bounds", &definition);
    let baseline = get_definition(&router, &scope.application, &definition_id)
        .await
        .json();

    // Rows are one step past each bound `StartExperimentRun`'s input publishes
    // (`smithy_query(query="operation", operation="StartExperimentRun")` and
    // `smithy_query(query="shape", shape="TreatmentOverrideMap")`): `Description` len
    // 0..1024, `ExposurePercentage` range 0..100, the inline `TreatmentOverrides` map len
    // 0..32 with `EntityId` keys of len 1..2048 and `TreatmentKey` values matching
    // `^[a-zA-Z0-9]{1,8}$`, and `Tags` len 0..50 with `TagKey` keys of len 1..128. The
    // bound inside the union and the ones outside it are refused alike: same status, same
    // code, same envelope.
    let rows: Vec<PastTheBound> = vec![
        PastTheBound {
            case: "a description one character past its 1024 upper bound".to_string(),
            body: json!({ "Description": filler(1025, 1) }),
            clause: Some((
                "description",
                "Member must have length less than or equal to 1024".to_string(),
            )),
        },
        PastTheBound {
            case: "an exposure past the upper end of its 0..100 range".to_string(),
            body: json!({ "ExposurePercentage": 100.5 }),
            clause: Some((
                "exposurePercentage",
                "Member must have value less than or equal to 100".to_string(),
            )),
        },
        PastTheBound {
            case: "an exposure below the lower end of its 0..100 range".to_string(),
            body: json!({ "ExposurePercentage": -0.5 }),
            clause: Some((
                "exposurePercentage",
                "Member must have value greater than or equal to 0".to_string(),
            )),
        },
        PastTheBound {
            case: "thirty-three inline overrides, one past the maximum of 32".to_string(),
            body: json!({ "TreatmentOverrides": { "Inline": inline_overrides(33) } }),
            clause: Some((
                "treatmentOverrides.inline",
                // The numbers are the MODEL's, spelled out: an expected clause built from
                // the implementation's own constants would assert a wrong bound as right.
                "Member must have length less than or equal to 32".to_string(),
            )),
        },
        PastTheBound {
            case: "an override whose entity id is below its 1-character bound".to_string(),
            body: json!({ "TreatmentOverrides": { "Inline": one_pair("", "T1") } }),
            clause: Some((
                "treatmentOverrides.inline",
                "Member must have length greater than or equal to 1".to_string(),
            )),
        },
        PastTheBound {
            case: "an override whose entity id is one character past its 2048 bound".to_string(),
            body: json!({ "TreatmentOverrides": { "Inline": one_pair(&filler(2049, 3), "T1") } }),
            clause: Some((
                "treatmentOverrides.inline",
                "Member must have length less than or equal to 2048".to_string(),
            )),
        },
        PastTheBound {
            case: "an override naming a treatment key one character past the pattern".to_string(),
            body: json!({ "TreatmentOverrides": { "Inline": one_pair("entity-1", "T12345678") } }),
            clause: Some((
                "treatmentOverrides.inline",
                // The pattern as this service quotes one: the model's expression without
                // the end anchor, which is how every other pattern refusal here reads.
                "Member must satisfy regular expression pattern: ^[a-zA-Z0-9]{1,8}".to_string(),
            )),
        },
        PastTheBound {
            case: "an override naming a treatment key outside the pattern's character set"
                .to_string(),
            body: json!({ "TreatmentOverrides": { "Inline": one_pair("entity-1", "T-1") } }),
            clause: Some((
                "treatmentOverrides.inline",
                "Member must satisfy regular expression pattern: ^[a-zA-Z0-9]{1,8}".to_string(),
            )),
        },
        PastTheBound {
            case: "fifty-one tags, one past the maximum of 50".to_string(),
            body: json!({ "Tags": tag_map(51) }),
            clause: Some((
                "tags",
                "Member must have length less than or equal to 50".to_string(),
            )),
        },
        PastTheBound {
            case: "a tag key one character past its 128 upper bound".to_string(),
            body: json!({ "Tags": one_pair(&filler(129, 2), "past the key bound") }),
            clause: None,
        },
    ];

    for row in &rows {
        let case = &row.case;
        let refused = start_run(&router, &scope.application, &definition_id, &row.body).await;
        assert_refused(case, &refused, 400, "BadRequestException");
        if let Some((member, requirement)) = &row.clause {
            let message = refused
                .string("Message")
                .or_else(|| refused.string("message"))
                .unwrap_or_else(|| panic!("case {case:?}: no message in {}", refused.json()));
            assert!(
                message.starts_with("1 validation error detected: "),
                "case {case:?}: expected AWS's validation envelope, got {message:?}"
            );
            assert!(
                message.contains(&format!("at '{member}")),
                "case {case:?}: expected the message to name {member:?}, got {message:?}"
            );
            assert!(
                message.contains(requirement.as_str()),
                "case {case:?}: expected the message to state {requirement:?}, got {message:?}"
            );
        }
        // Nothing was written: the definition holds no run at all, and it still reports
        // the record its create stored.
        let absent = get_run(&router, &scope.application, &definition_id, 1).await;
        assert_refused(case, &absent, 404, "ResourceNotFoundException");
        let read = get_definition(&router, &scope.application, &definition_id).await;
        assert_eq!(
            read.json(),
            baseline,
            "case {case:?}: expected the definition to still be {baseline:?}, got {:?}",
            read.json()
        );
    }

    // The bounds themselves are inside: the same members AT their published maxima start a
    // run, so each refusal above differs from an accepted start by one character or one map
    // entry alone. One definition per row, since a definition serves one run at a time.
    let accepted_rows: Vec<(&str, Value)> = vec![
        (
            "every bounded member at its published maximum",
            json!({
                "Description": filler(1024, 1),
                "ExposurePercentage": 100.0,
                "TreatmentOverrides": { "Inline": inline_overrides(32) },
                "Tags": tag_map(50),
            }),
        ),
        (
            "an override whose entity id is at its 2048 upper bound",
            json!({ "TreatmentOverrides": { "Inline": one_pair(&filler(2048, 3), "T1") } }),
        ),
        (
            "an override whose entity id is at its 1-character lower bound",
            json!({ "TreatmentOverrides": { "Inline": one_pair("e", "C") } }),
        ),
        (
            "a tag key at its 128 upper bound",
            json!({ "Tags": one_pair(&filler(128, 2), "at the key bound") }),
        ),
    ];
    for (index, (case, body)) in accepted_rows.iter().enumerate() {
        let accepted_definition =
            create_test_definition(&router, &scope, &format!("at-the-bound-{index}")).await;
        let accepted_id = id_of(case, &accepted_definition);
        let accepted = start_run(&router, &scope.application, &accepted_id, body).await;
        let started = assert_running(case, &accepted, 1, &accepted_definition);
        assert_reads_back(&router, case, &scope.application, &accepted_id, &started).await;
    }
}

/// A run document reports the members its output shape models and no others: the `Tags` and
/// `DeploymentParameters` a start accepts have no member in `ExperimentRun`, and the
/// instant it was last written is the instant it was started while nothing has written the
/// record since.
#[tokio::test]
async fn a_started_run_reports_only_the_members_its_output_shape_models() {
    let router = router();
    let scope = scope(&router, "members-modelled").await;

    // Rows differ along which INPUT-only members the start carries: none of them adds a
    // member to the document, and a start that carries them is still accepted.
    let rows: Vec<(&str, Value)> = vec![
        ("nothing optional", json!({})),
        (
            "tags, which the run document has no member for",
            json!({ "Tags": tag_map(3) }),
        ),
        (
            "deployment parameters, which the run document has no member for",
            json!({
                "DeploymentParameters": {
                    "DynamicExtensionParameters": { "S3 bucket#name": "experiment-metrics" },
                    "Tags": { "owner": "experiments" },
                },
            }),
        ),
        (
            "every input-only member at once",
            json!({
                "Description": "fully specified",
                "ExposurePercentage": 2.5,
                "TreatmentOverrides": { "Inline": inline_overrides(2) },
                "Tags": tag_map(2),
                "DeploymentParameters": {
                    "DynamicExtensionParameters": { "S3 bucket#name": "experiment-metrics" },
                },
            }),
        ),
    ];

    for (index, (case, body)) in rows.iter().enumerate() {
        // One definition per row: a definition serves one run at a time.
        let definition =
            create_test_definition(&router, &scope, &format!("modelled-{index}")).await;
        let definition_id = id_of(case, &definition);
        let created = start_run(&router, &scope.application, &definition_id, body).await;
        let started = assert_running(case, &created, 1, &definition);

        let document = started
            .as_object()
            .unwrap_or_else(|| panic!("case {case:?}: expected a JSON object, got {started:?}"));
        for member in document.keys() {
            assert!(
                EXPERIMENT_RUN_MEMBERS.contains(&member.as_str()),
                "case {case:?}: {member} is not a member of the run's output shape, which \
                 holds {EXPERIMENT_RUN_MEMBERS:?}"
            );
        }
        for absent in ["Tags", "DeploymentParameters"] {
            assert_eq!(
                document.get(absent),
                None,
                "case {case:?}: {absent} is an input member only, got {:?}",
                document.get(absent)
            );
        }
        assert!(
            document
                .get("ApplicationId")
                .and_then(Value::as_str)
                .is_some_and(is_modelled_id),
            "case {case:?}: expected a modelled application id, got {:?}",
            document.get("ApplicationId")
        );
        // Nothing has written the record since the start created it, so the instant it was
        // last written is the instant it was started.
        assert_eq!(
            document.get("StartedAt"),
            document.get("UpdatedAt"),
            "case {case:?}: expected UpdatedAt to be StartedAt, got {:?} and {:?}",
            document.get("StartedAt"),
            document.get("UpdatedAt")
        );
        assert!(
            document.get("StartedAt").is_some(),
            "case {case:?}: expected a StartedAt, got {started:?}"
        );
    }
}

/// Reading a run REPORTS it: however many times it is read, and whatever refusals arrive
/// in between, every reader sees the document the start wrote and the definition the start
/// left.
#[tokio::test]
async fn reading_a_run_does_not_change_what_a_later_reader_sees() {
    let router = router();
    let scope = scope(&router, "reports").await;
    let definition = create_test_definition(&router, &scope, "reports").await;
    let definition_id = id_of("reports", &definition);

    let created = start_run(
        &router,
        &scope.application,
        &definition_id,
        &json!({ "Description": "read many times", "ExposurePercentage": 7.5 }),
    )
    .await;
    let started = assert_running("the run under test", &created, 1, &definition);
    let definition_after_start = get_definition(&router, &scope.application, &definition_id)
        .await
        .json();

    // Rows differ along HOW the run is addressed between reads and what arrives in
    // between: a plain re-read, a re-read by the definition's name, a read that follows a
    // refused duplicate start, and one that follows a read that found nothing.
    for (case, interleaved) in [
        ("two reads in a row", None),
        (
            "a read after a refused duplicate start",
            Some(json!({ "Description": "must not be stored" })),
        ),
        (
            "a read after a refused duplicate start carrying every optional member",
            Some(json!({
                "Description": "must not be stored either",
                "ExposurePercentage": 99.0,
                "TreatmentOverrides": { "Inline": inline_overrides(4) },
                "Tags": tag_map(4),
            })),
        ),
    ] {
        if let Some(body) = interleaved {
            let refused = start_run(&router, &scope.application, &definition_id, &body).await;
            assert_refused(case, &refused, 409, "ConflictException");
        }
        // Several reads, by id and by the definition's name, all of the same record.
        for addressed in [definition_id.as_str(), "reports"] {
            for round in 0..3 {
                let read = get_run(&router, &scope.application, addressed, 1).await;
                assert_eq!(
                    read.status,
                    200,
                    "case {case:?} (round {round}, addressed {addressed:?}): expected 200, got \
                     {} / {}",
                    read.status,
                    String::from_utf8_lossy(&read.body)
                );
                assert_eq!(
                    read.json(),
                    started,
                    "case {case:?} (round {round}, addressed {addressed:?}): expected the run to \
                     still be {started:?}, got {:?}",
                    read.json()
                );
            }
            // A read of a number nothing issued does not create one either.
            let absent = get_run(&router, &scope.application, addressed, 2).await;
            assert_refused(case, &absent, 404, "ResourceNotFoundException");
        }
        // And the definition is exactly as the start left it: still ACTIVE, still written
        // at the instant the start wrote it.
        let read_definition = get_definition(&router, &scope.application, &definition_id).await;
        assert_eq!(
            read_definition.json(),
            definition_after_start,
            "case {case:?}: expected the definition to still be {definition_after_start:?}, got \
             {:?}",
            read_definition.json()
        );
    }
}
