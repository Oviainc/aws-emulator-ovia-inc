// SPDX-License-Identifier: CC0-1.0

//! Updating the configuration of a Lightsail container service, driven over the
//! wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: `UpdateContainerService` applies exactly the members the
//! request names to the stored (region, name) record and leaves every other
//! member of it alone, so `GetContainerServices` reports the updated record; a
//! change to the service's configured capacity (its power or its scale), to its
//! custom domains, or to whether it is disabled puts it in `UPDATING`, from which
//! the clock settles it into `DISABLED` when it is disabled and otherwise into
//! `RUNNING` when it has a deployment to serve or `READY` when it has none; and a
//! request that asks for nothing the service does not already carry moves it
//! nowhere at all.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix this service routes on (`OPERATIONS.md`, "Wire
/// protocol").
const TARGET_PREFIX: &str = "Lightsail_20161128";

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN
/// carries.
const ACCOUNT: &str = "000000000000";

/// The endpoint every call in this suite reaches the emulator on.
const ENDPOINT_HOST: &str = "127.0.0.1:9297";

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

    /// Update a container service, answering the status and the parsed body so a
    /// refusal can be inspected too.
    async fn update(&self, region: &str, body: Value) -> (u16, Value) {
        self.call(region, "UpdateContainerService", body).await
    }

    /// Update a container service and require the modelled 200, answering the
    /// `containerService` the update reports.
    async fn update_ok(&self, case: &str, region: &str, body: Value) -> Value {
        let (status, parsed) = self.update(region, body).await;
        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200 from the update, got {status} {parsed:?}"
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

/// The one Region the certificate API is served in (the modelled
/// `InvalidInputException` documentation: "Domain and distribution APIs are only
/// available in the N. Virginia (us-east-1) Amazon Web Services Region"), which
/// is where this suite's harness sends a call the shared stand-up makes with no
/// credential scope of its own, whichever region the container service of a row
/// lives in.
const CERTIFICATE_REGION: &str = "us-east-1";

/// One signed call to this service, which is all the shared certificate stand-up
/// needs of this suite's harness. Every call of this suite carries a credential
/// scope, so one the stand-up leaves open is made in the certificate API's own
/// Region.
impl crate::distribution_certificate_attachment::SignedCall for Emulator {
    async fn call_signed(
        &self,
        region: Option<&str>,
        operation: &str,
        body: Value,
    ) -> (u16, Value) {
        self.call(region.unwrap_or(CERTIFICATE_REGION), operation, body)
            .await
    }
}

/// Stand up every certificate the `publicDomainNames` of `body` are keyed by,
/// issued, so a request carrying them is one the account can be served.
///
/// A container service's `publicDomainNames` is keyed by the name of the SSL/TLS
/// certificate that secures the domains listed under it, and "You must create and
/// validate an SSL/TLS certificate before you can use public domain names with
/// your container service" — so an update naming one has to hold an issued
/// certificate of that name first. The stand-up and the issuance wait are the ONE
/// pair this test target holds (`distribution_certificate_attachment`), and the
/// certificate it requests covers exactly the domains the request keys by it.
async fn issue_certificates_of(emulator: &Emulator, body: &Value) {
    let Some(names) = body["publicDomainNames"].as_object() else {
        return;
    };
    for (certificate, domains) in names {
        let domains: Vec<&str> = domains
            .as_array()
            .map(|domains| {
                domains
                    .iter()
                    .map(|domain| domain.as_str().unwrap_or_default())
                    .collect()
            })
            .unwrap_or_default();
        crate::distribution_certificate_attachment::issue_certificate(
            emulator,
            certificate,
            &domains,
        )
        .await;
    }
}

// ---------------------------------------------------------------------------
// The update table
// ---------------------------------------------------------------------------

/// What the rule says a service's configuration reports after one update: the
/// members the request named at their new values, and every other member at the
/// value it already carried.
#[derive(Debug, Clone)]
struct Expected {
    power: &'static str,
    /// The published plan of `power` (`crate::state::CONTAINER_SERVICE_POWERS`).
    power_id: &'static str,
    scale: i64,
    /// The custom domains, as `(certificate, domains)` pairs.
    public_domain_names: Vec<(&'static str, Vec<&'static str>)>,
    is_disabled: bool,
    ecr_image_puller_active: bool,
}

/// One `UpdateContainerService` request applied to the service of its row, and
/// what the rule says the record then reports.
#[derive(Debug, Clone)]
struct Step {
    case: &'static str,
    /// The members the request names beyond `serviceName`.
    request: Value,
    expected: Expected,
    /// Whether this update changes the service's configured capacity, its custom
    /// domains or whether it is disabled — the three the modelled
    /// `ContainerService$state` documentation puts in `UPDATING`/`DISABLED`.
    moves_state: bool,
    /// The state the clock settles the service into after this update.
    settled_state: &'static str,
}

/// One created container service and the updates applied to it IN ORDER.
#[derive(Debug, Clone)]
struct Row {
    case: &'static str,
    region: &'static str,
    name: &'static str,
    /// The `CreateContainerService` body, beyond the service name.
    create: Value,
    /// The state the clock settles the created service into, before any update.
    created_settled_state: &'static str,
    steps: Vec<Step>,
}

/// The table. The rows differ along everything the rule generalises over: the
/// region the calls are made in, which member the update names (power, scale,
/// custom domains, private registry access, disabled-ness, and all of them at
/// once), whether the value asked for differs from the stored one, how many
/// domain names the update carries (including none at all), whether the service
/// has a deployment to come back to, and how many updates are applied in a row.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "capacity changes on a service with no deployment",
            region: "us-east-1",
            name: "capacity-service",
            create: json!({"power": "nano", "scale": 1}),
            created_settled_state: "READY",
            steps: vec![
                Step {
                    case: "the power alone, which carries its published plan with it",
                    request: json!({"power": "medium"}),
                    expected: Expected {
                        power: "medium",
                        power_id: "medium-1",
                        scale: 1,
                        public_domain_names: vec![],
                        is_disabled: false,
                        ecr_image_puller_active: false,
                    },
                    moves_state: true,
                    settled_state: "READY",
                },
                Step {
                    case: "the scale alone, at the modelled upper bound",
                    request: json!({"scale": 20}),
                    expected: Expected {
                        power: "medium",
                        power_id: "medium-1",
                        scale: 20,
                        public_domain_names: vec![],
                        is_disabled: false,
                        ecr_image_puller_active: false,
                    },
                    moves_state: true,
                    settled_state: "READY",
                },
                Step {
                    case: "the capacity the service already carries, which asks for nothing",
                    request: json!({"power": "medium", "scale": 20}),
                    expected: Expected {
                        power: "medium",
                        power_id: "medium-1",
                        scale: 20,
                        public_domain_names: vec![],
                        is_disabled: false,
                        ecr_image_puller_active: false,
                    },
                    moves_state: false,
                    settled_state: "READY",
                },
                Step {
                    case: "both at once, the scale back at the modelled lower bound",
                    request: json!({"power": "xlarge", "scale": 1}),
                    expected: Expected {
                        power: "xlarge",
                        power_id: "xlarge-1",
                        scale: 1,
                        public_domain_names: vec![],
                        is_disabled: false,
                        ecr_image_puller_active: false,
                    },
                    moves_state: true,
                    settled_state: "READY",
                },
            ],
        },
        Row {
            case: "custom domains and private registry access",
            region: "us-west-2",
            name: "domain-service",
            create: json!({
                "power": "micro",
                "scale": 2,
                "publicDomainNames": {"site-cert": ["example.com", "www.example.com"]},
                "privateRegistryAccess": {"ecrImagePullerRole": {"isActive": true}},
            }),
            created_settled_state: "READY",
            steps: vec![
                Step {
                    case: "the domains replaced by another certificate's single domain",
                    request: json!({"publicDomainNames": {"api-cert": ["api.example.com"]}}),
                    expected: Expected {
                        power: "micro",
                        power_id: "micro-1",
                        scale: 2,
                        public_domain_names: vec![("api-cert", vec!["api.example.com"])],
                        is_disabled: false,
                        ecr_image_puller_active: true,
                    },
                    moves_state: true,
                    settled_state: "READY",
                },
                Step {
                    case: "the domains emptied, which is a change like any other",
                    request: json!({"publicDomainNames": {}}),
                    expected: Expected {
                        power: "micro",
                        power_id: "micro-1",
                        scale: 2,
                        public_domain_names: vec![],
                        is_disabled: false,
                        ecr_image_puller_active: true,
                    },
                    moves_state: true,
                    settled_state: "READY",
                },
                Step {
                    case: "the image puller role deactivated, which is neither capacity nor a \
                           custom domain",
                    request: json!({"privateRegistryAccess": {"ecrImagePullerRole": {"isActive": false}}}),
                    expected: Expected {
                        power: "micro",
                        power_id: "micro-1",
                        scale: 2,
                        public_domain_names: vec![],
                        is_disabled: false,
                        ecr_image_puller_active: false,
                    },
                    moves_state: false,
                    settled_state: "READY",
                },
                Step {
                    case: "the image puller role activated again, alongside four domain names",
                    request: json!({
                        "privateRegistryAccess": {"ecrImagePullerRole": {"isActive": true}},
                        "publicDomainNames": {
                            "one-cert": ["a.example.com", "b.example.com"],
                            "two-cert": ["c.example.com", "d.example.com"],
                        },
                    }),
                    expected: Expected {
                        power: "micro",
                        power_id: "micro-1",
                        scale: 2,
                        public_domain_names: vec![
                            ("one-cert", vec!["a.example.com", "b.example.com"]),
                            ("two-cert", vec!["c.example.com", "d.example.com"]),
                        ],
                        is_disabled: false,
                        ecr_image_puller_active: true,
                    },
                    moves_state: true,
                    settled_state: "READY",
                },
            ],
        },
        Row {
            case: "disabling and re-enabling a service that serves a deployment",
            region: "eu-west-1",
            name: "served-service",
            create: json!({
                "power": "small",
                "scale": 3,
                "deployment": {
                    "containers": {"web": {"image": "nginx:latest", "ports": {"80": "HTTP"}}},
                    "publicEndpoint": {"containerName": "web", "containerPort": 80},
                },
            }),
            created_settled_state: "RUNNING",
            steps: vec![
                Step {
                    case: "disabled, so the service settles out of service",
                    request: json!({"isDisabled": true}),
                    expected: Expected {
                        power: "small",
                        power_id: "small-1",
                        scale: 3,
                        public_domain_names: vec![],
                        is_disabled: true,
                        ecr_image_puller_active: false,
                    },
                    moves_state: true,
                    settled_state: "DISABLED",
                },
                Step {
                    case: "disabled again, which asks for nothing",
                    request: json!({"isDisabled": true}),
                    expected: Expected {
                        power: "small",
                        power_id: "small-1",
                        scale: 3,
                        public_domain_names: vec![],
                        is_disabled: true,
                        ecr_image_puller_active: false,
                    },
                    moves_state: false,
                    settled_state: "DISABLED",
                },
                Step {
                    case: "the capacity of a disabled service, which stays disabled",
                    request: json!({"power": "large", "scale": 4}),
                    expected: Expected {
                        power: "large",
                        power_id: "large-1",
                        scale: 4,
                        public_domain_names: vec![],
                        is_disabled: true,
                        ecr_image_puller_active: false,
                    },
                    moves_state: true,
                    settled_state: "DISABLED",
                },
                Step {
                    case: "enabled again, so the service comes back to serving its deployment",
                    request: json!({"isDisabled": false}),
                    expected: Expected {
                        power: "large",
                        power_id: "large-1",
                        scale: 4,
                        public_domain_names: vec![],
                        is_disabled: false,
                        ecr_image_puller_active: false,
                    },
                    moves_state: true,
                    settled_state: "RUNNING",
                },
            ],
        },
        Row {
            case: "every member of one update at once, in a third region",
            region: "ap-northeast-1",
            name: "all-at-once",
            create: json!({"power": "nano", "scale": 1}),
            created_settled_state: "READY",
            steps: vec![Step {
                case: "power, scale, domains, registry access and disabled-ness together",
                request: json!({
                    "power": "large",
                    "scale": 9,
                    "isDisabled": true,
                    "publicDomainNames": {"shop-cert": ["shop.example.com"]},
                    "privateRegistryAccess": {"ecrImagePullerRole": {"isActive": true}},
                }),
                expected: Expected {
                    power: "large",
                    power_id: "large-1",
                    scale: 9,
                    public_domain_names: vec![("shop-cert", vec!["shop.example.com"])],
                    is_disabled: true,
                    ecr_image_puller_active: true,
                },
                moves_state: true,
                settled_state: "DISABLED",
            }],
        },
    ]
}

/// The `CreateContainerService` body for `row`.
fn create_body(row: &Row) -> Value {
    let mut body = row.create.clone();
    body["serviceName"] = json!(row.name);
    body
}

/// The `UpdateContainerService` body for `step` on the service `name`.
fn update_body(name: &str, step: &Step) -> Value {
    let mut body = step.request.clone();
    body["serviceName"] = json!(name);
    body
}

/// Assert `service` reports the configuration `expected` describes.
fn assert_configuration(case: &str, region: &str, name: &str, expected: &Expected, service: &Value) {
    assert_eq!(
        service["power"].as_str(),
        Some(expected.power),
        "case {case}: expected power {}, got {:?}",
        expected.power,
        service["power"]
    );
    assert_eq!(
        service["powerId"].as_str(),
        Some(expected.power_id),
        "case {case}: expected the published plan of {}, got {:?}",
        expected.power,
        service["powerId"]
    );
    assert_eq!(
        service["scale"].as_i64(),
        Some(expected.scale),
        "case {case}: expected scale {}, got {:?}",
        expected.scale,
        service["scale"]
    );
    assert_eq!(
        service["isDisabled"].as_bool(),
        Some(expected.is_disabled),
        "case {case}: expected isDisabled {}, got {:?}",
        expected.is_disabled,
        service["isDisabled"]
    );

    let reported = service["publicDomainNames"]
        .as_object()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: expected a publicDomainNames map, got {:?}",
                service["publicDomainNames"]
            )
        })
        .clone();
    assert_eq!(
        reported.len(),
        expected.public_domain_names.len(),
        "case {case}: expected exactly {} certificates, got {reported:?}",
        expected.public_domain_names.len()
    );
    for (certificate, domains) in &expected.public_domain_names {
        let got: Vec<&str> = reported
            .get(*certificate)
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case}: expected the domains of '{certificate}', got {reported:?}"))
            .iter()
            .map(|domain| domain.as_str().unwrap_or_default())
            .collect();
        let mut want = domains.clone();
        want.sort_unstable();
        assert_eq!(
            got, want,
            "case {case}: expected the domains of '{certificate}', got {got:?}"
        );
    }

    // The ECR image puller role grants an access, so it has a principal exactly
    // while it is active — the service's own role, under its own token.
    let puller = &service["privateRegistryAccess"]["ecrImagePullerRole"];
    assert_eq!(
        puller["isActive"].as_bool(),
        Some(expected.ecr_image_puller_active),
        "case {case}: expected the image puller role active {}, got {puller:?}",
        expected.ecr_image_puller_active
    );
    let principal = service["principalArn"].as_str().unwrap_or_default();
    let want_prefix = format!("arn:aws:iam::{ACCOUNT}:role/amazon/lightsail/{region}/containers/{name}/");
    assert!(
        principal.starts_with(&want_prefix),
        "case {case}: expected the service's own principal ARN under {want_prefix}, got \
         {principal:?}"
    );
    if expected.ecr_image_puller_active {
        assert_eq!(
            puller["principalArn"].as_str(),
            Some(format!("{principal}/ecr-image-puller").as_str()),
            "case {case}: expected the puller role's principal under the service's own, got \
             {puller:?}"
        );
    } else {
        assert!(
            puller["principalArn"].is_null(),
            "case {case}: an inactive puller role grants nothing, so it reports no principal, \
             got {puller:?}"
        );
    }
}

/// RULE: an update applies exactly the members it names to the stored service,
/// leaves every other member of it alone, and is what a later read reports.
#[tokio::test]
async fn an_update_applies_only_the_members_it_names() {
    let emulator = Emulator::new();
    for row in rows() {
        // Every certificate the row's requests key public domain names by, held
        // and issued before the first of them is made.
        issue_certificates_of(&emulator, &row.create).await;
        for step in &row.steps {
            issue_certificates_of(&emulator, &step.request).await;
        }
        let created = emulator.create(row.region, create_body(&row)).await;
        // The identity the update must not touch: nothing here is an update's
        // business, whatever it changes.
        let arn = created["arn"].clone();
        let created_at = created["createdAt"].clone();
        let url = created["url"].clone();
        let private_domain_name = created["privateDomainName"].clone();
        let principal_arn = created["principalArn"].clone();
        let tags = created["tags"].clone();
        wait_for_state(&emulator, row.region, row.name, row.created_settled_state).await;

        for step in &row.steps {
            let case = format!("{} / {}", row.case, step.case);
            let answered = emulator
                .update_ok(&case, row.region, update_body(row.name, step))
                .await;

            // Applying the change is the service's own work: a caller that
            // looks straight after the call sees the service still updating.
            let want_state = if step.moves_state {
                "UPDATING"
            } else {
                step.settled_state
            };
            assert_eq!(
                answered["state"].as_str(),
                Some(want_state),
                "case {case}: expected state {want_state} from the update, got {:?}",
                answered["state"]
            );
            assert_configuration(&case, row.region, row.name, &step.expected, &answered);
            for (member, before) in [
                ("arn", &arn),
                ("createdAt", &created_at),
                ("url", &url),
                ("privateDomainName", &private_domain_name),
                ("principalArn", &principal_arn),
                ("tags", &tags),
            ] {
                assert_eq!(
                    &answered[member], before,
                    "case {case}: an update must not change {member}, got {:?}",
                    answered[member]
                );
            }

            // The STORE holds the change: a read reports it, and the clock —
            // not the read — settles the service.
            assert_configuration(
                &format!("{case} read back"),
                row.region,
                row.name,
                &step.expected,
                &emulator.read(row.region, row.name).await,
            );
            let settled =
                wait_for_state(&emulator, row.region, row.name, step.settled_state).await;
            assert_configuration(
                &format!("{case} settled"),
                row.region,
                row.name,
                &step.expected,
                &settled,
            );
        }
    }
}

/// RULE: an update neither launches nor retires a deployment — the one the
/// service serves survives being disabled and is serving again once the service
/// is enabled.
#[tokio::test]
async fn an_update_keeps_the_service_deployment() {
    let emulator = Emulator::new();
    let region = "us-east-2";
    let name = "deployment-kept";
    emulator
        .create(
            region,
            json!({
                "serviceName": name,
                "power": "nano",
                "scale": 1,
                "deployment": {
                    "containers": {"web": {"image": "nginx:1.27", "ports": {"80": "HTTP"}}},
                    "publicEndpoint": {"containerName": "web", "containerPort": 80},
                },
            }),
        )
        .await;
    let running = wait_for_state(&emulator, region, name, "RUNNING").await;
    let deployment = running["currentDeployment"].clone();
    assert_eq!(
        deployment["version"].as_i64(),
        Some(1),
        "case the created deployment: a service's first deployment is version 1, got \
         {deployment:?}"
    );

    let disabled = emulator
        .update_ok(
            "disabling a serving service",
            region,
            json!({"serviceName": name, "isDisabled": true}),
        )
        .await;
    assert_eq!(
        disabled["currentDeployment"], deployment,
        "case disabling a serving service: the deployment is kept as it stands, got {:?}",
        disabled["currentDeployment"]
    );
    let settled = wait_for_state(&emulator, region, name, "DISABLED").await;
    assert_eq!(
        settled["currentDeployment"], deployment,
        "case a disabled service: the deployment is still the one it holds, got {:?}",
        settled["currentDeployment"]
    );
    assert!(
        settled["nextDeployment"].is_null(),
        "case a disabled service: nothing is waiting to be deployed, got {:?}",
        settled["nextDeployment"]
    );

    emulator
        .update_ok(
            "enabling it again",
            region,
            json!({"serviceName": name, "isDisabled": false}),
        )
        .await;
    let enabled = wait_for_state(&emulator, region, name, "RUNNING").await;
    assert_eq!(
        enabled["currentDeployment"], deployment,
        "case enabling it again: the same deployment is serving, got {:?}",
        enabled["currentDeployment"]
    );
}

/// RULE: an update that names a deployment still waiting to be activated leaves
/// it waiting, and the clock activates it as the service comes back to serving.
#[tokio::test]
async fn an_update_leaves_a_waiting_deployment_waiting() {
    let emulator = Emulator::new();
    let region = "eu-central-1";
    let name = "deploying-service";
    emulator
        .create(region, json!({"serviceName": name, "power": "nano", "scale": 1}))
        .await;
    wait_for_state(&emulator, region, name, "READY").await;

    let (status, deployed) = emulator
        .call(
            region,
            "CreateContainerServiceDeployment",
            json!({
                "serviceName": name,
                "containers": {"api": {"image": "my-registry/api:1", "ports": {"8080": "HTTP"}}},
                "publicEndpoint": {"containerName": "api", "containerPort": 8080},
            }),
        )
        .await;
    assert_eq!(
        status, 201,
        "case deploying to it: expected HTTP 201, got {status} {deployed:?}"
    );

    // The update lands while that deployment is still waiting.
    let updated = emulator
        .update_ok(
            "updating a service that is launching a deployment",
            region,
            json!({"serviceName": name, "scale": 5}),
        )
        .await;
    assert_eq!(
        updated["state"].as_str(),
        Some("UPDATING"),
        "case updating a launching service: expected UPDATING, got {updated:?}"
    );
    assert_eq!(
        updated["nextDeployment"]["version"].as_i64(),
        Some(1),
        "case updating a launching service: the waiting deployment is kept, got {:?}",
        updated["nextDeployment"]
    );

    let settled = wait_for_state(&emulator, region, name, "RUNNING").await;
    assert_eq!(
        settled["currentDeployment"]["state"].as_str(),
        Some("ACTIVE"),
        "case after the clock settled it: the waiting deployment became the active one, got \
         {settled:?}"
    );
    assert_eq!(
        settled["scale"].as_i64(),
        Some(5),
        "case after the clock settled it: expected the updated scale, got {:?}",
        settled["scale"]
    );
}

/// RULE: a well-formed name that names no stored container service in the region
/// of the request is refused with the service's `DoesNotExist` envelope — HTTP
/// 400, `NotFoundException`, body code `DoesNotExist` and a message naming the
/// resource type and the requested name — and changes nothing.
#[tokio::test]
async fn an_update_naming_no_stored_service_does_not_exist() {
    let emulator = Emulator::new();
    let present = emulator
        .create(
            "us-east-1",
            json!({"serviceName": "present", "power": "nano", "scale": 1}),
        )
        .await;

    for (case, region, name) in [
        ("a name nothing created", "us-east-1", "absent"),
        ("a name created in another region", "us-west-1", "present"),
    ] {
        let (status, refused) = emulator
            .update(region, json!({"serviceName": name, "scale": 2}))
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected a 400, got {status} {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "NotFoundException",
            "case {case}: expected NotFoundException, got {refused:?}"
        );
        assert_eq!(
            refused["code"].as_str(),
            Some("DoesNotExist"),
            "case {case}: expected the DoesNotExist body code, got {refused:?}"
        );
        assert_eq!(
            refused["message"].as_str(),
            Some(format!("The ContainerService does not exist: {name}").as_str()),
            "case {case}: expected the resource-type/name message, got {refused:?}"
        );
    }

    // And the service that does exist is untouched by either refusal.
    let read = emulator.read("us-east-1", "present").await;
    assert_eq!(
        read["scale"].as_i64(),
        present["scale"].as_i64(),
        "case the stored service after two refusals: expected its scale unchanged, got {read:?}"
    );
}

/// RULE: an update this service cannot serve changes nothing and is refused with
/// a 400 — the modelled `InvalidInputException` for a rule the model cannot
/// express, and the boundary walk's own refusal for a member outside a modelled
/// constraint.
#[tokio::test]
async fn an_unservable_update_is_refused_and_changes_nothing() {
    // The rows: one public domain name past the documented four, a scale one
    // past each modelled bound, and a power outside the modelled enum.
    let cases: &[(&str, Value, Option<&str>)] = &[
        (
            "one public domain name more than the documented four",
            json!({
                "publicDomainNames": {
                    "cert": ["a.example.com", "b.example.com", "c.example.com"],
                    "other": ["d.example.com", "e.example.com"],
                },
            }),
            Some("InvalidInputException"),
        ),
        (
            "a scale below the modelled minimum",
            json!({"scale": 0}),
            None,
        ),
        (
            "a scale above the modelled maximum",
            json!({"scale": 21}),
            None,
        ),
        (
            "a power outside the modelled enum",
            json!({"power": "gigantic"}),
            None,
        ),
    ];

    for (case, asked, want_code) in cases {
        let emulator = Emulator::new();
        let region = "us-east-1";
        let name = "unchanged";
        let create = json!({
            "serviceName": name,
            "power": "micro",
            "scale": 2,
            "publicDomainNames": {"kept-cert": ["kept.example.com"]},
        });
        issue_certificates_of(&emulator, &create).await;
        let created = emulator.create(region, create).await;

        let mut body = asked.clone();
        body["serviceName"] = json!(name);
        let (status, refused) = emulator.update(region, body).await;
        assert_eq!(
            status, 400,
            "case {case}: expected a 400, got {status} {refused:?}"
        );
        if let Some(want_code) = want_code {
            assert_eq!(
                error_code(&refused),
                *want_code,
                "case {case}: expected {want_code}, got {refused:?}"
            );
        }

        let read = emulator.read(region, name).await;
        for member in ["power", "powerId", "scale", "publicDomainNames", "state"] {
            assert_eq!(
                read[member], created[member],
                "case {case}: a refused update must leave {member} alone, got {:?}",
                read[member]
            );
        }
    }
}

/// RULE: a container service name identifies one service WITHIN one region, so an
/// update made in one region reaches that region's service only.
#[tokio::test]
async fn an_update_reaches_only_the_service_of_its_own_region() {
    let emulator = Emulator::new();
    let name = "shared-name";
    let body = json!({"serviceName": name, "power": "nano", "scale": 1});
    for region in ["us-east-1", "eu-west-2"] {
        emulator.create(region, body.clone()).await;
        wait_for_state(&emulator, region, name, "READY").await;
    }

    emulator
        .update_ok(
            "updating the service of one region",
            "us-east-1",
            json!({"serviceName": name, "power": "large", "scale": 11}),
        )
        .await;

    let updated = wait_for_state(&emulator, "us-east-1", name, "READY").await;
    assert_eq!(
        (updated["power"].as_str(), updated["scale"].as_i64()),
        (Some("large"), Some(11)),
        "case the updated region: expected the new capacity, got {updated:?}"
    );
    let untouched = emulator.read("eu-west-2", name).await;
    assert_eq!(
        (untouched["power"].as_str(), untouched["scale"].as_i64()),
        (Some("nano"), Some(1)),
        "case the other region's service of the same name: expected it untouched, got \
         {untouched:?}"
    );
}
