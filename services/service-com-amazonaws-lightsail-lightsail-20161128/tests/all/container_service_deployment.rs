// SPDX-License-Identifier: CC0-1.0

//! Deploying containers to an existing Lightsail container service, driven over
//! the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: `CreateContainerServiceDeployment` stores the deployment
//! its `containers` and `publicEndpoint` describe as the named service's WAITING
//! deployment, at the next version in that service's own sequence and in
//! `ACTIVATING`, puts the service in `DEPLOYING` with the progress detail that
//! says so, and the clock then settles the service in `RUNNING` with that
//! deployment `ACTIVE` — so `GetContainerServices` reports it. A request naming a
//! service the request's region does not hold is the service's `DoesNotExist`
//! rejection, and one describing nothing launchable is the modelled
//! `InvalidInputException`; neither changes a thing.

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

/// The `ContainerServiceStateDetail$code` a service launching a deployment
/// reports (`crate::state::STATE_DETAIL_CREATING_DEPLOYMENT`).
const CREATING_DEPLOYMENT: &str = "CREATING_DEPLOYMENT";

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

    /// Create the container service `body` describes, answering the record the
    /// create reports. `CreateContainerService` answers HTTP 201.
    async fn create(&self, region: &str, body: Value) -> Value {
        let (status, parsed) = self.call(region, "CreateContainerService", body).await;
        assert_eq!(
            status, 201,
            "case creating a service in {region}: expected HTTP 201, got {status} {parsed:?}"
        );
        parsed["containerService"].clone()
    }

    /// Deploy `body` to a container service, answering the status and the parsed
    /// body so a refusal can be inspected too. `CreateContainerServiceDeployment`
    /// answers HTTP 201 (its modelled success status).
    async fn deploy(&self, region: &str, body: Value) -> (u16, Value) {
        self.call(region, "CreateContainerServiceDeployment", body)
            .await
    }

    /// Deploy `body` and require the modelled 201, answering the
    /// `containerService` the deployment reports.
    async fn deploy_ok(&self, case: &str, region: &str, body: Value) -> Value {
        let (status, parsed) = self.deploy(region, body).await;
        assert_eq!(
            status, 201,
            "case {case}: expected HTTP 201 from the deployment, got {status} {parsed:?}"
        );
        parsed["containerService"].clone()
    }

    /// The stored container service `name` in `region`, as `GetContainerServices`
    /// reports it.
    async fn read(&self, region: &str, name: &str) -> Value {
        let (status, parsed) = self
            .call(region, "GetContainerServices", json!({"serviceName": name}))
            .await;
        assert_eq!(
            status, 200,
            "case reading {name} in {region}: expected HTTP 200, got {status} {parsed:?}"
        );
        parsed["containerServices"][0].clone()
    }
}

/// Poll `GetContainerServices` until the service `name` reports `want`, answering
/// the whole record. The transition is owed by the clock, not by the poll.
async fn wait_for_state(emulator: &Emulator, region: &str, name: &str, want: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let service = emulator.read(region, name).await;
        if service["state"].as_str() == Some(want) {
            return service;
        }
        assert!(
            Instant::now() < deadline,
            "case waiting for '{want}' on {name}: expected it within 5s, got {:?}",
            service["state"]
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

// ---------------------------------------------------------------------------
// The deployment table
// ---------------------------------------------------------------------------

/// One container of a requested deployment.
#[derive(Debug, Clone, Copy)]
struct ContainerRow {
    name: &'static str,
    image: &'static str,
    /// The container's open firewall ports, as `(port, protocol)` pairs.
    ports: &'static [(&'static str, &'static str)],
    command: &'static [&'static str],
    environment: &'static [(&'static str, &'static str)],
}

/// One `CreateContainerServiceDeployment` request: the containers it launches,
/// the public endpoint that fronts them as `(containerName, port)`, and the
/// health-check members it asks for (everything else is left to its documented
/// default).
#[derive(Debug, Clone, Copy)]
struct DeploymentRow {
    containers: &'static [ContainerRow],
    public_endpoint: Option<(&'static str, i64)>,
    /// `(member, value)` pairs sent as the endpoint's `healthCheck`.
    health_check: &'static [(&'static str, i64)],
}

/// One service, and the deployments created on it IN ORDER. The rows differ along
/// everything the rule generalises over: the region the calls are made in, whether
/// the service was created carrying a deployment already (and so which version
/// the first explicit deployment takes), how many containers a deployment
/// describes, whether it is fronted by a public endpoint, which health-check
/// members it asks for, and how many deployments are created in a row.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    region: &'static str,
    name: &'static str,
    /// Whether the `CreateContainerService` call carries a deployment of its own.
    created_with_deployment: bool,
    deployments: &'static [DeploymentRow],
}

/// The table. The health-check defaults asserted below are the ones each
/// `ContainerServiceHealthCheckConfig` member documents, as recorded in
/// `src/state.rs`.
const ROWS: &[Row] = &[
    Row {
        case: "one fronted container on a service created empty",
        region: "us-east-1",
        name: "deploy-empty",
        created_with_deployment: false,
        deployments: &[DeploymentRow {
            containers: &[ContainerRow {
                name: "web",
                image: "nginx:latest",
                ports: &[("80", "HTTP")],
                command: &[],
                environment: &[],
            }],
            public_endpoint: Some(("web", 80)),
            health_check: &[],
        }],
    },
    Row {
        case: "a second deployment on a service created with one",
        region: "us-west-2",
        name: "deploy-over-existing",
        created_with_deployment: true,
        deployments: &[DeploymentRow {
            containers: &[
                ContainerRow {
                    name: "api",
                    image: "my-registry/api:2",
                    ports: &[("8080", "HTTP"), ("9000", "TCP")],
                    command: &["/bin/api", "--serve"],
                    environment: &[("STAGE", "prod"), ("LOG", "info")],
                },
                ContainerRow {
                    name: "sidecar",
                    image: "my-registry/sidecar:1",
                    ports: &[],
                    command: &[],
                    environment: &[("TARGET", "api")],
                },
            ],
            public_endpoint: Some(("api", 8080)),
            health_check: &[("timeoutSeconds", 30), ("intervalSeconds", 60)],
        }],
    },
    Row {
        case: "three deployments in a row, the last one fronting nothing",
        region: "eu-west-1",
        name: "deploy-thrice",
        created_with_deployment: false,
        deployments: &[
            DeploymentRow {
                containers: &[ContainerRow {
                    name: "web",
                    image: "nginx:1.25",
                    ports: &[("80", "HTTP")],
                    command: &[],
                    environment: &[],
                }],
                public_endpoint: Some(("web", 80)),
                health_check: &[("healthyThreshold", 5), ("unhealthyThreshold", 4)],
            },
            DeploymentRow {
                containers: &[
                    ContainerRow {
                        name: "web",
                        image: "nginx:1.27",
                        ports: &[("80", "HTTP"), ("443", "HTTPS")],
                        command: &[],
                        environment: &[],
                    },
                    ContainerRow {
                        name: "cache",
                        image: "redis:7",
                        ports: &[("6379", "TCP")],
                        command: &["redis-server"],
                        environment: &[],
                    },
                    ContainerRow {
                        name: "worker",
                        image: "my-registry/worker:5",
                        ports: &[],
                        command: &["/bin/worker"],
                        environment: &[("QUEUE", "jobs")],
                    },
                ],
                public_endpoint: Some(("web", 443)),
                health_check: &[("timeoutSeconds", 2), ("intervalSeconds", 300)],
            },
            DeploymentRow {
                containers: &[ContainerRow {
                    name: "batch",
                    image: "my-registry/batch:9",
                    ports: &[],
                    command: &["/bin/batch", "--once"],
                    environment: &[],
                }],
                public_endpoint: None,
                health_check: &[],
            },
        ],
    },
];

/// The `CreateContainerService` body for `row`.
fn create_body(row: &Row) -> Value {
    let mut body = json!({"serviceName": row.name, "power": "nano", "scale": 1});
    if row.created_with_deployment {
        body["deployment"] = json!({
            "containers": {"seed": {"image": "nginx:stable", "ports": {"80": "HTTP"}}},
            "publicEndpoint": {"containerName": "seed", "containerPort": 80},
        });
    }
    body
}

/// The `CreateContainerServiceDeployment` body for `deployment` on `name`.
fn deploy_body(name: &str, deployment: &DeploymentRow) -> Value {
    let mut containers = Map::new();
    for container in deployment.containers {
        let mut entry = json!({"image": container.image});
        if !container.ports.is_empty() {
            let mut ports = Map::new();
            for (port, protocol) in container.ports {
                ports.insert((*port).to_string(), json!(protocol));
            }
            entry["ports"] = Value::Object(ports);
        }
        if !container.command.is_empty() {
            entry["command"] = json!(container.command);
        }
        if !container.environment.is_empty() {
            let mut environment = Map::new();
            for (key, value) in container.environment {
                environment.insert((*key).to_string(), json!(value));
            }
            entry["environment"] = Value::Object(environment);
        }
        containers.insert(container.name.to_string(), entry);
    }
    let mut body = json!({"serviceName": name, "containers": Value::Object(containers)});
    if let Some((container_name, container_port)) = deployment.public_endpoint {
        let mut endpoint = json!({"containerName": container_name, "containerPort": container_port});
        if !deployment.health_check.is_empty() {
            let mut asked = Map::new();
            for (member, value) in deployment.health_check {
                asked.insert((*member).to_string(), json!(value));
            }
            endpoint["healthCheck"] = Value::Object(asked);
        }
        body["publicEndpoint"] = endpoint;
    }
    body
}

/// Assert `reported` is the deployment `row` asked for, at `want_version` and in
/// `want_state`.
fn assert_deployment(
    case: &str,
    row: &DeploymentRow,
    reported: &Value,
    want_version: i64,
    want_state: &str,
) {
    assert_eq!(
        reported["version"].as_i64(),
        Some(want_version),
        "case {case}: expected deployment version {want_version}, got {reported:?}"
    );
    assert_eq!(
        reported["state"].as_str(),
        Some(want_state),
        "case {case}: expected deployment state {want_state}, got {reported:?}"
    );
    assert!(
        reported["createdAt"].as_f64().is_some(),
        "case {case}: expected a creation timestamp on the deployment, got {reported:?}"
    );
    assert_eq!(
        reported["containers"].as_object().map(Map::len),
        Some(row.containers.len()),
        "case {case}: expected exactly the requested containers, got {:?}",
        reported["containers"]
    );
    for container in row.containers {
        let got = &reported["containers"][container.name];
        assert_eq!(
            got["image"].as_str(),
            Some(container.image),
            "case {case}: expected container '{}' to keep its image, got {got:?}",
            container.name
        );
        for (port, protocol) in container.ports {
            assert_eq!(
                got["ports"][*port].as_str(),
                Some(*protocol),
                "case {case}: expected port {port} of '{}' open as {protocol}, got {got:?}",
                container.name
            );
        }
        if !container.command.is_empty() {
            let want: Vec<Value> = container.command.iter().map(|part| json!(part)).collect();
            assert_eq!(
                got["command"].as_array(),
                Some(&want),
                "case {case}: expected the launch command of '{}' kept verbatim, got {got:?}",
                container.name
            );
        }
        for (key, value) in container.environment {
            assert_eq!(
                got["environment"][*key].as_str(),
                Some(*value),
                "case {case}: expected environment {key} of '{}', got {got:?}",
                container.name
            );
        }
    }

    match row.public_endpoint {
        None => assert!(
            reported["publicEndpoint"].is_null(),
            "case {case}: a deployment fronting nothing reports no public endpoint, got {:?}",
            reported["publicEndpoint"]
        ),
        Some((container_name, container_port)) => {
            let endpoint = &reported["publicEndpoint"];
            assert_eq!(
                endpoint["containerName"].as_str(),
                Some(container_name),
                "case {case}: expected the requested endpoint container, got {endpoint:?}"
            );
            assert_eq!(
                endpoint["containerPort"].as_i64(),
                Some(container_port),
                "case {case}: expected the requested endpoint port, got {endpoint:?}"
            );
            // Every health-check member is reported: the ones the request asked
            // for verbatim, the rest at the default that member documents.
            let health = &endpoint["healthCheck"];
            let asked = |member: &str| {
                row.health_check
                    .iter()
                    .find(|(name, _)| *name == member)
                    .map(|(_, value)| *value)
            };
            for (member, default) in [
                ("healthyThreshold", 2),
                ("unhealthyThreshold", 2),
                ("timeoutSeconds", 2),
                ("intervalSeconds", 5),
            ] {
                assert_eq!(
                    health[member].as_i64(),
                    Some(asked(member).unwrap_or(default)),
                    "case {case}: expected the asked-for or default {member}, got {health:?}"
                );
            }
            assert_eq!(
                health["path"].as_str(),
                Some("/"),
                "case {case}: expected the documented default health-check path, got {health:?}"
            );
            assert_eq!(
                health["successCodes"].as_str(),
                Some("200-499"),
                "case {case}: expected the documented default successCodes, got {health:?}"
            );
        }
    }
}

/// RULE: a created deployment is the service's WAITING deployment at the next
/// version of that service's own sequence, kept exactly as the request described
/// it, and the clock then makes it the service's one ACTIVE deployment — which is
/// what a later read reports.
#[tokio::test]
async fn a_created_deployment_waits_then_becomes_the_active_one() {
    let emulator = Emulator::new();
    for row in ROWS {
        let created = emulator.create(row.region, create_body(row)).await;
        // A service created carrying a deployment already spent version 1 on it,
        // so the first deployment created here is version 2.
        let first_version = if row.created_with_deployment { 2 } else { 1 };
        // Nothing about the service itself is deployment business.
        let arn = created["arn"].clone();
        let created_at = created["createdAt"].clone();

        for (index, deployment) in row.deployments.iter().enumerate() {
            let case = format!("{} (deployment {index})", row.case);
            let version = first_version + index as i64;
            let answered = emulator
                .deploy_ok(&case, row.region, deploy_body(row.name, deployment))
                .await;

            // The deployment is waiting, and the service says it is launching it.
            assert_eq!(
                answered["state"].as_str(),
                Some("DEPLOYING"),
                "case {case}: launching a deployment puts the service in DEPLOYING, got \
                 {answered:?}"
            );
            assert_eq!(
                answered["stateDetail"]["code"].as_str(),
                Some(CREATING_DEPLOYMENT),
                "case {case}: expected the creating-deployment progress code, got {:?}",
                answered["stateDetail"]
            );
            assert!(
                answered["stateDetail"]["message"]
                    .as_str()
                    .is_some_and(|message| message.contains(&version.to_string())),
                "case {case}: expected the progress message to name version {version}, got {:?}",
                answered["stateDetail"]
            );
            assert_deployment(
                &case,
                deployment,
                &answered["nextDeployment"],
                version,
                "ACTIVATING",
            );
            assert_eq!(
                answered["arn"], arn,
                "case {case}: deploying must not re-identify the service, got {:?}",
                answered["arn"]
            );
            assert_eq!(
                answered["createdAt"], created_at,
                "case {case}: deploying must not re-date the service, got {:?}",
                answered["createdAt"]
            );

            // The STORE holds it: a read reports the same deployment, waiting or
            // (where the clock got there first) already active.
            let read = emulator.read(row.region, row.name).await;
            let waiting = !read["nextDeployment"].is_null();
            assert_deployment(
                &format!("{case} read back"),
                deployment,
                if waiting {
                    &read["nextDeployment"]
                } else {
                    &read["currentDeployment"]
                },
                version,
                if waiting { "ACTIVATING" } else { "ACTIVE" },
            );

            // The clock — not the read — activates it.
            let settled = wait_for_state(&emulator, row.region, row.name, "RUNNING").await;
            assert_deployment(
                &case,
                deployment,
                &settled["currentDeployment"],
                version,
                "ACTIVE",
            );
            assert!(
                settled["nextDeployment"].is_null(),
                "case {case}: the waiting deployment became the active one, got {:?}",
                settled["nextDeployment"]
            );
            assert!(
                settled["stateDetail"].is_null(),
                "case {case}: a settled service has no progress detail, got {:?}",
                settled["stateDetail"]
            );

            // Reading twice changes nothing a later caller sees.
            let again = emulator.read(row.region, row.name).await;
            assert_eq!(
                again, settled,
                "case {case}: a read must not change the record it reports"
            );
        }
    }
}

/// RULE: the version sequence belongs to ONE service — each service numbers its
/// own deployments from its own count, so services deployed to in the same region
/// never share a sequence.
#[tokio::test]
async fn each_service_numbers_its_own_deployments() {
    let emulator = Emulator::new();
    let region = "ap-northeast-1";
    let body = |name: &str| {
        json!({
            "serviceName": name,
            "containers": {"web": {"image": "nginx:latest", "ports": {"80": "HTTP"}}},
            "publicEndpoint": {"containerName": "web", "containerPort": 80},
        })
    };

    // (service name, how many deployments it already carries)
    for (name, already) in [("seq-a", 0_i64), ("seq-b", 0), ("seq-c", 1)] {
        emulator
            .create(
                region,
                create_body(&Row {
                    case: "a service of the sequence table",
                    region,
                    name,
                    created_with_deployment: already == 1,
                    deployments: &[],
                }),
            )
            .await;
    }

    // Deploy to each service a different number of times, interleaved, so a
    // shared counter would show up as a version one service never reached.
    let mut expected: Vec<(&str, i64)> = vec![("seq-a", 1), ("seq-b", 1), ("seq-c", 2)];
    for round in 0..3 {
        for (name, next) in expected.iter_mut() {
            if round > 0 && *name == "seq-b" {
                continue;
            }
            let case = format!("{name} round {round}");
            let answered = emulator.deploy_ok(&case, region, body(name)).await;
            assert_eq!(
                answered["nextDeployment"]["version"].as_i64(),
                Some(*next),
                "case {case}: expected version {next} from this service's own sequence, got {:?}",
                answered["nextDeployment"]
            );
            *next += 1;
        }
    }

    for (name, next) in expected {
        let settled = wait_for_state(&emulator, region, name, "RUNNING").await;
        assert_eq!(
            settled["currentDeployment"]["version"].as_i64(),
            Some(next - 1),
            "case {name}: expected the last deployment active, got {settled:?}"
        );
    }
}

/// RULE: a deployment is created FOR a stored service, so a name that names none
/// in the region the request is made in is the service's `DoesNotExist` rejection
/// — HTTP 400, wire error `NotFoundException`, body `code` `DoesNotExist` — and
/// nothing is stored for it.
#[tokio::test]
async fn deploying_to_a_service_that_is_not_there_is_refused() {
    let emulator = Emulator::new();
    let body = |name: &str| {
        json!({
            "serviceName": name,
            "containers": {"web": {"image": "nginx:latest"}},
        })
    };
    // One service exists, in one region only: the rows below are each a way of
    // naming something else.
    emulator
        .create(
            "us-east-1",
            json!({"serviceName": "present", "power": "nano", "scale": 1}),
        )
        .await;

    // (case, region, service name)
    let rows: [(&str, &str, &str); 3] = [
        ("a name no service carries", "us-east-1", "absent"),
        ("the right name in another region", "us-west-2", "present"),
        ("the shortest name the model admits", "us-east-1", "ab"),
    ];
    for (case, region, name) in rows {
        let (status, parsed) = emulator.deploy(region, body(name)).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} {parsed:?}"
        );
        assert_eq!(
            error_code(&parsed),
            "NotFoundException",
            "case {case}: expected the modelled NotFoundException, got {parsed:?}"
        );
        assert_eq!(
            parsed["code"].as_str(),
            Some("DoesNotExist"),
            "case {case}: expected Lightsail's DoesNotExist code, got {parsed:?}"
        );
    }

    // The service that does exist is untouched: a refused deployment stores
    // nothing anywhere.
    let present = emulator.read("us-east-1", "present").await;
    assert!(
        present["currentDeployment"].is_null() && present["nextDeployment"].is_null(),
        "case the service that exists: a refused deployment must store nothing, got {present:?}"
    );
}

/// RULE: a deployment this service cannot launch is refused with the modelled
/// `InvalidInputException` (400), naming the member that is wrong, and the
/// service's deployments are left exactly as they were.
#[tokio::test]
async fn an_unlaunchable_deployment_is_refused_and_changes_nothing() {
    let emulator = Emulator::new();
    let region = "us-east-2";
    let name = "refusing-service";
    emulator
        .create(
            region,
            json!({"serviceName": name, "power": "micro", "scale": 1}),
        )
        .await;
    // One good deployment first, so the refusals below have something they could
    // have damaged.
    let good = DeploymentRow {
        containers: &[ContainerRow {
            name: "web",
            image: "nginx:latest",
            ports: &[("80", "HTTP")],
            command: &[],
            environment: &[],
        }],
        public_endpoint: Some(("web", 80)),
        health_check: &[],
    };
    emulator
        .deploy_ok("the good deployment", region, deploy_body(name, &good))
        .await;
    let before = wait_for_state(&emulator, region, name, "RUNNING").await;

    // Each row is one way the deployment is unlaunchable: nothing to launch, a
    // container with no image, an endpoint on a container the deployment does not
    // describe, an endpoint on a port that container does not open (and on one
    // that opens none), and a health check one past each published bound of its
    // two documented ranges (2..60 seconds and 5..300 seconds).
    let rows: &[(&str, Value)] = &[
        (
            "no containers at all",
            json!({"serviceName": name, "containers": {}}),
        ),
        (
            "no containers member at all",
            json!({"serviceName": name}),
        ),
        (
            "a container with no image to run",
            json!({"serviceName": name, "containers": {"web": {"ports": {"80": "HTTP"}}}}),
        ),
        (
            "an endpoint on a container the deployment does not describe",
            json!({
                "serviceName": name,
                "containers": {"web": {"image": "nginx"}},
                "publicEndpoint": {"containerName": "api", "containerPort": 80},
            }),
        ),
        (
            "an endpoint on a port the container does not open",
            json!({
                "serviceName": name,
                "containers": {"web": {"image": "nginx", "ports": {"80": "HTTP"}}},
                "publicEndpoint": {"containerName": "web", "containerPort": 8080},
            }),
        ),
        (
            "an endpoint on a container that opens no port at all",
            json!({
                "serviceName": name,
                "containers": {"web": {"image": "nginx"}},
                "publicEndpoint": {"containerName": "web", "containerPort": 80},
            }),
        ),
        (
            "a health-check timeout one second under the documented minimum",
            health_check_body(name, json!({"timeoutSeconds": 1})),
        ),
        (
            "a health-check timeout one second over the documented maximum",
            health_check_body(name, json!({"timeoutSeconds": 61})),
        ),
        (
            "a health-check interval one second under the documented minimum",
            health_check_body(name, json!({"intervalSeconds": 4})),
        ),
        (
            "a health-check interval one second over the documented maximum",
            health_check_body(name, json!({"intervalSeconds": 301})),
        ),
    ];
    for (case, body) in rows {
        let (status, parsed) = emulator.deploy(region, body.clone()).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} {parsed:?}"
        );
        assert_eq!(
            error_code(&parsed),
            "InvalidInputException",
            "case {case}: expected the modelled InvalidInputException, got {parsed:?}"
        );
        assert!(
            parsed["message"]
                .as_str()
                .is_some_and(|message| message.contains("containers")
                    || message.contains("publicEndpoint")),
            "case {case}: expected the refusal to name the member that is wrong, got {parsed:?}"
        );

        let after = emulator.read(region, name).await;
        assert_eq!(
            after, before,
            "case {case}: a refused deployment must leave the service as it was"
        );
    }
}

/// RULE: the span a deployment takes to become active is never zero, so a caller
/// reading straight after the call still sees the service launching it — the
/// deployment that was running keeps serving until then.
#[tokio::test]
async fn the_clock_owes_the_deployment_its_activation() {
    let emulator = Emulator::new();
    let region = "ca-central-1";
    let name = "transient-service";
    emulator
        .create(
            region,
            json!({
                "serviceName": name,
                "power": "nano",
                "scale": 1,
                "deployment": {
                    "containers": {"seed": {"image": "nginx:stable", "ports": {"80": "HTTP"}}},
                },
            }),
        )
        .await;
    let running = wait_for_state(&emulator, region, name, "RUNNING").await;
    assert_eq!(
        running["currentDeployment"]["version"].as_i64(),
        Some(1),
        "case the seeded service: expected its first deployment active, got {running:?}"
    );

    emulator
        .deploy_ok(
            "the replacement",
            region,
            json!({
                "serviceName": name,
                "containers": {"web": {"image": "nginx:1.27", "ports": {"80": "HTTP"}}},
            }),
        )
        .await;
    let straight_after = emulator.read(region, name).await;
    assert_eq!(
        straight_after["state"].as_str(),
        Some("DEPLOYING"),
        "case straight after the deployment: expected DEPLOYING, got {straight_after:?}"
    );
    assert_eq!(
        straight_after["currentDeployment"]["version"].as_i64(),
        Some(1),
        "case straight after the deployment: the running deployment keeps serving, got {:?}",
        straight_after["currentDeployment"]
    );
    assert_eq!(
        straight_after["nextDeployment"]["state"].as_str(),
        Some("ACTIVATING"),
        "case straight after the deployment: expected the new one activating, got {:?}",
        straight_after["nextDeployment"]
    );

    let settled = wait_for_state(&emulator, region, name, "RUNNING").await;
    assert_eq!(
        settled["currentDeployment"]["version"].as_i64(),
        Some(2),
        "case once the clock settled it: expected the new deployment active, got {settled:?}"
    );
    assert_eq!(
        settled["currentDeployment"]["containers"]["web"]["image"].as_str(),
        Some("nginx:1.27"),
        "case once the clock settled it: expected the new deployment's container, got {settled:?}"
    );
}

/// RULE: only one deployment can be WAITING, as only one can be ACTIVE, so a
/// deployment created while another is still waiting SUPERSEDES it: the newest
/// one is the service's `nextDeployment` and the one the clock makes active,
/// while whatever was already running keeps serving until then.
#[tokio::test]
async fn a_deployment_created_while_one_waits_supersedes_it() {
    let emulator = Emulator::new();
    let region = "eu-central-1";
    let name = "superseding-service";
    // (case, the image the deployment launches)
    let rows: [(&str, &str); 3] = [
        ("the first waiting deployment", "nginx:1.25"),
        ("the one that supersedes it", "nginx:1.27"),
        ("the one that supersedes that", "nginx:1.29"),
    ];
    emulator
        .create(
            region,
            json!({
                "serviceName": name,
                "power": "nano",
                "scale": 1,
                "deployment": {
                    "containers": {"seed": {"image": "nginx:stable", "ports": {"80": "HTTP"}}},
                },
            }),
        )
        .await;
    let running = wait_for_state(&emulator, region, name, "RUNNING").await;
    assert_eq!(
        running["currentDeployment"]["version"].as_i64(),
        Some(1),
        "case the seeded service: expected its first deployment active, got {running:?}"
    );

    // The deployments are created back to back, each one taking the next version
    // of the service's own sequence and leaving exactly one waiting.
    for (index, (case, image)) in rows.iter().enumerate() {
        let version = 2 + index as i64;
        let answered = emulator
            .deploy_ok(
                case,
                region,
                json!({
                    "serviceName": name,
                    "containers": {"web": {"image": image, "ports": {"80": "HTTP"}}},
                }),
            )
            .await;
        assert_eq!(
            answered["nextDeployment"]["version"].as_i64(),
            Some(version),
            "case {case}: expected the newest deployment waiting at version {version}, got {:?}",
            answered["nextDeployment"]
        );
        assert_eq!(
            answered["nextDeployment"]["containers"]["web"]["image"].as_str(),
            Some(*image),
            "case {case}: expected the waiting deployment to be this request's, got {:?}",
            answered["nextDeployment"]
        );
        assert_eq!(
            answered["state"].as_str(),
            Some("DEPLOYING"),
            "case {case}: expected the service launching it, got {answered:?}"
        );
        // Superseding does not retire the deployment that is RUNNING.
        assert!(
            answered["currentDeployment"]["version"]
                .as_i64()
                .is_some_and(|active| active < version),
            "case {case}: expected the running deployment to keep serving, got {:?}",
            answered["currentDeployment"]
        );
    }

    // Exactly the LAST one is the deployment the clock activates; the ones it
    // superseded never become the active deployment.
    let last_version = 1 + rows.len() as i64;
    let settled = wait_for_state(&emulator, region, name, "RUNNING").await;
    assert_eq!(
        settled["currentDeployment"]["version"].as_i64(),
        Some(last_version),
        "case once the clock settled it: expected version {last_version} active, got {settled:?}"
    );
    assert_eq!(
        settled["currentDeployment"]["containers"]["web"]["image"].as_str(),
        Some(rows[rows.len() - 1].1),
        "case once the clock settled it: expected the last deployment's container, got {settled:?}"
    );
    assert!(
        settled["nextDeployment"].is_null(),
        "case once the clock settled it: nothing is waiting any more, got {:?}",
        settled["nextDeployment"]
    );
}

/// A deployment of one container fronted by a public endpoint whose health check
/// asks for exactly `asked`.
fn health_check_body(name: &str, asked: Value) -> Value {
    json!({
        "serviceName": name,
        "containers": {"web": {"image": "nginx:latest", "ports": {"80": "HTTP"}}},
        "publicEndpoint": {
            "containerName": "web",
            "containerPort": 80,
            "healthCheck": asked,
        },
    })
}
