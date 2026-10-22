// SPDX-License-Identifier: CC0-1.0

//! The Lightsail container service control plane — create and read back — driven
//! over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: `CreateContainerService` stores ONE container service
//! record per (region, name) — the caller's power, scale, tags, public domain
//! names, private registry access and deployment kept verbatim, every
//! server-assigned member (ARN, creation time, power id, default domain,
//! principal ARN, lifecycle) computed from the request plus the store's own
//! allocators — and `GetContainerServices` reports exactly that record, in the
//! region the request is made in, with a name that names nothing answered as the
//! service's `DoesNotExist` rejection.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix this service routes on (`OPERATIONS.md`, "Wire
/// protocol").
const TARGET_PREFIX: &str = "Lightsail_20161128";

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN
/// carries.
const ACCOUNT: &str = "000000000000";

/// The region a request that carries no SigV4 credential scope is served in
/// (`crate::state::DEFAULT_REGION`).
const DEFAULT_REGION: &str = "us-east-1";

/// The suffix of every container service's default domain, from the modelled
/// `serviceName` documentation.
const DOMAIN_SUFFIX: &str = "cs.amazonlightsail.com";

/// How many characters the per-service token of that domain carries, from the
/// same documentation's example (`ur4EXAMPLE2uq`).
const DOMAIN_TOKEN_LEN: usize = 13;

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

    /// Call `operation` with `body`, in `region` when one is given, answering its
    /// HTTP status and parsed body.
    async fn call(&self, region: Option<&str>, operation: &str, body: Value) -> (u16, Value) {
        let mut builder = ::http::Request::builder()
            .method("POST")
            .uri("/")
            .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
            .header("content-type", "application/x-amz-json-1.1");
        if let Some(region) = region {
            builder = builder.header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261022/{region}/lightsail/\
                     aws4_request, SignedHeaders=host, Signature=not-verified"
                ),
            );
        }
        let request = builder
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

    /// Call `operation` in `region` and require `want_status`, answering the body.
    async fn ok(
        &self,
        region: Option<&str>,
        operation: &str,
        body: Value,
        want_status: u16,
    ) -> Value {
        let (status, parsed) = self.call(region, operation, body).await;
        assert_eq!(
            status, want_status,
            "case {operation} in {region:?}: expected HTTP {want_status}, got {status} {parsed:?}"
        );
        parsed
    }

    /// Create the container service `body` describes, answering the
    /// `containerService` the create reports. `CreateContainerService` answers
    /// HTTP 201 (its modelled success status).
    async fn create(&self, region: Option<&str>, body: Value) -> Value {
        self.ok(region, "CreateContainerService", body, 201).await["containerService"].clone()
    }

    /// The stored container service `name` in `region`, as
    /// `GetContainerServices` reports it.
    async fn read(&self, region: Option<&str>, name: &str) -> Value {
        let body = self
            .ok(
                region,
                "GetContainerServices",
                json!({"serviceName": name}),
                200,
            )
            .await;
        let services = body["containerServices"]
            .as_array()
            .unwrap_or_else(|| panic!("case reading {name}: expected an array, got {body:?}"))
            .clone();
        assert_eq!(
            services.len(),
            1,
            "case reading {name}: expected exactly the one named service, got {services:?}"
        );
        services[0].clone()
    }

    /// Every container service name `GetContainerServices` reports in `region`.
    async fn list(&self, region: Option<&str>) -> Vec<String> {
        let body = self
            .ok(region, "GetContainerServices", json!({}), 200)
            .await;
        body["containerServices"]
            .as_array()
            .unwrap_or_else(|| panic!("case listing in {region:?}: expected an array, got {body:?}"))
            .iter()
            .map(|service| {
                service["containerServiceName"]
                    .as_str()
                    .expect("every listed container service carries a name")
                    .to_string()
            })
            .collect()
    }
}

/// The wire error code of a failure body: awsJson carries it in `__type`, either
/// bare or namespaced.
fn error_code(body: &Value) -> String {
    body["__type"]
        .as_str()
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// Whether `value` has the shape of the UUID Lightsail's resource ids take: five
/// hex groups of 8-4-4-4-12.
fn is_uuid_shaped(value: &str) -> bool {
    let lengths: Vec<usize> = value.split('-').map(str::len).collect();
    lengths == vec![8, 4, 4, 4, 12]
        && value.chars().all(|c| c == '-' || c.is_ascii_hexdigit())
}

/// Poll `GetContainerServices` until the service `name` reports `want`,
/// answering the whole record.
///
/// The transition is owed by the clock, not by the poll: it arrives without
/// anyone asking more than once.
async fn wait_for_state(emulator: &Emulator, region: &str, name: &str, want: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let service = emulator.read(Some(region), name).await;
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
// The create/read table
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

/// What a request asks of a public endpoint's health check. Every member left
/// `None` must come back as that member's own documented default.
#[derive(Debug, Clone, Copy, Default)]
struct HealthCheckRow {
    healthy_threshold: Option<i64>,
    unhealthy_threshold: Option<i64>,
    timeout_seconds: Option<i64>,
    interval_seconds: Option<i64>,
    path: Option<&'static str>,
    success_codes: Option<&'static str>,
}

/// One `CreateContainerService` request, and what the rule says the stored
/// service then reports. The rows differ along everything the rule generalises
/// over: the region the request is made in (and so the ARN, the location and the
/// URL), the power (and so the power id), the scale, the number of tags, the
/// number of public domain names, whether private registry access is asked for,
/// and whether the create carries a deployment (and so which lifecycle the clock
/// owes the service, and whether a public endpoint and its health check exist).
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    region: &'static str,
    name: &'static str,
    power: &'static str,
    power_id: &'static str,
    scale: i64,
    tags: &'static [(&'static str, &'static str)],
    public_domain_names: &'static [(&'static str, &'static [&'static str])],
    /// The containers of the deployment the create carries; empty for a create
    /// that carries no deployment at all.
    containers: &'static [ContainerRow],
    /// The public endpoint of that deployment, as `(containerName, port)`.
    public_endpoint: Option<(&'static str, i64)>,
    health_check: Option<HealthCheckRow>,
    /// What the request sends as `privateRegistryAccess.ecrImagePullerRole.isActive`,
    /// or `None` to send no `privateRegistryAccess` at all.
    ecr_image_puller_active: Option<bool>,
    /// The state the clock settles this service into: `READY` without a
    /// deployment, `RUNNING` with one.
    settled_state: &'static str,
}

/// The table. Power ids are `crate::state::CONTAINER_SERVICE_POWERS`' published
/// plans, recorded in `src/state.rs`; the health-check defaults are the ones
/// each `ContainerServiceHealthCheckConfig` member documents.
const ROWS: &[Row] = &[
    Row {
        case: "smallest power, scale at the modelled lower bound, nothing optional",
        region: "us-east-1",
        name: "plain-service",
        power: "nano",
        power_id: "nano-1",
        scale: 1,
        tags: &[],
        public_domain_names: &[],
        containers: &[],
        public_endpoint: None,
        health_check: None,
        ecr_image_puller_active: None,
        settled_state: "READY",
    },
    Row {
        case: "one tag, two domains under one certificate, ECR puller role active",
        region: "us-west-2",
        name: "tagged-service",
        power: "micro",
        power_id: "micro-1",
        scale: 3,
        tags: &[("env", "test")],
        public_domain_names: &[("site-cert", &["example.com", "www.example.com"])],
        containers: &[],
        public_endpoint: None,
        health_check: None,
        ecr_image_puller_active: Some(true),
        settled_state: "READY",
    },
    Row {
        case: "scale at the modelled upper bound, one container fronted by a fully \
               specified health check",
        region: "eu-west-1",
        name: "wide-service",
        power: "medium",
        power_id: "medium-1",
        scale: 20,
        tags: &[("owner", "platform"), ("tier", "gold")],
        public_domain_names: &[],
        containers: &[ContainerRow {
            name: "web",
            image: "nginx:latest",
            ports: &[("80", "HTTP")],
            command: &[],
            environment: &[],
        }],
        public_endpoint: Some(("web", 80)),
        health_check: Some(HealthCheckRow {
            healthy_threshold: Some(3),
            unhealthy_threshold: Some(4),
            timeout_seconds: Some(30),
            interval_seconds: Some(60),
            path: Some("/healthz"),
            success_codes: Some("200-299"),
        }),
        ecr_image_puller_active: Some(false),
        settled_state: "RUNNING",
    },
    Row {
        case: "two containers, the endpoint on the second, health check left to its \
               documented defaults",
        region: "ap-northeast-1",
        name: "two-container-service",
        power: "xlarge",
        power_id: "xlarge-1",
        scale: 7,
        tags: &[],
        public_domain_names: &[
            ("api-cert", &["api.example.com"]),
            ("app-cert", &["app.example.com"]),
        ],
        containers: &[
            ContainerRow {
                name: "worker",
                image: ":two-container-service.worker.latest",
                ports: &[],
                command: &["/bin/sh", "-c", "run"],
                environment: &[("QUEUE", "main")],
            },
            ContainerRow {
                name: "api",
                image: "public.ecr.aws/nginx/nginx:1.25",
                ports: &[("8080", "HTTP"), ("8443", "HTTPS")],
                command: &[],
                environment: &[],
            },
        ],
        public_endpoint: Some(("api", 8443)),
        health_check: Some(HealthCheckRow {
            // Only the path is asked for; every other member must come back as
            // its documented default.
            healthy_threshold: None,
            unhealthy_threshold: None,
            timeout_seconds: None,
            interval_seconds: None,
            path: Some("/ping"),
            success_codes: None,
        }),
        ecr_image_puller_active: None,
        settled_state: "RUNNING",
    },
    Row {
        case: "a deployment with no public endpoint at all",
        region: "sa-east-1",
        name: "headless-service",
        power: "large",
        power_id: "large-1",
        scale: 2,
        tags: &[("only", "tag")],
        public_domain_names: &[],
        containers: &[ContainerRow {
            name: "batch",
            image: "amazonlinux:2023",
            ports: &[],
            command: &[],
            environment: &[],
        }],
        public_endpoint: None,
        health_check: None,
        ecr_image_puller_active: None,
        settled_state: "RUNNING",
    },
];

/// The documented default of each health-check member, from its own
/// documentation on `ContainerServiceHealthCheckConfig`: "The default value is
/// `2`" (both thresholds and the timeout), "The default value is `5`" (the
/// interval), "The default value is `/`" (the path), and the whole of the
/// documented `200`..`499` success range.
const DEFAULT_HEALTHY_THRESHOLD: i64 = 2;
const DEFAULT_UNHEALTHY_THRESHOLD: i64 = 2;
const DEFAULT_TIMEOUT_SECONDS: i64 = 2;
const DEFAULT_INTERVAL_SECONDS: i64 = 5;
const DEFAULT_PATH: &str = "/";
const DEFAULT_SUCCESS_CODES: &str = "200-499";

/// The request body `row` describes.
fn create_body(row: &Row) -> Value {
    let mut body = json!({"serviceName": row.name, "power": row.power, "scale": row.scale});
    if !row.tags.is_empty() {
        body["tags"] = Value::Array(
            row.tags
                .iter()
                .map(|(key, value)| json!({"key": key, "value": value}))
                .collect(),
        );
    }
    if !row.public_domain_names.is_empty() {
        let mut domains = Map::new();
        for (certificate, names) in row.public_domain_names {
            domains.insert((*certificate).to_string(), json!(names));
        }
        body["publicDomainNames"] = Value::Object(domains);
    }
    if !row.containers.is_empty() {
        let mut containers = Map::new();
        for container in row.containers {
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
        let mut deployment = json!({"containers": Value::Object(containers)});
        if let Some((container_name, container_port)) = row.public_endpoint {
            let mut endpoint = json!({"containerName": container_name, "containerPort": container_port});
            if let Some(health_check) = row.health_check {
                let mut asked = Map::new();
                if let Some(value) = health_check.healthy_threshold {
                    asked.insert("healthyThreshold".to_string(), json!(value));
                }
                if let Some(value) = health_check.unhealthy_threshold {
                    asked.insert("unhealthyThreshold".to_string(), json!(value));
                }
                if let Some(value) = health_check.timeout_seconds {
                    asked.insert("timeoutSeconds".to_string(), json!(value));
                }
                if let Some(value) = health_check.interval_seconds {
                    asked.insert("intervalSeconds".to_string(), json!(value));
                }
                if let Some(value) = health_check.path {
                    asked.insert("path".to_string(), json!(value));
                }
                if let Some(value) = health_check.success_codes {
                    asked.insert("successCodes".to_string(), json!(value));
                }
                endpoint["healthCheck"] = Value::Object(asked);
            }
            deployment["publicEndpoint"] = endpoint;
        }
        body["deployment"] = deployment;
    }
    if let Some(active) = row.ecr_image_puller_active {
        body["privateRegistryAccess"] = json!({"ecrImagePullerRole": {"isActive": active}});
    }
    body
}

/// The per-service token of `service`'s default domain, taken out of the URL it
/// reports, after checking the URL's whole documented form.
fn domain_token(case: &str, row: &Row, service: &Value) -> String {
    let url = service["url"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected a url, got {service:?}"));
    let prefix = format!("https://{}.", row.name);
    let suffix = format!(".{}.{DOMAIN_SUFFIX}", row.region);
    let token = url
        .strip_prefix(&prefix)
        .and_then(|rest| rest.strip_suffix(&suffix))
        .unwrap_or_else(|| {
            panic!("case {case}: expected a url '{prefix}<token>{suffix}', got {url:?}")
        });
    assert_eq!(
        token.len(),
        DOMAIN_TOKEN_LEN,
        "case {case}: expected a {DOMAIN_TOKEN_LEN}-character domain token, got {token:?}"
    );
    assert!(
        token.chars().all(|c| c.is_ascii_alphanumeric()),
        "case {case}: expected an alphanumeric domain token, got {token:?}"
    );
    token.to_string()
}

/// Assert everything the rule says `service` reports for `row`, except the
/// lifecycle (which the clock moves) — so the same assertions hold on the create
/// response and on every later read.
fn assert_static_members(case: &str, row: &Row, service: &Value) -> String {
    assert_eq!(
        service["containerServiceName"].as_str(),
        Some(row.name),
        "case {case}: expected the requested name, got {service:?}"
    );
    let arn = service["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected an arn, got {service:?}"));
    let arn_prefix = format!("arn:aws:lightsail:{}:{ACCOUNT}:ContainerService/", row.region);
    let id = arn
        .strip_prefix(&arn_prefix)
        .unwrap_or_else(|| panic!("case {case}: expected an arn '{arn_prefix}<id>', got {arn:?}"));
    assert!(
        is_uuid_shaped(id),
        "case {case}: expected a uuid-shaped resource id, got {id:?}"
    );
    assert_eq!(
        service["resourceType"].as_str(),
        Some("ContainerService"),
        "case {case}: expected the ContainerService resource type, got {service:?}"
    );
    assert_eq!(
        service["location"]["regionName"].as_str(),
        Some(row.region),
        "case {case}: expected the region of the request, got {:?}",
        service["location"]
    );
    assert!(
        service["location"]["availabilityZone"].is_null(),
        "case {case}: a container service is regional, so it must report no availability zone, \
         got {:?}",
        service["location"]
    );
    assert_eq!(
        service["power"].as_str(),
        Some(row.power),
        "case {case}: expected the requested power, got {service:?}"
    );
    assert_eq!(
        service["powerId"].as_str(),
        Some(row.power_id),
        "case {case}: expected the power id of {}, got {:?}",
        row.power,
        service["powerId"]
    );
    assert_eq!(
        service["scale"].as_i64(),
        Some(row.scale),
        "case {case}: expected the requested scale, got {:?}",
        service["scale"]
    );
    assert_eq!(
        service["isDisabled"].as_bool(),
        Some(false),
        "case {case}: a service nothing disabled must report isDisabled false, got {:?}",
        service["isDisabled"]
    );
    assert!(
        service["createdAt"].as_f64().is_some_and(|at| at > 0.0),
        "case {case}: expected a creation timestamp, got {:?}",
        service["createdAt"]
    );
    assert_eq!(
        service["privateDomainName"].as_str(),
        Some(format!("{}.service.local", row.name).as_str()),
        "case {case}: expected the service's private domain name, got {:?}",
        service["privateDomainName"]
    );

    // The tags and public domain names the request carried, kept verbatim.
    let tags: Vec<(String, String)> = service["tags"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected a tags array, got {service:?}"))
        .iter()
        .map(|tag| {
            (
                tag["key"].as_str().unwrap_or_default().to_string(),
                tag["value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    let want_tags: Vec<(String, String)> = row
        .tags
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect();
    assert_eq!(
        tags, want_tags,
        "case {case}: expected the requested tags, got {tags:?}"
    );
    for (certificate, names) in row.public_domain_names {
        let reported: Vec<&str> = service["publicDomainNames"][certificate]
            .as_array()
            .unwrap_or_else(|| {
                panic!(
                    "case {case}: expected the domains of '{certificate}', got {:?}",
                    service["publicDomainNames"]
                )
            })
            .iter()
            .map(|domain| domain.as_str().unwrap_or_default())
            .collect();
        let mut want: Vec<&str> = names.to_vec();
        want.sort();
        assert_eq!(
            reported, want,
            "case {case}: expected the domains requested for '{certificate}', got {reported:?}"
        );
    }
    assert_eq!(
        service["publicDomainNames"]
            .as_object()
            .map(serde_json::Map::len),
        Some(row.public_domain_names.len()),
        "case {case}: expected exactly the requested certificates, got {:?}",
        service["publicDomainNames"]
    );

    // The default domain, the principal ARN and the ECR puller role all carry
    // the one token this service was minted with.
    let token = domain_token(case, row, service);
    assert_eq!(
        service["principalArn"].as_str(),
        Some(
            format!(
                "arn:aws:iam::{ACCOUNT}:role/amazon/lightsail/{}/containers/{}/{token}",
                row.region, row.name
            )
            .as_str()
        ),
        "case {case}: expected the service's own principal ARN, got {:?}",
        service["principalArn"]
    );
    let puller = &service["privateRegistryAccess"]["ecrImagePullerRole"];
    let active = row.ecr_image_puller_active.unwrap_or(false);
    assert_eq!(
        puller["isActive"].as_bool(),
        Some(active),
        "case {case}: expected the requested ECR puller activation, got {puller:?}"
    );
    if active {
        assert_eq!(
            puller["principalArn"].as_str(),
            Some(
                format!(
                    "arn:aws:iam::{ACCOUNT}:role/amazon/lightsail/{}/containers/{}/{token}/ecr-image-puller",
                    row.region, row.name
                )
                .as_str()
            ),
            "case {case}: expected the ECR puller role's principal, got {puller:?}"
        );
    } else {
        assert!(
            puller["principalArn"].is_null(),
            "case {case}: an inactive ECR puller role grants nothing, so it must report no \
             principal, got {puller:?}"
        );
    }
    token
}

/// Assert `deployment` is the deployment `row` asked for, in `want_state`.
fn assert_deployment(case: &str, row: &Row, deployment: &Value, want_state: &str) {
    assert_eq!(
        deployment["version"].as_i64(),
        Some(1),
        "case {case}: a service's first deployment is version 1, got {deployment:?}"
    );
    assert_eq!(
        deployment["state"].as_str(),
        Some(want_state),
        "case {case}: expected deployment state {want_state}, got {deployment:?}"
    );
    for container in row.containers {
        let reported = &deployment["containers"][container.name];
        assert_eq!(
            reported["image"].as_str(),
            Some(container.image),
            "case {case}: expected container '{}' to keep its image, got {reported:?}",
            container.name
        );
        for (port, protocol) in container.ports {
            assert_eq!(
                reported["ports"][port].as_str(),
                Some(*protocol),
                "case {case}: expected port {port} of '{}' open for {protocol}, got {reported:?}",
                container.name
            );
        }
        if !container.command.is_empty() {
            let command: Vec<&str> = reported["command"]
                .as_array()
                .unwrap_or_else(|| {
                    panic!(
                        "case {case}: expected the launch command of '{}', got {reported:?}",
                        container.name
                    )
                })
                .iter()
                .map(|word| word.as_str().unwrap_or_default())
                .collect();
            assert_eq!(
                command,
                container.command.to_vec(),
                "case {case}: expected the requested launch command, got {command:?}"
            );
        }
        for (key, value) in container.environment {
            assert_eq!(
                reported["environment"][key].as_str(),
                Some(*value),
                "case {case}: expected environment {key} of '{}', got {reported:?}",
                container.name
            );
        }
    }
    assert_eq!(
        deployment["containers"]
            .as_object()
            .map(serde_json::Map::len),
        Some(row.containers.len()),
        "case {case}: expected exactly the requested containers, got {:?}",
        deployment["containers"]
    );

    match row.public_endpoint {
        None => assert!(
            deployment["publicEndpoint"].is_null(),
            "case {case}: a deployment with no public endpoint must report none, got {:?}",
            deployment["publicEndpoint"]
        ),
        Some((container_name, container_port)) => {
            let endpoint = &deployment["publicEndpoint"];
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
            let asked = row.health_check.unwrap_or_default();
            let health = &endpoint["healthCheck"];
            assert_eq!(
                health["healthyThreshold"].as_i64(),
                Some(asked.healthy_threshold.unwrap_or(DEFAULT_HEALTHY_THRESHOLD)),
                "case {case}: expected the asked-for or default healthyThreshold, got {health:?}"
            );
            assert_eq!(
                health["unhealthyThreshold"].as_i64(),
                Some(
                    asked
                        .unhealthy_threshold
                        .unwrap_or(DEFAULT_UNHEALTHY_THRESHOLD)
                ),
                "case {case}: expected the asked-for or default unhealthyThreshold, got {health:?}"
            );
            assert_eq!(
                health["timeoutSeconds"].as_i64(),
                Some(asked.timeout_seconds.unwrap_or(DEFAULT_TIMEOUT_SECONDS)),
                "case {case}: expected the asked-for or default timeoutSeconds, got {health:?}"
            );
            assert_eq!(
                health["intervalSeconds"].as_i64(),
                Some(asked.interval_seconds.unwrap_or(DEFAULT_INTERVAL_SECONDS)),
                "case {case}: expected the asked-for or default intervalSeconds, got {health:?}"
            );
            assert_eq!(
                health["path"].as_str(),
                Some(asked.path.unwrap_or(DEFAULT_PATH)),
                "case {case}: expected the asked-for or default path, got {health:?}"
            );
            assert_eq!(
                health["successCodes"].as_str(),
                Some(asked.success_codes.unwrap_or(DEFAULT_SUCCESS_CODES)),
                "case {case}: expected the asked-for or default successCodes, got {health:?}"
            );
        }
    }
}

/// RULE: a created container service reads back exactly as it was created — the
/// caller's power, scale, tags, public domain names, registry access and
/// deployment verbatim, every server-assigned member computed from them plus the
/// store's allocators, and no two services sharing an ARN or a default domain.
#[tokio::test]
async fn a_container_service_reads_back_as_it_was_created() {
    let emulator = Emulator::new();
    let mut arns: BTreeSet<String> = BTreeSet::new();
    let mut tokens: BTreeSet<String> = BTreeSet::new();

    for row in ROWS {
        let case = row.case;
        let created = emulator.create(Some(row.region), create_body(row)).await;
        let token = assert_static_members(case, row, &created);

        // A new service is not serving yet: the clock owes it the rest of its
        // lifecycle, so the create answers the transient state.
        assert_eq!(
            created["state"].as_str(),
            Some("PENDING"),
            "case {case}: expected a new service to be PENDING, got {created:?}"
        );
        assert!(
            created["currentDeployment"].is_null(),
            "case {case}: nothing runs on a service still being created, got {:?}",
            created["currentDeployment"]
        );
        if row.containers.is_empty() {
            assert!(
                created["nextDeployment"].is_null(),
                "case {case}: a create with no deployment has none waiting, got {:?}",
                created["nextDeployment"]
            );
        } else {
            assert_deployment(case, row, &created["nextDeployment"], "ACTIVATING");
        }

        // The read reports the SAME record: everything the clock does not move
        // is identical.
        let read = emulator.read(Some(row.region), row.name).await;
        assert_static_members(case, row, &read);
        assert_eq!(
            read["arn"], created["arn"],
            "case {case}: expected the read to report the created ARN, got {:?}",
            read["arn"]
        );
        assert_eq!(
            read["createdAt"], created["createdAt"],
            "case {case}: expected the read to report the created timestamp, got {:?}",
            read["createdAt"]
        );

        // The clock settles the service: READY without a deployment, RUNNING
        // with one, whose deployment is then the active one.
        let settled = wait_for_state(&emulator, row.region, row.name, row.settled_state).await;
        assert_eq!(
            settled["arn"], created["arn"],
            "case {case}: the settled record is the same record, got {:?}",
            settled["arn"]
        );
        if row.containers.is_empty() {
            assert!(
                settled["currentDeployment"].is_null() && settled["nextDeployment"].is_null(),
                "case {case}: a READY service has no deployment at all, got {settled:?}"
            );
        } else {
            assert_deployment(case, row, &settled["currentDeployment"], "ACTIVE");
            assert!(
                settled["nextDeployment"].is_null(),
                "case {case}: the waiting deployment became the active one, got {:?}",
                settled["nextDeployment"]
            );
        }
        assert!(
            settled["stateDetail"].is_null(),
            "case {case}: a settled service has no state detail to report, got {:?}",
            settled["stateDetail"]
        );

        assert!(
            arns.insert(
                created["arn"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            ),
            "case {case}: expected a fresh ARN, got the one of an earlier service"
        );
        assert!(
            tokens.insert(token.clone()),
            "case {case}: expected a fresh domain token, got {token:?} again"
        );
    }
}

/// RULE: a created service is answered in a non-terminal state and the clock owes
/// it the rest of its lifecycle; the span is never zero, so a caller reading
/// straight after the create still sees the transient state — and a service
/// launching a deployment reports why it is busy.
#[tokio::test]
async fn the_clock_owes_a_new_service_the_rest_of_its_lifecycle() {
    let emulator = Emulator::new();
    let row = ROWS
        .iter()
        .find(|row| !row.containers.is_empty())
        .expect("the table has a row whose create carries a deployment");
    emulator.create(Some(row.region), create_body(row)).await;

    let first = emulator.read(Some(row.region), row.name).await;
    assert_eq!(
        first["state"].as_str(),
        Some("PENDING"),
        "case straight after create: expected PENDING, got {:?}",
        first["state"]
    );

    let settled = wait_for_state(&emulator, row.region, row.name, "RUNNING").await;
    assert_eq!(
        settled["currentDeployment"]["state"].as_str(),
        Some("ACTIVE"),
        "case after the clock settled it: expected the deployment active, got {settled:?}"
    );
}

/// RULE: a container service name identifies one service WITHIN one region, so
/// the same name is refused in the region that holds it and free in another —
/// and each region's listing reports only its own services.
#[tokio::test]
async fn names_are_unique_within_one_region_only() {
    let emulator = Emulator::new();
    let name = "shared-name";
    let body = json!({"serviceName": name, "power": "nano", "scale": 1});

    emulator.create(Some("us-east-2"), body.clone()).await;

    let (status, refused) = emulator
        .call(Some("us-east-2"), "CreateContainerService", body.clone())
        .await;
    assert_eq!(
        status, 400,
        "case the same name in the same region: expected a 400, got {status} {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "InvalidInputException",
        "case the same name in the same region: expected InvalidInputException, got {refused:?}"
    );

    // The same name in another region is a different service.
    let elsewhere = emulator.create(Some("eu-central-1"), body).await;
    assert_eq!(
        elsewhere["location"]["regionName"].as_str(),
        Some("eu-central-1"),
        "case the same name in another region: expected that region, got {elsewhere:?}"
    );

    // And a second service in one region joins that region's listing only.
    emulator
        .create(
            Some("us-east-2"),
            json!({"serviceName": "other-name", "power": "small", "scale": 1}),
        )
        .await;
    assert_eq!(
        emulator.list(Some("us-east-2")).await,
        vec!["other-name".to_string(), "shared-name".to_string()],
        "case listing us-east-2: expected its own two services"
    );
    assert_eq!(
        emulator.list(Some("eu-central-1")).await,
        vec!["shared-name".to_string()],
        "case listing eu-central-1: expected only the service created there"
    );
    assert!(
        emulator.list(Some("ap-south-1")).await.is_empty(),
        "case listing a region with no services: expected nothing"
    );
}

/// RULE: a request that carries no SigV4 credential scope is served in the
/// default region, so what an unsigned call creates is what a call signed for
/// that region reads.
#[tokio::test]
async fn an_unsigned_request_is_served_in_the_default_region() {
    let emulator = Emulator::new();
    let created = emulator
        .create(None, json!({"serviceName": "unsigned", "power": "nano", "scale": 1}))
        .await;
    assert_eq!(
        created["location"]["regionName"].as_str(),
        Some(DEFAULT_REGION),
        "case an unsigned create: expected the default region, got {created:?}"
    );
    let read = emulator.read(Some(DEFAULT_REGION), "unsigned").await;
    assert_eq!(
        read["arn"], created["arn"],
        "case reading it from the default region: expected the same record, got {read:?}"
    );
}

/// RULE: a well-formed name that names no stored container service in the
/// region of the request is refused with the service's `DoesNotExist` envelope —
/// HTTP 400, `NotFoundException`, body code `DoesNotExist` and a message naming
/// the resource type and the requested name — never a 501 and never a hollow 200.
#[tokio::test]
async fn an_unknown_container_service_does_not_exist() {
    let emulator = Emulator::new();
    emulator
        .create(
            Some("us-east-1"),
            json!({"serviceName": "present", "power": "nano", "scale": 1}),
        )
        .await;

    for (case, region, name) in [
        ("a name nothing created", "us-east-1", "absent"),
        ("a name created in another region", "us-west-1", "present"),
    ] {
        let (status, body) = emulator
            .call(
                Some(region),
                "GetContainerServices",
                json!({"serviceName": name}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected a 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {case}: expected NotFoundException, got {body:?}"
        );
        assert_eq!(
            body["code"].as_str(),
            Some("DoesNotExist"),
            "case {case}: expected code DoesNotExist, got {body:?}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(format!("The ContainerService does not exist: {name}").as_str()),
            "case {case}: expected the resource-type/name message, got {body:?}"
        );
    }
}

/// RULE: every power the model admits maps to the published plan's power id,
/// which is the power's own name suffixed `-1`.
///
/// The catalog is the modelled `ContainerServicePowerName` enum
/// (`smithy_query(query="shape",shape="ContainerServicePowerName")`), walked
/// whole, so no power is only covered by the table above.
#[tokio::test]
async fn every_modelled_power_maps_to_its_published_power_id() {
    const POWERS: &[&str] = &["nano", "micro", "small", "medium", "large", "xlarge"];
    let emulator = Emulator::new();
    for power in POWERS {
        let name = format!("power-{power}");
        let created = emulator
            .create(
                Some("us-east-1"),
                json!({"serviceName": name, "power": power, "scale": 1}),
            )
            .await;
        assert_eq!(
            created["powerId"].as_str(),
            Some(format!("{power}-1").as_str()),
            "case power {power}: expected the published power id, got {created:?}"
        );
    }
}

/// RULE: a create this service cannot serve stores nothing and is refused with
/// the modelled `InvalidInputException` (400), naming the member that is wrong.
#[tokio::test]
async fn an_unservable_create_is_refused_and_stores_nothing() {
    // Each row is one way the request is unacceptable: the count of public
    // domain names, a deployment with nothing to launch, a container with no
    // image, an endpoint on a container the deployment does not describe, an
    // endpoint on a port that container does not open (and on a container that
    // opens none), and a health check outside each of its two documented ranges
    // (one past each published bound).
    let cases: &[(&str, Value)] = &[
        (
            "one public domain name more than the documented four",
            json!({
                "serviceName": "refused",
                "power": "nano",
                "scale": 1,
                "publicDomainNames": {
                    "cert": ["a.example.com", "b.example.com", "c.example.com"],
                    "other": ["d.example.com", "e.example.com"],
                },
            }),
        ),
        (
            "a deployment describing no container",
            json!({
                "serviceName": "refused",
                "power": "nano",
                "scale": 1,
                "deployment": {"containers": {}},
            }),
        ),
        (
            "a container with no image to run",
            json!({
                "serviceName": "refused",
                "power": "nano",
                "scale": 1,
                "deployment": {"containers": {"web": {"ports": {"80": "HTTP"}}}},
            }),
        ),
        (
            "an endpoint on a container the deployment does not describe",
            json!({
                "serviceName": "refused",
                "power": "nano",
                "scale": 1,
                "deployment": {
                    "containers": {"web": {"image": "nginx"}},
                    "publicEndpoint": {"containerName": "api", "containerPort": 80},
                },
            }),
        ),
        (
            "an endpoint on a port the container does not open",
            json!({
                "serviceName": "refused",
                "power": "nano",
                "scale": 1,
                "deployment": {
                    "containers": {"web": {"image": "nginx", "ports": {"80": "HTTP"}}},
                    "publicEndpoint": {"containerName": "web", "containerPort": 8080},
                },
            }),
        ),
        (
            "an endpoint on a container that opens no port at all",
            json!({
                "serviceName": "refused",
                "power": "nano",
                "scale": 1,
                "deployment": {
                    "containers": {"web": {"image": "nginx"}},
                    "publicEndpoint": {"containerName": "web", "containerPort": 80},
                },
            }),
        ),
        (
            "a health check timeout one second under the documented minimum",
            health_check_body(json!({"timeoutSeconds": 1})),
        ),
        (
            "a health check timeout one second over the documented maximum",
            health_check_body(json!({"timeoutSeconds": 61})),
        ),
        (
            "a health check interval one second under the documented minimum",
            health_check_body(json!({"intervalSeconds": 4})),
        ),
        (
            "a health check interval one second over the documented maximum",
            health_check_body(json!({"intervalSeconds": 301})),
        ),
    ];

    for (case, body) in cases {
        let emulator = Emulator::new();
        let (status, refused) = emulator
            .call(Some("us-east-1"), "CreateContainerService", body.clone())
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected a 400, got {status} {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "InvalidInputException",
            "case {case}: expected InvalidInputException, got {refused:?}"
        );
        assert!(
            emulator.list(Some("us-east-1")).await.is_empty(),
            "case {case}: a refused create must store nothing"
        );
    }
}

/// RULE: each published bound of a health check's documented ranges is accepted —
/// the refusals above are for the values just outside them.
#[tokio::test]
async fn the_published_health_check_bounds_are_accepted() {
    for (case, asked, member, want) in [
        ("the minimum timeout", json!({"timeoutSeconds": 2}), "timeoutSeconds", 2),
        ("the maximum timeout", json!({"timeoutSeconds": 60}), "timeoutSeconds", 60),
        ("the minimum interval", json!({"intervalSeconds": 5}), "intervalSeconds", 5),
        ("the maximum interval", json!({"intervalSeconds": 300}), "intervalSeconds", 300),
    ] {
        let emulator = Emulator::new();
        let created = emulator
            .create(Some("us-east-1"), health_check_body(asked))
            .await;
        assert_eq!(
            created["nextDeployment"]["publicEndpoint"]["healthCheck"][member].as_i64(),
            Some(want),
            "case {case}: expected {member} {want}, got {created:?}"
        );
    }
}

/// A create carrying one container fronted by a public endpoint whose health
/// check asks for exactly `asked`.
fn health_check_body(asked: Value) -> Value {
    json!({
        "serviceName": "health-checked",
        "power": "nano",
        "scale": 1,
        "deployment": {
            "containers": {"web": {"image": "nginx:latest", "ports": {"80": "HTTP"}}},
            "publicEndpoint": {
                "containerName": "web",
                "containerPort": 80,
                "healthCheck": asked,
            },
        },
    })
}

/// RULE: a member outside its modelled constraint — a scale outside the 1..20
/// range, a power outside the `ContainerServicePowerName` enum — is refused
/// before any handler runs, so nothing is stored for it either.
#[tokio::test]
async fn a_member_outside_its_modelled_constraint_is_refused() {
    let cases: &[(&str, Value)] = &[
        (
            "a scale below the modelled minimum",
            json!({"serviceName": "out-of-range", "power": "nano", "scale": 0}),
        ),
        (
            "a scale above the modelled maximum",
            json!({"serviceName": "out-of-range", "power": "nano", "scale": 21}),
        ),
        (
            "a power outside the modelled enum",
            json!({"serviceName": "out-of-range", "power": "gigantic", "scale": 1}),
        ),
    ];
    for (case, body) in cases {
        let emulator = Emulator::new();
        let (status, refused) = emulator
            .call(Some("us-east-1"), "CreateContainerService", body.clone())
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected a 400, got {status} {refused:?}"
        );
        assert!(
            emulator.list(Some("us-east-1")).await.is_empty(),
            "case {case}: a refused create must store nothing"
        );
    }
}
