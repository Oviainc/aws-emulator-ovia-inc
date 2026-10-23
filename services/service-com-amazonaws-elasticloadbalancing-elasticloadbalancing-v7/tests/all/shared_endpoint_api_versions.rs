// SPDX-License-Identifier: CC0-1.0

//! ONE Elastic Load Balancing endpoint, TWO API versions: which model serves an awsQuery
//! request, and what happens to a pair neither model resolves.
//!
//! Both Elastic Load Balancing models declare `endpointPrefix` / `arnNamespace` /
//! `aws.auth#sigv4` name `elasticloadbalancing` (`smithy_query(query="service_info")` on
//! either), differing only in `sdkId` and in the API version they declare — `2012-06-01`
//! here and `2015-12-01` in
//! `services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10`. Every
//! AWS SDK for either API signs that one name, and nothing in a request's headers or URI
//! tells the two apart, so this front door claims BOTH APIs' traffic and the `Version`
//! field of the awsQuery envelope is what names the API a request is for.
//!
//! The rule these suites pin, over the whole class rather than one action:
//!
//! > On an endpoint two API models share, an awsQuery request resolves the PAIR of its
//! > `Action` and its `Version`: the model that declares the named version serves the
//! > action, and a pair no model resolves is refused `400 InvalidAction` naming the action
//! > and the version it could not be found for.
//!
//! So an action declared by both models (ten are) is answered out of the model whose
//! version the envelope names — in that model's own `@xmlNamespace` document, with that
//! model's own input and output shapes — an action declared by only one is served when its
//! own version is named and refused at the other, and a version neither model declares
//! resolves nothing. Before this, every `2015-12-01` envelope was refused `InvalidAction`,
//! so no ELBv2 control-plane call could be made at all: `InvalidAction` is reserved for an
//! action the service does not have.
//!
//! The rows run IN ORDER against one router over one engine, so a later row reads what an
//! earlier one wrote. The ELBv2 model's operations are reached here through THIS service
//! alone, which is the production path for an ELBv2 SDK client (it signs the shared name);
//! that the same store answers under the ELBv2 model's own routing key as well is pinned in
//! `core/router/tests/all/elb_classic_and_v2_signing_name_routing.rs`, which needs both
//! services mounted.

use crate::wire::{call_at_version, emulator, find, kid_text, param, parse, Element};

/// The API version THIS model declares.
const CLASSIC_VERSION: &str = "2012-06-01";

/// The API version the ELBv2 model declares.
const V2_VERSION: &str = "2015-12-01";

/// A version no Elastic Load Balancing model declares.
const UNKNOWN_VERSION: &str = "2099-01-01";

/// The `@xmlNamespace` of this model's document (`smithy_query(query="service_info")`).
const CLASSIC_XMLNS: &str = "http://elasticloadbalancing.amazonaws.com/doc/2012-06-01/";

/// The `@xmlNamespace` of the ELBv2 model's document (the same query on that model).
const V2_XMLNS: &str = "http://elasticloadbalancing.amazonaws.com/doc/2015-12-01/";

/// The region every request below is signed for.
const REGION: &str = "us-west-2";

/// One call on the shared endpoint and what the model that owns its action/version pair
/// must answer.
struct Row {
    case: &'static str,
    action: &'static str,
    /// The `Version` the envelope carries, or `None` to send no `Version` field at all.
    version: Option<&'static str>,
    params: &'static [(&'static str, &'static str)],
    expected_status: u16,
    /// The awsQuery `<Code>` the answer must carry, for a row that pins a refusal.
    expected_code: Option<&'static str>,
    /// Substrings the answer must carry — the serving model's envelope, its namespace and
    /// the shapes only that model declares.
    expected_body_contains: &'static [&'static str],
}

#[test]
fn the_version_in_the_envelope_chooses_the_model_that_serves_the_action() {
    let router = emulator();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("a current-thread runtime");

    // The rows differ along both halves of the pair: the version (this model's, the ELBv2
    // model's, none at all, and one neither declares) and the action (one only this model
    // declares, two only the ELBv2 model declares, `CreateLoadBalancer` and
    // `DescribeLoadBalancers`, which BOTH declare with different inputs and outputs, and
    // one neither declares).
    let rows: [Row; 14] = [
        Row {
            // The call the issue reports as `InvalidAction`: an action only the ELBv2
            // model declares, at the ELBv2 version.
            case: "CreateTargetGroup at the ELBv2 version",
            action: "CreateTargetGroup",
            version: Some(V2_VERSION),
            params: &[
                ("Name", "shared-door-tg"),
                ("Protocol", "HTTP"),
                ("Port", "80"),
                ("VpcId", "vpc-0abc1234"),
            ],
            expected_status: 200,
            expected_code: None,
            expected_body_contains: &[
                "<CreateTargetGroupResponse",
                V2_XMLNS,
                "<TargetGroupName>shared-door-tg</TargetGroupName>",
                "<VpcId>vpc-0abc1234</VpcId>",
            ],
        },
        Row {
            // …read back through the ELBv2 model: the front door serves that model's
            // state, not a fresh one per call.
            case: "DescribeTargetGroups at the ELBv2 version",
            action: "DescribeTargetGroups",
            version: Some(V2_VERSION),
            params: &[("Names.member.1", "shared-door-tg")],
            expected_status: 200,
            expected_code: None,
            expected_body_contains: &[
                "<DescribeTargetGroupsResponse",
                V2_XMLNS,
                "<TargetGroupName>shared-door-tg</TargetGroupName>",
                "<HealthCheckProtocol>HTTP</HealthCheckProtocol>",
            ],
        },
        Row {
            // An action BOTH models declare, at the ELBv2 version: the ELBv2 input
            // members (`Name`, `Subnets`) and the ELBv2 `<LoadBalancers>` output.
            case: "CreateLoadBalancer at the ELBv2 version",
            action: "CreateLoadBalancer",
            version: Some(V2_VERSION),
            params: &[("Name", "shared-door-v2-lb"), ("Subnets.member.1", "subnet-0abc1234")],
            expected_status: 200,
            expected_code: None,
            expected_body_contains: &[
                "<CreateLoadBalancerResponse",
                V2_XMLNS,
                "<LoadBalancers>",
                "<LoadBalancerArn>",
                "<LoadBalancerName>shared-door-v2-lb</LoadBalancerName>",
            ],
        },
        Row {
            // The same action at THIS model's version: this model's input members
            // (`LoadBalancerName`, `Listeners`, `AvailabilityZones`) and its `<DNSName>`
            // output, in its own namespace.
            case: "CreateLoadBalancer at the Classic version",
            action: "CreateLoadBalancer",
            version: Some(CLASSIC_VERSION),
            params: &[
                ("LoadBalancerName", "shared-door-classic-lb"),
                ("AvailabilityZones.member.1", "us-west-2a"),
                ("Listeners.member.1.Protocol", "HTTP"),
                ("Listeners.member.1.LoadBalancerPort", "80"),
                ("Listeners.member.1.InstancePort", "8080"),
            ],
            expected_status: 200,
            expected_code: None,
            expected_body_contains: &[
                "<CreateLoadBalancerResponse",
                CLASSIC_XMLNS,
                "<DNSName>shared-door-classic-lb-",
            ],
        },
        Row {
            // `DescribeLoadBalancers` is declared by both: at the ELBv2 version it
            // reports the ELBv2 load balancer, by ARN.
            case: "DescribeLoadBalancers at the ELBv2 version",
            action: "DescribeLoadBalancers",
            version: Some(V2_VERSION),
            params: &[("Names.member.1", "shared-door-v2-lb")],
            expected_status: 200,
            expected_code: None,
            expected_body_contains: &[
                V2_XMLNS,
                "<LoadBalancerArn>",
                "<LoadBalancerName>shared-door-v2-lb</LoadBalancerName>",
            ],
        },
        Row {
            // …and at this model's version it reports this model's load balancer, out of
            // this model's `<LoadBalancerDescriptions>` shape.
            case: "DescribeLoadBalancers at the Classic version",
            action: "DescribeLoadBalancers",
            version: Some(CLASSIC_VERSION),
            params: &[("LoadBalancerNames.member.1", "shared-door-classic-lb")],
            expected_status: 200,
            expected_code: None,
            expected_body_contains: &[
                CLASSIC_XMLNS,
                "<LoadBalancerDescriptions>",
                "<LoadBalancerName>shared-door-classic-lb</LoadBalancerName>",
            ],
        },
        Row {
            // No `Version` field at all: the caller has stated no API, so the front
            // door's OWN model serves the action.
            case: "a Classic action with no Version field",
            action: "DescribeLoadBalancers",
            version: None,
            params: &[],
            expected_status: 200,
            expected_code: None,
            expected_body_contains: &[CLASSIC_XMLNS, "<LoadBalancerDescriptions>"],
        },
        Row {
            // The other half of that rule: with no version stated, an action only the
            // ELBv2 model declares is not this model's, so the pair resolves to nothing.
            case: "an ELBv2-only action with no Version field",
            action: "DescribeTargetGroups",
            version: None,
            params: &[("Names.member.1", "shared-door-tg")],
            expected_status: 400,
            expected_code: Some("InvalidAction"),
            expected_body_contains: &["<ErrorResponse", "DescribeTargetGroups"],
        },
        Row {
            // An action only THIS model declares, asked for at the ELBv2 version: that
            // API does not have it, so the pair is refused rather than served out of this
            // model's shapes.
            case: "a Classic-only action at the ELBv2 version",
            action: "ConfigureHealthCheck",
            version: Some(V2_VERSION),
            params: &[
                ("LoadBalancerName", "shared-door-classic-lb"),
                ("HealthCheck.Target", "HTTP:8080/health"),
                ("HealthCheck.Interval", "10"),
                ("HealthCheck.Timeout", "5"),
                ("HealthCheck.UnhealthyThreshold", "2"),
                ("HealthCheck.HealthyThreshold", "3"),
            ],
            expected_status: 400,
            expected_code: Some("InvalidAction"),
            expected_body_contains: &["<ErrorResponse", "ConfigureHealthCheck", V2_VERSION],
        },
        Row {
            // …and the mirror image: an ELBv2-only action at this model's version.
            case: "an ELBv2-only action at the Classic version",
            action: "CreateTargetGroup",
            version: Some(CLASSIC_VERSION),
            params: &[
                ("Name", "shared-door-wrong-version"),
                ("Protocol", "HTTP"),
                ("Port", "80"),
                ("VpcId", "vpc-0abc1234"),
            ],
            expected_status: 400,
            expected_code: Some("InvalidAction"),
            expected_body_contains: &["<ErrorResponse", "CreateTargetGroup", CLASSIC_VERSION],
        },
        Row {
            // A version no Elastic Load Balancing model declares, for an action both
            // declare: nothing resolves it.
            case: "an action both models declare, at a version neither does",
            action: "DescribeLoadBalancers",
            version: Some(UNKNOWN_VERSION),
            params: &[],
            expected_status: 400,
            expected_code: Some("InvalidAction"),
            expected_body_contains: &["<ErrorResponse", "DescribeLoadBalancers", UNKNOWN_VERSION],
        },
        Row {
            case: "an ELBv2 action at a version neither model declares",
            action: "DescribeTargetGroups",
            version: Some(UNKNOWN_VERSION),
            params: &[("Names.member.1", "shared-door-tg")],
            expected_status: 400,
            expected_code: Some("InvalidAction"),
            expected_body_contains: &["<ErrorResponse", "DescribeTargetGroups", UNKNOWN_VERSION],
        },
        Row {
            // An action NEITHER model declares, at the ELBv2 version: `InvalidAction` is
            // what that is for.
            case: "an action neither model declares, at the ELBv2 version",
            action: "DescribeTargetGardens",
            version: Some(V2_VERSION),
            params: &[],
            expected_status: 400,
            expected_code: Some("InvalidAction"),
            expected_body_contains: &["<ErrorResponse"],
        },
        Row {
            case: "an action neither model declares, at the Classic version",
            action: "DescribeTargetGardens",
            version: Some(CLASSIC_VERSION),
            params: &[],
            expected_status: 400,
            expected_code: Some("InvalidAction"),
            expected_body_contains: &["<ErrorResponse"],
        },
    ];

    for row in rows {
        let params: Vec<(String, String)> = row
            .params
            .iter()
            .map(|(name, value)| param(name, value))
            .collect();
        let answer = runtime.block_on(call_at_version(
            &router,
            REGION,
            row.action,
            row.version,
            &params,
        ));
        let case = row.case;
        let (status, body) = (answer.status, answer.body.clone());
        // Whatever the answer is, it is an awsQuery envelope — XML a query-protocol
        // client can parse — and never a router-level refusal in a JSON document.
        assert!(
            body.trim_start().starts_with('<') && !body.contains("__type"),
            "case {case:?}: an awsQuery client must get XML, got {status} with {body}"
        );
        assert_eq!(
            status, row.expected_status,
            "case {case:?}: expected HTTP {}, got {status} with {body}",
            row.expected_status
        );
        if let Some(expected) = row.expected_code {
            let actual = answer.error_code();
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the error code {expected}, got {actual:?} in \
                 {status} with {body}"
            );
        }
        for expected in row.expected_body_contains {
            assert!(
                body.contains(expected),
                "case {case:?}: expected {expected:?} in the serving model's answer, got \
                 {status} with {body}"
            );
        }
    }
}

/// The ELBv2 target-group calls the issue's checks drive, end to end on the shared
/// endpoint: a create, the documented idempotent replay of it, the read back, and the
/// modelled refusal of a conflicting replay.
///
/// Every row is an ELBv2 operation reached at `Version=2015-12-01` through the service
/// that claims the shared signing name, which is the production path for an SDK ELBv2
/// client — and each answer is that model's own, so the pair resolving is not enough: the
/// state it reports has to be the ELBv2 store's.
#[test]
fn elbv2_target_groups_created_through_the_shared_endpoint_round_trip() {
    let router = emulator();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("a current-thread runtime");

    // (case, the target group's name, its port) — the create is replayed and read back
    // for each, so the suite does not rest on one name or one port.
    let groups: [(&str, &str, &str); 3] = [
        ("an HTTP target group on the default port", "round-trip-tg-80", "80"),
        ("a second group, differing only in port", "round-trip-tg-8080", "8080"),
        ("a group at the top of the modelled port range", "round-trip-tg-max", "65535"),
    ];

    for (case, name, port) in groups {
        let settings = [
            param("Name", name),
            param("Protocol", "HTTP"),
            param("Port", port),
            param("VpcId", "vpc-0abc1234"),
        ];

        let created = runtime.block_on(call_at_version(
            &router,
            REGION,
            "CreateTargetGroup",
            Some(V2_VERSION),
            &settings,
        ));
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreateTargetGroup must succeed, got {} with {}",
            created.status, created.body
        );
        let arn = target_group_arn(&created.body)
            .unwrap_or_else(|| panic!("case {case:?}: no TargetGroupArn in {}", created.body));
        assert!(
            arn.starts_with("arn:aws:elasticloadbalancing:")
                && arn.contains(REGION)
                && arn.contains(&format!("targetgroup/{name}/")),
            "case {case:?}: expected an ELBv2 target-group ARN in {REGION} for {name}, got \
             {arn:?}"
        );

        // "This operation is idempotent … If you attempt to create multiple target groups
        // with the same settings, each call succeeds."
        // (`smithy_query(query="doc",shape="CreateTargetGroup")`.)
        let replayed = runtime.block_on(call_at_version(
            &router,
            REGION,
            "CreateTargetGroup",
            Some(V2_VERSION),
            &settings,
        ));
        assert_eq!(
            replayed.status, 200,
            "case {case:?}: the idempotent replay must succeed, got {} with {}",
            replayed.status, replayed.body
        );
        let replayed_arn = target_group_arn(&replayed.body).unwrap_or_default();
        assert_eq!(
            replayed_arn, arn,
            "case {case:?}: the replay must report the stored target group, expected the \
             ARN {arn:?}, got {replayed_arn:?} in {}",
            replayed.body
        );

        // The read back reports what the create persisted, through the ELBv2 model's own
        // describe.
        let described = runtime.block_on(call_at_version(
            &router,
            REGION,
            "DescribeTargetGroups",
            Some(V2_VERSION),
            &[param("Names.member.1", name)],
        ));
        assert_eq!(
            described.status, 200,
            "case {case:?}: DescribeTargetGroups must read it back, got {} with {}",
            described.status, described.body
        );
        let doc = parse(&described.body);
        let group = find(&doc, "member")
            .unwrap_or_else(|| panic!("case {case:?}: no member in {}", described.body));
        assert_eq!(
            kid_text(group, "TargetGroupArn").as_deref(),
            Some(arn.as_str()),
            "case {case:?}: expected the stored ARN {arn:?}, got {}",
            described.body
        );
        assert_eq!(
            kid_text(group, "Port").as_deref(),
            Some(port),
            "case {case:?}: expected the stored port {port}, got {}",
            described.body
        );

        // A repeat of the same NAME with different settings is the one refusal the model
        // declares for it, as its `awsQueryError` code (`smithy_query(query="operation",
        // operation="CreateTargetGroup")`: `DuplicateTargetGroupName`, HTTP 400).
        let conflicting = runtime.block_on(call_at_version(
            &router,
            REGION,
            "CreateTargetGroup",
            Some(V2_VERSION),
            &[
                param("Name", name),
                param("Protocol", "HTTPS"),
                param("Port", port),
                param("VpcId", "vpc-0abc1234"),
            ],
        ));
        assert_eq!(
            conflicting.status, 400,
            "case {case:?}: a conflicting replay must be refused with HTTP 400, got {} \
             with {}",
            conflicting.status, conflicting.body
        );
        let code = conflicting.error_code();
        assert_eq!(
            code, "DuplicateTargetGroupName",
            "case {case:?}: expected the modelled awsQuery code \
             DuplicateTargetGroupName, got {code:?} in {}",
            conflicting.body
        );
    }

    // Every group above is one target group in the ONE ELBv2 store, listed by a describe
    // that names none of them.
    let all = runtime.block_on(call_at_version(
        &router,
        REGION,
        "DescribeTargetGroups",
        Some(V2_VERSION),
        &[],
    ));
    for (case, name, _) in groups {
        assert!(
            all.body
                .contains(&format!("<TargetGroupName>{name}</TargetGroupName>")),
            "case {case:?}: expected {name} among every target group, got {} with {}",
            all.status,
            all.body
        );
    }
}

/// The `<TargetGroupArn>` of the first target group an answer reports.
fn target_group_arn(body: &str) -> Option<String> {
    let doc = parse(body);
    find(&doc, "TargetGroupArn")
        .and_then(Element::text)
        .map(str::to_string)
}
