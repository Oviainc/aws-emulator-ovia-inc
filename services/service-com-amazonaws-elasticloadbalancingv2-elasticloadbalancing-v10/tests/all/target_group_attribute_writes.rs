// SPDX-License-Identifier: CC0-1.0

//! What `ModifyTargetGroupAttributes` persists, and what it answers — over the wire,
//! through the router.
//!
//! The rule these tables pin: a write persists exactly the attributes the request names
//! onto the target group the ARN names, leaving every attribute it does not name at the
//! value it already had — or at its documented default — and answers with the attributes
//! it wrote, each carrying the value it stored, in the order the request named them; a
//! later `DescribeTargetGroupAttributes` reports the merged set, so the last write of a
//! key is what a read sees and re-sending a write changes nothing.
//!
//! The rows span what the rule generalises over: how many attributes one request
//! carries and in which order, which load balancer family's attributes a group carries
//! (`smithy_query(query="doc",shape="TargetGroupAttribute",member="Key")` is the only
//! catalog of them), and how repeated writes of one key compose.
//!
//! The domain of each attribute, the refusals a bad write answers and the defaults a
//! fresh group reports are pinned by `all/target_group_attributes_and_modification.rs`;
//! nothing here restates them. The wire harness (`emulator`, `call`, `action`, `p`,
//! `members`, `first_text`) and the group creator and attribute reader come from there
//! and from `all/provisioning_loop.rs`.

use crate::provisioning_loop::{action, call, emulator, first_text, members, p, Reply};
use crate::target_group_attributes_and_modification::{create_target_group, described_attributes};
use smithy_server_runtime::MultiServiceRouter;
use std::collections::BTreeMap;

/// One row of the echo table: the case name and the attributes one request writes, in
/// the order it names them.
type EchoCase = (&'static str, Vec<(&'static str, &'static str)>);

/// One row of the family table: the case name, the fields the target group is created
/// with, and the attributes written onto it.
type FamilyCase = (
    &'static str,
    Vec<(&'static str, &'static str)>,
    Vec<(&'static str, &'static str)>,
);

/// Write `writes` onto the target group `arn` names and answer the reply, whatever it
/// is. The form numbers the members in the order given, which is the order the reply's
/// `Attributes` list must come back in.
async fn write_attributes(
    router: &MultiServiceRouter,
    arn: &str,
    writes: &[(&str, &str)],
) -> Reply {
    let mut form = action("ModifyTargetGroupAttributes");
    form.push(p("TargetGroupArn", arn));
    for (index, (key, value)) in writes.iter().enumerate() {
        let position = index + 1;
        form.push(p(&format!("Attributes.member.{position}.Key"), key));
        form.push(p(&format!("Attributes.member.{position}.Value"), value));
    }
    call(router, &form).await
}

/// The `Attributes` list of `body` IN ORDER — unlike the map the sibling suite compares,
/// because the order a write answers in is part of what is asserted here.
fn attribute_sequence(body: &str) -> Vec<(String, String)> {
    members(body, "Attributes")
        .iter()
        .map(|member| {
            (
                first_text(member, "Key").unwrap_or_default(),
                first_text(member, "Value").unwrap_or_default(),
            )
        })
        .collect()
}

/// The pairs `writes` names, as the reply's list of them should read.
fn expected_sequence(writes: &[(&str, &str)]) -> Vec<(String, String)> {
    writes
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// `before` with `writes` applied, which is the whole set a later read must report.
fn merged(before: &BTreeMap<String, String>, writes: &[(&str, &str)]) -> BTreeMap<String, String> {
    let mut expected = before.clone();
    for (key, value) in writes {
        expected.insert((*key).to_string(), (*value).to_string());
    }
    expected
}

/// A write answers with exactly the attributes the request named, in the request's own
/// order, and a later read reports those values beside every attribute the request left
/// alone.
///
/// The rows differ in how many attributes one request carries and in which order it
/// names them — including the same three attributes in both orders, so the answer
/// follows the request rather than any order the store happens to hold.
#[tokio::test]
async fn a_write_answers_the_attributes_it_stored_in_the_order_the_request_named_them() {
    let three: Vec<(&str, &str)> = vec![
        ("deregistration_delay.timeout_seconds", "45"),
        ("load_balancing.algorithm.type", "weighted_random"),
        ("slow_start.duration_seconds", "120"),
    ];
    let mut reversed = three.clone();
    reversed.reverse();

    let cases: Vec<EchoCase> = vec![
        (
            "one attribute",
            vec![("stickiness.enabled", "true")],
        ),
        ("three attributes in catalog order", three),
        ("the same three in the opposite order", reversed),
        (
            "two attributes, one of them written to the value it already defaults to",
            vec![
                ("stickiness.type", "app_cookie"),
                ("stickiness.enabled", "false"),
            ],
        ),
        (
            "an attribute written on the upper bound of its documented range",
            vec![("deregistration_delay.timeout_seconds", "3600")],
        ),
    ];

    let (router, _engine) = emulator();

    for (index, (case, writes)) in cases.iter().enumerate() {
        // A fresh group per row, so one row's write never decides the next row's answer.
        let name = format!("attr-echo-{index}");
        let arn = create_target_group(
            &router,
            &name,
            &[("Protocol", "HTTP"), ("Port", "80"), ("VpcId", "vpc-1")],
        )
        .await;
        let before = described_attributes(&router, &arn).await;

        let reply = write_attributes(&router, &arn, writes).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected 200, got {} body {}",
            reply.status, reply.body
        );
        let answered = attribute_sequence(&reply.body);
        assert_eq!(
            answered,
            expected_sequence(writes),
            "case {case:?}: expected the written attributes answered in request order, got \
             {answered:?}"
        );

        let after = described_attributes(&router, &arn).await;
        assert_eq!(
            after,
            merged(&before, writes),
            "case {case:?}: expected the written values beside every untouched attribute, got \
             {after:?}"
        );
    }
}

/// A write lands on a target group of every load balancer family, for the attributes
/// that family's groups carry, and a read reports it back.
///
/// One row per family the attribute catalog names (Application, Network, Gateway) plus
/// the `lambda` target type, each writing an attribute the issue did not cite — the
/// refusal a family's own attribute gets on ANOTHER family's group is the sibling
/// suite's table.
#[tokio::test]
async fn a_write_lands_on_the_attributes_of_every_load_balancer_family() {
    let cases: Vec<FamilyCase> = vec![
        (
            "an HTTP instance target group (Application)",
            vec![("Protocol", "HTTP"), ("Port", "80"), ("VpcId", "vpc-1")],
            vec![
                ("load_balancing.algorithm.anomaly_mitigation", "on"),
                // The model documents no default for this one: a write is what makes a
                // read report it at all.
                ("stickiness.app_cookie.cookie_name", "session-id"),
            ],
        ),
        (
            "a TCP instance target group (Network)",
            vec![("Protocol", "TCP"), ("Port", "1234"), ("VpcId", "vpc-1")],
            vec![
                ("proxy_protocol_v2.enabled", "true"),
                // "The range is 0-360000 seconds."
                ("target_health_state.unhealthy.draining_interval_seconds", "360000"),
            ],
        ),
        (
            "a UDP ip target group (Network)",
            vec![
                ("Protocol", "UDP"),
                ("Port", "53"),
                ("VpcId", "vpc-1"),
                ("TargetType", "ip"),
            ],
            vec![
                ("preserve_client_ip.enabled", "false"),
                ("deregistration_delay.connection_termination.enabled", "false"),
            ],
        ),
        (
            "a GENEVE target group (Gateway)",
            vec![("Protocol", "GENEVE"), ("Port", "6081"), ("VpcId", "vpc-1")],
            vec![
                ("target_failover.on_deregistration", "rebalance"),
                ("target_failover.on_unhealthy", "rebalance"),
                ("send_tcp_reset.on_unhealthy.enabled", "true"),
            ],
        ),
        (
            "a lambda target group",
            vec![("TargetType", "lambda")],
            vec![("lambda.multi_value_headers.enabled", "true")],
        ),
    ];

    let (router, _engine) = emulator();

    for (index, (case, fields, writes)) in cases.iter().enumerate() {
        let name = format!("attr-family-{index}");
        let arn = create_target_group(&router, &name, fields).await;
        let before = described_attributes(&router, &arn).await;

        let reply = write_attributes(&router, &arn, writes).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: a family's own attribute must be writable, got {} body {}",
            reply.status, reply.body
        );
        let answered = attribute_sequence(&reply.body);
        assert_eq!(
            answered,
            expected_sequence(writes),
            "case {case:?}: expected the written attributes answered, got {answered:?}"
        );

        let after = described_attributes(&router, &arn).await;
        assert_eq!(
            after,
            merged(&before, writes),
            "case {case:?}: expected the written values read back, got {after:?}"
        );
    }
}

/// Writes compose: each one replaces only the keys it names, the LAST write of a key is
/// what a read reports, and re-sending a write a group already carries changes nothing.
#[tokio::test]
async fn repeated_writes_replace_only_the_keys_they_name_and_are_idempotent() {
    let (router, _engine) = emulator();
    let arn = create_target_group(
        &router,
        "attr-sequence",
        &[("Protocol", "HTTP"), ("Port", "80"), ("VpcId", "vpc-1")],
    )
    .await;

    let defaults = described_attributes(&router, &arn).await;
    assert_eq!(
        defaults
            .get("deregistration_delay.timeout_seconds")
            .map(String::as_str),
        Some("300"),
        "case setup: expected the documented default before any write, got {defaults:?}"
    );

    // (case, the attributes this step writes, the whole set the group must then report)
    let steps: Vec<(&str, Vec<(&str, &str)>)> = vec![
        (
            "the first write",
            vec![("deregistration_delay.timeout_seconds", "0")],
        ),
        ("another key", vec![("stickiness.enabled", "true")]),
        (
            "replacing the first key",
            vec![("deregistration_delay.timeout_seconds", "3600")],
        ),
        (
            "re-sending the replacement",
            vec![("deregistration_delay.timeout_seconds", "3600")],
        ),
    ];

    let mut expected = defaults.clone();
    for (case, writes) in &steps {
        let reply = write_attributes(&router, &arn, writes).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected 200, got {} body {}",
            reply.status, reply.body
        );
        expected = merged(&expected, writes);
        let after = described_attributes(&router, &arn).await;
        assert_eq!(
            after, expected,
            "case {case:?}: expected the cumulative set, got {after:?}"
        );
    }

    // Every key the group started with is still reported, and no key appeared: a write
    // replaces values, it never changes which attributes a read answers.
    let after = described_attributes(&router, &arn).await;
    let reported_keys: Vec<&String> = after.keys().collect();
    let original_keys: Vec<&String> = defaults.keys().collect();
    assert_eq!(
        reported_keys, original_keys,
        "case the whole sequence: expected the same attribute keys reported, got \
         {reported_keys:?} against {original_keys:?}"
    );
}
