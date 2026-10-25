// SPDX-License-Identifier: CC0-1.0

//! What a created target group REPORTS for the members whose default the model documents
//! in prose rather than as a `@default` trait — the protocol version, and the address
//! type an `ip` target group carries its targets in.
//!
//! The rule under test (`src/ops/create_target_group.rs`): every member whose
//! documentation states a default has that default MATERIALIZED on the stored record at
//! create time, under the condition that documentation states for it, so the create's own
//! answer and every later read of the target group report what AWS reports. The two
//! members tabled here are the ones no other suite pins:
//!
//! * `ProtocolVersion` — "[HTTP/HTTPS protocol] The protocol version. … The default is
//!   `HTTP1`" (`smithy_query(query="doc",shape="CreateTargetGroupInput",
//!   member="ProtocolVersion")`). The bracketed prefix is a CONDITION: a target group
//!   whose protocol carries no HTTP semantics reports no protocol version at all.
//! * `IpAddressType` — "The IP address type. The default value is `ipv4`"
//!   (`member="IpAddressType"`), stated for every target group with no protocol or target
//!   type qualifying it.
//!
//! Every row is asserted on BOTH the create's answer and a later `DescribeTargetGroups`,
//! so a value the response writer computed rather than the create persisted cannot pass,
//! and then re-created: the model documents the operation as idempotent for "multiple
//! target groups with the same settings", which a materialized default must not break.
//!
//! The wire plumbing — the emulator under test, the request builder and the envelope
//! reader — is [`crate::wire`], shared with this crate's other suites rather than copied
//! per suite.

use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{action, call, p, router, Reply};

/// One ELBv2 awsQuery call.
async fn elbv2(router: &MultiServiceRouter, name: &str, params: &[(String, String)]) -> Reply {
    let mut pairs = action(name);
    pairs.extend_from_slice(params);
    call(router, &pairs).await
}

/// One row of the table: the members the create carries beyond `Name`, and the
/// `ProtocolVersion` / `IpAddressType` the stored target group must report.
#[derive(Debug, Clone)]
struct DefaultsRow {
    case: &'static str,
    params: Vec<(String, String)>,
    protocol_version: Option<&'static str>,
    ip_address_type: Option<&'static str>,
}

#[tokio::test]
async fn a_created_target_group_reports_the_defaults_the_model_documents() {
    let router = router();

    // The rows span the protocols the protocol-version default does and does not apply to
    // — `Protocol`'s own documentation pairs HTTP and HTTPS with an Application Load
    // Balancer, TCP, TLS, UDP, TCP_UDP and QUIC with a Network one and GENEVE with a
    // Gateway one, and this table carries one of each family, including the TCP_UDP and
    // GENEVE groups no recorded call cites — a protocol version and an address type
    // supplied explicitly, and the one target type that has no protocol at all.
    let rows: [DefaultsRow; 8] = [
        DefaultsRow {
            case: "an HTTP target group takes both defaults",
            params: vec![p("Protocol", "HTTP"), p("Port", "80"), p("VpcId", "vpc-1")],
            protocol_version: Some("HTTP1"),
            ip_address_type: Some("ipv4"),
        },
        DefaultsRow {
            case: "an HTTPS target group takes both defaults",
            params: vec![p("Protocol", "HTTPS"), p("Port", "443"), p("VpcId", "vpc-1")],
            protocol_version: Some("HTTP1"),
            ip_address_type: Some("ipv4"),
        },
        DefaultsRow {
            case: "an explicit protocol version wins over the default",
            params: vec![
                p("Protocol", "HTTP"),
                p("ProtocolVersion", "GRPC"),
                p("Port", "80"),
                p("VpcId", "vpc-1"),
            ],
            protocol_version: Some("GRPC"),
            ip_address_type: Some("ipv4"),
        },
        DefaultsRow {
            case: "an ip target group with an explicit address type",
            params: vec![
                p("Protocol", "HTTP"),
                p("Port", "80"),
                p("VpcId", "vpc-1"),
                p("TargetType", "ip"),
                p("IpAddressType", "ipv6"),
            ],
            protocol_version: Some("HTTP1"),
            ip_address_type: Some("ipv6"),
        },
        DefaultsRow {
            case: "a TCP target group has no protocol version",
            params: vec![p("Protocol", "TCP"), p("Port", "1234"), p("VpcId", "vpc-1")],
            protocol_version: None,
            ip_address_type: Some("ipv4"),
        },
        DefaultsRow {
            case: "a TCP_UDP target group has no protocol version",
            params: vec![
                p("Protocol", "TCP_UDP"),
                p("Port", "1234"),
                p("VpcId", "vpc-1"),
            ],
            protocol_version: None,
            ip_address_type: Some("ipv4"),
        },
        DefaultsRow {
            case: "a GENEVE target group has no protocol version",
            params: vec![
                p("Protocol", "GENEVE"),
                p("Port", "6081"),
                p("VpcId", "vpc-1"),
            ],
            protocol_version: None,
            ip_address_type: Some("ipv4"),
        },
        DefaultsRow {
            // `IpAddressType`'s default is stated for every target group, with no protocol
            // or target type qualifying it — so a Lambda target group, which has no
            // protocol at all and therefore no protocol version, still reports it.
            case: "a lambda target group has no protocol at all",
            params: vec![p("TargetType", "lambda")],
            protocol_version: None,
            ip_address_type: Some("ipv4"),
        },
    ];

    for (index, row) in rows.into_iter().enumerate() {
        let DefaultsRow {
            case,
            params,
            protocol_version,
            ip_address_type,
        } = row;
        let name = format!("defaults-tg-{index}");
        let mut form = vec![p("Name", &name)];
        form.extend(params);
        let created = elbv2(&router, "CreateTargetGroup", &form).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200, got {} with {}",
            created.status, created.body
        );
        assert_eq!(
            (
                created.text("ProtocolVersion").as_deref(),
                created.text("IpAddressType").as_deref()
            ),
            (protocol_version, ip_address_type),
            "case {case:?}: expected ({protocol_version:?}, {ip_address_type:?}), got {}",
            created.body
        );

        // Read back: the defaults are PERSISTED by the create, not computed by the
        // response writer, so the describe reports exactly the same values.
        let described = elbv2(
            &router,
            "DescribeTargetGroups",
            &[p("Names.member.1", &name)],
        )
        .await;
        assert_eq!(
            (
                described.text("ProtocolVersion").as_deref(),
                described.text("IpAddressType").as_deref()
            ),
            (protocol_version, ip_address_type),
            "case {case:?}: the stored target group must report \
             ({protocol_version:?}, {ip_address_type:?}), got {}",
            described.body
        );

        // And the repeat the model documents as idempotent still answers the stored
        // target group: a materialized default is the same setting as the one the request
        // left out, so the second call must not read as a different configuration.
        let repeat = elbv2(&router, "CreateTargetGroup", &form).await;
        assert_eq!(
            (repeat.status, repeat.text("TargetGroupArn")),
            (200, created.text("TargetGroupArn")),
            "case {case:?}: a repeat of the same settings must answer the stored group, \
             got {} {}",
            repeat.status,
            repeat.body
        );
    }
}
