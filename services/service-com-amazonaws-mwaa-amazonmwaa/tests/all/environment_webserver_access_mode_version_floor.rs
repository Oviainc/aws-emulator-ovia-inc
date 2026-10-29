// SPDX-License-Identifier: CC0-1.0

//! The Apache Airflow release floor a member value carries, end to end over HTTP through
//! the router this service mounts.
//!
//! The rule every case here asserts: a create or an update is refused with HTTP 400
//! `ValidationException` — and leaves the store exactly as it was — when the configuration
//! it would leave the environment in gives a member a value the member's own doc makes
//! available only from a later Apache Airflow release than the environment's resolved
//! `AirflowVersion`; a value the resolved release offers is accepted and reads back through
//! `GetEnvironment`.
//!
//! `WebserverAccessMode`'s both-public-and-private mode is the one value this service's
//! model floors that way ("This mode is only available for Apache Airflow version 3.2 and
//! later" — see [`FLOORED_MODE`] / [`MODE_FLOOR`]), so the rows vary what the rule
//! generalises over: which mode the request names (all three the model lists), which
//! release the environment resolves to (every version the `AirflowVersion` doc lists, plus
//! the create that names none and so gets the documented default), whether the request
//! carries the mode, the version, or both, and whether it is a create or a patch of a
//! stored environment. The release comparison itself is asserted over its own table: each
//! published bound, one either side of it, and a release the issue did not cite.

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

/// Every `WebserverAccessMode` value, from
/// `smithy_query(query="shape", shape="WebserverAccessMode")`.
const WEBSERVER_ACCESS_MODES: [&str; 3] = ["PRIVATE_ONLY", "PUBLIC_AND_PRIVATE", "PUBLIC_ONLY"];

/// The one mode whose doc states a release floor, from
/// `smithy_query(query="doc", shape="CreateEnvironmentInput", member="WebserverAccessMode")`:
/// "If set to PUBLIC_AND_PRIVATE, creates both a public network load balancer (NLB) for
/// browser access and a private VPC endpoint (VPCE) for worker-to-webserver communication.
/// This mode is only available for Apache Airflow version 3.2 and later."
const FLOORED_MODE: &str = "PUBLIC_AND_PRIVATE";

/// The release floor that same sentence states for [`FLOORED_MODE`].
const MODE_FLOOR: &str = "3.2";

/// Every `AirflowVersion` the member's own doc lists as valid ("Valid values: 2.7.2, 2.8.1,
/// 2.9.2, 2.10.1, 2.10.3, 2.11.0, and 3.0.6"), from
/// `smithy_query(query="doc", shape="CreateEnvironmentInput", member="AirflowVersion")`.
const AIRFLOW_VERSIONS: [&str; 7] = [
    "2.7.2", "2.8.1", "2.9.2", "2.10.1", "2.10.3", "2.11.0", "3.0.6",
];

/// The rule under test, restated from the member docs rather than read off the service: a
/// mode with no release floor is available on every release the service runs, and the
/// floored mode only from its floor onwards.
fn mode_available(mode: &str, airflow_version: &str) -> bool {
    mode != FLOORED_MODE || release_at_least(airflow_version, MODE_FLOOR)
}

/// Whether `version` is the release `floor` names or a later one — the comparison the rule
/// needs, restated here component by component so these rows do not borrow the service's
/// own implementation of it.
fn release_at_least(version: &str, floor: &str) -> bool {
    let components = |release: &str| -> Vec<u64> {
        release
            .split('.')
            .map(|component| component.parse::<u64>().unwrap_or(0))
            .collect()
    };
    let (version, floor) = (components(version), components(floor));
    let width = version.len().max(floor.len());
    let padded = |release: &[u64]| -> Vec<u64> {
        let mut values = release.to_vec();
        values.resize(width, 0);
        values
    };
    padded(&version) >= padded(&floor)
}

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
        let mut body = json!({
            "ExecutionRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/mwaa-execution"),
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
        self.send("PUT", &format!("/environments/{name}"), Some(body))
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

/// Assert a refusal is this service's business-rule refusal for this rule: HTTP 400, the
/// modelled `ValidationException`, and a message that names the member, the mode it
/// refused, the release the environment resolves to and the floor the mode needs.
fn assert_refused(case: &str, airflow_version: &str, answer: &Answer) {
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
    let message = answer.message();
    for expected in [
        "WebserverAccessMode",
        FLOORED_MODE,
        airflow_version,
        MODE_FLOOR,
    ] {
        assert!(
            message.contains(expected),
            "case {case}: the refusal must name {expected}, got {message:?}"
        );
    }
}

/// `EnvironmentName` is `^[a-zA-Z][0-9a-zA-Z-_]*$`, so a row's environment is named from
/// its table and its index rather than from values that hold dots and underscores.
fn env_name(table: &str, index: usize) -> String {
    format!("{table}Row{index}")
}

/// Every row of the create/patch tables: each mode against each release the service runs,
/// plus the request that names no release at all (`None`, which resolves to the documented
/// default).
fn rows() -> Vec<(&'static str, Option<&'static str>)> {
    let mut rows: Vec<(&'static str, Option<&'static str>)> = Vec::new();
    for mode in WEBSERVER_ACCESS_MODES {
        rows.push((mode, None));
        for version in AIRFLOW_VERSIONS {
            rows.push((mode, Some(version)));
        }
    }
    rows
}

/// The release a row's environment ends up running: the one it names, or the documented
/// default for a request that names none ("If no value is specified, it defaults to the
/// latest version").
fn resolved_version(requested: Option<&str>) -> String {
    requested
        .unwrap_or(state::LATEST_AIRFLOW_VERSION)
        .to_string()
}

#[tokio::test]
async fn the_service_enforces_the_documented_release_floor() {
    // The emulator's own constants are the documented mode and release, so a drift in
    // either fails here rather than silently moving the floor these rows assert.
    assert_eq!(
        state::WEBSERVER_ACCESS_MODE_PUBLIC_AND_PRIVATE,
        FLOORED_MODE,
        "the floored mode must be the one the member's doc names"
    );
    assert_eq!(
        state::PUBLIC_AND_PRIVATE_MIN_AIRFLOW_VERSION,
        MODE_FLOOR,
        "the floor must be the release the member's doc names"
    );
    // The mode a create that names none gets is itself available on every release the
    // service runs, so the documented default can never be a configuration the rule
    // refuses.
    for version in AIRFLOW_VERSIONS {
        assert!(
            mode_available(state::DEFAULT_WEBSERVER_ACCESS_MODE, version),
            "case {version}: the default access mode must be available on every supported release"
        );
    }
}

#[tokio::test]
async fn the_release_comparison_holds_at_each_bound() {
    // Rows: the floor itself and its zero-padded spelling, the release just below it in
    // each component, the releases above it (including a two-digit minor, which a text
    // comparison would sort below a one-digit one), every release the service runs, and a
    // release from a line the service does not run. `true` means "at or above the floor".
    let rows: [(&str, &str, bool); 13] = [
        (MODE_FLOOR, MODE_FLOOR, true),
        ("3.2.0", MODE_FLOOR, true),
        ("3.2.1", MODE_FLOOR, true),
        ("3.3", MODE_FLOOR, true),
        ("3.10.0", MODE_FLOOR, true),
        ("4.0.0", MODE_FLOOR, true),
        ("3.1.9", MODE_FLOOR, false),
        ("3.0.6", MODE_FLOOR, false),
        ("2.11.0", MODE_FLOOR, false),
        ("1.10.12", MODE_FLOOR, false),
        // The floor read against itself from the other side, and two releases of the same
        // line compared with each other: the comparison is numeric, not textual.
        ("2.9.2", "2.10.1", false),
        ("2.10.1", "2.9.2", true),
        ("2.10.3", "2.10.3", true),
    ];
    for (version, floor, expected) in rows {
        let case = format!("{version} against {floor}");
        let actual = state::airflow_version_at_least(version, floor);
        assert_eq!(
            actual, expected,
            "case {case}: expected at-or-above = {expected}, got {actual}"
        );
        assert_eq!(
            actual,
            release_at_least(version, floor),
            "case {case}: the service must agree with the restated comparison, got {actual}"
        );
    }
}

#[tokio::test]
async fn a_create_whose_release_does_not_offer_the_mode_is_refused() {
    let harness = Harness::new();
    for (index, (mode, version)) in rows().into_iter().enumerate() {
        let name = env_name("ModeCreate", index);
        let mut members = json!({"WebserverAccessMode": mode});
        if let Some(version) = version {
            members
                .as_object_mut()
                .expect("an object of members")
                .insert("AirflowVersion".into(), json!(version));
        }
        let resolved = resolved_version(version);
        let case = format!("create {members} resolving to {resolved}");
        let answer = harness.create(&name, &members).await;
        if mode_available(mode, &resolved) {
            assert_eq!(
                answer.status, 200,
                "case {case}: expected CreateEnvironment 200, got {} {}",
                answer.status, answer.body
            );
            let environment = harness.environment(&case, &name).await;
            assert_eq!(
                environment.get("WebserverAccessMode"),
                Some(&json!(mode)),
                "case {case}: WebserverAccessMode must read back as the create stored it, got {:?}",
                environment.get("WebserverAccessMode")
            );
            assert_eq!(
                environment.get("AirflowVersion"),
                Some(&json!(resolved)),
                "case {case}: AirflowVersion must read back as the create resolved it, got {:?}",
                environment.get("AirflowVersion")
            );
        } else {
            assert_refused(&case, &resolved, &answer);
            let gone = harness.get(&name).await;
            assert_eq!(
                gone.status, 404,
                "case {case}: a refused create must store nothing, got {} {}",
                gone.status, gone.body
            );
        }
    }
}

#[tokio::test]
async fn a_patch_is_judged_by_the_release_the_merged_configuration_runs() {
    let harness = Harness::new();
    // Each row gets its own environment, seeded on a release and a mode the rule accepts,
    // so only what the patch merges in can break it. The patch carries the mode, the
    // release, or both: a patch naming only one side is judged against the value already
    // stored for the other.
    let mut cases: Vec<(String, String, Value, String)> = Vec::new();
    for (index, (mode, version)) in rows().into_iter().enumerate() {
        let name = env_name("ModePatch", index);
        let seed_version = AIRFLOW_VERSIONS[index % AIRFLOW_VERSIONS.len()];
        let seed = json!({
            "AirflowVersion": seed_version,
            "WebserverAccessMode": state::DEFAULT_WEBSERVER_ACCESS_MODE,
        });
        let answer = harness.create(&name, &seed).await;
        assert_eq!(
            answer.status, 200,
            "case seed {seed}: the fixture create must succeed, got {} {}",
            answer.status, answer.body
        );
        // The patch's mode, and its release when the row names one; otherwise the release
        // the seed stored is what the mode is judged against.
        let mut patch = json!({"WebserverAccessMode": mode});
        if let Some(version) = version {
            patch
                .as_object_mut()
                .expect("an object patch")
                .insert("AirflowVersion".into(), json!(version));
        }
        let resolved = version.unwrap_or(seed_version).to_string();
        let case = format!("patch {patch} of an environment running {seed_version}");
        cases.push((name, case, patch, resolved));
    }
    // One wait for every row: an update is accepted only once the create has settled.
    tokio::time::sleep(PAST_TRANSITION).await;

    let mut accepted: Vec<(String, String, Value)> = Vec::new();
    for (name, case, patch, resolved) in &cases {
        let mode = patch["WebserverAccessMode"]
            .as_str()
            .expect("a mode in the patch");
        let before = harness.environment(case, name).await;
        let answer = harness.update(name, patch).await;
        if mode_available(mode, resolved) {
            assert_eq!(
                answer.status, 200,
                "case {case}: expected UpdateEnvironment 200, got {} {}",
                answer.status, answer.body
            );
            accepted.push((name.clone(), case.clone(), patch.clone()));
        } else {
            assert_refused(case, resolved, &answer);
            let after = harness.environment(case, name).await;
            assert_eq!(
                after, before,
                "case {case}: a refused update must leave the stored record untouched"
            );
        }
    }
    // An accepted patch is read back only once the phase that APPLIES the update has come
    // due: the environment's database is snapshotted first, so a read before that reports
    // the configuration the environment is still running on. One wait for every row.
    tokio::time::sleep(PAST_TRANSITION).await;

    for (name, case, patch) in &accepted {
        let after = harness.environment(case, name).await;
        for (member, value) in patch.as_object().expect("an object patch") {
            assert_eq!(
                after.get(member),
                Some(value),
                "case {case}: {member} must read back as the patch left it, got {:?}",
                after.get(member)
            );
        }
    }
}
