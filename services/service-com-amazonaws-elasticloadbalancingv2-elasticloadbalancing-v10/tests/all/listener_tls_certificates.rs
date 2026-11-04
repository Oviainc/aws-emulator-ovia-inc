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

// The wire plumbing — the emulator under test, the request builder and the XML envelope
// reader — is `crate::wire`, shared with this crate's other suites rather than copied per
// suite. `VERSION` there is this model's own API version, read from the crate's
// `service::shared_endpoint` rather than restated.
use crate::wire::{action, call, first_text, members, p, router, VERSION};

/// The awsQuery error code AWS reserves for an action the service does not have.
const INVALID_ACTION: &str = "InvalidAction";

/// The error code this service answers an input its own documented rules refuse with.
const VALIDATION_ERROR: &str = "ValidationError";

/// The awsQuery code of `TooManyCertificatesException`, which `CreateListener` models
/// (`smithy_query(query="operation",operation="CreateListener")`).
const TOO_MANY_CERTIFICATES: &str = "TooManyCertificates";

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

/// A certificate ARN to hand a listener. Any Certificate Manager certificate ARN does:
/// this service resolves the ARN FORM of a requested certificate (pinned in
/// `all/certificate_resolution_and_redirect_protocol.rs`) and not the certificate itself
/// against ACM, and the rule under test is about how MANY certificates a protocol takes.
const CERT_ONE: &str = "arn:aws:acm:us-west-2:000000000000:certificate/11111111-1111-1111-1111-111111111111";

/// A second certificate ARN, for the "more than one" rows.
const CERT_TWO: &str = "arn:aws:acm:us-west-2:000000000000:certificate/22222222-2222-2222-2222-222222222222";

/// The form fields of one awsQuery request.
type Form = Vec<(String, String)>;

/// One row of the dispatch table: the action's name, the request fields it carries and
/// the HTTP status the one operation that owns that action must answer.
type DispatchCase = (&'static str, Form, u16);

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

    let mut load_balancer_attributes = action("DescribeLoadBalancerAttributes");
    load_balancer_attributes.push(p("LoadBalancerArn", &lb_arn));

    // A write that stores something on a resource an earlier row created.
    let mut tagging = action("AddTags");
    tagging.push(p("ResourceArns.member.1", &lb_arn));
    tagging.push(p("Tags.member.1.Key", "team"));
    tagging.push(p("Tags.member.1.Value", "dispatch"));

    // A write that withdraws what the row above stored.
    let mut untagging = action("RemoveTags");
    untagging.push(p("ResourceArns.member.1", &lb_arn));
    untagging.push(p("TagKeys.member.1", "team"));

    let bound: Vec<DispatchCase> = vec![
        ("CreateListener", listener, 200),
        ("DescribeLoadBalancers", action("DescribeLoadBalancers"), 200),
        ("DescribeTargetGroups", action("DescribeTargetGroups"), 200),
        ("DescribeListeners", listeners_of, 200),
        // Two catalogue reads: the account's quotas and the predefined TLS security
        // policies, both reporting a catalogue the service publishes.
        (
            "DescribeAccountLimits",
            action("DescribeAccountLimits"),
            200,
        ),
        ("DescribeSSLPolicies", action("DescribeSSLPolicies"), 200),
        // A reporting read of one load balancer's attributes.
        (
            "DescribeLoadBalancerAttributes",
            load_balancer_attributes,
            200,
        ),
        // A tag write, which stores the tags it was given on the load balancer the
        // first row created.
        ("AddTags", tagging, 200),
        // The sibling tag write, which withdraws from that same load balancer the key
        // the row above stored on it.
        ("RemoveTags", untagging, 200),
        // Implemented, and answering with its OWN refusal: `DescribeRules` is
        // documented "You must specify either a listener or rules", so a request
        // naming neither is that operation's 400 rather than a routing failure.
        ("DescribeRules", action("DescribeRules"), 400),
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
            // A list carrying more certificate ARNs than one call may name is the
            // modelled certificate-count refusal, whatever protocol the listener names;
            // every other break of the bracket is this service's input-validation code.
            let expected_code = if case.certificates.len() > 1 {
                TOO_MANY_CERTIFICATES
            } else {
                VALIDATION_ERROR
            };
            assert_eq!(
                (reply.status, reply.error_code().as_str()),
                (400, expected_code),
                "case {:?}: expected 400 {expected_code}, got {} {}",
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
            .find(|member| first_text(member, "Port") == Some(port.to_string()))
            .unwrap_or_else(|| {
                panic!("case {case:?}: no stored listener on port {port} in {stored:?}")
            });
        let reported_protocol = first_text(listener, "Protocol");
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
