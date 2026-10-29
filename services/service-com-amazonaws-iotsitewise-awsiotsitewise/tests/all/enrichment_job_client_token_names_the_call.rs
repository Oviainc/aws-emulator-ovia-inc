// SPDX-License-Identifier: CC0-1.0

//! What a `clientToken` already spent on a DIFFERENT `CreateEnrichmentJob`
//! request means, driven over HTTP through the same `MultiServiceRouter` the
//! emulator boots with, so every assertion is about the wire behaviour a real
//! client sees.
//!
//! The rule these rows pin: a `clientToken` is spent by ONE call. A repeat of the
//! SAME request under that token is a RETRY of that call — answered from the job
//! it started, writing nothing — while a request differing in any member it is
//! recognised by belongs to another call and is refused
//! `ConflictingOperationException` (409) naming the job the token's own call
//! started. A refused request starts no job, so the work a second request asked
//! for is never silently dropped on the floor, and the refusal spends nothing of
//! its own.
//!
//! The rows span what the rule generalises over — the member the second request
//! differs in: the dataset the configuration analyses, the `timeSeriesId` it
//! names, the `propertyAlias` it names, the identifier FORM it names the time
//! series by (an alias where the first call named an id, and the identifier
//! dropped altogether), each bound of the `trimSettings` window, and the
//! `workspaceName` the job is created in.
//!
//! The HTTP harness (`call`, `error_code`, `router_with_engine`) is
//! `asset_lifecycle`'s and the `CreateEnrichmentJob` / workspace / dataset
//! wrappers are `enrichment_jobs`'s; both are reused rather than copied.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::asset_lifecycle::{call, error_code, router_with_engine};
use crate::enrichment_jobs::{
    create, create_dataset, create_workspace, instant, jobs_uri, CAMERA_ALIAS, DOOR_ALIAS,
    LONG_SERIES, SHORT_SERIES,
};

/// The workspace the rows create their jobs in.
const WORKSPACE: &str = "token-enrichment-floor";

/// A second, HELD workspace, for the row whose differing request names another
/// one: a job is created IN a workspace, so the workspace is a member the request
/// is recognised by.
const OTHER_WORKSPACE: &str = "token-enrichment-annex";

/// A `clientToken` built from `seed`: `ClientToken` is `len 36..64, pattern
/// ^\S{36,64}$` (`smithy_query(query="operation",
/// operation="CreateEnrichmentJob")`), so the seed is padded with non-space
/// characters out to the shortest admitted length.
fn client_token(seed: &str) -> String {
    let mut token: String = seed
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    while token.len() < 36 {
        token.push('z');
    }
    token.truncate(36);
    token
}

/// The create body for `configuration`, carrying `token` when one is given.
fn create_body(configuration: &Value, token: Option<&str>) -> Value {
    let mut body = json!({ "jobConfiguration": configuration });
    if let Some(token) = token {
        body.as_object_mut()
            .expect("a CreateEnrichmentJob body is an object")
            .insert("clientToken".to_string(), json!(token));
    }
    body
}

/// An `eventDetection` configuration over `dataset`, naming the time series by
/// `member`/`identifier` when one is given, trimmed to the window
/// `start_seconds..end_seconds`.
fn event_detection(
    dataset: &str,
    member: Option<(&str, &str)>,
    start_seconds: i64,
    end_seconds: i64,
) -> Value {
    let mut detection = json!({
        "datasetId": dataset,
        "trimSettings": {
            "startTime": instant(start_seconds, 0),
            "endTime": instant(end_seconds, 0),
        },
    });
    if let Some((member, identifier)) = member {
        detection
            .as_object_mut()
            .expect("an eventDetection configuration is an object")
            .insert(member.to_string(), json!(identifier));
    }
    json!({ "eventDetection": detection })
}

/// `DescribeEnrichmentJob`, as `OPERATIONS.md` reports its route.
async fn describe(router: &MultiServiceRouter, workspace: &str, job_id: &str) -> (u16, Value) {
    let uri = format!("{}/{job_id}", jobs_uri(workspace));
    call(router, "GET", &uri, None).await
}

/// The `jobId`s `ListEnrichmentJobs` reports for `workspace`, as a set: what a
/// refused create must leave untouched is which jobs the workspace holds, not the
/// order they are enumerated in.
async fn held_jobs(router: &MultiServiceRouter, workspace: &str) -> BTreeSet<String> {
    let uri = format!("{}?maxResults=250", jobs_uri(workspace));
    let (status, body) = call(router, "GET", &uri, None).await;
    assert_eq!(status, 200, "list {workspace}: got {status} {body}");
    body["jobs"]
        .as_array()
        .unwrap_or_else(|| panic!("list {workspace}: jobs is a required list, got {body}"))
        .iter()
        .map(|summary| {
            summary["jobId"]
                .as_str()
                .unwrap_or_else(|| panic!("list {workspace}: a summary without a jobId: {summary}"))
                .to_string()
        })
        .collect()
}

/// `true` when `arn` is an ARN of this service naming the enrichment job
/// `job_id`: the modelled `ResourceArn` form
/// (`^arn:aws(-cn|-us-gov)?:[a-zA-Z0-9-:\/_\.]+$`, `smithy_query(query="shape",
/// shape="ConflictingOperationException")`) whose resource section names the job.
///
/// Hand-rolled as an INDEPENDENT oracle rather than compared against whatever the
/// handler built, so an ARN naming some other resource cannot agree with itself
/// and pass.
fn names_the_job(arn: &str, job_id: &str) -> bool {
    let sections: Vec<&str> = arn.split(':').collect();
    sections.len() >= 6
        && sections[0] == "arn"
        && sections[1].starts_with("aws")
        && sections[2] == "iotsitewise"
        && arn
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-:/_.".contains(&byte))
        && sections[5..].join(":").ends_with(&format!("/{job_id}"))
}

/// One row: the request a token is spent on, and a DIFFERENT request of the same
/// operation sent under that same, now spent, token.
struct Row {
    case: &'static str,
    /// The workspace the first create names, and the configuration it carries.
    workspace: &'static str,
    configuration: Value,
    /// The workspace the DIFFERING create names — another one, for the row that
    /// differs in the workspace it creates in — and the configuration it carries.
    other_workspace: &'static str,
    other_configuration: Value,
}

/// The rows: the same operation sent twice under one token, differing in exactly
/// one member the request is recognised by. `camera` and `door` are two datasets
/// this store really holds.
fn rows(camera: &str, door: &str) -> Vec<Row> {
    vec![
        Row {
            case: "another dataset",
            workspace: WORKSPACE,
            configuration: event_detection(
                camera,
                Some(("propertyAlias", CAMERA_ALIAS)),
                1_700_000_000,
                1_700_003_600,
            ),
            other_workspace: WORKSPACE,
            other_configuration: event_detection(
                door,
                Some(("propertyAlias", CAMERA_ALIAS)),
                1_700_000_000,
                1_700_003_600,
            ),
        },
        Row {
            case: "another timeSeriesId",
            workspace: WORKSPACE,
            configuration: event_detection(
                camera,
                Some(("timeSeriesId", SHORT_SERIES)),
                1_700_000_000,
                1_700_003_600,
            ),
            other_workspace: WORKSPACE,
            other_configuration: event_detection(
                camera,
                Some(("timeSeriesId", LONG_SERIES)),
                1_700_000_000,
                1_700_003_600,
            ),
        },
        Row {
            case: "another propertyAlias",
            workspace: WORKSPACE,
            configuration: event_detection(
                camera,
                Some(("propertyAlias", CAMERA_ALIAS)),
                1_700_000_000,
                1_700_003_600,
            ),
            other_workspace: WORKSPACE,
            other_configuration: event_detection(
                camera,
                Some(("propertyAlias", DOOR_ALIAS)),
                1_700_000_000,
                1_700_003_600,
            ),
        },
        Row {
            case: "the time series named by an alias instead of an id",
            workspace: WORKSPACE,
            configuration: event_detection(
                camera,
                Some(("timeSeriesId", SHORT_SERIES)),
                1_700_000_000,
                1_700_003_600,
            ),
            other_workspace: WORKSPACE,
            other_configuration: event_detection(
                camera,
                Some(("propertyAlias", CAMERA_ALIAS)),
                1_700_000_000,
                1_700_003_600,
            ),
        },
        Row {
            case: "the time series identifier dropped",
            workspace: WORKSPACE,
            configuration: event_detection(
                camera,
                Some(("propertyAlias", CAMERA_ALIAS)),
                1_700_000_000,
                1_700_003_600,
            ),
            other_workspace: WORKSPACE,
            other_configuration: event_detection(camera, None, 1_700_000_000, 1_700_003_600),
        },
        Row {
            case: "another trim window start",
            workspace: WORKSPACE,
            configuration: event_detection(
                camera,
                Some(("propertyAlias", CAMERA_ALIAS)),
                1_700_000_000,
                1_700_003_600,
            ),
            other_workspace: WORKSPACE,
            other_configuration: event_detection(
                camera,
                Some(("propertyAlias", CAMERA_ALIAS)),
                1_700_000_060,
                1_700_003_600,
            ),
        },
        Row {
            case: "another trim window end",
            workspace: WORKSPACE,
            configuration: event_detection(
                camera,
                Some(("propertyAlias", CAMERA_ALIAS)),
                1_700_000_000,
                1_700_003_600,
            ),
            other_workspace: WORKSPACE,
            other_configuration: event_detection(
                camera,
                Some(("propertyAlias", CAMERA_ALIAS)),
                1_700_000_000,
                1_700_007_200,
            ),
        },
        Row {
            case: "another workspace",
            workspace: WORKSPACE,
            configuration: event_detection(
                camera,
                Some(("propertyAlias", CAMERA_ALIAS)),
                1_700_000_000,
                1_700_003_600,
            ),
            other_workspace: OTHER_WORKSPACE,
            other_configuration: event_detection(
                camera,
                Some(("propertyAlias", CAMERA_ALIAS)),
                1_700_000_000,
                1_700_003_600,
            ),
        },
    ]
}

/// The two held workspaces and the two datasets every row works from, on a router
/// of its own so the listings report exactly what that row's calls left behind.
/// Answers the router, the engine behind it and the two dataset ids.
async fn fixture() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>, String, String) {
    let (router, engine) = router_with_engine();
    for name in [WORKSPACE, OTHER_WORKSPACE] {
        create_workspace(&router, name).await;
    }
    let camera = create_dataset(&router, "Camera footage").await;
    let door = create_dataset(&router, "Door footage").await;
    (router, engine, camera, door)
}

#[tokio::test]
async fn a_token_one_create_spent_refuses_a_different_create_and_starts_nothing() {
    let row_count = rows("camera", "door").len();
    for at in 0..row_count {
        // A router per row, so the listings below report exactly what this row's
        // own calls left behind.
        let (router, _engine, camera, door) = fixture().await;
        let mut rows = rows(&camera, &door);
        let row = rows.swap_remove(at);
        let token = client_token(row.case);

        let (status, first) = create(
            &router,
            row.workspace,
            create_body(&row.configuration, Some(&token)),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: the first create expected 200, got {status} {first}",
            row.case
        );
        let job_id = first["jobId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: no jobId in {first}", row.case))
            .to_string();

        // The SAME request under that token is a retry of the call above: it is
        // answered from the job that call started, at that job's own identity and
        // creation instant.
        let (status, retried) = create(
            &router,
            row.workspace,
            create_body(&row.configuration, Some(&token)),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: the retry expected 200, got {status} {retried}",
            row.case
        );
        assert_eq!(
            retried["jobId"].as_str().unwrap_or(""),
            job_id,
            "case {:?}: a retry answers from the job its token started, got {retried}",
            row.case
        );
        assert_eq!(
            retried["createdAt"].as_f64(),
            first["createdAt"].as_f64(),
            "case {:?}: a retry reports the job's own creation instant, got {retried}",
            row.case
        );

        // The DIFFERING request reusing that spent token repeats no call, so it is
        // the modelled conflict, naming the job the token's own call started.
        let (status, refused) = create(
            &router,
            row.other_workspace,
            create_body(&row.other_configuration, Some(&token)),
        )
        .await;
        assert_eq!(
            (status, error_code(&refused)),
            (409, "ConflictingOperationException"),
            "case {:?}: a spent token expected the modelled conflict, got {status} {refused}",
            row.case
        );
        assert_eq!(
            refused["resourceId"].as_str().unwrap_or(""),
            job_id,
            "case {:?}: the conflict names the job the first call started, got {refused}",
            row.case
        );
        let reported_arn = refused["resourceArn"].as_str().unwrap_or("");
        assert!(
            names_the_job(reported_arn, &job_id),
            "case {:?}: expected an ARN naming the job {job_id}, got {reported_arn:?}",
            row.case
        );

        // The refused request started nothing: the stored job still carries the
        // configuration the FIRST call submitted, and no workspace holds a second
        // job.
        let (status, described) = describe(&router, row.workspace, &job_id).await;
        assert_eq!(
            status, 200,
            "case {:?}: the read of the first job got {status} {described}",
            row.case
        );
        assert_eq!(
            described["jobConfiguration"], row.configuration,
            "case {:?}: the refused request must not rewrite the stored job, got {described}",
            row.case
        );
        let held = held_jobs(&router, row.workspace).await;
        assert_eq!(
            held,
            BTreeSet::from([job_id.clone()]),
            "case {:?}: {} must hold only the job the first call started, holds {held:?}",
            row.case,
            row.workspace
        );
        if row.other_workspace != row.workspace {
            let elsewhere = held_jobs(&router, row.other_workspace).await;
            assert!(
                elsewhere.is_empty(),
                "case {:?}: the refused request must start no job in {}, holds {elsewhere:?}",
                row.case,
                row.other_workspace
            );
        }

        // The refusal spent nothing of its own: the token still names the first
        // call, so a retry of THAT request is still answered from its job.
        let (status, again) = create(
            &router,
            row.workspace,
            create_body(&row.configuration, Some(&token)),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: the retry after the refusal expected 200, got {status} {again}",
            row.case
        );
        assert_eq!(
            again["jobId"].as_str().unwrap_or(""),
            job_id,
            "case {:?}: a refused request must not unspend the token, got {again}",
            row.case
        );
    }
}

#[tokio::test]
async fn a_token_no_create_has_spent_starts_a_job_of_its_own() {
    let (router, _engine, camera, _door) = fixture().await;

    // Rows: fresh tokens at each published `ClientToken` length bound and one in
    // between, plus two requests carrying no token at all. None of them repeats a
    // call, so each must start a job of its own. Each row analyses a time series
    // of its own, so no row is the duplicate of the work another row started —
    // what is under test is the token, not the duplicate-job rule.
    let mut tokens: Vec<Option<String>> = vec![None, None];
    // Source: `ClientToken` is `@length(min: 36, max: 64)`.
    for length in [36_usize, 50, 64] {
        let mut token = client_token(&format!("fresh-token-of-{length}-characters"));
        while token.len() < length {
            token.push('f');
        }
        assert_eq!(
            token.len(),
            length,
            "case {length}: this row needs a token of {length} characters, got {token:?}"
        );
        tokens.push(Some(token));
    }

    let mut started: Vec<String> = Vec::new();
    for (at, token) in tokens.iter().enumerate() {
        let configuration = event_detection(
            &camera,
            Some((
                "propertyAlias",
                &format!("/company/windfarm/{at}/turbine/{at}/camera"),
            )),
            1_700_000_000,
            1_700_003_600,
        );
        let case = token.as_deref().unwrap_or("no token");
        let (status, answered) = create(
            &router,
            WORKSPACE,
            create_body(&configuration, token.as_deref()),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {answered}"
        );
        let job_id = answered["jobId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no jobId in {answered}"))
            .to_string();
        assert!(
            !started.contains(&job_id),
            "case {case:?}: a call that repeats none must start a job of its own, \
             got {job_id} again"
        );
        started.push(job_id);
    }

    let held = held_jobs(&router, WORKSPACE).await;
    assert_eq!(
        held,
        started.iter().cloned().collect::<BTreeSet<String>>(),
        "every row started a job of its own: {started:?} started, {held:?} held"
    );
}
