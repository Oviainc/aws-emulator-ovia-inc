// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the SHAPE of what CodeBuild's reads answer with, driven over
//! the wire the router serves (`POST /` with
//! `X-Amz-Target: CodeBuild_20161006.<Operation>` on awsJson1_1) through the harness
//! [`super::project_build_lifecycle`] owns.
//!
//! Three rules are under test, each over a table that spans the class it generalises
//! over rather than the one request an issue happened to record:
//!
//! 1. A collection member that STATES THE RESULT of a read — a registry listing, a
//!    batch read's found/not-found partition, a resource's tag set — is always
//!    serialized, `[]` when it has no entry. The `@length(min: 1)` the model puts on
//!    those list shapes bounds what a REQUEST may carry, not whether an answer states
//!    its result.
//! 2. A started build or batch build reports the configuration it RUNS WITH: a member
//!    AWS documents a default for is resolved at start time, and a collection it takes
//!    from the project is stated even when the project names none.
//! 3. A constraint-violation message renders the offending value in the wire form of
//!    its member's modelled type, so a floating-point member's value carries a
//!    fractional part and an integral member's does not.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::project_build_lifecycle::{
    batch_enabled, call, create_project, emulator, expected_arn, merged, REGION,
};

/// The KMS key a run with no `encryptionKey` configured reports, from the default AWS
/// documents on `Project$encryptionKey`: "If you don't specify a value, CodeBuild uses
/// the managed CMK for Amazon Simple Storage Service (Amazon S3)."
///
/// `pub(crate)`: the project-configuration suite asserts the same resolved default on
/// the stored project a run of it reads its configuration from.
pub(crate) fn managed_s3_key() -> String {
    format!(
        "arn:aws:kms:{REGION}:{}:alias/aws/s3",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// The value at a dotted `path` of `body`, or `Value::Null` when the path is absent. A
/// segment that parses as a number indexes a list, so a path can reach into one.
///
/// `pub(crate)`: shared with the sibling suites that assert member paths of the same
/// records.
pub(crate) fn at(body: &Value, path: &str) -> Value {
    let mut reported = body;
    for segment in path.split('.') {
        reported = match segment.parse::<usize>() {
            Ok(index) => &reported[index],
            Err(_) => &reported[segment],
        };
    }
    reported.clone()
}

/// Call `operation`, asserting it answered 200, and hand back the body.
async fn read(router: &MultiServiceRouter, operation: &str, request: Value) -> Value {
    let (status, body) = call(router, operation, request.clone()).await;
    assert_eq!(
        status, 200,
        "{operation} {request}: expected 200, got {status} with {body}"
    );
    body
}

/// RULE 1: every read's result collections are serialized whether or not they hold an
/// entry.
///
/// The table spans the classes the rule generalises over: the account-wide and
/// per-project registries, both halves of every implemented batch read's partition,
/// and the delete partition — each exercised twice, once against an account that holds
/// NOTHING (so every list is empty) and once against one that holds a project, a
/// build, a batch build, a report group and a fleet (so the found half is non-empty and
/// the not-found half is empty). The operation names come from the service's own
/// `OPERATIONS.md` surface.
#[tokio::test]
async fn result_collections_are_always_stated_even_when_empty() {
    // (case, the account's contents before the reads)
    for case in ["an empty account", "an account holding one of each resource"] {
        let router = emulator();
        let mut held_build = "no-such-project:00000000-0000-8000-8000-000000000000".to_string();
        let mut held_batch = held_build.clone();
        if case != "an empty account" {
            create_project(&router, "shape-proj", batch_enabled()).await;
            let started = read(&router, "StartBuild", json!({"projectName": "shape-proj"})).await;
            held_build = started["build"]["id"]
                .as_str()
                .unwrap_or_else(|| panic!("StartBuild must report an id, got {started}"))
                .to_string();
            let batch = read(
                &router,
                "StartBuildBatch",
                json!({"projectName": "shape-proj"}),
            )
            .await;
            held_batch = batch["buildBatch"]["id"]
                .as_str()
                .unwrap_or_else(|| panic!("StartBuildBatch must report an id, got {batch}"))
                .to_string();
            read(
                &router,
                "CreateReportGroup",
                json!({
                    "name": "shape-group",
                    "type": "TEST",
                    "exportConfig": {"exportConfigType": "NO_EXPORT"},
                }),
            )
            .await;
            read(
                &router,
                "CreateFleet",
                json!({
                    "name": "shape-fleet",
                    "baseCapacity": 2,
                    "environmentType": "LINUX_CONTAINER",
                    "computeType": "BUILD_GENERAL1_SMALL",
                }),
            )
            .await;
        }

        // (operation, request, the result members that must be lists)
        let reads: Vec<(&str, Value, &[&str])> = vec![
            ("ListBuilds", json!({}), &["ids"]),
            ("ListProjects", json!({}), &["projects"]),
            (
                "ListBuildsForProject",
                json!({"projectName": "shape-proj"}),
                &["ids"],
            ),
            ("ListSandboxes", json!({}), &["ids"]),
            ("ListSourceCredentials", json!({}), &["sourceCredentialsInfos"]),
            (
                "BatchGetProjects",
                json!({"names": ["shape-proj"]}),
                &["projects", "projectsNotFound"],
            ),
            (
                "BatchGetBuilds",
                json!({"ids": [held_build.clone()]}),
                &["builds", "buildsNotFound"],
            ),
            (
                "BatchGetBuildBatches",
                json!({"ids": [held_batch.clone()]}),
                &["buildBatches", "buildBatchesNotFound"],
            ),
            (
                "BatchGetReportGroups",
                json!({"reportGroupArns": [expected_arn("report-group/shape-group")]}),
                &["reportGroups", "reportGroupsNotFound"],
            ),
            (
                "BatchGetFleets",
                json!({"names": ["shape-fleet"]}),
                &["fleets", "fleetsNotFound"],
            ),
            (
                "BatchGetSandboxes",
                json!({"ids": ["shape-proj:never-started"]}),
                &["sandboxes", "sandboxesNotFound"],
            ),
        ];

        for (operation, request, members) in reads {
            // `ListBuildsForProject` needs a project to list; on the empty account
            // there is none, and the operation's own modelled not-found answer is
            // another issue's subject.
            if case == "an empty account" && operation == "ListBuildsForProject" {
                continue;
            }
            let body = read(&router, operation, request.clone()).await;
            for member in members {
                assert!(
                    body[*member].is_array(),
                    "{case} / {operation}: {member} must be stated as a list, got {body}"
                );
            }
        }

        // The delete partition answers the same way, so it is driven last: it removes
        // what the reads above were asserted against.
        let deleted = read(
            &router,
            "BatchDeleteBuilds",
            json!({"ids": [held_build.clone()]}),
        )
        .await;
        for member in ["buildsDeleted", "buildsNotDeleted"] {
            assert!(
                deleted[member].is_array(),
                "{case} / BatchDeleteBuilds: {member} must be stated as a list, got {deleted}"
            );
        }
    }
}

/// RULE 1, for the not-found half specifically: an identifier a batch read cannot
/// resolve is reported in the not-found half, and the found half is still stated.
///
/// The rows differ along the identifier's form — a name, an ARN, an id — one per
/// implemented batch read.
#[tokio::test]
async fn an_unresolvable_identifier_lands_in_the_not_found_half() {
    let router = emulator();

    // (operation, request, the found member, the not-found member, what must be reported missing)
    let rows: Vec<(&str, Value, &str, &str, Value)> = vec![
        (
            "BatchGetProjects",
            json!({"names": ["never-created"]}),
            "projects",
            "projectsNotFound",
            json!(["never-created"]),
        ),
        (
            "BatchGetBuilds",
            json!({"ids": ["never-created:00000000-0000-8000-8000-000000000000"]}),
            "builds",
            "buildsNotFound",
            json!(["never-created:00000000-0000-8000-8000-000000000000"]),
        ),
        (
            "BatchGetBuildBatches",
            json!({"ids": ["never-created:00000000-0000-8000-8000-000000000001"]}),
            "buildBatches",
            "buildBatchesNotFound",
            json!(["never-created:00000000-0000-8000-8000-000000000001"]),
        ),
        (
            "BatchGetReportGroups",
            json!({"reportGroupArns": [expected_arn("report-group/never-created")]}),
            "reportGroups",
            "reportGroupsNotFound",
            json!([expected_arn("report-group/never-created")]),
        ),
        (
            "BatchGetSandboxes",
            json!({"ids": ["never-created:1"]}),
            "sandboxes",
            "sandboxesNotFound",
            json!(["never-created:1"]),
        ),
    ];

    for (operation, request, found, not_found, missing) in rows {
        let body = read(&router, operation, request.clone()).await;
        assert_eq!(
            body[found],
            json!([]),
            "{operation}: nothing resolved, so {found} must be [], got {body}"
        );
        assert_eq!(
            body[not_found], missing,
            "{operation}: {not_found} must report the identifier, got {body}"
        );
    }

    // `BatchGetFleets` partitions on the identifier's FORM: a fleet's ARN carries the
    // fleet's own id (`fleet/<name>:<id>`), so the service can report back an ARN it
    // could not find but has no ARN to report an unheld NAME by — which is why a probe
    // for an absent fleet name is answered with a plain empty result.
    let absent_arn = format!(
        "{}:00000000-0000-8000-8000-000000000000",
        expected_arn("fleet/never-created")
    );
    // (case, identifier, what fleetsNotFound must hold)
    let fleet_rows: Vec<(&str, String, Value)> = vec![
        ("a name no fleet is held under", "never-created".to_string(), json!([])),
        ("a fleet ARN", absent_arn.clone(), json!([absent_arn])),
        (
            "an ARN of another resource type",
            expected_arn("project/never-created"),
            json!([expected_arn("project/never-created")]),
        ),
    ];
    for (case, identifier, expected) in fleet_rows {
        let body = read(&router, "BatchGetFleets", json!({"names": [identifier]})).await;
        assert_eq!(
            body["fleets"],
            json!([]),
            "{case}: no fleet resolved, so fleets must be [], got {body}"
        );
        assert_eq!(
            body["fleetsNotFound"], expected,
            "{case}: expected fleetsNotFound {expected}, got {body}"
        );
    }
}

/// RULE 1, for a resource's tag set: the set is stated as it is held, `[]` when the
/// create named no tag.
///
/// The rows span the taggable resources this service creates and the tag lists
/// `TagList`'s `@length(min: 0, max: 50)` allows at the interesting counts: no `tags`
/// member at all, an explicitly empty list, one tag and several.
#[tokio::test]
async fn a_resources_tag_set_is_stated_as_held_and_empty_when_it_holds_none() {
    // (case, what the request carried for tags, what a read must report)
    let tag_cases: Vec<(&str, Option<Value>, Value)> = vec![
        ("no tags member at all", None, json!([])),
        ("an explicitly empty list", Some(json!([])), json!([])),
        (
            "one tag",
            Some(json!([{"key": "team", "value": "platform"}])),
            json!([{"key": "team", "value": "platform"}]),
        ),
        (
            "several tags",
            Some(json!([
                {"key": "team", "value": "platform"},
                {"key": "env", "value": "test"},
            ])),
            json!([
                {"key": "team", "value": "platform"},
                {"key": "env", "value": "test"},
            ]),
        ),
    ];

    for (index, (case, sent, expected)) in tag_cases.into_iter().enumerate() {
        let router = emulator();
        let name = format!("tagged-{index}");

        // A project, read back through BatchGetProjects.
        let mut extra = json!({});
        if let (Value::Object(target), Some(tags)) = (&mut extra, sent.clone()) {
            target.insert("tags".to_string(), tags);
        }
        let project = create_project(&router, &name, extra).await;
        assert_eq!(
            project["tags"], expected,
            "{case}: CreateProject must report the tag set it holds, got {project}"
        );
        let fetched = read(&router, "BatchGetProjects", json!({"names": [name]})).await;
        assert_eq!(
            fetched["projects"][0]["tags"], expected,
            "{case}: BatchGetProjects must report the stored tag set, got {fetched}"
        );

        // A report group, read back through BatchGetReportGroups.
        let mut group_request = json!({
            "name": name,
            "type": "TEST",
            "exportConfig": {"exportConfigType": "NO_EXPORT"},
        });
        if let (Value::Object(target), Some(tags)) = (&mut group_request, sent.clone()) {
            target.insert("tags".to_string(), tags);
        }
        let created = read(&router, "CreateReportGroup", group_request).await;
        assert_eq!(
            created["reportGroup"]["tags"], expected,
            "{case}: CreateReportGroup must report the tag set it holds, got {created}"
        );
        let fetched = read(
            &router,
            "BatchGetReportGroups",
            json!({"reportGroupArns": [expected_arn(&format!("report-group/{name}"))]}),
        )
        .await;
        assert_eq!(
            fetched["reportGroups"][0]["tags"], expected,
            "{case}: BatchGetReportGroups must report the stored tag set, got {fetched}"
        );

        // A compute fleet, read back through BatchGetFleets.
        let mut fleet_request = json!({
            "name": name,
            "baseCapacity": 2,
            "environmentType": "LINUX_CONTAINER",
            "computeType": "BUILD_GENERAL1_SMALL",
        });
        if let (Value::Object(target), Some(tags)) = (&mut fleet_request, sent) {
            target.insert("tags".to_string(), tags);
        }
        let created = read(&router, "CreateFleet", fleet_request).await;
        assert_eq!(
            created["fleet"]["tags"], expected,
            "{case}: CreateFleet must report the tag set it holds, got {created}"
        );
        let fetched = read(&router, "BatchGetFleets", json!({"names": [name]})).await;
        assert_eq!(
            fetched["fleets"][0]["tags"], expected,
            "{case}: BatchGetFleets must report the stored tag set, got {fetched}"
        );
    }
}

/// RULE 2: a started build and a started batch build report the configuration they RUN
/// WITH — the project's value where it configured one, and the value AWS documents as
/// the default where it did not — and a later read reports the same record.
///
/// The rows span the classes the rule generalises over: a project that configures none
/// of these members, one that configures each of them, and the published values of the
/// enums involved (`CacheType`: NO_CACHE | S3 | LOCAL and `ImagePullCredentialsType`:
/// CODEBUILD | SERVICE_ROLE, from
/// `smithy_query(query="shape",shape="ProjectCache,ProjectEnvironment")`).
#[tokio::test]
async fn a_started_run_reports_the_configuration_it_runs_with() {
    /// One row: the case's name, the project's extra configuration, and the
    /// (member path, effective value) pairs a run of that project must report.
    type Row = (&'static str, Value, Vec<(&'static str, Value)>);

    // (case, the project's extra configuration, the effective values a run reports)
    let rows: Vec<Row> = vec![
        (
            "a project that configures none of them",
            json!({}),
            vec![
                ("cache.type", json!("NO_CACHE")),
                ("encryptionKey", json!(managed_s3_key())),
                ("environment.privilegedMode", json!(false)),
                ("environment.imagePullCredentialsType", json!("CODEBUILD")),
                ("environment.environmentVariables", json!([])),
                ("source.insecureSsl", json!(false)),
                ("secondarySources", json!([])),
                ("secondarySourceVersions", json!([])),
            ],
        ),
        (
            "an S3 cache and a customer key",
            json!({
                "cache": {"type": "S3", "location": "cache-bucket/prefix"},
                "encryptionKey": "arn:aws:kms:us-west-2:000000000000:key/11111111-1111-1111-1111-111111111111",
            }),
            vec![
                ("cache.type", json!("S3")),
                ("cache.location", json!("cache-bucket/prefix")),
                (
                    "encryptionKey",
                    json!("arn:aws:kms:us-west-2:000000000000:key/11111111-1111-1111-1111-111111111111"),
                ),
            ],
        ),
        (
            "a LOCAL cache with modes",
            json!({"cache": {"type": "LOCAL", "modes": ["LOCAL_SOURCE_CACHE"]}}),
            vec![
                ("cache.type", json!("LOCAL")),
                ("cache.modes", json!(["LOCAL_SOURCE_CACHE"])),
            ],
        ),
        (
            "a privileged environment pulling with the service role",
            json!({
                "environment": {
                    "type": "LINUX_CONTAINER",
                    "image": "aws/codebuild/standard:7.0",
                    "computeType": "BUILD_GENERAL1_SMALL",
                    "privilegedMode": true,
                    "imagePullCredentialsType": "SERVICE_ROLE",
                    "environmentVariables": [{"name": "FOO", "value": "bar"}],
                },
            }),
            vec![
                ("environment.privilegedMode", json!(true)),
                ("environment.imagePullCredentialsType", json!("SERVICE_ROLE")),
                (
                    "environment.environmentVariables",
                    json!([{"name": "FOO", "value": "bar"}]),
                ),
            ],
        ),
        (
            "a source that ignores SSL warnings, plus a secondary source",
            json!({
                "source": {
                    "type": "GITHUB",
                    "location": "https://github.com/example/repo",
                    "insecureSsl": true,
                },
                "secondarySources": [{
                    "type": "GITHUB",
                    "location": "https://github.com/example/second",
                    "sourceIdentifier": "second",
                }],
                "secondarySourceVersions": [
                    {"sourceIdentifier": "second", "sourceVersion": "main"},
                ],
            }),
            vec![
                ("source.insecureSsl", json!(true)),
                // A secondary source reports its flags resolved the same way the
                // primary one does.
                ("secondarySources.0.insecureSsl", json!(false)),
                ("secondarySources.0.sourceIdentifier", json!("second")),
                (
                    "secondarySourceVersions",
                    json!([{"sourceIdentifier": "second", "sourceVersion": "main"}]),
                ),
            ],
        ),
    ];

    for (index, (case, configuration, effective)) in rows.into_iter().enumerate() {
        let router = emulator();
        let project = format!("run-config-{index}");
        create_project(&router, &project, merged(configuration, batch_enabled())).await;

        // (what starts the run, the member its answer carries, the read that reports
        // it back, the request member and the member the read's answer carries)
        let runs: [(&str, &str, &str, &str, &str); 2] = [
            ("StartBuild", "build", "BatchGetBuilds", "ids", "builds"),
            (
                "StartBuildBatch",
                "buildBatch",
                "BatchGetBuildBatches",
                "ids",
                "buildBatches",
            ),
        ];
        for (start, started_member, reader, request_member, read_member) in runs {
            let started = read(&router, start, json!({"projectName": project})).await;
            let record = started[started_member].clone();
            for (path, expected) in &effective {
                assert_eq!(
                    &at(&record, path),
                    expected,
                    "{case} / {start}: {path} must report {expected}, got {record}"
                );
            }

            // The run is PERSISTED with those values: the read reports the same record.
            let id = record["id"]
                .as_str()
                .unwrap_or_else(|| panic!("{case} / {start} must report an id, got {record}"))
                .to_string();
            let fetched = read(&router, reader, json!({request_member: [id]})).await;
            assert_eq!(
                fetched[read_member][0], record,
                "{case} / {reader} must report the stored record verbatim, got {fetched}"
            );
        }
    }
}

/// RULE 3: the value a constraint violation echoes is rendered in the wire form of its
/// member's modelled type — a `double` always carries a fractional part, an `integer`
/// never does — inside the envelope AWS words it with.
///
/// The rows span both of `DescribeCodeCoverages`' floating-point members (each
/// targeting `Percentage`, a `double` with `@range(min: 0, max: 100)`) and its integral
/// `maxResults` (`PageSize`, `@range(min: 1, max: 100)`), from
/// `smithy_query(query="operation",operation="DescribeCodeCoverages")`; the values take
/// each published bound, one past each bound, and a fractional value past the bound.
#[tokio::test]
async fn a_constraint_violation_renders_the_value_in_its_members_wire_type() {
    let router = emulator();
    let report_arn = expected_arn("report/coverage-report:1");

    // (case, member, the JSON value sent, the clause the refusal must state)
    let refused: Vec<(&str, &str, Value, String)> = vec![
        (
            "a double one past its upper bound",
            "minLineCoveragePercentage",
            json!(150),
            "Value '150.0' at 'minLineCoveragePercentage' failed to satisfy constraint: \
             Member must have value less than or equal to 100"
                .to_string(),
        ),
        (
            "a double just past its upper bound",
            "minLineCoveragePercentage",
            json!(100.5),
            "Value '100.5' at 'minLineCoveragePercentage' failed to satisfy constraint: \
             Member must have value less than or equal to 100"
                .to_string(),
        ),
        (
            "a double just below its lower bound",
            "minLineCoveragePercentage",
            json!(-0.5),
            "Value '-0.5' at 'minLineCoveragePercentage' failed to satisfy constraint: \
             Member must have value greater than or equal to 0"
                .to_string(),
        ),
        (
            "the other double member, one past its upper bound",
            "maxLineCoveragePercentage",
            json!(101),
            "Value '101.0' at 'maxLineCoveragePercentage' failed to satisfy constraint: \
             Member must have value less than or equal to 100"
                .to_string(),
        ),
        (
            "an integral member one past its upper bound renders no point",
            "maxResults",
            json!(101),
            "Value '101' at 'maxResults' failed to satisfy constraint: \
             Member must have value less than or equal to 100"
                .to_string(),
        ),
        (
            "an integral member one below its lower bound renders no point",
            "maxResults",
            json!(0),
            "Value '0' at 'maxResults' failed to satisfy constraint: \
             Member must have value greater than or equal to 1"
                .to_string(),
        ),
    ];

    for (case, member, sent, clause) in refused {
        let request = json!({"reportArn": report_arn, member: sent});
        let (status, body) = call(&router, "DescribeCodeCoverages", request.clone()).await;
        assert_eq!(
            status, 400,
            "{case}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            body["__type"], "InvalidInputException",
            "{case}: CodeBuild states a bad input as InvalidInputException, got {body}"
        );
        assert_eq!(
            body["message"],
            json!(format!("1 validation error detected: {clause}")),
            "{case}: unexpected refusal message, got {body}"
        );
    }

    // Values INSIDE the range are not refused at all — the rendering above is a
    // property of the refusal, not a refusal of every numeric member. The operation
    // itself is still scaffolded, so a request that satisfies every constraint reaches
    // the 501 channel rather than a 400.
    for inside in [json!(0), json!(100), json!(50.5)] {
        let request = json!({"reportArn": report_arn, "minLineCoveragePercentage": inside});
        let (status, body) = call(&router, "DescribeCodeCoverages", request.clone()).await;
        assert_ne!(
            status, 400,
            "{inside} is inside the published range, so it must not be refused, got {body}"
        );
        assert!(
            !body["message"]
                .as_str()
                .unwrap_or_default()
                .contains("validation error detected"),
            "{inside} is inside the published range, so no validation clause may be \
             stated, got {body}"
        );
    }
}
