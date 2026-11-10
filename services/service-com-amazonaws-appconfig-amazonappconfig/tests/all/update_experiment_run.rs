// SPDX-License-Identifier: CC0-1.0

//! `UpdateExperimentRun`, and the read that proves what it wrote — `GetExperimentRun` —
//! end to end over HTTP.
//!
//! The rules under test:
//!
//! * an update is a PATCH over the stored run: a member the request carries replaces the
//!   stored one, a member it omits keeps the value the run holds, and the members a run
//!   does not take from this request (`Run`, `Status`, `StartedAt`, `EndedAt`, `Result`,
//!   the definition snapshot) are reported back untouched, while the record's
//!   last-written instant moves to the write;
//! * audience exposure can only be increased: an `ExposurePercentage` below the one the
//!   run is already delivering is refused HTTP 400 `BadRequestException` and writes
//!   nothing, while one equal to it or above it is written;
//! * what an update writes is what a later read reports, so `GetExperimentRun` answers
//!   exactly the document the update answered;
//! * every tag key this service will not store is refused whichever member carried it, so
//!   a `DeploymentParameters$Tags` key under the reserved prefix, an empty one or one past
//!   the published length bound is HTTP 400 and writes nothing;
//! * an inline treatment-override map past a bound the model publishes for it — its entry
//!   count, an entity id's length, a treatment key's shape — is refused HTTP 400
//!   `BadRequestException` in AWS's validation envelope and writes nothing, exactly as the
//!   start of a run refuses it;
//! * an identifier or a run number that addresses nothing is HTTP 404
//!   `ResourceNotFoundException`, and a run number outside the modelled range is refused
//!   before any handler runs.
//!
//! The tables span what those rules generalise over rather than the one recorded request:
//! WHICH members the patch carries (each alone, all together, and none at all), their
//! values at the published bounds (`Description` len 0..1024, `NullablePercentage` range
//! 0..100, the inline `TreatmentOverrides` map from empty upwards, from
//! `smithy_query(query="operation", operation="UpdateExperimentRun")`), whether the run
//! already holds the member being patched, how far an exposure moves and in which
//! direction, which tag key is unacceptable and why, and WHICH part of the path —
//! application, definition or run number — resolves to nothing.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::appconfig_workflow::{create_application, json_call, router, Answer};
use crate::experiment_definitions::{scope, Scope};
use crate::resource_tags::{filler, pct};
use crate::start_experiment_run::{
    assert_reads_back, assert_refused, assert_running, create_test_definition, get_run, id_of,
    inline_overrides, one_pair, start_run,
};
use crate::update_experiment_definition::instant;

/// The members an update never rewrites: which run of which definition this is, what it is
/// doing, when it started, what it concluded, and the definition it was started from.
const FIXED_MEMBERS: &[&str] = &[
    "ApplicationId",
    "ExperimentDefinitionId",
    "Run",
    "Status",
    "StartedAt",
    "Result",
    "EndedAt",
    "ExperimentDefinitionSnapshot",
];

/// The members this request patches.
const PATCHED_MEMBERS: &[&str] = &["Description", "ExposurePercentage", "TreatmentOverrides"];

/// `PATCH /applications/{ApplicationIdentifier}/experimentdefinitions/{ExperimentDefinitionIdentifier}/experimentruns/{Run}/update`.
/// `pub(crate)` because more than one suite in this target updates a run.
pub(crate) async fn update_run(
    router: &MultiServiceRouter,
    application: &str,
    definition: &str,
    run: i64,
    body: &Value,
) -> Answer {
    json_call(
        router,
        "PATCH",
        &format!(
            "/applications/{}/experimentdefinitions/{}/experimentruns/{run}/update",
            pct(application),
            pct(definition)
        ),
        body,
    )
    .await
}

/// A fresh definition with one run started on it from `start`, answering the definition's
/// id and the run document the start wrote.
async fn running_run(
    router: &MultiServiceRouter,
    scope: &Scope,
    label: &str,
    start: &Value,
) -> (String, Value) {
    let definition = create_test_definition(router, scope, label).await;
    let definition_id = id_of(label, &definition);
    let created = start_run(router, &scope.application, &definition_id, start).await;
    let started = assert_running(label, &created, 1, &definition);
    (definition_id, started)
}

/// Assert one member of an answered run, comparing numbers by value so the wire form of a
/// percentage (`25` or `25.0`) does not decide the case.
fn assert_member(case: &str, body: &Value, member: &str, expected: Option<&Value>) {
    let actual = body.get(member);
    match (expected, actual) {
        (Some(Value::Number(want)), Some(Value::Number(got))) => {
            let want = want.as_f64().unwrap_or_default();
            let got = got.as_f64().unwrap_or_default();
            assert!(
                (want - got).abs() < 1e-6,
                "case {case:?}: expected {member} {want}, got {got}"
            );
        }
        _ => assert_eq!(
            actual, expected,
            "case {case:?}: expected {member} {expected:?}, got {actual:?}"
        ),
    }
}

/// Assert that `body` reports every member the update does not write exactly as `before`
/// did.
fn assert_fixed_members(case: &str, before: &Value, body: &Value) {
    for member in FIXED_MEMBERS {
        assert_eq!(
            body.get(member),
            before.get(member),
            "case {case:?}: expected {member} to stay {:?}, got {:?}",
            before.get(member),
            body.get(member)
        );
    }
}

/// One row of the patch table: what the run was started with, what the update carries, and
/// the patched members the stored run must then report.
struct Patch {
    case: &'static str,
    start: Value,
    patch: Value,
    description: Option<Value>,
    exposure: Option<Value>,
    overrides: Option<Value>,
}

/// An update writes the members the request carries, keeps every other member of the run,
/// moves the record's last-written instant, and what it leaves behind is what
/// `GetExperimentRun` answers.
#[tokio::test]
async fn an_update_patches_only_the_members_the_request_carries() {
    let router = router();
    let scope = scope(&router, "run-patch").await;

    // A started run carrying all three patchable members, so a row can patch one of them
    // and prove the other two were left alone.
    let full_start = json!({
        "Description": "as started",
        "ExposurePercentage": 25.0,
        "TreatmentOverrides": { "Inline": { "entity-1": "t1", "entity-2": "c" } },
    });
    // `Description` is len 0..1024: the longest text the member takes.
    let longest_description = filler(1024, 9);

    let rows: Vec<Patch> = vec![
        Patch {
            case: "an update carrying nothing keeps every stored member",
            start: full_start.clone(),
            patch: json!({}),
            description: Some(json!("as started")),
            exposure: Some(json!(25.0)),
            overrides: Some(json!({ "Inline": { "entity-1": "t1", "entity-2": "c" } })),
        },
        Patch {
            case: "a description alone replaces only the description",
            start: full_start.clone(),
            patch: json!({ "Description": "revised while running" }),
            description: Some(json!("revised while running")),
            exposure: Some(json!(25.0)),
            overrides: Some(json!({ "Inline": { "entity-1": "t1", "entity-2": "c" } })),
        },
        Patch {
            case: "a description at the lower length bound is a value, not an omission",
            start: full_start.clone(),
            patch: json!({ "Description": "" }),
            description: Some(json!("")),
            exposure: Some(json!(25.0)),
            overrides: Some(json!({ "Inline": { "entity-1": "t1", "entity-2": "c" } })),
        },
        Patch {
            case: "a description at the upper length bound",
            start: full_start.clone(),
            patch: json!({ "Description": longest_description.clone() }),
            description: Some(json!(longest_description.clone())),
            exposure: Some(json!(25.0)),
            overrides: Some(json!({ "Inline": { "entity-1": "t1", "entity-2": "c" } })),
        },
        Patch {
            case: "a description on a run that was started without one",
            start: json!({}),
            patch: json!({ "Description": "named after the fact" }),
            description: Some(json!("named after the fact")),
            // A run started without an exposure delivers to nobody, and reports that zero
            // share, so a patch that does not touch it keeps it.
            exposure: Some(json!(0.0)),
            overrides: None,
        },
        Patch {
            case: "an exposure increased from the stored one",
            start: full_start.clone(),
            patch: json!({ "ExposurePercentage": 37.5 }),
            description: Some(json!("as started")),
            exposure: Some(json!(37.5)),
            overrides: Some(json!({ "Inline": { "entity-1": "t1", "entity-2": "c" } })),
        },
        Patch {
            case: "an exposure increased to the upper range bound",
            start: full_start.clone(),
            patch: json!({ "ExposurePercentage": 100.0 }),
            description: Some(json!("as started")),
            exposure: Some(json!(100.0)),
            overrides: Some(json!({ "Inline": { "entity-1": "t1", "entity-2": "c" } })),
        },
        Patch {
            case: "an exposure at the lower range bound on a run started without one",
            start: json!({}),
            patch: json!({ "ExposurePercentage": 0.0 }),
            description: None,
            exposure: Some(json!(0.0)),
            overrides: None,
        },
        Patch {
            case: "overrides replace the stored assignment map rather than merging into it",
            start: full_start.clone(),
            patch: json!({ "TreatmentOverrides": { "Inline": { "entity-9": "t1" } } }),
            description: Some(json!("as started")),
            exposure: Some(json!(25.0)),
            overrides: Some(json!({ "Inline": { "entity-9": "t1" } })),
        },
        Patch {
            case: "an empty inline override map clears the stored assignments",
            start: full_start.clone(),
            patch: json!({ "TreatmentOverrides": { "Inline": {} } }),
            description: Some(json!("as started")),
            exposure: Some(json!(25.0)),
            overrides: Some(json!({ "Inline": {} })),
        },
        Patch {
            case: "overrides on a run that was started without any",
            start: json!({ "ExposurePercentage": 12.5 }),
            patch: json!({ "TreatmentOverrides": { "Inline": {
                "entity-1": "c",
                "entity-2": "t1",
                "entity-3": "t1",
            } } }),
            description: None,
            exposure: Some(json!(12.5)),
            overrides: Some(json!({ "Inline": {
                "entity-1": "c",
                "entity-2": "t1",
                "entity-3": "t1",
            } })),
        },
        Patch {
            case: "every patchable member at once",
            start: full_start.clone(),
            patch: json!({
                "Description": "wider audience",
                "ExposurePercentage": 50.0,
                "TreatmentOverrides": { "Inline": { "entity-7": "c" } },
            }),
            description: Some(json!("wider audience")),
            exposure: Some(json!(50.0)),
            overrides: Some(json!({ "Inline": { "entity-7": "c" } })),
        },
        Patch {
            case: "deployment parameters alongside a patched member",
            start: full_start.clone(),
            patch: json!({
                "Description": "rolled wider",
                "DeploymentParameters": {
                    "Tags": { "owner": "experiments" },
                    "DynamicExtensionParameters": { "S3 backup#bucket": "experiment-logs" },
                },
            }),
            description: Some(json!("rolled wider")),
            exposure: Some(json!(25.0)),
            overrides: Some(json!({ "Inline": { "entity-1": "t1", "entity-2": "c" } })),
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        // One definition per row: a definition delivers one experiment at a time.
        let (definition_id, started) =
            running_run(&router, &scope, &format!("run-patch-{index}"), &row.start).await;
        // Far enough apart for the two write instants to be distinguishable in the
        // reported timestamp.
        std::thread::sleep(std::time::Duration::from_millis(10));

        let answered = update_run(&router, &scope.application, &definition_id, 1, &row.patch).await;
        assert_eq!(
            answered.status,
            200,
            "case {case:?}: expected 200, got {} / {}",
            answered.status,
            String::from_utf8_lossy(&answered.body)
        );
        let updated = answered.json();

        assert_member(case, &updated, "Description", row.description.as_ref());
        assert_member(case, &updated, "ExposurePercentage", row.exposure.as_ref());
        assert_member(case, &updated, "TreatmentOverrides", row.overrides.as_ref());
        assert_fixed_members(case, &started, &updated);

        // The update wrote the record, so it was last written at the update; the run
        // started before that and `StartedAt` does not move (asserted above).
        let started_at = instant(case, "StartedAt", updated.get("StartedAt"));
        let previously_updated = instant(case, "UpdatedAt", started.get("UpdatedAt"));
        let updated_at = instant(case, "UpdatedAt", updated.get("UpdatedAt"));
        assert!(
            updated_at > previously_updated,
            "case {case:?}: expected UpdatedAt past {previously_updated}, got {updated_at}"
        );
        assert!(
            updated_at >= started_at,
            "case {case:?}: expected UpdatedAt at or after StartedAt {started_at}, got \
             {updated_at}"
        );

        // What the update wrote is what the read reports, member for member.
        assert_reads_back(&router, case, &scope.application, &definition_id, &updated).await;
    }
}

/// Audience exposure can only be increased: a lower `ExposurePercentage` is refused and
/// writes nothing, while one equal to or above the stored exposure is written.
#[tokio::test]
async fn an_update_cannot_lower_the_audience_a_run_already_exposes() {
    let router = router();
    let scope = scope(&router, "run-exposure").await;

    // Rows differ along the stored exposure, how far the request moves it and in which
    // direction, including both published bounds of `NullablePercentage` (range 0..100)
    // and a run started with no exposure at all, which delivers to nobody.
    let rows: Vec<(&str, Value, f64, bool)> = vec![
        (
            "a fraction below the stored exposure",
            json!({ "ExposurePercentage": 50.0 }),
            49.5,
            false,
        ),
        (
            "down to the lower range bound",
            json!({ "ExposurePercentage": 50.0 }),
            0.0,
            false,
        ),
        (
            "halved from the upper range bound",
            json!({ "ExposurePercentage": 100.0 }),
            50.0,
            false,
        ),
        (
            "a fraction below a fractional exposure",
            json!({ "ExposurePercentage": 0.5 }),
            0.25,
            false,
        ),
        (
            "equal to the stored exposure, which lowers nothing",
            json!({ "ExposurePercentage": 50.0 }),
            50.0,
            true,
        ),
        (
            "equal to a stored exposure of zero",
            json!({ "ExposurePercentage": 0.0 }),
            0.0,
            true,
        ),
        (
            "up from the lower range bound to the upper one",
            json!({ "ExposurePercentage": 0.0 }),
            100.0,
            true,
        ),
        (
            "any exposure on a run started without one, which exposes nobody",
            json!({ "Description": "no exposure yet" }),
            0.0,
            true,
        ),
    ];

    for (index, (case, start, requested, accepted)) in rows.iter().enumerate() {
        let (definition_id, started) =
            running_run(&router, &scope, &format!("run-exposure-{index}"), start).await;
        let answered = update_run(
            &router,
            &scope.application,
            &definition_id,
            1,
            &json!({ "ExposurePercentage": requested }),
        )
        .await;

        if *accepted {
            assert_eq!(
                answered.status,
                200,
                "case {case:?}: expected 200, got {} / {}",
                answered.status,
                String::from_utf8_lossy(&answered.body)
            );
            let updated = answered.json();
            assert_member(
                case,
                &updated,
                "ExposurePercentage",
                Some(&json!(requested)),
            );
            assert_reads_back(&router, case, &scope.application, &definition_id, &updated).await;
        } else {
            assert_refused(case, &answered, 400, "BadRequestException");
            // The refusal wrote nothing: the run is still the one the start left behind,
            // down to the instant it was last written.
            let read = get_run(&router, &scope.application, &definition_id, 1).await;
            assert_eq!(
                read.json(),
                started,
                "case {case:?}: expected the refused update to leave {started:?}, got {:?}",
                read.json()
            );
        }
    }
}

/// A tag key this service will not store is refused whichever member carried it, and the
/// refusal writes nothing; a key it does store is accepted.
#[tokio::test]
async fn an_update_whose_deployment_tags_name_an_unacceptable_key_is_refused() {
    let router = router();
    let scope = scope(&router, "run-deployment-tags").await;

    // A tag key is `length 1..128` and must not start with `aws:` (the `Tags` member
    // documentation), so the rows run along both bounds and both sides of the prefix rule.
    let longest_key = filler(128, 3);
    let too_long_key = filler(129, 4);
    let rows: Vec<(&str, String, bool)> = vec![
        (
            "a key under the reserved prefix",
            "aws:owner".to_string(),
            false,
        ),
        ("an empty key", String::new(), false),
        (
            "a key one character past the upper length bound",
            too_long_key,
            false,
        ),
        (
            "a key at the upper length bound",
            longest_key,
            true,
        ),
        (
            "a key that merely contains the reserved prefix later on",
            "team-aws:billing".to_string(),
            true,
        ),
    ];

    for (index, (case, key, accepted)) in rows.iter().enumerate() {
        let (definition_id, started) = running_run(
            &router,
            &scope,
            &format!("run-deployment-tags-{index}"),
            &json!({ "Description": "as started" }),
        )
        .await;
        let patch = json!({
            "Description": "patched alongside the deployment tags",
            "DeploymentParameters": { "Tags": { key.clone(): "experiments" } },
        });
        let answered = update_run(&router, &scope.application, &definition_id, 1, &patch).await;

        if *accepted {
            assert_eq!(
                answered.status,
                200,
                "case {case:?}: expected 200, got {} / {}",
                answered.status,
                String::from_utf8_lossy(&answered.body)
            );
            let updated = answered.json();
            assert_member(
                case,
                &updated,
                "Description",
                Some(&json!("patched alongside the deployment tags")),
            );
        } else {
            assert_refused(case, &answered, 400, "BadRequestException");
            let read = get_run(&router, &scope.application, &definition_id, 1).await;
            assert_eq!(
                read.json(),
                started,
                "case {case:?}: expected the refused update to leave {started:?}, got {:?}",
                read.json()
            );
        }
    }
}

/// An update whose inline treatment overrides break a bound the model publishes for them is
/// refused HTTP 400 `BadRequestException`, states AWS's validation envelope for that member
/// and writes nothing — the same rule, and the same refusal, `StartExperimentRun` applies,
/// because the map sits behind a union member the schema walk does not descend into.
#[tokio::test]
async fn an_update_whose_overrides_break_a_modelled_bound_is_refused_without_writing() {
    let router = router();
    let scope = scope(&router, "run-override-bounds").await;

    // Rows are one step past each bound the inline override map publishes
    // (`smithy_query(query="shape", shape="TreatmentOverrideMap")`: `length 0..32`, keys
    // `EntityId` len 1..2048, values `TreatmentKey` `^[a-zA-Z0-9]{1,8}$`), each paired with
    // the requirement the refusal must state; the last two rows are AT those bounds and are
    // written, so an accepted and a refused patch differ by one character or one entry.
    let rows: Vec<(&str, Value, Option<&str>)> = vec![
        (
            "thirty-three inline overrides, one past the maximum of 32",
            json!({ "TreatmentOverrides": { "Inline": inline_overrides(33) } }),
            Some("Member must have length less than or equal to 32"),
        ),
        (
            "an override whose entity id is below its 1-character bound",
            json!({ "TreatmentOverrides": { "Inline": one_pair("", "t1") } }),
            Some("Member must have length greater than or equal to 1"),
        ),
        (
            "an override whose entity id is one character past its 2048 bound",
            json!({ "TreatmentOverrides": { "Inline": one_pair(&filler(2049, 5), "t1") } }),
            Some("Member must have length less than or equal to 2048"),
        ),
        (
            "an override naming a treatment key one character past the pattern",
            json!({ "TreatmentOverrides": { "Inline": one_pair("entity-1", "T12345678") } }),
            Some("Member must satisfy regular expression pattern: ^[a-zA-Z0-9]{1,8}"),
        ),
        (
            "an override naming a treatment key outside the pattern's character set",
            json!({ "TreatmentOverrides": { "Inline": one_pair("entity-1", "T-1") } }),
            Some("Member must satisfy regular expression pattern: ^[a-zA-Z0-9]{1,8}"),
        ),
        (
            "thirty-two inline overrides, the published maximum",
            json!({ "TreatmentOverrides": { "Inline": inline_overrides(32) } }),
            None,
        ),
        (
            "an override whose entity id is at its 2048 upper bound",
            json!({ "TreatmentOverrides": { "Inline": one_pair(&filler(2048, 5), "c") } }),
            None,
        ),
    ];

    for (index, (case, patch, refused)) in rows.iter().enumerate() {
        let (definition_id, started) = running_run(
            &router,
            &scope,
            &format!("run-override-bounds-{index}"),
            &json!({ "Description": "as started" }),
        )
        .await;
        let answered = update_run(&router, &scope.application, &definition_id, 1, patch).await;

        match refused {
            Some(requirement) => {
                assert_refused(case, &answered, 400, "BadRequestException");
                let message = answered
                    .string("Message")
                    .or_else(|| answered.string("message"))
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: no message in {}", answered.json())
                    });
                assert!(
                    message.starts_with("1 validation error detected: "),
                    "case {case:?}: expected AWS's validation envelope, got {message:?}"
                );
                assert!(
                    message.contains("at 'treatmentOverrides.inline"),
                    "case {case:?}: expected the message to name the member, got {message:?}"
                );
                assert!(
                    message.contains(requirement),
                    "case {case:?}: expected the message to state {requirement:?}, got \
                     {message:?}"
                );
                // The refused update left the run exactly as the start wrote it.
                let read = get_run(&router, &scope.application, &definition_id, 1).await;
                assert_eq!(
                    read.json(),
                    started,
                    "case {case:?}: expected the refused update to leave {started:?}, got {:?}",
                    read.json()
                );
            }
            None => {
                assert_eq!(
                    answered.status,
                    200,
                    "case {case:?}: expected 200, got {} / {}",
                    answered.status,
                    String::from_utf8_lossy(&answered.body)
                );
                let updated = answered.json();
                assert_member(
                    case,
                    &updated,
                    "TreatmentOverrides",
                    patch.get("TreatmentOverrides"),
                );
                let read = get_run(&router, &scope.application, &definition_id, 1).await;
                assert_eq!(
                    read.json(),
                    updated,
                    "case {case:?}: expected the read to report {updated:?}, got {:?}",
                    read.json()
                );
            }
        }
    }
}

/// An identifier or a run number that addresses nothing is HTTP 404, a run number below the
/// modelled range is refused before any handler runs, and no refusal writes anything.
#[tokio::test]
async fn an_update_that_addresses_nothing_is_not_found() {
    let router = router();
    let scope = scope(&router, "run-misses").await;
    let other_application = create_application(&router, "app-run-misses-other").await;
    let (definition_id, started) = running_run(
        &router,
        &scope,
        "run-misses",
        &json!({ "Description": "as started", "ExposurePercentage": 10.0 }),
    )
    .await;

    // Rows differ along WHICH level of the path resolves to nothing.
    for (case, application, addressed, run) in [
        (
            "an application no resource answers to",
            "nosuchapp".to_string(),
            definition_id.clone(),
            1_i64,
        ),
        (
            "a definition name nothing holds",
            scope.application.clone(),
            "no-such-definition".to_string(),
            1,
        ),
        (
            "a definition addressed under the wrong application",
            other_application.clone(),
            definition_id.clone(),
            1,
        ),
        (
            "a run number the definition never issued",
            scope.application.clone(),
            definition_id.clone(),
            2,
        ),
        (
            "a far run number the definition never issued",
            scope.application.clone(),
            definition_id.clone(),
            7,
        ),
        (
            "the largest run number the model's Integer holds",
            scope.application.clone(),
            definition_id.clone(),
            2_147_483_647,
        ),
    ] {
        let refused = update_run(
            &router,
            &application,
            &addressed,
            run,
            &json!({ "Description": "must not be stored" }),
        )
        .await;
        assert_refused(case, &refused, 404, "ResourceNotFoundException");
    }

    // `Run` is a `PositiveInteger` (range 1..), so one below that bound is refused as a bad
    // request rather than looked up.
    let case = "a run number below the modelled range";
    let refused = update_run(
        &router,
        &scope.application,
        &definition_id,
        0,
        &json!({ "Description": "must not be stored" }),
    )
    .await;
    assert_eq!(
        refused.status,
        400,
        "case {case:?}: expected 400, got {} / {}",
        refused.status,
        String::from_utf8_lossy(&refused.body)
    );

    // Every refusal above left the one started run exactly as it was.
    let read = get_run(&router, &scope.application, &definition_id, 1).await;
    assert_eq!(
        read.json(),
        started,
        "expected the refused updates to leave {started:?}, got {:?}",
        read.json()
    );
    for member in PATCHED_MEMBERS {
        assert_eq!(
            read.json().get(member),
            started.get(member),
            "expected {member} to stay {:?}, got {:?}",
            started.get(member),
            read.json().get(member)
        );
    }
}
