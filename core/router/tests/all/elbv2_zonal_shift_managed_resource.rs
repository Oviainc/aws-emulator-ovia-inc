// SPDX-License-Identifier: CC0-1.0

//! A load balancer with zonal-shift integration enabled, across the three services that
//! span: Amazon EC2 over ec2Query, Elastic Load Balancing v2 over awsQuery and ARC Zonal
//! Shift over restJson1, all served by ONE router over ONE engine — so what is asserted is
//! what a client holding all three SDKs sees.
//!
//! RULE: ARC models no operation that registers a managed resource — a resource becomes one
//! through the service that OWNS it — so turning zonal-shift integration on for a load
//! balancer (`ModifyLoadBalancerAttributes` with `zonal_shift.config.enabled` = `true`)
//! makes that load balancer a managed resource of ARC: `GetManagedResource` on its ARN
//! answers 200 with one `appliedWeights` entry per Availability Zone the load balancer is
//! deployed in, a zonal shift can then be started, updated and read back through it, and
//! the integration turned off or the load balancer deleted takes it away again.
//!
//! This suite lives beside the router because the three stores are distinct: the VPC and
//! the subnets are EC2's, so every subnet id below is one EC2 minted and every Availability
//! Zone ID asserted on is the one EC2 reports for that subnet — ARC names a zone by its ID
//! ("The Availability Zone (for example, `use1-az1`)",
//! `StartZonalShiftRequest$awayFrom`) — the load balancer is Elastic Load Balancing's, and
//! what ARC reports is what the load balancer's own service recorded rather than a copy of
//! it.
//!
//! The rows differ along what the rule generalises over: the TYPE and SCHEME of the load
//! balancer (both the families the attribute's documentation lists it for) and how many
//! Availability Zones it spans.

use std::collections::BTreeMap;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{element, form_encode};

/// The region every request below is signed for.
const REGION: &str = "us-west-2";

/// The EC2 API version a client sends in an ec2Query body.
const EC2_API_VERSION: &str = "2016-11-15";

/// The API version an awsQuery request naming the ELBv2 model carries.
const ELB_VERSION: &str = "2015-12-01";

/// The attribute that turns zonal-shift integration on, read from the service that owns it.
const ZONAL_SHIFT_ATTRIBUTE: &str =
    service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::state::ZONAL_SHIFT_ATTRIBUTE;

/// One emulator carrying Amazon EC2, Elastic Load Balancing v2 and ARC Zonal Shift over
/// ONE shared engine, as the production router mounts them.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_arczonalshift_percdataplane::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `signing_name`, which is
/// what routes the request here; the signature is never checked.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}\
         /aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// One form-encoded call to a query-protocol service, answered with its status and body.
async fn query(
    router: &MultiServiceRouter,
    signing_name: &str,
    host: &str,
    body: String,
) -> (u16, String) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", host)
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(signing_name))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    (status, text)
}

/// A form body for `action` carrying `params`.
fn form(action: &str, version: &str, params: &[(&str, &str)]) -> String {
    let mut body = format!("Action={action}&Version={version}");
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    body
}

/// One ec2Query action that must succeed, answered with its XML body.
async fn ec2(router: &MultiServiceRouter, action: &str, params: &[(&str, &str)]) -> String {
    let (status, text) = query(
        router,
        "ec2",
        &format!("ec2.{REGION}.amazonaws.com"),
        form(action, EC2_API_VERSION, params),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: EC2 {action} must succeed, got {status} with {text}"
    );
    text
}

/// One ELBv2 awsQuery action, answered with its status and XML body.
async fn elbv2(router: &MultiServiceRouter, action: &str, params: &[(&str, &str)]) -> (u16, String) {
    query(
        router,
        "elasticloadbalancingv2",
        &format!("elasticloadbalancingv2.{REGION}.amazonaws.com"),
        form(action, ELB_VERSION, params),
    )
    .await
}

/// One ELBv2 action that must succeed, answered with its XML body.
async fn elbv2_ok(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    params: &[(&str, &str)],
) -> String {
    let (status, body) = elbv2(router, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {action} must answer 200, got {status} {body}"
    );
    body
}

/// One restJson1 call to ARC Zonal Shift, answered with its status and parsed body.
async fn arc(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body
        .map(|value| serde_json::to_vec(&value).expect("the request body serializes"))
        .unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("arc-zonal-shift.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("arc-zonal-shift"))
        .body(Bytes::from(payload))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice::<Value>(&payload).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// `GetManagedResource` for one resource identifier.
async fn get_managed_resource(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    arc(
        router,
        "GET",
        &format!("/managedresources/{}", form_encode(arn)),
        None,
    )
    .await
}

/// The wire error code of a restJson1 error envelope, with any `shapeId#` prefix dropped.
fn error_code(body: &Value) -> String {
    body["__type"]
        .as_str()
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The `appliedWeights` of a resource view, as a sorted map.
fn weights(view: &Value) -> BTreeMap<String, f64> {
    view["appliedWeights"]
        .as_object()
        .unwrap_or_else(|| panic!("appliedWeights is required, got {view}"))
        .iter()
        .map(|(zone, weight)| (zone.clone(), weight.as_f64().unwrap_or(f64::NAN)))
        .collect()
}

/// The weights a resource deployed in `zones` reports while traffic is moved away from
/// `away_from`: 0 there, 1 everywhere else.
fn expected_weights(zones: &[String], away_from: Option<&str>) -> BTreeMap<String, f64> {
    zones
        .iter()
        .map(|zone| {
            let weight = if Some(zone.as_str()) == away_from {
                0.0
            } else {
                1.0
            };
            (zone.clone(), weight)
        })
        .collect()
}

/// One subnet EC2 minted: its id and the Availability Zone ID EC2 reports for it.
struct Subnet {
    id: String,
    zone_id: String,
}

/// Create one VPC carrying one subnet per entry of `zones`, through EC2's own operations —
/// so every id and every zone ID below is one EC2 handed out.
async fn ec2_network(router: &MultiServiceRouter, second_octet: u8, zones: &[&str]) -> Vec<Subnet> {
    let cidr = format!("10.{second_octet}.0.0/16");
    let vpc_id = element(&ec2(router, "CreateVpc", &[("CidrBlock", &cidr)]).await, "vpcId");
    let mut subnets = Vec::new();
    for (index, zone) in zones.iter().enumerate() {
        let subnet_cidr = format!("10.{second_octet}.{}.0/24", index + 1);
        let created = ec2(
            router,
            "CreateSubnet",
            &[
                ("VpcId", vpc_id.as_str()),
                ("CidrBlock", subnet_cidr.as_str()),
                ("AvailabilityZone", zone),
            ],
        )
        .await;
        subnets.push(Subnet {
            id: element(&created, "subnetId"),
            zone_id: element(&created, "availabilityZoneId"),
        });
    }
    subnets
}

/// Write the zonal-shift attribute of one load balancer, requiring the 200.
async fn set_zonal_shift(router: &MultiServiceRouter, case: &str, lb_arn: &str, value: &str) {
    elbv2_ok(
        router,
        case,
        "ModifyLoadBalancerAttributes",
        &[
            ("LoadBalancerArn", lb_arn),
            ("Attributes.member.1.Key", ZONAL_SHIFT_ATTRIBUTE),
            ("Attributes.member.1.Value", value),
        ],
    )
    .await;
}

/// One load balancer the rule is asserted over.
struct Row {
    case: &'static str,
    name: &'static str,
    lb_type: &'static str,
    scheme: &'static str,
    /// The Availability Zones its subnets are created in — one subnet per zone.
    zones: &'static [&'static str],
    /// The second octet of the VPC CIDR, so each row builds its own network.
    octet: u8,
}

#[tokio::test]
async fn a_load_balancer_with_zonal_shift_enabled_is_a_managed_resource_of_arc() {
    // Both families the attribute's documentation lists it for — "supported by both
    // Application Load Balancers and Network Load Balancers" — across two zone counts and
    // both schemes.
    let rows: &[Row] = &[
        Row {
            case: "an internal Network Load Balancer across two zones",
            name: "zonal-shift-nlb",
            lb_type: "network",
            scheme: "internal",
            zones: &["us-west-2a", "us-west-2b"],
            octet: 201,
        },
        Row {
            case: "an internet-facing Application Load Balancer across three zones",
            name: "zonal-shift-alb",
            lb_type: "application",
            scheme: "internet-facing",
            zones: &["us-west-2a", "us-west-2b", "us-west-2c"],
            octet: 202,
        },
    ];

    for row in rows {
        let case = row.case;
        // One emulator per row, so each row's assertions are about its own stores.
        let router = emulator();
        let subnets = ec2_network(&router, row.octet, row.zones).await;
        // The Availability Zone IDs EC2 reports for those subnets, which is how ARC names
        // a zone — never a value this suite made up.
        let zone_ids: Vec<String> = subnets.iter().map(|s| s.zone_id.clone()).collect();
        assert_eq!(
            zone_ids.len(),
            row.zones.len(),
            "setup {case:?}: expected one zone ID per subnet, got {zone_ids:?}"
        );

        let mut params: Vec<(&str, &str)> = vec![
            ("Name", row.name),
            ("Type", row.lb_type),
            ("Scheme", row.scheme),
        ];
        let subnet_keys: Vec<String> = (1..=subnets.len())
            .map(|index| format!("Subnets.member.{index}"))
            .collect();
        for (key, subnet) in subnet_keys.iter().zip(&subnets) {
            params.push((key.as_str(), subnet.id.as_str()));
        }
        let created = elbv2_ok(&router, case, "CreateLoadBalancer", &params).await;
        let lb_arn = element(&created, "LoadBalancerArn");

        // Before the integration is turned on, ARC has no record of the load balancer.
        let (status, body) = get_managed_resource(&router, &lb_arn).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: a load balancer without the integration is not a managed \
             resource, got {status} {body}"
        );

        // Turned on, it is one — with one weight per zone EC2 put it in.
        set_zonal_shift(&router, case, &lb_arn, "true").await;
        let (status, view) = get_managed_resource(&router, &lb_arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetManagedResource must answer 200 once zonal shift is \
             enabled, got {status} {view}"
        );
        assert_eq!(
            (view["arn"].as_str(), view["name"].as_str()),
            (Some(lb_arn.as_str()), Some(row.name)),
            "case {case:?}: expected the load balancer's own ARN and name, got {view}"
        );
        assert_eq!(
            weights(&view),
            expected_weights(&zone_ids, None),
            "case {case:?}: expected weight 1 in each of {zone_ids:?}, got {:?}",
            weights(&view)
        );
        assert_eq!(
            view["zonalShifts"].as_array().map(Vec::len),
            Some(0),
            "case {case:?}: a resource with no shift in progress lists none, got {view}"
        );

        // A shift against it: started, extended, and read back through the resource.
        let away_from = zone_ids[zone_ids.len() - 1].clone();
        let (status, started) = arc(
            &router,
            "POST",
            "/zonalshifts",
            Some(json!({
                "resourceIdentifier": lb_arn,
                "awayFrom": away_from,
                "expiresIn": "1h",
                "comment": "shifting away from one zone",
            })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: StartZonalShift must answer 201, got {status} {started}"
        );
        let shift_id = started["zonalShiftId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no zonalShiftId in {started}"))
            .to_string();
        let first_expiry = started["expiryTime"]
            .as_f64()
            .unwrap_or_else(|| panic!("case {case:?}: no expiryTime in {started}"));

        let (_, view) = get_managed_resource(&router, &lb_arn).await;
        assert_eq!(
            weights(&view),
            expected_weights(&zone_ids, Some(&away_from)),
            "case {case:?}: expected weight 0 for {away_from} and 1 elsewhere, got {:?}",
            weights(&view)
        );

        let updated_comment = "extended while the shift is running";
        let (status, updated) = arc(
            &router,
            "PATCH",
            &format!("/zonalshifts/{shift_id}"),
            Some(json!({ "comment": updated_comment, "expiresIn": "3h" })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateZonalShift must answer 200, got {status} {updated}"
        );
        let second_expiry = updated["expiryTime"]
            .as_f64()
            .unwrap_or_else(|| panic!("case {case:?}: no expiryTime in {updated}"));
        assert_eq!(
            (updated["status"].as_str(), updated["comment"].as_str()),
            (Some("ACTIVE"), Some(updated_comment)),
            "case {case:?}: an updated shift stays ACTIVE with the new comment, got {updated}"
        );
        assert!(
            second_expiry > first_expiry,
            "case {case:?}: expected an expiryTime past {first_expiry}, got {second_expiry}"
        );

        let (_, view) = get_managed_resource(&router, &lb_arn).await;
        let in_resource = view["zonalShifts"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: zonalShifts is required, got {view}"));
        assert_eq!(
            in_resource.len(),
            1,
            "case {case:?}: exactly the one shift in progress is listed, got {in_resource:?}"
        );
        assert_eq!(
            (
                in_resource[0]["zonalShiftId"].as_str(),
                in_resource[0]["comment"].as_str(),
                in_resource[0]["expiryTime"].as_f64(),
                in_resource[0]["awayFrom"].as_str(),
            ),
            (
                Some(shift_id.as_str()),
                Some(updated_comment),
                Some(second_expiry),
                Some(away_from.as_str()),
            ),
            "case {case:?}: expected the updated shift reported through the resource, got \
             {in_resource:?}"
        );

        // The integration turned off, ARC has no record of it again — and turning it back
        // on makes it a managed resource once more.
        set_zonal_shift(&router, case, &lb_arn, "false").await;
        let (status, body) = get_managed_resource(&router, &lb_arn).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: the integration turned off takes the managed resource away, \
             got {status} {body}"
        );
        set_zonal_shift(&router, case, &lb_arn, "true").await;
        let (status, view) = get_managed_resource(&router, &lb_arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: turned back on, it is a managed resource again, got {status} \
             {view}"
        );
        assert_eq!(
            weights(&view).keys().cloned().collect::<Vec<_>>(),
            zone_ids,
            "case {case:?}: expected the same zones EC2 put it in, got {:?}",
            weights(&view)
        );

        // And the load balancer deleted, it is gone from ARC as well.
        elbv2_ok(
            &router,
            case,
            "DeleteLoadBalancer",
            &[("LoadBalancerArn", &lb_arn)],
        )
        .await;
        let (status, body) = get_managed_resource(&router, &lb_arn).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: a deleted load balancer is no managed resource, got {status} \
             {body}"
        );
    }
}
