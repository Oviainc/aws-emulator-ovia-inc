// SPDX-License-Identifier: CC0-1.0

//! Directory Service resolves the AVAILABILITY ZONES of the subnets a directory would be
//! placed in against Amazon EC2, which owns them.
//!
//! RULE: a directory's servers sit in two subnets in two DIFFERENT Availability Zones
//! (`DirectoryVpcSettings$SubnetIds`: "The two subnets must be in different Availability
//! Zones. Directory Service creates a directory server and a DNS server in each of these
//! subnets"; `DirectoryConnectSettings$SubnetIds` places an AD Connector's the same way), so
//! every create that places servers in a VPC resolves the requested subnets against the
//! account's EC2 inventory BEFORE anything is minted — a placement whose subnets EC2 reports
//! in ONE zone is refused with the modelled `ClientException` (HTTP 400) stating that rule,
//! which is a DIFFERENT class to the `ValidationException` a malformed subnet id gets, and a
//! placement in subnets EC2 reports in two zones is accepted. A refused create stores
//! nothing: the directory it would have minted is in no listing.
//!
//! This suite lives beside the router because the zones are EC2's: the VPCs and subnets below
//! are minted by the real EC2 operations on the SAME engine that serves Directory Service, so
//! every id in an accepted request is one EC2 handed out rather than one this suite made up,
//! and every refusal is grounded in what EC2 actually answered.
//!
//! The rows differ along what the rule generalises over: which create places the servers
//! (`CreateDirectory`, `CreateMicrosoftAD`, `ConnectDirectory`), how many subnets the request
//! names (two and three), which zone they are all in, and — on the accepted side — which two
//! zones they span.
//!
//! Error source: `smithy_query(query="operation", operation="CreateMicrosoftAD,CreateDirectory,ConnectDirectory")`
//! — each models `ClientException`, `errorKind` client, `httpError` 400.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{element, form_encode};

/// The Region every request below is signed for, and so the Region the subnets live in.
const REGION: &str = "us-west-2";

/// The EC2 API version a client sends in an ec2Query body.
const EC2_API_VERSION: &str = "2016-11-15";

/// The `X-Amz-Target` prefix Directory Service names an operation under: its service shape's
/// name.
const DS_TARGET_PREFIX: &str = "DirectoryService_20150416";

/// A password the modelled `Password` pattern admits.
const PASSWORD: &str = "Passw0rd!2345";

/// The FIXED sentence a placement whose subnets all sit in one zone is refused with
/// (`state::subnet_zones::SUBNETS_IN_ONE_ZONE`).
const SUBNETS_IN_ONE_ZONE: &str =
    "Invalid subnet ID(s). The two subnets must be in different Availability Zones.";

/// Amazon EC2 and Directory Service over ONE shared engine, as the production router mounts
/// them.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_directoryservice_directoryservice_20150416::EmulatedService::new(
            engine,
        ),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `signing_name`, which is what
/// routes the request here; the signature is never checked.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}\
         /aws4_request, SignedHeaders=host, Signature=0000"
    )
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

/// One Directory Service control-plane call: its status and its parsed body.
async fn ds(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("ds.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{DS_TARGET_PREFIX}.{operation}"))
        .header("authorization", authorization("ds"))
        .body(Bytes::from(body.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let raw = response.into_body();
    let parsed = if raw.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(&raw).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// One VPC of the account carrying one subnet per entry of `zones`, created through EC2's own
/// operations, as `(vpc id, subnet ids)`.
async fn ec2_vpc(
    router: &MultiServiceRouter,
    second_octet: u8,
    zones: &[&str],
) -> (String, Vec<String>) {
    let cidr = format!("10.{second_octet}.0.0/16");
    let vpc_id = element(
        &ec2(router, "CreateVpc", &[("CidrBlock", &cidr)]).await,
        "vpcId",
    );
    let mut subnets = Vec::new();
    for (index, zone) in zones.iter().enumerate() {
        let subnet_cidr = format!("10.{second_octet}.{}.0/24", index + 1);
        subnets.push(element(
            &ec2(
                router,
                "CreateSubnet",
                &[
                    ("VpcId", vpc_id.as_str()),
                    ("CidrBlock", subnet_cidr.as_str()),
                    ("AvailabilityZone", zone),
                ],
            )
            .await,
            "subnetId",
        ));
    }
    (vpc_id, subnets)
}

/// Every directory id the account holds, read back through `DescribeDirectories`.
async fn stored_directories(router: &MultiServiceRouter) -> Vec<String> {
    let (status, body) = ds(router, "DescribeDirectories", json!({})).await;
    assert_eq!(
        status, 200,
        "reading the account's directories must succeed, got {status} with {body}"
    );
    body["DirectoryDescriptions"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|description| {
            description
                .get("DirectoryId")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect()
}

/// The create body of `operation`, placing its servers in `vpc_id`'s `subnets`.
fn create_body(operation: &str, name: &str, vpc_id: &str, subnets: &[String]) -> Value {
    let settings = json!({ "VpcId": vpc_id, "SubnetIds": subnets });
    match operation {
        "CreateDirectory" => json!({
            "Name": name,
            "Password": PASSWORD,
            "Size": "Small",
            "VpcSettings": settings,
        }),
        "CreateMicrosoftAD" => json!({
            "Name": name,
            "Password": PASSWORD,
            "VpcSettings": settings,
        }),
        "ConnectDirectory" => json!({
            "Name": name,
            "Password": PASSWORD,
            "Size": "Small",
            "ConnectSettings": {
                "VpcId": vpc_id,
                "SubnetIds": subnets,
                "CustomerDnsIps": ["10.0.0.10", "10.0.1.10"],
                "CustomerUserName": "Admin",
            },
        }),
        other => panic!("setup: {other} places no directory servers in a VPC"),
    }
}

/// The every-create list: the operations whose request places a directory's servers in a VPC.
const PLACEMENTS: [&str; 3] = ["CreateDirectory", "CreateMicrosoftAD", "ConnectDirectory"];

#[tokio::test]
async fn a_directory_is_placed_only_in_subnets_ec2_holds_in_two_zones() {
    let router = emulator();
    // One VPC whose subnets span three zones, and one whose two subnets are both in a single
    // zone — each built by EC2 itself, so every id below is one EC2 handed out.
    let (spread_vpc, spread) = ec2_vpc(&router, 70, &["us-west-2a", "us-west-2b", "us-west-2c"]).await;
    let (one_zone_vpc, one_zone) = ec2_vpc(&router, 71, &["us-west-2a", "us-west-2a"]).await;
    let (crowded_vpc, crowded) =
        ec2_vpc(&router, 72, &["us-west-2c", "us-west-2c", "us-west-2c"]).await;

    // The refused side: every create, over placements that differ in how many subnets they
    // name and which single zone those subnets are in.
    let refused: Vec<(&str, &String, Vec<String>)> = vec![
        ("two subnets in one zone", &one_zone_vpc, one_zone.clone()),
        (
            "three subnets in one other zone",
            &crowded_vpc,
            crowded.clone(),
        ),
        (
            "two subnets of the crowded VPC",
            &crowded_vpc,
            crowded[..2].to_vec(),
        ),
    ];
    let mut index = 0usize;
    for (placement, vpc_id, subnets) in &refused {
        for operation in PLACEMENTS {
            let case = format!("{operation} with {placement}");
            index += 1;
            let (status, body) = ds(
                &router,
                operation,
                create_body(
                    operation,
                    &format!("one-zone{index}.example.com"),
                    vpc_id,
                    subnets,
                ),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: a placement EC2 reports in one zone must be refused, got \
                 {status} with {body}"
            );
            assert_eq!(
                body["__type"],
                json!("ClientException"),
                "case {case:?}: expected the modelled resource-semantics refusal, got {body}"
            );
            let message = body["Message"].as_str().unwrap_or_default();
            let request_id = body["RequestId"].as_str().unwrap_or_default();
            assert_eq!(
                message,
                format!("{SUBNETS_IN_ONE_ZONE} : RequestId: {request_id}"),
                "case {case:?}: expected the zone-topology sentence, got {message:?}"
            );
            assert!(
                body.get("DirectoryId").is_none(),
                "case {case:?}: a refused create must mint no directory, got {body}"
            );
        }
    }

    // Nothing was stored by any of those: the account still holds no directory.
    let after_refusals = stored_directories(&router).await;
    assert!(
        after_refusals.is_empty(),
        "case \"after the refused placements\": expected no directory to have been stored, \
         got {after_refusals:?}"
    );

    // A MALFORMED subnet id is a different failure, refused by the boundary as a
    // `ValidationException` rather than by this rule — which is what makes the two
    // distinguishable to a client.
    let (status, body) = ds(
        &router,
        "CreateMicrosoftAD",
        create_body(
            "CreateMicrosoftAD",
            "malformed.example.com",
            &spread_vpc,
            &["not-a-subnet".to_string(), spread[1].clone()],
        ),
    )
    .await;
    assert_eq!(
        (status, &body["__type"]),
        (400, &json!("ValidationException")),
        "case \"a malformed subnet id\": expected the boundary's own refusal, got {status} \
         with {body}"
    );

    // The accepted side: every create, over pairs EC2 reports in two DIFFERENT zones, taken
    // in both orders, plus a three-subnet placement spanning three zones.
    let accepted: Vec<(&str, Vec<String>)> = vec![
        ("two zones", vec![spread[0].clone(), spread[1].clone()]),
        (
            "the same two zones in the other order",
            vec![spread[1].clone(), spread[0].clone()],
        ),
        ("another pair of zones", vec![spread[1].clone(), spread[2].clone()]),
        ("three zones", spread.clone()),
    ];
    let mut expected: Vec<String> = Vec::new();
    for (placement, subnets) in &accepted {
        for operation in PLACEMENTS {
            let case = format!("{operation} with {placement}");
            index += 1;
            let (status, body) = ds(
                &router,
                operation,
                create_body(
                    operation,
                    &format!("two-zone{index}.example.com"),
                    &spread_vpc,
                    subnets,
                ),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: a placement EC2 reports in two zones must be served, got \
                 {status} with {body}"
            );
            let directory_id = body["DirectoryId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a DirectoryId, got {body}"))
                .to_string();
            expected.push(directory_id);
        }
    }

    // Every accepted create is backed by a stored directory, and the refused ones added
    // nothing: the listing holds exactly the ids the accepted creates minted.
    let mut stored = stored_directories(&router).await;
    stored.sort();
    expected.sort();
    assert_eq!(
        stored, expected,
        "case \"every accepted placement\": expected the listing to hold exactly the \
         directories the served creates minted"
    );
}
