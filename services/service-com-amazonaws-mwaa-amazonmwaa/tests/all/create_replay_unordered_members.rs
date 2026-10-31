// SPDX-License-Identifier: CC0-1.0

//! `CreateEnvironment`'s `@idempotent` replay check, judged on members whose value is an
//! unordered COLLECTION, end to end over HTTP through the router this service mounts in
//! production.
//!
//! The rule every case here asserts: whether a second `CreateEnvironment` for a stored
//! name is a replay (the stored ARN, writing nothing) or a different create of a taken
//! name (HTTP 400 `ValidationException`) is decided by what each member MEANS, so for a
//! member the model gives no order — `NetworkConfiguration.SubnetIds` and
//! `.SecurityGroupIds`, which name sets of ids, and the key/value members
//! `AirflowConfigurationOptions` / `Tags` — only the entries decide, never the order the
//! request listed them in. Two creates naming the same entries in different orders are one
//! create; a create whose entries actually differ is still refused.
//!
//! Each table row varies what the rule generalises over: WHICH unordered member is
//! re-ordered (the subnets, the security groups, the configuration options, several at
//! once), HOW MANY entries it carries (the published bounds of each list: 2 subnets, 1 to 5
//! security groups) and HOW the second create differs when it does differ (an entry
//! replaced, dropped, added, duplicated, or an ordered member moved beside a re-ordered
//! one). The second test sweeps EVERY permutation of a stored list rather than one
//! re-ordering, since order-insensitivity is an invariant over permutations.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// A signed-looking request header so the router's `can_handle` resolves this service by
/// its SigV4 credential scope, the way an SDK request does. Signature verification is off
/// in this harness, so the signature value is never checked.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTEST/20260101/us-east-1/airflow/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// The region the `AUTHORIZATION` scope above signs for; every ARN this service mints for
/// these requests carries it.
const REGION: &str = "us-east-1";

/// The canonical emulator account id every emulated ARN carries.
const ACCOUNT: &str = "000000000000";

/// One HTTP answer, reduced to what these tests assert on.
struct Answer {
    status: u16,
    body: Vec<u8>,
}

impl Answer {
    fn json(&self) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "expected a JSON body, got {error}: {}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    /// The wire error code, which restJson1 carries in `__type`.
    fn error_code(&self) -> String {
        self.json()
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .rsplit(['#', '/'])
            .next()
            .unwrap_or_default()
            .to_string()
    }

    fn message(&self) -> String {
        let body = self.json();
        body.get("message")
            .or_else(|| body.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    fn arn(&self) -> Value {
        self.json().get("Arn").cloned().unwrap_or(Value::Null)
    }
}

/// The emulator, assembled exactly as the router assembles it: one shared engine behind
/// one mounted service.
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
        Answer {
            status: response.status().as_u16(),
            body: response.into_body().to_vec(),
        }
    }

    async fn create(&self, name: &str, body: Value) -> Answer {
        self.send("PUT", &format!("/environments/{name}"), Some(body))
            .await
    }

    /// The `Environment` of a successful `GetEnvironment` — the stored record read back.
    async fn environment(&self, name: &str) -> Value {
        let answer = self.send("GET", &format!("/environments/{name}"), None).await;
        assert_eq!(
            answer.status,
            200,
            "GetEnvironment({name}) must succeed, got {} {}",
            answer.status,
            answer.text()
        );
        answer
            .json()
            .get("Environment")
            .cloned()
            .unwrap_or_else(|| panic!("GetEnvironment({name}) answered without an Environment"))
    }

    /// The `NetworkConfiguration` the stored environment reports.
    async fn network_configuration(&self, name: &str) -> Value {
        self.environment(name)
            .await
            .get("NetworkConfiguration")
            .cloned()
            .unwrap_or(Value::Null)
    }
}

/// A create body whose every member satisfies its modelled constraints, placed in the
/// given subnets and security groups and carrying the given Apache Airflow configuration
/// options.
fn create_body(
    subnets: &[&str],
    groups: &[&str],
    options: &[(&str, &str)],
    dag_s3_path: &str,
) -> Value {
    let mut body = json!({
        "ExecutionRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/mwaa-execution"),
        "SourceBucketArn": "arn:aws:s3:::my-airflow-bucket",
        "DagS3Path": dag_s3_path,
        "NetworkConfiguration": {
            "SubnetIds": subnets,
            "SecurityGroupIds": groups,
        },
    });
    if !options.is_empty() {
        body.as_object_mut()
            .expect("the create body is an object")
            .insert(
                "AirflowConfigurationOptions".to_string(),
                Value::Object(
                    options
                        .iter()
                        .map(|(key, value)| ((*key).to_string(), json!(value)))
                        .collect(),
                ),
            );
    }
    body
}

/// How a second create of an already stored name is spelled, and therefore whether it is a
/// replay of the create that stored the environment.
struct Case {
    /// Names the case in every assertion.
    label: &'static str,
    stored_subnets: &'static [&'static str],
    stored_groups: &'static [&'static str],
    stored_options: &'static [(&'static str, &'static str)],
    second_subnets: &'static [&'static str],
    second_groups: &'static [&'static str],
    second_options: &'static [(&'static str, &'static str)],
    /// `DagS3Path` for the second create: an ORDERED member, so a row that moves it asks
    /// for a different environment however its unordered members are spelled.
    second_dag_s3_path: &'static str,
    /// Whether the second create asks for exactly the stored environment.
    replay: bool,
}

/// The `DagS3Path` every row's first create carries.
const STORED_DAG_PATH: &str = "dags";

/// Two subnets — `SubnetList` is `@length(2, 2)`, so every placement names exactly two.
const SUBNETS: &[&str] = &["subnet-0b1c2d3e4f5a6b7c8", "subnet-0a9b8c7d6e5f4a3b2"];
const SUBNETS_REVERSED: &[&str] = &["subnet-0a9b8c7d6e5f4a3b2", "subnet-0b1c2d3e4f5a6b7c8"];

/// Security-group lists at the published bounds of `SecurityGroupList`, `@length(1, 5)`.
const ONE_GROUP: &[&str] = &["sg-0a1b2c3d4e5f60001"];
const THREE_GROUPS: &[&str] = &[
    "sg-0a1b2c3d4e5f60001",
    "sg-0a1b2c3d4e5f60002",
    "sg-0a1b2c3d4e5f60003",
];
const THREE_GROUPS_ROTATED: &[&str] = &[
    "sg-0a1b2c3d4e5f60003",
    "sg-0a1b2c3d4e5f60001",
    "sg-0a1b2c3d4e5f60002",
];
const FIVE_GROUPS: &[&str] = &[
    "sg-0a1b2c3d4e5f60001",
    "sg-0a1b2c3d4e5f60002",
    "sg-0a1b2c3d4e5f60003",
    "sg-0a1b2c3d4e5f60004",
    "sg-0a1b2c3d4e5f60005",
];
const FIVE_GROUPS_REVERSED: &[&str] = &[
    "sg-0a1b2c3d4e5f60005",
    "sg-0a1b2c3d4e5f60004",
    "sg-0a1b2c3d4e5f60003",
    "sg-0a1b2c3d4e5f60002",
    "sg-0a1b2c3d4e5f60001",
];

const OPTIONS: &[(&str, &str)] = &[
    ("core.default_task_retries", "3"),
    ("core.parallelism", "32"),
];
const OPTIONS_REVERSED: &[(&str, &str)] = &[
    ("core.parallelism", "32"),
    ("core.default_task_retries", "3"),
];

/// The table. The replay rows are what the comparison must ACCEPT (a re-ordering of each
/// unordered member, at each published list size); the refusal rows are entry sets that
/// really differ, so the fix cannot be "accept any second create naming the same count".
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "the byte-identical create",
            stored_subnets: SUBNETS,
            stored_groups: ONE_GROUP,
            stored_options: &[],
            second_subnets: SUBNETS,
            second_groups: ONE_GROUP,
            second_options: &[],
            second_dag_s3_path: STORED_DAG_PATH,
            replay: true,
        },
        Case {
            label: "the two subnets listed the other way round",
            stored_subnets: SUBNETS,
            stored_groups: ONE_GROUP,
            stored_options: &[],
            second_subnets: SUBNETS_REVERSED,
            second_groups: ONE_GROUP,
            second_options: &[],
            second_dag_s3_path: STORED_DAG_PATH,
            replay: true,
        },
        Case {
            label: "three security groups rotated",
            stored_subnets: SUBNETS,
            stored_groups: THREE_GROUPS,
            stored_options: &[],
            second_subnets: SUBNETS,
            second_groups: THREE_GROUPS_ROTATED,
            second_options: &[],
            second_dag_s3_path: STORED_DAG_PATH,
            replay: true,
        },
        Case {
            label: "the full five security groups reversed",
            stored_subnets: SUBNETS,
            stored_groups: FIVE_GROUPS,
            stored_options: &[],
            second_subnets: SUBNETS,
            second_groups: FIVE_GROUPS_REVERSED,
            second_options: &[],
            second_dag_s3_path: STORED_DAG_PATH,
            replay: true,
        },
        Case {
            label: "every unordered member re-ordered at once",
            stored_subnets: SUBNETS,
            stored_groups: FIVE_GROUPS,
            stored_options: OPTIONS,
            second_subnets: SUBNETS_REVERSED,
            second_groups: FIVE_GROUPS_REVERSED,
            second_options: OPTIONS_REVERSED,
            second_dag_s3_path: STORED_DAG_PATH,
            replay: true,
        },
        Case {
            label: "the configuration options listed the other way round",
            stored_subnets: SUBNETS,
            stored_groups: THREE_GROUPS,
            stored_options: OPTIONS,
            second_subnets: SUBNETS,
            second_groups: THREE_GROUPS,
            second_options: OPTIONS_REVERSED,
            second_dag_s3_path: STORED_DAG_PATH,
            replay: true,
        },
        Case {
            label: "one subnet replaced by another, the count kept",
            stored_subnets: SUBNETS,
            stored_groups: ONE_GROUP,
            stored_options: &[],
            second_subnets: &["subnet-0b1c2d3e4f5a6b7c8", "subnet-0ffffffffffffffff"],
            second_groups: ONE_GROUP,
            second_options: &[],
            second_dag_s3_path: STORED_DAG_PATH,
            replay: false,
        },
        Case {
            label: "one security group replaced by another, the count kept",
            stored_subnets: SUBNETS,
            stored_groups: THREE_GROUPS,
            stored_options: &[],
            second_subnets: SUBNETS,
            second_groups: &[
                "sg-0a1b2c3d4e5f60003",
                "sg-0a1b2c3d4e5f60009",
                "sg-0a1b2c3d4e5f60001",
            ],
            second_options: &[],
            second_dag_s3_path: STORED_DAG_PATH,
            replay: false,
        },
        Case {
            label: "a security group dropped, the rest re-ordered",
            stored_subnets: SUBNETS,
            stored_groups: THREE_GROUPS,
            stored_options: &[],
            second_subnets: SUBNETS,
            second_groups: &["sg-0a1b2c3d4e5f60002", "sg-0a1b2c3d4e5f60001"],
            second_options: &[],
            second_dag_s3_path: STORED_DAG_PATH,
            replay: false,
        },
        Case {
            label: "a security group added before the stored one",
            stored_subnets: SUBNETS,
            stored_groups: ONE_GROUP,
            stored_options: &[],
            second_subnets: SUBNETS,
            second_groups: &["sg-0a1b2c3d4e5f60002", "sg-0a1b2c3d4e5f60001"],
            second_options: &[],
            second_dag_s3_path: STORED_DAG_PATH,
            replay: false,
        },
        Case {
            label: "a stored group repeated in place of another",
            stored_subnets: SUBNETS,
            stored_groups: &["sg-0a1b2c3d4e5f60001", "sg-0a1b2c3d4e5f60002"],
            stored_options: &[],
            second_subnets: SUBNETS,
            second_groups: &["sg-0a1b2c3d4e5f60001", "sg-0a1b2c3d4e5f60001"],
            second_options: &[],
            second_dag_s3_path: STORED_DAG_PATH,
            replay: false,
        },
        Case {
            label: "one configuration option's value changed, the pairs re-ordered",
            stored_subnets: SUBNETS,
            stored_groups: ONE_GROUP,
            stored_options: OPTIONS,
            second_subnets: SUBNETS,
            second_groups: ONE_GROUP,
            second_options: &[
                ("core.parallelism", "64"),
                ("core.default_task_retries", "3"),
            ],
            second_dag_s3_path: STORED_DAG_PATH,
            replay: false,
        },
        Case {
            label: "an ordered member moved beside re-ordered unordered ones",
            stored_subnets: SUBNETS,
            stored_groups: THREE_GROUPS,
            stored_options: OPTIONS,
            second_subnets: SUBNETS_REVERSED,
            second_groups: THREE_GROUPS_ROTATED,
            second_options: OPTIONS_REVERSED,
            second_dag_s3_path: "somewhere/else",
            replay: false,
        },
    ]
}

impl Case {
    fn first_body(&self) -> Value {
        create_body(
            self.stored_subnets,
            self.stored_groups,
            self.stored_options,
            STORED_DAG_PATH,
        )
    }

    fn second_body(&self) -> Value {
        create_body(
            self.second_subnets,
            self.second_groups,
            self.second_options,
            self.second_dag_s3_path,
        )
    }
}

#[tokio::test]
async fn a_second_create_replays_when_only_the_order_of_an_unordered_member_differs() {
    let harness = Harness::new();

    for (index, case) in cases().into_iter().enumerate() {
        let name = format!("OrderCase{index:02}");
        let arn = format!("arn:aws:airflow:{REGION}:{ACCOUNT}:environment/{name}");

        let first = harness.create(&name, case.first_body()).await;
        assert_eq!(
            first.status,
            200,
            "case {:?}: the first create must succeed, got {} {}",
            case.label,
            first.status,
            first.text()
        );

        // The stored placement, as the create that stored it wrote it: the comparison below
        // must not change what a reader reports.
        let stored_network = json!({
            "SubnetIds": case.stored_subnets,
            "SecurityGroupIds": case.stored_groups,
        });
        let before = harness.network_configuration(&name).await;
        assert_eq!(
            before, stored_network,
            "case {:?}: the create must store the placement it named, got {before:?}",
            case.label
        );

        let second = harness.create(&name, case.second_body()).await;
        if case.replay {
            assert_eq!(
                second.status,
                200,
                "case {:?}: a create naming the same entries of every unordered member is a \
                 replay and must succeed, got {} {}",
                case.label,
                second.status,
                second.text()
            );
            assert_eq!(
                second.arn(),
                json!(arn),
                "case {:?}: a replay must answer the stored ARN, got {:?}",
                case.label,
                second.arn()
            );
        } else {
            assert_eq!(
                second.status,
                400,
                "case {:?}: a create of a taken name whose entries differ must be refused, \
                 got {} {}",
                case.label,
                second.status,
                second.text()
            );
            assert_eq!(
                second.error_code(),
                "ValidationException",
                "case {:?}: a name clash is a ValidationException, got {:?}",
                case.label,
                second.error_code()
            );
            let message = second.message();
            assert!(
                message.contains(&name) && message.contains("already exists"),
                "case {:?}: the refusal must name the environment whose name is taken, got \
                 {message:?}",
                case.label
            );
        }

        // Either way the environment is the one the FIRST create stored, in the order it
        // listed: a replay writes nothing, and a refused create writes nothing.
        let after = harness.network_configuration(&name).await;
        assert_eq!(
            after, stored_network,
            "case {:?}: the second create must leave the stored placement exactly as it was, \
             got {after:?}",
            case.label
        );
    }
}

/// Every ordering of `items`, in a fixed (lexicographic-by-index) order, so the sweep below
/// is exhaustive rather than sampled and needs no seed.
fn permutations(items: &[&str]) -> Vec<Vec<String>> {
    if items.is_empty() {
        return vec![Vec::new()];
    }
    let mut out = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let mut rest: Vec<&str> = items.to_vec();
        rest.remove(index);
        for mut tail in permutations(&rest) {
            let mut ordering = vec![(*item).to_string()];
            ordering.append(&mut tail);
            out.push(ordering);
        }
    }
    out
}

#[tokio::test]
async fn every_permutation_of_the_stored_placement_replays() {
    let harness = Harness::new();
    let name = "OrderSweep";
    let arn = format!("arn:aws:airflow:{REGION}:{ACCOUNT}:environment/{name}");
    // Four security groups (inside `SecurityGroupList`'s published 1..5) times the two
    // orderings of the two subnets: 48 spellings of ONE placement.
    let groups: &[&str] = &[
        "sg-0a1b2c3d4e5f60001",
        "sg-0a1b2c3d4e5f60002",
        "sg-0a1b2c3d4e5f60003",
        "sg-0a1b2c3d4e5f60004",
    ];

    let first = harness
        .create(name, create_body(SUBNETS, groups, OPTIONS, STORED_DAG_PATH))
        .await;
    assert_eq!(
        first.status,
        200,
        "the create that stores the environment must succeed, got {} {}",
        first.status,
        first.text()
    );
    let stored_network = json!({"SubnetIds": SUBNETS, "SecurityGroupIds": groups});

    let group_orderings = permutations(groups);
    assert_eq!(
        group_orderings.len(),
        24,
        "the sweep must cover every ordering of the four security groups, got {}",
        group_orderings.len()
    );
    for subnets in permutations(SUBNETS) {
        for ordering in &group_orderings {
            let case = (subnets.clone(), ordering.clone());
            let subnet_refs: Vec<&str> = subnets.iter().map(String::as_str).collect();
            let group_refs: Vec<&str> = ordering.iter().map(String::as_str).collect();
            let replay = harness
                .create(
                    name,
                    create_body(&subnet_refs, &group_refs, OPTIONS, STORED_DAG_PATH),
                )
                .await;
            assert_eq!(
                replay.status,
                200,
                "case {case:?}: the same placement spelled in another order is the same \
                 create and must replay, got {} {}",
                replay.status,
                replay.text()
            );
            assert_eq!(
                replay.arn(),
                json!(arn),
                "case {case:?}: a replay must answer the stored ARN, got {:?}",
                replay.arn()
            );
            let after = harness.network_configuration(name).await;
            assert_eq!(
                after, stored_network,
                "case {case:?}: a replay must leave the stored placement as the create wrote \
                 it, got {after:?}",
                );
        }
    }
}
