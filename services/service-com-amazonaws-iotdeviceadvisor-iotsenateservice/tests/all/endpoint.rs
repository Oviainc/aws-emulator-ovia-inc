// SPDX-License-Identifier: CC0-1.0

//! `GetEndpoint` over the wire: which host Device Advisor reports, for which request.
//!
//! The rule under test is one sentence: the endpoint is a single DNS label under
//! `.deviceadvisor.iot.<region>.amazonaws.com`, where the region is the one the request
//! was signed for, the label is the account's when the request names no device and a
//! stable per-device one when it names a device, and a device identity whose ARN is not
//! one for the member carrying it — or that the named authentication method cannot use —
//! is refused with the modelled `ValidationException`.
//!
//! Every case below drives a real HTTP request through the same `MultiServiceRouter` the
//! emulator serves on a socket. The tables span what the rule generalises over: the
//! signing region (several shapes, plus one that is not a region at all), which of the
//! three device members name the device and how many of them, both `AuthenticationMethod`
//! values the model enumerates, and the ways an ARN can fail to be one.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iotdeviceadvisor_iotsenateservice as iotdeviceadvisor;
use smithy_server_runtime::MultiServiceRouter;

/// The suffix every Device Advisor host carries below its label, for `region`.
fn host_suffix(region: &str) -> String {
    format!(".deviceadvisor.iot.{region}.amazonaws.com")
}

/// A thing ARN for a device under test (`arn:aws:iot:<region>:<account>:thing/<name>`).
fn thing_arn(name: &str) -> String {
    format!("arn:aws:iot:us-east-1:000000000000:thing/{name}")
}

/// An IoT client-certificate ARN (`arn:aws:iot:<region>:<account>:cert/<id>`).
fn certificate_arn(id: &str) -> String {
    format!("arn:aws:iot:us-east-1:000000000000:cert/{id}")
}

/// An IAM role ARN — a global service's ARN, so its region field is empty.
fn role_arn(name: &str) -> String {
    format!("arn:aws:iam::000000000000:role/{name}")
}

/// The emulator, assembled in-process with authentication off: this suite asserts
/// behaviour, and signature verification is the framework's own concern.
struct Emulator {
    router: MultiServiceRouter,
    /// The one shared engine the handlers write through, kept so a case can read the
    /// endpoint a suite run PERSISTED and compare it with the vended one.
    engine: Arc<engine::InMemoryEngine>,
}

/// One response, reduced to what a client branches on.
struct Answer {
    status: u16,
    body: Value,
    /// The restJson1 error code (`x-amzn-errortype`), when the answer carries one.
    error_code: Option<String>,
}

impl Answer {
    fn string(&self, name: &str) -> String {
        self.body[name]
            .as_str()
            .unwrap_or_else(|| panic!("member {name} is not a string in {}", self.body))
            .to_string()
    }
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(iotdeviceadvisor::EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// Send one request, signed (in scope only — authentication is off) for `region`, so
    /// the handler resolves the same region a real SDK client would.
    async fn call(
        &self,
        method: &str,
        path_and_query: &str,
        region: &str,
        body: Option<Value>,
    ) -> Answer {
        let payload = body.map_or_else(bytes::Bytes::new, |value| {
            bytes::Bytes::from(serde_json::to_vec(&value).expect("the request body serializes"))
        });
        let request = http::Request::builder()
            .method(method)
            .uri(path_and_query)
            .header("host", "api.iotdeviceadvisor.us-east-1.amazonaws.com")
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/iotdeviceadvisor/\
                     aws4_request, SignedHeaders=host, Signature=0123456789abcdef"
                ),
            )
            .body(payload)
            .expect("the request builds");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(ToString::to_string);
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Answer {
            status,
            body,
            error_code,
        }
    }

    /// `GET /endpoint` with the query members this request carries.
    async fn get_endpoint(&self, query: &[(&str, &str)], region: &str) -> Answer {
        let mut path = "/endpoint".to_string();
        for (index, (name, value)) in query.iter().enumerate() {
            path.push(if index == 0 { '?' } else { '&' });
            path.push_str(name);
            path.push('=');
            path.push_str(&percent_encode(value));
        }
        self.call("GET", &path, region, None).await
    }

    /// The `endpoint` the store holds on the single suite run it has, if any.
    fn persisted_run_endpoint(&self) -> Option<String> {
        let state = iotdeviceadvisor::state::service_state(&*self.engine)
            .expect("the service's store is reachable from the shared engine");
        let store = state.lock();
        store.runs.values().next().map(|run| run.endpoint.clone())
    }
}

/// A query value as an SDK sends it: everything outside the unreserved set escaped, which
/// for an ARN is the `:` and `/` it is built from.
fn percent_encode(value: &str) -> String {
    value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '~') {
                c.to_string()
            } else {
                c.to_string()
                    .bytes()
                    .map(|byte| format!("%{byte:02X}"))
                    .collect()
            }
        })
        .collect()
}

/// Assert `endpoint` is a Device Advisor host in `region`: one lowercase-alphanumeric DNS
/// label under the service's endpoint domain, inside the modelled `Endpoint` length
/// bounds. Returns the label.
fn assert_host_shape(case: &str, endpoint: &str, region: &str) -> String {
    let suffix = host_suffix(region);
    let label = endpoint
        .strip_suffix(&suffix)
        .unwrap_or_else(|| panic!("case {case:?}: expected a host ending in {suffix}, got {endpoint:?}"))
        .to_string();
    assert!(
        !label.is_empty()
            && label
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()),
        "case {case:?}: the host's label must be one lowercase-alphanumeric DNS label, \
         got {label:?} (endpoint {endpoint:?})"
    );
    let bounds = iotdeviceadvisor::state::MIN_ENDPOINT_LENGTH
        ..=iotdeviceadvisor::state::MAX_ENDPOINT_LENGTH;
    assert!(
        bounds.contains(&endpoint.len()),
        "case {case:?}: the endpoint must satisfy the modelled len 45..75, got {endpoint:?} \
         ({} chars)",
        endpoint.len()
    );
    label
}

/// A request that names no device gets the ACCOUNT-level host, in the region it was signed
/// for, and the same host every time.
#[tokio::test]
async fn an_account_level_endpoint_names_the_requests_region() {
    let emulator = Emulator::new();

    // Regions of several shapes, including the longest AWS publishes and a GovCloud one.
    for region in [
        "us-east-1",
        "us-west-2",
        "eu-central-1",
        "ap-southeast-4",
        "il-central-1",
        "us-gov-west-1",
    ] {
        let answer = emulator.get_endpoint(&[], region).await;
        assert_eq!(
            answer.status, 200,
            "case {region:?}: expected HTTP 200 from GetEndpoint, got {} {}",
            answer.status, answer.body
        );
        let endpoint = answer.string("endpoint");
        assert_host_shape(region, &endpoint, region);

        let again = emulator.get_endpoint(&[], region).await;
        assert_eq!(
            again.string("endpoint"),
            endpoint,
            "case {region:?}: the account-level endpoint must be stable across calls"
        );
    }
}

/// A credential scope that is not a published region shape cannot be reflected into a
/// hostname, so the host falls back to the default region — the same fallback the ARNs
/// this service mints use.
#[tokio::test]
async fn a_region_that_is_not_one_falls_back_to_the_default_region() {
    let emulator = Emulator::new();
    let default_region = iotdeviceadvisor::state::DEFAULT_REGION;

    // (case, the region in the credential scope)
    let rows: Vec<(&str, &str)> = vec![
        ("upper case", "US-EAST-1"),
        ("a host, not a region", "evil.example.com"),
        ("an ARN separator", "us-east-1:9999"),
        ("no trailing number", "us-east"),
        ("a single-letter prefix", "u-east-1"),
    ];

    for (case, region) in rows {
        let answer = emulator.get_endpoint(&[], region).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200 from GetEndpoint, got {} {}",
            answer.status, answer.body
        );
        let endpoint = answer.string("endpoint");
        assert!(
            endpoint.ends_with(&host_suffix(default_region)),
            "case {case:?}: a region that is not one must fall back to {default_region}, \
             got {endpoint:?}"
        );
        assert_host_shape(case, &endpoint, default_region);
    }
}

/// The host `GetEndpoint` vends is the host a suite run of that region PERSISTED: one
/// derivation, so what a device is told to connect to cannot differ between the two.
#[tokio::test]
async fn the_vended_host_is_the_host_a_suite_run_persisted() {
    let emulator = Emulator::new();
    let region = "eu-west-3";

    let created = emulator
        .call(
            "POST",
            "/suiteDefinitions",
            region,
            Some(json!({
                "suiteDefinitionConfiguration": {
                    "suiteDefinitionName": "endpoint-suite",
                    "rootGroup": "g",
                    "devicePermissionRoleArn": role_arn("DeviceAdvisorRole"),
                }
            })),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case \"create a definition\": expected HTTP 200, got {} {}",
        created.status, created.body
    );
    let definition_id = created.string("suiteDefinitionId");

    let started = emulator
        .call(
            "POST",
            &format!("/suiteDefinitions/{definition_id}/suiteRuns"),
            region,
            Some(json!({
                "suiteRunConfiguration": {
                    "primaryDevice": { "thingArn": thing_arn("endpoint-device") }
                }
            })),
        )
        .await;
    assert_eq!(
        started.status, 200,
        "case \"start a run\": expected HTTP 200, got {} {}",
        started.status, started.body
    );

    let vended = emulator.get_endpoint(&[], region).await.string("endpoint");
    assert_eq!(
        started.string("endpoint"),
        vended,
        "case \"a started run\": StartSuiteRun must report the host GetEndpoint vends"
    );
    assert_eq!(
        emulator.persisted_run_endpoint().as_deref(),
        Some(vended.as_str()),
        "case \"the stored run\": the persisted run endpoint must be the host GetEndpoint vends"
    );
}

/// One row of the device-level table: a request that names a device, and the device it
/// names.
struct DeviceRow {
    /// What this row is called in an assertion message.
    case: &'static str,
    /// The query members the request carries.
    query: Vec<(&'static str, String)>,
    /// The DEVICE IDENTITY the row names — the ARNs it carries, and nothing else. Two
    /// rows with the same identity must get the same host: the authentication method says
    /// how the device connects, not which device it is.
    identity: &'static str,
}

/// The device identities the device-level rule is asserted over: rows differing in WHICH
/// of the three members name the device, in HOW MANY of them do, and in the
/// `AuthenticationMethod` the request carries (both values the model enumerates).
fn device_rows() -> Vec<DeviceRow> {
    let row = |case, query, identity| DeviceRow {
        case,
        query,
        identity,
    };
    vec![
        row(
            "thing only",
            vec![("thingArn", thing_arn("device-a"))],
            "thing-a",
        ),
        row(
            "the same thing with X.509",
            vec![
                ("thingArn", thing_arn("device-a")),
                ("authenticationMethod", "X509ClientCertificate".to_string()),
            ],
            "thing-a",
        ),
        row(
            "another thing",
            vec![("thingArn", thing_arn("device-b"))],
            "thing-b",
        ),
        row(
            "certificate only",
            vec![("certificateArn", certificate_arn("cert-a"))],
            "cert-a",
        ),
        row(
            "role with SigV4",
            vec![
                ("deviceRoleArn", role_arn("DeviceRoleA")),
                ("authenticationMethod", "SignatureVersion4".to_string()),
            ],
            "role-a",
        ),
        row(
            "the same role with no method named",
            vec![("deviceRoleArn", role_arn("DeviceRoleA"))],
            "role-a",
        ),
        row(
            "thing and certificate",
            vec![
                ("thingArn", thing_arn("device-a")),
                ("certificateArn", certificate_arn("cert-a")),
            ],
            "thing-a+cert-a",
        ),
        row(
            "every member, SigV4",
            vec![
                ("thingArn", thing_arn("device-a")),
                ("certificateArn", certificate_arn("cert-a")),
                ("deviceRoleArn", role_arn("DeviceRoleA")),
                ("authenticationMethod", "SignatureVersion4".to_string()),
            ],
            "thing-a+cert-a+role-a",
        ),
    ]
}

/// A request that names a device gets a DEVICE-level host: a well-formed host of this
/// service's endpoint domain, stable for that device, different from the account-level one
/// and from every other device's — and unchanged by the authentication method, which says
/// how the device connects rather than which device it is.
#[tokio::test]
async fn a_named_device_gets_its_own_stable_endpoint() {
    let emulator = Emulator::new();
    let region = "us-east-1";
    let account_level = emulator.get_endpoint(&[], region).await.string("endpoint");

    // Every row already asserted, so a later row can be compared with it.
    let mut seen: Vec<(&str, &str, String)> = Vec::new();
    for DeviceRow {
        case,
        query: members,
        identity,
    } in device_rows()
    {
        let query: Vec<(&str, &str)> = members
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect();
        let answer = emulator.get_endpoint(&query, region).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200 from GetEndpoint, got {} {}",
            answer.status, answer.body
        );
        let endpoint = answer.string("endpoint");
        let label = assert_host_shape(case, &endpoint, region);
        assert_eq!(
            label.len(),
            iotdeviceadvisor::state::DEVICE_LABEL_LENGTH,
            "case {case:?}: a device label is one fixed-length DNS label, got {label:?}"
        );
        assert!(
            label.starts_with(|c: char| c.is_ascii_lowercase()),
            "case {case:?}: a hostname label starts with a letter, got {label:?}"
        );
        assert_ne!(
            endpoint, account_level,
            "case {case:?}: a device-level endpoint must differ from the account-level one"
        );

        let again = emulator.get_endpoint(&query, region).await;
        assert_eq!(
            again.string("endpoint"),
            endpoint,
            "case {case:?}: the same request must get the same endpoint on every call"
        );

        for (other_case, other_identity, other) in &seen {
            if *other_identity == identity {
                assert_eq!(
                    &endpoint, other,
                    "case {case:?}: names the same device as case {other_case:?}, so it must \
                     get the same host, got {endpoint:?}"
                );
            } else {
                assert_ne!(
                    &endpoint, other,
                    "case {case:?}: names another device than case {other_case:?}, so it must \
                     get another host, got {endpoint:?}"
                );
            }
        }
        seen.push((case, identity, endpoint));
    }
}

/// A device-level host names the region the request was signed for, exactly as the
/// account-level one does: one device, two regions, two hosts.
#[tokio::test]
async fn a_device_endpoint_is_scoped_to_the_requests_region() {
    let emulator = Emulator::new();
    let thing = thing_arn("multi-region-device");
    let query = [("thingArn", thing.as_str())];

    let mut seen: Vec<(&str, String)> = Vec::new();
    for region in ["us-east-1", "eu-central-1", "ap-southeast-4"] {
        let endpoint = emulator
            .get_endpoint(&query, region)
            .await
            .string("endpoint");
        assert_host_shape(region, &endpoint, region);
        for (other_region, other) in &seen {
            assert_ne!(
                &endpoint, other,
                "case {region:?}: a device's host in {region} must differ from its host in \
                 {other_region}"
            );
        }
        seen.push((region, endpoint));
    }
}

/// An ARN that is not one for the member carrying it, and an authentication method with no
/// identity it can use, are refused with the operation's modelled client-input error —
/// never a 501 and never a host.
#[tokio::test]
async fn a_malformed_or_contradictory_request_is_refused() {
    let emulator = Emulator::new();

    // (case, query) — every ARN here is at least the modelled 20 characters, so what is
    // under test is the ARN form and the member/method agreement, not the length bound.
    let rows: Vec<(&str, Vec<(&str, String)>)> = vec![
        (
            "thingArn is not an ARN at all",
            vec![("thingArn", "my-favourite-thing-name".to_string())],
        ),
        (
            "thingArn is an IAM role ARN",
            vec![("thingArn", role_arn("NotAThing"))],
        ),
        (
            "certificateArn is an IAM role ARN",
            vec![("certificateArn", role_arn("NotACertificate"))],
        ),
        (
            "deviceRoleArn is an IoT thing ARN",
            vec![("deviceRoleArn", thing_arn("not-a-role"))],
        ),
        (
            "thingArn has no resource",
            vec![("thingArn", "arn:aws:iot:us-east-1:000000000000:".to_string())],
        ),
        (
            "thingArn has no partition",
            vec![("thingArn", "arn::iot:us-east-1:000000000000:thing/x".to_string())],
        ),
        (
            "thingArn has too few fields",
            vec![("thingArn", "arn:aws:iot:thing/my-thing-name".to_string())],
        ),
        (
            "SigV4 with no device role",
            vec![
                ("thingArn", thing_arn("sigv4-device")),
                ("authenticationMethod", "SignatureVersion4".to_string()),
            ],
        ),
        (
            "X.509 with only a device role",
            vec![
                ("deviceRoleArn", role_arn("X509Role")),
                ("authenticationMethod", "X509ClientCertificate".to_string()),
            ],
        ),
    ];

    for (case, members) in rows {
        let query: Vec<(&str, &str)> = members
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect();
        let answer = emulator.get_endpoint(&query, "us-east-1").await;
        assert_eq!(
            (answer.status, answer.error_code.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: expected a 400 ValidationException, got {} {:?} {}",
            answer.status,
            answer.error_code,
            answer.body
        );
    }
}

/// The modelled constraints are refused before the handler runs: an ARN below the member's
/// minimum length and an `authenticationMethod` outside the enum are both 400s, not hosts.
#[tokio::test]
async fn values_outside_the_modelled_constraints_are_refused() {
    let emulator = Emulator::new();

    // (case, query) — `AmazonResourceName` is len 20..2048 and `AuthenticationMethod` is
    // an enum of exactly two values (source: `smithy_query(query="operation",
    // operation="GetEndpoint")`).
    let rows: Vec<(&str, Vec<(&str, &str)>)> = vec![
        ("thingArn below the minimum length", vec![("thingArn", "arn:aws:iot:::t")]),
        (
            "certificateArn below the minimum length",
            vec![("certificateArn", "arn:aws:iot:::c")],
        ),
        (
            "authenticationMethod outside the enum",
            vec![("authenticationMethod", "Anonymous")],
        ),
    ];

    for (case, query) in rows {
        let answer = emulator.get_endpoint(&query, "us-east-1").await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: a value outside a modelled constraint must be refused, got {} {}",
            answer.status, answer.body
        );
    }
}

/// An authentication method on its own names no device, so it asks for the account-level
/// endpoint — for both values the model enumerates.
#[tokio::test]
async fn an_authentication_method_alone_still_asks_for_the_account_endpoint() {
    let emulator = Emulator::new();
    let region = "us-east-1";
    let account_level = emulator.get_endpoint(&[], region).await.string("endpoint");

    for method in ["X509ClientCertificate", "SignatureVersion4"] {
        let answer = emulator
            .get_endpoint(&[("authenticationMethod", method)], region)
            .await;
        assert_eq!(
            answer.status, 200,
            "case {method:?}: expected HTTP 200 from GetEndpoint, got {} {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.string("endpoint"),
            account_level,
            "case {method:?}: a request naming no device must get the account-level endpoint"
        );
    }
}
