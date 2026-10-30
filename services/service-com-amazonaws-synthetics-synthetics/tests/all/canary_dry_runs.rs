// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `StartCanaryDryRun` and the reads that address a dry run.
//!
//! The rule asserted here: a dry run is one run of a canary that already exists, started
//! with the canary's configuration overridden by the members the request supplies, minted
//! under its own `DryRunId` that the dry-run reads address it by — and the canary itself is
//! left exactly as it was, because a dry run is independent of the canary's own schedule
//! and runs.
//!
//! Every row of every table below varies the request along what that rule generalises
//! over (which configuration member is overridden, how many dry runs a canary has, which
//! page is asked for), and every assertion goes over the wire through the same
//! `MultiServiceRouter` the emulator boots, through the harness the lifecycle suite
//! defines.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Value};

use crate::canary_lifecycle::{create_body, is_uuid, Answer, Synthetics};

/// Comfortably longer than the provisioning span the emulator owes, so a canary created at
/// the start of a step is `READY` by the next one.
const PAST_PROVISIONING: Duration = Duration::from_millis(900);

/// Comfortably longer than the span after which the emulator owes a started run its
/// outcome, so a run read after this wait has provably settled.
const PAST_RUN_SETTLE: Duration = Duration::from_millis(900);

/// Shorter than that span, so an assertion about a run that is still OPEN is only made
/// when the read provably happened inside it.
const RUN_STILL_OPEN_WITHIN: Duration = Duration::from_millis(250);

/// The runtime version `create_body` gives the canary, so an override row can assert on a
/// value that is not this one.
const CANARY_RUNTIME_VERSION: &str = "syn-nodejs-puppeteer-9.0";

/// The artifact location `create_body` gives the canary.
const CANARY_ARTIFACT_LOCATION: &str = "s3://artifacts/canary";

/// The fixed sentence a dry-run lookup that misses answers with.
const DRY_RUN_NOT_FOUND: &str = "Dry run not found.";

/// `POST /canary/{Name}/dry-run/start`.
async fn start_dry_run(aws: &Synthetics, name: &str, body: Value) -> Answer {
    aws.call(
        "POST",
        &format!("/canary/{name}/dry-run/start"),
        Some(body),
    )
    .await
}

/// `GET /canary/{Name}?dryRunId=...`.
async fn get_for_dry_run(aws: &Synthetics, name: &str, dry_run_id: &str) -> Answer {
    aws.call("GET", &format!("/canary/{name}?dryRunId={dry_run_id}"), None)
        .await
}

/// Create one canary and leave it `READY`, panicking with what came back if it does not
/// get there: every test below starts from a canary with no runs of its own.
async fn ready_canary(aws: &Synthetics, name: &str) {
    let created = aws.create(create_body(name)).await;
    assert_eq!(
        created.status, 200,
        "case create {name}: expected 200, got {} ({:?})",
        created.status, created.body
    );
    tokio::time::sleep(PAST_PROVISIONING).await;
    let read = aws.get(name).await;
    assert_eq!(
        read.at("Canary.Status.State").as_str(),
        Some("READY"),
        "case create {name}: expected the canary to be READY before the dry run, got {:?}",
        read.at("Canary.Status")
    );
}

/// The `DryRunId` a successful `StartCanaryDryRun` answers with, asserted to be a minted
/// UUID and reported as a run that is still open.
fn minted_dry_run_id(case: &str, answer: &Answer) -> String {
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200, got {} ({:?})",
        answer.status, answer.body
    );
    let id = answer.string_at("DryRunConfig.DryRunId");
    assert!(
        is_uuid(&id),
        "case {case}: expected DryRunConfig.DryRunId to be a UUID, got {id:?}"
    );
    assert_eq!(
        answer.at("DryRunConfig.LastDryRunExecutionStatus").as_str(),
        Some("RUNNING"),
        "case {case}: expected the dry run just started to be RUNNING, got {:?}",
        answer.at("DryRunConfig")
    );
    id
}

/// One override row: the members the dry run supplies, and the path in the reported
/// `Canary` each one shows up at with the value it should carry.
struct Override {
    case: &'static str,
    body: Value,
    /// `(path, expected value under the dry run)`. The same paths are asserted UNCHANGED
    /// on the canary's own read, so each row pins both halves of the rule.
    reported: Vec<(&'static str, Value)>,
}

/// The whole rule over one canary per row: the dry run reports the canary's configuration
/// with this row's members overridden, the canary itself is byte-for-byte unchanged, the
/// dry run is a run of its own, and the canary still has no runs.
#[tokio::test]
async fn a_dry_run_reports_the_overridden_configuration_and_leaves_the_canary_alone() {
    // Every row overrides a different member of the configuration, which is what the rule
    // generalises over; the values come from the members' own modelled types, and
    // `RuntimeVersion` from this service's published catalogue
    // (`DescribeRuntimeVersions`).
    let rows = vec![
        Override {
            case: "no overrides",
            body: json!({}),
            reported: vec![
                ("Canary.RuntimeVersion", json!(CANARY_RUNTIME_VERSION)),
                ("Canary.ArtifactS3Location", json!(CANARY_ARTIFACT_LOCATION)),
            ],
        },
        Override {
            case: "runtime version",
            body: json!({ "RuntimeVersion": "syn-nodejs-puppeteer-10.0" }),
            reported: vec![(
                "Canary.RuntimeVersion",
                json!("syn-nodejs-puppeteer-10.0"),
            )],
        },
        Override {
            case: "runtime version of another family",
            body: json!({ "RuntimeVersion": "syn-python-selenium-5.1" }),
            reported: vec![("Canary.RuntimeVersion", json!("syn-python-selenium-5.1"))],
        },
        Override {
            case: "artifact location",
            body: json!({ "ArtifactS3Location": "s3://dry-run-artifacts/probe" }),
            reported: vec![(
                "Canary.ArtifactS3Location",
                json!("s3://dry-run-artifacts/probe"),
            )],
        },
        Override {
            case: "execution role",
            body: json!({
                "ExecutionRoleArn": "arn:aws:iam::123456789012:role/DryRunRole"
            }),
            reported: vec![(
                "Canary.ExecutionRoleArn",
                json!("arn:aws:iam::123456789012:role/DryRunRole"),
            )],
        },
        Override {
            case: "code",
            body: json!({
                "Code": {
                    "Handler": "dry-run.handler",
                    "S3Bucket": "scripts",
                    "S3Key": "dry-run.zip"
                }
            }),
            reported: vec![("Canary.Code.Handler", json!("dry-run.handler"))],
        },
        Override {
            case: "run config",
            body: json!({ "RunConfig": { "TimeoutInSeconds": 120, "MemoryInMB": 1024 } }),
            reported: vec![
                ("Canary.RunConfig.TimeoutInSeconds", json!(120)),
                ("Canary.RunConfig.MemoryInMB", json!(1024)),
            ],
        },
        Override {
            case: "retention periods",
            body: json!({
                "SuccessRetentionPeriodInDays": 3,
                "FailureRetentionPeriodInDays": 5
            }),
            reported: vec![
                ("Canary.SuccessRetentionPeriodInDays", json!(3)),
                ("Canary.FailureRetentionPeriodInDays", json!(5)),
            ],
        },
        Override {
            case: "provisioned resource cleanup",
            body: json!({ "ProvisionedResourceCleanup": "OFF" }),
            reported: vec![("Canary.ProvisionedResourceCleanup", json!("OFF"))],
        },
        Override {
            case: "browser configs",
            body: json!({ "BrowserConfigs": [{ "BrowserType": "FIREFOX" }] }),
            reported: vec![(
                "Canary.BrowserConfigs",
                json!([{ "BrowserType": "FIREFOX" }]),
            )],
        },
        Override {
            case: "vpc config",
            body: json!({
                "VpcConfig": {
                    "SubnetIds": ["subnet-0123456789abcdef0"],
                    "SecurityGroupIds": ["sg-0123456789abcdef0"]
                }
            }),
            reported: vec![(
                "Canary.VpcConfig.SubnetIds",
                json!(["subnet-0123456789abcdef0"]),
            )],
        },
        Override {
            case: "artifact encryption",
            body: json!({
                "ArtifactConfig": { "S3Encryption": { "EncryptionMode": "SSE_S3" } }
            }),
            reported: vec![(
                "Canary.ArtifactConfig.S3Encryption.EncryptionMode",
                json!("SSE_S3"),
            )],
        },
    ];

    let aws = Synthetics::new("eu-west-1");
    // One canary per row, all created first so the provisioning span is waited out once.
    let names: Vec<String> = (0..rows.len())
        .map(|row| format!("dry-run-canary-{row}"))
        .collect();
    for name in &names {
        let created = aws.create(create_body(name)).await;
        assert_eq!(
            created.status, 200,
            "case create {name}: expected 200, got {} ({:?})",
            created.status, created.body
        );
    }
    tokio::time::sleep(PAST_PROVISIONING).await;

    let mut ids: BTreeSet<String> = BTreeSet::new();
    for (over, name) in rows.iter().zip(&names) {
        let case = over.case;
        let before = aws.get(name).await;
        assert_eq!(
            before.at("Canary.Status.State").as_str(),
            Some("READY"),
            "case {case}: expected the canary to be READY before the dry run, got {:?}",
            before.at("Canary.Status")
        );

        let answer = start_dry_run(&aws, name, over.body.clone()).await;
        let dry_run_id = minted_dry_run_id(case, &answer);
        assert!(
            ids.insert(dry_run_id.clone()),
            "case {case}: expected a DryRunId no earlier dry run was given, got {dry_run_id:?}"
        );

        // The canary is untouched: its whole reported record is the one it had before the
        // dry run, down to its state, timeline and configuration.
        let after = aws.get(name).await;
        assert_eq!(
            after.body, before.body,
            "case {case}: expected the canary to be unchanged by the dry run, got {:?} \
             (was {:?})",
            after.body, before.body
        );

        // The dry run reports the canary's configuration with this row's overrides.
        let reported = get_for_dry_run(&aws, name, &dry_run_id).await;
        assert_eq!(
            reported.status, 200,
            "case {case}: expected 200 reading the dry run, got {} ({:?})",
            reported.status, reported.body
        );
        for (path, expected) in &over.reported {
            assert_eq!(
                reported.at(path),
                expected,
                "case {case}: expected {path} to be {expected:?} under the dry run, got {:?}",
                reported.at(path)
            );
            if case != "no overrides" {
                assert_ne!(
                    before.at(path),
                    expected,
                    "case {case}: the row asserts an OVERRIDE, so the canary's own {path} \
                     must differ from {expected:?}, got {:?}",
                    before.at(path)
                );
            }
        }
        // Identity is the canary's: a dry run is a configuration, not a second canary.
        assert_eq!(
            reported.at("Canary.Id"),
            before.at("Canary.Id"),
            "case {case}: expected the canary's own Id under the dry run, got {:?}",
            reported.at("Canary.Id")
        );
        assert_eq!(
            reported.at("Canary.Name").as_str(),
            Some(name.as_str()),
            "case {case}: expected the canary's own Name under the dry run, got {:?}",
            reported.at("Canary.Name")
        );
        assert_eq!(
            reported.at("Canary.DryRunConfig.DryRunId").as_str(),
            Some(dry_run_id.as_str()),
            "case {case}: expected the read to report the dry run it was asked for, got {:?}",
            reported.at("Canary.DryRunConfig")
        );

        // The dry run is a run of its own, and the canary's own run history is still
        // empty: nothing about the canary's schedule was touched.
        let dry_runs = reported_runs(&aws, name, json!({ "RunType": "DRY_RUN" })).await;
        assert_eq!(
            dry_runs.len(),
            1,
            "case {case}: expected exactly one dry run, got {dry_runs:?}"
        );
        assert_eq!(
            dry_runs[0]["DryRunConfig"]["DryRunId"].as_str(),
            Some(dry_run_id.as_str()),
            "case {case}: expected the dry run's own id on its run, got {:?}",
            dry_runs[0]["DryRunConfig"]
        );
        assert_eq!(
            dry_runs[0]["Name"].as_str(),
            Some(name.as_str()),
            "case {case}: expected the run to name its canary, got {:?}",
            dry_runs[0]["Name"]
        );
        let canary_runs = reported_runs(&aws, name, json!({})).await;
        assert!(
            canary_runs.is_empty(),
            "case {case}: expected the canary's own run history to stay empty, got \
             {canary_runs:?}"
        );
    }
}

/// `GetCanaryRuns` for `name`, returning the reported runs or panicking with what came
/// back instead.
async fn reported_runs(aws: &Synthetics, name: &str, body: Value) -> Vec<Value> {
    let answer = aws.runs(name, body.clone()).await;
    assert_eq!(
        answer.status, 200,
        "case runs {name} {body}: expected 200, got {} ({:?})",
        answer.status, answer.body
    );
    answer
        .at("CanaryRuns")
        .as_array()
        .unwrap_or_else(|| panic!("CanaryRuns is an array, got {:?}", answer.at("CanaryRuns")))
        .clone()
}

/// Each dry run of one canary is its own run under its own id, newest first, and a
/// `DryRunId` narrows the history to that one dry run — over a count of dry runs and over
/// a page size.
#[tokio::test]
async fn repeated_dry_runs_are_each_their_own_run_addressed_by_its_own_id() {
    let aws = Synthetics::new("us-west-2");
    let name = "repeat-dry-run";
    ready_canary(&aws, name).await;

    // Three is enough to pin the order and to page: one page of two and one of one.
    let mut minted: Vec<String> = Vec::new();
    for round in 0..3 {
        let case = format!("dry run {round}");
        let answer = start_dry_run(&aws, name, json!({})).await;
        minted.push(minted_dry_run_id(&case, &answer));
    }
    let distinct: BTreeSet<&String> = minted.iter().collect();
    assert_eq!(
        distinct.len(),
        minted.len(),
        "case distinct ids: expected one id per dry run, got {minted:?}"
    );

    let all = reported_runs(&aws, name, json!({ "RunType": "DRY_RUN" })).await;
    let reported: Vec<String> = all
        .iter()
        .map(|run| run["DryRunConfig"]["DryRunId"].as_str().unwrap_or_default().to_string())
        .collect();
    let newest_first: Vec<String> = minted.iter().rev().cloned().collect();
    assert_eq!(
        reported, newest_first,
        "case order: expected every dry run newest first, got {reported:?}"
    );

    // A `DryRunId` narrows the history to the runs of that one dry run.
    for (round, id) in minted.iter().enumerate() {
        let one = reported_runs(
            &aws,
            name,
            json!({ "RunType": "DRY_RUN", "DryRunId": id }),
        )
        .await;
        assert_eq!(
            one.len(),
            1,
            "case filter to dry run {round}: expected exactly its own run, got {one:?}"
        );
        assert_eq!(
            one[0]["DryRunConfig"]["DryRunId"].as_str(),
            Some(id.as_str()),
            "case filter to dry run {round}: expected {id:?}, got {:?}",
            one[0]["DryRunConfig"]
        );
    }

    // Paging runs over the filtered history, two rows then the last one.
    let first_page = aws
        .runs(name, json!({ "RunType": "DRY_RUN", "MaxResults": 2 }))
        .await;
    assert_eq!(
        first_page.at("CanaryRuns").as_array().map(Vec::len),
        Some(2),
        "case page of two: expected two runs, got {:?}",
        first_page.at("CanaryRuns")
    );
    let token = first_page.string_at("NextToken");
    let last_page = aws
        .runs(
            name,
            json!({ "RunType": "DRY_RUN", "MaxResults": 2, "NextToken": token }),
        )
        .await;
    assert_eq!(
        last_page.at("CanaryRuns").as_array().map(Vec::len),
        Some(1),
        "case last page: expected the one remaining run, got {:?}",
        last_page.at("CanaryRuns")
    );
    assert!(
        last_page.at("NextToken").is_null(),
        "case last page: expected no further token, got {:?}",
        last_page.at("NextToken")
    );

    // The canary itself still has no runs of its own.
    let own = reported_runs(&aws, name, json!({})).await;
    assert!(
        own.is_empty(),
        "case canary runs: expected the canary's own history to stay empty, got {own:?}"
    );
}

/// A dry run's outcome is owed by the clock, exactly as a canary run's is, and the status
/// the dry-run reads report is read back off that run rather than fixed when it started.
#[tokio::test]
async fn a_dry_run_settles_on_the_clock_and_survives_stopping_the_canary() {
    let aws = Synthetics::new("ap-south-1");
    let name = "settling-dry-run";
    ready_canary(&aws, name).await;

    assert_eq!(
        aws.start(name).await.status,
        200,
        "case start: expected 200"
    );
    let started_at = std::time::Instant::now();
    let answer = start_dry_run(&aws, name, json!({})).await;
    let dry_run_id = minted_dry_run_id("while the canary runs", &answer);
    let read_within = started_at.elapsed();

    // A canary that is RUNNING is no obstacle to a dry run: the dry run is independent of
    // the canary's schedule and of its own runs.
    let open = reported_runs(
        &aws,
        name,
        json!({ "RunType": "DRY_RUN", "DryRunId": dry_run_id }),
    )
    .await;
    assert_eq!(
        open.len(),
        1,
        "case open dry run: expected its one run, got {open:?}"
    );
    // Guarded on elapsed wall-clock time: the outcome comes due a known span after the
    // start, so this is only asserted when the read provably happened before that.
    if read_within < RUN_STILL_OPEN_WITHIN {
        assert_eq!(
            open[0]["Status"]["State"].as_str(),
            Some("RUNNING"),
            "case open dry run (read within {read_within:?}): expected RUNNING, got {:?}",
            open[0]["Status"]
        );
        // Stopping the canary closes the canary's own run and leaves the dry run open.
        assert_eq!(aws.stop(name).await.status, 200, "case stop: expected 200");
        let still_open = reported_runs(
            &aws,
            name,
            json!({ "RunType": "DRY_RUN", "DryRunId": dry_run_id }),
        )
        .await;
        if started_at.elapsed() < RUN_STILL_OPEN_WITHIN {
            assert_eq!(
                still_open[0]["Status"]["State"].as_str(),
                Some("RUNNING"),
                "case stop does not stop the dry run: expected RUNNING, got {:?}",
                still_open[0]["Status"]
            );
        }
    }

    tokio::time::sleep(PAST_RUN_SETTLE).await;
    let settled = reported_runs(
        &aws,
        name,
        json!({ "RunType": "DRY_RUN", "DryRunId": dry_run_id }),
    )
    .await;
    let state = settled[0]["Status"]["State"].as_str().unwrap_or_default();
    assert_ne!(
        state, "RUNNING",
        "case settled dry run: expected the clock to have closed the run, got {:?}",
        settled[0]["Status"]
    );
    assert!(
        settled[0]["Timeline"]["Completed"].is_number(),
        "case settled dry run: expected a completion instant, got {:?}",
        settled[0]["Timeline"]
    );
    // `LastDryRunExecutionStatus` is read off that run, so it moved with it.
    let reported = get_for_dry_run(&aws, name, &dry_run_id).await;
    assert_eq!(
        reported.at("Canary.DryRunConfig.LastDryRunExecutionStatus").as_str(),
        Some(state),
        "case settled dry run: expected the status of the run itself ({state:?}), got {:?}",
        reported.at("Canary.DryRunConfig")
    );
}

/// What a dry run refuses: a canary that does not exist, a configuration the service's own
/// prose rules reject, and the members this emulator cannot honour.
#[tokio::test]
async fn a_dry_run_refuses_a_missing_canary_and_a_configuration_the_rules_reject() {
    let aws = Synthetics::new("eu-north-1");
    let name = "refusing-dry-run";
    ready_canary(&aws, name).await;

    let missing = start_dry_run(&aws, "no-such-canary", json!({})).await;
    assert_eq!(
        missing.status, 404,
        "case missing canary: expected 404, got {} ({:?})",
        missing.status, missing.body
    );
    assert_eq!(
        missing.error_code.as_deref(),
        Some("ResourceNotFoundException"),
        "case missing canary: expected ResourceNotFoundException, got {:?}",
        missing.error_code
    );

    // Each row breaks one rule the model states in prose: a runtime version outside the
    // service's published catalogue, an artifact location that is not an S3 location, a
    // memory size that is not a multiple of 64, and the visual-reference members, whose
    // screenshots this emulator has none of.
    let rows = [
        (
            "unpublished runtime version",
            json!({ "RuntimeVersion": "syn-nodejs-puppeteer-99.0" }),
        ),
        (
            "runtime version that is not one at all",
            json!({ "RuntimeVersion": "nodejs18.x" }),
        ),
        (
            "artifact location without a scheme",
            json!({ "ArtifactS3Location": "artifacts/canary" }),
        ),
        (
            "artifact bucket with a period",
            json!({ "ArtifactS3Location": "s3://my.bucket/canary" }),
        ),
        (
            "memory that is not a multiple of 64",
            json!({ "RunConfig": { "MemoryInMB": 1000 } }),
        ),
        (
            "visual reference",
            json!({ "VisualReference": { "BaseCanaryRunId": "nextrun" } }),
        ),
        (
            "visual references",
            json!({ "VisualReferences": [{ "BaseCanaryRunId": "nextrun" }] }),
        ),
    ];
    for (case, body) in rows {
        let answer = start_dry_run(&aws, name, body.clone()).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected 400, got {} ({:?})",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code.as_deref(),
            Some("ValidationException"),
            "case {case}: expected ValidationException, got {:?}",
            answer.error_code
        );
    }

    // A refused dry run wrote nothing: the canary has no dry runs at all.
    let dry_runs = reported_runs(&aws, name, json!({ "RunType": "DRY_RUN" })).await;
    assert!(
        dry_runs.is_empty(),
        "case refusals persist nothing: expected no dry runs, got {dry_runs:?}"
    );
}

/// A dry run belongs to the canary it was started on: an id that canary has no dry run for
/// — never minted, or minted for another canary — resolves to nothing, through every read
/// that takes one.
#[tokio::test]
async fn a_dry_run_id_another_canary_owns_resolves_to_nothing() {
    let aws = Synthetics::new("ca-central-1");
    let owner = "dry-run-owner";
    let other = "dry-run-other";
    ready_canary(&aws, owner).await;
    ready_canary(&aws, other).await;

    let mine = minted_dry_run_id("owner", &start_dry_run(&aws, owner, json!({})).await);

    // Every row is a legal `UUID` that `other` has no dry run for: two this emulator never
    // minted, and one it minted for a different canary.
    let rows = [
        ("never minted, all zero", "00000000-0000-4000-8000-000000000000".to_string()),
        ("never minted, mixed", "1a2b3c4d-5e6f-4a7b-8c9d-0e1f2a3b4c5d".to_string()),
        ("minted for another canary", mine.clone()),
    ];
    for (case, id) in rows {
        let read = get_for_dry_run(&aws, other, &id).await;
        assert_eq!(
            read.status, 404,
            "case GetCanary/{case}: expected 404, got {} ({:?})",
            read.status, read.body
        );
        assert_eq!(
            read.at("Message").as_str(),
            Some(DRY_RUN_NOT_FOUND),
            "case GetCanary/{case}: expected the fixed missing-dry-run sentence, got {:?}",
            read.body
        );
        let runs = aws
            .runs(other, json!({ "RunType": "DRY_RUN", "DryRunId": id }))
            .await;
        assert_eq!(
            runs.status, 404,
            "case GetCanaryRuns/{case}: expected 404, got {} ({:?})",
            runs.status, runs.body
        );
    }

    // The owner still reads its own dry run back.
    let owned = get_for_dry_run(&aws, owner, &mine).await;
    assert_eq!(
        owned.at("Canary.DryRunConfig.DryRunId").as_str(),
        Some(mine.as_str()),
        "case owner reads its own dry run: expected {mine:?}, got {:?}",
        owned.body
    );
}

/// A canary whose provisioning is still in flight has no settled configuration to dry run
/// against, which is a conflict rather than a dry run.
#[tokio::test]
async fn a_dry_run_of_a_canary_that_is_still_being_created_conflicts() {
    let aws = Synthetics::new("us-east-2");
    let name = "unprovisioned-canary";
    let created = aws.create(create_body(name)).await;
    assert_eq!(
        created.status, 200,
        "case create: expected 200, got {} ({:?})",
        created.status, created.body
    );
    assert_eq!(
        created.at("Canary.Status.State").as_str(),
        Some("CREATING"),
        "case create: expected a freshly created canary to still be provisioning, got {:?}",
        created.at("Canary.Status")
    );
    // The provisioning transition is owed by the clock, so a caller acting immediately
    // still sees CREATING and the conflict that state implies.
    let answer = start_dry_run(&aws, name, json!({})).await;
    assert_eq!(
        answer.status, 409,
        "case dry run while creating: expected 409, got {} ({:?})",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code.as_deref(),
        Some("ConflictException"),
        "case dry run while creating: expected ConflictException, got {:?}",
        answer.error_code
    );
    // Nothing was written: a refused dry run leaves the canary with no dry runs.
    tokio::time::sleep(PAST_PROVISIONING).await;
    let none_yet = reported_runs(&aws, name, json!({ "RunType": "DRY_RUN" })).await;
    assert!(
        none_yet.is_empty(),
        "case dry run while creating: expected no dry run to have been recorded, got \
         {none_yet:?}"
    );

    // Once the canary is provisioned the same request is served.
    let answer = start_dry_run(&aws, name, json!({})).await;
    minted_dry_run_id("dry run once ready", &answer);
}
