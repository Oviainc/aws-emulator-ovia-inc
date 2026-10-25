// SPDX-License-Identifier: CC0-1.0

//! The completeness rule of a layer-7 health check on the MODIFY path, over the wire.
//!
//! `HealthCheckPath` and `Matcher` are the two members the model documents for an
//! HTTP-semantic check alone (`HealthCheckPath`: "[HTTP/HTTPS health checks] The
//! destination for health checks on the targets"; `Matcher`: "[HTTP/HTTPS health checks]
//! The HTTP or gRPC codes to use when checking for a successful response from a
//! target"), and they are what makes such a check evaluable at all: something to request
//! and a return code that counts as success. The model ties neither to
//! `HealthCheckProtocol` — `Matcher` is an optional structure of two optional members
//! (`smithy_query(query="operation",operation="ModifyTargetGroup")`) — so the rule is
//! the service's and the handler is the only gate.
//!
//! The rule these tables pin: a `ModifyTargetGroup` whose RESULTING health check is a
//! layer-7 one must leave that check with both a path and a return code, carried by the
//! request or already stored, because a modify fills in no default for either; otherwise
//! the call is refused with HTTP 400 `ValidationError` and stores nothing. A modify whose
//! resulting check is not a layer-7 one is outside the rule.
//!
//! `CreateTargetGroup` is deliberately the other way round — it describes a whole target
//! group, so it fills both documented defaults — and the last test below pins that
//! asymmetry rather than leaving it implied.
//!
//! The wire harness (`emulator`, `call`, `action`, `p`) is `provisioning_loop`'s and the
//! target-group helpers are `target_group_health_check_port`'s; nothing here restates
//! either.

use crate::provisioning_loop::{action, call, emulator, p, Reply};
use crate::target_group_attributes_and_modification::unescaped;
use crate::target_group_health_check_port::{create_group, describe, health_check_of};

/// The starting target groups the table modifies, each one a different shape of stored
/// health check: a TCP check (which stores NEITHER http-only member), an HTTP check
/// (which stores both, from the create path's defaults), and an HTTP check of a gRPC
/// target group (whose stored return code is a `GrpcCode` rather than an `HttpCode`).
const START_GROUPS: &[(&str, &[(&str, &str)])] = &[
    ("tcp", &[("Protocol", "TCP"), ("Port", "1234"), ("VpcId", "vpc-1")]),
    ("http", &[("Protocol", "HTTP"), ("Port", "80"), ("VpcId", "vpc-1")]),
    (
        "grpc",
        &[
            ("Protocol", "HTTP"),
            ("ProtocolVersion", "GRPC"),
            ("Port", "80"),
            ("VpcId", "vpc-1"),
        ],
    ),
];

/// One row: the case name, the starting group it is asked of, the `ModifyTargetGroup`
/// fields, and whether the service accepts the modify.
type ModifyCase = (&'static str, &'static str, &'static [(&'static str, &'static str)], bool);

/// Every row the rule is tabled over. They differ along what the rule generalises over:
/// which http-only member the request supplies (neither, one, both), which form the
/// return code takes (`HttpCode`, `GrpcCode`), which layer-7 protocol the check ends up
/// speaking (HTTP, HTTPS), whether the missing member is already stored, and whether the
/// resulting check is a layer-7 one at all.
const MODIFY_CASES: &[ModifyCase] = &[
    // A TCP-probed group stores neither member, so every row below has to carry both.
    (
        "to HTTP with neither member",
        "tcp",
        &[("HealthCheckProtocol", "HTTP")],
        false,
    ),
    (
        "to HTTPS with neither member",
        "tcp",
        &[("HealthCheckProtocol", "HTTPS")],
        false,
    ),
    (
        "to HTTP with a path and no return code",
        "tcp",
        &[
            ("HealthCheckProtocol", "HTTP"),
            ("HealthCheckPath", "/health-check"),
        ],
        false,
    ),
    (
        "to HTTPS with a path and no return code",
        "tcp",
        &[("HealthCheckProtocol", "HTTPS"), ("HealthCheckPath", "/ping")],
        false,
    ),
    (
        "to HTTP with a return code and no path",
        "tcp",
        &[
            ("HealthCheckProtocol", "HTTP"),
            ("Matcher.HttpCode", "200-399"),
        ],
        false,
    ),
    (
        "to HTTP with both",
        "tcp",
        &[
            ("HealthCheckProtocol", "HTTP"),
            ("HealthCheckPath", "/health-check"),
            ("Matcher.HttpCode", "200-399"),
        ],
        true,
    ),
    (
        "to HTTPS with both",
        "tcp",
        &[
            ("HealthCheckProtocol", "HTTPS"),
            ("HealthCheckPath", "/ping"),
            ("Matcher.HttpCode", "200"),
        ],
        true,
    ),
    (
        // A gRPC code IS a return code: the model gives `Matcher` both members.
        "to HTTP with a path and a gRPC return code",
        "tcp",
        &[
            ("HealthCheckProtocol", "HTTP"),
            ("HealthCheckPath", "/package.service/method"),
            ("Matcher.GrpcCode", "0"),
        ],
        true,
    ),
    (
        // Outside the rule: the resulting check is not a layer-7 one.
        "staying a TCP check",
        "tcp",
        &[("HealthCheckProtocol", "TCP")],
        true,
    ),
    (
        "another member of a TCP check",
        "tcp",
        &[("HealthCheckIntervalSeconds", "20")],
        true,
    ),
    // A group whose check is already a layer-7 one stores both members, so a modify
    // that names neither still leaves a complete check.
    (
        "another member of an HTTP check",
        "http",
        &[("HealthCheckPort", "8080")],
        true,
    ),
    (
        "an HTTP check moving to HTTPS",
        "http",
        &[("HealthCheckProtocol", "HTTPS")],
        true,
    ),
    (
        "a new path on an HTTP check",
        "http",
        &[("HealthCheckPath", "/other")],
        true,
    ),
    (
        "a new return code on an HTTP check",
        "http",
        &[("Matcher.HttpCode", "200-299")],
        true,
    ),
    (
        // The stored return code is a `GrpcCode` here, and it counts the same.
        "an HTTP check of a gRPC group moving to HTTPS",
        "grpc",
        &[("HealthCheckProtocol", "HTTPS")],
        true,
    ),
    (
        "a new path on an HTTP check of a gRPC group",
        "grpc",
        &[("HealthCheckPath", "/pkg.svc/method")],
        true,
    ),
];

/// The fields of the starting group `kind`, as the create form carries them.
fn start_fields(kind: &str) -> Vec<(String, String)> {
    let (_, fields) = START_GROUPS
        .iter()
        .find(|(name, _)| *name == kind)
        .unwrap_or_else(|| panic!("case setup: no starting group named {kind}"));
    fields.iter().map(|(key, value)| p(key, value)).collect()
}

/// The path and the return code one reply reports for a health check — the two members
/// the rule is about.
fn layer_7_members(reply: &Reply) -> (Option<String>, Option<String>) {
    (
        reply.text("HealthCheckPath"),
        reply.text("HttpCode").or_else(|| reply.text("GrpcCode")),
    )
}

/// Whether the health check a reply reports speaks a layer-7 protocol. A target group
/// that reports no health-check protocol at all is a Lambda one, whose check is an HTTP
/// one all the same.
fn reports_a_layer_7_check(reply: &Reply) -> bool {
    matches!(
        reply.text("HealthCheckProtocol").as_deref(),
        None | Some("HTTP") | Some("HTTPS")
    )
}

#[tokio::test]
async fn a_modify_leaving_a_layer_7_check_requires_a_path_and_a_return_code() {
    let (router, _engine) = emulator();

    for (index, (case, kind, fields, accepted)) in MODIFY_CASES.iter().enumerate() {
        let name = format!("l7-{index}");
        let arn = create_group(&router, case, &name, &start_fields(kind)).await;
        let before = health_check_of(&describe(&router, &arn).await);

        let mut form = action("ModifyTargetGroup");
        form.push(p("TargetGroupArn", &arn));
        for (key, value) in fields.iter() {
            form.push(p(key, value));
        }
        let reply = call(&router, &form).await;
        let described = describe(&router, &arn).await;

        if *accepted {
            assert_eq!(
                reply.status, 200,
                "case {case:?}: a complete health check must be accepted, got {} body {}",
                reply.status, reply.body
            );
            // Every member the request named is the stored one.
            for (key, value) in fields.iter() {
                let element = key.rsplit('.').next().expect("a member names an element");
                assert_eq!(
                    described.text(element).as_deref(),
                    Some(*value),
                    "case {case:?}: expected {element} to read back as {value}, got {}",
                    described.body
                );
            }
            // And the invariant the rule exists for: no accepted modify leaves a
            // layer-7 check without a path or without a return code.
            if reports_a_layer_7_check(&described) {
                let (path, code) = layer_7_members(&described);
                assert!(
                    path.is_some() && code.is_some(),
                    "case {case:?}: a stored layer-7 check must report a path and a \
                     return code, got path {path:?} and code {code:?} in {}",
                    described.body
                );
            }
        } else {
            assert_eq!(
                (reply.status, reply.error_code().as_str()),
                (400, "ValidationError"),
                "case {case:?}: expected 400 ValidationError, got {} body {}",
                reply.status,
                reply.body
            );
            // The refusal has to be actionable from its text alone, so it names the two
            // members the caller has to supply.
            let message = unescaped(&reply.text("Message").unwrap_or_default());
            assert!(
                message.contains("Path") && message.contains("return code"),
                "case {case:?}: the refusal must name the members the check needs, got \
                 {message:?}"
            );
            assert_eq!(
                health_check_of(&described),
                before,
                "case {case:?}: a refused modify must change nothing, got {}",
                described.body
            );
        }
    }
}

#[tokio::test]
async fn the_create_path_fills_the_two_members_the_modify_path_requires() {
    // The asymmetry: a CREATE of a layer-7 check that names neither member is served and
    // stores the documented defaults, while the MODIFY of the same group to the same
    // check is refused — so a client reading a created group back never has to supply
    // what it did not set, and one changing a probe always does.
    let (router, _engine) = emulator();

    for (index, protocol) in ["HTTP", "HTTPS"].iter().enumerate() {
        let case = format!("a created {protocol} check");
        let name = format!("l7-create-{index}");
        let mut fields = start_fields("tcp");
        fields.push(p("HealthCheckProtocol", protocol));
        let arn = create_group(&router, &case, &name, &fields).await;

        let described = describe(&router, &arn).await;
        let (path, code) = layer_7_members(&described);
        assert!(
            path.is_some() && code.is_some(),
            "case {case}: a created layer-7 check must carry both documented defaults, \
             got path {path:?} and code {code:?} in {}",
            described.body
        );

        // The same check reached by a modify instead: refused, because a modify fills
        // neither default.
        let tcp_name = format!("l7-modify-{index}");
        let tcp_arn = create_group(&router, &case, &tcp_name, &start_fields("tcp")).await;
        let mut form = action("ModifyTargetGroup");
        form.push(p("TargetGroupArn", &tcp_arn));
        form.push(p("HealthCheckProtocol", protocol));
        let reply = call(&router, &form).await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (400, "ValidationError"),
            "case {case}: the modify to the same check must be refused, got {} body {}",
            reply.status,
            reply.body
        );
    }
}
