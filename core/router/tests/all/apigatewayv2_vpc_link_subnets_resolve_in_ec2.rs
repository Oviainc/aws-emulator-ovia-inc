// SPDX-License-Identifier: CC0-1.0

//! API Gateway v2 resolves the SUBNETS a VPC link would be built in against Amazon EC2,
//! which owns them.
//!
//! RULE: a subnet is Amazon EC2's resource and not API Gateway's, so every subnet id
//! `CreateVpcLink` names is resolved against the account's EC2 inventory BEFORE anything is
//! minted — a create naming an id EC2 holds no subnet for is refused with the modelled
//! `BadRequestException` (HTTP 400) naming that id, and so is one whose subnets are spread
//! over more than one VPC, since a link's network interfaces all live in the ONE VPC the
//! link attaches the api to; a create naming EC2's own subnets of one VPC is accepted and
//! the link it mints is the one `GetVpcLink` then reports. A refused create writes nothing.
//!
//! This suite lives beside the router because the subnets are EC2's: the VPCs and subnets
//! below are minted by the real EC2 operations on the SAME engine that serves API Gateway
//! v2, so every id in an accepted request is one EC2 handed out rather than one this suite
//! made up, and every refusal is grounded in what EC2 actually answered.
//!
//! The rows differ along what the rule generalises over: how MANY subnets the request names,
//! WHERE in the list the unresolvable one sits, the FORM of the value that names no subnet
//! (EC2's 17-hex id form and its older 8-hex form), how many VPCs a cross-VPC list straddles,
//! and the order the VPCs appear in.
//!
//! Error source: `smithy_query(query="operation", operation="CreateVpcLink")` on the
//! apigatewayv2 model — `BadRequestException`, `errorKind` client, `httpError` 400; success
//! `code: 201`.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_apigatewayv2_apigatewayv2::state::SUBNETS_NOT_IN_ONE_VPC_MESSAGE;
use service_com_amazonaws_apigatewayv2_apigatewayv2::ComAmazonawsApigatewayv2Apigatewayv2State;
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{element, form_encode};

/// The region every request below is signed for.
const REGION: &str = "us-west-2";

/// The EC2 API version a client sends in an ec2Query body.
const EC2_API_VERSION: &str = "2016-11-15";

/// The collection URI `CreateVpcLink` is bound to (`POST /v2/vpclinks`).
const VPC_LINKS: &str = "/v2/vpclinks";

/// One emulator carrying Amazon EC2 and API Gateway v2 over ONE shared engine, as the
/// production router mounts them, plus the engine itself so a case can assert on what a
/// refused create did NOT write.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_apigatewayv2_apigatewayv2::EmulatedService::new(Arc::clone(&engine)),
    );
    (router, engine)
}

/// A SigV4 `Authorization` header whose credential scope names `signing_name`, which is
/// what routes the request here; the signature is never checked.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}\
         /aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// One answer: its HTTP status and its body as JSON (`Value::Null` when it is not JSON).
struct Reply {
    status: u16,
    body: Value,
}

impl Reply {
    /// The wire error code (`__type` of the restJson1 error envelope), or `""` on success.
    fn error_code(&self) -> &str {
        self.body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
    }

    /// The error envelope's message, or `""` when there is none.
    fn message(&self) -> &str {
        self.body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
    }
}

/// Call one API Gateway v2 operation over its restJson1 wire.
async fn apigateway(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> Reply {
    let payload = body
        .map(|value| serde_json::to_vec(&value).expect("serialize the request body"))
        .unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("apigateway.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("apigateway"))
        .body(Bytes::from(payload))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    Reply { status, body }
}

/// One ec2Query action that must succeed, answered with its XML body.
async fn ec2(router: &MultiServiceRouter, action: &str, params: &[(&str, &str)]) -> String {
    let mut body = format!("Action={action}&Version={EC2_API_VERSION}");
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("ec2.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("ec2"))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 200,
        "setup: EC2 {action} must succeed, got {status} with {text}"
    );
    text
}

/// One VPC of the account with the subnets EC2 minted in it.
struct Vpc {
    id: String,
    subnets: Vec<String>,
}

/// Create one VPC carrying one subnet per entry of `zones`, through EC2's own operations.
async fn ec2_vpc(router: &MultiServiceRouter, second_octet: u8, zones: &[&str]) -> Vpc {
    let cidr = format!("10.{second_octet}.0.0/16");
    let id = element(&ec2(router, "CreateVpc", &[("CidrBlock", &cidr)]).await, "vpcId");
    let mut subnets = Vec::new();
    for (index, zone) in zones.iter().enumerate() {
        let subnet_cidr = format!("10.{second_octet}.{}.0/24", index + 1);
        subnets.push(element(
            &ec2(
                router,
                "CreateSubnet",
                &[
                    ("VpcId", id.as_str()),
                    ("CidrBlock", subnet_cidr.as_str()),
                    ("AvailabilityZone", zone),
                ],
            )
            .await,
            "subnetId",
        ));
    }
    Vpc { id, subnets }
}

/// A `CreateVpcLink` body naming `subnets`.
fn create_body(name: &str, subnets: &[String]) -> Value {
    json!({ "name": name, "subnetIds": subnets })
}

/// How many VPC links the account holds, read off the store API Gateway v2 writes into.
fn stored_links(engine: &engine::InMemoryEngine) -> usize {
    engine
        .state::<ComAmazonawsApigatewayv2Apigatewayv2State>()
        .lock()
        .vpc_links
        .len()
}

/// Values that name no subnet of the account, in both of EC2's published id forms.
fn unresolvable_subnet_ids() -> Vec<(&'static str, String)> {
    vec![
        (
            "EC2's 17-hex id form",
            "subnet-0f1e2d3c4b5a69780".to_string(),
        ),
        ("EC2's older 8-hex id form", "subnet-1a2b3c4d".to_string()),
    ]
}

#[tokio::test]
async fn a_vpc_link_is_built_only_in_subnets_ec2_holds_in_one_vpc() {
    let (router, engine) = emulator();
    let first = ec2_vpc(&router, 80, &["us-west-2a", "us-west-2b", "us-west-2c"]).await;
    let second = ec2_vpc(&router, 81, &["us-west-2a", "us-west-2b"]).await;
    let third = ec2_vpc(&router, 82, &["us-west-2c"]).await;

    // The accepted side of the rule, over subnet counts: each create names subnets EC2
    // itself minted in ONE VPC, so each is accepted and reports the very ids it was given.
    let mut accepted = 0usize;
    for (case, subnets) in [
        ("one of EC2's subnets", first.subnets[..1].to_vec()),
        ("two of EC2's subnets", first.subnets[..2].to_vec()),
        ("all three of one VPC's subnets", first.subnets.clone()),
        (
            "both subnets of the second VPC",
            second.subnets.clone(),
        ),
        ("the only subnet of the third VPC", third.subnets.clone()),
    ] {
        let created = apigateway(
            &router,
            "POST",
            VPC_LINKS,
            Some(create_body(&format!("resolved-{accepted}"), &subnets)),
        )
        .await;
        assert_eq!(
            (created.status, created.error_code()),
            (201, ""),
            "case {case:?}: a create placed in EC2's own subnets of one VPC must be accepted, \
             got {} with {}",
            created.status,
            created.body
        );
        let vpc_link_id = created
            .body
            .get("vpcLinkId")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: the create reported no VpcLinkId: {}", created.body))
            .to_string();
        accepted += 1;
        assert_eq!(
            stored_links(&engine),
            accepted,
            "case {case:?}: expected the account to hold {accepted} VPC link(s)"
        );

        // The round trip: the reader reports the subnets the link was built in.
        let read = apigateway(
            &router,
            "GET",
            &format!("{VPC_LINKS}/{}", form_encode(&vpc_link_id)),
            None,
        )
        .await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected 200 from GetVpcLink, got {} with {}",
            read.status, read.body
        );
        assert_eq!(
            read.body.get("subnetIds").cloned().unwrap_or(Value::Null),
            json!(subnets),
            "case {case:?}: expected GetVpcLink to report the subnets the link was built in, \
             got {}",
            read.body
        );
    }

    // The refusal side. One row per way the subnets fail to describe a buildable link: an
    // id EC2 holds no subnet for (in either published form, alone, last in the list or
    // first of three), and a list straddling two or three of EC2's own VPCs.
    //
    // (case, the subnets, the text the refusal must carry)
    let mut rows: Vec<(String, Vec<String>, String)> = Vec::new();
    for (form, unknown) in unresolvable_subnet_ids() {
        rows.push((
            format!("one subnet EC2 does not hold, in {form}"),
            vec![unknown.clone()],
            unknown.clone(),
        ));
        rows.push((
            format!("one subnet EC2 holds and then one it does not, in {form}"),
            vec![first.subnets[0].clone(), unknown.clone()],
            unknown.clone(),
        ));
        rows.push((
            format!("the FIRST of three subnets EC2 does not hold, in {form}"),
            vec![
                unknown.clone(),
                first.subnets[0].clone(),
                first.subnets[1].clone(),
            ],
            unknown.clone(),
        ));
    }
    rows.push((
        "one subnet from each of two of EC2's VPCs".to_string(),
        vec![first.subnets[0].clone(), second.subnets[0].clone()],
        SUBNETS_NOT_IN_ONE_VPC_MESSAGE.to_string(),
    ));
    rows.push((
        "the same two VPCs in the other order".to_string(),
        vec![second.subnets[0].clone(), first.subnets[0].clone()],
        SUBNETS_NOT_IN_ONE_VPC_MESSAGE.to_string(),
    ));
    rows.push((
        "two subnets of one VPC followed by one of another".to_string(),
        vec![
            first.subnets[0].clone(),
            first.subnets[1].clone(),
            second.subnets[1].clone(),
        ],
        SUBNETS_NOT_IN_ONE_VPC_MESSAGE.to_string(),
    ));
    rows.push((
        "one subnet from each of three of EC2's VPCs".to_string(),
        vec![
            first.subnets[0].clone(),
            second.subnets[0].clone(),
            third.subnets[0].clone(),
        ],
        SUBNETS_NOT_IN_ONE_VPC_MESSAGE.to_string(),
    ));

    for (index, (case, subnets, expected)) in rows.iter().enumerate() {
        let refused = apigateway(
            &router,
            "POST",
            VPC_LINKS,
            Some(create_body(&format!("dead-{index}"), subnets)),
        )
        .await;
        assert_eq!(
            (refused.status, refused.error_code()),
            (400, "BadRequestException"),
            "case {case:?}: expected a 400 BadRequestException, got {} with {}",
            refused.status,
            refused.body
        );
        assert!(
            refused.message().contains(expected.as_str()),
            "case {case:?}: expected the refusal to carry {expected:?}, got {}",
            refused.body
        );
        // Nothing was created: a refused create leaves the account exactly as it was.
        assert_eq!(
            stored_links(&engine),
            accepted,
            "case {case:?}: a refused CreateVpcLink must write no link"
        );
    }

    // And the same emulator still serves a well-formed create: the refusals consumed
    // nothing.
    let created = apigateway(
        &router,
        "POST",
        VPC_LINKS,
        Some(create_body("after-refusals", &third.subnets)),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case \"a valid create after the refusals\": expected 201, got {} with {}",
        created.status, created.body
    );
    assert_eq!(
        stored_links(&engine),
        accepted + 1,
        "case \"a valid create after the refusals\": expected the account to hold {} links",
        accepted + 1
    );

    // The VPC ids EC2 minted are distinct, which is what makes the cross-VPC rows above a
    // real straddle rather than three readings of one network.
    let vpcs = [&first.id, &second.id, &third.id];
    for (left, right) in [(0, 1), (0, 2), (1, 2)] {
        assert_ne!(
            vpcs[left], vpcs[right],
            "setup: EC2 must mint a distinct VPC id per VPC, got {:?} twice",
            vpcs[left]
        );
    }
}
