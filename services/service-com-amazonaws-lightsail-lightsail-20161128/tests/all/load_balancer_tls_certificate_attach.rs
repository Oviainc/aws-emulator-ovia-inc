// SPDX-License-Identifier: CC0-1.0

//! `AttachLoadBalancerTlsCertificate` — putting one of a Lightsail load
//! balancer's SSL/TLS certificates in front of its traffic — driven over the
//! wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: a load balancer terminates HTTPS with ONE of its
//! certificates, so attaching the certificate `certificateName` names makes it
//! that one and detaches whichever of the same load balancer's certificates was
//! attached before, leaving every other member of the record as it was and
//! answering one terminal operation-log entry for the certificate it attached;
//! an attached certificate is what makes the load balancer report the
//! `HTTP_HTTPS` protocol and both public ports, and a request naming a load
//! balancer, or a certificate of that load balancer, the account does not hold
//! attaches nothing and is answered with Lightsail's `DoesNotExist` rejection.
//!
//! The rows differ along everything the rule generalises over: the Region the
//! load balancer lives in, how many certificates it holds, which of them the
//! request attaches, whether that certificate came from `CreateLoadBalancer` or
//! from a later `CreateLoadBalancerTlsCertificate`, the certificate's name and
//! domains, the instance port the load balancer forwards to, and whether the
//! load balancer has settled into `active` before the attach.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The UUID shape check this crate's suites share, rather than an Nth copy of it.
use crate::instance_lifecycle::is_uuid_shaped;

/// The `X-Amz-Target` prefix this service routes on (`OPERATIONS.md`, "Wire
/// protocol").
const TARGET_PREFIX: &str = "Lightsail_20161128";

/// The public ports a load balancer listens on, from the modelled
/// `LoadBalancer$publicPorts` documentation ("For HTTP, use port 80. For HTTPS,
/// use port 443.").
const HTTP_PORT: i64 = 80;
/// See [`HTTP_PORT`].
const HTTPS_PORT: i64 = 443;

/// The `LoadBalancerProtocol` a load balancer with no attached certificate
/// serves, and the one it serves with an attached certificate, from the modelled
/// enum ("You can't just have `HTTP_HTTPS`, but you can have just `HTTP`").
const PROTOCOL_HTTP: &str = "HTTP";
/// See [`PROTOCOL_HTTP`].
const PROTOCOL_HTTP_HTTPS: &str = "HTTP_HTTPS";

/// The `ResourceType` member an SSL/TLS certificate of a load balancer reports,
/// from the modelled `ResourceType` enum.
const CERTIFICATE_RESOURCE_TYPE: &str = "LoadBalancerTlsCertificate";

/// The `ResourceType` member a load balancer reports, from the same enum.
const LOAD_BALANCER_RESOURCE_TYPE: &str = "LoadBalancer";

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
        // is what names the Region a request is made in.
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
            .header("host", "127.0.0.1:9297")
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

    /// Call `operation` in `region` and require HTTP 200, answering the body.
    async fn ok(&self, region: &str, operation: &str, body: Value) -> Value {
        let (status, parsed) = self.call(region, operation, body).await;
        assert_eq!(
            status, 200,
            "case {operation} in {region}: expected HTTP 200, got {status} {parsed:?}"
        );
        parsed
    }

    /// The stored load balancer `name` in `region`, as `GetLoadBalancer` reports
    /// it.
    async fn read(&self, region: &str, name: &str) -> Value {
        self.ok(region, "GetLoadBalancer", json!({"loadBalancerName": name}))
            .await["loadBalancer"]
            .clone()
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

/// Poll `GetLoadBalancer` until `name` reports `want`.
///
/// The transition is owed by the clock, not by the poll: it arrives without
/// anyone asking more than once.
async fn wait_for_state(emulator: &Emulator, region: &str, name: &str, want: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let balancer = emulator.read(region, name).await;
        if balancer["state"].as_str() == Some(want) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case waiting for '{want}' on {name}: expected it within 5s, got {:?}",
            balancer["state"]
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// The `(name, isAttached)` pair of every certificate summary a load balancer
/// reports, in report order.
fn summaries(balancer: &Value) -> Vec<(String, bool)> {
    balancer["tlsCertificateSummaries"]
        .as_array()
        .map(|summaries| {
            summaries
                .iter()
                .map(|summary| {
                    (
                        summary["name"].as_str().unwrap_or_default().to_string(),
                        summary["isAttached"].as_bool().unwrap_or(false),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The public ports a load balancer reports, in report order.
fn public_ports(balancer: &Value) -> Vec<i64> {
    balancer["publicPorts"]
        .as_array()
        .map(|ports| ports.iter().filter_map(Value::as_i64).collect())
        .unwrap_or_default()
}

/// Assert the one operation-log entry an attach answers describes the
/// attachment of `certificate` in `region`.
fn assert_attach_operation(case: &str, answered: &Value, certificate: &str, region: &str) {
    let operations = answered["operations"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected an operations array, got {answered:?}"));
    assert_eq!(
        operations.len(),
        1,
        "case {case}: one certificate was attached, so one operation is answered, \
         got {operations:?}"
    );
    let operation = &operations[0];
    assert_eq!(
        operation["operationType"].as_str(),
        Some("AttachLoadBalancerTlsCertificate"),
        "case {case}: expected the AttachLoadBalancerTlsCertificate operation type, \
         got {operation:?}"
    );
    assert_eq!(
        operation["resourceType"].as_str(),
        Some(CERTIFICATE_RESOURCE_TYPE),
        "case {case}: expected the certificate resource type, got {operation:?}"
    );
    assert_eq!(
        operation["resourceName"].as_str(),
        Some(certificate),
        "case {case}: expected the attached certificate's name, got {operation:?}"
    );
    assert_eq!(
        operation["status"].as_str(),
        Some("Succeeded"),
        "case {case}: the attachment is done, so the operation succeeded, got {operation:?}"
    );
    assert_eq!(
        operation["isTerminal"].as_bool(),
        Some(true),
        "case {case}: the attachment is done, so the operation is terminal, got {operation:?}"
    );
    assert_eq!(
        operation["location"]["regionName"].as_str(),
        Some(region),
        "case {case}: expected the Region the load balancer lives in, got {operation:?}"
    );
    let operation_id = operation["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected an operation id, got {operation:?}"));
    assert!(
        is_uuid_shaped(operation_id),
        "case {case}: expected a UUID-shaped operation id, got {operation_id:?}"
    );
}

// ---------------------------------------------------------------------------
// The attach table
// ---------------------------------------------------------------------------

/// One load balancer, the certificates it holds, and which of them the row
/// attaches.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    region: &'static str,
    load_balancer: &'static str,
    /// The certificate `CreateLoadBalancer` is asked for, with its primary
    /// domain and the alternative domains it also covers.
    created_with: (&'static str, &'static str, &'static [&'static str]),
    /// A second certificate added by `CreateLoadBalancerTlsCertificate` — up to
    /// the 2 the modelled `certificateName` documentation admits per load
    /// balancer ("Each Lightsail load balancer can have up to 2 certificates
    /// associated with it at one time").
    added: Option<(&'static str, &'static str, &'static [&'static str])>,
    /// WHICH of the certificates above the row attaches first: `0` is the one
    /// the create asked for, `1` the added one.
    attach_first: usize,
    instance_port: i64,
    /// Whether to let the clock settle the load balancer into `active` before
    /// attaching.
    settle_first: bool,
}

const ROWS: &[Row] = &[
    Row {
        case: "the only certificate, no alternative domains, attached while provisioning",
        region: "us-east-1",
        load_balancer: "shop-lb",
        created_with: ("shop-cert", "example.com", &[]),
        added: None,
        attach_first: 0,
        instance_port: HTTP_PORT,
        settle_first: false,
    },
    Row {
        case: "the second certificate of two, attached once active",
        region: "eu-west-1",
        load_balancer: "eu-edge-lb",
        created_with: ("eu-edge-cert", "eu.example.com", &["www.eu.example.com"]),
        added: Some(("eu-edge-cert-2", "eu2.example.com", &[])),
        attach_first: 1,
        instance_port: 8080,
        settle_first: true,
    },
    Row {
        case: "the first certificate of two, three alternative domains",
        region: "ap-northeast-1",
        load_balancer: "apac-lb-01",
        created_with: (
            "apac-cert-01",
            "apac.example.com",
            &[
                "www.apac.example.com",
                "shop.apac.example.com",
                "apac.example.net",
            ],
        ),
        added: Some(("apac-cert-02", "apac2.example.com", &["www.apac2.example.com"])),
        attach_first: 0,
        instance_port: 3000,
        settle_first: false,
    },
    Row {
        case: "the highest modelled instance port, an underscore certificate name",
        region: "us-west-2",
        load_balancer: "west-lb-2",
        created_with: ("west_cert", "west.example.org", &["cdn.west.example.org"]),
        added: Some(("west-cert-2026-01", "west2.example.org", &[])),
        attach_first: 1,
        instance_port: 65535,
        settle_first: true,
    },
    Row {
        case: "the nine alternative domains the documentation admits, port 1",
        region: "us-east-2",
        load_balancer: "wide-lb",
        created_with: (
            "wide_cert",
            "wide.example.com",
            &[
                "a.wide.example.com",
                "b.wide.example.com",
                "c.wide.example.com",
                "d.wide.example.com",
                "e.wide.example.com",
                "f.wide.example.com",
                "g.wide.example.com",
                "h.wide.example.com",
                "i.wide.example.com",
            ],
        ),
        added: None,
        attach_first: 0,
        instance_port: 1,
        settle_first: false,
    },
];

impl Row {
    /// The `CreateLoadBalancer` request that puts this row's first certificate
    /// on this row's load balancer.
    fn create(&self) -> Value {
        let (certificate, domain, alternatives) = self.created_with;
        let mut body = serde_json::Map::new();
        body.insert("loadBalancerName".to_string(), json!(self.load_balancer));
        body.insert("instancePort".to_string(), json!(self.instance_port));
        body.insert("certificateName".to_string(), json!(certificate));
        body.insert("certificateDomainName".to_string(), json!(domain));
        if !alternatives.is_empty() {
            body.insert(
                "certificateAlternativeNames".to_string(),
                json!(alternatives),
            );
        }
        Value::Object(body)
    }

    /// The `CreateLoadBalancerTlsCertificate` request that adds this row's
    /// second certificate, when it has one.
    fn add(&self) -> Option<Value> {
        let (certificate, domain, alternatives) = self.added?;
        let mut body = serde_json::Map::new();
        body.insert("loadBalancerName".to_string(), json!(self.load_balancer));
        body.insert("certificateName".to_string(), json!(certificate));
        body.insert("certificateDomainName".to_string(), json!(domain));
        if !alternatives.is_empty() {
            body.insert(
                "certificateAlternativeNames".to_string(),
                json!(alternatives),
            );
        }
        Some(Value::Object(body))
    }

    /// Every certificate name this row's load balancer holds, in the order it
    /// got them.
    fn certificates(&self) -> Vec<&'static str> {
        let mut names = vec![self.created_with.0];
        if let Some((added, _, _)) = self.added {
            names.push(added);
        }
        names
    }

    /// The certificate this row attaches first, and the one it attaches second
    /// to check the replacement — the OTHER certificate when the row holds two,
    /// and the same one again when it holds one, which is the same request
    /// twice.
    fn attach_order(&self) -> (&'static str, &'static str) {
        let names = self.certificates();
        let first = names[self.attach_first % names.len()];
        let second = names
            .iter()
            .copied()
            .find(|name| *name != first)
            .unwrap_or(first);
        (first, second)
    }

    /// The `AttachLoadBalancerTlsCertificate` request for `certificate`.
    fn attach(&self, certificate: &str) -> Value {
        json!({
            "loadBalancerName": self.load_balancer,
            "certificateName": certificate,
        })
    }
}

/// Every row: attach one of a load balancer's certificates, check the load
/// balancer reports exactly that one attached and serves HTTPS, then attach the
/// other (or the same one again) and check the attachment moved.
#[tokio::test(flavor = "current_thread")]
async fn attach_load_balancer_tls_certificate_attaches_the_named_certificate() {
    let emulator = Emulator::new();
    for row in ROWS {
        let case = row.case;
        emulator
            .ok(row.region, "CreateLoadBalancer", row.create())
            .await;
        if let Some(add) = row.add() {
            emulator
                .ok(row.region, "CreateLoadBalancerTlsCertificate", add)
                .await;
        }
        if row.settle_first {
            wait_for_state(&emulator, row.region, row.load_balancer, "active").await;
        }

        // Nothing is attached until this operation attaches it: the load
        // balancer serves the plain HTTP listener alone.
        let before = emulator.read(row.region, row.load_balancer).await;
        assert_eq!(
            summaries(&before),
            row.certificates()
                .iter()
                .map(|name| ((*name).to_string(), false))
                .collect::<Vec<_>>(),
            "case {case}: expected every certificate unattached before the attach, got {:?}",
            before["tlsCertificateSummaries"]
        );
        assert_eq!(
            before["protocol"].as_str(),
            Some(PROTOCOL_HTTP),
            "case {case}: with no attached certificate the protocol must be HTTP, got {:?}",
            before["protocol"]
        );

        let (first, second) = row.attach_order();
        let answered = emulator
            .ok(
                row.region,
                "AttachLoadBalancerTlsCertificate",
                row.attach(first),
            )
            .await;
        assert_attach_operation(case, &answered, first, row.region);

        // The attached certificate is the one the request named, and it is the
        // only attached one; the load balancer now terminates HTTPS, so it
        // reports both the protocol and the port for it.
        let after = emulator.read(row.region, row.load_balancer).await;
        assert_eq!(
            summaries(&after),
            row.certificates()
                .iter()
                .map(|name| ((*name).to_string(), *name == first))
                .collect::<Vec<_>>(),
            "case {case}: expected only '{first}' attached, got {:?}",
            after["tlsCertificateSummaries"]
        );
        assert_eq!(
            after["protocol"].as_str(),
            Some(PROTOCOL_HTTP_HTTPS),
            "case {case}: with an attached certificate the protocol must be HTTP_HTTPS, got {:?}",
            after["protocol"]
        );
        assert_eq!(
            public_ports(&after),
            vec![HTTP_PORT, HTTPS_PORT],
            "case {case}: with an attached certificate both public ports must be reported, \
             got {:?}",
            after["publicPorts"]
        );

        // Nothing else about the load balancer moved: this call attaches a
        // certificate, not a load balancer.
        for member in [
            "name",
            "arn",
            "supportCode",
            "createdAt",
            "dnsName",
            "instancePort",
            "healthCheckPath",
            "ipAddressType",
            "location",
            "configurationOptions",
            "instanceHealthSummary",
        ] {
            assert_eq!(
                after[member], before[member],
                "case {case}: attaching a certificate must leave '{member}' as it was, got {:?}",
                after[member]
            );
        }
        assert_eq!(
            after["instancePort"].as_i64(),
            Some(row.instance_port),
            "case {case}: expected the requested instance port, got {:?}",
            after["instancePort"]
        );

        // Attaching the next certificate replaces the attachment: a load
        // balancer terminates HTTPS with ONE certificate. Where the row holds a
        // single certificate, `second` IS `first` — the same request twice,
        // which asks for the state the record is already in.
        let answered = emulator
            .ok(
                row.region,
                "AttachLoadBalancerTlsCertificate",
                row.attach(second),
            )
            .await;
        assert_attach_operation(case, &answered, second, row.region);
        let replaced = emulator.read(row.region, row.load_balancer).await;
        assert_eq!(
            summaries(&replaced),
            row.certificates()
                .iter()
                .map(|name| ((*name).to_string(), *name == second))
                .collect::<Vec<_>>(),
            "case {case}: expected only '{second}' attached after attaching it, got {:?}",
            replaced["tlsCertificateSummaries"]
        );
        assert_eq!(
            replaced["protocol"].as_str(),
            Some(PROTOCOL_HTTP_HTTPS),
            "case {case}: one certificate is still attached, so the protocol stays HTTP_HTTPS, \
             got {:?}",
            replaced["protocol"]
        );
    }
}

// ---------------------------------------------------------------------------
// An attached certificate is in use
// ---------------------------------------------------------------------------

/// The certificate the attachment put in front of traffic is in use, so deleting
/// it needs `force` — and once it is forced out the load balancer drops back to
/// the plain HTTP listener.
#[tokio::test(flavor = "current_thread")]
async fn an_attached_certificate_is_in_use_by_the_load_balancer() {
    let emulator = Emulator::new();
    let region = "us-east-1";
    let load_balancer = "in-use-lb";
    let certificate = "in-use-cert";
    emulator
        .ok(
            region,
            "CreateLoadBalancer",
            json!({
                "loadBalancerName": load_balancer,
                "instancePort": HTTP_PORT,
                "certificateName": certificate,
                "certificateDomainName": "in-use.example.com",
            }),
        )
        .await;
    emulator
        .ok(
            region,
            "AttachLoadBalancerTlsCertificate",
            json!({"loadBalancerName": load_balancer, "certificateName": certificate}),
        )
        .await;

    let request = json!({
        "loadBalancerName": load_balancer,
        "certificateName": certificate,
    });
    let (status, body) = emulator
        .call(region, "DeleteLoadBalancerTlsCertificate", request.clone())
        .await;
    assert_eq!(
        status, 400,
        "case deleting an attached certificate without force: expected HTTP 400, \
         got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "OperationFailureException",
        "case deleting an attached certificate without force: expected the modelled \
         OperationFailureException, got {body:?}"
    );

    // The refused delete removed nothing: the certificate is still attached and
    // the load balancer still terminates HTTPS.
    let held = emulator.read(region, load_balancer).await;
    assert_eq!(
        summaries(&held),
        vec![(certificate.to_string(), true)],
        "case a refused delete: expected the certificate still attached, got {:?}",
        held["tlsCertificateSummaries"]
    );

    // Forced, the attached certificate goes, and with it the HTTPS listener it
    // was what gave the load balancer.
    let mut forced = request;
    forced["force"] = json!(true);
    emulator
        .ok(region, "DeleteLoadBalancerTlsCertificate", forced)
        .await;
    let after = emulator.read(region, load_balancer).await;
    assert!(
        summaries(&after).is_empty(),
        "case a forced delete: expected no certificate summary left, got {:?}",
        after["tlsCertificateSummaries"]
    );
    assert_eq!(
        after["protocol"].as_str(),
        Some(PROTOCOL_HTTP),
        "case a forced delete: with no attached certificate the protocol must be HTTP, got {:?}",
        after["protocol"]
    );
    assert_eq!(
        public_ports(&after),
        vec![HTTP_PORT],
        "case a forced delete: with no attached certificate the only public port must be 80, \
         got {:?}",
        after["publicPorts"]
    );
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// One `AttachLoadBalancerTlsCertificate` request that names something the
/// account does not hold, and which resource type the refusal must name.
#[derive(Debug, Clone, Copy)]
struct Refusal {
    case: &'static str,
    /// The Region the attach is addressed in.
    region: &'static str,
    load_balancer: &'static str,
    certificate: &'static str,
    /// The `ResourceType` the `DoesNotExist` message must name.
    names_type: &'static str,
    /// The name that message must carry.
    names: &'static str,
}

/// The load balancer every refusal row is checked against: one load balancer in
/// `us-east-1` holding one certificate.
const HELD_REGION: &str = "us-east-1";
const HELD_LOAD_BALANCER: &str = "held-lb";
const HELD_CERTIFICATE: &str = "held-cert";

const REFUSALS: &[Refusal] = &[
    Refusal {
        case: "a load balancer name the account does not hold",
        region: HELD_REGION,
        load_balancer: "absent-lb",
        certificate: HELD_CERTIFICATE,
        names_type: LOAD_BALANCER_RESOURCE_TYPE,
        names: "absent-lb",
    },
    Refusal {
        case: "a certificate name this load balancer does not hold",
        region: HELD_REGION,
        load_balancer: HELD_LOAD_BALANCER,
        certificate: "other-cert",
        names_type: CERTIFICATE_RESOURCE_TYPE,
        names: "other-cert",
    },
    Refusal {
        case: "a load balancer that holds no certificate at all",
        region: HELD_REGION,
        load_balancer: "bare-lb",
        certificate: HELD_CERTIFICATE,
        names_type: CERTIFICATE_RESOURCE_TYPE,
        names: HELD_CERTIFICATE,
    },
    Refusal {
        case: "the right names, in a Region the load balancer does not live in",
        region: "eu-central-1",
        load_balancer: HELD_LOAD_BALANCER,
        certificate: HELD_CERTIFICATE,
        names_type: LOAD_BALANCER_RESOURCE_TYPE,
        names: HELD_LOAD_BALANCER,
    },
];

/// An attach that names a load balancer, or a certificate of one, the account
/// does not hold attaches nothing.
#[tokio::test(flavor = "current_thread")]
async fn attach_load_balancer_tls_certificate_refuses_what_the_account_does_not_hold() {
    let emulator = Emulator::new();
    emulator
        .ok(
            HELD_REGION,
            "CreateLoadBalancer",
            json!({
                "loadBalancerName": HELD_LOAD_BALANCER,
                "instancePort": HTTP_PORT,
                "certificateName": HELD_CERTIFICATE,
                "certificateDomainName": "held.example.com",
            }),
        )
        .await;
    // A load balancer with no certificate members at all, so an attach naming
    // one of its certificates names nothing.
    emulator
        .ok(
            HELD_REGION,
            "CreateLoadBalancer",
            json!({"loadBalancerName": "bare-lb", "instancePort": HTTP_PORT}),
        )
        .await;

    for refusal in REFUSALS {
        let case = refusal.case;
        let (status, body) = emulator
            .call(
                refusal.region,
                "AttachLoadBalancerTlsCertificate",
                json!({
                    "loadBalancerName": refusal.load_balancer,
                    "certificateName": refusal.certificate,
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: Lightsail answers a missing resource with 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {case}: expected the modelled NotFoundException, got {body:?}"
        );
        assert_eq!(
            body["code"].as_str(),
            Some("DoesNotExist"),
            "case {case}: expected the DoesNotExist body code, got {body:?}"
        );
        let message = body["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(refusal.names_type) && message.contains(refusal.names),
            "case {case}: expected a message naming the {} '{}', got {message:?}",
            refusal.names_type,
            refusal.names
        );

        // Every refused attach attaches nothing: the held certificate is still
        // unattached and its load balancer still serves plain HTTP.
        let held = emulator.read(HELD_REGION, HELD_LOAD_BALANCER).await;
        assert_eq!(
            summaries(&held),
            vec![(HELD_CERTIFICATE.to_string(), false)],
            "case {case}: a refused attach must attach nothing, got {:?}",
            held["tlsCertificateSummaries"]
        );
        assert_eq!(
            held["protocol"].as_str(),
            Some(PROTOCOL_HTTP),
            "case {case}: a refused attach must leave the protocol at HTTP, got {:?}",
            held["protocol"]
        );
    }
}

/// A certificate is addressed per load balancer: attaching one load balancer's
/// certificate leaves the same certificate name on another load balancer
/// unattached, in the same Region and in another.
#[tokio::test(flavor = "current_thread")]
async fn a_certificate_is_attached_to_one_load_balancer_only() {
    let emulator = Emulator::new();
    // The same certificate name on three load balancers: two in one Region, one
    // in another.
    let placements = [
        ("us-east-1", "first-lb"),
        ("us-east-1", "second-lb"),
        ("eu-west-1", "first-lb"),
    ];
    for (region, load_balancer) in placements {
        emulator
            .ok(
                region,
                "CreateLoadBalancer",
                json!({
                    "loadBalancerName": load_balancer,
                    "instancePort": HTTP_PORT,
                    "certificateName": "shared-name-cert",
                    "certificateDomainName": "shared.example.com",
                }),
            )
            .await;
    }

    emulator
        .ok(
            "us-east-1",
            "AttachLoadBalancerTlsCertificate",
            json!({"loadBalancerName": "first-lb", "certificateName": "shared-name-cert"}),
        )
        .await;

    for (region, load_balancer) in placements {
        let balancer = emulator.read(region, load_balancer).await;
        let case = format!("{load_balancer} in {region}");
        let attached = (region, load_balancer) == ("us-east-1", "first-lb");
        assert_eq!(
            summaries(&balancer),
            vec![("shared-name-cert".to_string(), attached)],
            "case {case}: expected isAttached {attached} after attaching only \
             us-east-1/first-lb's certificate, got {:?}",
            balancer["tlsCertificateSummaries"]
        );
        assert_eq!(
            balancer["protocol"].as_str(),
            Some(if attached {
                PROTOCOL_HTTP_HTTPS
            } else {
                PROTOCOL_HTTP
            }),
            "case {case}: expected the protocol of a load balancer with attachment \
             {attached}, got {:?}",
            balancer["protocol"]
        );
    }
}
