// SPDX-License-Identifier: CC0-1.0

//! The listener half of this API, driven over the wire exactly as a client drives it:
//! reconfiguring a listener, deleting it, managing its certificate list, reading and
//! writing its attributes, and reading its rules.
//!
//! Five rules are asserted here, each over a SET of inputs rather than over the one call
//! a recording happens to carry:
//!
//! 1. Every operation that addresses ONE listener by ARN resolves that ARN against the
//!    store and refuses an ARN naming no listener with `ListenerNotFound` / HTTP 400 —
//!    whatever the operation and however the ARN came to be absent.
//!    [`every_listener_operation_refuses_an_absent_listener`] crosses all nine of them
//!    with an ARN that never named a listener and with one whose listener was deleted.
//!    A value that is no listener ARN at all names no listener this API can have, so it
//!    is refused as an invalid ARN instead
//!    ([`a_value_that_is_no_listener_arn_is_refused_as_invalid`]).
//! 2. `DeleteListener` releases exactly the listener named: its siblings and their
//!    target group associations stay, and a second delete of it is the same refusal as
//!    for a listener that never existed
//!    ([`deleting_a_listener_releases_only_that_listener`]).
//! 3. `ModifyListener` replaces the properties the request names and leaves the rest as
//!    they were, and the certificate and security policy follow the protocol the call
//!    LEAVES the listener on ([`modify_listener_replaces_only_what_the_request_names`],
//!    [`modify_listener_refuses_a_configuration_the_model_rules_out`]).
//! 4. A listener's certificate LIST is a set managed by `AddListenerCertificates` and
//!    `RemoveListenerCertificates` and reported, beside the default certificate, by
//!    `DescribeListenerCertificates`; the listener's default certificate itself is not
//!    removable through it
//!    ([`the_certificate_list_round_trips_through_add_describe_and_remove`]).
//! 5. A listener reports every attribute its load balancer's family carries, defaulted
//!    as that attribute's documentation states, and accepts only values in the domain it
//!    states ([`listener_attributes_default_and_accept_only_their_documented_domain`]).
//! 6. Every listener has one default rule, carrying the listener's default actions
//!    ([`describe_rules_reports_the_listeners_default_rule`]).
//!
//! Every accepted write is read back through a describe rather than trusted from the
//! write's own answer.

use crate::provisioning_loop::{
    account, action, call, create_http_target_group, emulator, first_text, members, p, Reply, REGION,
};
use crate::target_group_attributes_and_modification::unescaped;
use smithy_server_runtime::MultiServiceRouter;
use std::collections::BTreeMap;

/// The awsQuery error code AWS answers a listener ARN naming no listener with
/// (`smithy_query(query="operation",operation="ModifyListener")`:
/// `ListenerNotFoundException` -> `awsQueryError` code `ListenerNotFound`).
const LISTENER_NOT_FOUND: &str = "ListenerNotFound";

/// The error code this service answers an input its own documented rules refuse with.
const VALIDATION_ERROR: &str = "ValidationError";

/// The awsQuery code of `InvalidConfigurationRequestException`, which
/// `ModifyListenerAttributes` models for an attribute it cannot store.
const INVALID_CONFIGURATION: &str = "InvalidConfigurationRequest";

/// The awsQuery code of `OperationNotPermittedException`, which
/// `RemoveListenerCertificates` models.
const OPERATION_NOT_PERMITTED: &str = "OperationNotPermitted";

/// The awsQuery code of `DuplicateListenerException`.
const DUPLICATE_LISTENER: &str = "DuplicateListener";

/// The awsQuery code of `TooManyCertificatesException`, which `CreateListener` and
/// `ModifyListener` model for a request carrying more certificate ARNs than one call may.
const TOO_MANY_CERTIFICATES: &str = "TooManyCertificates";

/// The awsQuery code of `TargetGroupNotFoundException`.
const TARGET_GROUP_NOT_FOUND: &str = "TargetGroupNotFound";

/// The awsQuery code of `RuleNotFoundException`.
const RULE_NOT_FOUND: &str = "RuleNotFound";

/// Certificate ARNs to hand a listener. Any Certificate Manager certificate ARN does:
/// this service resolves the ARN FORM of a requested certificate (pinned in
/// `all/certificate_resolution_and_redirect_protocol.rs`) and not the certificate itself
/// against ACM, so the rules under test here are about which listener takes certificates
/// and how the list is managed.
const CERT_DEFAULT: &str =
    "arn:aws:acm:us-west-2:000000000000:certificate/00000000-0000-0000-0000-000000000000";
const CERT_ONE: &str =
    "arn:aws:acm:us-west-2:000000000000:certificate/11111111-1111-1111-1111-111111111111";
const CERT_TWO: &str =
    "arn:aws:acm:us-west-2:000000000000:certificate/22222222-2222-2222-2222-222222222222";
const CERT_THREE: &str =
    "arn:aws:acm:us-west-2:000000000000:certificate/33333333-3333-3333-3333-333333333333";

/// One predefined security policy name, from the catalog `DescribeSSLPolicies` reports.
const SSL_POLICY: &str = "ELBSecurityPolicy-2016-08";

/// The form fields of one awsQuery request.
type Form = Vec<(String, String)>;

/// Create a load balancer of `lb_type` across two subnets and answer its ARN.
async fn create_load_balancer(router: &MultiServiceRouter, name: &str, lb_type: &str) -> String {
    let mut form = action("CreateLoadBalancer");
    form.push(p("Name", name));
    form.push(p("Type", lb_type));
    form.push(p("Subnets.member.1", "subnet-11111111"));
    form.push(p("Subnets.member.2", "subnet-22222222"));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case create {lb_type} load balancer {name}: expected 200, got {} body {}",
        reply.status, reply.body
    );
    reply.text("LoadBalancerArn").unwrap_or_else(|| {
        panic!(
            "case create {lb_type} load balancer {name}: no LoadBalancerArn in {}",
            reply.body
        )
    })
}

/// Create a listener carrying `extra` beside the load balancer and a forwarding default
/// action, and answer its ARN.
///
/// `pub(crate)` because `all/attribute_collections.rs` makes listeners of every load
/// balancer family the same way: ONE creator for the crate, so the two suites cannot
/// drift on how a listener is made.
pub(crate) async fn create_listener(
    router: &MultiServiceRouter,
    case: &str,
    lb_arn: &str,
    tg_arn: &str,
    extra: Form,
) -> String {
    let mut form = action("CreateListener");
    form.push(p("LoadBalancerArn", lb_arn));
    form.push(p("DefaultActions.member.1.Type", "forward"));
    form.push(p("DefaultActions.member.1.TargetGroupArn", tg_arn));
    form.extend(extra);
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case {case}: expected the listener to be created, got {} {}",
        reply.status, reply.body
    );
    reply
        .text("ListenerArn")
        .unwrap_or_else(|| panic!("case {case}: no ListenerArn in {}", reply.body))
}

/// `DescribeListeners` for one listener ARN.
async fn describe_listener(router: &MultiServiceRouter, arn: &str) -> Reply {
    let mut form = action("DescribeListeners");
    form.push(p("ListenerArns.member.1", arn));
    call(router, &form).await
}

/// `DescribeListenerCertificates` for one listener, optionally paged.
async fn describe_certificates(
    router: &MultiServiceRouter,
    arn: &str,
    page_size: Option<i32>,
    marker: Option<&str>,
) -> Reply {
    let mut form = action("DescribeListenerCertificates");
    form.push(p("ListenerArn", arn));
    if let Some(size) = page_size {
        form.push(p("PageSize", &size.to_string()));
    }
    if let Some(marker) = marker {
        form.push(p("Marker", marker));
    }
    call(router, &form).await
}

/// `DescribeListenerAttributes` as a key/value map, so a row asserts on values rather
/// than on element order.
async fn describe_attributes(router: &MultiServiceRouter, arn: &str) -> (Reply, KeyValues) {
    let mut form = action("DescribeListenerAttributes");
    form.push(p("ListenerArn", arn));
    let reply = call(router, &form).await;
    let reported = key_values(&reply.body, "Attributes");
    (reply, reported)
}

/// The `Key`/`Value` pairs of every `<member>` of `container`.
type KeyValues = BTreeMap<String, String>;

fn key_values(xml: &str, container: &str) -> KeyValues {
    members(xml, container)
        .iter()
        .filter_map(|member| {
            let key = first_text(member, "Key")?;
            Some((key, first_text(member, "Value").unwrap_or_default()))
        })
        .collect()
}

/// Every `CertificateArn`/`IsDefault` pair of a `Certificates` list, in the order
/// reported.
fn certificate_rows(xml: &str) -> Vec<(String, Option<String>)> {
    members(xml, "Certificates")
        .iter()
        .map(|member| {
            (
                first_text(member, "CertificateArn").unwrap_or_default(),
                first_text(member, "IsDefault"),
            )
        })
        .collect()
}

/// The request fields that address `arn` with `operation`, beside `Action` and
/// `Version`: the member each operation names a listener through, plus whatever else it
/// requires.
fn addressing(operation: &str, arn: &str) -> Form {
    let mut form = action(operation);
    match operation {
        // The two list reads take the listener in a list member.
        "DescribeListeners" => form.push(p("ListenerArns.member.1", arn)),
        _ => form.push(p("ListenerArn", arn)),
    }
    match operation {
        "ModifyListener" => form.push(p("Port", "8080")),
        "ModifyListenerAttributes" => {
            form.push(p("Attributes.member.1.Key", "tcp.idle_timeout.seconds"));
            form.push(p("Attributes.member.1.Value", "400"));
        }
        "AddListenerCertificates" | "RemoveListenerCertificates" => {
            form.push(p("Certificates.member.1.CertificateArn", CERT_ONE));
        }
        _ => {}
    }
    form
}

/// Rule 1: an ARN naming no listener is `ListenerNotFound`, in every operation that
/// addresses one.
///
/// The rows are the WHOLE set of operations that take a listener ARN — the reconfigure,
/// the delete, the three certificate operations, the two attribute operations and the
/// two reads — crossed with the two ways an ARN comes to name nothing: it never named a
/// listener, or the listener it named was deleted. The refusal of an operation that acts
/// on the listener names the ARN it was given, which is the only thing the caller can
/// act on.
#[tokio::test]
async fn every_listener_operation_refuses_an_absent_listener() {
    let (router, _engine) = emulator();
    let lb_arn = create_load_balancer(&router, "absent-listener-alb", "application").await;
    let tg_arn = create_http_target_group(&router, "absent-listener-tg").await;

    // An ARN that never named a listener: well formed, of this account and region, with
    // a load balancer name and ids no resource here carries.
    let never_existed = format!(
        "arn:aws:elasticloadbalancing:{REGION}:{}:listener/app/probelb/0123456789abcdef/fedcba9876543210",
        account()
    );
    // And one whose listener existed and was deleted, so the store itself made it
    // absent.
    let deleted = create_listener(
        &router,
        "a listener to delete",
        &lb_arn,
        &tg_arn,
        vec![p("Protocol", "HTTP"), p("Port", "80")],
    )
    .await;
    let mut delete = action("DeleteListener");
    delete.push(p("ListenerArn", &deleted));
    let reply = call(&router, &delete).await;
    assert_eq!(
        reply.status, 200,
        "case delete the listener first: expected 200, got {} {}",
        reply.status, reply.body
    );

    // Every operation whose input carries a listener ARN
    // (`smithy_query(query="digest")`: the listener's own operations plus the two reads
    // that select by listener).
    let operations = [
        "ModifyListener",
        "ModifyListenerAttributes",
        "DescribeListenerAttributes",
        "AddListenerCertificates",
        "RemoveListenerCertificates",
        "DescribeListenerCertificates",
        "DeleteListener",
        "DescribeListeners",
        "DescribeRules",
    ];
    // The operations that ACT on the one listener they name, and the attribute read,
    // whose refusal quotes that ARN back; the list reads refuse the selection instead
    // and name no value.
    let names_the_arn = [
        "ModifyListener",
        "ModifyListenerAttributes",
        "DescribeListenerAttributes",
        "AddListenerCertificates",
        "RemoveListenerCertificates",
        "DeleteListener",
    ];

    for operation in operations {
        for (how, arn) in [
            ("an ARN that never named a listener", &never_existed),
            ("the ARN of a deleted listener", &deleted),
        ] {
            let case = format!("{operation} with {how}");
            let reply = call(&router, &addressing(operation, arn)).await;
            assert_eq!(
                (reply.status, reply.error_code().as_str()),
                (400, LISTENER_NOT_FOUND),
                "case {case}: expected 400 {LISTENER_NOT_FOUND}, got {} {}",
                reply.status,
                reply.body
            );
            let message = unescaped(&reply.text("Message").unwrap_or_default());
            assert!(
                !message.is_empty(),
                "case {case}: the refusal must carry a message, got {}",
                reply.body
            );
            assert_eq!(
                message.contains(arn.as_str()),
                names_the_arn.contains(&operation),
                "case {case}: an operation addressing one listener names the ARN it was \
                 given and a list read does not, got {message:?}"
            );
            // Every awsQuery error of this endpoint carries the request id AWS puts
            // beside `<Error>`.
            assert!(
                reply.text("RequestId").is_some(),
                "case {case}: the error envelope must carry a RequestId, got {}",
                reply.body
            );
        }
    }
}

/// Rule 1b: a value that is no listener ARN at all is refused as an INVALID ARN, not
/// reported as a missing listener.
///
/// A listener ARN is `listener/<type>/<load-balancer-name>/<load-balancer-id>/<id>`, and
/// the load balancer name in it obeys the rule the model states for a load balancer name
/// ("can have a maximum of 32 characters, must contain only alphanumeric characters or
/// hyphens, must not begin or end with a hyphen"). The rows below break one part of that
/// each — including a name one character past the published maximum, and one exactly on
/// it, which must be accepted as well formed — and every operation that addresses a
/// listener is asked with each.
#[tokio::test]
async fn a_value_that_is_no_listener_arn_is_refused_as_invalid() {
    let (router, _engine) = emulator();

    /// A listener ARN of this account and Region carrying `resource`.
    fn listener_arn(resource: &str) -> String {
        format!(
            "arn:aws:elasticloadbalancing:{REGION}:{}:{resource}",
            account()
        )
    }

    // A load balancer name exactly at the published maximum, and one past it.
    let at_maximum = "a".repeat(32);
    let past_maximum = "a".repeat(33);

    // Each row: what is wrong with the value, the value, and whether it is a well-formed
    // listener ARN.
    let rows: Vec<(String, String, bool)> = vec![
        (
            "a name at the published maximum length".to_string(),
            listener_arn(&format!(
                "listener/app/{at_maximum}/0123456789abcdef/fedcba9876543210"
            )),
            true,
        ),
        (
            "a name one character past the published maximum".to_string(),
            listener_arn(&format!(
                "listener/app/{past_maximum}/0123456789abcdef/fedcba9876543210"
            )),
            false,
        ),
        (
            "a name carrying a character a load balancer name may not".to_string(),
            listener_arn("listener/app/probe_lb/0123456789abcdef/fedcba9876543210"),
            false,
        ),
        (
            "a name ending in a hyphen".to_string(),
            listener_arn("listener/app/probelb-/0123456789abcdef/fedcba9876543210"),
            false,
        ),
        (
            "a load balancer type code this service never mints".to_string(),
            listener_arn("listener/web/probelb/0123456789abcdef/fedcba9876543210"),
            false,
        ),
        (
            "a listener id that is not a resource id".to_string(),
            listener_arn("listener/app/probelb/0123456789abcdef/not-hex"),
            false,
        ),
        (
            "one segment too few".to_string(),
            listener_arn("listener/app/probelb/0123456789abcdef"),
            false,
        ),
        (
            "the ARN of another resource of this service".to_string(),
            listener_arn("targetgroup/probetg/0123456789abcdef"),
            false,
        ),
        (
            "the ARN of another service".to_string(),
            CERT_ONE.to_string(),
            false,
        ),
        (
            "a value that is no ARN at all".to_string(),
            "probelb-listener".to_string(),
            false,
        ),
    ];

    let operations = [
        "ModifyListener",
        "ModifyListenerAttributes",
        "DescribeListenerAttributes",
        "AddListenerCertificates",
        "RemoveListenerCertificates",
        "DescribeListenerCertificates",
        "DeleteListener",
        "DescribeListeners",
        "DescribeRules",
    ];

    for operation in operations {
        for (wrong, value, well_formed) in &rows {
            let case = format!("{operation} with {wrong}");
            let reply = call(&router, &addressing(operation, value)).await;
            if *well_formed {
                // Well formed and naming nothing: the operation's own missing-listener
                // refusal, not the invalid-ARN one.
                assert_eq!(
                    (reply.status, reply.error_code().as_str()),
                    (400, LISTENER_NOT_FOUND),
                    "case {case}: expected 400 {LISTENER_NOT_FOUND}, got {} {}",
                    reply.status,
                    reply.body
                );
                continue;
            }
            assert_eq!(
                (reply.status, reply.error_code().as_str()),
                (400, VALIDATION_ERROR),
                "case {case}: expected 400 {VALIDATION_ERROR}, got {} {}",
                reply.status,
                reply.body
            );
            assert_eq!(
                reply.text("Message").as_deref().map(unescaped),
                Some(format!("'{value}' is not a valid listener ARN")),
                "case {case}: the refusal must name the value it judged, got {}",
                reply.body
            );
        }
    }
}

/// Rule 2: `DeleteListener` releases exactly the listener it names.
#[tokio::test]
async fn deleting_a_listener_releases_only_that_listener() {
    let (router, _engine) = emulator();
    let lb_arn = create_load_balancer(&router, "delete-listener-alb", "application").await;
    let first_group = create_http_target_group(&router, "delete-listener-tg-1").await;
    let second_group = create_http_target_group(&router, "delete-listener-tg-2").await;

    let first = create_listener(
        &router,
        "the listener to delete",
        &lb_arn,
        &first_group,
        vec![p("Protocol", "HTTP"), p("Port", "80")],
    )
    .await;
    let second = create_listener(
        &router,
        "the sibling listener",
        &lb_arn,
        &second_group,
        vec![p("Protocol", "HTTP"), p("Port", "8080")],
    )
    .await;

    let mut all = action("DescribeListeners");
    all.push(p("LoadBalancerArn", &lb_arn));
    let before = call(&router, &all).await;
    assert_eq!(
        members(&before.body, "Listeners").len(),
        2,
        "case before the delete: expected both listeners, got {}",
        before.body
    );

    let mut form = action("DeleteListener");
    form.push(p("ListenerArn", &first));
    let deleted = call(&router, &form).await;
    assert_eq!(
        deleted.status, 200,
        "case delete the first listener: expected 200, got {} {}",
        deleted.status, deleted.body
    );

    let after = call(&router, &all).await;
    let remaining = members(&after.body, "Listeners");
    assert_eq!(
        remaining.len(),
        1,
        "case after the delete: expected the sibling alone, got {}",
        after.body
    );
    assert_eq!(
        first_text(&remaining[0], "ListenerArn").as_deref(),
        Some(second.as_str()),
        "case after the delete: the surviving listener must be the sibling, got {}",
        remaining[0]
    );

    // A target group reports the load balancers routing to it: the deleted listener's
    // group no longer does, and the sibling's still does.
    let groups = call(&router, &action("DescribeTargetGroups")).await;
    for (case, group_arn, expected) in [
        ("the deleted listener's target group", &first_group, false),
        ("the sibling listener's target group", &second_group, true),
    ] {
        let described = members(&groups.body, "TargetGroups")
            .into_iter()
            .find(|member| first_text(member, "TargetGroupArn").as_deref() == Some(group_arn))
            .unwrap_or_else(|| panic!("case {case}: not reported in {}", groups.body));
        assert_eq!(
            described.contains(lb_arn.as_str()),
            expected,
            "case {case}: expected the association to be {expected}, got {described}"
        );
    }

    // Deleting it again is the same refusal as for a listener that never existed.
    let again = call(&router, &form).await;
    assert_eq!(
        (again.status, again.error_code().as_str()),
        (400, LISTENER_NOT_FOUND),
        "case delete the same listener twice: expected 400 {LISTENER_NOT_FOUND}, got {} {}",
        again.status,
        again.body
    );
}

/// What a modified listener must report: the members the rows assert on.
struct Expected {
    port: &'static str,
    protocol: &'static str,
    certificates: Vec<&'static str>,
    ssl_policy: Option<&'static str>,
    forwards_to: usize,
}

/// Rule 3: `ModifyListener` replaces what the request names and leaves the rest.
///
/// The rows are applied in order to ONE listener, each naming a different member or
/// combination — a port alone, a protocol that starts terminating TLS (with the
/// certificate and policy the model then requires), a security policy alone, a default
/// action list, and a protocol that stops terminating TLS — and after each the listener
/// is read back through `DescribeListeners`, which is what proves the members the row
/// did NOT name are unchanged.
#[tokio::test]
async fn modify_listener_replaces_only_what_the_request_names() {
    let (router, _engine) = emulator();
    let lb_arn = create_load_balancer(&router, "modify-listener-alb", "application").await;
    let groups = [
        create_http_target_group(&router, "modify-listener-tg-1").await,
        create_http_target_group(&router, "modify-listener-tg-2").await,
    ];
    let listener = create_listener(
        &router,
        "the listener to modify",
        &lb_arn,
        &groups[0],
        vec![p("Protocol", "HTTP"), p("Port", "80")],
    )
    .await;

    let rows: Vec<(&str, Form, Expected)> = vec![
        (
            "the port alone",
            vec![p("Port", "9001")],
            Expected {
                port: "9001",
                protocol: "HTTP",
                certificates: Vec::new(),
                ssl_policy: None,
                forwards_to: 0,
            },
        ),
        (
            "a protocol that terminates TLS, with its certificate and policy",
            vec![
                p("Protocol", "HTTPS"),
                p("SslPolicy", SSL_POLICY),
                p("Certificates.member.1.CertificateArn", CERT_DEFAULT),
            ],
            Expected {
                port: "9001",
                protocol: "HTTPS",
                certificates: vec![CERT_DEFAULT],
                ssl_policy: Some(SSL_POLICY),
                forwards_to: 0,
            },
        ),
        (
            "the default certificate alone, replacing the one stored",
            vec![p("Certificates.member.1.CertificateArn", CERT_ONE)],
            Expected {
                port: "9001",
                protocol: "HTTPS",
                certificates: vec![CERT_ONE],
                ssl_policy: Some(SSL_POLICY),
                forwards_to: 0,
            },
        ),
        (
            "the default actions alone, forwarding to the other target group",
            vec![
                p("DefaultActions.member.1.Type", "forward"),
                p("DefaultActions.member.1.TargetGroupArn", &groups[1]),
            ],
            Expected {
                port: "9001",
                protocol: "HTTPS",
                certificates: vec![CERT_ONE],
                ssl_policy: Some(SSL_POLICY),
                forwards_to: 1,
            },
        ),
        (
            // "Changing the protocol from HTTPS to HTTP, or from TLS to TCP, removes the
            // security policy and default certificate properties."
            "a protocol that terminates no TLS, which drops the certificate and policy",
            vec![p("Protocol", "HTTP")],
            Expected {
                port: "9001",
                protocol: "HTTP",
                certificates: Vec::new(),
                ssl_policy: None,
                forwards_to: 1,
            },
        ),
    ];

    for (case, fields, expected) in rows {
        let mut form = action("ModifyListener");
        form.push(p("ListenerArn", &listener));
        form.extend(fields);
        let reply = call(&router, &form).await;
        assert_eq!(
            reply.status, 200,
            "case {case}: expected the modification to be accepted, got {} {}",
            reply.status, reply.body
        );

        let described = describe_listener(&router, &listener).await;
        let reported = members(&described.body, "Listeners");
        assert_eq!(
            reported.len(),
            1,
            "case {case}: expected exactly the modified listener, got {}",
            described.body
        );
        let member = &reported[0];
        assert_eq!(
            first_text(member, "Port").as_deref(),
            Some(expected.port),
            "case {case}: unexpected Port in {member}"
        );
        assert_eq!(
            first_text(member, "Protocol").as_deref(),
            Some(expected.protocol),
            "case {case}: unexpected Protocol in {member}"
        );
        assert_eq!(
            certificate_rows(member)
                .into_iter()
                .map(|(arn, _)| arn)
                .collect::<Vec<String>>(),
            expected.certificates,
            "case {case}: unexpected default certificate in {member}"
        );
        assert_eq!(
            first_text(member, "SslPolicy").as_deref(),
            expected.ssl_policy,
            "case {case}: unexpected SslPolicy in {member}"
        );
        assert!(
            member.contains(groups[expected.forwards_to].as_str()),
            "case {case}: expected the default action to forward to target group {}, got {member}",
            expected.forwards_to + 1
        );
        // `Certificate$IsDefault` "is not included in the output when describing a
        // listener".
        assert!(
            first_text(member, "IsDefault").is_none(),
            "case {case}: a described listener reports no IsDefault, got {member}"
        );
    }
}

/// Rule 3, the refusals: a modification the model's own rules rule out is refused, and
/// the listener keeps the configuration it had.
#[tokio::test]
async fn modify_listener_refuses_a_configuration_the_model_rules_out() {
    let (router, _engine) = emulator();
    let lb_arn = create_load_balancer(&router, "modify-refusal-alb", "application").await;
    let tg_arn = create_http_target_group(&router, "modify-refusal-tg").await;
    let listener = create_listener(
        &router,
        "the listener to modify",
        &lb_arn,
        &tg_arn,
        vec![p("Protocol", "HTTP"), p("Port", "80")],
    )
    .await;
    // A sibling, so the duplicate-port row has a port to collide with.
    create_listener(
        &router,
        "the sibling listener",
        &lb_arn,
        &tg_arn,
        vec![p("Protocol", "HTTP"), p("Port", "8080")],
    )
    .await;

    let unknown_group = format!(
        "arn:aws:elasticloadbalancing:{REGION}:{}:targetgroup/nowhere/0123456789abcdef",
        account()
    );
    let rows: Vec<(&str, Form, &str)> = vec![
        (
            // "If you change the protocol from HTTP to HTTPS ... you must add the
            // security policy and default certificate properties."
            "a protocol that terminates TLS with no certificate",
            vec![p("Protocol", "HTTPS")],
            VALIDATION_ERROR,
        ),
        (
            "a protocol that terminates TLS with two certificates",
            vec![
                p("Protocol", "HTTPS"),
                p("Certificates.member.1.CertificateArn", CERT_ONE),
                p("Certificates.member.2.CertificateArn", CERT_TWO),
            ],
            TOO_MANY_CERTIFICATES,
        ),
        (
            "a certificate on a listener that terminates no TLS",
            vec![p("Certificates.member.1.CertificateArn", CERT_ONE)],
            VALIDATION_ERROR,
        ),
        (
            "a certificate entry carrying no CertificateArn",
            vec![
                p("Protocol", "HTTPS"),
                p("Certificates.member.1.IsDefault", "true"),
            ],
            VALIDATION_ERROR,
        ),
        (
            "a port the load balancer already listens on",
            vec![p("Port", "8080")],
            DUPLICATE_LISTENER,
        ),
        (
            "a default action forwarding to a target group that does not exist",
            vec![
                p("DefaultActions.member.1.Type", "forward"),
                p("DefaultActions.member.1.TargetGroupArn", &unknown_group),
            ],
            TARGET_GROUP_NOT_FOUND,
        ),
    ];

    for (case, fields, expected_code) in rows {
        let mut form = action("ModifyListener");
        form.push(p("ListenerArn", &listener));
        form.extend(fields);
        let reply = call(&router, &form).await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (400, expected_code),
            "case {case}: expected 400 {expected_code}, got {} {}",
            reply.status,
            reply.body
        );

        // A refused modification stored nothing: the listener is still the HTTP/80 one.
        let described = describe_listener(&router, &listener).await;
        let reported = members(&described.body, "Listeners");
        assert_eq!(
            (
                first_text(&reported[0], "Protocol").as_deref(),
                first_text(&reported[0], "Port").as_deref()
            ),
            (Some("HTTP"), Some("80")),
            "case {case}: a refused modification must leave the listener as it was, got {}",
            reported[0]
        );
    }
}

/// Rule 4: the certificate list round trips, and the default certificate is not part of
/// it.
#[tokio::test]
async fn the_certificate_list_round_trips_through_add_describe_and_remove() {
    let (router, _engine) = emulator();
    let lb_arn = create_load_balancer(&router, "certificate-list-alb", "application").await;
    let tg_arn = create_http_target_group(&router, "certificate-list-tg").await;
    let listener = create_listener(
        &router,
        "an HTTPS listener",
        &lb_arn,
        &tg_arn,
        vec![
            p("Protocol", "HTTPS"),
            p("Port", "443"),
            p("Certificates.member.1.CertificateArn", CERT_DEFAULT),
        ],
    )
    .await;
    let plain = create_listener(
        &router,
        "an HTTP listener",
        &lb_arn,
        &tg_arn,
        vec![p("Protocol", "HTTP"), p("Port", "80")],
    )
    .await;

    // Before anything is added, the listener describes its default certificate alone,
    // flagged as the default — "this value ... is included when describing listener
    // certificates".
    let described = describe_certificates(&router, &listener, None, None).await;
    assert_eq!(
        certificate_rows(&described.body),
        vec![(CERT_DEFAULT.to_string(), Some("true".to_string()))],
        "case the fresh listener: expected its default certificate alone, got {}",
        described.body
    );

    // Rows differ along which certificate one call adds and whether it is already in the
    // list: adding a certificate twice leaves one entry, because the list is a set. One
    // call adds ONE certificate — `Certificates` is "The certificate to add. You can
    // specify one certificate per call" — so growing the list is a call per certificate.
    let rows: Vec<(&str, Vec<&str>, Vec<&str>)> = vec![
        ("one certificate", vec![CERT_ONE], vec![CERT_ONE]),
        (
            "the same certificate again",
            vec![CERT_ONE],
            vec![CERT_ONE],
        ),
        ("a second one", vec![CERT_TWO], vec![CERT_ONE, CERT_TWO]),
        (
            "a third one",
            vec![CERT_THREE],
            vec![CERT_ONE, CERT_TWO, CERT_THREE],
        ),
        (
            "the listener's own default certificate",
            vec![CERT_DEFAULT],
            vec![CERT_ONE, CERT_TWO, CERT_THREE, CERT_DEFAULT],
        ),
    ];

    for (case, adding, expected_list) in rows {
        let mut form = action("AddListenerCertificates");
        form.push(p("ListenerArn", &listener));
        for (index, arn) in adding.iter().enumerate() {
            form.push(p(
                &format!("Certificates.member.{}.CertificateArn", index + 1),
                arn,
            ));
        }
        let reply = call(&router, &form).await;
        assert_eq!(
            reply.status, 200,
            "case adding {case}: expected 200, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            certificate_rows(&reply.body)
                .into_iter()
                .map(|(arn, _)| arn)
                .collect::<Vec<String>>(),
            expected_list,
            "case adding {case}: unexpected certificate list in the answer {}",
            reply.body
        );

        // And the read reports the default certificate first, then the list, each
        // flagged as what it is. A certificate that is both — the last row — appears
        // once as each.
        let described = describe_certificates(&router, &listener, None, None).await;
        let mut expected_rows = vec![(CERT_DEFAULT.to_string(), Some("true".to_string()))];
        expected_rows.extend(
            expected_list
                .iter()
                .map(|arn| (arn.to_string(), Some("false".to_string()))),
        );
        assert_eq!(
            certificate_rows(&described.body),
            expected_rows,
            "case adding {case}: unexpected described certificates in {}",
            described.body
        );
    }

    // One page at a time walks the same certificates: `PageSize` of one, resumed by the
    // marker it answers.
    let mut walked: Vec<String> = Vec::new();
    let mut marker: Option<String> = None;
    loop {
        let page = describe_certificates(&router, &listener, Some(1), marker.as_deref()).await;
        let rows = certificate_rows(&page.body);
        assert_eq!(
            rows.len(),
            1,
            "case paging the certificate list: expected one certificate per page, got {}",
            page.body
        );
        walked.push(rows[0].0.clone());
        marker = page.text("NextMarker");
        if marker.is_none() {
            break;
        }
    }
    let described = describe_certificates(&router, &listener, None, None).await;
    assert_eq!(
        walked,
        certificate_rows(&described.body)
            .into_iter()
            .map(|(arn, _)| arn)
            .collect::<Vec<String>>(),
        "case paging the certificate list: the pages must walk the whole list in order, got {}",
        described.body
    );

    // Removing a certificate takes it out of the list; removing one the list no longer
    // holds leaves the list exactly as it was.
    for (case, removing) in [
        ("a certificate in the list", CERT_ONE),
        ("a certificate the list no longer holds", CERT_ONE),
    ] {
        let mut form = action("RemoveListenerCertificates");
        form.push(p("ListenerArn", &listener));
        form.push(p("Certificates.member.1.CertificateArn", removing));
        let reply = call(&router, &form).await;
        assert_eq!(
            reply.status, 200,
            "case removing {case}: expected 200, got {} {}",
            reply.status, reply.body
        );
        // Both rows leave the same list: the one the add rows built, minus CERT_ONE.
        let mut expected_rows = vec![(CERT_DEFAULT.to_string(), Some("true".to_string()))];
        expected_rows.extend(
            [CERT_TWO, CERT_THREE, CERT_DEFAULT]
                .iter()
                .map(|arn| (arn.to_string(), Some("false".to_string()))),
        );
        let described = describe_certificates(&router, &listener, None, None).await;
        assert_eq!(
            certificate_rows(&described.body),
            expected_rows,
            "case removing {case}: unexpected certificates in {}",
            described.body
        );
    }

    // The default certificate is in the certificate LIST too (a row above added it
    // there), and a list entry is removable: what cannot be removed is the listener's
    // default certificate itself.
    let mut form = action("RemoveListenerCertificates");
    form.push(p("ListenerArn", &listener));
    form.push(p("Certificates.member.1.CertificateArn", CERT_DEFAULT));
    let reply = call(&router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case removing the default certificate's list entry: expected 200, got {} {}",
        reply.status, reply.body
    );
    let described = describe_certificates(&router, &listener, None, None).await;
    assert_eq!(
        certificate_rows(&described.body),
        vec![
            (CERT_DEFAULT.to_string(), Some("true".to_string())),
            (CERT_TWO.to_string(), Some("false".to_string())),
            (CERT_THREE.to_string(), Some("false".to_string())),
        ],
        "case removing the default certificate's list entry: it must stay the default and \
         leave the list, got {}",
        described.body
    );

    // Now it is the default certificate alone, and removing THAT is refused: "You can't
    // remove the default certificate for a listener."
    let before = describe_certificates(&router, &listener, None, None).await;
    let reply = call(&router, &form).await;
    assert_eq!(
        (reply.status, reply.error_code().as_str()),
        (400, OPERATION_NOT_PERMITTED),
        "case removing the default certificate: expected 400 {OPERATION_NOT_PERMITTED}, got {} {}",
        reply.status,
        reply.body
    );
    let after = describe_certificates(&router, &listener, None, None).await;
    assert_eq!(
        certificate_rows(&after.body),
        certificate_rows(&before.body),
        "case removing the default certificate: the refusal must change nothing, got {}",
        after.body
    );

    // The certificate list belongs to the protocols the model brackets certificates for.
    let mut form = action("AddListenerCertificates");
    form.push(p("ListenerArn", &plain));
    form.push(p("Certificates.member.1.CertificateArn", CERT_ONE));
    let reply = call(&router, &form).await;
    assert_eq!(
        (reply.status, reply.error_code().as_str()),
        (400, VALIDATION_ERROR),
        "case adding a certificate to an HTTP listener: expected 400 {VALIDATION_ERROR}, got {} {}",
        reply.status,
        reply.body
    );

    // One call cannot name more certificates than it may carry, and the bound is read off
    // the list the request carried before anything is stored, so neither operation can be
    // made to do work that grows with the request. `AddListenerCertificates` takes one
    // certificate per call and `RemoveListenerCertificates` at most a whole list (25
    // here), so 26 is past both.
    let before = describe_certificates(&router, &listener, None, None).await;
    for (case, operation, expected_code) in [
        ("adding", "AddListenerCertificates", VALIDATION_ERROR),
        ("removing", "RemoveListenerCertificates", VALIDATION_ERROR),
    ] {
        let mut form = action(operation);
        form.push(p("ListenerArn", &listener));
        for index in 0..26 {
            form.push(p(
                &format!("Certificates.member.{}.CertificateArn", index + 1),
                &format!("{CERT_ONE}-{index}"),
            ));
        }
        let reply = call(&router, &form).await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (400, expected_code),
            "case {case} 26 certificates in one call: expected 400 {expected_code}, got {} {}",
            reply.status,
            reply.body
        );
        let after = describe_certificates(&router, &listener, None, None).await;
        assert_eq!(
            certificate_rows(&after.body),
            certificate_rows(&before.body),
            "case {case} 26 certificates in one call: the refusal must change nothing, got {}",
            after.body
        );
    }

    // And a described LISTENER still reports its default certificate only — the
    // certificate list is `DescribeListenerCertificates`'s to report.
    let described = describe_listener(&router, &listener).await;
    let reported = members(&described.body, "Listeners");
    assert_eq!(
        certificate_rows(&reported[0])
            .into_iter()
            .map(|(arn, _)| arn)
            .collect::<Vec<String>>(),
        vec![CERT_DEFAULT.to_string()],
        "case describing the listener: expected its default certificate alone, got {}",
        reported[0]
    );
}

/// One load balancer family under test: the `Type` to create, the listener fields its
/// listener takes, and the attributes a fresh listener of it must report.
struct Family {
    lb_type: &'static str,
    listener: Vec<(String, String)>,
    defaults: &'static [(&'static str, &'static str)],
}

/// Rule 5: a listener's attributes are the ones its family carries, defaulted as their
/// documentation states, and only a value in the documented domain is stored.
///
/// The families are the whole `LoadBalancerTypeEnum` (`application`, `network`,
/// `gateway`), and the write rows cross them with the keys the model's
/// `ListenerAttribute$Key` documentation lists — including the published bounds of the
/// one attribute it gives a range for and a value either side of them, and attributes
/// the issue did not cite.
#[tokio::test]
async fn listener_attributes_default_and_accept_only_their_documented_domain() {
    let (router, _engine) = emulator();
    let tg_arn = create_http_target_group(&router, "listener-attributes-tg").await;

    let families = [
        Family {
            lb_type: "application",
            listener: vec![p("Protocol", "HTTP"), p("Port", "80")],
            // An Application Load Balancer listener's attribute collection is the
            // response-header set: the server header, which the listener allows until a
            // caller removes it, and the ten header values, which the collection carries
            // at the empty string. The mutual-TLS request header NAMES are not in it.
            defaults: &[
                ("routing.http.response.server.enabled", "true"),
                ("routing.http.response.strict_transport_security.header_value", ""),
                (
                    "routing.http.response.access_control_allow_origin.header_value",
                    "",
                ),
                (
                    "routing.http.response.access_control_allow_methods.header_value",
                    "",
                ),
                (
                    "routing.http.response.access_control_allow_headers.header_value",
                    "",
                ),
                (
                    "routing.http.response.access_control_allow_credentials.header_value",
                    "",
                ),
                (
                    "routing.http.response.access_control_expose_headers.header_value",
                    "",
                ),
                ("routing.http.response.access_control_max_age.header_value", ""),
                ("routing.http.response.content_security_policy.header_value", ""),
                ("routing.http.response.x_content_type_options.header_value", ""),
                ("routing.http.response.x_frame_options.header_value", ""),
            ],
        },
        Family {
            lb_type: "network",
            listener: vec![p("Protocol", "TCP"), p("Port", "80")],
            // "tcp.idle_timeout.seconds ... The default is 350 seconds."
            defaults: &[("tcp.idle_timeout.seconds", "350")],
        },
        Family {
            lb_type: "gateway",
            // "You can't specify a protocol for a Gateway Load Balancer."
            listener: Vec::new(),
            defaults: &[
                ("send_tcp_reset.on_idle_timeout.enabled", "false"),
                ("tcp.idle_timeout.seconds", "350"),
            ],
        },
    ];

    let mut listeners: BTreeMap<&str, String> = BTreeMap::new();
    for family in &families {
        let lb_arn = create_load_balancer(
            &router,
            &format!("attributes-{}-lb", family.lb_type),
            family.lb_type,
        )
        .await;
        let listener = create_listener(
            &router,
            &format!("the {} listener", family.lb_type),
            &lb_arn,
            &tg_arn,
            family.listener.clone(),
        )
        .await;
        let (reply, reported) = describe_attributes(&router, &listener).await;
        assert_eq!(
            reply.status, 200,
            "case {} defaults: expected 200, got {} {}",
            family.lb_type, reply.status, reply.body
        );
        let expected: KeyValues = family
            .defaults
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        assert_eq!(
            reported, expected,
            "case {} defaults: unexpected attributes in {}",
            family.lb_type, reply.body
        );
        listeners.insert(family.lb_type, listener);
    }

    // Rows differ along the family, the key and the value: the documented bounds of the
    // one ranged attribute and a value either side of them, a boolean inside and outside
    // its two values, header-name and header-value attributes the issue did not cite, an
    // attribute of another family, and a key no attribute carries. The last column is the
    // awsQuery code the row is refused under, empty for a row that is stored: a value
    // outside an attribute's domain, and an attribute this listener's family does not
    // carry, are configurations this listener cannot hold, while a key the service defines
    // no attribute under is an input value the request has to fix.
    let rows: Vec<(&str, &str, &str, &str)> = vec![
        ("network", "tcp.idle_timeout.seconds", "60", ""),
        ("network", "tcp.idle_timeout.seconds", "6000", ""),
        (
            "network",
            "tcp.idle_timeout.seconds",
            "59",
            INVALID_CONFIGURATION,
        ),
        (
            "network",
            "tcp.idle_timeout.seconds",
            "6001",
            INVALID_CONFIGURATION,
        ),
        (
            "network",
            "tcp.idle_timeout.seconds",
            "not-a-number",
            INVALID_CONFIGURATION,
        ),
        (
            "gateway",
            "send_tcp_reset.on_idle_timeout.enabled",
            "true",
            "",
        ),
        (
            "gateway",
            "send_tcp_reset.on_idle_timeout.enabled",
            "yes",
            INVALID_CONFIGURATION,
        ),
        (
            "application",
            "routing.http.response.server.enabled",
            "false",
            "",
        ),
        (
            "application",
            "routing.http.request.x_amzn_mtls_clientcert_subject.header_name",
            "X-Client-Subject",
            "",
        ),
        (
            "application",
            "routing.http.response.x_frame_options.header_value",
            "DENY",
            "",
        ),
        // The ranged attribute belongs to the Network and Gateway families only, so an
        // Application Load Balancer's listener does not carry it...
        (
            "application",
            "tcp.idle_timeout.seconds",
            "350",
            INVALID_CONFIGURATION,
        ),
        // ...and the Application-only routing attributes are not a Network listener's.
        (
            "network",
            "routing.http.response.server.enabled",
            "true",
            INVALID_CONFIGURATION,
        ),
        // A Gateway-only attribute on a Network listener, which carries the other one.
        (
            "network",
            "send_tcp_reset.on_idle_timeout.enabled",
            "true",
            INVALID_CONFIGURATION,
        ),
        (
            "application",
            "not.a.listener.attribute",
            "1",
            VALIDATION_ERROR,
        ),
    ];

    for (lb_type, key, value, code) in rows {
        let case = format!("{lb_type} listener setting {key} to {value:?}");
        let listener = listeners
            .get(lb_type)
            .unwrap_or_else(|| panic!("case {case}: no {lb_type} listener"));
        let (_, before) = describe_attributes(&router, listener).await;

        let mut form = action("ModifyListenerAttributes");
        form.push(p("ListenerArn", listener));
        form.push(p("Attributes.member.1.Key", key));
        form.push(p("Attributes.member.1.Value", value));
        let reply = call(&router, &form).await;

        if code.is_empty() {
            assert_eq!(
                reply.status, 200,
                "case {case}: expected the attribute to be stored, got {} {}",
                reply.status, reply.body
            );
            assert_eq!(
                key_values(&reply.body, "Attributes").get(key).cloned(),
                Some(value.to_string()),
                "case {case}: the answer must report the attribute it modified, got {}",
                reply.body
            );
            let (_, after) = describe_attributes(&router, listener).await;
            assert_eq!(
                after.get(key).cloned(),
                Some(value.to_string()),
                "case {case}: the stored attribute must read back, got {after:?}"
            );
            // Every other attribute is untouched.
            let others_before: KeyValues = before
                .iter()
                .filter(|(stored, _)| stored.as_str() != key)
                .map(|(stored, value)| (stored.clone(), value.clone()))
                .collect();
            let others_after: KeyValues = after
                .iter()
                .filter(|(stored, _)| stored.as_str() != key)
                .map(|(stored, value)| (stored.clone(), value.clone()))
                .collect();
            assert_eq!(
                others_before, others_after,
                "case {case}: the write must change no other attribute, got {others_after:?}"
            );
        } else {
            assert_eq!(
                (reply.status, reply.error_code().as_str()),
                (400, code),
                "case {case}: expected 400 {code}, got {} {}",
                reply.status,
                reply.body
            );
            let (_, after) = describe_attributes(&router, listener).await;
            assert_eq!(
                before, after,
                "case {case}: a refused write must store nothing, got {after:?}"
            );
        }
    }
}

/// Rule 6: every listener has exactly one default rule, and it carries the listener's
/// default actions.
#[tokio::test]
async fn describe_rules_reports_the_listeners_default_rule() {
    let (router, _engine) = emulator();
    let lb_arn = create_load_balancer(&router, "rules-alb", "application").await;
    let groups = [
        create_http_target_group(&router, "rules-tg-1").await,
        create_http_target_group(&router, "rules-tg-2").await,
    ];
    let first = create_listener(
        &router,
        "the first listener",
        &lb_arn,
        &groups[0],
        vec![p("Protocol", "HTTP"), p("Port", "80")],
    )
    .await;
    let second = create_listener(
        &router,
        "the second listener",
        &lb_arn,
        &groups[1],
        vec![p("Protocol", "HTTP"), p("Port", "8080")],
    )
    .await;

    /// The one rule a listener's rules read must report, and the ARN it carries.
    async fn only_rule(router: &MultiServiceRouter, case: &str, listener: &str) -> String {
        let mut form = action("DescribeRules");
        form.push(p("ListenerArn", listener));
        let reply = call(router, &form).await;
        assert_eq!(
            reply.status, 200,
            "case {case}: expected 200, got {} {}",
            reply.status, reply.body
        );
        let rules = members(&reply.body, "Rules");
        assert_eq!(
            rules.len(),
            1,
            "case {case}: a listener nothing has added a rule to has its default rule alone, got {}",
            reply.body
        );
        let rule = &rules[0];
        assert_eq!(
            (
                first_text(rule, "IsDefault").as_deref(),
                first_text(rule, "Priority").as_deref()
            ),
            (Some("true"), Some("default")),
            "case {case}: the default rule is flagged and prioritized as the default, got {rule}"
        );
        assert!(
            first_text(rule, "Conditions").is_none(),
            "case {case}: the default rule matches on no condition, got {rule}"
        );
        let arn = first_text(rule, "RuleArn")
            .unwrap_or_else(|| panic!("case {case}: no RuleArn in {rule}"));
        let expected_prefix = format!("{}/", listener.replacen(":listener/", ":listener-rule/", 1));
        assert!(
            arn.starts_with(&expected_prefix),
            "case {case}: a rule of this listener has the ARN prefix {expected_prefix}, got {arn}"
        );
        assert!(
            arn.strip_prefix(&expected_prefix)
                .is_some_and(|id| id.len() == 16 && id.chars().all(|c| c.is_ascii_hexdigit())),
            "case {case}: a rule id is 16 hex characters, got {arn}"
        );
        arn
    }

    let first_rule = only_rule(&router, "the first listener's rules", &first).await;
    let second_rule = only_rule(&router, "the second listener's rules", &second).await;
    assert_ne!(
        first_rule, second_rule,
        "case two listeners: each default rule is its own, got {first_rule} twice"
    );
    assert_eq!(
        only_rule(&router, "the first listener's rules again", &first).await,
        first_rule,
        "case reading twice: a rule ARN is stable across reads, got a different one"
    );

    // The default rule's actions ARE the listener's default actions, which is why they
    // follow a modification of them.
    let mut form = action("DescribeRules");
    form.push(p("RuleArns.member.1", &first_rule));
    let by_arn = call(&router, &form).await;
    assert_eq!(
        by_arn.status, 200,
        "case selecting the rule by ARN: expected 200, got {} {}",
        by_arn.status, by_arn.body
    );
    assert!(
        by_arn.body.contains(groups[0].as_str()),
        "case selecting the rule by ARN: its action forwards to the listener's target group, got {}",
        by_arn.body
    );

    let mut form = action("ModifyListener");
    form.push(p("ListenerArn", &first));
    form.push(p("DefaultActions.member.1.Type", "forward"));
    form.push(p("DefaultActions.member.1.TargetGroupArn", &groups[1]));
    let modified = call(&router, &form).await;
    assert_eq!(
        modified.status, 200,
        "case modifying the default actions: expected 200, got {} {}",
        modified.status, modified.body
    );
    let mut form = action("DescribeRules");
    form.push(p("ListenerArn", &first));
    let after = call(&router, &form).await;
    assert!(
        after.body.contains(groups[1].as_str()) && !after.body.contains(groups[0].as_str()),
        "case after modifying the default actions: the default rule must carry them, got {}",
        after.body
    );

    // A rule ARN naming no rule, and a request naming neither a listener nor rules —
    // "You must specify either a listener or rules."
    let unknown_rule = format!("{first_rule}0");
    let mut form = action("DescribeRules");
    form.push(p("RuleArns.member.1", &unknown_rule));
    let missing = call(&router, &form).await;
    assert_eq!(
        (missing.status, missing.error_code().as_str()),
        (400, RULE_NOT_FOUND),
        "case a rule ARN naming no rule: expected 400 {RULE_NOT_FOUND}, got {} {}",
        missing.status,
        missing.body
    );

    let neither = call(&router, &action("DescribeRules")).await;
    assert_eq!(
        (neither.status, neither.error_code().as_str()),
        (400, VALIDATION_ERROR),
        "case naming neither a listener nor rules: expected 400 {VALIDATION_ERROR}, got {} {}",
        neither.status,
        neither.body
    );

    // The selector is bounded and deduplicated before anything is resolved, so what one
    // request can ask this read to do does not grow with the list it carries. The rows
    // sit either side of the bound (AWS publishes 100 rules per Application Load
    // Balancer), read off the list the request CARRIED: 100 entries reach the lookup —
    // and are refused on the rules, not on the selector — 101 are refused as a selector
    // however many distinct rules they name, and a selector of 100 repeats of the one
    // rule that exists resolves it once.
    for (case, entries, repeat_the_existing_rule, expected_code) in [
        ("one rule repeated 100 times", 100usize, true, ""),
        ("one rule repeated 101 times", 101, true, VALIDATION_ERROR),
        ("100 absent rule ARNs", 100, false, RULE_NOT_FOUND),
        ("101 absent rule ARNs", 101, false, VALIDATION_ERROR),
    ] {
        let mut form = action("DescribeRules");
        for index in 0..entries {
            let arn = if repeat_the_existing_rule {
                first_rule.clone()
            } else {
                // A well-formed rule ARN of this listener that no rule carries: the
                // stored rule id with its last four characters replaced.
                format!("{}{index:04x}", &first_rule[..first_rule.len() - 4])
            };
            form.push(p(&format!("RuleArns.member.{}", index + 1), &arn));
        }
        let reply = call(&router, &form).await;
        assert_eq!(
            reply.error_code(),
            expected_code,
            "case {case}: unexpected error code in {} {}",
            reply.status,
            reply.body
        );
        if expected_code.is_empty() {
            assert_eq!(
                members(&reply.body, "Rules").len(),
                1,
                "case {case}: a repeated ARN names one rule and is reported once, got {}",
                reply.body
            );
        } else {
            assert_eq!(
                reply.status, 400,
                "case {case}: expected 400, got {} {}",
                reply.status, reply.body
            );
        }
    }
}
