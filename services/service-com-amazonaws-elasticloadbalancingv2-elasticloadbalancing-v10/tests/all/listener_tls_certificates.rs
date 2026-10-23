// SPDX-License-Identifier: CC0-1.0

//! Two rules of this awsQuery API, driven over the wire exactly as a client drives
//! them: which actions the `2015-12-01` API has, and which listeners take a default
//! certificate.
//!
//! 1. Every action this model binds is DISPATCHABLE at the API version the model
//!    declares: it is served, or answered by the operation itself (one of its modelled
//!    errors, or the scaffold's 501 while it is unimplemented) — never `InvalidAction`,
//!    which is reserved for an action the service does not have.
//!    [`every_bound_action_dispatches_at_the_models_api_version`] walks the
//!    create/describe actions and one action this model does NOT bind, so the row that
//!    must answer `InvalidAction` and the rows that must not are asserted together.
//!
//! 2. A listener's default certificate belongs to the protocols the model brackets it
//!    for: `CreateListenerInput$Certificates` is documented "[HTTPS and TLS listeners]
//!    The default certificate for the listener. You must provide exactly one
//!    certificate." So a listener whose protocol terminates TLS is created with
//!    exactly one certificate and refused with none or with several, and a listener
//!    whose protocol does not — or which names no protocol at all — is refused when it
//!    carries one. [`the_default_certificate_is_bracketed_to_tls_terminating_protocols`]
//!    asserts that over the WHOLE `ProtocolEnum` catalog crossed with the certificate
//!    counts either side of "exactly one".
//!
//! Every request goes through the router as a client's would — awsQuery form body in,
//! `<{Operation}Response>` XML out — and every accepted row is read back through
//! `DescribeListeners` rather than trusted from the create's own answer.

use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// The region every request below is signed for.
const REGION: &str = "us-west-2";

/// The API version every awsQuery request carries, from this model's service version
/// (`smithy_query(query="service_info")`: `version: 2015-12-01`).
const VERSION: &str = "2015-12-01";

/// The awsQuery error code AWS reserves for an action the service does not have.
const INVALID_ACTION: &str = "InvalidAction";

/// The error code this service answers an input its own documented rules refuse with.
const VALIDATION_ERROR: &str = "ValidationError";

/// Every value of this model's `ProtocolEnum`, from
/// `src/ops/types/shared_types.rs`'s `PROTOCOL_ENUM_VALUES` (generated from
/// `smithy_query(query="shape",shape="ProtocolEnum")`). The table below crosses ALL of
/// them with the certificate counts, so a protocol added to the model shows up here as
/// a row that was never asserted.
const PROTOCOLS: &[&str] = &[
    "GENEVE", "HTTP", "HTTPS", "QUIC", "TCP", "TCP_QUIC", "TCP_UDP", "TLS", "UDP",
];

/// The protocols that terminate TLS at the load balancer, i.e. the ones the
/// `Certificates` documentation brackets ("[HTTPS and TLS listeners]").
const TLS_TERMINATING: &[&str] = &["HTTPS", "TLS"];

/// A certificate ARN to hand a listener. Any ARN does: this service does not resolve a
/// certificate against ACM, and the rule under test is about how MANY certificates a
/// protocol takes.
const CERT_ONE: &str = "arn:aws:acm:us-west-2:000000000000:certificate/11111111-1111-1111-1111-111111111111";

/// A second certificate ARN, for the "more than one" rows.
const CERT_TWO: &str = "arn:aws:acm:us-west-2:000000000000:certificate/22222222-2222-2222-2222-222222222222";

/// A router carrying only this service, over one shared engine.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::EmulatedService::new(
            engine,
        ),
    );
    router
}

/// One answer from the emulator.
struct Reply {
    status: u16,
    body: String,
}

impl Reply {
    /// The text of the first element named `tag`, or `None` when it is absent or
    /// empty.
    fn text(&self, tag: &str) -> Option<String> {
        let (inner, _) = element_span(&self.body, tag, 0)?;
        (!inner.is_empty()).then(|| inner.to_string())
    }

    /// The `<Code>` of an awsQuery error envelope, or the empty string on a success.
    fn error_code(&self) -> String {
        self.text("Code").unwrap_or_default()
    }
}

/// Percent-encode one form component, so an ARN crosses the wire intact.
///
/// Hand-written, as in this crate's sibling suite, so the change adds no dependency
/// and leaves `Cargo.lock` alone: escaping every non-alphanumeric byte is always a
/// valid form encoding.
fn encode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' => out.push(char::from(*byte)),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// One form field.
fn p(key: &str, value: &str) -> (String, String) {
    (key.to_string(), value.to_string())
}

/// The form fields of one awsQuery request.
type Form = Vec<(String, String)>;

/// One row of the dispatch table: the action's name, the request fields it carries and
/// the HTTP status the one operation that owns that action must answer.
type DispatchCase = (&'static str, Form, u16);

/// `Action` plus `Version`, the two routing fields every awsQuery request carries.
fn action(name: &str) -> Form {
    vec![p("Action", name), p("Version", VERSION)]
}

/// Send one awsQuery request: `pairs` are the form fields, `Action` included.
async fn call(router: &MultiServiceRouter, pairs: &[(String, String)]) -> Reply {
    let body = pairs
        .iter()
        .map(|(key, value)| format!("{}={}", encode(key), encode(value)))
        .collect::<Vec<_>>()
        .join("&");
    // Signed for this model's OWN routing key: the `elasticloadbalancing` signing name
    // both Elastic Load Balancing models declare is claimed by the Classic front door,
    // which serves this model for a `Version=2015-12-01` envelope. That shared-endpoint
    // half needs both services mounted and lives in
    // `core/router/tests/all/elb_classic_and_v2_signing_name_routing.rs`; this suite
    // mounts this service alone, so it addresses it directly.
    let request = http::Request::builder()
        .method("POST")
        .uri("https://elasticloadbalancingv2.us-west-2.amazonaws.com/")
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/elasticloadbalancingv2/aws4_request, SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8_lossy(response.body()).to_string();
    Reply { status, body }
}

/// The XML inside each `<member>` of the first element named `container`.
fn members(xml: &str, container: &str) -> Vec<String> {
    let Some((inner, _)) = element_span(xml, container, 0) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    let mut at = 0usize;
    while let Some((member, past)) = element_span(inner, "member", at) {
        found.push(member.to_string());
        at = past;
    }
    found
}

/// The XML inside the first element named `name` at or after `from`, and the offset
/// just past that element's closing tag.
///
/// The responses are the framework's own serializer output: elements, text and the
/// `xmlns` attribute it puts on the response root and on each `<member>` — no
/// comments, CDATA or processing instructions.
fn element_span<'a>(xml: &'a str, name: &str, from: usize) -> Option<(&'a str, usize)> {
    let (_, inner_start, self_closing) = find_tag(xml, name, from)?;
    if self_closing {
        return Some(("", inner_start));
    }
    let close = format!("</{name}>");
    let mut depth = 1usize;
    let mut cursor = inner_start;
    while depth > 0 {
        let next_close = xml[cursor..].find(&close).map(|at| cursor + at)?;
        match find_tag(xml, name, cursor) {
            Some((open_at, inner, false)) if open_at < next_close => {
                depth += 1;
                cursor = inner;
            }
            Some((open_at, past, true)) if open_at < next_close => {
                // `<name/>` opens and closes at once.
                cursor = past;
            }
            _ => {
                depth -= 1;
                if depth == 0 {
                    return Some((&xml[inner_start..next_close], next_close + close.len()));
                }
                cursor = next_close + close.len();
            }
        }
    }
    None
}

/// The offsets of the first opening tag named `name` at or after `from`: where the tag
/// starts, where its content starts, and whether it is self-closing.
fn find_tag(xml: &str, name: &str, from: usize) -> Option<(usize, usize, bool)> {
    let needle = format!("<{name}");
    let mut at = from;
    loop {
        let start = xml[at..].find(&needle).map(|found| at + found)?;
        let after = start + needle.len();
        let rest = &xml[after..];
        if rest.starts_with('>') {
            return Some((start, after + 1, false));
        }
        if rest.starts_with("/>") {
            return Some((start, after + 2, true));
        }
        if rest.starts_with(|c: char| c.is_ascii_whitespace()) {
            let end = rest.find('>').map(|found| found + after)?;
            return Some((start, end + 1, xml[..end].ends_with('/')));
        }
        // A longer element name that merely starts with `name`.
        at = after;
    }
}

/// Create a load balancer across two subnets and answer its ARN.
async fn create_load_balancer(router: &MultiServiceRouter, name: &str) -> String {
    let mut form = action("CreateLoadBalancer");
    form.push(p("Name", name));
    form.push(p("Subnets.member.1", "subnet-11111111"));
    form.push(p("Subnets.member.2", "subnet-22222222"));
    let reply = call(router, &form).await;
    assert_eq!(
        (reply.status, reply.error_code().as_str()),
        (200, "provisioning"),
        "case create {name}: expected a 200 whose new load balancer is provisioning, got {} {}",
        reply.status,
        reply.body
    );
    reply
        .text("LoadBalancerArn")
        .unwrap_or_else(|| panic!("case create {name}: no LoadBalancerArn in {}", reply.body))
}

/// Create an HTTP/80 target group and answer its ARN.
async fn create_target_group(router: &MultiServiceRouter, name: &str) -> String {
    let mut form = action("CreateTargetGroup");
    form.push(p("Name", name));
    form.push(p("Protocol", "HTTP"));
    form.push(p("Port", "80"));
    form.push(p("VpcId", "vpc-12345678"));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case create target group {name}: expected 200, got {} body {}",
        reply.status, reply.body
    );
    reply.text("TargetGroupArn").unwrap_or_else(|| {
        panic!(
            "case create target group {name}: no TargetGroupArn in {}",
            reply.body
        )
    })
}

/// One row of the certificate table: the case name, the listener's `Protocol` (or
/// `None` to send no protocol field), the certificate ARNs it carries, and whether the
/// create must be accepted.
struct CertificateCase {
    case: String,
    protocol: Option<&'static str>,
    certificates: Vec<&'static str>,
    accepted: bool,
}

/// What an accepted certificate row must read back as: the case it came from, the port
/// it was created on, the protocol it named and how many certificates it carried.
struct Accepted {
    case: String,
    port: i32,
    protocol: Option<&'static str>,
    certificates: usize,
}

/// Rule 1: an action this model binds is reachable at the version the model declares,
/// and `InvalidAction` is reserved for an action it does not bind.
///
/// This is the defect the issue reports — `CreateLoadBalancer` answered
/// `InvalidAction` at `2015-12-01`, so nothing downstream of a load balancer could be
/// built — asserted as the rule rather than as the one call: every row below names an
/// action BOUND to this service (`smithy_query(query="list_operations")`) and must be
/// served or refused with one of ITS OWN modelled errors, while the last row names an
/// action bound to the Classic Elastic Load Balancing model instead and must be the
/// `InvalidAction` refusal.
#[tokio::test]
async fn every_bound_action_dispatches_at_the_models_api_version() {
    let router = router();

    // Rows differ along the kind of action: a create that mints, a create that depends
    // on an earlier one, a reporting read, and a read with a filter. Each is run in
    // order over the one engine, so the later rows read what the earlier ones wrote.
    let lb_arn = create_load_balancer(&router, "dispatch-alb").await;
    let tg_arn = create_target_group(&router, "dispatch-tg").await;

    let mut listener = action("CreateListener");
    listener.push(p("LoadBalancerArn", &lb_arn));
    listener.push(p("Protocol", "HTTP"));
    listener.push(p("Port", "80"));
    listener.push(p("DefaultActions.member.1.Type", "forward"));
    listener.push(p("DefaultActions.member.1.TargetGroupArn", &tg_arn));

    let mut listeners_of = action("DescribeListeners");
    listeners_of.push(p("LoadBalancerArn", &lb_arn));

    let bound: Vec<DispatchCase> = vec![
        ("CreateListener", listener, 200),
        ("DescribeLoadBalancers", action("DescribeLoadBalancers"), 200),
        ("DescribeTargetGroups", action("DescribeTargetGroups"), 200),
        ("DescribeListeners", listeners_of, 200),
        // Bound and still a scaffold: the 501 `NotImplemented` channel is also a
        // DISPATCHED answer — the request reached the operation — and is what a stub
        // must answer until its own issue implements it. What it must never be is
        // `InvalidAction`, which would tell the caller this API has no such action.
        (
            "DescribeAccountLimits",
            action("DescribeAccountLimits"),
            501,
        ),
    ];

    for (case, form, expected_status) in bound {
        let reply = call(&router, &form).await;
        assert_eq!(
            reply.status, expected_status,
            "case {case}: an action this model binds must be served at {VERSION}, got {} {}",
            reply.status, reply.body
        );
        assert_ne!(
            reply.error_code(),
            INVALID_ACTION,
            "case {case}: {INVALID_ACTION} is reserved for an action the service does not have, got {}",
            reply.body
        );
    }

    // An action bound to the OTHER Elastic Load Balancing model (`2012-06-01`) and not
    // to this one: this is what `InvalidAction` is for, and the refusal names the action
    // it could not find. (That the SHARED `elasticloadbalancing` signing name resolves
    // the two models by the API version the envelope names, and names that version in
    // its own refusal, is pinned in
    // `core/router/tests/all/elb_classic_and_v2_signing_name_routing.rs`, which needs
    // both Elastic Load Balancing services mounted.)
    let mut classic_only = action("ConfigureHealthCheck");
    classic_only.push(p("LoadBalancerName", "dispatch-alb"));
    let reply = call(&router, &classic_only).await;
    assert_eq!(
        (reply.status, reply.error_code().as_str()),
        (400, INVALID_ACTION),
        "case an action this model does not bind: expected 400 {INVALID_ACTION}, got {} {}",
        reply.status,
        reply.body
    );
    assert!(
        reply.body.contains("ConfigureHealthCheck"),
        "case an action this model does not bind: the refusal must name the action it could not find, got {}",
        reply.body
    );
}

/// Rule 2: the default certificate is bracketed to the TLS-terminating protocols.
///
/// The table crosses the WHOLE `ProtocolEnum` catalog — plus a listener that names no
/// protocol, which a Gateway Load Balancer's listener is — with three certificate
/// counts: none, exactly one (what the documentation requires of an HTTPS or TLS
/// listener), and two. Each accepted row is read back through `DescribeListeners`, and
/// each refused row must leave nothing stored.
#[tokio::test]
async fn the_default_certificate_is_bracketed_to_tls_terminating_protocols() {
    let router = router();
    let lb_arn = create_load_balancer(&router, "cert-rule-alb").await;
    let tg_arn = create_target_group(&router, "cert-rule-tg").await;

    let mut cases: Vec<CertificateCase> = Vec::new();
    for protocol in PROTOCOLS {
        let terminates_tls = TLS_TERMINATING.contains(protocol);
        cases.push(CertificateCase {
            case: format!("{protocol} with no certificate"),
            protocol: Some(protocol),
            certificates: Vec::new(),
            accepted: !terminates_tls,
        });
        cases.push(CertificateCase {
            case: format!("{protocol} with exactly one certificate"),
            protocol: Some(protocol),
            certificates: vec![CERT_ONE],
            accepted: terminates_tls,
        });
        cases.push(CertificateCase {
            case: format!("{protocol} with two certificates"),
            protocol: Some(protocol),
            certificates: vec![CERT_ONE, CERT_TWO],
            accepted: false,
        });
    }
    // A listener that names no protocol at all — the model: "You can't specify a
    // protocol for a Gateway Load Balancer" — terminates no TLS either.
    cases.push(CertificateCase {
        case: "no protocol and no certificate".to_string(),
        protocol: None,
        certificates: Vec::new(),
        accepted: true,
    });
    cases.push(CertificateCase {
        case: "no protocol with one certificate".to_string(),
        protocol: None,
        certificates: vec![CERT_ONE],
        accepted: false,
    });

    // One port per row, so a refusal is never confused with a duplicate listener and an
    // accepted row never collides with the one before it.
    let mut port = 10_000;
    let mut accepted_rows: Vec<Accepted> = Vec::new();
    for case in &cases {
        port += 1;
        let mut form = action("CreateListener");
        form.push(p("LoadBalancerArn", &lb_arn));
        form.push(p("Port", &port.to_string()));
        if let Some(protocol) = case.protocol {
            form.push(p("Protocol", protocol));
        }
        for (index, arn) in case.certificates.iter().enumerate() {
            form.push(p(
                &format!("Certificates.member.{}.CertificateArn", index + 1),
                arn,
            ));
        }
        form.push(p("DefaultActions.member.1.Type", "forward"));
        form.push(p("DefaultActions.member.1.TargetGroupArn", &tg_arn));
        let reply = call(&router, &form).await;

        if case.accepted {
            assert_eq!(
                reply.status, 200,
                "case {:?}: expected the listener to be created, got {} {}",
                case.case, reply.status, reply.body
            );
            accepted_rows.push(Accepted {
                case: case.case.clone(),
                port,
                protocol: case.protocol,
                certificates: case.certificates.len(),
            });
        } else {
            assert_eq!(
                (reply.status, reply.error_code().as_str()),
                (400, VALIDATION_ERROR),
                "case {:?}: expected 400 {VALIDATION_ERROR}, got {} {}",
                case.case,
                reply.status,
                reply.body
            );
            if let Some(protocol) = case.protocol.filter(|_| case.certificates.is_empty()) {
                // The one wording of this family AWS itself publishes, built from the
                // protocol the request named rather than pasted from one request.
                assert_eq!(
                    reply.text("Message"),
                    Some(format!("A certificate must be specified for {protocol} listeners")),
                    "case {:?}: the refusal must say which listener needs a certificate, got {}",
                    case.case,
                    reply.body
                );
            }
        }
    }

    // A refusal stored nothing, and every accepted row reads back with the protocol and
    // the certificate it was created with.
    let mut form = action("DescribeListeners");
    form.push(p("LoadBalancerArn", &lb_arn));
    let described = call(&router, &form).await;
    assert_eq!(
        described.status, 200,
        "reading the listeners back must succeed, got {} {}",
        described.status, described.body
    );
    let stored = members(&described.body, "Listeners");
    assert_eq!(
        stored.len(),
        accepted_rows.len(),
        "expected exactly the accepted listeners to be stored, got {}",
        described.body
    );

    for row in &accepted_rows {
        let case = &row.case;
        let port = row.port;
        let listener = stored
            .iter()
            .find(|member| {
                element_span(member, "Port", 0).map(|(inner, _)| inner) == Some(&port.to_string())
            })
            .unwrap_or_else(|| {
                panic!("case {case:?}: no stored listener on port {port} in {stored:?}")
            });
        let reported_protocol = element_span(listener, "Protocol", 0)
            .and_then(|(inner, _)| (!inner.is_empty()).then(|| inner.to_string()));
        assert_eq!(
            reported_protocol.as_deref(),
            row.protocol,
            "case {case:?}: unexpected stored Protocol in {listener}"
        );
        assert_eq!(
            members(listener, "Certificates").len(),
            row.certificates,
            "case {case:?}: unexpected stored certificate count in {listener}"
        );
    }
}

/// The certificate must be NAMED, not merely present: an HTTPS listener whose one
/// certificate carries no `CertificateArn` identifies no certificate, so it is the same
/// refusal as sending none.
#[tokio::test]
async fn a_certificate_without_an_arn_names_no_certificate() {
    let router = router();
    let lb_arn = create_load_balancer(&router, "cert-arn-alb").await;
    let tg_arn = create_target_group(&router, "cert-arn-tg").await;

    // The rows differ in what the certificate entry does carry: nothing but
    // `IsDefault`, and an empty ARN.
    let rows: [(&str, Form); 2] = [
        (
            "an entry carrying only IsDefault",
            vec![p("Certificates.member.1.IsDefault", "true")],
        ),
        (
            "an entry carrying an empty CertificateArn",
            vec![p("Certificates.member.1.CertificateArn", "")],
        ),
    ];

    for (case, certificate) in rows {
        let mut form = action("CreateListener");
        form.push(p("LoadBalancerArn", &lb_arn));
        form.push(p("Protocol", "HTTPS"));
        form.push(p("Port", "443"));
        form.extend(certificate);
        form.push(p("DefaultActions.member.1.Type", "forward"));
        form.push(p("DefaultActions.member.1.TargetGroupArn", &tg_arn));
        let reply = call(&router, &form).await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (400, VALIDATION_ERROR),
            "case {case:?}: expected 400 {VALIDATION_ERROR}, got {} {}",
            reply.status,
            reply.body
        );
    }

    // And the same request WITH an ARN is accepted and reads back, so the rows above
    // are refused for the certificate and nothing else.
    let mut form = action("CreateListener");
    form.push(p("LoadBalancerArn", &lb_arn));
    form.push(p("Protocol", "HTTPS"));
    form.push(p("Port", "443"));
    form.push(p("Certificates.member.1.CertificateArn", CERT_ONE));
    form.push(p("DefaultActions.member.1.Type", "forward"));
    form.push(p("DefaultActions.member.1.TargetGroupArn", &tg_arn));
    let created = call(&router, &form).await;
    assert_eq!(
        created.status, 200,
        "case an HTTPS listener with one named certificate: expected 200, got {} {}",
        created.status, created.body
    );

    let mut form = action("DescribeListeners");
    form.push(p("LoadBalancerArn", &lb_arn));
    let described = call(&router, &form).await;
    let stored = members(&described.body, "Listeners");
    assert_eq!(
        stored.len(),
        1,
        "case an HTTPS listener with one named certificate: expected exactly it to be stored, got {}",
        described.body
    );
    assert!(
        stored[0].contains(CERT_ONE),
        "case an HTTPS listener with one named certificate: the stored listener must report it, got {}",
        stored[0]
    );
}
