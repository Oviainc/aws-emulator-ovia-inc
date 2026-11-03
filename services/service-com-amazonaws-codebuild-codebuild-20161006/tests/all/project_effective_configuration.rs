// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the configuration a stored build project reports, driven over
//! the wire the router serves through the harness `crate::project_build_lifecycle` owns.
//!
//! Two rules are under test, each over a table that spans the class it generalises over
//! rather than the one request a trace happened to record:
//!
//! 1. A build project is STORED with every setting the service supplies rather than the
//!    client resolved, so each read reports the project's effective configuration: the
//!    cache (no cache at all is the explicit `NO_CACHE` cache), the managed S3 key for an
//!    absent `encryptionKey`, the queue and build timeouts, the environment's
//!    `privilegedMode`, `imagePullCredentialsType` and `environmentVariables`, and every
//!    stored source's `insecureSsl`. A setting the request DID carry is stored as sent,
//!    and an update that replaces a setting resolves the replacement the same way the
//!    create resolved the setting it replaces.
//! 2. `BatchGetProjects` resolves `names` as a SET: each distinct identifier is answered
//!    exactly once, in the order the request first named it, so `projects` is not
//!    index-aligned with `names`.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::project_build_lifecycle::{call, create_project, emulator};
use crate::read_response_shape::{at, managed_s3_key};

/// The members every stored project states, whatever the request carried, with the value
/// the service resolves for a request that names none of them.
///
/// The member list is `Project`'s own
/// (`smithy_query(query="shape",shape="Project,ProjectCache,ProjectEnvironment,ProjectSource")`);
/// each value is the default that member's documentation states — `NO_CACHE` ("The build
/// project does not use any cache"), the managed CMK for Amazon S3, `privilegedMode`
/// ("The default setting is false"), `CODEBUILD` credentials for a curated image, the
/// opt-in `insecureSsl` flag off, `timeoutInMinutes` ("The default is 60 minutes") and
/// the whole of `TimeOut`'s `@range(min: 5, max: 480)` window for an unset queue timeout.
fn resolved_defaults() -> Vec<(&'static str, Value)> {
    vec![
        ("cache", json!({"type": "NO_CACHE"})),
        ("encryptionKey", json!(managed_s3_key())),
        ("environment.environmentVariables", json!([])),
        ("environment.imagePullCredentialsType", json!("CODEBUILD")),
        ("environment.privilegedMode", json!(false)),
        ("queuedTimeoutInMinutes", json!(480)),
        ("source.insecureSsl", json!(false)),
        ("timeoutInMinutes", json!(60)),
    ]
}

/// The project `name` as `BatchGetProjects` reports it.
async fn fetch_project(router: &MultiServiceRouter, name: &str) -> Value {
    let (status, body) = call(router, "BatchGetProjects", json!({"names": [name]})).await;
    assert_eq!(
        status, 200,
        "BatchGetProjects {name:?}: expected 200, got {status} with {body}"
    );
    body["projects"][0].clone()
}

/// `UpdateProject` for `name` with `extra`'s members merged in, asserting it succeeded.
async fn update_project(router: &MultiServiceRouter, name: &str, extra: Value) -> Value {
    let mut body = json!({"name": name});
    if let (Value::Object(target), Value::Object(members)) = (&mut body, extra) {
        target.extend(members);
    }
    let (status, response) = call(router, "UpdateProject", body.clone()).await;
    assert_eq!(
        status, 200,
        "UpdateProject {body}: expected 200, got {status} with {response}"
    );
    response["project"].clone()
}

/// RULE 1: the project a create stores — and therefore every read of it — states the
/// settings the service resolved, and states the ones the request carried as sent.
///
/// The rows differ along what the rule generalises over: a request that names none of
/// these members, one that names each of them, the published values of the enums
/// involved (`CacheType`: NO_CACHE | S3 | LOCAL, `ImagePullCredentialsType`: CODEBUILD |
/// SERVICE_ROLE), both bounds of `TimeOut`'s `@range(min: 5, max: 480)`, both values of
/// each boolean flag, an empty and a populated variable list, and a secondary source
/// beside the primary one.
#[tokio::test]
async fn a_stored_project_states_the_configuration_the_service_resolved() {
    /// One row: the case, the create request's extra members, and the (member path,
    /// value) pairs the stored project must state for them.
    type Row = (&'static str, Value, Vec<(&'static str, Value)>);

    let rows: Vec<Row> = vec![
        (
            "none of the defaulted members named",
            json!({}),
            resolved_defaults(),
        ),
        (
            "an S3 cache and a customer-managed key",
            json!({
                "cache": {"type": "S3", "location": "cache-bucket/prefix"},
                "encryptionKey": "arn:aws:kms:us-west-2:000000000000:key/11111111-1111-1111-1111-111111111111",
            }),
            vec![
                ("cache", json!({"type": "S3", "location": "cache-bucket/prefix"})),
                (
                    "encryptionKey",
                    json!("arn:aws:kms:us-west-2:000000000000:key/11111111-1111-1111-1111-111111111111"),
                ),
                // The members this request did NOT name are still resolved.
                ("queuedTimeoutInMinutes", json!(480)),
                ("source.insecureSsl", json!(false)),
            ],
        ),
        (
            "a LOCAL cache with modes",
            json!({"cache": {"type": "LOCAL", "modes": ["LOCAL_SOURCE_CACHE"]}}),
            vec![
                (
                    "cache",
                    json!({"type": "LOCAL", "modes": ["LOCAL_SOURCE_CACHE"]}),
                ),
                ("environment.privilegedMode", json!(false)),
            ],
        ),
        (
            "a queue timeout at the modelled lower bound",
            json!({"queuedTimeoutInMinutes": 5}),
            vec![
                ("queuedTimeoutInMinutes", json!(5)),
                ("timeoutInMinutes", json!(60)),
            ],
        ),
        (
            "a queue timeout at the modelled upper bound",
            json!({"queuedTimeoutInMinutes": 480, "timeoutInMinutes": 2160}),
            vec![
                ("queuedTimeoutInMinutes", json!(480)),
                ("timeoutInMinutes", json!(2160)),
            ],
        ),
        (
            "an environment naming every member the service would default",
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
                ("cache", json!({"type": "NO_CACHE"})),
                ("encryptionKey", json!(managed_s3_key())),
            ],
        ),
        (
            "a source that ignores SSL warnings, plus a secondary source that does not",
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
            }),
            vec![
                ("source.insecureSsl", json!(true)),
                // A secondary source is a stored source too.
                ("secondarySources.0.insecureSsl", json!(false)),
                ("secondarySources.0.sourceIdentifier", json!("second")),
            ],
        ),
    ];

    for (index, (case, configuration, expected)) in rows.into_iter().enumerate() {
        let router = emulator();
        let name = format!("effective-{index:02}");
        let created = create_project(&router, &name, configuration).await;

        for (path, want) in &expected {
            assert_eq!(
                &at(&created, path),
                want,
                "case {case:?} / CreateProject: expected {path} = {want}, got {:?}",
                at(&created, path)
            );
        }

        // The defaults are PERSISTED, not computed per answer: the read reports the very
        // record the create stored.
        let fetched = fetch_project(&router, &name).await;
        assert_eq!(
            fetched, created,
            "case {case:?}: BatchGetProjects must report the stored project verbatim, \
             got {fetched}"
        );

        // An update that names nothing but the project leaves every resolved setting
        // where it was, so a client that round-trips a project cannot lose them.
        let updated = update_project(&router, &name, json!({})).await;
        for (path, want) in &expected {
            assert_eq!(
                &at(&updated, path),
                want,
                "case {case:?} / UpdateProject: {path} must stay {want}, got {:?}",
                at(&updated, path)
            );
        }
    }
}

/// RULE 1, for a replacement: a setting `UpdateProject` replaces is stored with the
/// members the service supplies resolved, exactly as the create resolved the setting it
/// replaced.
///
/// The rows span the structures that carry a defaulted member — the primary source, a
/// secondary source, the environment and the cache — each replaced by one naming none of
/// them, against a project created with every one of them set to a non-default value.
#[tokio::test]
async fn an_update_resolves_the_defaults_of_the_setting_it_replaces() {
    // The project each row starts from: every defaulted member explicitly set away from
    // its default, so a reappearing default can only have come from the resolution.
    let configured = json!({
        "cache": {"type": "S3", "location": "cache-bucket/prefix"},
        "encryptionKey": "arn:aws:kms:us-west-2:000000000000:key/configured",
        "queuedTimeoutInMinutes": 5,
        "timeoutInMinutes": 2160,
        "source": {"type": "NO_SOURCE", "buildspec": "version: 0.2", "insecureSsl": true},
        "secondarySources": [{
            "type": "GITHUB",
            "location": "https://github.com/example/second",
            "sourceIdentifier": "second",
            "insecureSsl": true,
        }],
        "environment": {
            "type": "LINUX_CONTAINER",
            "image": "aws/codebuild/standard:7.0",
            "computeType": "BUILD_GENERAL1_SMALL",
            "privilegedMode": true,
            "imagePullCredentialsType": "SERVICE_ROLE",
            "environmentVariables": [{"name": "FOO", "value": "bar"}],
        },
    });

    /// One row: the case, the update request, and the (member path, value) pairs the
    /// result must state.
    type ReplacementRow = (&'static str, Value, Vec<(&'static str, Value)>);

    // (case, the update request, the (member path, value) pairs the result must state)
    let rows: Vec<ReplacementRow> = vec![
        (
            "a replacement primary source naming no flags",
            json!({"source": {"type": "NO_SOURCE", "buildspec": "version: 0.2"}}),
            vec![
                ("source.insecureSsl", json!(false)),
                // Every other setting is untouched by this request.
                ("secondarySources.0.insecureSsl", json!(true)),
                ("environment.privilegedMode", json!(true)),
            ],
        ),
        (
            "a replacement secondary source naming no flags",
            json!({"secondarySources": [
                {"type": "S3", "location": "bucket/src.zip", "sourceIdentifier": "other"},
            ]}),
            vec![
                ("secondarySources.0.insecureSsl", json!(false)),
                ("secondarySources.0.sourceIdentifier", json!("other")),
                ("source.insecureSsl", json!(true)),
            ],
        ),
        (
            "a replacement environment naming no defaulted member",
            json!({"environment": {
                "type": "ARM_CONTAINER",
                "image": "aws/codebuild/amazonlinux2-aarch64-standard:3.0",
                "computeType": "BUILD_GENERAL1_LARGE",
            }}),
            vec![
                ("environment.privilegedMode", json!(false)),
                ("environment.imagePullCredentialsType", json!("CODEBUILD")),
                ("environment.environmentVariables", json!([])),
                ("environment.type", json!("ARM_CONTAINER")),
            ],
        ),
        (
            "a replacement cache of another type",
            json!({"cache": {"type": "NO_CACHE"}}),
            vec![
                ("cache", json!({"type": "NO_CACHE"})),
                ("encryptionKey", json!("arn:aws:kms:us-west-2:000000000000:key/configured")),
            ],
        ),
        (
            "a replacement queue timeout",
            json!({"queuedTimeoutInMinutes": 480}),
            vec![
                ("queuedTimeoutInMinutes", json!(480)),
                ("timeoutInMinutes", json!(2160)),
            ],
        ),
    ];

    for (index, (case, request, expected)) in rows.into_iter().enumerate() {
        let router = emulator();
        let name = format!("replaced-{index:02}");
        create_project(&router, &name, configured.clone()).await;

        let updated = update_project(&router, &name, request).await;
        for (path, want) in &expected {
            assert_eq!(
                &at(&updated, path),
                want,
                "case {case:?} / UpdateProject: expected {path} = {want}, got {:?}",
                at(&updated, path)
            );
        }

        let fetched = fetch_project(&router, &name).await;
        assert_eq!(
            fetched, updated,
            "case {case:?}: BatchGetProjects must report the updated record, got {fetched}"
        );
    }
}

/// RULE 2: `BatchGetProjects` resolves `names` as a set — one entry per DISTINCT name, in
/// the order the request first named it — for both halves of its partition.
///
/// The rows vary the repeat count (twice, three times, the modelled `ProjectNames`
/// ceiling of 100 entries), the order the repeats appear in, whether the repeated name is
/// held or not, and how many distinct names the request mixes.
#[tokio::test]
async fn batch_get_projects_resolves_its_names_as_a_set() {
    let router = emulator();
    create_project(&router, "set-one", json!({})).await;
    create_project(&router, "set-two", json!({})).await;
    let absent = "never-created".to_string();

    /// One row: the case, the names asked for, the project names expected in order, and
    /// the names expected under `projectsNotFound`.
    type SetRow = (&'static str, Vec<String>, Vec<&'static str>, Vec<&'static str>);

    // (case, the names asked for, the project names expected in order, the names
    // expected under projectsNotFound)
    let rows: Vec<SetRow> = vec![
        (
            "one name, once",
            vec!["set-one".to_string()],
            vec!["set-one"],
            vec![],
        ),
        (
            "the same name twice",
            vec!["set-one".to_string(), "set-one".to_string()],
            vec!["set-one"],
            vec![],
        ),
        (
            "the same name three times",
            vec![
                "set-one".to_string(),
                "set-one".to_string(),
                "set-one".to_string(),
            ],
            vec!["set-one"],
            vec![],
        ),
        (
            "two names, each repeated, answered in the order first named",
            vec![
                "set-two".to_string(),
                "set-one".to_string(),
                "set-two".to_string(),
                "set-one".to_string(),
            ],
            vec!["set-two", "set-one"],
            vec![],
        ),
        (
            "a held and an unheld name, both repeated",
            vec![
                "set-one".to_string(),
                absent.clone(),
                "set-one".to_string(),
                absent.clone(),
            ],
            vec!["set-one"],
            vec!["never-created"],
        ),
        (
            "an unheld name repeated",
            vec![absent.clone(), absent.clone(), absent.clone()],
            vec![],
            vec!["never-created"],
        ),
        (
            "one name at the modelled ProjectNames ceiling of 100 entries",
            vec!["set-two".to_string(); 100],
            vec!["set-two"],
            vec![],
        ),
    ];

    for (case, asked, expected_found, expected_missing) in rows {
        let (status, body) = call(&router, "BatchGetProjects", json!({"names": asked})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {body}"
        );
        let reported: Vec<String> = body["projects"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: projects must be a list, got {body}"))
            .iter()
            .map(|project| project["name"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            reported, expected_found,
            "case {case:?}: expected {expected_found:?}, got {reported:?}"
        );
        assert_eq!(
            body["projectsNotFound"],
            json!(expected_missing),
            "case {case:?}: expected {expected_missing:?} missing, got {body}"
        );
    }

    // The deduplicated entry is the whole stored project, not a trimmed copy of it: a
    // repeated name answers exactly what asking once answers.
    let once = fetch_project(&router, "set-one").await;
    let (_, twice) = call(
        &router,
        "BatchGetProjects",
        json!({"names": ["set-one", "set-one"]}),
    )
    .await;
    assert_eq!(
        twice["projects"],
        json!([once]),
        "a repeated name must answer the same single project asking once answers, got {twice}"
    );
}
