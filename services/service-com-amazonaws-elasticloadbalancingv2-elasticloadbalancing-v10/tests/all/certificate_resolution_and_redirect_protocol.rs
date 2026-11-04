// SPDX-License-Identifier: CC0-1.0

//! Two configurations this API REFUSES, both asserted as the rule rather than as the one
//! request that reported them, and over every operation the rule reaches.
//!
//! 1. A certificate is named by a certificate ARN: a `Certificate$CertificateArn` that is
//!    not the ARN of a Certificate Manager certificate, nor of a server certificate
//!    uploaded to IAM, resolves to no certificate of the account, so the call is refused
//!    with `CertificateNotFoundException` (`CertificateNotFound`, 400) and the listener's
//!    certificates are left exactly as they were.
//!    [`a_certificate_arn_naming_no_certificate_is_refused_wherever_one_is_named`] crosses
//!    the ARN forms either side of that line with the three operations that take a
//!    certificate — `CreateListener`, `AddListenerCertificates` and `ModifyListener`.
//!
//! 2. A redirect action is judged against the PROTOCOL of the listener it is attached to:
//!    `RedirectActionConfig$Protocol` is documented "You can redirect HTTP to HTTP, HTTP
//!    to HTTPS, and HTTPS to HTTPS. You can't redirect HTTPS to HTTP", so a listener that
//!    terminates TLS cannot carry a redirect to cleartext, while the identical
//!    configuration on a listener that terminates no TLS is accepted. The refusal is
//!    `InvalidLoadBalancerActionException` (`InvalidLoadBalancerAction`, 400).
//!    [`a_redirect_to_http_is_refused_only_on_a_listener_that_terminates_tls`] crosses the
//!    listener protocols with the values the redirect's `Protocol` takes, over
//!    `CreateRule`, `ModifyListener`'s default actions and `CreateListener`'s.
//!
//! 3. A redirect's target URI keeps every component the request does not modify:
//!    `RedirectActionConfig` is documented "Any components that you do not modify retain
//!    their original values. You can reuse URI components using the following reserved
//!    keywords: #{protocol}, #{host}, #{port}, #{path} (the leading "/" is removed),
//!    #{query}" — so a read reports an unmodified component as the keyword that retains
//!    it, and one the request named exactly as it was named.
//!    [`an_unmodified_redirect_component_is_reported_as_the_keyword_that_retains_it`]
//!    crosses the components a request modifies over `CreateRule` and `ModifyListener`.
//!
//! Every accepted row is read back through a describe, and every refused row must leave
//! nothing behind.

use smithy_server_runtime::MultiServiceRouter;

// The wire plumbing — the emulator under test, the request builder and the XML envelope
// reader — is `crate::wire`, shared with this crate's other suites rather than copied per
// suite; the HTTP target group every forwarding listener below needs is the one
// `crate::provisioning_loop` already builds.
use crate::provisioning_loop::create_http_target_group;
// The crate's ONE reader of an element's text with the serializer's XML entities resolved,
// so a refusal message is compared as the VALUE the caller sees rather than as its
// encoding.
use crate::target_group_attributes_and_modification::unescaped;
use crate::wire::{action, call, first_text, members, p, router, Reply};

/// The awsQuery code of `CertificateNotFoundException`
/// (`smithy_query(query="shape",shape="CertificateNotFoundException")`: `awsQueryError`
/// code `CertificateNotFound`, `httpError` 400).
const CERTIFICATE_NOT_FOUND: &str = "CertificateNotFound";

/// The awsQuery code of `InvalidLoadBalancerActionException`
/// (`smithy_query(query="shape",shape="InvalidLoadBalancerActionException")`:
/// `awsQueryError` code `InvalidLoadBalancerAction`, `httpError` 400).
const INVALID_LOAD_BALANCER_ACTION: &str = "InvalidLoadBalancerAction";

/// The refusal a redirect that would downgrade TLS to cleartext is worded with. Entirely
/// fixed text: the rule is about the two protocols themselves, so nothing of the request
/// appears in it.
const DOWNGRADE_MESSAGE: &str = "You cannot redirect HTTPS to HTTP.";

/// The certificate a listener below is created with, which every refused row must leave
/// in place.
const CERT_DEFAULT: &str =
    "arn:aws:acm:us-west-2:000000000000:certificate/00000000-0000-0000-0000-000000000000";

/// One row of the certificate table: how the case reads, the `CertificateArn` it sends
/// and whether that value names a certificate at all.
struct CertificateCase {
    case: &'static str,
    value: &'static str,
    names_a_certificate: bool,
}

/// The ARN forms either side of "this value names a certificate".
///
/// The admitted forms are the two places the certificates an HTTPS or TLS listener serves
/// live — `Certificate$CertificateArn` is "The Amazon Resource Name (ARN) of the
/// certificate" — read against the workspace's one requirement for each
/// (`service_common::arn`'s `ACM_CERTIFICATE`, and the IAM `server-certificate` form
/// beside it). The refused rows walk the ways a value fails to be one: not an ARN at all,
/// another vendor's ARN, the right vendor's ARN of another resource type, and an ARN with
/// an empty field. Only the first of them is the value the report carried.
const CERTIFICATE_CASES: &[CertificateCase] = &[
    CertificateCase {
        case: "a Certificate Manager certificate",
        value: "arn:aws:acm:us-west-2:000000000000:certificate/11111111-1111-1111-1111-111111111111",
        names_a_certificate: true,
    },
    CertificateCase {
        case: "a certificate of another Region",
        value: "arn:aws:acm:eu-west-2:000000000000:certificate/22222222-2222-2222-2222-222222222222",
        names_a_certificate: true,
    },
    CertificateCase {
        case: "a certificate of another partition",
        value:
            "arn:aws-us-gov:acm:us-gov-west-1:000000000000:certificate/33333333-3333-3333-3333-333333333333",
        names_a_certificate: true,
    },
    CertificateCase {
        case: "a server certificate uploaded to IAM",
        value: "arn:aws:iam::000000000000:server-certificate/legacy-tls",
        names_a_certificate: true,
    },
    CertificateCase {
        case: "a value that is no ARN at all",
        value: "not-a-certificate-arn-at-all",
        names_a_certificate: false,
    },
    CertificateCase {
        case: "an ARN of another vendor",
        value: "arn:aws:s3:::certificates/leaf.pem",
        names_a_certificate: false,
    },
    CertificateCase {
        case: "a private certificate authority, which is not a certificate",
        value: "arn:aws:acm:us-west-2:000000000000:certificate-authority/44444444-4444-4444-4444-444444444444",
        names_a_certificate: false,
    },
    CertificateCase {
        case: "an IAM ARN of another resource type",
        value: "arn:aws:iam::000000000000:role/load-balancer",
        names_a_certificate: false,
    },
    CertificateCase {
        case: "an ARN whose vendor field is empty",
        value: "arn:aws::us-west-2:000000000000:certificate/55555555-5555-5555-5555-555555555555",
        names_a_certificate: false,
    },
    CertificateCase {
        case: "an ARN carrying no resource field",
        value: "arn:aws:acm:us-west-2:000000000000",
        names_a_certificate: false,
    },
];

/// Every listener protocol the redirect table is crossed with: the two the model brackets
/// the certificate members for, which are the ones that terminate TLS, and one that does
/// not — from `ProtocolEnum` (`src/ops/types/shared_types.rs`'s `PROTOCOL_ENUM_VALUES`).
const LISTENER_PROTOCOLS: &[&str] = &["HTTP", "HTTPS", "TLS"];

/// Every value a redirect's `Protocol` takes, from `RedirectActionProtocol`'s pattern
/// `^(HTTPS?|#\{protocol\})$` — plus `None`, the member being optional, which keeps the
/// request's own protocol exactly as `#{protocol}` does.
const REDIRECT_PROTOCOLS: &[Option<&str>] = &[Some("HTTP"), Some("HTTPS"), Some("#{protocol}"), None];

/// The form fields of one awsQuery request.
type Form = Vec<(String, String)>;

/// Create an application load balancer across two subnets and answer its ARN.
async fn create_load_balancer(router: &MultiServiceRouter, name: &str) -> String {
    let mut form = action("CreateLoadBalancer");
    form.push(p("Name", name));
    form.push(p("Subnets.member.1", "subnet-11111111"));
    form.push(p("Subnets.member.2", "subnet-22222222"));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case create load balancer {name}: expected 200, got {} body {}",
        reply.status, reply.body
    );
    reply
        .text("LoadBalancerArn")
        .unwrap_or_else(|| panic!("case create load balancer {name}: no LoadBalancerArn in {}", reply.body))
}

/// `CreateListener` on `lb_arn` carrying `extra` beside its default actions.
async fn create_listener(
    router: &MultiServiceRouter,
    lb_arn: &str,
    port: i32,
    protocol: &str,
    certificate: Option<&str>,
    default_actions: &Form,
) -> Reply {
    let mut form = action("CreateListener");
    form.push(p("LoadBalancerArn", lb_arn));
    form.push(p("Port", &port.to_string()));
    form.push(p("Protocol", protocol));
    if let Some(arn) = certificate {
        form.push(p("Certificates.member.1.CertificateArn", arn));
    }
    form.extend(default_actions.clone());
    call(router, &form).await
}

/// The default action every listener that is not under test for its actions carries: a
/// forward to `tg_arn`.
fn forward_to(tg_arn: &str) -> Form {
    vec![
        p("DefaultActions.member.1.Type", "forward"),
        p("DefaultActions.member.1.TargetGroupArn", tg_arn),
    ]
}

/// The five components of a redirect's target URI as a read reports them, in the order
/// `RedirectActionConfig` documents the URI itself:
/// `protocol://hostname:port/path?query`.
type UriComponents = (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

/// The `RedirectConfig` element of `xml`, which is where those components are reported.
fn redirect_config(xml: &str) -> Option<&str> {
    let open = "<RedirectConfig>";
    let start = xml.find(open)? + open.len();
    let end = xml[start..].find("</RedirectConfig>")? + start;
    Some(&xml[start..end])
}

/// The five components the `RedirectConfig` in `xml` reports, with the serializer's XML
/// entities resolved so a value is compared as the caller sees it.
fn reported_components(xml: &str) -> UriComponents {
    let config = redirect_config(xml).unwrap_or_default();
    let read = |tag: &str| first_text(config, tag).map(|text| unescaped(&text));
    (
        read("Protocol"),
        read("Host"),
        read("Port"),
        read("Path"),
        read("Query"),
    )
}

/// A redirect action's form fields under `prefix` (`Actions.member.1` for a rule,
/// `DefaultActions.member.1` for a listener), with the `Protocol` the row carries.
///
/// `StatusCode` is `RedirectActionConfig`'s one required member, so every row sends it.
fn redirect_action(prefix: &str, protocol: Option<&str>) -> Form {
    let mut fields = vec![
        p(&format!("{prefix}.Type"), "redirect"),
        p(&format!("{prefix}.RedirectConfig.StatusCode"), "HTTP_302"),
        p(&format!("{prefix}.RedirectConfig.Port"), "8080"),
    ];
    if let Some(protocol) = protocol {
        fields.push(p(&format!("{prefix}.RedirectConfig.Protocol"), protocol));
    }
    fields
}

/// `DescribeListenerCertificates` for one listener, as `(CertificateArn, IsDefault)` rows
/// in the order reported.
async fn certificate_rows(router: &MultiServiceRouter, arn: &str) -> Vec<(String, Option<String>)> {
    let mut form = action("DescribeListenerCertificates");
    form.push(p("ListenerArn", arn));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "reading the certificates of {arn} back must succeed, got {} {}",
        reply.status, reply.body
    );
    members(&reply.body, "Certificates")
        .iter()
        .map(|member| {
            (
                first_text(member, "CertificateArn").unwrap_or_default(),
                first_text(member, "IsDefault"),
            )
        })
        .collect()
}

/// The non-default rule ARNs `DescribeRules` reports for one listener.
async fn rule_arns(router: &MultiServiceRouter, listener_arn: &str) -> Vec<String> {
    let mut form = action("DescribeRules");
    form.push(p("ListenerArn", listener_arn));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "reading the rules of {listener_arn} back must succeed, got {} {}",
        reply.status, reply.body
    );
    members(&reply.body, "Rules")
        .iter()
        .filter(|member| first_text(member, "IsDefault").as_deref() != Some("true"))
        .filter_map(|member| first_text(member, "RuleArn"))
        .collect()
}

/// Rule 1: a `CertificateArn` that names no certificate is refused, by every operation
/// that takes one, and changes nothing.
#[tokio::test]
async fn a_certificate_arn_naming_no_certificate_is_refused_wherever_one_is_named() {
    let router = router();
    let lb_arn = create_load_balancer(&router, "certificate-resolution").await;
    let tg_arn = create_http_target_group(&router, "certificate-resolution-tg").await;
    let forward = forward_to(&tg_arn);

    // Two listeners that already serve a certificate: one the rows add certificates to,
    // one they replace the default certificate of. Separate, so the add rows and the
    // modify rows cannot be confused with each other on the read back.
    let added_to = create_listener(&router, &lb_arn, 443, "HTTPS", Some(CERT_DEFAULT), &forward)
        .await
        .text("ListenerArn")
        .expect("the listener certificates are added to");
    let modified = create_listener(&router, &lb_arn, 8443, "HTTPS", Some(CERT_DEFAULT), &forward)
        .await
        .text("ListenerArn")
        .expect("the listener whose default certificate is replaced");

    // What the rows leave behind: the certificates the add rows attached, the default
    // certificate the last accepted modify row left, and the listeners the create rows
    // minted.
    let mut attached: Vec<&str> = Vec::new();
    let mut default_certificate = CERT_DEFAULT;
    let mut created_listeners = 2usize;

    for (index, row) in CERTIFICATE_CASES.iter().enumerate() {
        let case = row.case;
        let expected_message = format!("Certificate '{}' not found", row.value);

        // (a) `CreateListener`: the default certificate of a listener being created. Each
        // row takes its own port, so a refusal is never a duplicate listener.
        let port = 9_000 + index as i32;
        let created = create_listener(
            &router,
            &lb_arn,
            port,
            "HTTPS",
            Some(row.value),
            &forward,
        )
        .await;
        if row.names_a_certificate {
            assert_eq!(
                created.status, 200,
                "case {case:?} on CreateListener: expected the listener to be created, got {} {}",
                created.status, created.body
            );
            created_listeners += 1;
        } else {
            assert_eq!(
                (created.status, created.error_code().as_str()),
                (400, CERTIFICATE_NOT_FOUND),
                "case {case:?} on CreateListener: expected 400 {CERTIFICATE_NOT_FOUND}, got {} {}",
                created.status,
                created.body
            );
            assert_eq!(
                created.text("Message").map(|text| unescaped(&text)),
                Some(expected_message.clone()),
                "case {case:?} on CreateListener: unexpected refusal wording in {}",
                created.body
            );
        }

        // (b) `AddListenerCertificates`: a certificate added to a listener's list.
        let mut form = action("AddListenerCertificates");
        form.push(p("ListenerArn", &added_to));
        form.push(p("Certificates.member.1.CertificateArn", row.value));
        let added = call(&router, &form).await;
        if row.names_a_certificate {
            assert_eq!(
                added.status, 200,
                "case {case:?} on AddListenerCertificates: expected 200, got {} {}",
                added.status, added.body
            );
            attached.push(row.value);
        } else {
            assert_eq!(
                (added.status, added.error_code().as_str()),
                (400, CERTIFICATE_NOT_FOUND),
                "case {case:?} on AddListenerCertificates: expected 400 {CERTIFICATE_NOT_FOUND}, got {} {}",
                added.status,
                added.body
            );
            assert_eq!(
                added.text("Message").map(|text| unescaped(&text)),
                Some(expected_message.clone()),
                "case {case:?} on AddListenerCertificates: unexpected refusal wording in {}",
                added.body
            );
        }

        // (c) `ModifyListener`: the default certificate replacing the stored one.
        let mut form = action("ModifyListener");
        form.push(p("ListenerArn", &modified));
        form.push(p("Certificates.member.1.CertificateArn", row.value));
        let changed = call(&router, &form).await;
        if row.names_a_certificate {
            assert_eq!(
                changed.status, 200,
                "case {case:?} on ModifyListener: expected 200, got {} {}",
                changed.status, changed.body
            );
            default_certificate = row.value;
        } else {
            assert_eq!(
                (changed.status, changed.error_code().as_str()),
                (400, CERTIFICATE_NOT_FOUND),
                "case {case:?} on ModifyListener: expected 400 {CERTIFICATE_NOT_FOUND}, got {} {}",
                changed.status,
                changed.body
            );
            assert_eq!(
                changed.text("Message").map(|text| unescaped(&text)),
                Some(expected_message.clone()),
                "case {case:?} on ModifyListener: unexpected refusal wording in {}",
                changed.body
            );
        }
    }

    // The certificate list holds the default certificate and exactly the ARNs the
    // accepted rows attached — nothing a refused row named.
    let mut expected: Vec<(String, Option<String>)> =
        vec![(CERT_DEFAULT.to_string(), Some("true".to_string()))];
    expected.extend(
        attached
            .iter()
            .map(|arn| ((*arn).to_string(), Some("false".to_string()))),
    );
    let reported = certificate_rows(&router, &added_to).await;
    assert_eq!(
        reported, expected,
        "the certificate list must hold the default certificate and the accepted ARNs only, got {reported:?}"
    );

    // The modified listener still serves the last certificate an accepted row gave it.
    let reported = certificate_rows(&router, &modified).await;
    assert_eq!(
        reported,
        vec![(default_certificate.to_string(), Some("true".to_string()))],
        "the default certificate must be the last accepted one, got {reported:?}"
    );

    // And a refused create stored no listener.
    let mut form = action("DescribeListeners");
    form.push(p("LoadBalancerArn", &lb_arn));
    let described = call(&router, &form).await;
    assert_eq!(
        members(&described.body, "Listeners").len(),
        created_listeners,
        "expected exactly the accepted listeners to be stored, got {}",
        described.body
    );
}

/// Rule 2: a redirect to cleartext HTTP is refused exactly on the listeners that
/// terminate TLS, on every operation that attaches an action to one.
#[tokio::test]
async fn a_redirect_to_http_is_refused_only_on_a_listener_that_terminates_tls() {
    let router = router();
    let lb_arn = create_load_balancer(&router, "redirect-protocol").await;
    let tg_arn = create_http_target_group(&router, "redirect-protocol-tg").await;
    let forward = forward_to(&tg_arn);

    // One listener per protocol under test, each with a forwarding default action, so a
    // row's refusal is about the action it attaches and nothing else.
    let mut listeners: Vec<(&str, String)> = Vec::new();
    for (index, protocol) in LISTENER_PROTOCOLS.iter().enumerate() {
        let certificate = (*protocol == "HTTPS" || *protocol == "TLS").then_some(CERT_DEFAULT);
        let port = 1_000 + index as i32;
        let created = create_listener(&router, &lb_arn, port, protocol, certificate, &forward).await;
        assert_eq!(
            created.status, 200,
            "case create the {protocol} listener: expected 200, got {} {}",
            created.status, created.body
        );
        let arn = created
            .text("ListenerArn")
            .unwrap_or_else(|| panic!("case create the {protocol} listener: no ListenerArn in {}", created.body));
        listeners.push((protocol, arn));
    }

    // The rules each listener must hold once the table has run, and the listeners the
    // create rows minted.
    let mut accepted_rules: Vec<(&str, String)> = Vec::new();
    let mut created_listeners = LISTENER_PROTOCOLS.len();
    let mut priority = 10;
    let mut port = 2_000;

    for (listener_protocol, listener_arn) in &listeners {
        // A listener that terminates TLS is the one the member's documentation rules the
        // downgrade out for; every other protocol takes the identical configuration.
        let terminates_tls = *listener_protocol == "HTTPS" || *listener_protocol == "TLS";
        for redirect_protocol in REDIRECT_PROTOCOLS {
            let accepted = !(terminates_tls && *redirect_protocol == Some("HTTP"));
            let case = format!(
                "a {listener_protocol} listener redirecting to {}",
                redirect_protocol.unwrap_or("the request's own protocol")
            );

            // (a) `CreateRule` on that listener. Each row takes its own priority, so a
            // refusal is never a priority already in use.
            priority += 1;
            let mut form = action("CreateRule");
            form.push(p("ListenerArn", listener_arn));
            form.push(p("Priority", &priority.to_string()));
            form.push(p("Conditions.member.1.Field", "path-pattern"));
            form.push(p("Conditions.member.1.Values.member.1", "/plain"));
            form.extend(redirect_action("Actions.member.1", *redirect_protocol));
            let reply = call(&router, &form).await;
            if accepted {
                assert_eq!(
                    reply.status, 200,
                    "case {case:?} on CreateRule: expected the rule to be created, got {} {}",
                    reply.status, reply.body
                );
                let rule = reply
                    .text("RuleArn")
                    .unwrap_or_else(|| panic!("case {case:?} on CreateRule: no RuleArn in {}", reply.body));
                accepted_rules.push((listener_protocol, rule));
            } else {
                assert_eq!(
                    (reply.status, reply.error_code().as_str()),
                    (400, INVALID_LOAD_BALANCER_ACTION),
                    "case {case:?} on CreateRule: expected 400 {INVALID_LOAD_BALANCER_ACTION}, got {} {}",
                    reply.status,
                    reply.body
                );
                assert_eq!(
                    reply.text("Message"),
                    Some(DOWNGRADE_MESSAGE.to_string()),
                    "case {case:?} on CreateRule: unexpected refusal wording in {}",
                    reply.body
                );
            }

            // (b) `ModifyListener`, where the same action becomes the listener's default
            // action.
            let mut form = action("ModifyListener");
            form.push(p("ListenerArn", listener_arn));
            form.extend(redirect_action("DefaultActions.member.1", *redirect_protocol));
            let reply = call(&router, &form).await;
            if accepted {
                assert_eq!(
                    reply.status, 200,
                    "case {case:?} on ModifyListener: expected 200, got {} {}",
                    reply.status, reply.body
                );
            } else {
                assert_eq!(
                    (reply.status, reply.error_code().as_str()),
                    (400, INVALID_LOAD_BALANCER_ACTION),
                    "case {case:?} on ModifyListener: expected 400 {INVALID_LOAD_BALANCER_ACTION}, got {} {}",
                    reply.status,
                    reply.body
                );
                assert_eq!(
                    reply.text("Message"),
                    Some(DOWNGRADE_MESSAGE.to_string()),
                    "case {case:?} on ModifyListener: unexpected refusal wording in {}",
                    reply.body
                );
            }

            // (c) `CreateListener`, where the action is the default action of a listener
            // being created on that same protocol.
            port += 1;
            let certificate = terminates_tls.then_some(CERT_DEFAULT);
            let reply = create_listener(
                &router,
                &lb_arn,
                port,
                listener_protocol,
                certificate,
                &redirect_action("DefaultActions.member.1", *redirect_protocol),
            )
            .await;
            if accepted {
                assert_eq!(
                    reply.status, 200,
                    "case {case:?} on CreateListener: expected the listener to be created, got {} {}",
                    reply.status, reply.body
                );
                created_listeners += 1;
            } else {
                assert_eq!(
                    (reply.status, reply.error_code().as_str()),
                    (400, INVALID_LOAD_BALANCER_ACTION),
                    "case {case:?} on CreateListener: expected 400 {INVALID_LOAD_BALANCER_ACTION}, got {} {}",
                    reply.status,
                    reply.body
                );
                assert_eq!(
                    reply.text("Message"),
                    Some(DOWNGRADE_MESSAGE.to_string()),
                    "case {case:?} on CreateListener: unexpected refusal wording in {}",
                    reply.body
                );
            }
        }
    }

    // Each listener holds exactly the rules the accepted rows created: a refused
    // `CreateRule` left none behind.
    for (listener_protocol, listener_arn) in &listeners {
        let expected: Vec<String> = accepted_rules
            .iter()
            .filter(|(protocol, _)| protocol == listener_protocol)
            .map(|(_, rule)| rule.clone())
            .collect();
        let reported = rule_arns(&router, listener_arn).await;
        assert_eq!(
            reported, expected,
            "case the rules of the {listener_protocol} listener: expected only the accepted ones, got {reported:?}"
        );
    }

    // And a refused create stored no listener.
    let mut form = action("DescribeListeners");
    form.push(p("LoadBalancerArn", &lb_arn));
    let described = call(&router, &form).await;
    assert_eq!(
        members(&described.body, "Listeners").len(),
        created_listeners,
        "expected exactly the accepted listeners to be stored, got {}",
        described.body
    );
}

/// One row of the URI-component table: how the case reads, the components the request
/// modifies and the five components a read must then report.
struct ComponentCase {
    case: &'static str,
    modified: &'static [(&'static str, &'static str)],
    expected: (
        &'static str,
        &'static str,
        &'static str,
        &'static str,
        &'static str,
    ),
}

/// The rows differ along WHICH components of the target URI a request modifies, which is
/// what the rule generalises over. Every row modifies at least one, as
/// `RedirectActionConfig` requires ("You must modify at least one of the following
/// components to avoid a redirect loop"), and the last row shows that a keyword the
/// request writes out itself is reported exactly as it was written.
const COMPONENT_CASES: &[ComponentCase] = &[
    ComponentCase {
        case: "a host rewrite",
        modified: &[("Host", "moved.example.com")],
        expected: (
            "#{protocol}",
            "moved.example.com",
            "#{port}",
            "/#{path}",
            "#{query}",
        ),
    },
    ComponentCase {
        case: "a protocol and a host",
        modified: &[("Protocol", "HTTPS"), ("Host", "secure.example.com")],
        expected: (
            "HTTPS",
            "secure.example.com",
            "#{port}",
            "/#{path}",
            "#{query}",
        ),
    },
    ComponentCase {
        case: "a path that reuses the original",
        modified: &[("Path", "/new/#{path}")],
        expected: (
            "#{protocol}",
            "#{host}",
            "#{port}",
            "/new/#{path}",
            "#{query}",
        ),
    },
    ComponentCase {
        case: "a numeric port",
        modified: &[("Port", "8443")],
        expected: ("#{protocol}", "#{host}", "8443", "/#{path}", "#{query}"),
    },
    ComponentCase {
        // The query is NOT one of the four components whose modification avoids a
        // redirect loop ("protocol, hostname, port, or path"), so this row modifies the
        // port beside it: a redirect that changed only the query would be refused.
        case: "a query that reuses the original",
        modified: &[("Port", "8080"), ("Query", "#{query}&value=xyz")],
        expected: (
            "#{protocol}",
            "#{host}",
            "8080",
            "/#{path}",
            "#{query}&value=xyz",
        ),
    },
    ComponentCase {
        case: "every component at once",
        modified: &[
            ("Protocol", "HTTPS"),
            ("Host", "all.example.com"),
            ("Port", "443"),
            ("Path", "/landing"),
            ("Query", "from=elb"),
        ],
        expected: ("HTTPS", "all.example.com", "443", "/landing", "from=elb"),
    },
    ComponentCase {
        case: "a protocol keyword the request writes out itself",
        modified: &[("Protocol", "#{protocol}"), ("Host", "keep.example.com")],
        expected: (
            "#{protocol}",
            "keep.example.com",
            "#{port}",
            "/#{path}",
            "#{query}",
        ),
    },
];

/// Rule 3: a component the request does not modify is reported as the keyword that
/// retains its original value, and one the request names is reported as named.
#[tokio::test]
async fn an_unmodified_redirect_component_is_reported_as_the_keyword_that_retains_it() {
    let router = router();
    let lb_arn = create_load_balancer(&router, "redirect-components").await;
    let tg_arn = create_http_target_group(&router, "redirect-components-tg").await;
    let listener = create_listener(&router, &lb_arn, 80, "HTTP", None, &forward_to(&tg_arn))
        .await
        .text("ListenerArn")
        .expect("the listener the rows attach their redirects to");

    let mut priority = 100;
    for row in COMPONENT_CASES {
        let case = row.case;
        let (protocol, host, port, path, query) = row.expected;
        let expected: UriComponents = (
            Some(protocol.to_string()),
            Some(host.to_string()),
            Some(port.to_string()),
            Some(path.to_string()),
            Some(query.to_string()),
        );

        // (a) a rule's action, read back through `DescribeRules` rather than trusted from
        // the create's own answer.
        priority += 1;
        let mut form = action("CreateRule");
        form.push(p("ListenerArn", &listener));
        form.push(p("Priority", &priority.to_string()));
        form.push(p("Conditions.member.1.Field", "path-pattern"));
        form.push(p("Conditions.member.1.Values.member.1", "/plain"));
        form.push(p("Actions.member.1.Type", "redirect"));
        form.push(p("Actions.member.1.RedirectConfig.StatusCode", "HTTP_301"));
        for (member, value) in row.modified {
            form.push(p(
                &format!("Actions.member.1.RedirectConfig.{member}"),
                value,
            ));
        }
        let created = call(&router, &form).await;
        assert_eq!(
            created.status, 200,
            "case {case:?} on CreateRule: expected the rule to be created, got {} {}",
            created.status, created.body
        );
        let rule = created.text("RuleArn").unwrap_or_else(|| {
            panic!(
                "case {case:?} on CreateRule: no RuleArn in {}",
                created.body
            )
        });

        let mut form = action("DescribeRules");
        form.push(p("ListenerArn", &listener));
        let described = call(&router, &form).await;
        let stored = members(&described.body, "Rules")
            .into_iter()
            .find(|member| member.contains(&rule))
            .unwrap_or_else(|| panic!("case {case:?}: no rule {rule} in {}", described.body));
        let reported = reported_components(&stored);
        assert_eq!(
            reported, expected,
            "case {case:?} read back through DescribeRules: expected {expected:?}, got {reported:?}"
        );

        // (b) the same action as the listener's default action, read back through
        // `DescribeListeners`: one normalisation serves both.
        let mut form = action("ModifyListener");
        form.push(p("ListenerArn", &listener));
        form.push(p("DefaultActions.member.1.Type", "redirect"));
        form.push(p(
            "DefaultActions.member.1.RedirectConfig.StatusCode",
            "HTTP_301",
        ));
        for (member, value) in row.modified {
            form.push(p(
                &format!("DefaultActions.member.1.RedirectConfig.{member}"),
                value,
            ));
        }
        let changed = call(&router, &form).await;
        assert_eq!(
            changed.status, 200,
            "case {case:?} on ModifyListener: expected 200, got {} {}",
            changed.status, changed.body
        );

        let mut form = action("DescribeListeners");
        form.push(p("ListenerArns.member.1", &listener));
        let described = call(&router, &form).await;
        let reported = reported_components(&described.body);
        assert_eq!(
            reported, expected,
            "case {case:?} read back through DescribeListeners: expected {expected:?}, got {reported:?}"
        );
    }
}
