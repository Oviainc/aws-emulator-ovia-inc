// SPDX-License-Identifier: CC0-1.0

//! The per-class range the scheduler and web-server counts must lie in, end to end over
//! HTTP through the router this service mounts.
//!
//! The rule every case here asserts: a create or an update is refused with HTTP 400
//! `ValidationException` — and leaves the store exactly as it was — when the configuration
//! it would leave the environment in gives a service-scaled unit count a value outside the
//! range the environment's own class accepts; a count inside that range is accepted and
//! reads back through `GetEnvironment`. The range is the one the members' docs fix: the
//! single-instance class runs exactly one of each unit, every larger class runs two
//! through five (see [`FLOOR`] / [`CEILING`] for the source sentence).
//!
//! Each row varies what the rule generalises over: which of the three members carries the
//! count (`Schedulers`, `MinWebservers`, `MaxWebservers`), which class the environment
//! ends up on (all six, so the single-instance special case is one row among them), and
//! the count itself — each published bound, one past each bound, zero, a negative value
//! and a count far above the ceiling. The update tables add the member the request does
//! NOT carry: a patch that only moves the class is judged by the range of the class it
//! moves to.

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

/// Every `EnvironmentClass` the member's own doc lists as valid ("Valid values: mw1.micro,
/// mw1.small, mw1.medium, mw1.large, mw1.xlarge, and mw1.2xlarge"), from
/// `smithy_query(query="doc", shape="CreateEnvironmentInput", member="EnvironmentClass")`.
const ENVIRONMENT_CLASSES: [&str; 6] = [
    "mw1.micro",
    "mw1.small",
    "mw1.medium",
    "mw1.large",
    "mw1.xlarge",
    "mw1.2xlarge",
];

/// The one class the documented range excludes — "For environments larger than mw1.micro,
/// accepts values from 2 to 5" — and so the one that runs [`SINGLE_INSTANCE_COUNT`] of
/// each unit.
const SINGLE_INSTANCE_CLASS: &str = "mw1.micro";

/// The number of each unit the excluded class runs: the count its own docs give as that
/// class's default ("Defaults to 2 for all environment sizes except mw1.micro, which
/// defaults to 1").
const SINGLE_INSTANCE_COUNT: i32 = 1;

/// The published floor of the range a larger class accepts, from the `Schedulers` /
/// `MinWebservers` / `MaxWebservers` docs ("accepts values from 2 to 5"), via
/// `smithy_query(query="doc", shape="CreateEnvironmentInput", member="Schedulers")`.
const FLOOR: i32 = 2;

/// The published ceiling of that same range — see [`FLOOR`].
const CEILING: i32 = 5;

/// The three members whose docs fix the range above. `MinWorkers` / `MaxWorkers` are NOT
/// here: their docs fix no such range, so their only per-member bound is the modelled
/// `@range(min: 1)` the boundary validator enforces.
const RANGED_MEMBERS: [&str; 3] = ["Schedulers", "MinWebservers", "MaxWebservers"];

/// The counts every member is tried with: both published bounds, one below the floor and
/// one above the ceiling, both interior values, zero and a negative count (which
/// `Schedulers`'s modelled `@range(max: 5)` does not exclude), and a count far above the
/// ceiling (which the web-server members' modelled `@range(min: 1)` does not exclude).
const CANDIDATE_COUNTS: [i32; 9] = [-3, 0, 1, 2, 3, 4, 5, 6, 100];

/// The rule under test, restated from the member docs rather than read off the service: an
/// environment on the excluded class runs exactly one of each unit, every larger
/// environment runs [`FLOOR`] through [`CEILING`].
fn accepts(environment_class: &str, count: i32) -> bool {
    if environment_class == SINGLE_INSTANCE_CLASS {
        count == SINGLE_INSTANCE_COUNT
    } else {
        (FLOOR..=CEILING).contains(&count)
    }
}

/// The smallest and largest count `environment_class` accepts, by the same restated rule.
fn bounds(environment_class: &str) -> (i32, i32) {
    if environment_class == SINGLE_INSTANCE_CLASS {
        (SINGLE_INSTANCE_COUNT, SINGLE_INSTANCE_COUNT)
    } else {
        (FLOOR, CEILING)
    }
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

/// Assert a refusal is this service's business-rule refusal: HTTP 400, the modelled
/// `ValidationException`, and a message that names the member the row put out of range.
///
/// The name is matched case-insensitively because a count the MODEL already excludes (a
/// `Schedulers` above its `@range(max: 5)`) is refused by the boundary validator, which
/// names the member by its wire-level lowercase path; either refusal is the right answer
/// for the row, and both have to name the member.
fn assert_refused(case: &str, member: &str, answer: &Answer) {
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
        names_member(&answer.message(), member),
        "case {case}: the refusal must name {member}, got {:?}",
        answer.message()
    );
}

/// Whether `message` names `member`, however the refusal cased it — see [`assert_refused`].
fn names_member(message: &str, member: &str) -> bool {
    message.to_lowercase().contains(&member.to_lowercase())
}

/// `EnvironmentName` is `^[a-zA-Z][0-9a-zA-Z-_]*$`, so a row's environment is named from
/// its table and its index rather than from values that hold dots and minus signs.
fn env_name(table: &str, index: usize) -> String {
    format!("{table}Row{index}")
}

/// The members one row carries: the class it runs on, the member under test at `count`,
/// and — for a web-server count — the OTHER side of that pair at a value the class
/// accepts, so the separate Min<=Max pairing rule cannot be what refuses the row.
fn row_members(environment_class: &str, member: &str, count: i32) -> Value {
    let (floor, ceiling) = bounds(environment_class);
    let mut members = json!({"EnvironmentClass": environment_class, member: count});
    let target = members.as_object_mut().expect("an object of members");
    match member {
        "MinWebservers" => {
            target.insert("MaxWebservers".into(), json!(ceiling));
        }
        "MaxWebservers" => {
            target.insert("MinWebservers".into(), json!(floor));
        }
        _ => {}
    }
    members
}

#[tokio::test]
async fn the_service_enforces_the_documented_bounds() {
    // The emulator's own constants are the documented numbers, so a drift in either
    // direction fails here rather than silently moving the range these rows assert.
    assert_eq!(
        state::MIN_UNIT_COUNT,
        FLOOR,
        "the accepted floor must be the members' documented one"
    );
    assert_eq!(
        state::MAX_UNIT_COUNT,
        CEILING,
        "the accepted ceiling must be the members' documented one"
    );
    assert_eq!(
        state::MICRO_UNIT_COUNT,
        SINGLE_INSTANCE_COUNT,
        "the excluded class must run its documented single instance"
    );
    // And the count a create that names none gets is itself inside the range of the class
    // it lands on, so the documented defaults can never be a configuration the rule
    // refuses.
    for class in ENVIRONMENT_CLASSES {
        let default = state::default_unit_count(class);
        assert!(
            accepts(class, default),
            "case {class}: the default unit count must be inside the class's range, got {default}"
        );
    }
}

#[tokio::test]
async fn a_create_whose_unit_count_is_outside_its_classs_range_is_refused() {
    let harness = Harness::new();
    let mut index = 0;
    for class in ENVIRONMENT_CLASSES {
        for member in RANGED_MEMBERS {
            for count in CANDIDATE_COUNTS {
                let name = env_name("CountCreate", index);
                index += 1;
                let members = row_members(class, member, count);
                let case = format!("create {members}");
                let answer = harness.create(&name, &members).await;
                if accepts(class, count) {
                    assert_eq!(
                        answer.status, 200,
                        "case {case}: expected CreateEnvironment 200, got {} {}",
                        answer.status, answer.body
                    );
                    let environment = harness.environment(&case, &name).await;
                    assert_eq!(
                        environment.get(member),
                        Some(&json!(count)),
                        "case {case}: {member} must read back as the create stored it, got {:?}",
                        environment.get(member)
                    );
                } else {
                    assert_refused(&case, member, &answer);
                    let gone = harness.get(&name).await;
                    assert_eq!(
                        gone.status, 404,
                        "case {case}: a refused create must store nothing, got {} {}",
                        gone.status, gone.body
                    );
                }
            }
        }
    }
}

#[tokio::test]
async fn a_patch_whose_unit_count_is_outside_the_stored_classs_range_is_refused() {
    let harness = Harness::new();
    // One environment per row, each seeded on its row's class with the counts that class
    // accepts, so only the patch's own count can break the rule.
    let mut rows: Vec<(String, &str, &str, i32)> = Vec::new();
    for class in ENVIRONMENT_CLASSES {
        for member in RANGED_MEMBERS {
            for count in CANDIDATE_COUNTS {
                let name = env_name("CountPatch", rows.len());
                let (floor, _) = bounds(class);
                let seed = json!({
                    "EnvironmentClass": class,
                    "Schedulers": floor,
                    "MinWebservers": floor,
                    "MaxWebservers": floor,
                });
                let answer = harness.create(&name, &seed).await;
                assert_eq!(
                    answer.status, 200,
                    "case seed {seed}: the fixture create must succeed, got {} {}",
                    answer.status, answer.body
                );
                rows.push((name, class, member, count));
            }
        }
    }
    // One wait for every row: an update is accepted only once the create has settled.
    tokio::time::sleep(PAST_TRANSITION).await;

    for (name, class, member, count) in &rows {
        let (floor, ceiling) = bounds(class);
        // The patch carries ONLY the member under test (plus, for a web-server count, the
        // other side of its pair), never the class: the stored class is what the range is
        // judged against.
        let mut patch = json!({ *member: count });
        let target = patch.as_object_mut().expect("an object patch");
        match *member {
            "MinWebservers" => {
                target.insert("MaxWebservers".into(), json!(ceiling));
            }
            "MaxWebservers" => {
                target.insert("MinWebservers".into(), json!(floor));
            }
            _ => {}
        }
        let case = format!("patch {patch} of a {class} environment");
        let before = harness.environment(&case, name).await;
        let answer = harness.update(name, &patch).await;
        if accepts(class, *count) {
            assert_eq!(
                answer.status, 200,
                "case {case}: expected UpdateEnvironment 200, got {} {}",
                answer.status, answer.body
            );
            let after = harness.environment(&case, name).await;
            assert_eq!(
                after.get(*member),
                Some(&json!(count)),
                "case {case}: {member} must read back as the patch left it, got {:?}",
                after.get(*member)
            );
        } else {
            assert_refused(&case, member, &answer);
            let after = harness.environment(&case, name).await;
            assert_eq!(
                after, before,
                "case {case}: a refused update must leave the stored record untouched"
            );
        }
    }
}

#[tokio::test]
async fn a_patch_that_moves_the_class_is_judged_by_the_range_of_the_class_it_moves_to() {
    let harness = Harness::new();
    // (label, the class and counts the environment is created on, the patch, accepted)
    let rows: Vec<(&str, Value, Value, bool)> = vec![
        (
            "onto the single-instance class without lowering the counts",
            json!({"EnvironmentClass": "mw1.small", "Schedulers": 3}),
            json!({"EnvironmentClass": SINGLE_INSTANCE_CLASS}),
            false,
        ),
        (
            "onto the single-instance class, lowering the counts in the same request",
            json!({"EnvironmentClass": "mw1.small", "Schedulers": 3}),
            json!({
                "EnvironmentClass": SINGLE_INSTANCE_CLASS,
                "Schedulers": SINGLE_INSTANCE_COUNT,
                "MinWebservers": SINGLE_INSTANCE_COUNT,
                "MaxWebservers": SINGLE_INSTANCE_COUNT,
            }),
            true,
        ),
        (
            "off the single-instance class without raising the counts",
            json!({
                "EnvironmentClass": SINGLE_INSTANCE_CLASS,
                "Schedulers": SINGLE_INSTANCE_COUNT,
            }),
            json!({"EnvironmentClass": "mw1.large"}),
            false,
        ),
        (
            "off the single-instance class, raising the counts in the same request",
            json!({
                "EnvironmentClass": SINGLE_INSTANCE_CLASS,
                "Schedulers": SINGLE_INSTANCE_COUNT,
            }),
            json!({
                "EnvironmentClass": "mw1.large",
                "Schedulers": CEILING,
                "MinWebservers": FLOOR,
                "MaxWebservers": CEILING,
            }),
            true,
        ),
        (
            "between two larger classes, whose ranges are the same",
            json!({"EnvironmentClass": "mw1.medium", "Schedulers": CEILING}),
            json!({"EnvironmentClass": "mw1.2xlarge"}),
            true,
        ),
    ];

    for (index, (label, seed, _, _)) in rows.iter().enumerate() {
        let name = env_name("ClassMove", index);
        let answer = harness.create(&name, seed).await;
        assert_eq!(
            answer.status, 200,
            "case {label}: the fixture create must succeed, got {} {}",
            answer.status, answer.body
        );
    }
    tokio::time::sleep(PAST_TRANSITION).await;

    for (index, (label, _, patch, accepted)) in rows.iter().enumerate() {
        let name = env_name("ClassMove", index);
        let case = format!("{label}: {patch}");
        let before = harness.environment(&case, &name).await;
        let answer = harness.update(&name, patch).await;
        if *accepted {
            assert_eq!(
                answer.status, 200,
                "case {case}: expected UpdateEnvironment 200, got {} {}",
                answer.status, answer.body
            );
            let after = harness.environment(&case, &name).await;
            for (member, value) in patch.as_object().expect("an object patch") {
                assert_eq!(
                    after.get(member),
                    Some(value),
                    "case {case}: {member} must read back as the patch left it, got {:?}",
                    after.get(member)
                );
            }
        } else {
            // The refusal names one of the counts the class it moves to does not run.
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
                RANGED_MEMBERS
                    .iter()
                    .any(|member| names_member(&answer.message(), member)),
                "case {case}: the refusal must name a ranged member, got {:?}",
                answer.message()
            );
            let after = harness.environment(&case, &name).await;
            assert_eq!(
                after, before,
                "case {case}: a refused update must leave the stored record untouched"
            );
        }
    }
}
