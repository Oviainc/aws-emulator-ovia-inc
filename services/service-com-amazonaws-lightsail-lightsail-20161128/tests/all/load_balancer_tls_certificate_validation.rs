// SPDX-License-Identifier: CC0-1.0

//! `AttachLoadBalancerTlsCertificate` takes only a certificate the certificate
//! authority has ISSUED — driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: a certificate the certificate authority has not issued
//! secures nothing, so no operation may put one in front of a resource's HTTPS
//! traffic — the request is refused before anything is written, and the same
//! request is served once the certificate is issued. For a load balancer's
//! SSL/TLS certificate that is "Once you create and validate your certificate,
//! you can attach it to your load balancer" (the modelled
//! `AttachLoadBalancerTlsCertificate` documentation), so an attach naming a
//! certificate the load balancer holds but Lightsail is still validating is
//! answered with Lightsail's `OperationFailureException` and leaves every
//! certificate of that load balancer — and the listeners it serves — exactly as
//! they were. A name the load balancer holds no certificate under is still the
//! `DoesNotExist` rejection: the name is resolved first.
//!
//! The rows differ along everything the rule generalises over: the Region the
//! load balancer lives in, whether `CreateLoadBalancer` or a later
//! `CreateLoadBalancerTlsCertificate` minted the certificate, how many
//! certificates the load balancer holds and which of them is attached, how many
//! domains the certificate covers, the certificate's name form, and whether an
//! already issued certificate is attached when the refused attach arrives.

use serde_json::{json, Value};

// The harness and the wire error code this test target already exposes, and the
// ONE read and the ONE issuance wait for a load balancer's SSL/TLS certificate,
// which the suite for the operation that reports them owns: a sibling suite
// drives the service through the same front door rather than standing up a
// second one.
use crate::instance_lifecycle::{error_code, Emulator};
use crate::load_balancer_tls_certificate_read::{certificate, wait_for_issued};

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

/// The `LoadBalancerTlsCertificateStatus` values this suite spans, from the
/// modelled enum: the status a certificate this account just asked for carries
/// while Lightsail validates its domains, and the status it carries once the
/// certificate authority has issued it.
const STATUS_PENDING_VALIDATION: &str = "PENDING_VALIDATION";
/// See [`STATUS_PENDING_VALIDATION`].
const STATUS_ISSUED: &str = "ISSUED";

/// The `ResourceType` member an SSL/TLS certificate of a load balancer reports,
/// from the modelled `ResourceType` enum.
const CERTIFICATE_RESOURCE_TYPE: &str = "LoadBalancerTlsCertificate";

/// The stored load balancer `name` in `region`, as `GetLoadBalancer` reports it.
async fn load_balancer(emulator: &Emulator, region: &str, name: &str) -> Value {
    emulator
        .ok_at(
            None,
            Some(region),
            "GetLoadBalancer",
            json!({"loadBalancerName": name}),
        )
        .await["loadBalancer"]
        .clone()
}

/// Attach `certificate` to the load balancer `name` in `region`, answering the
/// HTTP status and the parsed body — the call under test, served or refused.
async fn attach(
    emulator: &Emulator,
    region: &str,
    name: &str,
    certificate: &str,
) -> (u16, Value) {
    emulator
        .call_in(
            Some(region),
            "AttachLoadBalancerTlsCertificate",
            json!({"loadBalancerName": name, "certificateName": certificate}),
        )
        .await
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

// ---------------------------------------------------------------------------
// The table
// ---------------------------------------------------------------------------

/// A certificate a row asks for: its name, the domain it is for and the
/// alternative domains it also covers.
type Certificate = (&'static str, &'static str, &'static [&'static str]);

/// One load balancer, the certificates it holds, and which of them the row
/// attaches while Lightsail is still validating it.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    region: &'static str,
    load_balancer: &'static str,
    /// The certificate `CreateLoadBalancer` is asked for, when the row asks it
    /// for one — a load balancer may be created with no certificate members at
    /// all.
    created_with: Option<Certificate>,
    /// A certificate added later by `CreateLoadBalancerTlsCertificate` — up to
    /// the 2 the modelled `certificateName` documentation admits per load
    /// balancer ("Each Lightsail load balancer can have up to 2 certificates
    /// associated with it at one time").
    added: Option<Certificate>,
    /// WHICH of the certificates above the row attaches while it is still
    /// `PENDING_VALIDATION`: `0` is the first the load balancer got.
    pending_target: usize,
    /// Whether the row first waits for the OTHER certificate to be issued and
    /// attaches it, so the refused attach has an attachment it must leave in
    /// place.
    attach_issued_first: bool,
}

const ROWS: &[Row] = &[
    Row {
        case: "the certificate CreateLoadBalancer asked for, attached in the next call",
        region: "us-east-1",
        load_balancer: "pending-lb",
        created_with: Some(("pending-cert", "pending.example.com", &[])),
        added: None,
        pending_target: 0,
        attach_issued_first: false,
    },
    Row {
        case: "the only certificate, added by CreateLoadBalancerTlsCertificate, two \
               alternative domains",
        region: "eu-west-1",
        load_balancer: "eu-pending-lb",
        created_with: None,
        added: Some((
            "eu_pending_cert",
            "eu.example.com",
            &["www.eu.example.com", "shop.eu.example.com"],
        )),
        pending_target: 0,
        attach_issued_first: false,
    },
    Row {
        case: "the second certificate of two, while an issued one is already attached",
        region: "ap-northeast-1",
        load_balancer: "apac-pending-lb",
        created_with: Some(("apac-cert-01", "apac.example.com", &[])),
        added: Some((
            "apac-cert-02",
            "apac2.example.com",
            &["www.apac2.example.com"],
        )),
        pending_target: 1,
        attach_issued_first: true,
    },
    Row {
        case: "the nine alternative domains the documentation admits, an underscore name",
        region: "us-west-2",
        load_balancer: "west-pending-lb",
        created_with: Some((
            "west_pending_cert",
            "west.example.org",
            &[
                "a.west.example.org",
                "b.west.example.org",
                "c.west.example.org",
                "d.west.example.org",
                "e.west.example.org",
                "f.west.example.org",
                "g.west.example.org",
                "h.west.example.org",
                "i.west.example.org",
            ],
        )),
        added: None,
        pending_target: 0,
        attach_issued_first: false,
    },
];

impl Row {
    /// The `CreateLoadBalancer` request this row's load balancer is created by.
    fn create(&self) -> Value {
        let mut body = serde_json::Map::new();
        body.insert("loadBalancerName".to_string(), json!(self.load_balancer));
        body.insert("instancePort".to_string(), json!(HTTP_PORT));
        if let Some((certificate, domain, alternatives)) = self.created_with {
            body.insert("certificateName".to_string(), json!(certificate));
            body.insert("certificateDomainName".to_string(), json!(domain));
            if !alternatives.is_empty() {
                body.insert(
                    "certificateAlternativeNames".to_string(),
                    json!(alternatives),
                );
            }
        }
        Value::Object(body)
    }

    /// The `CreateLoadBalancerTlsCertificate` request that adds this row's later
    /// certificate, when it has one.
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
        self.created_with
            .into_iter()
            .chain(self.added)
            .map(|(name, _, _)| name)
            .collect()
    }

    /// The certificate this row attaches while it is still being validated.
    fn target(&self) -> &'static str {
        let names = self.certificates();
        names[self.pending_target % names.len()]
    }

    /// The certificate this row attaches BEFORE the refused one, when it asks
    /// for an attachment the refusal must leave in place.
    fn already_attached(&self) -> Option<&'static str> {
        if !self.attach_issued_first {
            return None;
        }
        let target = self.target();
        self.certificates()
            .into_iter()
            .find(|name| *name != target)
    }
}

/// Every row: attaching a certificate the certificate authority has not issued
/// is refused and changes nothing, and the SAME request is served once the
/// certificate is issued.
#[tokio::test(flavor = "current_thread")]
async fn attach_load_balancer_tls_certificate_refuses_a_certificate_still_being_validated() {
    let emulator = Emulator::new();
    for row in ROWS {
        let case = row.case;
        let region = row.region;
        let name = row.load_balancer;
        emulator
            .ok_at(None, Some(region), "CreateLoadBalancer", row.create())
            .await;

        // The attachment the refusal must leave in place, where the row asks
        // for one: a certificate the clock has already issued.
        if let Some(attached) = row.already_attached() {
            wait_for_issued(&emulator, region, name, attached).await;
            let (status, body) = attach(&emulator, region, name, attached).await;
            assert_eq!(
                status, 200,
                "case {case}: attaching the issued certificate '{attached}' must be served, \
                 got {status} {body:?}"
            );
        }

        // The certificate this row adds later is minted by THIS call, so it is
        // still being validated when the attach below names it.
        if let Some(add) = row.add() {
            emulator
                .ok_at(
                    None,
                    Some(region),
                    "CreateLoadBalancerTlsCertificate",
                    add,
                )
                .await;
        }

        let target = row.target();
        let pending = certificate(&emulator, region, name, target).await;
        assert_eq!(
            pending["status"].as_str(),
            Some(STATUS_PENDING_VALIDATION),
            "case {case}: the certificate must still be being validated before the refused \
             attach, got {:?}",
            pending["status"]
        );

        let before = load_balancer(&emulator, region, name).await;
        let (status, body) = attach(&emulator, region, name, target).await;
        assert_eq!(
            status, 400,
            "case {case}: attaching a certificate that is {STATUS_PENDING_VALIDATION} must be \
             refused with HTTP 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "OperationFailureException",
            "case {case}: expected the modelled OperationFailureException, got {body:?}"
        );
        let message = body["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(target) && message.contains(STATUS_PENDING_VALIDATION),
            "case {case}: expected a message naming '{target}' and the status it is in, \
             got {message:?}"
        );

        // The refused attach wrote nothing: every certificate of the load
        // balancer is where it was, and so are the listeners it serves.
        let refused = load_balancer(&emulator, region, name).await;
        assert_eq!(
            summaries(&refused),
            summaries(&before),
            "case {case}: a refused attach must leave every certificate summary as it was, \
             got {:?}",
            refused["tlsCertificateSummaries"]
        );
        assert_eq!(
            summaries(&refused)
                .into_iter()
                .filter(|(_, attached)| *attached)
                .map(|(name, _)| name)
                .collect::<Vec<String>>(),
            row.already_attached()
                .map(|name| vec![name.to_string()])
                .unwrap_or_default(),
            "case {case}: expected only the certificate attached before the refusal to be \
             attached, got {:?}",
            refused["tlsCertificateSummaries"]
        );
        assert_eq!(
            refused["protocol"], before["protocol"],
            "case {case}: a refused attach must leave the protocol as it was, got {:?}",
            refused["protocol"]
        );
        assert_eq!(
            public_ports(&refused),
            public_ports(&before),
            "case {case}: a refused attach must leave the public ports as they were, got {:?}",
            refused["publicPorts"]
        );
        let still_pending = certificate(&emulator, region, name, target).await;
        assert_eq!(
            still_pending["isAttached"].as_bool(),
            Some(false),
            "case {case}: the refused certificate must not be reported attached, \
             got {still_pending:?}"
        );

        // The same request, once the certificate authority has issued the
        // certificate: served, and the load balancer terminates HTTPS with it.
        wait_for_issued(&emulator, region, name, target).await;
        let (status, answered) = attach(&emulator, region, name, target).await;
        assert_eq!(
            status, 200,
            "case {case}: once issued, the same attach must be served, got {status} {answered:?}"
        );
        let operations = answered["operations"].as_array().unwrap_or_else(|| {
            panic!("case {case}: expected an operations array, got {answered:?}")
        });
        assert_eq!(
            operations.len(),
            1,
            "case {case}: one certificate was attached, so one operation is answered, \
             got {operations:?}"
        );
        assert_eq!(
            operations[0]["resourceType"].as_str(),
            Some(CERTIFICATE_RESOURCE_TYPE),
            "case {case}: expected the certificate resource type, got {:?}",
            operations[0]
        );
        assert_eq!(
            operations[0]["resourceName"].as_str(),
            Some(target),
            "case {case}: expected the attached certificate's name, got {:?}",
            operations[0]
        );

        let attached = load_balancer(&emulator, region, name).await;
        assert_eq!(
            summaries(&attached),
            row.certificates()
                .iter()
                .map(|name| ((*name).to_string(), *name == target))
                .collect::<Vec<(String, bool)>>(),
            "case {case}: expected only the issued '{target}' attached, got {:?}",
            attached["tlsCertificateSummaries"]
        );
        assert_eq!(
            attached["protocol"].as_str(),
            Some(PROTOCOL_HTTP_HTTPS),
            "case {case}: with an attached certificate the protocol must be HTTP_HTTPS, got {:?}",
            attached["protocol"]
        );
        assert_eq!(
            public_ports(&attached),
            vec![HTTP_PORT, HTTPS_PORT],
            "case {case}: with an attached certificate both public ports must be reported, \
             got {:?}",
            attached["publicPorts"]
        );
        let issued = certificate(&emulator, region, name, target).await;
        assert_eq!(
            (issued["status"].as_str(), issued["isAttached"].as_bool()),
            (Some(STATUS_ISSUED), Some(true)),
            "case {case}: the attached certificate must be reported issued AND attached, \
             got {issued:?}"
        );
    }
}

/// A name the load balancer holds no certificate under is resolved FIRST, so it
/// is still Lightsail's `DoesNotExist` rejection while the certificate it does
/// hold is being validated — and neither attach attaches anything.
#[tokio::test(flavor = "current_thread")]
async fn a_name_the_load_balancer_does_not_hold_is_refused_before_the_validation_rule() {
    let emulator = Emulator::new();
    let region = "us-east-2";
    let name = "absent-name-lb";
    let held = "held-pending-cert";
    emulator
        .ok_at(
            None,
            Some(region),
            "CreateLoadBalancer",
            json!({
                "loadBalancerName": name,
                "instancePort": HTTP_PORT,
                "certificateName": held,
                "certificateDomainName": "held.example.com",
            }),
        )
        .await;

    let (status, body) = attach(&emulator, region, name, "other-cert").await;
    assert_eq!(
        status, 400,
        "case a certificate name the load balancer does not hold: expected HTTP 400, \
         got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "NotFoundException",
        "case a certificate name the load balancer does not hold: expected the modelled \
         NotFoundException, got {body:?}"
    );
    assert_eq!(
        body["code"].as_str(),
        Some("DoesNotExist"),
        "case a certificate name the load balancer does not hold: expected the DoesNotExist \
         body code, got {body:?}"
    );

    let balancer = load_balancer(&emulator, region, name).await;
    assert_eq!(
        summaries(&balancer),
        vec![(held.to_string(), false)],
        "case a refused attach: the certificate being validated must stay unattached, got {:?}",
        balancer["tlsCertificateSummaries"]
    );
    assert_eq!(
        balancer["protocol"].as_str(),
        Some(PROTOCOL_HTTP),
        "case a refused attach: the load balancer must still serve plain HTTP, got {:?}",
        balancer["protocol"]
    );
}
