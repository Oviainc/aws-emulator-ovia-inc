// SPDX-License-Identifier: CC0-1.0

//! Reading a Lightsail container service's deployment history over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: `GetContainerServiceDeployments` reports every
//! deployment the named container service holds — the one running, the one still
//! being created and the ones that ran before — newest version first, each
//! carrying the configuration of the request that created it and the state it is
//! in. A service that has never been deployed to reports an EMPTY list, and a
//! name the request's region holds no container service under is the service's
//! `DoesNotExist` rejection.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::instance_lifecycle::error_code;

/// The `X-Amz-Target` prefix this service routes on (`OPERATIONS.md`, "Wire
/// protocol").
const TARGET_PREFIX: &str = "Lightsail_20161128";

/// The endpoint every call in this suite reaches the emulator on.
const ENDPOINT_HOST: &str = "127.0.0.1:9297";

// The deployment states a stored deployment reports, from the modelled
// `ContainerServiceDeploymentState` enum
// (`smithy_query(query="shape",shape="ContainerServiceDeploymentState")`, whose
// values are `ACTIVATING`, `ACTIVE`, `INACTIVE` and `FAILED`). Nothing in this
// emulator fails a deployment, so `FAILED` is not among the states a history may
// report here.
/// A deployment that is being created.
const ACTIVATING: &str = "ACTIVATING";
/// The deployment currently running on the service.
const ACTIVE: &str = "ACTIVE";
/// A deployment that was created and is not running.
const INACTIVE: &str = "INACTIVE";

/// One emulator with one Lightsail service mounted on one shared engine.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suite signs nothing; routing is by `X-Amz-Target`, and the
        // `Authorization` header below carries only the credential SCOPE, which
        // is what names the region a request is made in.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_lightsail_lightsail_20161128::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// Call `operation` in `region` with `body`, answering its HTTP status and
    /// parsed body.
    async fn call(&self, region: &str, operation: &str, body: Value) -> (u16, Value) {
        let request = ::http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", ENDPOINT_HOST)
            .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
            .header("content-type", "application/x-amz-json-1.1")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261022/{region}/lightsail/\
                     aws4_request, SignedHeaders=host, Signature=not-verified"
                ),
            )
            .body(bytes::Bytes::from(body.to_string()))
            .expect("the request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Create the container service `body` describes. `CreateContainerService`
    /// answers HTTP 201.
    async fn create(&self, case: &str, region: &str, body: Value) {
        let (status, parsed) = self.call(region, "CreateContainerService", body).await;
        assert_eq!(
            status, 201,
            "case {case}: expected HTTP 201 from the create in {region}, got {status} {parsed:?}"
        );
    }

    /// Deploy `body` to a container service, answering the status and the parsed
    /// body so a refusal can be inspected too.
    async fn deploy(&self, region: &str, body: Value) -> (u16, Value) {
        self.call(region, "CreateContainerServiceDeployment", body)
            .await
    }

    /// The deployment history of `name` in `region`, as
    /// `GetContainerServiceDeployments` reports it.
    async fn history(&self, case: &str, region: &str, name: &str) -> Vec<Value> {
        let (status, parsed) = self.history_raw(region, name).await;
        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200 reading the deployments of {name} in {region}, got \
             {status} {parsed:?}"
        );
        parsed["deployments"]
            .as_array()
            .cloned()
            .unwrap_or_else(|| panic!("case {case}: expected a deployment list, got {parsed:?}"))
    }

    /// The raw answer of `GetContainerServiceDeployments`, for a case that
    /// expects a refusal.
    async fn history_raw(&self, region: &str, name: &str) -> (u16, Value) {
        self.call(
            region,
            "GetContainerServiceDeployments",
            json!({"serviceName": name}),
        )
        .await
    }

    /// The stored container service `name` in `region`, as `GetContainerServices`
    /// reports it.
    async fn read(&self, case: &str, region: &str, name: &str) -> Value {
        let (status, parsed) = self
            .call(region, "GetContainerServices", json!({"serviceName": name}))
            .await;
        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200 reading {name} in {region}, got {status} {parsed:?}"
        );
        parsed["containerServices"][0].clone()
    }

    /// Poll `GetContainerServices` until the service reports `RUNNING`, which is
    /// the phase it settles into once the waiting deployment is the running one.
    /// The transition is owed by the clock, not by the poll.
    async fn settle(&self, case: &str, region: &str, name: &str) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let service = self.read(case, region, name).await;
            if service["state"].as_str() == Some("RUNNING") {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "case {case}: expected {name} to reach RUNNING within 5s, got {:?}",
                service["state"]
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }
}

// ---------------------------------------------------------------------------
// The table
// ---------------------------------------------------------------------------

/// What one call does to a service's deployment history.
#[derive(Debug, Clone, Copy)]
enum Step {
    /// Create a deployment of `containers` containers, fronted by a public
    /// endpoint on the first of them when `fronted`.
    Deploy { containers: usize, fronted: bool },
    /// Let the clock settle the service, so the deployment that was waiting
    /// becomes the running one.
    Settle,
}

/// One service and the calls made against it, in order. The rows differ along
/// everything the rule generalises over: the region the calls are made in,
/// whether the service was created carrying a deployment already, how many
/// deployments the history holds, how many containers each describes, whether a
/// deployment is fronted by a public endpoint, and whether the clock activated a
/// deployment before the next one superseded it.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    region: &'static str,
    name: &'static str,
    /// Whether `CreateContainerService` carries a deployment of its own, which
    /// spends version 1 of the service's sequence.
    created_with_deployment: bool,
    steps: &'static [Step],
}

const ROWS: &[Row] = &[
    Row {
        case: "a service never deployed to",
        region: "us-east-1",
        name: "history-empty",
        created_with_deployment: false,
        steps: &[],
    },
    Row {
        case: "one deployment, settled",
        region: "us-east-1",
        name: "history-one",
        created_with_deployment: false,
        steps: &[
            Step::Deploy {
                containers: 1,
                fronted: true,
            },
            Step::Settle,
        ],
    },
    Row {
        case: "a service created carrying a deployment, deployed to twice more",
        region: "us-west-2",
        name: "history-seeded",
        created_with_deployment: true,
        steps: &[
            Step::Settle,
            Step::Deploy {
                containers: 2,
                fronted: true,
            },
            Step::Settle,
            Step::Deploy {
                containers: 1,
                fronted: false,
            },
        ],
    },
    Row {
        case: "three deployments in a row, none settled in between",
        region: "eu-west-1",
        name: "history-superseded",
        created_with_deployment: false,
        steps: &[
            Step::Deploy {
                containers: 1,
                fronted: false,
            },
            Step::Deploy {
                containers: 3,
                fronted: true,
            },
            // Ten is the most containers one deployment may describe
            // (`crate::state::MAX_CONTAINERS_PER_DEPLOYMENT`).
            Step::Deploy {
                containers: 10,
                fronted: true,
            },
            Step::Settle,
        ],
    },
];

/// One deployment the history is expected to hold: its version, and the shape of
/// the request that created it.
#[derive(Debug, Clone, Copy)]
struct Expected {
    version: i64,
    containers: usize,
    fronted: bool,
}

/// The `containers` of a deployment at `version`: `c0`..`c{count-1}`, each
/// running an image and carrying an environment that names that version, and the
/// first one opening port 80 so a public endpoint can front it.
fn containers_json(version: i64, count: usize) -> Value {
    let mut containers = Map::new();
    for index in 0..count {
        let mut entry = json!({
            "image": format!("registry/app{index}:v{version}"),
            "environment": {"VERSION": version.to_string()},
        });
        if index == 0 {
            entry["ports"] = json!({"80": "HTTP"});
        }
        containers.insert(format!("c{index}"), entry);
    }
    Value::Object(containers)
}

/// The public endpoint a fronted deployment asks for.
fn endpoint_json() -> Value {
    json!({"containerName": "c0", "containerPort": 80})
}

/// The `CreateContainerService` body for `row`.
fn create_body(row: &Row) -> Value {
    let mut body = json!({"serviceName": row.name, "power": "nano", "scale": 1});
    if row.created_with_deployment {
        body["deployment"] = json!({
            "containers": containers_json(1, 1),
            "publicEndpoint": endpoint_json(),
        });
    }
    body
}

/// The `CreateContainerServiceDeployment` body for the deployment `name` will
/// store at `version`.
fn deploy_body(name: &str, version: i64, containers: usize, fronted: bool) -> Value {
    let mut body = json!({"serviceName": name, "containers": containers_json(version, containers)});
    if fronted {
        body["publicEndpoint"] = endpoint_json();
    }
    body
}

/// Assert `entry` is the deployment `expected` describes: its version, its
/// creation time, and the containers and public endpoint of the request that
/// created it.
fn assert_entry(case: &str, entry: &Value, expected: Expected) {
    let Expected {
        version,
        containers,
        fronted,
    } = expected;
    assert_eq!(
        entry["version"].as_i64(),
        Some(version),
        "case {case}: expected deployment version {version}, got {entry:?}"
    );
    assert!(
        entry["createdAt"].as_f64().is_some(),
        "case {case}: expected a creation timestamp on deployment {version}, got {entry:?}"
    );
    assert_eq!(
        entry["containers"].as_object().map(Map::len),
        Some(containers),
        "case {case}: expected the {containers} containers of version {version}, got {:?}",
        entry["containers"]
    );
    for index in 0..containers {
        let got = &entry["containers"][format!("c{index}")];
        assert_eq!(
            got["image"].as_str(),
            Some(format!("registry/app{index}:v{version}").as_str()),
            "case {case}: expected container c{index} of version {version} to keep its image, \
             got {got:?}"
        );
        assert_eq!(
            got["environment"]["VERSION"].as_str(),
            Some(version.to_string().as_str()),
            "case {case}: expected the environment of c{index} at version {version}, got {got:?}"
        );
    }
    assert_eq!(
        entry["containers"]["c0"]["ports"]["80"].as_str(),
        Some("HTTP"),
        "case {case}: expected port 80 open on c0 of version {version}, got {:?}",
        entry["containers"]["c0"]
    );
    let reported = &entry["publicEndpoint"];
    if fronted {
        assert_eq!(
            reported["containerName"].as_str(),
            Some("c0"),
            "case {case}: expected version {version} fronted by c0, got {reported:?}"
        );
        assert_eq!(
            reported["containerPort"].as_i64(),
            Some(80),
            "case {case}: expected version {version} fronted on port 80, got {reported:?}"
        );
    } else {
        assert!(
            reported.is_null(),
            "case {case}: version {version} fronts nothing, so it reports no public endpoint, \
             got {reported:?}"
        );
    }
}

/// Assert `history` is exactly the deployments `want` describes, newest version
/// first, and that it holds the lifecycle invariants the model states: every
/// state is one this emulator stores, at most one deployment runs, at most one
/// is being created, and the one being created is the newest.
fn assert_history(case: &str, history: &[Value], want: &[Expected]) {
    assert_eq!(
        history.len(),
        want.len(),
        "case {case}: expected {} deployments in the history, got {history:?}",
        want.len()
    );
    for (entry, expected) in history.iter().zip(want) {
        assert_entry(case, entry, *expected);
    }
    for pair in history.windows(2) {
        assert!(
            pair[0]["version"].as_i64() > pair[1]["version"].as_i64(),
            "case {case}: expected the newest version at the top, got {:?} before {:?}",
            pair[0]["version"],
            pair[1]["version"]
        );
    }

    let states: Vec<&str> = history
        .iter()
        .map(|entry| entry["state"].as_str().unwrap_or("<no state>"))
        .collect();
    for (index, state) in states.iter().enumerate() {
        assert!(
            [ACTIVATING, ACTIVE, INACTIVE].contains(state),
            "case {case}: deployment {index} reports the state {state}, which is none this \
             emulator stores"
        );
    }
    assert!(
        states.iter().filter(|state| **state == ACTIVE).count() <= 1,
        "case {case}: only one deployment runs on a service at a time, got {states:?}"
    );
    let activating = states.iter().filter(|state| **state == ACTIVATING).count();
    assert!(
        activating <= 1,
        "case {case}: only one deployment is being created at a time, got {states:?}"
    );
    if activating == 1 {
        assert_eq!(
            states.first().copied(),
            Some(ACTIVATING),
            "case {case}: the deployment being created is the newest one, got {states:?}"
        );
    }
}

/// RULE: the history reports every deployment the service holds, newest version
/// first, each with the configuration of the request that created it; the clock
/// is what moves the waiting deployment to running and steps the one that ran
/// before it down, and a read reports that without changing it.
#[tokio::test]
async fn the_history_reports_every_deployment_newest_version_first() {
    let emulator = Emulator::new();
    for row in ROWS {
        emulator.create(row.case, row.region, create_body(row)).await;

        // A service created carrying a deployment holds that one already, at
        // version 1 of its own sequence.
        let mut want: Vec<Expected> = Vec::new();
        if row.created_with_deployment {
            want.push(Expected {
                version: 1,
                containers: 1,
                fronted: true,
            });
        }
        let mut next_version = want.len() as i64 + 1;

        let as_created = format!("{} (as created)", row.case);
        let history = emulator.history(&as_created, row.region, row.name).await;
        assert_history(&as_created, &history, &want);

        for (index, step) in row.steps.iter().enumerate() {
            let case = format!("{} (step {index})", row.case);
            match *step {
                Step::Deploy {
                    containers,
                    fronted,
                } => {
                    let version = next_version;
                    next_version += 1;
                    let (status, answered) = emulator
                        .deploy(
                            row.region,
                            deploy_body(row.name, version, containers, fronted),
                        )
                        .await;
                    assert_eq!(
                        status, 201,
                        "case {case}: expected HTTP 201 from the deployment, got {status} \
                         {answered:?}"
                    );
                    want.insert(
                        0,
                        Expected {
                            version,
                            containers,
                            fronted,
                        },
                    );
                }
                Step::Settle => emulator.settle(&case, row.region, row.name).await,
            }

            let history = emulator.history(&case, row.region, row.name).await;
            assert_history(&case, &history, &want);

            if !matches!(step, Step::Settle) {
                continue;
            }
            // A settled service owes no further transition, so the history is
            // fixed: the newest deployment runs and every older one has stepped
            // down — and reading it again reports the very same thing.
            assert_eq!(
                history[0]["state"].as_str(),
                Some(ACTIVE),
                "case {case}: expected the newest deployment to be the running one, got {:?}",
                history[0]
            );
            for older in history.iter().skip(1) {
                assert_eq!(
                    older["state"].as_str(),
                    Some(INACTIVE),
                    "case {case}: expected a deployment that ran before to have stepped down, \
                     got {older:?}"
                );
            }
            let again = emulator.history(&case, row.region, row.name).await;
            assert_eq!(
                again, history,
                "case {case}: a read must not change the history it reports"
            );
            // The service record and the history agree about which deployment
            // runs, since both are read out of the one stored history.
            let service = emulator.read(&case, row.region, row.name).await;
            assert_eq!(
                service["currentDeployment"], history[0],
                "case {case}: expected the service to report the newest deployment as its \
                 current one, got {:?}",
                service["currentDeployment"]
            );
            assert!(
                service["nextDeployment"].is_null(),
                "case {case}: a settled service has no waiting deployment, got {:?}",
                service["nextDeployment"]
            );
        }
    }
}

/// RULE: a container service is a regional resource, so the history a request
/// gets is the one of the service that request's region holds under that name —
/// never another region's.
#[tokio::test]
async fn the_history_is_the_one_of_the_service_in_the_requests_region() {
    let emulator = Emulator::new();
    let name = "history-regional";

    // (the region, how many deployments the service there is given)
    let rows = [("us-east-1", 1_i64), ("ap-southeast-2", 3)];
    for (region, deployments) in rows {
        emulator
            .create(
                "a regional service",
                region,
                json!({"serviceName": name, "power": "micro", "scale": 2}),
            )
            .await;
        for version in 1..=deployments {
            let (status, answered) = emulator
                .deploy(region, deploy_body(name, version, 1, true))
                .await;
            assert_eq!(
                status, 201,
                "case deploying version {version} in {region}: expected HTTP 201, got {status} \
                 {answered:?}"
            );
        }
    }

    for (region, deployments) in rows {
        let case = format!("the history in {region}");
        let want: Vec<Expected> = (1..=deployments)
            .rev()
            .map(|version| Expected {
                version,
                containers: 1,
                fronted: true,
            })
            .collect();
        let history = emulator.history(&case, region, name).await;
        assert_history(&case, &history, &want);
    }

    // A region holding no service of that name holds no history of it either.
    let (status, refused) = emulator.history_raw("eu-central-1", name).await;
    assert_eq!(
        status, 400,
        "case a region holding no such service: expected HTTP 400, got {status} {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "NotFoundException",
        "case a region holding no such service: expected the NotFoundException wire code, got \
         {refused:?}"
    );
}

/// RULE: a name the request's region holds no container service under is the
/// service's `DoesNotExist` rejection — HTTP 400, the `NotFoundException` wire
/// code, the `DoesNotExist` body code and a message naming the resource type and
/// the name the request carried — which is a different answer from the empty
/// history of a service that exists and has never been deployed to.
#[tokio::test]
async fn a_name_no_service_is_held_under_is_refused() {
    let emulator = Emulator::new();
    let region = "us-east-1";
    let elsewhere = "eu-west-1";
    emulator
        .create(
            "the service that does exist",
            region,
            json!({"serviceName": "history-present", "power": "nano", "scale": 1}),
        )
        .await;
    emulator
        .create(
            "the service of another region",
            elsewhere,
            json!({"serviceName": "history-far", "power": "nano", "scale": 1}),
        )
        .await;

    // (the case, the name the request carries) — names the model admits, at the
    // ends of its own length range (1..63) too, that no service is held under.
    let cases = [
        ("a name nothing created", "history-missing".to_string()),
        ("a service held in another region", "history-far".to_string()),
        ("the shortest name the model admits", "a".to_string()),
        ("the longest name the model admits", "a".repeat(63)),
    ];
    for (case, name) in &cases {
        let (status, refused) = emulator.history_raw(region, name).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "NotFoundException",
            "case {case}: expected the NotFoundException wire code, got {refused:?}"
        );
        assert_eq!(
            refused["code"].as_str(),
            Some("DoesNotExist"),
            "case {case}: expected the DoesNotExist body code, got {refused:?}"
        );
        assert_eq!(
            refused["message"].as_str(),
            Some(format!("The ContainerService does not exist: {name}").as_str()),
            "case {case}: expected the message to name the resource type and the name the \
             request carried, got {refused:?}"
        );
    }

    // The service that DOES exist answers an empty history, and a refusal left
    // it exactly there.
    let history = emulator
        .history("the service that does exist", region, "history-present")
        .await;
    assert!(
        history.is_empty(),
        "case the service that does exist: a service never deployed to reports an empty \
         history, got {history:?}"
    );
}
