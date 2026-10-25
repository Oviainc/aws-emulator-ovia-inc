// SPDX-License-Identifier: CC0-1.0

//! The VALUE rule of `HealthCheckPort`, over the wire, on every operation that accepts
//! the member.
//!
//! `HealthCheckPort` is modelled as a bare string
//! (`smithy_query(query="shape",shape="HealthCheckPort")` -> `{"type": "string"}`), so
//! the boundary validator passes anything and the handler is the only gate. The rule it
//! enforces: the value is the literal `traffic-port` the member's own documentation
//! names as its default ("traffic-port, which is the port on which each target receives
//! traffic from the load balancer"), or a port inside the range the model puts on every
//! port member it does type as an integer (`Port`, `TargetControlPort`: `range
//! 1..65535`). Anything else is a value the service cannot accept, so it answers HTTP
//! 400 `ValidationError` — never a 501, and never a silent success that stores a port no
//! load balancer could check.
//!
//! The rule holds wherever the member is accepted, so each row below runs against BOTH
//! `ModifyTargetGroup` and `CreateTargetGroup`, on each published bound, one past each,
//! and on values that are no port at all; and every refusal is read back, because a
//! refused call must store nothing.
//!
//! The wire harness (`emulator`, `call`, `action`, `p`) is the one `provisioning_loop`
//! already owns; nothing here restates it.

use crate::provisioning_loop::{action, call, emulator, p, Reply};
use crate::target_group_attributes_and_modification::unescaped;
use smithy_server_runtime::MultiServiceRouter;

/// One row: the case name, the `HealthCheckPort` value, and whether the service accepts
/// it.
type PortCase = (&'static str, &'static str, bool);

/// Every value the rule is tabled over: the literal default, each published bound, a
/// port in between that no recorded call cites, one under and several past the high
/// bound, and values that are no port at all.
const PORT_CASES: &[PortCase] = &[
    ("the literal default", "traffic-port", true),
    ("the low bound", "1", true),
    ("the high bound", "65535", true),
    ("a port the issue does not cite", "8080", true),
    ("one under the low bound", "0", false),
    ("one past the high bound", "65536", false),
    ("far past the high bound", "70000", false),
    ("negative", "-1", false),
    ("not a number", "http", false),
    ("a number with a suffix", "80x", false),
    ("a port range", "80-81", false),
];

/// The health-check members an HTTP/80 target group reports, as the element names the
/// awsQuery response carries — read before and after a refused call, because a refusal
/// must change none of them.
pub(crate) const HEALTH_CHECK_MEMBERS: &[&str] = &[
    "HealthCheckProtocol",
    "HealthCheckPort",
    "HealthCheckPath",
    "HealthCheckEnabled",
    "HealthCheckIntervalSeconds",
    "HealthCheckTimeoutSeconds",
    "HealthyThresholdCount",
    "UnhealthyThresholdCount",
    "HttpCode",
];

/// What a reply reports for each member of [`HEALTH_CHECK_MEMBERS`].
pub(crate) fn health_check_of(reply: &Reply) -> Vec<(&'static str, Option<String>)> {
    HEALTH_CHECK_MEMBERS
        .iter()
        .map(|member| (*member, reply.text(member)))
        .collect()
}

/// Create a target group and answer its ARN, failing the case it belongs to if the
/// create itself is refused.
pub(crate) async fn create_group(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    fields: &[(String, String)],
) -> String {
    let mut form = action("CreateTargetGroup");
    form.push(p("Name", name));
    form.extend(fields.iter().cloned());
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case {case}: expected the create to be accepted with 200, got {} body {}",
        reply.status, reply.body
    );
    reply
        .text("TargetGroupArn")
        .unwrap_or_else(|| panic!("case {case}: no TargetGroupArn in {}", reply.body))
}

/// The fields of an HTTP/80 target group, the shape every row starts from.
pub(crate) fn http_group_fields() -> Vec<(String, String)> {
    vec![p("Protocol", "HTTP"), p("Port", "80"), p("VpcId", "vpc-1")]
}

/// Read one target group back by ARN.
pub(crate) async fn describe(router: &MultiServiceRouter, arn: &str) -> Reply {
    let mut form = action("DescribeTargetGroups");
    form.push(p("TargetGroupArns.member.1", arn));
    call(router, &form).await
}

#[tokio::test]
async fn modify_target_group_accepts_a_health_check_port_only_by_the_published_rule() {
    let (router, _engine) = emulator();

    for (index, (case, value, accepted)) in PORT_CASES.iter().enumerate() {
        let name = format!("mtg-port-{index}");
        let arn = create_group(&router, case, &name, &http_group_fields()).await;
        let before = health_check_of(&describe(&router, &arn).await);

        let mut form = action("ModifyTargetGroup");
        form.push(p("TargetGroupArn", &arn));
        form.push(p("HealthCheckPort", value));
        let reply = call(&router, &form).await;
        let described = describe(&router, &arn).await;

        if *accepted {
            assert_eq!(
                reply.status, 200,
                "case {case} ({value}): a port inside the rule must be accepted, got {} body {}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.text("HealthCheckPort").as_deref(),
                Some(*value),
                "case {case} ({value}): the modify must answer the stored port, got {}",
                reply.body
            );
            assert_eq!(
                described.text("HealthCheckPort").as_deref(),
                Some(*value),
                "case {case} ({value}): expected the stored port to read back as {value}, got {}",
                described.body
            );
        } else {
            assert_eq!(
                (reply.status, reply.error_code().as_str()),
                (400, "ValidationError"),
                "case {case} ({value}): expected 400 ValidationError, got {} {}",
                reply.status,
                reply.body
            );
            // The refusal has to be actionable from its text alone, so it names the
            // value it refused and the published bounds the caller has to meet.
            let message = unescaped(&reply.text("Message").unwrap_or_default());
            assert!(
                message.contains(&format!("'{value}'")) && message.contains("'1-65535'"),
                "case {case} ({value}): the refusal must name the value and the bounds, \
                 got {message:?}"
            );
            assert_eq!(
                health_check_of(&described),
                before,
                "case {case} ({value}): a refused modify must change nothing, got {}",
                described.body
            );
        }
    }
}

#[tokio::test]
async fn create_target_group_accepts_a_health_check_port_only_by_the_published_rule() {
    let (router, _engine) = emulator();

    for (index, (case, value, accepted)) in PORT_CASES.iter().enumerate() {
        let name = format!("ctg-port-{index}");
        let mut fields = http_group_fields();
        fields.push(p("HealthCheckPort", value));

        let mut form = action("CreateTargetGroup");
        form.push(p("Name", &name));
        form.extend(fields);
        let reply = call(&router, &form).await;

        if *accepted {
            assert_eq!(
                reply.status, 200,
                "case {case} ({value}): a port inside the rule must be accepted, got {} body {}",
                reply.status, reply.body
            );
            let arn = reply
                .text("TargetGroupArn")
                .unwrap_or_else(|| panic!("case {case} ({value}): no ARN in {}", reply.body));
            let described = describe(&router, &arn).await;
            assert_eq!(
                described.text("HealthCheckPort").as_deref(),
                Some(*value),
                "case {case} ({value}): expected the stored port to read back as {value}, got {}",
                described.body
            );
        } else {
            assert_eq!(
                (reply.status, reply.error_code().as_str()),
                (400, "ValidationError"),
                "case {case} ({value}): expected 400 ValidationError, got {} {}",
                reply.status,
                reply.body
            );
            let message = unescaped(&reply.text("Message").unwrap_or_default());
            assert!(
                message.contains(&format!("'{value}'")) && message.contains("'1-65535'"),
                "case {case} ({value}): the refusal must name the value and the bounds, \
                 got {message:?}"
            );
            // Nothing may be persisted by a refused create.
            let mut form = action("DescribeTargetGroups");
            form.push(p("Names.member.1", &name));
            let described = call(&router, &form).await;
            assert_eq!(
                (described.status, described.error_code().as_str()),
                (400, "TargetGroupNotFound"),
                "case {case} ({value}): a refused create must store nothing, got {} {}",
                described.status,
                described.body
            );
        }
    }
}

#[tokio::test]
async fn an_accepted_health_check_port_survives_a_later_modify_of_another_member() {
    // The port is stored, not re-derived: a modify that names other members leaves it
    // exactly as the call that set it left it, whichever accepted form it took.
    let (router, _engine) = emulator();

    for (index, (case, value, accepted)) in PORT_CASES.iter().enumerate() {
        if !*accepted {
            continue;
        }
        let name = format!("mtg-keep-{index}");
        let arn = create_group(&router, case, &name, &http_group_fields()).await;

        for (step, fields) in [
            ("the port itself", vec![p("HealthCheckPort", value)]),
            (
                "another member",
                vec![p("HealthCheckIntervalSeconds", "17")],
            ),
            ("a third member", vec![p("HealthyThresholdCount", "3")]),
        ] {
            let mut form = action("ModifyTargetGroup");
            form.push(p("TargetGroupArn", &arn));
            form.extend(fields);
            let reply = call(&router, &form).await;
            assert_eq!(
                reply.status, 200,
                "case {case} ({value}) at {step}: expected 200, got {} body {}",
                reply.status, reply.body
            );
        }

        let described = describe(&router, &arn).await;
        assert_eq!(
            (
                described.text("HealthCheckPort").as_deref(),
                described.text("HealthCheckIntervalSeconds").as_deref(),
                described.text("HealthyThresholdCount").as_deref()
            ),
            (Some(*value), Some("17"), Some("3")),
            "case {case} ({value}): each modify must keep what the one before stored, got {}",
            described.body
        );
    }
}
