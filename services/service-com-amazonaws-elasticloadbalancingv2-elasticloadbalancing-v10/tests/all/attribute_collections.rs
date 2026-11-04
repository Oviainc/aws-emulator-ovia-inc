// SPDX-License-Identifier: CC0-1.0

//! The three attribute families of this API — a load balancer's, a target group's and a
//! listener's — read and written over the wire, through the router.
//!
//! The ONE rule these tables pin, across all three: `Modify*Attributes` and
//! `Describe*Attributes` both answer the resource's WHOLE attribute collection — every
//! key the catalog carries for that resource, each holding what a caller wrote or the
//! value its key is carried at, in one order — so a write that names a single key answers
//! exactly what a read then reports, and never only the keys the request named.
//!
//! The rows span what the rule generalises over: the three attribute families, every load
//! balancer family each is carried by, the target types that change a group's collection,
//! and how many keys one request writes. The per-family values each key is carried at are
//! pinned by `all/load_balancer_attributes.rs`,
//! `all/target_group_attributes_and_modification.rs` and
//! `all/listener_lifecycle_certificates_and_rules.rs`; this suite asserts WHICH keys the
//! collection carries, IN WHICH ORDER, and that one write answers all of them.
//!
//! The wire harness (`emulator`, `call`, `action`, `p`, `attribute_sequence`) and the
//! resource creators are the ones those suites already own; nothing here restates them.

use crate::listener_lifecycle_certificates_and_rules::create_listener;
use crate::load_balancer_attributes::create_load_balancer;
use crate::provisioning_loop::{
    action, attribute_sequence, call, create_http_target_group, emulator, p,
};
use crate::target_group_attributes_and_modification::create_target_group;
use smithy_server_runtime::MultiServiceRouter;
use std::collections::BTreeMap;

/// One attribute family: the pair of operations that read and write it, and the input
/// member each names its resource through
/// (`smithy_query(query="operation",operation="DescribeLoadBalancerAttributes,DescribeTargetGroupAttributes,DescribeListenerAttributes")`).
struct Family {
    describe: &'static str,
    modify: &'static str,
    id_member: &'static str,
}

const LOAD_BALANCER: Family = Family {
    describe: "DescribeLoadBalancerAttributes",
    modify: "ModifyLoadBalancerAttributes",
    id_member: "LoadBalancerArn",
};

const TARGET_GROUP: Family = Family {
    describe: "DescribeTargetGroupAttributes",
    modify: "ModifyTargetGroupAttributes",
    id_member: "TargetGroupArn",
};

const LISTENER: Family = Family {
    describe: "DescribeListenerAttributes",
    modify: "ModifyListenerAttributes",
    id_member: "ListenerArn",
};

/// One row: what the resource is, which family's attributes it carries, its ARN, the keys
/// its collection must carry IN THE ORDER it must report them, and the attributes one
/// request writes onto it.
struct Row {
    case: String,
    family: &'static Family,
    arn: String,
    keys: Vec<&'static str>,
    writes: Vec<(&'static str, &'static str)>,
}

/// What `family`'s read reports for `arn`, in the order it reports it.
async fn read_collection(
    router: &MultiServiceRouter,
    family: &Family,
    case: &str,
    arn: &str,
) -> Vec<(String, String)> {
    let mut form = action(family.describe);
    form.push(p(family.id_member, arn));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case {case}: expected 200 from {}, got {} body {}",
        family.describe, reply.status, reply.body
    );
    attribute_sequence(&reply.body)
}

/// Write `writes` through `family`'s write and answer what it reported, in order.
async fn write_collection(
    router: &MultiServiceRouter,
    family: &Family,
    case: &str,
    arn: &str,
    writes: &[(&str, &str)],
) -> Vec<(String, String)> {
    let mut form = action(family.modify);
    form.push(p(family.id_member, arn));
    for (index, (key, value)) in writes.iter().enumerate() {
        let position = index + 1;
        form.push(p(&format!("Attributes.member.{position}.Key"), key));
        form.push(p(&format!("Attributes.member.{position}.Value"), value));
    }
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case {case}: expected 200 from {}, got {} body {}",
        family.modify, reply.status, reply.body
    );
    attribute_sequence(&reply.body)
}

/// Every resource kind this API carries attributes for, each with the keys its collection
/// must carry — in the order the service reports them — and a write to make on it.
///
/// The key lists come from the three catalogs the model documents
/// (`smithy_query(query="doc",shape="LoadBalancerAttribute",member="Key")` and the same
/// for `TargetGroupAttribute` and `ListenerAttribute`), filtered to the family and target
/// type of the resource in the row.
async fn rows(router: &MultiServiceRouter) -> Vec<Row> {
    // The three keys documented for "both Application Load Balancers and Network Load
    // Balancers" that every load balancer below carries.
    let shared_logs = ["access_logs.s3.enabled", "access_logs.s3.bucket", "access_logs.s3.prefix"];
    let application_load_balancer: Vec<&'static str> = shared_logs
        .iter()
        .copied()
        .chain([
            "health_check_logs.s3.enabled",
            "health_check_logs.s3.bucket",
            "health_check_logs.s3.prefix",
            "idle_timeout.timeout_seconds",
            "deletion_protection.enabled",
            "routing.http2.enabled",
            "routing.http.drop_invalid_header_fields.enabled",
            "routing.http.xff_client_port.enabled",
            "routing.http.xff_header_processing.mode",
            "routing.http.preserve_host_header.enabled",
            "routing.http.x_amzn_tls_version_and_cipher_suite.enabled",
            "routing.http.desync_mitigation_mode",
            "load_balancing.cross_zone.enabled",
            "client_keep_alive.seconds",
            "waf.fail_open.enabled",
            "zonal_shift.config.enabled",
            "connection_logs.s3.enabled",
            "connection_logs.s3.bucket",
            "connection_logs.s3.prefix",
        ])
        .collect();

    // The keys every target group that CONNECTS to its targets carries: the three
    // "supported by all load balancers" and the five "supported by Application Load
    // Balancers and Network Load Balancers".
    let connecting_group = [
        "deregistration_delay.timeout_seconds",
        "stickiness.enabled",
        "stickiness.type",
        "load_balancing.cross_zone.enabled",
        "target_group_health.dns_failover.minimum_healthy_targets.count",
        "target_group_health.dns_failover.minimum_healthy_targets.percentage",
        "target_group_health.unhealthy_state_routing.minimum_healthy_targets.count",
        "target_group_health.unhealthy_state_routing.minimum_healthy_targets.percentage",
    ];

    // An Application Load Balancer listener's collection: the response server header and
    // the ten response header values.
    let application_listener = [
        "routing.http.response.server.enabled",
        "routing.http.response.strict_transport_security.header_value",
        "routing.http.response.access_control_allow_origin.header_value",
        "routing.http.response.access_control_allow_methods.header_value",
        "routing.http.response.access_control_allow_headers.header_value",
        "routing.http.response.access_control_allow_credentials.header_value",
        "routing.http.response.access_control_expose_headers.header_value",
        "routing.http.response.access_control_max_age.header_value",
        "routing.http.response.content_security_policy.header_value",
        "routing.http.response.x_content_type_options.header_value",
        "routing.http.response.x_frame_options.header_value",
    ];

    let alb = create_load_balancer(router, "collection-alb", &[]).await;
    let nlb = create_load_balancer(router, "collection-nlb", &[("Type", "network")]).await;
    let gwlb = create_load_balancer(router, "collection-gwlb", &[("Type", "gateway")]).await;
    let http_group = create_http_target_group(router, "collection-http-tg").await;
    let tcp_group = create_target_group(
        router,
        "collection-tcp-tg",
        &[("Protocol", "TCP"), ("Port", "1234"), ("VpcId", "vpc-1")],
    )
    .await;
    let lambda_group =
        create_target_group(router, "collection-lambda-tg", &[("TargetType", "lambda")]).await;
    let geneve_group = create_target_group(
        router,
        "collection-geneve-tg",
        &[("Protocol", "GENEVE"), ("Port", "6081"), ("VpcId", "vpc-1")],
    )
    .await;
    let alb_listener = create_listener(
        router,
        "the application listener",
        &alb,
        &http_group,
        vec![p("Protocol", "HTTP"), p("Port", "80")],
    )
    .await;
    let nlb_listener = create_listener(
        router,
        "the network listener",
        &nlb,
        &tcp_group,
        vec![p("Protocol", "TCP"), p("Port", "1234")],
    )
    .await;
    // "You can't specify a protocol for a Gateway Load Balancer."
    let gwlb_listener = create_listener(
        router,
        "the gateway listener",
        &gwlb,
        &geneve_group,
        Vec::new(),
    )
    .await;

    vec![
        Row {
            case: "an application load balancer".to_string(),
            family: &LOAD_BALANCER,
            arn: alb,
            keys: application_load_balancer,
            // One key, which is what a client that changes one setting sends.
            writes: vec![("idle_timeout.timeout_seconds", "120")],
        },
        Row {
            case: "a network load balancer".to_string(),
            family: &LOAD_BALANCER,
            arn: nlb,
            keys: shared_logs
                .iter()
                .copied()
                .chain([
                    "deletion_protection.enabled",
                    "load_balancing.cross_zone.enabled",
                    "zonal_shift.config.enabled",
                    "secondary_ips.auto_assigned.per_subnet",
                ])
                .collect(),
            // Two keys, one of them a destination the collection carries at the empty
            // string.
            writes: vec![
                ("access_logs.s3.enabled", "true"),
                ("access_logs.s3.bucket", "nlb-access-logs"),
            ],
        },
        Row {
            case: "a gateway load balancer".to_string(),
            family: &LOAD_BALANCER,
            arn: gwlb,
            keys: vec![
                "deletion_protection.enabled",
                "load_balancing.cross_zone.enabled",
            ],
            writes: vec![("deletion_protection.enabled", "true")],
        },
        Row {
            case: "an HTTP instance target group".to_string(),
            family: &TARGET_GROUP,
            arn: http_group,
            keys: connecting_group
                .iter()
                .copied()
                .chain([
                    "load_balancing.algorithm.type",
                    "load_balancing.algorithm.anomaly_mitigation",
                    "slow_start.duration_seconds",
                    "stickiness.app_cookie.cookie_name",
                    "stickiness.app_cookie.duration_seconds",
                    "stickiness.lb_cookie.duration_seconds",
                ])
                .collect(),
            writes: vec![("deregistration_delay.timeout_seconds", "45")],
        },
        Row {
            case: "a TCP instance target group".to_string(),
            family: &TARGET_GROUP,
            arn: tcp_group,
            keys: connecting_group
                .iter()
                .copied()
                .chain([
                    "deregistration_delay.connection_termination.enabled",
                    "preserve_client_ip.enabled",
                    "proxy_protocol_v2.enabled",
                    "target_health_state.unhealthy.connection_termination.enabled",
                    "target_health_state.unhealthy.draining_interval_seconds",
                ])
                .collect(),
            writes: vec![("proxy_protocol_v2.enabled", "true")],
        },
        Row {
            case: "a lambda target group".to_string(),
            family: &TARGET_GROUP,
            arn: lambda_group,
            // The one attribute documented for a Lambda target: nothing the load
            // balancer CONNECTS to a target with applies.
            keys: vec!["lambda.multi_value_headers.enabled"],
            writes: vec![("lambda.multi_value_headers.enabled", "true")],
        },
        Row {
            case: "an application load balancer listener".to_string(),
            family: &LISTENER,
            arn: alb_listener,
            keys: application_listener.to_vec(),
            writes: vec![("routing.http.response.server.enabled", "false")],
        },
        Row {
            case: "a network load balancer listener".to_string(),
            family: &LISTENER,
            arn: nlb_listener,
            keys: vec!["tcp.idle_timeout.seconds"],
            writes: vec![("tcp.idle_timeout.seconds", "400")],
        },
        Row {
            case: "a gateway load balancer listener".to_string(),
            family: &LISTENER,
            arn: gwlb_listener,
            keys: vec![
                "tcp.idle_timeout.seconds",
                "send_tcp_reset.on_idle_timeout.enabled",
            ],
            writes: vec![("send_tcp_reset.on_idle_timeout.enabled", "true")],
        },
    ]
}

/// A read reports the resource's WHOLE collection: exactly the keys the catalog carries
/// for it, in the one order the service reports them — the catalog's, which is neither
/// alphabetical nor the request's — and including the string-valued keys the
/// documentation states no value for, which are carried at the empty string.
///
/// The expected list of each row is written in that order, so a reordering of any of the
/// three catalogs fails here, for the target group and listener families as much as for
/// the load balancer one.
#[tokio::test]
async fn a_read_reports_every_key_the_collection_carries() {
    let (router, _engine) = emulator();

    for row in rows(&router).await {
        let reported = read_collection(&router, row.family, &row.case, &row.arn).await;
        let keys: Vec<&str> = reported.iter().map(|(key, _)| key.as_str()).collect();
        // Ordered equality, so a key reported twice, a key missing, a key the collection
        // should not carry and a key out of place each fail.
        assert_eq!(
            keys, row.keys,
            "case {}: expected the whole documented collection in catalog order, got {keys:?}",
            row.case
        );
        // Every key carries a value element, even the ones carried at the empty string:
        // the serializer writes `<Value></Value>`, which the reader reports as "".
        assert_eq!(
            reported.len(),
            row.keys.len(),
            "case {}: expected one key/value pair per carried key, got {reported:?}",
            row.case
        );
    }
}

/// A write answers the resource's WHOLE collection: the same keys, in the same order,
/// with the same values the read that follows it reports — not only the keys the request
/// named.
#[tokio::test]
async fn a_write_answers_the_same_collection_a_read_reports() {
    let (router, _engine) = emulator();

    for row in rows(&router).await {
        let before = read_collection(&router, row.family, &row.case, &row.arn).await;
        let answered =
            write_collection(&router, row.family, &row.case, &row.arn, &row.writes).await;
        let after = read_collection(&router, row.family, &row.case, &row.arn).await;

        assert_eq!(
            answered, after,
            "case {}: expected the write to answer the collection the read reports ({after:?}), \
             got {answered:?}",
            row.case
        );
        // Every key of the collection is in the write's answer, however few the request
        // named.
        let answered_keys: Vec<&str> = answered.iter().map(|(key, _)| key.as_str()).collect();
        let before_keys: Vec<&str> = before.iter().map(|(key, _)| key.as_str()).collect();
        assert_eq!(
            answered_keys, before_keys,
            "case {}: a write must change no key of the collection, got {answered_keys:?}",
            row.case
        );
        // The written values are in it, and nothing else moved.
        let written: BTreeMap<&str, &str> = row.writes.iter().copied().collect();
        let expected: Vec<(String, String)> = before
            .iter()
            .map(|(key, value)| {
                let value = written
                    .get(key.as_str())
                    .map_or_else(|| value.clone(), |written| (*written).to_string());
                (key.clone(), value)
            })
            .collect();
        assert_eq!(
            answered, expected,
            "case {}: expected the written values beside every untouched key, got {answered:?}",
            row.case
        );
    }
}

/// A key the collection does not carry is reported by neither operation until a caller
/// writes it, and by both afterwards — so a client can tell "carried, at its default"
/// from "not carried".
///
/// The rows are the keys the catalogs list for a resource whose collection does not carry
/// them: the load balancer's internet-gateway block, whose value follows the scheme, and
/// an Application Load Balancer listener's mutual-TLS request header names.
#[tokio::test]
async fn a_key_the_collection_does_not_carry_appears_only_once_written() {
    let (router, _engine) = emulator();

    let alb = create_load_balancer(&router, "uncarried-alb", &[]).await;
    let group = create_http_target_group(&router, "uncarried-tg").await;
    let listener = create_listener(
        &router,
        "the listener of the uncarried keys",
        &alb,
        &group,
        vec![p("Protocol", "HTTP"), p("Port", "8080")],
    )
    .await;

    let cases: Vec<(&str, &Family, &str, &str, &str)> = vec![
        (
            "the internet-gateway block of a load balancer",
            &LOAD_BALANCER,
            alb.as_str(),
            "ipv6.deny_all_igw_traffic",
            "true",
        ),
        (
            "a listener's mutual-TLS client certificate header name",
            &LISTENER,
            listener.as_str(),
            "routing.http.request.x_amzn_mtls_clientcert.header_name",
            "X-Client-Cert",
        ),
        (
            "a listener's TLS version header name",
            &LISTENER,
            listener.as_str(),
            "routing.http.request.x_amzn_tls_version.header_name",
            "X-Tls-Version",
        ),
    ];

    for (case, family, arn, key, value) in cases {
        let before = read_collection(&router, family, case, arn).await;
        assert!(
            !before.iter().any(|(reported, _)| reported == key),
            "case {case}: expected no {key} before a write, got {before:?}"
        );

        let answered = write_collection(&router, family, case, arn, &[(key, value)]).await;
        assert!(
            answered
                .iter()
                .any(|(reported, held)| reported == key && held == value),
            "case {case}: expected the write to answer {key} at {value:?}, got {answered:?}"
        );
        let after = read_collection(&router, family, case, arn).await;
        assert_eq!(
            after, answered,
            "case {case}: expected the read to report what the write answered, got {after:?}"
        );
    }
}
