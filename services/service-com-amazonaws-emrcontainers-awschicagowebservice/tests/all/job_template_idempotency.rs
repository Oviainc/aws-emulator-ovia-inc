// SPDX-License-Identifier: CC0-1.0

//! `CreateJobTemplate` and the `@idempotencyToken` contract on its `clientToken`, end to
//! end.
//!
//! Every call here goes through the production path: the service is mounted in the real
//! `MultiServiceRouter` (the harness `crate::virtual_cluster_job_run_loop::Emr` builds)
//! and driven with restJson1 HTTP requests, so routing, deserialization, constraint
//! validation, the handler and serialization all run exactly as they do for an AWS SDK
//! client.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one recorded
//! request:
//!
//! 1. A `clientToken` already spent on this operation does not mint a second resource:
//!    N callers issuing ONE identical create — concurrently or in sequence — all answer
//!    HTTP 200 with the SAME id, and exactly one template is persisted. The reported case
//!    (8 racers) is one row.
//! 2. The same token carrying a DIFFERENT request is not a replay, and is refused with
//!    the modelled `ValidationException` (HTTP 400) rather than answered with the first
//!    template.
//! 3. Distinct tokens mint distinct templates, even for byte-identical parameters.
//! 4. What `CreateJobTemplate` persisted is what `DescribeJobTemplate` and
//!    `ListJobTemplates` answer: every member the request carried comes back and the
//!    ones it omitted stay absent.
//! 5. `DeleteJobTemplate` removes the record, so the token that minted it is spendable
//!    again.
//! 6. `@paginated` paging walks the stored set with no repeats and no gaps.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};

use crate::virtual_cluster_job_run_loop::{
    assert_resource_id, field, instant_millis, text, Answer, Emr, REGION,
};

/// `POST /jobtemplates` — `CreateJobTemplate`.
async fn create_template(emr: &Emr, body: Value) -> Answer {
    emr.send("POST", "/jobtemplates", Some(body)).await
}

/// `GET /jobtemplates/{id}` — `DescribeJobTemplate`.
async fn describe_template(emr: &Emr, id: &str) -> Answer {
    emr.send("GET", &format!("/jobtemplates/{id}"), None).await
}

/// `DELETE /jobtemplates/{id}` — `DeleteJobTemplate`.
async fn delete_template(emr: &Emr, id: &str) -> Answer {
    emr.send("DELETE", &format!("/jobtemplates/{id}"), None)
        .await
}

/// `GET /jobtemplates{query}` — `ListJobTemplates`.
async fn list_templates(emr: &Emr, query: &str) -> Answer {
    emr.send("GET", &format!("/jobtemplates{query}"), None).await
}

/// The ids `ListJobTemplates` reports, in the order it reported them.
async fn listed_ids(emr: &Emr, query: &str, case: &str) -> Vec<String> {
    let answer = list_templates(emr, query).await;
    let body = answer.ok(case);
    field(body, "/templates", case)
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: templates is not a list: {body}"))
        .iter()
        .map(|template| text(template, "/id", case))
        .collect()
}

/// The smallest `jobTemplateData` the model accepts: `executionRoleArn`, `releaseLabel`
/// and `jobDriver` are all `@required` (smithy_query: shape `JobTemplateData`).
fn minimal_data() -> Value {
    json!({
        "executionRoleArn": "arn:aws:iam::000000000000:role/EMRJobRole",
        "releaseLabel": "emr-6.15.0-latest",
        "jobDriver": {"sparkSubmitJobDriver": {"entryPoint": "s3://bucket/a.py"}},
    })
}

/// A `jobTemplateData` that exercises every nested member the shape has: a SparkSQL
/// driver, parametric configuration overrides (including a nested `Configuration`),
/// declared template parameters and job tags.
fn deep_data() -> Value {
    json!({
        "executionRoleArn": "arn:aws:iam::000000000000:role/DeepRole",
        "releaseLabel": "${releaseVersion}",
        "jobDriver": {
            "sparkSqlJobDriver": {
                "entryPoint": "s3://bucket/query.sql",
                "sparkSqlParameters": "--conf spark.sql.shuffle.partitions=10",
            },
        },
        "configurationOverrides": {
            "applicationConfiguration": [{
                "classification": "spark-defaults",
                "properties": {"spark.executor.memory": "2G"},
                "configurations": [{
                    "classification": "nested",
                    "properties": {"a": "1", "b": "2"},
                }],
            }],
            "monitoringConfiguration": {
                "persistentAppUI": "ENABLED",
                "cloudWatchMonitoringConfiguration": {
                    "logGroupName": "/emr-containers/jobs",
                    "logStreamNamePrefix": "deep",
                },
                "s3MonitoringConfiguration": {"logUri": "s3://bucket/logs"},
            },
        },
        "parameterConfiguration": {
            "releaseVersion": {"type": "STRING", "defaultValue": "emr-6.15.0-latest"},
            "rows": {"type": "NUMBER"},
        },
        "jobTags": {"pipeline": "nightly"},
    })
}

/// One row of the dedup table: a `CreateJobTemplate` request, and how many callers fire
/// that exact request at the same time.
struct RaceCase {
    label: &'static str,
    racers: usize,
    body: Value,
}

/// The dedup rows. They differ along everything the rule generalises over: how many
/// callers race (1, 2, 3, 5 and the reported 8), the form of the `name` and the
/// `clientToken` (including the 64-character bound `ClientToken` declares), and which
/// optional members the body carries — none, `tags`, `kmsKeyArn`, and the whole nested
/// `jobTemplateData` tree.
fn race_cases() -> Vec<RaceCase> {
    vec![
        RaceCase {
            label: "one caller, only the required members",
            racers: 1,
            body: json!({
                "name": "solo",
                "clientToken": "tok-solo",
                "jobTemplateData": minimal_data(),
            }),
        },
        RaceCase {
            label: "two racers, only the required members",
            racers: 2,
            body: json!({
                "name": "pair",
                "clientToken": "tok-pair",
                "jobTemplateData": minimal_data(),
            }),
        },
        RaceCase {
            label: "eight racers, the reported case",
            racers: 8,
            body: json!({
                "name": "racy",
                "clientToken": "tok-racy",
                "jobTemplateData": minimal_data(),
            }),
        },
        RaceCase {
            label: "five racers, tags and a kms key",
            racers: 5,
            body: json!({
                "name": "tagged.template_1#x",
                "clientToken": "tok/tagged.1-a_b",
                "jobTemplateData": minimal_data(),
                "tags": {"team": "data", "env": "test"},
                "kmsKeyArn": "arn:aws:kms:us-west-2:000000000000:key/abc-123",
            }),
        },
        RaceCase {
            label: "three racers, the whole nested jobTemplateData tree",
            racers: 3,
            body: json!({
                "name": "deep/name",
                "clientToken": "tok-deep",
                "jobTemplateData": deep_data(),
            }),
        },
        RaceCase {
            label: "four racers, a clientToken at the 64-character bound",
            racers: 4,
            body: json!({
                "name": "bounded",
                "clientToken": "t".repeat(64),
                "jobTemplateData": minimal_data(),
            }),
        },
    ]
}

/// RULE 1: `clientToken` is `@idempotencyToken`, so N callers issuing ONE identical
/// `CreateJobTemplate` concurrently all succeed with the SAME id and leave exactly one
/// template in the store.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_identical_creates_collapse_to_one_job_template() {
    for case in race_cases() {
        let RaceCase {
            label,
            racers,
            body,
        } = case;
        // One emulator per row, so a row's own store is all that is counted.
        let emr = Arc::new(Emr::new());

        let mut tasks = Vec::new();
        for racer in 0..racers {
            let emr = Arc::clone(&emr);
            let body = body.clone();
            tasks.push(tokio::spawn(async move {
                (racer, create_template(&emr, body).await)
            }));
        }

        let mut ids = BTreeSet::new();
        for task in tasks {
            let (racer, answer) = task.await.expect("the racing create task ran");
            assert_eq!(
                answer.status, 200,
                "case {label:?}: racer {racer} must win or share the winner's template, \
                 got status {} code {:?} body {}",
                answer.status, answer.error_code, answer.body
            );
            let id = text(&answer.body, "/id", label);
            assert_resource_id(label, "the minted job template id", &id);
            ids.insert(id);
        }

        assert_eq!(
            ids.len(),
            1,
            "case {label:?}: {racers} racers sharing one clientToken must share one id, got {ids:?}"
        );
        let shared = ids.iter().next().expect("one id").clone();

        // Exactly ONE template is persisted, and it is the one every racer was told
        // about: the dedup is in the store, not only in the answer.
        let listed = listed_ids(&emr, "", label).await;
        assert_eq!(
            listed,
            vec![shared.clone()],
            "case {label:?}: expected exactly one stored template after the race, got {listed:?}"
        );

        let described = describe_template(&emr, &shared).await;
        let template = field(described.ok(label), "/jobTemplate", label).clone();
        assert_eq!(
            text(&template, "/name", label),
            text(&body, "/name", label),
            "case {label:?}: the stored template does not carry the requested name"
        );
        assert_eq!(
            field(&template, "/jobTemplateData", label),
            field(&body, "/jobTemplateData", label),
            "case {label:?}: the stored jobTemplateData is not the submitted one"
        );
    }
}

/// RULE 1, reached sequentially: the same create REPLAYED any number of times answers
/// with the first template every time, which is what retry-on-timeout relies on.
#[tokio::test]
async fn a_replayed_create_answers_with_the_first_job_template() {
    // (case, how many times the identical create is issued)
    for (label, attempts) in [
        ("issued twice", 2usize),
        ("issued three times", 3),
        ("issued six times", 6),
    ] {
        let emr = Emr::new();
        let body = json!({
            "name": "replayed",
            "clientToken": "tok-replayed",
            "jobTemplateData": minimal_data(),
            "tags": {"k": "v"},
        });

        let mut ids = BTreeSet::new();
        for attempt in 0..attempts {
            let answer = create_template(&emr, body.clone()).await;
            let id = text(answer.ok(label), "/id", label);
            assert_resource_id(label, "the minted job template id", &id);
            ids.insert(id);
            assert_eq!(
                ids.len(),
                1,
                "case {label:?}: attempt {attempt} minted a second template, ids {ids:?}"
            );
        }

        let listed = listed_ids(&emr, "", label).await;
        assert_eq!(
            listed.len(),
            1,
            "case {label:?}: {attempts} identical creates must leave one template, got {listed:?}"
        );
    }
}

/// RULE 2: the same `clientToken` carrying a DIFFERENT request is not a replay. Answering
/// it with the first template would silently drop the parameters the second call asked
/// for, so it is refused with the modelled `ValidationException` (HTTP 400).
///
/// The rows differ in WHICH member differs, and cover every branch of the fingerprint:
/// a top-level member, each required member of `jobTemplateData`, a list's contents and
/// its ORDER, a nested `Configuration`'s properties, a declared template parameter, and
/// each of `tags` / `jobTags` / `kmsKeyArn`.
#[tokio::test]
async fn one_client_token_with_different_parameters_is_refused() {
    let base = json!({
        "name": "conflict",
        "clientToken": "tok-conflict",
        "jobTemplateData": {
            "executionRoleArn": "arn:aws:iam::000000000000:role/BaseRole",
            "releaseLabel": "emr-6.15.0-latest",
            "jobDriver": {
                "sparkSubmitJobDriver": {
                    "entryPoint": "s3://bucket/a.py",
                    "entryPointArguments": ["one", "two"],
                },
            },
            "parameterConfiguration": {"rows": {"type": "NUMBER", "defaultValue": "10"}},
            "jobTags": {"pipeline": "nightly"},
        },
        "tags": {"team": "data"},
        "kmsKeyArn": "arn:aws:kms:us-west-2:000000000000:key/abc-123",
    });

    // (case, JSON pointer into the body, the differing value)
    let cases: [(&str, &str, Value); 11] = [
        ("a different name", "/name", json!("conflict2")),
        (
            "a different executionRoleArn",
            "/jobTemplateData/executionRoleArn",
            json!("arn:aws:iam::000000000000:role/OtherRole"),
        ),
        (
            "a different releaseLabel",
            "/jobTemplateData/releaseLabel",
            json!("emr-7.0.0-latest"),
        ),
        (
            "a different jobDriver entryPoint",
            "/jobTemplateData/jobDriver/sparkSubmitJobDriver/entryPoint",
            json!("s3://bucket/b.py"),
        ),
        (
            "one more driver argument",
            "/jobTemplateData/jobDriver/sparkSubmitJobDriver/entryPointArguments",
            json!(["one", "two", "three"]),
        ),
        (
            "the driver arguments in the other order",
            "/jobTemplateData/jobDriver/sparkSubmitJobDriver/entryPointArguments",
            json!(["two", "one"]),
        ),
        (
            "a different union member on the driver",
            "/jobTemplateData/jobDriver",
            json!({"sparkSqlJobDriver": {"entryPoint": "s3://bucket/q.sql"}}),
        ),
        (
            "added configuration overrides",
            "/jobTemplateData/configurationOverrides",
            json!({"applicationConfiguration": [{
                "classification": "spark-defaults",
                "properties": {"spark.executor.memory": "2G"},
            }]}),
        ),
        (
            "a different template parameter default",
            "/jobTemplateData/parameterConfiguration",
            json!({"rows": {"type": "NUMBER", "defaultValue": "20"}}),
        ),
        (
            "a different jobTags entry",
            "/jobTemplateData/jobTags",
            json!({"pipeline": "hourly"}),
        ),
        (
            "a different kmsKeyArn",
            "/kmsKeyArn",
            json!("arn:aws:kms:us-west-2:000000000000:key/def-456"),
        ),
    ];

    for (label, pointer, differing) in cases {
        let emr = Emr::new();
        let first = create_template(&emr, base.clone()).await;
        let first_id = text(first.ok(label), "/id", label);

        let mut conflicting = base.clone();
        set_at(&mut conflicting, pointer, differing, label);
        create_template(&emr, conflicting)
            .await
            .refused(label, 400, "ValidationException");

        // The refusal changed nothing: the first template is still the one the token
        // names, and it is still the only one stored.
        let replay = create_template(&emr, base.clone()).await;
        assert_eq!(
            text(replay.ok(label), "/id", label),
            first_id,
            "case {label:?}: a refused conflict must leave the token naming the first template"
        );
        let listed = listed_ids(&emr, "", label).await;
        assert_eq!(
            listed,
            vec![first_id.clone()],
            "case {label:?}: a refused conflict must not persist a second template, got {listed:?}"
        );
    }

    // Absent and empty render the same way, because they persist the same record: a
    // second call that spells the same template differently is still a replay.
    let emr = Emr::new();
    let label = "an empty tag map beside an absent one";
    let without = json!({
        "name": "empties",
        "clientToken": "tok-empties",
        "jobTemplateData": minimal_data(),
    });
    let with_empty = json!({
        "name": "empties",
        "clientToken": "tok-empties",
        "jobTemplateData": minimal_data(),
        "tags": {},
    });
    let first_id = text(create_template(&emr, without).await.ok(label), "/id", label);
    assert_eq!(
        text(create_template(&emr, with_empty).await.ok(label), "/id", label),
        first_id,
        "case {label:?}: an absent tag map and an empty one must fingerprint alike"
    );
}

/// RULE 3: the token is what the dedup is keyed on, so DISTINCT tokens mint distinct
/// templates even when every other parameter is byte-identical.
#[tokio::test]
async fn distinct_client_tokens_mint_distinct_job_templates() {
    let emr = Emr::new();
    // Rows differ in the token's form; the rest of the request never changes.
    let tokens = [
        "tok-plain",
        "tok/punctuated.1-a_b",
        &"t".repeat(64)[..],
        "x",
        "\u{e9}-unicode",
    ];

    let mut ids = BTreeSet::new();
    for token in tokens {
        let label = format!("clientToken {token:?}");
        let answer = create_template(
            &emr,
            json!({
                "name": "shared.name",
                "clientToken": token,
                "jobTemplateData": minimal_data(),
            }),
        )
        .await;
        let id = text(answer.ok(&label), "/id", &label);
        assert_resource_id(&label, "the minted job template id", &id);
        assert!(
            ids.insert(id.clone()),
            "case {label:?}: id {id:?} was minted twice, so a distinct token reused a template"
        );
    }

    let listed = listed_ids(&emr, "", "distinct tokens").await;
    assert_eq!(
        listed.len(),
        tokens.len(),
        "each distinct clientToken must mint its own template, got {listed:?}"
    );
}

/// Set `pointer` in `body` to `value`, CREATING the member when the base body does not
/// carry it: a row that ADDS a member must be testable too, and `Value::pointer_mut`
/// only reaches members that already exist.
fn set_at(body: &mut Value, pointer: &str, value: Value, case: &str) {
    let mut cursor = body;
    let mut segments = pointer.split('/').skip(1).peekable();
    while let Some(segment) = segments.next() {
        let object = cursor
            .as_object_mut()
            .unwrap_or_else(|| panic!("case {case:?}: {pointer} does not name an object member"));
        if segments.peek().is_none() {
            object.insert(segment.to_string(), value);
            return;
        }
        cursor = object
            .entry(segment.to_string())
            .or_insert_with(|| json!({}));
    }
}

/// One round-trip row: a request body, and the members its describe must carry.
struct RoundTripCase {
    label: &'static str,
    body: Value,
}

/// RULE 4: what `CreateJobTemplate` persisted is what `DescribeJobTemplate` answers —
/// every member the request carried comes back, the ones it omitted stay absent, and the
/// `createdAt` and `arn` the create reported are the stored ones.
///
/// The rows differ along which optional members are present, the driver's union member,
/// the depth of the configuration tree and the number of declared parameters and tags.
#[tokio::test]
async fn a_created_job_template_is_described_from_state() {
    let emr = Emr::new();
    let cases = vec![
        RoundTripCase {
            label: "only the required members",
            body: json!({
                "name": "rt-minimal",
                "clientToken": "rt-tok-minimal",
                "jobTemplateData": minimal_data(),
            }),
        },
        RoundTripCase {
            label: "tags, a kms key and driver arguments",
            body: json!({
                "name": "rt-tagged",
                "clientToken": "rt-tok-tagged",
                "jobTemplateData": {
                    "executionRoleArn": "arn:aws:iam::000000000000:role/RtRole",
                    "releaseLabel": "emr-7.0.0-latest",
                    "jobDriver": {"sparkSubmitJobDriver": {
                        "entryPoint": "s3://bucket/rt.py",
                        "entryPointArguments": ["--rows", "10"],
                        "sparkSubmitParameters": "--conf spark.executor.instances=2",
                    }},
                },
                "tags": {"team": "data", "env": "test"},
                "kmsKeyArn": "arn:aws:kms:us-west-2:000000000000:key/rt-1",
            }),
        },
        RoundTripCase {
            label: "the whole nested jobTemplateData tree",
            body: json!({
                "name": "rt-deep",
                "clientToken": "rt-tok-deep",
                "jobTemplateData": deep_data(),
            }),
        },
        RoundTripCase {
            label: "an empty driver argument list",
            body: json!({
                "name": "rt-empty-args",
                "clientToken": "rt-tok-empty-args",
                "jobTemplateData": {
                    "executionRoleArn": "arn:aws:iam::000000000000:role/RtRole",
                    "releaseLabel": "emr-6.15.0-latest",
                    "jobDriver": {"sparkSubmitJobDriver": {
                        "entryPoint": "s3://bucket/rt.py",
                        "entryPointArguments": [],
                    }},
                },
            }),
        },
    ];

    for case in &cases {
        let label = case.label;
        let answer = create_template(&emr, case.body.clone()).await;
        let created = answer.ok(label).clone();
        let id = text(&created, "/id", label);
        assert_resource_id(label, "the minted job template id", &id);

        // `JobTemplateArn` is
        // `^arn:(aws[a-zA-Z0-9-]*):emr-containers:.+:(\d{12}):\/jobtemplates\/[0-9a-zA-Z]+$`
        // (smithy_query: shape `JobTemplateArn`), in the region the caller signed for.
        let expected_arn = format!(
            "arn:aws:emr-containers:{REGION}:{}:/jobtemplates/{id}",
            engine::emulator_account_id()
        );
        assert_eq!(
            text(&created, "/arn", label),
            expected_arn,
            "case {label:?}: CreateJobTemplate answered the wrong arn"
        );
        assert_eq!(
            created.get("name"),
            case.body.get("name"),
            "case {label:?}: CreateJobTemplate answered the wrong name"
        );
        let created_at = instant_millis(&created, "/createdAt", label);

        let described = describe_template(&emr, &id).await;
        let template = field(described.ok(label), "/jobTemplate", label).clone();
        assert_eq!(
            text(&template, "/id", label),
            id,
            "case {label:?}: the described template is not the created one"
        );
        assert_eq!(
            text(&template, "/arn", label),
            expected_arn,
            "case {label:?}: the described arn is not the created one"
        );
        assert_eq!(
            instant_millis(&template, "/createdAt", label),
            created_at,
            "case {label:?}: the described createdAt is not the created one"
        );
        assert_eq!(
            field(&template, "/jobTemplateData", label),
            field(&case.body, "/jobTemplateData", label),
            "case {label:?}: the described jobTemplateData is not the submitted one"
        );
        assert_eq!(
            template.get("kmsKeyArn"),
            case.body.get("kmsKeyArn"),
            "case {label:?}: the described kmsKeyArn is not the submitted one, got {:?}",
            template.get("kmsKeyArn")
        );
        let expected_tags = case.body.get("tags").cloned().unwrap_or_else(|| json!({}));
        assert_eq!(
            template.get("tags"),
            Some(&expected_tags),
            "case {label:?}: the described tags are not the submitted ones, got {:?}",
            template.get("tags")
        );
        // Nothing attributed this template to an IAM identity and nothing was encrypted,
        // so neither member is reported.
        for absent in ["createdBy", "decryptionError"] {
            assert!(
                template.get(absent).is_none(),
                "case {label:?}: {absent} must stay absent, got {:?}",
                template.get(absent)
            );
        }
    }

    // RULE 4 for the reader's refusal: an id that names no stored template is not found.
    for (label, unknown) in [
        ("an id that was never minted", "doesnotexist1"),
        ("an id of the right shape but no record", "0zzzz9"),
    ] {
        describe_template(&emr, unknown)
            .await
            .refused(label, 400, "ResourceNotFoundException");
    }
}

/// RULE 5: `DeleteJobTemplate` removes the record, so the template stops being readable
/// and the `clientToken` that minted it is spendable again — the token names a template,
/// and once there is none it names nothing.
#[tokio::test]
async fn deleting_a_job_template_frees_its_client_token() {
    // (case, how many create/delete cycles to run on one token)
    for (label, cycles) in [("one cycle", 1usize), ("three cycles", 3)] {
        let emr = Emr::new();
        let body = json!({
            "name": "recycled",
            "clientToken": "tok-recycled",
            "jobTemplateData": minimal_data(),
        });

        let mut seen = BTreeSet::new();
        for cycle in 0..cycles {
            let created = create_template(&emr, body.clone()).await;
            let id = text(created.ok(label), "/id", label);
            assert!(
                seen.insert(id.clone()),
                "case {label:?}: cycle {cycle} reused the id {id:?} of a deleted template"
            );

            let deleted = delete_template(&emr, &id).await;
            assert_eq!(
                text(deleted.ok(label), "/id", label),
                id,
                "case {label:?}: DeleteJobTemplate must report the id it deleted"
            );
            describe_template(&emr, &id)
                .await
                .refused(label, 400, "ResourceNotFoundException");
            let listed = listed_ids(&emr, "", label).await;
            assert!(
                listed.is_empty(),
                "case {label:?}: cycle {cycle} left {listed:?} behind after the delete"
            );
            // `DeleteJobTemplate` models no not-found error, so deleting the same id
            // again is refused as an invalid request.
            delete_template(&emr, &id)
                .await
                .refused(label, 400, "ValidationException");
        }
    }
}

/// RULE 4 for the other reader of a stored template: `StartJobRun` resolves a
/// `jobTemplateId` out of the SAME store, so an id naming no stored template is not
/// found, while one that does exist is refused with the modelled `ValidationException` —
/// this emulator does not apply a template's `jobTemplateData` to a run, and a run built
/// from the request alone would ignore the values the template exists to enforce.
#[tokio::test]
async fn start_job_run_resolves_a_job_template_id_out_of_the_store() {
    let emr = Emr::new();
    let cluster = {
        let answer = emr
            .send(
                "POST",
                "/virtualclusters",
                Some(json!({
                    "name": "templates",
                    "clientToken": "tok-templates",
                    "containerProvider": {"id": "eks-templates", "type": "EKS"},
                })),
            )
            .await;
        text(answer.ok("the fixture cluster"), "/id", "the fixture cluster")
    };

    // Rows differ in the stored template's shape, so the refusal is about the template
    // being unapplicable rather than about one particular body.
    let stored = [
        ("a minimal stored template", "sjr-minimal", minimal_data()),
        ("a parametric stored template", "sjr-deep", deep_data()),
    ];
    for (label, token, data) in stored {
        let created = create_template(
            &emr,
            json!({"name": "sjr", "clientToken": token, "jobTemplateData": data}),
        )
        .await;
        let template_id = text(created.ok(label), "/id", label);

        let refused = emr
            .send(
                "POST",
                &format!("/virtualclusters/{cluster}/jobruns"),
                Some(json!({
                    "clientToken": format!("run-{token}"),
                    "jobTemplateId": template_id,
                })),
            )
            .await;
        refused.refused(label, 400, "ValidationException");
    }

    // An id of the right shape naming no stored template is not found, not unapplicable.
    for (label, unknown) in [
        ("a template id that was never minted", "doesnotexist1"),
        ("a template id of the right shape but no record", "0zzzz9"),
    ] {
        emr.send(
            "POST",
            &format!("/virtualclusters/{cluster}/jobruns"),
            Some(json!({
                "clientToken": format!("run-{unknown}"),
                "jobTemplateId": unknown,
            })),
        )
        .await
        .refused(label, 400, "ResourceNotFoundException");
    }
}

/// RULE 6: `ListJobTemplates` is `@paginated`, so `maxResults` + `nextToken` walk the
/// stored set with no repeats and no gaps, and a `nextToken` is reported only while
/// templates remain.
#[tokio::test]
async fn listing_job_templates_pages_the_stored_set() {
    let emr = Emr::new();
    let total = 7;
    let mut minted = BTreeSet::new();
    for index in 0..total {
        let label = format!("template {index}");
        let answer = create_template(
            &emr,
            json!({
                "name": format!("paged{index}"),
                "clientToken": format!("tok-paged-{index}"),
                "jobTemplateData": minimal_data(),
            }),
        )
        .await;
        minted.insert(text(answer.ok(&label), "/id", &label));
    }

    // Rows differ in the page size, including one past the stored count.
    for page_size in [1usize, 2, 3, 7, 9] {
        let label = format!("maxResults {page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut query = format!("?maxResults={page_size}");
        loop {
            let answer = list_templates(&emr, &query).await;
            let body = answer.ok(&label).clone();
            let page: Vec<String> = field(&body, "/templates", &label)
                .as_array()
                .unwrap_or_else(|| panic!("case {label:?}: templates is not a list: {body}"))
                .iter()
                .map(|template| text(template, "/id", &label))
                .collect();
            assert!(
                page.len() <= page_size,
                "case {label:?}: a page carried {} templates, more than maxResults",
                page.len()
            );
            walked.extend(page);
            match body.get("nextToken").and_then(Value::as_str) {
                Some(token) => query = format!("?maxResults={page_size}&nextToken={token}"),
                None => break,
            }
        }

        let unique: BTreeSet<&String> = walked.iter().collect();
        assert_eq!(
            unique.len(),
            walked.len(),
            "case {label:?}: paging repeated a template, walked {walked:?}"
        );
        assert_eq!(
            unique.into_iter().cloned().collect::<BTreeSet<String>>(),
            minted,
            "case {label:?}: paging did not walk every stored template, walked {walked:?}"
        );
    }

    // A `nextToken` this service never issued is not a position in that order.
    list_templates(&emr, "?nextToken=not-a-cursor")
        .await
        .refused("a forged nextToken", 400, "ValidationException");
    // A page of fewer than one template is not a page.
    list_templates(&emr, "?maxResults=0")
        .await
        .refused("maxResults below one", 400, "ValidationException");
}
