// SPDX-License-Identifier: CC0-1.0

//! `DeleteLoadBalancerTlsCertificate` — removing an SSL/TLS certificate from a
//! Lightsail load balancer — driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: a `DeleteLoadBalancerTlsCertificate` call removes the ONE
//! certificate its `certificateName` names from the load balancer its
//! `loadBalancerName` names in the Region the request is made in, leaving every
//! other member of that load balancer exactly as it was, and answers one
//! terminal operation-log entry for the certificate it removed; a request naming
//! a load balancer, or a certificate of that load balancer, the account does not
//! hold removes nothing and is answered with Lightsail's `DoesNotExist`
//! rejection.
//!
//! The rows differ along everything the rule generalises over: the Region the
//! load balancer lives in, the certificate's name and domain, how many
//! alternative domain names it covers, the instance port the load balancer
//! forwards to, whether the load balancer has settled into `active` before the
//! delete, and whether `force` is sent as `true`, as `false` or not at all —
//! which, for a certificate nothing has attached, is the same request three
//! ways.
//!
//! One branch of the handler is deliberately not exercised here: a certificate
//! that is ATTACHED is refused without `force`, and nothing reachable over the
//! wire attaches one yet — `AttachLoadBalancerTlsCertificate` is still a
//! scaffold stub answering 501, and `CreateLoadBalancer` stores the certificate
//! it was asked for unattached on purpose. When that operation lands, its own
//! suite is where the `force` refusal and the forced deletion of an attached
//! certificate get pinned.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The UUID shape check this crate's suites share, rather than an Nth copy of it.
use crate::instance_lifecycle::is_uuid_shaped;

/// The `X-Amz-Target` prefix this service routes on (`OPERATIONS.md`, "Wire
/// protocol").
const TARGET_PREFIX: &str = "Lightsail_20161128";

/// The public port a load balancer with no attached certificate listens on, from
/// the modelled `LoadBalancer$publicPorts` documentation ("For HTTP, use port
/// 80").
const HTTP_PORT: i64 = 80;

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

/// The certificate names a load balancer reports a summary for, in report order.
fn summary_names(balancer: &Value) -> Vec<String> {
    balancer["tlsCertificateSummaries"]
        .as_array()
        .map(|summaries| {
            summaries
                .iter()
                .map(|summary| summary["name"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// The delete table
// ---------------------------------------------------------------------------

/// One load balancer that holds one certificate, and the delete that removes it.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    region: &'static str,
    load_balancer: &'static str,
    certificate: &'static str,
    domain: &'static str,
    /// The alternative domains the certificate also covers — up to the 9 the
    /// modelled `certificateAlternativeNames` documentation admits ("You can
    /// have a maximum of 9 alternative names").
    alternative_names: &'static [&'static str],
    instance_port: i64,
    /// `force` as sent, or `None` to send none at all.
    force: Option<bool>,
    /// Whether to let the clock settle the load balancer into `active` before
    /// deleting its certificate.
    settle_first: bool,
}

const ROWS: &[Row] = &[
    Row {
        case: "no alternative names, no force member, deleted while provisioning",
        region: "us-east-1",
        load_balancer: "shop-lb",
        certificate: "shop-cert",
        domain: "example.com",
        alternative_names: &[],
        instance_port: HTTP_PORT,
        force: None,
        settle_first: false,
    },
    Row {
        case: "one alternative name, force false, deleted once active",
        region: "eu-west-1",
        load_balancer: "eu-edge-lb",
        certificate: "eu-edge-cert",
        domain: "eu.example.com",
        alternative_names: &["www.eu.example.com"],
        instance_port: 8080,
        force: Some(false),
        settle_first: true,
    },
    Row {
        case: "three alternative names, force true on a certificate nothing attached",
        region: "ap-northeast-1",
        load_balancer: "apac-lb-01",
        certificate: "apac-cert-01",
        domain: "apac.example.com",
        alternative_names: &["www.apac.example.com", "shop.apac.example.com", "apac.example.net"],
        instance_port: 3000,
        force: Some(true),
        settle_first: false,
    },
    Row {
        case: "the highest modelled instance port, a digit-and-hyphen certificate name",
        region: "us-west-2",
        load_balancer: "west-lb-2",
        certificate: "west-cert-2026-01",
        domain: "west.example.org",
        alternative_names: &["cdn.west.example.org"],
        instance_port: 65535,
        force: None,
        settle_first: true,
    },
    Row {
        case: "the nine alternative names the documentation admits",
        region: "us-east-2",
        load_balancer: "wide-lb",
        certificate: "wide_cert",
        domain: "wide.example.com",
        alternative_names: &[
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
        instance_port: 1,
        force: Some(false),
        settle_first: false,
    },
];

impl Row {
    /// The `CreateLoadBalancer` request that puts this row's certificate on this
    /// row's load balancer.
    fn create(&self) -> Value {
        let mut body = serde_json::Map::new();
        body.insert("loadBalancerName".to_string(), json!(self.load_balancer));
        body.insert("instancePort".to_string(), json!(self.instance_port));
        body.insert("certificateName".to_string(), json!(self.certificate));
        body.insert("certificateDomainName".to_string(), json!(self.domain));
        if !self.alternative_names.is_empty() {
            body.insert(
                "certificateAlternativeNames".to_string(),
                json!(self.alternative_names),
            );
        }
        Value::Object(body)
    }

    /// The `DeleteLoadBalancerTlsCertificate` request this row sends.
    fn delete(&self) -> Value {
        let mut body = serde_json::Map::new();
        body.insert("loadBalancerName".to_string(), json!(self.load_balancer));
        body.insert("certificateName".to_string(), json!(self.certificate));
        if let Some(force) = self.force {
            body.insert("force".to_string(), json!(force));
        }
        Value::Object(body)
    }
}

/// Every row: create a load balancer holding one certificate, delete that
/// certificate, and check the load balancer reports it gone while everything
/// else about the record stands — then that deleting it again is refused.
#[tokio::test(flavor = "current_thread")]
async fn delete_load_balancer_tls_certificate_removes_the_named_certificate() {
    let emulator = Emulator::new();
    for row in ROWS {
        let case = row.case;
        emulator
            .ok(row.region, "CreateLoadBalancer", row.create())
            .await;
        if row.settle_first {
            wait_for_state(&emulator, row.region, row.load_balancer, "active").await;
        }

        // The certificate the create requested is there to be deleted.
        let before = emulator.read(row.region, row.load_balancer).await;
        assert_eq!(
            summary_names(&before),
            vec![row.certificate.to_string()],
            "case {case}: expected the created certificate before the delete, got {:?}",
            before["tlsCertificateSummaries"]
        );

        let deleted = emulator
            .ok(row.region, "DeleteLoadBalancerTlsCertificate", row.delete())
            .await;

        // One certificate removed, so one operation-log entry — terminal,
        // because the certificate is gone as the call returns.
        let operations = deleted["operations"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected an operations array, got {deleted:?}"));
        assert_eq!(
            operations.len(),
            1,
            "case {case}: one certificate was removed, so one operation is answered, \
             got {operations:?}"
        );
        let operation = &operations[0];
        assert_eq!(
            operation["operationType"].as_str(),
            Some("DeleteLoadBalancerTlsCertificate"),
            "case {case}: expected the DeleteLoadBalancerTlsCertificate operation type, \
             got {operation:?}"
        );
        assert_eq!(
            operation["resourceType"].as_str(),
            Some(CERTIFICATE_RESOURCE_TYPE),
            "case {case}: expected the certificate resource type, got {operation:?}"
        );
        assert_eq!(
            operation["resourceName"].as_str(),
            Some(row.certificate),
            "case {case}: expected the deleted certificate's name, got {operation:?}"
        );
        assert_eq!(
            operation["status"].as_str(),
            Some("Succeeded"),
            "case {case}: the deletion is done, so the operation succeeded, got {operation:?}"
        );
        assert_eq!(
            operation["isTerminal"].as_bool(),
            Some(true),
            "case {case}: the deletion is done, so the operation is terminal, got {operation:?}"
        );
        assert_eq!(
            operation["location"]["regionName"].as_str(),
            Some(row.region),
            "case {case}: expected the Region the load balancer lives in, got {operation:?}"
        );
        let operation_id = operation["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: expected an operation id, got {operation:?}"));
        assert!(
            is_uuid_shaped(operation_id),
            "case {case}: expected a UUID-shaped operation id, got {operation_id:?}"
        );

        // The load balancer reports no certificate at all, and keeps serving the
        // plain HTTP listener it had.
        let after = emulator.read(row.region, row.load_balancer).await;
        assert!(
            summary_names(&after).is_empty(),
            "case {case}: the deleted certificate must leave no summary behind, got {:?}",
            after["tlsCertificateSummaries"]
        );
        assert_eq!(
            after["protocol"].as_str(),
            Some("HTTP"),
            "case {case}: with no attached certificate the protocol must be HTTP, got {:?}",
            after["protocol"]
        );
        assert_eq!(
            after["publicPorts"]
                .as_array()
                .map(|ports| ports.iter().filter_map(Value::as_i64).collect::<Vec<_>>()),
            Some(vec![HTTP_PORT]),
            "case {case}: with no attached certificate the only public port must be 80, got {:?}",
            after["publicPorts"]
        );

        // Nothing else about the load balancer moved: this call deletes a
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
                "case {case}: deleting a certificate must leave '{member}' as it was, got {:?}",
                after[member]
            );
        }
        assert_eq!(
            after["instancePort"].as_i64(),
            Some(row.instance_port),
            "case {case}: expected the requested instance port, got {:?}",
            after["instancePort"]
        );

        // Deleting it again names a certificate the load balancer no longer
        // holds, so it is refused rather than reporting a deletion that did not
        // happen.
        let (status, body) = emulator
            .call(row.region, "DeleteLoadBalancerTlsCertificate", row.delete())
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
            message.contains(CERTIFICATE_RESOURCE_TYPE) && message.contains(row.certificate),
            "case {case}: expected a message naming the certificate type and name, \
             got {message:?}"
        );

        // And the load balancer itself survives the refused second delete.
        let survivor = emulator.read(row.region, row.load_balancer).await;
        assert_eq!(
            survivor["arn"], before["arn"],
            "case {case}: the load balancer must outlive its certificate, got {survivor:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// One `DeleteLoadBalancerTlsCertificate` request that names something the
/// account does not hold, and which resource type the refusal must name.
#[derive(Debug, Clone, Copy)]
struct Refusal {
    case: &'static str,
    /// The Region the delete is addressed in.
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

/// A delete that names a load balancer, or a certificate of one, the account
/// does not hold removes nothing.
#[tokio::test(flavor = "current_thread")]
async fn delete_load_balancer_tls_certificate_refuses_what_the_account_does_not_hold() {
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
    // A load balancer with no certificate members at all, so a delete naming one
    // of its certificates names nothing.
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
                "DeleteLoadBalancerTlsCertificate",
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

        // Every refused delete removes nothing: the held certificate is still
        // on the load balancer that holds it.
        let held = emulator.read(HELD_REGION, HELD_LOAD_BALANCER).await;
        assert_eq!(
            summary_names(&held),
            vec![HELD_CERTIFICATE.to_string()],
            "case {case}: a refused delete must remove nothing, got {:?}",
            held["tlsCertificateSummaries"]
        );
    }
}

/// A certificate is addressed per load balancer: deleting one load balancer's
/// certificate leaves the same certificate name on another load balancer alone,
/// in the same Region and in another.
#[tokio::test(flavor = "current_thread")]
async fn a_certificate_is_deleted_from_one_load_balancer_only() {
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
            "DeleteLoadBalancerTlsCertificate",
            json!({"loadBalancerName": "first-lb", "certificateName": "shared-name-cert"}),
        )
        .await;

    for (region, load_balancer) in placements {
        let balancer = emulator.read(region, load_balancer).await;
        let case = format!("{load_balancer} in {region}");
        let expected: Vec<String> = if (region, load_balancer) == ("us-east-1", "first-lb") {
            Vec::new()
        } else {
            vec!["shared-name-cert".to_string()]
        };
        assert_eq!(
            summary_names(&balancer),
            expected,
            "case {case}: expected {expected:?} after deleting only us-east-1/first-lb's \
             certificate, got {:?}",
            balancer["tlsCertificateSummaries"]
        );
    }
}
