// SPDX-License-Identifier: CC0-1.0

//! The business rules `CreateEnvironment` and `UpdateEnvironment` enforce that the Smithy
//! model cannot state, end to end over HTTP through the router this service mounts.
//!
//! The rule every case here asserts: a create or an update is refused with HTTP 400
//! `ValidationException`, and leaves the stored record exactly as it was, when the
//! configuration it would leave the environment in breaks a rule the model cannot express
//! — an Amazon S3 object path named without that object's version in the SAME request, an
//! enumerated member outside the value set its own doc fixes, a sizing floor above its own
//! ceiling (compared against the STORED value for whichever side a patch omits), or an
//! update of an environment that has not settled into `AVAILABLE`. Anything inside those
//! rules is accepted and reads back through `GetEnvironment`.
//!
//! Each table row varies what the rule generalises over: which of the three S3 objects is
//! named, whether its version travels with it, which enumerated member carries the value
//! and whether that value is in the catalog, which side of a sizing pair the request
//! carries, and which status the environment is in when the update arrives. The catalogs
//! come from the members' own docs and name their source on the constant below.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_mwaa_amazonmwaa::state;

/// A signed-looking request header so the router resolves this service by its SigV4
/// credential scope, as an SDK request does. Signature verification is off in this harness.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTEST/20260101/us-east-1/airflow/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// The canonical emulator account id every emulated ARN carries.
const ACCOUNT: &str = "000000000000";

/// Comfortably longer than [`state::TRANSITION_SPAN`], so a transition the clock owes has
/// certainly come due and the environment accepts an update.
const PAST_TRANSITION: Duration = Duration::from_millis(900);

/// Every `AirflowVersion` the member's own doc lists as valid ("Valid values: 2.7.2, 2.8.1,
/// 2.9.2, 2.10.1, 2.10.3, 2.11.0, and 3.0.6"), from
/// `smithy_query(query="doc", shape="UpdateEnvironmentInput", member="AirflowVersion")`.
const AIRFLOW_VERSIONS: [&str; 7] = [
    "2.7.2", "2.8.1", "2.9.2", "2.10.1", "2.10.3", "2.11.0", "3.0.6",
];

/// Values outside [`AIRFLOW_VERSIONS`] that still satisfy `AirflowVersion`'s modelled
/// `@length(1, 32)` and `^[0-9a-z.]+$`, so the boundary validator passes them and only the
/// business rule can refuse them. Each is a plausible Apache Airflow release Amazon MWAA
/// does not run: one just past the newest supported version, one just past a supported
/// patch level, one from the 1.x line and one the issue did not cite.
const UNSUPPORTED_AIRFLOW_VERSIONS: [&str; 4] = ["3.0.7", "2.7.3", "1.10.12", "9.9.9"];

/// Every `EnvironmentClass` the member's own doc lists as valid ("Valid values: mw1.micro,
/// mw1.small, mw1.medium, mw1.large, mw1.xlarge, and mw1.2xlarge"), from
/// `smithy_query(query="doc", shape="UpdateEnvironmentInput", member="EnvironmentClass")`.
const ENVIRONMENT_CLASSES: [&str; 6] = [
    "mw1.micro",
    "mw1.small",
    "mw1.medium",
    "mw1.large",
    "mw1.xlarge",
    "mw1.2xlarge",
];

/// Values outside [`ENVIRONMENT_CLASSES`] that `EnvironmentClass`'s `@length(1, 1024)`
/// accepts: a size that does not exist, a generation that does not exist, a class whose
/// case differs from the catalog's, and a near-miss of a real one.
const UNSUPPORTED_ENVIRONMENT_CLASSES: [&str; 4] =
    ["mw1.enormous", "mw2.small", "MW1.small", "mw1.xxlarge"];

/// One HTTP answer, reduced to what these tests assert on.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The modelled error code, which restJson1 carries in `__type`.
    fn error_code(&self) -> String {
        self.body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .rsplit(['#', '/'])
            .next()
            .unwrap_or_default()
            .to_string()
    }

    fn message(&self) -> String {
        self.body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
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

    /// A create carrying the required members plus whatever `extra` adds.
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

    async fn get(&self, name: &str) -> Answer {
        self.send("GET", &format!("/environments/{name}"), None).await
    }

    /// The `Environment` of a successful `GetEnvironment`.
    async fn environment(&self, case: &str, name: &str) -> Value {
        let answer = self.get(name).await;
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

/// The required members of a create, plus the row's own members merged over them.
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

/// `EnvironmentName` is `^[a-zA-Z][0-9a-zA-Z-_]*$`, so a row's environment is named from
/// its table and its index rather than from a value that may hold a dot.
fn env_name(table: &str, index: usize) -> String {
    format!("{table}Row{index}")
}

/// Assert a refusal is this service's business-rule refusal: HTTP 400, the modelled
/// `ValidationException`, and a message that says something.
fn assert_refused(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 400,
        "case {case}: expected HTTP 400, got {} {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "ValidationException",
        "case {case}: expected a ValidationException, got {}",
        answer.body
    );
    assert!(
        !answer.message().is_empty(),
        "case {case}: the refusal must carry a message, got {}",
        answer.body
    );
}

/// One row of a create/update table: the members it carries, and whether the rules accept
/// them.
struct Row {
    label: &'static str,
    /// The members the request carries, as a JSON object.
    members: Value,
    accepted: bool,
}

/// Run `rows` through `CreateEnvironment`: an accepted row stores a record a read reports,
/// a refused one is a 400 that stores NOTHING.
async fn assert_creates(table: &str, rows: &[Row]) {
    let harness = Harness::new();
    for (index, row) in rows.iter().enumerate() {
        let name = env_name(table, index);
        let case = format!("create/{} {}", row.label, row.members);
        let answer = harness.create(&name, &row.members).await;
        if row.accepted {
            assert_eq!(
                answer.status, 200,
                "case {case}: expected CreateEnvironment 200, got {} {}",
                answer.status, answer.body
            );
            let environment = harness.environment(&case, &name).await;
            for (member, value) in row.members.as_object().expect("an object of members") {
                assert_eq!(
                    environment.get(member),
                    Some(value),
                    "case {case}: {member} must read back as the create stored it, got {:?}",
                    environment.get(member)
                );
            }
        } else {
            assert_refused(&case, &answer);
            let gone = harness.get(&name).await;
            assert_eq!(
                gone.status, 404,
                "case {case}: a refused create must store nothing, got {} {}",
                gone.status, gone.body
            );
        }
    }
}

/// Run `rows` through `UpdateEnvironment`, each against its own environment seeded with
/// `seed` and settled into AVAILABLE: an accepted row is a 200 whose members read back, a
/// refused one is a 400 that leaves the whole stored record untouched.
async fn assert_updates(table: &str, seed: &Value, rows: &[Row]) {
    let harness = Harness::new();
    for (index, row) in rows.iter().enumerate() {
        let name = env_name(table, index);
        let answer = harness.create(&name, seed).await;
        assert_eq!(
            answer.status, 200,
            "case update/{} {}: the fixture create must succeed, got {} {}",
            row.label, row.members, answer.status, answer.body
        );
    }
    // One wait for every row: an update is accepted only once the create has settled.
    tokio::time::sleep(PAST_TRANSITION).await;

    for (index, row) in rows.iter().enumerate() {
        let name = env_name(table, index);
        let case = format!("update/{} {}", row.label, row.members);
        let before = harness.environment(&case, &name).await;
        let answer = harness.update(&name, &row.members).await;
        if row.accepted {
            assert_eq!(
                answer.status, 200,
                "case {case}: expected UpdateEnvironment 200, got {} {}",
                answer.status, answer.body
            );
            let after = harness.environment(&case, &name).await;
            for (member, value) in row.members.as_object().expect("an object of members") {
                assert_eq!(
                    after.get(member),
                    Some(value),
                    "case {case}: {member} must read back as the patch left it, got {:?}",
                    after.get(member)
                );
            }
        } else {
            assert_refused(&case, &answer);
            let after = harness.environment(&case, &name).await;
            assert_eq!(
                after, before,
                "case {case}: a refused update must leave the stored record untouched"
            );
        }
    }
}

/// The S3 object rows: every one of the three objects named with and without its version,
/// a version with no path of its own, an empty path (which names no object), and a request
/// whose path and version belong to DIFFERENT objects.
fn s3_object_rows() -> Vec<Row> {
    vec![
        Row {
            label: "plugins path without its version",
            members: json!({"PluginsS3Path": "plugins.zip"}),
            accepted: false,
        },
        Row {
            label: "plugins path with its version",
            members: json!({"PluginsS3Path": "plugins.zip", "PluginsS3ObjectVersion": "9VnR0zZr7Qz"}),
            accepted: true,
        },
        Row {
            label: "requirements path without its version",
            members: json!({"RequirementsS3Path": "requirements.txt"}),
            accepted: false,
        },
        Row {
            label: "requirements path with its version",
            members: json!({
                "RequirementsS3Path": "requirements.txt",
                "RequirementsS3ObjectVersion": "kQ1x8NnLm2A",
            }),
            accepted: true,
        },
        Row {
            label: "startup script path without its version",
            members: json!({"StartupScriptS3Path": "startup.sh"}),
            accepted: false,
        },
        Row {
            label: "startup script path with its version",
            members: json!({
                "StartupScriptS3Path": "startup.sh",
                "StartupScriptS3ObjectVersion": "aB3dEf5GhI7",
            }),
            accepted: true,
        },
        Row {
            label: "all three paths, each with its version",
            members: json!({
                "PluginsS3Path": "lib/plugins.zip",
                "PluginsS3ObjectVersion": "Lm3kQ9pR2sT",
                "RequirementsS3Path": "deps/requirements.txt",
                "RequirementsS3ObjectVersion": "Zx8cV7bN6mQ",
                "StartupScriptS3Path": "bin/startup.sh",
                "StartupScriptS3ObjectVersion": "Qw2eR4tY6uI",
            }),
            accepted: true,
        },
        Row {
            label: "a path paired with the OTHER object's version",
            members: json!({
                "RequirementsS3Path": "requirements.txt",
                "PluginsS3ObjectVersion": "9VnR0zZr7Qz",
            }),
            accepted: false,
        },
        Row {
            label: "an object version with no path of its own",
            members: json!({"PluginsS3ObjectVersion": "9VnR0zZr7Qz"}),
            accepted: true,
        },
        Row {
            label: "an empty path names no object, so it needs no version",
            members: json!({"PluginsS3Path": ""}),
            accepted: true,
        },
    ]
}

#[tokio::test]
async fn an_s3_object_path_without_its_version_is_refused_by_create_and_by_update() {
    let rows = s3_object_rows();
    assert_creates("S3Create", &rows).await;
    assert_updates("S3Update", &json!({}), &rows).await;
}

/// The enumerated rows: every value each member's doc lists, plus values outside those
/// catalogs that the modelled length and pattern still accept.
fn enumerated_rows() -> Vec<Row> {
    // The handler's catalogs are the ones these rows are written from, so a drift in
    // either direction is a failure here rather than a silently narrower service.
    assert_eq!(
        state::SUPPORTED_AIRFLOW_VERSIONS.as_slice(),
        AIRFLOW_VERSIONS.as_slice(),
        "the service's AirflowVersion catalog must be the member's documented value list"
    );
    assert_eq!(
        state::ENVIRONMENT_CLASSES.as_slice(),
        ENVIRONMENT_CLASSES.as_slice(),
        "the service's EnvironmentClass catalog must be the member's documented value list"
    );

    let mut rows: Vec<Row> = Vec::new();
    for version in AIRFLOW_VERSIONS {
        rows.push(Row {
            label: "a supported AirflowVersion",
            members: json!({"AirflowVersion": version}),
            accepted: true,
        });
    }
    for version in UNSUPPORTED_AIRFLOW_VERSIONS {
        rows.push(Row {
            label: "an unsupported AirflowVersion",
            members: json!({"AirflowVersion": version}),
            accepted: false,
        });
    }
    for class in ENVIRONMENT_CLASSES {
        rows.push(Row {
            label: "a supported EnvironmentClass",
            members: json!({"EnvironmentClass": class}),
            accepted: true,
        });
    }
    for class in UNSUPPORTED_ENVIRONMENT_CLASSES {
        rows.push(Row {
            label: "an unsupported EnvironmentClass",
            members: json!({"EnvironmentClass": class}),
            accepted: false,
        });
    }
    rows
}

#[tokio::test]
async fn a_value_outside_a_members_documented_catalog_is_refused_by_create_and_by_update() {
    let rows = enumerated_rows();
    assert_creates("EnumCreate", &rows).await;
    assert_updates("EnumUpdate", &json!({}), &rows).await;
}

#[tokio::test]
async fn a_sizing_floor_above_its_own_ceiling_is_refused_by_create() {
    // Both pairs, each as a floor below, equal to and above its ceiling — including the
    // modelled minimum (1) on both sides, and a floor one above its ceiling.
    let rows = vec![
        Row {
            label: "MinWorkers below MaxWorkers",
            members: json!({"MinWorkers": 2, "MaxWorkers": 10}),
            accepted: true,
        },
        Row {
            label: "MinWorkers equal to MaxWorkers",
            members: json!({"MinWorkers": 4, "MaxWorkers": 4}),
            accepted: true,
        },
        Row {
            label: "MinWorkers one above MaxWorkers",
            members: json!({"MinWorkers": 3, "MaxWorkers": 2}),
            accepted: false,
        },
        Row {
            label: "MinWorkers far above MaxWorkers",
            members: json!({"MinWorkers": 5, "MaxWorkers": 1}),
            accepted: false,
        },
        Row {
            label: "both worker counts at the modelled minimum",
            members: json!({"MinWorkers": 1, "MaxWorkers": 1}),
            accepted: true,
        },
        Row {
            label: "MinWebservers below MaxWebservers",
            members: json!({"MinWebservers": 2, "MaxWebservers": 5}),
            accepted: true,
        },
        Row {
            label: "MinWebservers equal to MaxWebservers",
            members: json!({"MinWebservers": 3, "MaxWebservers": 3}),
            accepted: true,
        },
        Row {
            label: "MinWebservers one above MaxWebservers",
            members: json!({"MinWebservers": 3, "MaxWebservers": 2}),
            accepted: false,
        },
        Row {
            label: "MinWebservers far above MaxWebservers",
            members: json!({"MinWebservers": 5, "MaxWebservers": 1}),
            accepted: false,
        },
    ];
    assert_creates("SizeCreate", &rows).await;
}

#[tokio::test]
async fn a_patch_carrying_one_side_of_a_sizing_pair_is_judged_against_the_stored_other_side() {
    // Every environment is seeded with the same fleet, so each row's single-sided patch is
    // judged against values the REQUEST does not carry.
    let seed = json!({
        "MinWorkers": 2,
        "MaxWorkers": 4,
        "MinWebservers": 2,
        "MaxWebservers": 4,
    });
    let rows = vec![
        Row {
            label: "MinWorkers alone, above the stored MaxWorkers",
            members: json!({"MinWorkers": 5}),
            accepted: false,
        },
        Row {
            label: "MinWorkers alone, equal to the stored MaxWorkers",
            members: json!({"MinWorkers": 4}),
            accepted: true,
        },
        Row {
            label: "MaxWorkers alone, below the stored MinWorkers",
            members: json!({"MaxWorkers": 1}),
            accepted: false,
        },
        Row {
            label: "MaxWorkers alone, equal to the stored MinWorkers",
            members: json!({"MaxWorkers": 2}),
            accepted: true,
        },
        Row {
            label: "both worker counts raised together",
            members: json!({"MinWorkers": 6, "MaxWorkers": 8}),
            accepted: true,
        },
        Row {
            label: "MinWebservers alone, above the stored MaxWebservers",
            members: json!({"MinWebservers": 5}),
            accepted: false,
        },
        Row {
            label: "MaxWebservers alone, below the stored MinWebservers",
            members: json!({"MaxWebservers": 1}),
            accepted: false,
        },
        Row {
            label: "both web-server counts moved together",
            members: json!({"MinWebservers": 3, "MaxWebservers": 5}),
            accepted: true,
        },
    ];
    assert_updates("SizePatch", &seed, &rows).await;
}

#[tokio::test]
async fn an_update_is_refused_while_the_environment_has_not_settled_into_available() {
    let harness = Harness::new();
    let name = "StatusGate";
    let patch = json!({"MaxWorkers": 4});

    let created = harness.create(name, &json!({})).await;
    assert_eq!(
        created.status, 200,
        "case create: the fixture create must succeed, got {} {}",
        created.status, created.body
    );

    // Still CREATING: the create has not landed, so there is nothing to update yet.
    let creating = harness.environment("while CREATING", name).await;
    assert_eq!(
        creating.get("Status"),
        Some(&json!("CREATING")),
        "case while CREATING: a freshly created environment is still CREATING, got {:?}",
        creating.get("Status")
    );
    let answer = harness.update(name, &patch).await;
    assert_refused("while CREATING", &answer);
    assert_eq!(
        harness.environment("while CREATING", name).await,
        creating,
        "case while CREATING: a refused update must not touch the pending create"
    );

    tokio::time::sleep(PAST_TRANSITION).await;
    let answer = harness.update(name, &patch).await;
    assert_eq!(
        answer.status, 200,
        "case once AVAILABLE: an update of a settled environment must be accepted, got {} {}",
        answer.status, answer.body
    );

    // Already UPDATING: the update in flight has not settled, so a second one is refused
    // rather than silently overwriting it.
    let updating = harness.environment("while UPDATING", name).await;
    assert_eq!(
        updating.get("Status"),
        Some(&json!("UPDATING")),
        "case while UPDATING: the accepted update leaves the environment UPDATING, got {:?}",
        updating.get("Status")
    );
    let answer = harness.update(name, &json!({"MaxWorkers": 9})).await;
    assert_refused("while UPDATING", &answer);
    assert_eq!(
        harness.environment("while UPDATING", name).await,
        updating,
        "case while UPDATING: a refused update must not overwrite the update in flight"
    );

    // And once the clock settles it, the same patch is accepted.
    tokio::time::sleep(PAST_TRANSITION).await;
    let settled = harness.environment("settled", name).await;
    assert_eq!(
        settled.get("Status"),
        Some(&json!("AVAILABLE")),
        "case settled: the clock owes AVAILABLE once {:?} has elapsed, got {:?}",
        state::TRANSITION_SPAN,
        settled.get("Status")
    );
    assert_eq!(
        settled.get("MaxWorkers"),
        Some(&json!(4)),
        "case settled: the accepted update is the one that stored MaxWorkers, got {:?}",
        settled.get("MaxWorkers")
    );
    let answer = harness.update(name, &json!({"MaxWorkers": 9})).await;
    assert_eq!(
        answer.status, 200,
        "case after settling: the refused patch succeeds once the environment is AVAILABLE, \
         got {} {}",
        answer.status, answer.body
    );
    tokio::time::sleep(PAST_TRANSITION).await;
    let after = harness.environment("after settling", name).await;
    assert_eq!(
        after.get("MaxWorkers"),
        Some(&json!(9)),
        "case after settling: MaxWorkers must read back as the accepted patch left it, got {:?}",
        after.get("MaxWorkers")
    );
}

#[tokio::test]
async fn an_unknown_environment_is_a_404_before_any_business_rule_runs() {
    let harness = Harness::new();
    // The rules above never turn a missing environment into a 400: the lookup answers
    // first, whatever the patch carries.
    for patch in [
        json!({"PluginsS3Path": "plugins.zip"}),
        json!({"MinWorkers": 5, "MaxWorkers": 1}),
        json!({"AirflowVersion": "9.9.9"}),
        json!({"EnvironmentClass": "mw1.enormous"}),
    ] {
        let answer = harness.update("NoSuchEnvironment", &patch).await;
        assert_eq!(
            answer.status, 404,
            "case {patch}: an update of an unknown environment is a 404, got {} {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ResourceNotFoundException",
            "case {patch}: expected ResourceNotFoundException, got {}",
            answer.body
        );
    }
}
