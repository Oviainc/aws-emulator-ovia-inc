// SPDX-License-Identifier: CC0-1.0

//! What `DescribeTask` reports, driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots with.
//!
//! The rule these suites pin: a read of a versioned resource answers from the
//! version the request names — the newest one when it names none — reporting the
//! members that version was written with, the identity the create minted and the
//! lifecycle state the clock has made due, and it leaves the resource exactly as a
//! later caller would have found it; a version label no write ever produced
//! addresses nothing the service holds, and one the modelled `Version` form does
//! not admit never reaches the store at all.
//!
//! The sibling `task_lifecycle` suite covers the WRITES (what a create and an
//! update persist); this one is about the read itself, so it reuses that slice's
//! HTTP harness rather than growing a second.

use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{
    ACTIVATION_DELAY, FIRST_TASK_VERSION,
};

use crate::asset_lifecycle::{error_code, router};
// The task slice's harness lives in `task_lifecycle`, which owns the writes:
// the routes, the request helpers, the configuration builder and the enum
// catalogues come from there so one copy serves both suites.
use crate::task_lifecycle::{
    create, describe, minimal_container, state, update, without_status, EXECUTION_ROLE,
    PROCESSING_TYPES, PROCESSING_UNITS, PUBLIC_IMAGE,
};

/// The workspace these suites work in.
const WORKSPACE: &str = "read-side";

/// Comfortably past [`ACTIVATION_DELAY`], so the `CREATING` -> `ACTIVE`
/// transition a create owes has fallen due whatever the machine is doing.
fn past_the_owed_transition() -> Duration {
    ACTIVATION_DELAY + Duration::from_millis(250)
}

/// The required members of a `containerTaskConfiguration` with the
/// public-registry `ecrUri` form, which [`minimal_container`] does not reach: it
/// always sends the private-registry form.
fn public_registry_container(processing_unit: &str) -> Value {
    json!({
        "containerTaskConfiguration": {
            "ecrUri": PUBLIC_IMAGE,
            "taskExecutionRole": EXECUTION_ROLE,
            "processingType": PROCESSING_TYPES[0],
            "processingUnit": processing_unit,
        }
    })
}

/// One row of the version table: the write that minted a version, and what a read
/// naming that version has to report.
#[derive(Debug)]
struct VersionCase {
    label: &'static str,
    /// The `UpdateTask` body that minted this version; `None` for the version the
    /// create minted.
    write: Option<Value>,
    description: Value,
    configuration: Value,
}

/// The rows: the version the create wrote, then one version per shape an update
/// can take — a write that carries only a description, only a configuration, and
/// both at once — so the read is checked against a different combination of
/// members each time rather than against a renamed copy of one.
fn version_cases() -> Vec<VersionCase> {
    vec![
        VersionCase {
            label: "the version the create minted",
            write: None,
            description: json!("the first configuration"),
            configuration: minimal_container(PROCESSING_TYPES[0], PROCESSING_UNITS[0]),
        },
        VersionCase {
            label: "a write carrying only a description",
            write: Some(json!({ "description": "described again" })),
            description: json!("described again"),
            configuration: minimal_container(PROCESSING_TYPES[0], PROCESSING_UNITS[0]),
        },
        VersionCase {
            label: "a write carrying only a configuration",
            write: Some(json!({ "taskConfiguration": public_registry_container(PROCESSING_UNITS[2]) })),
            description: json!("described again"),
            configuration: public_registry_container(PROCESSING_UNITS[2]),
        },
        VersionCase {
            label: "a write carrying both",
            write: Some(json!({
                "description": "the last word",
                "taskConfiguration": minimal_container(PROCESSING_TYPES[1], PROCESSING_UNITS[13]),
            })),
            description: json!("the last word"),
            configuration: minimal_container(PROCESSING_TYPES[1], PROCESSING_UNITS[13]),
        },
    ]
}

#[tokio::test]
async fn describe_task_answers_from_the_version_the_request_names() {
    let router = router();
    let name = "versioned-reader";
    let cases = version_cases();

    // Lay down one version per row, in order, so version N is the row at index
    // N - FIRST_TASK_VERSION.
    for (index, case) in cases.iter().enumerate() {
        let expected_version = FIRST_TASK_VERSION + index as u64;
        let (status, written) = match &case.write {
            None => {
                create(
                    &router,
                    WORKSPACE,
                    json!({
                        "taskName": name,
                        "description": case.description,
                        "taskConfiguration": case.configuration,
                    }),
                )
                .await
            }
            Some(body) => update(&router, WORKSPACE, name, body.clone()).await,
        };
        assert_eq!(
            status, 200,
            "case {:?}: the write expected 200, got {status} {written}",
            case.label
        );
        assert_eq!(
            written["version"].as_str().unwrap_or(""),
            expected_version.to_string(),
            "case {:?}: the write must mint version {expected_version}, got {written}",
            case.label
        );
    }

    let newest = FIRST_TASK_VERSION + cases.len() as u64 - 1;
    let (status, latest) = describe(&router, WORKSPACE, name, None).await;
    assert_eq!(
        status, 200,
        "case {:?}: DescribeTask expected 200, got {status} {latest}",
        "no version named"
    );
    let created_at = latest["createdAt"].clone();
    let task_arn = latest["taskArn"].clone();
    let mut previous_updated_at = f64::MIN;

    for (index, case) in cases.iter().enumerate() {
        let version = (FIRST_TASK_VERSION + index as u64).to_string();
        let (status, described) = describe(&router, WORKSPACE, name, Some(&version)).await;
        assert_eq!(
            status, 200,
            "case {:?}: reading version {version} expected 200, got {status} {described}",
            case.label
        );
        assert_eq!(
            described["version"].as_str().unwrap_or(""),
            version,
            "case {:?}: the read must answer from the version it named, got {described}",
            case.label
        );
        assert_eq!(
            described["description"], case.description,
            "case {:?}: version {version} must carry the description that write left, got {described}",
            case.label
        );
        assert_eq!(
            described["taskConfiguration"], case.configuration,
            "case {:?}: version {version} must carry the configuration that write left, got {described}",
            case.label
        );
        assert_eq!(
            described["taskName"].as_str().unwrap_or(""),
            name,
            "case {:?}: every version reports the task's own name, got {described}",
            case.label
        );
        assert_eq!(
            described["workspaceName"].as_str().unwrap_or(""),
            WORKSPACE,
            "case {:?}: every version reports the workspace it lives in, got {described}",
            case.label
        );
        assert_eq!(
            described["taskArn"], task_arn,
            "case {:?}: the identity the create minted does not change per version, got {described}",
            case.label
        );
        assert_eq!(
            described["createdAt"], created_at,
            "case {:?}: createdAt is the task's own, not the version's, got {described}",
            case.label
        );

        // `updatedAt` is when the version being read was written, so it rises (or
        // holds, on a clock of coarse resolution) with the version number.
        let updated_at = described["updatedAt"].as_f64().unwrap_or(f64::MIN);
        assert!(
            updated_at >= previous_updated_at,
            "case {:?}: version {version} reports updatedAt {updated_at} before the previous \
             version's {previous_updated_at}, got {described}",
            case.label
        );
        assert!(
            updated_at >= created_at.as_f64().unwrap_or(f64::MAX),
            "case {:?}: version {version} reports updatedAt {updated_at} before the task was \
             created, got {described}",
            case.label
        );
        previous_updated_at = updated_at;

        // Naming the newest version is the same read as naming none.
        if FIRST_TASK_VERSION + index as u64 == newest {
            assert_eq!(
                without_status(&described),
                without_status(&latest),
                "case {:?}: naming the newest version must read the same as naming none, got \
                 {described} against {latest}",
                case.label
            );
        }
    }
}

#[tokio::test]
async fn a_version_label_no_write_produced_or_the_model_refuses_never_reads_a_task() {
    let router = router();
    let name = "one-version-only";
    let (status, created) = create(
        &router,
        WORKSPACE,
        json!({
            "taskName": name,
            "taskConfiguration": minimal_container(PROCESSING_TYPES[0], PROCESSING_UNITS[1]),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: the create expected 200, got {status} {created}",
        "the create"
    );

    // (case, label) — labels the modelled `Version` form ADMITS (length 1..10,
    // pattern ^(0|([1-9]{1}\d*))$, smithy_query query="shape" shape="Version")
    // that no write to this task ever produced: the store holds no such version,
    // so the read is the modelled not-found.
    let absent = [
        ("the zero the pattern admits", "0"),
        ("one past the newest version", "2"),
        ("far past the newest version", "4294967295"),
        ("the longest label the length admits", "1000000000"),
    ];
    for (case, label) in absent {
        let (status, body) = describe(&router, WORKSPACE, name, Some(label)).await;
        assert_eq!(
            (status, error_code(&body)),
            (404, "ResourceNotFoundException"),
            "case {case:?}: version {label} names nothing this service holds, got {status} {body}"
        );
        assert!(
            body["message"].as_str().unwrap_or("").contains(name),
            "case {case:?}: the refusal names the task the request addressed, got {body}"
        );
        assert!(
            body["message"].as_str().unwrap_or("").contains(label),
            "case {case:?}: the refusal names the version the request asked for, got {body}"
        );
    }

    // (case, label) — labels OUTSIDE that form: refused before the handler runs,
    // so the request is a 400 and no read happens at all.
    let malformed = [
        ("a leading zero", "01"),
        ("a word", "latest"),
        ("a negative number", "-1"),
        ("a decimal", "1.0"),
        ("one digit past the longest label", "12345678901"),
    ];
    for (case, label) in malformed {
        let (status, body) = describe(&router, WORKSPACE, name, Some(label)).await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "InvalidRequestException"),
            "case {case:?}: version {label} is outside the modelled form, got {status} {body}"
        );
        assert!(
            body["message"].as_str().unwrap_or("").contains("taskVersion"),
            "case {case:?}: the refusal has to name the member that was wrong, got {body}"
        );
    }

    // The task itself is untouched by every refusal above: still at its first
    // version, still readable.
    let (status, described) = describe(&router, WORKSPACE, name, None).await;
    assert_eq!(
        status, 200,
        "case {:?}: expected 200, got {status} {described}",
        "the task the refusals addressed"
    );
    assert_eq!(
        described["version"].as_str().unwrap_or(""),
        FIRST_TASK_VERSION.to_string(),
        "case {:?}: a refused read must not mint a version, got {described}",
        "the task the refusals addressed"
    );
}

#[tokio::test]
async fn describing_a_task_reports_the_clock_and_changes_nothing() {
    let router = router();
    let name = "polled-task";
    let (status, created) = create(
        &router,
        WORKSPACE,
        json!({
            "taskName": name,
            "description": "read me repeatedly",
            "taskConfiguration": minimal_container(PROCESSING_TYPES[0], PROCESSING_UNITS[3]),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: the create expected 200, got {status} {created}",
        "the create"
    );
    assert_eq!(
        state(&created),
        "CREATING",
        "case {:?}: a create owes a transition, so it answers CREATING, got {created}",
        "the create"
    );

    // Polling inside the owed span reports the transient state and moves nothing:
    // every member but the status is identical read after read.
    let (status, first) = describe(&router, WORKSPACE, name, None).await;
    assert_eq!(
        status, 200,
        "case {:?}: DescribeTask expected 200, got {status} {first}",
        "the first read"
    );
    let mut settled = state(&first) == "ACTIVE";
    for poll in 1..=4 {
        let (status, again) = describe(&router, WORKSPACE, name, None).await;
        assert_eq!(
            status, 200,
            "case {poll:?}: DescribeTask expected 200, got {status} {again}"
        );
        assert_eq!(
            without_status(&again),
            without_status(&first),
            "case {poll:?}: a read must not change what a later read sees, got {again} against \
             {first}"
        );
        // The only member time moves, and it moves one way: once the owed
        // transition has fallen due it never goes back to the transient state.
        let reported = state(&again);
        assert!(
            matches!(reported, "CREATING" | "ACTIVE"),
            "case {poll:?}: a created task is CREATING or settled ACTIVE, got {again}"
        );
        assert!(
            !(settled && reported == "CREATING"),
            "case {poll:?}: the state went back to CREATING after settling, got {again}"
        );
        settled |= reported == "ACTIVE";
    }

    // Past the owed instant the state has settled — and stays settled, however
    // many times it is read.
    tokio::time::sleep(past_the_owed_transition()).await;
    for poll in 1..=2 {
        let (status, described) = describe(&router, WORKSPACE, name, None).await;
        assert_eq!(
            status, 200,
            "case {poll:?}: DescribeTask expected 200, got {status} {described}"
        );
        assert_eq!(
            state(&described),
            "ACTIVE",
            "case {poll:?}: past the owed transition the task is ACTIVE, got {described}"
        );
        assert_eq!(
            without_status(&described),
            without_status(&first),
            "case {poll:?}: settling changes only the status, got {described} against {first}"
        );
    }

    // And the task is still writable afterwards: the reads consumed nothing.
    let (status, updated) = update(
        &router,
        WORKSPACE,
        name,
        json!({ "description": "written after the reads" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: UpdateTask expected 200, got {status} {updated}",
        "the write after the reads"
    );
    assert_eq!(
        updated["version"].as_str().unwrap_or(""),
        (FIRST_TASK_VERSION + 1).to_string(),
        "case {:?}: the reads must not have minted a version, got {updated}",
        "the write after the reads"
    );
}

#[tokio::test]
async fn a_task_is_addressed_by_the_exact_names_it_was_created_under() {
    let router = router();
    let name = "Exact-Name";
    let (status, created) = create(
        &router,
        WORKSPACE,
        json!({
            "taskName": name,
            "taskConfiguration": minimal_container(PROCESSING_TYPES[0], PROCESSING_UNITS[4]),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: the create expected 200, got {status} {created}",
        "the create"
    );

    // (case, workspace, task) — each differs from the stored pair in one way the
    // identity does not forgive: the case of a name, a name that is a prefix of
    // the stored one, and a workspace nothing was created in.
    let cases = [
        ("the task name in another case", WORKSPACE, "exact-name"),
        ("the task name upper-cased", WORKSPACE, "EXACT-NAME"),
        ("a prefix of the task name", WORKSPACE, "Exact"),
        ("the workspace name in another case", "Read-Side", name),
        ("a workspace nothing was created in", "elsewhere", name),
    ];
    for (case, workspace, task) in cases {
        let (status, body) = describe(&router, workspace, task, None).await;
        assert_eq!(
            (status, error_code(&body)),
            (404, "ResourceNotFoundException"),
            "case {case:?}: {workspace}/{task} addresses nothing this service holds, got {status} \
             {body}"
        );
        assert!(
            body["message"].as_str().unwrap_or("").contains(task),
            "case {case:?}: the refusal names the task the request addressed, got {body}"
        );
    }

    // The pair the create did make still reads back.
    let (status, described) = describe(&router, WORKSPACE, name, None).await;
    assert_eq!(
        status, 200,
        "case {:?}: expected 200, got {status} {described}",
        "the stored pair"
    );
    assert_eq!(
        described["taskName"].as_str().unwrap_or(""),
        name,
        "case {:?}: the read reports the name the create was given, got {described}",
        "the stored pair"
    );
}
