// SPDX-License-Identifier: CC0-1.0

//! Each optional Amazon S3 object an environment loads is stored as the PAIR that
//! identifies it, end to end over HTTP through the router this service mounts.
//!
//! The rule every case here asserts: an environment's record keeps an optional Amazon S3
//! object only as the complete pair that identifies it — the object's relative path
//! together with that object's version — or not at all, so a configuration in which either
//! member names nothing (both are `@length(0, 1024)`, so the empty string is a legal value
//! that names nothing) loads no such object and reports NEITHER member, however it came to
//! load none.
//!
//! Each table row varies what the rule generalises over: which of the three objects the
//! pair belongs to, which side of the pair the request clears, how many pairs one request
//! touches, whether the environment was loading that object before the request, and whether
//! the pair arrived through the create or through a later update.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

/// A signed-looking request header so the router resolves this service by its SigV4
/// credential scope, as an SDK request does. Signature verification is off in this harness.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTEST/20260101/us-east-1/airflow/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// The canonical emulator account id every emulated ARN carries.
const ACCOUNT: &str = "000000000000";

/// Comfortably longer than `state::TRANSITION_SPAN`, so a transition the clock owes has
/// certainly come due: a create has settled into `AVAILABLE` and an accepted update has
/// reached the phase that applies it.
const PAST_TRANSITION: Duration = Duration::from_millis(900);

/// The three pairs, as `(path member, version member)`. From the members of
/// `smithy_query(query="operation", operation="UpdateEnvironment")`: `PluginsS3Path` /
/// `PluginsS3ObjectVersion`, `RequirementsS3Path` / `RequirementsS3ObjectVersion` and
/// `StartupScriptS3Path` / `StartupScriptS3ObjectVersion`.
const PAIRS: [(&str, &str); 3] = [
    ("PluginsS3Path", "PluginsS3ObjectVersion"),
    ("RequirementsS3Path", "RequirementsS3ObjectVersion"),
    ("StartupScriptS3Path", "StartupScriptS3ObjectVersion"),
];

/// One HTTP answer, reduced to what these tests assert on.
struct Answer {
    status: u16,
    body: Value,
}

/// The emulator, assembled as the router assembles it: one shared engine, one service.
struct Harness {
    router: MultiServiceRouter,
}

impl Harness {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_mwaa_amazonmwaa::EmulatedService::new(Arc::clone(&engine)),
        );
        Self { router }
    }

    async fn send(&self, method: &str, uri: &str, body: Option<Value>) -> Answer {
        let payload = body
            .map(|value| serde_json::to_vec(&value).expect("a serialisable body"))
            .unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", "api.airflow.us-east-1.amazonaws.com")
            .header("authorization", AUTHORIZATION)
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body().to_vec();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                panic!(
                    "expected a JSON body, got {error}: {}",
                    String::from_utf8_lossy(&bytes)
                )
            })
        };
        Answer { status, body }
    }

    async fn create(&self, name: &str, extra: &Value) -> Answer {
        self.send(
            "PUT",
            &format!("/environments/{name}"),
            Some(create_body(name, extra)),
        )
        .await
    }

    async fn update(&self, name: &str, patch: &Value) -> Answer {
        self.send("PATCH", &format!("/environments/{name}"), Some(patch.clone()))
            .await
    }

    /// The `Environment` of a successful `GetEnvironment` — the read that proves what the
    /// record holds.
    async fn environment(&self, case: &str, name: &str) -> Value {
        let answer = self
            .send("GET", &format!("/environments/{name}"), None)
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected GetEnvironment 200, got {} {}",
            answer.status, answer.body
        );
        answer
            .body
            .get("Environment")
            .cloned()
            .unwrap_or_else(|| panic!("case {case}: GetEnvironment answered without Environment"))
    }
}

/// The required members of a create, plus `extra` merged over them.
fn create_body(name: &str, extra: &Value) -> Value {
    let mut body = json!({
        "ExecutionRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/{name}-execution"),
        "SourceBucketArn": "arn:aws:s3:::my-airflow-bucket-unique-name",
        "DagS3Path": "dags",
        "NetworkConfiguration": {
            "SubnetIds": ["subnet-aaa111", "subnet-bbb222"],
            "SecurityGroupIds": ["sg-111"],
        },
    });
    let target = body.as_object_mut().expect("an object body");
    for (member, value) in extra.as_object().expect("an object of extra members") {
        target.insert(member.clone(), value.clone());
    }
    body
}

/// `EnvironmentName` is `^[a-zA-Z][0-9a-zA-Z-_]*$`, so a row is named from its table and
/// its index rather than from a value that may hold a dot or be empty.
fn env_name(table: &str, index: usize) -> String {
    format!("{table}Row{index}")
}

/// Assert the six pair members of `environment` are exactly `expected` — a member
/// `expected` does not name must be absent, because a record that loads no object names
/// neither half of its pair.
fn assert_pairs(case: &str, environment: &Value, expected: &Value) {
    let expected = expected.as_object().expect("an object of expected members");
    for (path_member, version_member) in PAIRS {
        for member in [path_member, version_member] {
            let actual = environment.get(member).cloned().unwrap_or(Value::Null);
            let wanted = expected.get(member).cloned().unwrap_or(Value::Null);
            assert_eq!(
                actual, wanted,
                "case {case}: expected {member} {wanted}, got {actual}"
            );
        }
    }
}

/// One row: the objects the create loads, the patch a later update carries, and the pair
/// members the record reports once that update has landed.
struct Case {
    label: &'static str,
    /// The S3 pair members the create carries.
    seed: Value,
    /// The members the update carries.
    patch: Value,
    /// The pair members the record reports afterwards; every pair member not named here
    /// must be absent.
    reports: Value,
}

/// Every object loaded, each with its version: the fixture the clearing rows start from.
fn all_three() -> Value {
    json!({
        "PluginsS3Path": "plugins.zip",
        "PluginsS3ObjectVersion": "9VnR0zZr7Qz",
        "RequirementsS3Path": "requirements.txt",
        "RequirementsS3ObjectVersion": "kQ1x8NnLm2A",
        "StartupScriptS3Path": "startup.sh",
        "StartupScriptS3ObjectVersion": "aB3dEf5GhI7",
    })
}

/// The table: one row per shape of request the rule has to hold for.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "clearing the plugins path drops the version it was loaded with",
            seed: all_three(),
            patch: json!({"PluginsS3Path": ""}),
            reports: json!({
                "RequirementsS3Path": "requirements.txt",
                "RequirementsS3ObjectVersion": "kQ1x8NnLm2A",
                "StartupScriptS3Path": "startup.sh",
                "StartupScriptS3ObjectVersion": "aB3dEf5GhI7",
            }),
        },
        Case {
            label: "clearing the requirements path drops the version it was loaded with",
            seed: all_three(),
            patch: json!({"RequirementsS3Path": ""}),
            reports: json!({
                "PluginsS3Path": "plugins.zip",
                "PluginsS3ObjectVersion": "9VnR0zZr7Qz",
                "StartupScriptS3Path": "startup.sh",
                "StartupScriptS3ObjectVersion": "aB3dEf5GhI7",
            }),
        },
        Case {
            label: "clearing the startup-script path drops the version it was loaded with",
            seed: all_three(),
            patch: json!({"StartupScriptS3Path": ""}),
            reports: json!({
                "PluginsS3Path": "plugins.zip",
                "PluginsS3ObjectVersion": "9VnR0zZr7Qz",
                "RequirementsS3Path": "requirements.txt",
                "RequirementsS3ObjectVersion": "kQ1x8NnLm2A",
            }),
        },
        Case {
            label: "all three paths cleared in one request",
            seed: all_three(),
            patch: json!({
                "PluginsS3Path": "",
                "RequirementsS3Path": "",
                "StartupScriptS3Path": "",
            }),
            reports: json!({}),
        },
        Case {
            label: "one pair cleared while another is loaded by the same request",
            seed: json!({
                "PluginsS3Path": "plugins.zip",
                "PluginsS3ObjectVersion": "9VnR0zZr7Qz",
            }),
            patch: json!({
                "PluginsS3Path": "",
                "RequirementsS3Path": "deps/requirements.txt",
                "RequirementsS3ObjectVersion": "Zx8cV7bN6mQ",
            }),
            reports: json!({
                "RequirementsS3Path": "deps/requirements.txt",
                "RequirementsS3ObjectVersion": "Zx8cV7bN6mQ",
            }),
        },
        Case {
            label: "clearing a path the environment was not loading changes nothing",
            seed: json!({
                "StartupScriptS3Path": "startup.sh",
                "StartupScriptS3ObjectVersion": "aB3dEf5GhI7",
            }),
            patch: json!({"PluginsS3Path": ""}),
            reports: json!({
                "StartupScriptS3Path": "startup.sh",
                "StartupScriptS3ObjectVersion": "aB3dEf5GhI7",
            }),
        },
        Case {
            label: "a new path replaces the version its pair was loaded with",
            seed: all_three(),
            patch: json!({
                "PluginsS3Path": "lib/plugins.zip",
                "PluginsS3ObjectVersion": "Lm3kQ9pR2sT",
            }),
            reports: json!({
                "PluginsS3Path": "lib/plugins.zip",
                "PluginsS3ObjectVersion": "Lm3kQ9pR2sT",
                "RequirementsS3Path": "requirements.txt",
                "RequirementsS3ObjectVersion": "kQ1x8NnLm2A",
                "StartupScriptS3Path": "startup.sh",
                "StartupScriptS3ObjectVersion": "aB3dEf5GhI7",
            }),
        },
        Case {
            label: "clearing the version alone leaves the object unidentified, so unloaded",
            seed: all_three(),
            patch: json!({"RequirementsS3ObjectVersion": ""}),
            reports: json!({
                "PluginsS3Path": "plugins.zip",
                "PluginsS3ObjectVersion": "9VnR0zZr7Qz",
                "StartupScriptS3Path": "startup.sh",
                "StartupScriptS3ObjectVersion": "aB3dEf5GhI7",
            }),
        },
        Case {
            label: "a version with no path of its own identifies no object, so loads none",
            seed: json!({}),
            patch: json!({"PluginsS3ObjectVersion": "9VnR0zZr7Qz"}),
            reports: json!({}),
        },
        Case {
            label: "a new version for a loaded object keeps that object's path",
            seed: all_three(),
            patch: json!({"StartupScriptS3ObjectVersion": "Qw2eR4tY6uI"}),
            reports: json!({
                "PluginsS3Path": "plugins.zip",
                "PluginsS3ObjectVersion": "9VnR0zZr7Qz",
                "RequirementsS3Path": "requirements.txt",
                "RequirementsS3ObjectVersion": "kQ1x8NnLm2A",
                "StartupScriptS3Path": "startup.sh",
                "StartupScriptS3ObjectVersion": "Qw2eR4tY6uI",
            }),
        },
    ]
}

#[tokio::test]
async fn an_update_stores_each_s3_object_as_the_pair_that_identifies_it_or_not_at_all() {
    let harness = Harness::new();
    let table = "PairPatch";
    let rows = cases();

    for (index, row) in rows.iter().enumerate() {
        let name = env_name(table, index);
        let answer = harness.create(&name, &row.seed).await;
        assert_eq!(
            answer.status, 200,
            "case {}: the fixture create must succeed, got {} {}",
            row.label, answer.status, answer.body
        );
    }
    // An update is accepted only once the create has settled into AVAILABLE.
    tokio::time::sleep(PAST_TRANSITION).await;

    for (index, row) in rows.iter().enumerate() {
        let name = env_name(table, index);
        let answer = harness.update(&name, &row.patch).await;
        assert_eq!(
            answer.status, 200,
            "case {}: expected UpdateEnvironment 200, got {} {}",
            row.label, answer.status, answer.body
        );
    }
    // The change is applied by the second phase of the update, so the record is read back
    // only once that phase has come due.
    tokio::time::sleep(PAST_TRANSITION).await;

    for (index, row) in rows.iter().enumerate() {
        let name = env_name(table, index);
        let environment = harness.environment(row.label, &name).await;
        assert_pairs(row.label, &environment, &row.reports);
    }
}

#[tokio::test]
async fn a_create_stores_each_s3_object_as_the_pair_that_identifies_it_or_not_at_all() {
    let harness = Harness::new();
    // The same rule on the way IN: the create's own pair members, each row differing in
    // which side names nothing.
    let rows: Vec<(&str, Value, Value)> = vec![
        (
            "every object named with its version",
            all_three(),
            all_three(),
        ),
        (
            "an empty plugins path names no object, so none is loaded",
            json!({"PluginsS3Path": ""}),
            json!({}),
        ),
        (
            "an empty requirements path beside a named startup script",
            json!({
                "RequirementsS3Path": "",
                "StartupScriptS3Path": "bin/startup.sh",
                "StartupScriptS3ObjectVersion": "Qw2eR4tY6uI",
            }),
            json!({
                "StartupScriptS3Path": "bin/startup.sh",
                "StartupScriptS3ObjectVersion": "Qw2eR4tY6uI",
            }),
        ),
        (
            "all three paths empty",
            json!({
                "PluginsS3Path": "",
                "RequirementsS3Path": "",
                "StartupScriptS3Path": "",
            }),
            json!({}),
        ),
        (
            "a version with no path of its own loads no object",
            json!({"RequirementsS3ObjectVersion": "kQ1x8NnLm2A"}),
            json!({}),
        ),
        ("no S3 object member at all", json!({}), json!({})),
    ];

    for (index, (label, seed, reports)) in rows.iter().enumerate() {
        let name = env_name("PairCreate", index);
        let answer = harness.create(&name, seed).await;
        assert_eq!(
            answer.status, 200,
            "case {label}: expected CreateEnvironment 200, got {} {}",
            answer.status, answer.body
        );
        let environment = harness.environment(label, &name).await;
        assert_pairs(label, &environment, reports);
    }
}

#[tokio::test]
async fn an_environment_loading_no_object_reports_the_pair_the_same_way_however_it_came_to() {
    let harness = Harness::new();
    let loaded = json!({
        "PluginsS3Path": "plugins.zip",
        "PluginsS3ObjectVersion": "9VnR0zZr7Qz",
    });

    // Three ways to end up loading no plugins object: never naming the pair, naming a path
    // that names no object, and clearing a path a previous call named.
    for (name, seed) in [
        ("NeverNamed", json!({})),
        ("ClearedAtCreate", json!({"PluginsS3Path": ""})),
    ] {
        let answer = harness.create(name, &seed).await;
        assert_eq!(
            answer.status, 200,
            "case {name}: the fixture create must succeed, got {} {}",
            answer.status, answer.body
        );
    }
    let answer = harness.create("ClearedByUpdate", &loaded).await;
    assert_eq!(
        answer.status, 200,
        "case ClearedByUpdate: the fixture create must succeed, got {} {}",
        answer.status, answer.body
    );

    tokio::time::sleep(PAST_TRANSITION).await;
    let answer = harness
        .update("ClearedByUpdate", &json!({"PluginsS3Path": ""}))
        .await;
    assert_eq!(
        answer.status, 200,
        "case ClearedByUpdate: expected UpdateEnvironment 200, got {} {}",
        answer.status, answer.body
    );
    tokio::time::sleep(PAST_TRANSITION).await;

    let mut reports: Vec<(&str, Map<String, Value>)> = Vec::new();
    for name in ["NeverNamed", "ClearedAtCreate", "ClearedByUpdate"] {
        let environment = harness.environment(name, name).await;
        assert_pairs(name, &environment, &json!({}));
        let mut pairs = Map::new();
        for (path_member, version_member) in PAIRS {
            for member in [path_member, version_member] {
                pairs.insert(
                    member.to_string(),
                    environment.get(member).cloned().unwrap_or(Value::Null),
                );
            }
        }
        reports.push((name, pairs));
    }
    let (first_name, first) = &reports[0];
    for (name, pairs) in &reports[1..] {
        assert_eq!(
            pairs,
            first,
            "case {name}: an environment loading no object must report the pair exactly as \
             {first_name} does, got {:?}",
            Value::Object(pairs.clone())
        );
    }
}
