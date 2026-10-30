// SPDX-License-Identifier: CC0-1.0

//! The value domain of a public endpoint's `healthCheck.successCodes`, driven
//! over the wire through both doors that build a deployment from a request.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: a health-check member whose own modelled documentation
//! states a value domain is checked against that domain before the deployment is
//! stored. `ContainerServiceHealthCheckConfig$successCodes` states its domain
//! whole — the HTTP codes are "values between 200 and 499", given as "multiple
//! values (for example, 200,202) or a range of values (for example, 200-299)" —
//! so a value inside it is stored and reported back verbatim, and one outside it
//! is the modelled `InvalidInputException` (400) from either
//! `CreateContainerServiceDeployment` or `CreateContainerService`, which stores
//! nothing.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::instance_lifecycle::error_code;

/// The `X-Amz-Target` prefix this service routes on (`OPERATIONS.md`, "Wire
/// protocol").
const TARGET_PREFIX: &str = "Lightsail_20161128";

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

    /// Create the container service `body` describes, answering the status and
    /// the parsed body so a refusal can be inspected too.
    async fn create(&self, region: &str, body: Value) -> (u16, Value) {
        self.call(region, "CreateContainerService", body).await
    }

    /// Create the container service `body` describes and require its modelled
    /// 201, answering the record the create reports.
    async fn create_ok(&self, case: &str, region: &str, body: Value) -> Value {
        let (status, parsed) = self.create(region, body).await;
        assert_eq!(
            status, 201,
            "case {case}: expected HTTP 201 from the create, got {status} {parsed:?}"
        );
        parsed["containerService"].clone()
    }

    /// Deploy `body` to a container service, answering the status and the parsed
    /// body so a refusal can be inspected too.
    async fn deploy(&self, region: &str, body: Value) -> (u16, Value) {
        self.call(region, "CreateContainerServiceDeployment", body)
            .await
    }

    /// Deploy `body` and require the modelled 201 (this operation's success
    /// status), answering the `containerService` the deployment reports.
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

    /// The name of every container service stored in `region`, as
    /// `GetContainerServices` reports them when the request names none.
    async fn names(&self, case: &str, region: &str) -> Vec<String> {
        let (status, parsed) = self.call(region, "GetContainerServices", json!({})).await;
        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200 listing the services of {region}, got \
             {status} {parsed:?}"
        );
        parsed["containerServices"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|service| service["containerServiceName"].as_str().map(String::from))
            .collect()
    }

    /// The deployment history of `name` in `region`, as
    /// `GetContainerServiceDeployments` reports it.
    async fn history(&self, case: &str, region: &str, name: &str) -> Vec<Value> {
        let (status, parsed) = self
            .call(
                region,
                "GetContainerServiceDeployments",
                json!({"serviceName": name}),
            )
            .await;
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
}

/// Values INSIDE the documented domain, one row per way a value can be written:
/// a single code (at each published bound and between them), several codes, a
/// range, a range spanning the published bounds, a range of one code, and a list
/// mixing the two forms — the list is of VALUES, each of which is one of the two
/// documented forms.
///
/// Source of the domain and of both example forms:
/// `smithy_query(query="shape", shape="ContainerServiceHealthCheckConfig")`,
/// member `successCodes` — "You can specify values between 200 and 499. You can
/// specify multiple values (for example, 200,202) or a range of values (for
/// example, 200-299)."
const INSIDE_THE_DOMAIN: &[(&str, &str)] = &[
    ("the lower published bound on its own", "200"),
    ("the upper published bound on its own", "499"),
    ("one code between the published bounds", "404"),
    ("two codes, the documented multiple-value form", "200,202"),
    ("four codes in one list", "200,201,204,301"),
    ("a range, the documented range form", "200-299"),
    ("a range spanning both published bounds", "200-499"),
    ("a range whose two bounds are the same code", "418-418"),
    ("a list mixing single codes and a range", "200,301-303,404"),
];

/// Values OUTSIDE the documented domain, one row per way a value can fall out of
/// it: a code below the lower published bound, a code above the upper one, a
/// code far past it, text that names no code, no value at all, a list with a
/// missing or non-numeric value, a range missing a bound, a range whose bounds
/// are both outside the published ones, a range above them, a range running
/// downwards, a value not written as the documented examples write one, and a
/// range of three bounds.
const OUTSIDE_THE_DOMAIN: &[(&str, &str)] = &[
    ("a code one below the lower published bound", "199"),
    ("a code one above the upper published bound", "500"),
    ("a code far above the published bounds", "600"),
    ("text that names no code at all", "banana"),
    ("no value at all", ""),
    ("a list with an empty value", "200,"),
    ("a list whose second value names no code", "200,banana"),
    ("a range missing its upper bound", "200-"),
    ("a range whose bounds are both outside the published ones", "199-999"),
    ("a range entirely above the upper published bound", "500-600"),
    ("a range running downwards", "300-200"),
    ("a value written with a space in it", "200, 202"),
    ("a range of three bounds", "200-299-399"),
];

/// The member the refusals must name, as the model nests it in a deployment
/// request.
const REFUSED_MEMBER: &str = "publicEndpoint.healthCheck.successCodes";

/// The `publicEndpoint` of a deployment of one container, whose health check
/// asks for `codes` and leaves every other member to its documented default.
fn public_endpoint(codes: &str) -> Value {
    json!({
        "containerName": "web",
        "containerPort": 80,
        "healthCheck": {"successCodes": codes},
    })
}

/// The single container every deployment in this suite launches.
fn containers() -> Value {
    json!({"web": {"image": "nginx:latest", "ports": {"80": "HTTP"}}})
}

/// A `CreateContainerServiceDeployment` body for `name` whose public endpoint
/// asks for `codes`.
fn deploy_body(name: &str, codes: &str) -> Value {
    json!({
        "serviceName": name,
        "containers": containers(),
        "publicEndpoint": public_endpoint(codes),
    })
}

/// A `CreateContainerService` body for `name` carrying a deployment whose public
/// endpoint asks for `codes`.
fn create_body(name: &str, codes: &str) -> Value {
    json!({
        "serviceName": name,
        "power": "nano",
        "scale": 1,
        "deployment": {
            "containers": containers(),
            "publicEndpoint": public_endpoint(codes),
        },
    })
}

/// Assert `health` reports `codes` and leaves every sibling member at the default
/// that member documents — a checked value is stored as the request wrote it, and
/// checking it changes nothing else.
fn assert_health_check(case: &str, health: &Value, codes: &str) {
    assert_eq!(
        health["successCodes"].as_str(),
        Some(codes),
        "case {case}: expected the asked-for successCodes, got {health:?}"
    );
    for (member, default) in [
        ("healthyThreshold", 2),
        ("unhealthyThreshold", 2),
        ("timeoutSeconds", 2),
        ("intervalSeconds", 5),
    ] {
        assert_eq!(
            health[member].as_i64(),
            Some(default),
            "case {case}: expected the documented default {member}, got {health:?}"
        );
    }
    assert_eq!(
        health["path"].as_str(),
        Some("/"),
        "case {case}: expected the documented default path, got {health:?}"
    );
}

/// Assert `parsed` is the modelled `InvalidInputException` (400) naming the
/// health check's success codes.
fn assert_refused(case: &str, status: u16, parsed: &Value) {
    assert_eq!(
        status, 400,
        "case {case}: expected HTTP 400, got {status} {parsed:?}"
    );
    assert_eq!(
        error_code(parsed),
        "InvalidInputException",
        "case {case}: expected the modelled InvalidInputException, got {parsed:?}"
    );
    assert!(
        parsed["message"]
            .as_str()
            .is_some_and(|message| message.contains(REFUSED_MEMBER)),
        "case {case}: expected the refusal to name {REFUSED_MEMBER}, got {parsed:?}"
    );
}

/// RULE: a `successCodes` value inside the domain its member documents is
/// accepted and reported back exactly as the request wrote it, by both the
/// service record and the deployment history.
#[tokio::test]
async fn success_codes_inside_the_documented_domain_are_stored_and_reported() {
    let emulator = Emulator::new();
    let region = "us-east-1";
    let name = "codes-in-domain";
    emulator
        .create_ok(
            "creating the service",
            region,
            json!({"serviceName": name, "power": "nano", "scale": 1}),
        )
        .await;

    for (index, (case, codes)) in INSIDE_THE_DOMAIN.iter().enumerate() {
        // The service was created carrying no deployment, so the first
        // deployment created here is version 1.
        let version = i64::try_from(index).expect("the row count fits an i64") + 1;
        let reported = emulator
            .deploy_ok(case, region, deploy_body(name, codes))
            .await;
        // The deployment just created is the one the service is launching, so the
        // record it answers already carries it.
        let launching = [&reported["nextDeployment"], &reported["currentDeployment"]]
            .into_iter()
            .find(|deployment| deployment["version"].as_i64() == Some(version))
            .cloned()
            .unwrap_or_else(|| {
                panic!(
                    "case {case}: expected version {version} on the answered record, got \
                     {reported:?}"
                )
            });
        assert_health_check(case, &launching["publicEndpoint"]["healthCheck"], codes);

        // ... and so does a later read of it, through both readers.
        let service = emulator.read(case, region, name).await;
        let stored = [&service["nextDeployment"], &service["currentDeployment"]]
            .into_iter()
            .find(|deployment| deployment["version"].as_i64() == Some(version))
            .cloned()
            .unwrap_or_else(|| {
                panic!(
                    "case {case}: expected version {version} on the stored record, got \
                     {service:?}"
                )
            });
        assert_health_check(case, &stored["publicEndpoint"]["healthCheck"], codes);

        let history = emulator.history(case, region, name).await;
        let in_history = history
            .iter()
            .find(|deployment| deployment["version"].as_i64() == Some(version))
            .unwrap_or_else(|| {
                panic!("case {case}: expected version {version} in the history, got {history:?}")
            });
        assert_health_check(case, &in_history["publicEndpoint"]["healthCheck"], codes);
    }
}

/// RULE: a `successCodes` value outside the documented domain is refused by
/// `CreateContainerServiceDeployment` with the modelled `InvalidInputException`,
/// and the service's deployments are left exactly as they were.
#[tokio::test]
async fn success_codes_outside_the_documented_domain_are_refused_by_a_deployment() {
    let emulator = Emulator::new();
    let region = "eu-west-1";
    let name = "codes-out-of-domain";
    emulator
        .create_ok(
            "creating the service",
            region,
            json!({"serviceName": name, "power": "micro", "scale": 1}),
        )
        .await;
    emulator
        .deploy_ok("the deployment that stands", region, deploy_body(name, "200"))
        .await;
    let before = emulator.history("the deployment that stands", region, name).await;

    for (case, codes) in OUTSIDE_THE_DOMAIN {
        let (status, parsed) = emulator.deploy(region, deploy_body(name, codes)).await;
        assert_refused(case, status, &parsed);
        let after = emulator.history(case, region, name).await;
        assert_eq!(
            after.len(),
            before.len(),
            "case {case}: a refused deployment must store nothing, got {after:?}"
        );
        assert_eq!(
            after
                .first()
                .map(|deployment| deployment["publicEndpoint"]["healthCheck"]["successCodes"]
                    .clone()),
            Some(json!("200")),
            "case {case}: the deployment that stands must keep its own successCodes, got \
             {after:?}"
        );
    }
}

/// RULE: the same domain is enforced on the other door into a deployment —
/// `CreateContainerService`'s own `deployment` — and a refused create stores no
/// service at all.
#[tokio::test]
async fn success_codes_are_checked_on_the_deployment_a_create_carries() {
    let emulator = Emulator::new();
    let region = "ap-southeast-2";

    // Inside the domain: the create keeps the value, and a read reports it.
    for (index, (case, codes)) in INSIDE_THE_DOMAIN.iter().enumerate() {
        let name = format!("create-in-domain-{index}");
        let created = emulator.create_ok(case, region, create_body(&name, codes)).await;
        // A deployment named at create is the service's FIRST, so version 1.
        let launching = [&created["nextDeployment"], &created["currentDeployment"]]
            .into_iter()
            .find(|deployment| deployment["version"].as_i64() == Some(1))
            .cloned()
            .unwrap_or_else(|| {
                panic!("case {case}: expected version 1 on the answered record, got {created:?}")
            });
        assert_health_check(case, &launching["publicEndpoint"]["healthCheck"], codes);
        let history = emulator.history(case, region, &name).await;
        let stored = history
            .first()
            .unwrap_or_else(|| panic!("case {case}: expected one deployment, got {history:?}"));
        assert_health_check(case, &stored["publicEndpoint"]["healthCheck"], codes);
    }

    // Outside it: refused, and nothing is created under that name.
    for (index, (case, codes)) in OUTSIDE_THE_DOMAIN.iter().enumerate() {
        let name = format!("create-out-of-domain-{index}");
        let (status, parsed) = emulator.create(region, create_body(&name, codes)).await;
        assert_refused(case, status, &parsed);
        let names = emulator.names(case, region).await;
        assert!(
            !names.contains(&name),
            "case {case}: a refused create must store no service, got {names:?}"
        );
    }
}
