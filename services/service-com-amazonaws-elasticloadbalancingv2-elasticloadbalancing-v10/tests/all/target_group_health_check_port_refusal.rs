// SPDX-License-Identifier: CC0-1.0

//! The WORDING of the refusal a health-check port outside the accepted range earns, over
//! the wire, on every operation that accepts `HealthCheckPort`.
//!
//! `all/target_group_health_check_port.rs` pins WHICH values the service accepts; this
//! suite pins what it SAYS when it refuses one, because `ValidationError` is the single
//! code Elastic Load Balancing answers every service-side input rule of these operations
//! with, so the message is the only part of the answer that tells a caller which rule it
//! broke. The template is therefore a requirement, not a convenience:
//!
//! ```text
//! Health check port '<the refused value>' must be within '1-65535' or 'traffic-port'
//! ```
//!
//! — the refused value echoed in single quotes, then the bound, then the literal
//! default, in that order. Its one variable part is the value the REQUEST carried, so
//! every row below drives a different one through it rather than restating one run's.
//! The bound is the range the model puts on every port member it does type as an integer
//! (`smithy_query(query="shape",shape="Port")` -> `range 1..65535`); the literal is the
//! one `HealthCheckPort`'s own documentation names as its default ("traffic-port, which
//! is the port on which each target receives traffic from the load balancer").
//!
//! The wire harness (`emulator`, `call`, `action`, `p`) is the one `provisioning_loop`
//! owns, and the target-group helpers are the ones
//! `all/target_group_health_check_port.rs` owns; nothing here restates either.

use crate::provisioning_loop::{action, call, emulator, p, Reply};
use crate::target_group_attributes_and_modification::unescaped;
use crate::target_group_health_check_port::{
    create_group, describe, health_check_of, http_group_fields,
};

/// One row: the case name and the `HealthCheckPort` value the request carries.
type RefusedPort = (&'static str, &'static str);

/// Every KIND of value that is no health-check port: on each side of the published
/// bounds, far past the high one, a negative, a port written in a form the service does
/// not read as a number, and values that are no number at all — including a near miss of
/// the literal default, which is accepted only spelled exactly.
const REFUSED_PORTS: &[RefusedPort] = &[
    ("one under the low bound", "0"),
    ("one past the high bound", "65536"),
    ("far past the high bound", "70000"),
    ("past the widest port a number can name", "4294967296"),
    ("negative", "-1"),
    ("a port with a decimal point", "80.0"),
    ("a port with surrounding space", " 80 "),
    ("a port range", "8080-8081"),
    ("a protocol name", "http"),
    ("a near miss of the literal default", "traffic_port"),
    ("the literal default in other case", "TRAFFIC-PORT"),
];

/// The message Elastic Load Balancing answers a health-check port outside its rule with,
/// built the way the service builds it: the fixed text, with the value the request
/// carried in the one variable slot.
fn expected_refusal(value: &str) -> String {
    format!("Health check port '{value}' must be within '1-65535' or 'traffic-port'")
}

/// The status, code and message of a refusal, as a caller reads them.
fn refusal(reply: &Reply) -> (u16, String, String) {
    (
        reply.status,
        reply.error_code(),
        unescaped(&reply.text("Message").unwrap_or_default()),
    )
}

#[tokio::test]
async fn modify_target_group_refuses_a_health_check_port_in_aws_words() {
    let (router, _engine) = emulator();

    for (index, (case, value)) in REFUSED_PORTS.iter().enumerate() {
        let name = format!("mtg-msg-{index}");
        let arn = create_group(&router, case, &name, &http_group_fields()).await;
        let before = health_check_of(&describe(&router, &arn).await);

        let mut form = action("ModifyTargetGroup");
        form.push(p("TargetGroupArn", &arn));
        form.push(p("HealthCheckPort", value));
        let reply = call(&router, &form).await;

        assert_eq!(
            refusal(&reply),
            (400, "ValidationError".to_string(), expected_refusal(value)),
            "case {case} ({value}): expected the published refusal, got {}",
            reply.body
        );
        // The refusal is raised before the first write, so the stored health check is
        // the one the create left — the claim the recorded check never reached.
        let described = describe(&router, &arn).await;
        assert_eq!(
            health_check_of(&described),
            before,
            "case {case} ({value}): a refused modify must change nothing, got {}",
            described.body
        );
    }
}

#[tokio::test]
async fn create_target_group_refuses_a_health_check_port_in_the_same_words() {
    // The rule, and so its wording, belongs to the MEMBER: the operation that first
    // accepts it refuses it exactly as the one that modifies it does.
    let (router, _engine) = emulator();

    for (index, (case, value)) in REFUSED_PORTS.iter().enumerate() {
        let mut form = action("CreateTargetGroup");
        form.push(p("Name", &format!("ctg-msg-{index}")));
        form.extend(http_group_fields());
        form.push(p("HealthCheckPort", value));
        let reply = call(&router, &form).await;

        assert_eq!(
            refusal(&reply),
            (400, "ValidationError".to_string(), expected_refusal(value)),
            "case {case} ({value}): expected the published refusal, got {}",
            reply.body
        );
    }
}

#[tokio::test]
async fn a_refused_health_check_port_leaves_the_literal_default_still_acceptable() {
    // The refusal is a verdict on ONE request, not on the group: the shape the recorded
    // check used — a TCP group health-checked over HTTP on a port of its own — keeps
    // taking the literal default afterwards, and reports it back as that literal rather
    // than as a number.
    let (router, _engine) = emulator();
    let fields = vec![
        p("Protocol", "TCP"),
        p("Port", "44300"),
        p("VpcId", "vpc-1"),
        p("TargetType", "instance"),
        p("HealthCheckProtocol", "HTTP"),
        p("HealthCheckPort", "4000"),
        p("HealthCheckPath", "/health-check"),
    ];
    let arn = create_group(&router, "the recorded shape", "tg-after-refusal", &fields).await;

    for (case, value) in REFUSED_PORTS {
        let mut form = action("ModifyTargetGroup");
        form.push(p("TargetGroupArn", &arn));
        form.push(p("HealthCheckPort", value));
        let reply = call(&router, &form).await;
        assert_eq!(
            refusal(&reply),
            (400, "ValidationError".to_string(), expected_refusal(value)),
            "case {case} ({value}): expected the published refusal, got {}",
            reply.body
        );
        assert_eq!(
            describe(&router, &arn).await.text("HealthCheckPort").as_deref(),
            Some("4000"),
            "case {case} ({value}): a refused modify must leave the stored port alone"
        );
    }

    let mut form = action("ModifyTargetGroup");
    form.push(p("TargetGroupArn", &arn));
    form.push(p("HealthCheckPort", "traffic-port"));
    let reply = call(&router, &form).await;
    assert_eq!(
        reply.status, 200,
        "the literal default must still be accepted after a refusal, got {} body {}",
        reply.status, reply.body
    );
    assert_eq!(
        describe(&router, &arn).await.text("HealthCheckPort").as_deref(),
        Some("traffic-port"),
        "the literal default must round-trip as itself"
    );
}
