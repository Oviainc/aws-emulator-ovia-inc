// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListDiscoveredResources` — the organisation's inventory
//! of "resources in the organization's accounts that are available to be
//! associated with a resource set".
//!
//! RULE under test: for one member account and one resource type, Firewall
//! Manager reports exactly the resources the service that OWNS that type holds
//! for that account in the Region the request addresses — each as the ARN it
//! would be associated by, its type, its owning account and its name — in
//! bounded pages; it keeps no inventory of its own, so a resource created
//! through the owning service's API is discoverable with no propagation step and
//! one deleted there stops being discovered, and a type no emulated service owns
//! and an account that owns nothing both report an EMPTY list.
//!
//! LIMIT of the Region half of that rule, asserted as such in
//! [`a_resource_whose_service_records_no_region_is_discovered_from_every_region`]:
//! a resource is attributed to the Region its owning service RECORDED, so a
//! service whose records keep neither an ARN nor a Region (EC2's VPC, instance,
//! network-interface and address stores) has none to attribute, and its
//! resources are reported from every Region under an ARN naming the Region that
//! asked — the same answer that service's own Region-agnostic describes give.
//! Narrowing that needs those records to carry a Region, which is EC2's to
//! change, not Firewall Manager's.
//!
//! The table in [`discovers_what_the_owning_services_hold`] is the class that
//! rule ranges over: one row per resource type, the rows differing in the OWNING
//! SERVICE (Amazon EC2, Network Firewall, Elastic Load Balancing v2, API
//! Gateway), in the protocol the resource was created over (an `ec2Query` form
//! POST, an awsJson1_0 POST, an awsQuery form POST, a `restJson1` POST), in the
//! shape of the resource's ARN (including API Gateway's, whose account field is
//! EMPTY), in whether the name is an id or a caller-chosen name, and in count.
//! The resource types come from the catalogue
//! [`state::DISCOVERABLE_RESOURCE_TYPES`], itself sourced from the types
//! `Policy$ResourceType` documents. The two catalogue entries no row creates a
//! resource for — CloudFront's distribution and the Classic load balancer — are
//! asserted only on the EMPTY inventory their mounted-but-empty store answers
//! with, in [`an_inventory_with_nothing_in_it_is_empty`].
//!
//! Every call here goes over the wire through one `MultiServiceRouter` with
//! Firewall Manager AND every owning service mounted on ONE shared engine — the
//! way the emulator's own router assembles them, which is what makes the
//! cross-service read under test real.

use std::collections::BTreeSet;
use std::sync::Arc;

use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use serde_json::{json, Value};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use service_com_amazonaws_fms_awsfms_20180101::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::administrator_gated_operations::{associate_admin, with_store};
use crate::policy_lifecycle::{call, call_in, ACCOUNT, REGION};

/// A second Region, to assert a resource is discoverable in the Region its
/// owning service recorded it in rather than in every Region.
const OTHER_REGION: &str = "us-east-1";

/// An account id that is not this emulator's, so nothing it holds belongs to it.
const FOREIGN_ACCOUNT: &str = "222233334444";

/// The image id the EC2 launches below name. Any well-formed `ami-` id serves:
/// what is under test is the instance that ends up in EC2's store, not the image.
const AMI: &str = "ami-0123456789abcdef0";

/// The emulator the tests drive: one shared engine with Firewall Manager and the
/// services that own the resources it discovers mounted on one router.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_fms_awsfms_20180101::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(ec2::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(
        service_com_amazonaws_networkfirewall_networkfirewall_20201112::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_elasticloadbalancing_elasticloadbalancing_v7::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_cloudfront_cloudfront2020_05_31::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_apigateway_backplanecontrolservice::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    (router, engine)
}

/// Everything an `application/x-www-form-urlencoded` value may NOT carry
/// literally: every byte outside RFC 3986's unreserved set (`A-Za-z0-9` and
/// `-._~`), which is what an AWS SDK's own query serializer escapes.
const FORM_VALUE: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

/// Percent-encode one form value with the `percent-encoding` crate, so no value
/// a later row adds — a CIDR block's `/`, an ARN's `:`, a name carrying `&` or a
/// non-ASCII character — can be silently mangled by a hand-written encoder.
fn form_value(value: &str) -> String {
    utf8_percent_encode(value, FORM_VALUE).to_string()
}

/// One awsQuery/ec2Query call: an `application/x-www-form-urlencoded` POST whose
/// body carries `Action=…&Version=…`, answered with XML. `signing_name` is the
/// SigV4 credential scope's service, which is what the router routes on.
async fn form_call(
    router: &MultiServiceRouter,
    region: &str,
    signing_name: &str,
    action: &str,
    version: &str,
    params: &[(&str, String)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={version}");
    for (name, value) in params {
        body.push('&');
        body.push_str(name);
        body.push('=');
        body.push_str(&form_value(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{signing_name}.{region}.amazonaws.com/"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEXAMPLE/20260101/{region}/{signing_name}/aws4_request, \
                 SignedHeaders=host, Signature=deadbeef"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("a well-formed query request");
    let response = router.dispatch(request).await;
    (
        response.status().as_u16(),
        String::from_utf8_lossy(response.body()).to_string(),
    )
}

/// One EC2 call, signed for `region`: EC2 speaks `ec2Query`.
async fn ec2_call(
    router: &MultiServiceRouter,
    region: &str,
    action: &str,
    params: &[(&str, String)],
) -> (u16, String) {
    form_call(router, region, "ec2", action, "2016-11-15", params).await
}

/// One Elastic Load Balancing v2 call. Both load balancer models declare the
/// `elasticloadbalancing` signing name, which the Classic front door claims, so
/// the v2 model is addressed by its OWN routing key here.
async fn elbv2_call(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(&str, String)],
) -> (u16, String) {
    form_call(
        router,
        REGION,
        "elasticloadbalancingv2",
        action,
        "2015-12-01",
        params,
    )
    .await
}

/// One API Gateway call: `restJson1` over the operation's own HTTP method and
/// URI, signed for [`REGION`]. Returns the status and the parsed JSON body.
async fn api_gateway_call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("apigateway.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEXAMPLE/20260101/{REGION}/apigateway/aws4_request, \
                 SignedHeaders=host, Signature=deadbeef"
            ),
        )
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed restJson1 request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// An API Gateway call that must answer the modelled 201, and the body it
/// answered.
async fn api_gateway_created(
    case: &str,
    router: &MultiServiceRouter,
    uri: &str,
    body: Value,
) -> Value {
    let (status, parsed) = api_gateway_call(router, "POST", uri, Some(body)).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected HTTP 201 from POST {uri}, got {status} with {parsed}"
    );
    parsed
}

/// An EC2 call that must succeed, and the XML it answered.
async fn ec2_ok(
    case: &str,
    router: &MultiServiceRouter,
    action: &str,
    params: &[(&str, String)],
) -> String {
    let (status, body) = ec2_call(router, REGION, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with {body}"
    );
    body
}

/// One Network Firewall call, which speaks awsJson1_0 with an `X-Amz-Target`
/// header, signed for [`REGION`] so the ARNs it mints name that Region.
async fn network_firewall(router: &MultiServiceRouter, target: &str, body: &Value) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://network-firewall.{REGION}.amazonaws.com/"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("NetworkFirewall_20201112.{target}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEXAMPLE/20260101/{REGION}/network-firewall/aws4_request, \
                 SignedHeaders=host, Signature=deadbeef"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a valid request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    assert_eq!(
        status, 200,
        "case {target:?}: expected HTTP 200, got {status} with {parsed}"
    );
    parsed
}

/// Every `<tag>…</tag>` text of an ec2Query XML body, in document order.
fn elements(xml: &str, tag: &str) -> Vec<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    xml.split(open.as_str())
        .skip(1)
        .filter_map(|rest| rest.split_once(close.as_str()))
        .map(|(value, _)| value.to_string())
        .collect()
}

/// The first `<tag>` text of an ec2Query XML body, which is the id a create
/// answers with.
fn element(case: &str, xml: &str, tag: &str) -> String {
    elements(xml, tag)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: expected a <{tag}> in {xml}"))
}

/// `(key, value)` form fields for [`ec2_call`].
fn p(key: &'static str, value: &str) -> (&'static str, String) {
    (key, value.to_string())
}

/// `ListDiscoveredResources` for one account and type, with the optional page
/// bound and token, as the parsed body.
async fn list(
    router: &MultiServiceRouter,
    account: &str,
    resource_type: &str,
    max_results: Option<i64>,
    token: Option<&str>,
) -> Value {
    let mut request = json!({ "MemberAccountIds": [account], "ResourceType": resource_type });
    if let Some(max) = max_results {
        request["MaxResults"] = json!(max);
    }
    if let Some(token) = token {
        request["NextToken"] = json!(token);
    }
    let (status, body) = call(router, "ListDiscoveredResources", &request).await;
    assert_eq!(
        status, 200,
        "case {account}/{resource_type}: expected HTTP 200, got {status} with {body}"
    );
    body
}

/// The `Items` of one page, as `(URI, AccountId, Type, Name)` quadruples.
fn items(body: &Value) -> Vec<(String, String, String, String)> {
    body["Items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| {
            let text = |member: &str| {
                item[member]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            };
            (text("URI"), text("AccountId"), text("Type"), text("Name"))
        })
        .collect()
}

/// Every URI the inventory reports for one account and type, walking every page
/// so a bound cannot hide a resource.
async fn every_uri(router: &MultiServiceRouter, account: &str, resource_type: &str) -> Vec<String> {
    let mut seen = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let body = list(router, account, resource_type, None, token.as_deref()).await;
        seen.extend(items(&body).into_iter().map(|(uri, ..)| uri));
        token = body["NextToken"].as_str().map(str::to_string);
        if token.is_none() {
            return seen;
        }
    }
}

/// One row of the catalogue table: the resource type, the resources the owning
/// service was made to hold as `(URI, Name)`, and whether those are ALL the
/// resources of that type the emulator holds (true when the emulator starts with
/// none of them, so the reported set must equal the created set exactly).
struct Row {
    resource_type: &'static str,
    created: Vec<(String, String)>,
    exhaustive: bool,
}

/// An EC2 ARN of this account in [`REGION`], the URI a resource set associates an
/// EC2 resource by.
fn ec2_arn(resource: &str) -> String {
    format!("arn:aws:ec2:{REGION}:{ACCOUNT}:{resource}")
}

/// RULE: the inventory is what the OWNING service holds. Each row creates its
/// resources through that service's own API and then requires Firewall Manager
/// to report them — URI, account, type and name — and to report nothing else of
/// another type, another account or another Region.
#[tokio::test]
async fn discovers_what_the_owning_services_hold() {
    let (router, engine) = emulator();
    associate_admin(&router).await;

    // --- Amazon EC2, over ec2Query -----------------------------------------
    let vpc = element(
        "vpc",
        &ec2_ok(
            "vpc",
            &router,
            "CreateVpc",
            &[p("CidrBlock", "10.0.0.0/16")],
        )
        .await,
        "vpcId",
    );
    let second_vpc = element(
        "second vpc",
        &ec2_ok(
            "second vpc",
            &router,
            "CreateVpc",
            &[p("CidrBlock", "10.1.0.0/16")],
        )
        .await,
        "vpcId",
    );
    let subnet = element(
        "subnet",
        &ec2_ok(
            "subnet",
            &router,
            "CreateSubnet",
            &[p("VpcId", &vpc), p("CidrBlock", "10.0.1.0/24")],
        )
        .await,
        "subnetId",
    );
    let group_name = "fms-discovered-group";
    let group = element(
        "security group",
        &ec2_ok(
            "security group",
            &router,
            "CreateSecurityGroup",
            &[
                p("GroupName", group_name),
                p("GroupDescription", "a group the inventory must report"),
                p("VpcId", &vpc),
            ],
        )
        .await,
        "groupId",
    );
    let allocation = element(
        "elastic ip",
        &ec2_ok(
            "elastic ip",
            &router,
            "AllocateAddress",
            &[p("Domain", "vpc")],
        )
        .await,
        "allocationId",
    );
    // Three instances, so the paging test has more than one page to walk.
    let instances = elements(
        &ec2_ok(
            "instances",
            &router,
            "RunInstances",
            &[p("ImageId", AMI), p("MinCount", "3"), p("MaxCount", "3")],
        )
        .await,
        "instanceId",
    );
    assert_eq!(
        instances.len(),
        3,
        "RunInstances must mint three instances, got {instances:?}"
    );
    // `CreateNetworkInterface` is still a scaffold stub with its own open issue,
    // so the interface is created through the EC2 store's own writer seam — the
    // same seam that create will call — rather than through a second copy of it.
    let interface = {
        let handle = ec2::state::service_state(&*engine).expect("the EC2 store");
        let mut store = handle.lock();
        store
            .insert_network_interface(ec2::state::NetworkInterfaceSpec {
                subnet_id: subnet.clone(),
                vpc_id: vpc.clone(),
                ..ec2::state::NetworkInterfaceSpec::default()
            })
            .expect("the EC2 store takes one network interface")
    };

    // --- Network Firewall, over awsJson1_0 ---------------------------------
    let policy = network_firewall(
        &router,
        "CreateFirewallPolicy",
        &json!({
            "FirewallPolicyName": "fms-discovery-policy",
            "FirewallPolicy": {
                "StatelessDefaultActions": ["aws:forward_to_sfe"],
                "StatelessFragmentDefaultActions": ["aws:forward_to_sfe"],
            },
        }),
    )
    .await;
    let policy_arn = policy["FirewallPolicyResponse"]["FirewallPolicyArn"]
        .as_str()
        .expect("a minted FirewallPolicyArn")
        .to_string();
    let firewall_name = "fms-discovered-firewall";
    let firewall = network_firewall(
        &router,
        "CreateFirewall",
        &json!({
            "FirewallName": firewall_name,
            "FirewallPolicyArn": policy_arn,
            "VpcId": vpc,
            "SubnetMappings": [{ "SubnetId": subnet }],
        }),
    )
    .await;
    let firewall_arn = firewall["Firewall"]["FirewallArn"]
        .as_str()
        .expect("a minted FirewallArn")
        .to_string();

    // --- Elastic Load Balancing v2, over awsQuery -------------------------
    let load_balancer_name = "fms-discovered-lb";
    let (status, created) = elbv2_call(
        &router,
        "CreateLoadBalancer",
        &[
            p("Name", load_balancer_name),
            p("Subnets.member.1", &subnet),
            p("Subnets.member.2", "subnet-11111111"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"load balancer\": expected HTTP 200 from CreateLoadBalancer, got {status} with \
         {created}"
    );
    let load_balancer_arn = element("load balancer", &created, "LoadBalancerArn");

    // --- API Gateway, over restJson1 ---------------------------------------
    let api = api_gateway_created(
        "rest api",
        &router,
        "/restapis",
        json!({ "name": "fms-discovered-api" }),
    )
    .await["id"]
        .as_str()
        .expect("a minted REST API id")
        .to_string();
    let stage_name = "prod";
    api_gateway_created(
        "stage",
        &router,
        &format!("/restapis/{api}/deployments"),
        json!({ "stageName": stage_name }),
    )
    .await;
    // An API Gateway ARN carries an EMPTY account field, which is the form that
    // service's own `state::service_arn` mints.
    let stage_uri = format!("arn:aws:apigateway:{REGION}::/restapis/{api}/stages/{stage_name}");

    // One row per resource type, created through the service that owns it.
    let rows = vec![
        Row {
            resource_type: state::RESOURCE_TYPE_EC2_VPC,
            created: vec![
                (ec2_arn(&format!("vpc/{vpc}")), vpc.clone()),
                (ec2_arn(&format!("vpc/{second_vpc}")), second_vpc.clone()),
            ],
            // A fresh account already has its default VPC, which EC2 holds and
            // the inventory therefore reports beside these two.
            exhaustive: false,
        },
        Row {
            resource_type: state::RESOURCE_TYPE_EC2_SUBNET,
            created: vec![(ec2_arn(&format!("subnet/{subnet}")), subnet.clone())],
            exhaustive: false,
        },
        Row {
            resource_type: state::RESOURCE_TYPE_EC2_SECURITY_GROUP,
            created: vec![(
                ec2_arn(&format!("security-group/{group}")),
                group_name.to_string(),
            )],
            // Each VPC comes with its own default security group.
            exhaustive: false,
        },
        Row {
            resource_type: state::RESOURCE_TYPE_EC2_NETWORK_INTERFACE,
            created: vec![(
                ec2_arn(&format!("network-interface/{interface}")),
                interface.clone(),
            )],
            exhaustive: true,
        },
        Row {
            resource_type: state::RESOURCE_TYPE_EC2_INSTANCE,
            created: instances
                .iter()
                .map(|id| (ec2_arn(&format!("instance/{id}")), id.clone()))
                .collect(),
            exhaustive: true,
        },
        Row {
            resource_type: state::RESOURCE_TYPE_EC2_EIP,
            created: vec![(
                ec2_arn(&format!("elastic-ip/{allocation}")),
                allocation.clone(),
            )],
            exhaustive: true,
        },
        Row {
            resource_type: state::RESOURCE_TYPE_NETWORK_FIREWALL_FIREWALL,
            created: vec![(firewall_arn.clone(), firewall_name.to_string())],
            exhaustive: true,
        },
        Row {
            resource_type: state::RESOURCE_TYPE_ELBV2_LOAD_BALANCER,
            created: vec![(load_balancer_arn.clone(), load_balancer_name.to_string())],
            exhaustive: true,
        },
        Row {
            resource_type: state::RESOURCE_TYPE_APIGATEWAY_STAGE,
            created: vec![(stage_uri.clone(), stage_name.to_string())],
            exhaustive: true,
        },
    ];

    for row in &rows {
        let case = row.resource_type;
        assert!(
            state::DISCOVERABLE_RESOURCE_TYPES.contains(&case),
            "case {case:?}: the row's type is not in the catalogue {:?}",
            state::DISCOVERABLE_RESOURCE_TYPES
        );
        let body = list(&router, ACCOUNT, case, None, None).await;
        let reported = items(&body);

        // Every resource the owning service was made to hold is reported once,
        // with the URI it would be associated by and the members the model
        // declares for it.
        for (uri, name) in &row.created {
            let matching: Vec<_> = reported
                .iter()
                .filter(|(reported_uri, ..)| reported_uri == uri)
                .collect();
            assert_eq!(
                matching.len(),
                1,
                "case {case:?}: expected {uri} to be reported exactly once, got {matching:?} \
                 in {body}"
            );
            let (_, account_id, reported_type, reported_name) = matching[0];
            assert_eq!(
                (account_id.as_str(), reported_type.as_str(), reported_name.as_str()),
                (ACCOUNT, case, name.as_str()),
                "case {case:?}: expected {uri} reported for account {ACCOUNT} as {case} named \
                 {name}, got {:?} in {body}",
                matching[0]
            );
        }

        // Nothing of another type, another account or another Region is reported.
        for (uri, account_id, reported_type, _) in &reported {
            assert_eq!(
                (account_id.as_str(), reported_type.as_str()),
                (ACCOUNT, case),
                "case {case:?}: an item outside the filter was reported: {uri} \
                 ({account_id}/{reported_type})"
            );
            let region_segment = uri.split(':').nth(3).unwrap_or_default();
            assert!(
                region_segment.is_empty() || region_segment == REGION,
                "case {case:?}: {uri} is not a resource of {REGION}"
            );
            // The reported owner and the URI's own account field name the SAME
            // account, so a resource set offered this URI accepts it from the
            // account it was reported for. An ARN whose account field is empty
            // (API Gateway's) carries no account to disagree with.
            let account_segment = uri.split(':').nth(4).unwrap_or_default();
            assert!(
                account_segment.is_empty() || account_segment == account_id,
                "case {case:?}: {uri} is reported for account {account_id}, so its own account \
                 field must name that account, got {account_segment:?}"
            );
        }

        if row.exhaustive {
            let expected: BTreeSet<&str> =
                row.created.iter().map(|(uri, _)| uri.as_str()).collect();
            let got: BTreeSet<&str> = reported.iter().map(|(uri, ..)| uri.as_str()).collect();
            assert_eq!(
                got, expected,
                "case {case:?}: the emulator held none of this type before, so the inventory \
                 must be exactly what was created, got {body}"
            );
        }

        // A REPORTING operation: the second call answers what the first did.
        let again = list(&router, ACCOUNT, case, None, None).await;
        assert_eq!(
            items(&again),
            reported,
            "case {case:?}: two calls with nothing in between must answer identically, got \
             {again} after {body}"
        );
    }

    // The VPC rows agree with what EC2 itself reports: the inventory is that
    // store, not a copy of it.
    let described = ec2_ok("describe vpcs", &router, "DescribeVpcs", &[]).await;
    let held: BTreeSet<String> = elements(&described, "vpcId").into_iter().collect();
    let discovered: BTreeSet<String> = every_uri(&router, ACCOUNT, state::RESOURCE_TYPE_EC2_VPC)
        .await
        .into_iter()
        .map(|uri| uri.rsplit('/').next().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        discovered, held,
        "the discovered VPCs must be the VPCs DescribeVpcs reports, got {discovered:?} against \
         {held:?}"
    );
}

/// RULE: a resource is discoverable in the Region its owning service recorded it
/// in. The default subnets a fresh EC2 account holds were recorded in
/// `us-east-1`, so they are reported there and not in another Region, while a
/// subnet created in [`REGION`] is reported only there.
#[tokio::test]
async fn a_resource_is_discovered_in_its_own_region() {
    let (router, engine) = emulator();
    associate_admin(&router).await;

    let vpc = element(
        "vpc",
        &ec2_ok("vpc", &router, "CreateVpc", &[p("CidrBlock", "10.0.0.0/16")]).await,
        "vpcId",
    );
    let subnet = element(
        "subnet",
        &ec2_ok(
            "subnet",
            &router,
            "CreateSubnet",
            &[p("VpcId", &vpc), p("CidrBlock", "10.0.1.0/24")],
        )
        .await,
        "subnetId",
    );
    let subnet_uri = ec2_arn(&format!("subnet/{subnet}"));

    // The subnets EC2 holds whose own ARN names a Region other than REGION —
    // read from EC2's store only to pick the case, never to derive the answer.
    let elsewhere: Vec<String> = {
        let handle = ec2::state::service_state(&*engine).expect("the EC2 store");
        let store = handle.lock();
        store
            .subnets
            .values()
            .filter_map(|held| held.subnet_arn.clone())
            .filter(|arn| arn.split(':').nth(3) == Some(OTHER_REGION))
            .collect()
    };
    assert!(
        !elsewhere.is_empty(),
        "a fresh EC2 account holds its default subnets in {OTHER_REGION}, so there must be a \
         case to test"
    );

    let here = every_uri(&router, ACCOUNT, state::RESOURCE_TYPE_EC2_SUBNET).await;
    assert!(
        here.contains(&subnet_uri),
        "a subnet created in {REGION} must be discoverable there, got {here:?}"
    );
    for arn in &elsewhere {
        assert!(
            !here.contains(arn),
            "case {arn:?}: a subnet of {OTHER_REGION} must not be discoverable in {REGION}, \
             got {here:?}"
        );
    }

    // The same request addressed at the other Region answers the mirror image.
    let mut there = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut request = json!({
            "MemberAccountIds": [ACCOUNT],
            "ResourceType": state::RESOURCE_TYPE_EC2_SUBNET,
        });
        if let Some(token) = token.as_deref() {
            request["NextToken"] = json!(token);
        }
        let (status, body) = call_in(
            &router,
            "ListDiscoveredResources",
            &request,
            OTHER_REGION,
        )
        .await;
        assert_eq!(
            status, 200,
            "expected HTTP 200 from a {OTHER_REGION} listing, got {status} with {body}"
        );
        there.extend(items(&body).into_iter().map(|(uri, ..)| uri));
        token = body["NextToken"].as_str().map(str::to_string);
        if token.is_none() {
            break;
        }
    }
    for arn in &elsewhere {
        assert!(
            there.contains(arn),
            "case {arn:?}: a subnet of {OTHER_REGION} must be discoverable there, got {there:?}"
        );
    }
    assert!(
        !there.contains(&subnet_uri),
        "a subnet of {REGION} must not be discoverable in {OTHER_REGION}, got {there:?}"
    );
}

/// The other half of the Region rule, and this emulator's documented limit on
/// it: a resource whose owning service records NEITHER an ARN nor a Region for
/// it has no recorded Region to be attributed to, so it is reported in whichever
/// Region asks, under an ARN naming the Region that asked — exactly as that
/// service's own Region-agnostic describes report it.
///
/// EC2's VPC, instance, network-interface and address stores are such stores
/// (unlike its subnet and security-group records, which keep `SubnetArn` /
/// `SecurityGroupArn` and are filtered by Region above). This case therefore
/// DOCUMENTS the chosen behaviour instead of asserting a Region filter the
/// owning state cannot support; narrowing it needs those EC2 records to carry a
/// Region or an ARN, which belongs to that service rather than to Firewall
/// Manager.
#[tokio::test]
async fn a_resource_whose_service_records_no_region_is_discovered_from_every_region() {
    let (router, _engine) = emulator();
    associate_admin(&router).await;

    let vpc = element(
        "vpc",
        &ec2_ok("vpc", &router, "CreateVpc", &[p("CidrBlock", "10.0.0.0/16")]).await,
        "vpcId",
    );

    // One VPC, created once, asked for from two Regions: each answer names the
    // Region it was asked in, and neither invents a second VPC.
    for region in [REGION, OTHER_REGION] {
        let mut seen = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut request = json!({
                "MemberAccountIds": [ACCOUNT],
                "ResourceType": state::RESOURCE_TYPE_EC2_VPC,
            });
            if let Some(token) = token.as_deref() {
                request["NextToken"] = json!(token);
            }
            let (status, body) =
                call_in(&router, "ListDiscoveredResources", &request, region).await;
            assert_eq!(
                status, 200,
                "case {region:?}: expected HTTP 200, got {status} with {body}"
            );
            seen.extend(items(&body).into_iter().map(|(uri, ..)| uri));
            token = body["NextToken"].as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        let expected = format!("arn:aws:ec2:{region}:{ACCOUNT}:vpc/{vpc}");
        assert!(
            seen.contains(&expected),
            "case {region:?}: a VPC whose record keeps no Region is discoverable from every \
             Region, as {expected:?}; got {seen:?}"
        );
        assert_eq!(
            seen.iter().filter(|uri| *uri == &expected).count(),
            1,
            "case {region:?}: the one VPC must be reported once, got {seen:?}"
        );
    }
}

/// RULE: nothing is discovered for an account that owns nothing, or for a
/// resource type no emulated service holds resources of — an empty `Items` and
/// no `NextToken`, never a refusal and never an invented resource.
#[tokio::test]
async fn an_inventory_with_nothing_in_it_is_empty() {
    let (router, _engine) = emulator();
    associate_admin(&router).await;

    // Make EC2 hold something, so an empty answer is about the FILTER rather
    // than about an emulator that holds nothing at all.
    let vpc = element(
        "vpc",
        &ec2_ok("vpc", &router, "CreateVpc", &[p("CidrBlock", "10.0.0.0/16")]).await,
        "vpcId",
    );
    assert!(
        every_uri(&router, ACCOUNT, state::RESOURCE_TYPE_EC2_VPC)
            .await
            .contains(&ec2_arn(&format!("vpc/{vpc}"))),
        "the VPC must be discoverable before the empty cases are asserted"
    );

    // Rows differ in WHY the inventory is empty: a foreign account, catalogued
    // types whose owning service is mounted and holds nothing, a documented type
    // no emulated service owns, and a type that is not a resource type at all.
    let cases = [
        (FOREIGN_ACCOUNT, state::RESOURCE_TYPE_EC2_VPC),
        (ACCOUNT, state::RESOURCE_TYPE_ELBV2_LOAD_BALANCER),
        (ACCOUNT, state::RESOURCE_TYPE_CLOUDFRONT_DISTRIBUTION),
        (ACCOUNT, state::RESOURCE_TYPE_APIGATEWAY_STAGE),
        (ACCOUNT, state::RESOURCE_TYPE_ELB_LOAD_BALANCER),
        (ACCOUNT, "AWS::Route53Resolver::ResolverQueryLogConfig"),
        (ACCOUNT, "not-a-resource-type"),
    ];
    for (account, resource_type) in cases {
        let body = list(&router, account, resource_type, None, None).await;
        assert_eq!(
            body["Items"],
            json!([]),
            "case {account}/{resource_type}: expected an empty inventory, got {body}"
        );
        assert!(
            body["NextToken"].is_null(),
            "case {account}/{resource_type}: an empty page carries no NextToken, got {body}"
        );
    }
}

/// RULE: the inventory is served in bounded pages — `MaxResults` bounds a page,
/// the token walks the rest, each resource appears exactly once and the last
/// page carries no token.
#[tokio::test]
async fn paging_walks_the_inventory_once() {
    let (router, _engine) = emulator();
    associate_admin(&router).await;

    let instances = elements(
        &ec2_ok(
            "instances",
            &router,
            "RunInstances",
            &[p("ImageId", AMI), p("MinCount", "5"), p("MaxCount", "5")],
        )
        .await,
        "instanceId",
    );
    let expected: BTreeSet<String> = instances
        .iter()
        .map(|id| ec2_arn(&format!("instance/{id}")))
        .collect();
    assert_eq!(
        expected.len(),
        5,
        "RunInstances must mint five instances, got {instances:?}"
    );

    // Rows differ in page size, including one larger than the inventory and the
    // whole inventory in one page.
    for max_results in [1_i64, 2, 5, 100] {
        let case = format!("MaxResults={max_results}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let body = list(
                &router,
                ACCOUNT,
                state::RESOURCE_TYPE_EC2_INSTANCE,
                Some(max_results),
                token.as_deref(),
            )
            .await;
            let page = items(&body);
            pages += 1;
            assert!(
                i64::try_from(page.len()).unwrap_or(i64::MAX) <= max_results,
                "case {case}: a page must hold at most {max_results} items, got {body}"
            );
            assert!(
                pages <= 8,
                "case {case}: paging must terminate, got {pages} pages and {seen:?}"
            );
            seen.extend(page.into_iter().map(|(uri, ..)| uri));
            token = body["NextToken"].as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        let unique: BTreeSet<String> = seen.iter().cloned().collect();
        assert_eq!(
            unique.len(),
            seen.len(),
            "case {case}: paging must not repeat a resource, got {seen:?}"
        );
        assert_eq!(
            unique, expected,
            "case {case}: paging must report every instance once, got {seen:?}"
        );
    }

    // A token this service did not mint names no page boundary.
    let (status, body) = call(
        &router,
        "ListDiscoveredResources",
        &json!({
            "MemberAccountIds": [ACCOUNT],
            "ResourceType": state::RESOURCE_TYPE_EC2_INSTANCE,
            "NextToken": "not-a-token-this-service-minted",
        }),
    )
    .await;
    assert_eq!(
        (status, crate::policy_lifecycle::error_code(&body).as_str()),
        (400, "InvalidInputException"),
        "a token this service did not mint must be refused, got {status} with {body}"
    );
}

/// RULE: Firewall Manager keeps no copy of the inventory, so a resource deleted
/// through its owning service stops being discovered with no propagation step.
#[tokio::test]
async fn a_resource_deleted_in_its_own_service_stops_being_discovered() {
    let (router, _engine) = emulator();
    associate_admin(&router).await;

    let vpc = element(
        "vpc",
        &ec2_ok("vpc", &router, "CreateVpc", &[p("CidrBlock", "10.0.0.0/16")]).await,
        "vpcId",
    );
    let uri = ec2_arn(&format!("vpc/{vpc}"));
    assert!(
        every_uri(&router, ACCOUNT, state::RESOURCE_TYPE_EC2_VPC)
            .await
            .contains(&uri),
        "the created VPC must be discoverable, got nothing for {uri}"
    );

    ec2_ok("delete vpc", &router, "DeleteVpc", &[p("VpcId", &vpc)]).await;
    let after = every_uri(&router, ACCOUNT, state::RESOURCE_TYPE_EC2_VPC).await;
    assert!(
        !after.contains(&uri),
        "a deleted VPC must stop being discovered, got {after:?}"
    );
}

/// RULE: what is discoverable is "available to be associated with a resource
/// set", so `BatchAssociateResource` accepts a discovered resource's URI — the
/// two read the same inventory.
#[tokio::test]
async fn a_discovered_resource_is_associable() {
    let (router, engine) = emulator();
    associate_admin(&router).await;

    let policy = network_firewall(
        &router,
        "CreateFirewallPolicy",
        &json!({
            "FirewallPolicyName": "fms-associable-policy",
            "FirewallPolicy": {
                "StatelessDefaultActions": ["aws:forward_to_sfe"],
                "StatelessFragmentDefaultActions": ["aws:forward_to_sfe"],
            },
        }),
    )
    .await;
    let policy_arn = policy["FirewallPolicyResponse"]["FirewallPolicyArn"]
        .as_str()
        .expect("a minted FirewallPolicyArn")
        .to_string();
    network_firewall(
        &router,
        "CreateFirewall",
        &json!({
            "FirewallName": "fms-associable-firewall",
            "FirewallPolicyArn": policy_arn,
            "VpcId": "vpc-0123456789abcdef0",
            "SubnetMappings": [{ "SubnetId": "subnet-0123456789abcdef0" }],
        }),
    )
    .await;

    let discovered = every_uri(
        &router,
        ACCOUNT,
        state::RESOURCE_TYPE_NETWORK_FIREWALL_FIREWALL,
    )
    .await;
    assert_eq!(
        discovered.len(),
        1,
        "the firewall just created must be the discovered inventory, got {discovered:?}"
    );

    let (_, created) = call(
        &router,
        "PutResourceSet",
        &json!({
            "ResourceSet": {
                "Name": "discovered-resources",
                "ResourceTypeList": [state::RESOURCE_TYPE_NETWORK_FIREWALL_FIREWALL],
            },
        }),
    )
    .await;
    let id = created["ResourceSet"]["Id"]
        .as_str()
        .expect("a minted Id")
        .to_string();

    let (status, body) = call(
        &router,
        "BatchAssociateResource",
        &json!({ "ResourceSetIdentifier": id, "Items": discovered }),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected BatchAssociateResource to answer 200, got {status} with {body}"
    );
    assert_eq!(
        body["FailedItems"],
        json!([]),
        "a discovered resource must be associable, got {body}"
    );

    let (status, listed) = call(
        &router,
        "ListResourceSetResources",
        &json!({ "Identifier": id }),
    )
    .await;
    // The membership is read through the store below, because the modelled
    // reader is still a scaffold stub tracked by its own issue. Whether it
    // answers 501 today or the resources once it is implemented is that issue's
    // business, not this suite's: either is accepted, so implementing it cannot
    // fail a discovery test.
    assert!(
        status == 501 || status == 200,
        "ListResourceSetResources must either answer its membership or refuse as the \
         unimplemented operation it is, got {status} with {listed}"
    );
    let held = with_store(&engine, |store| {
        store.resource_sets[&id]
            .resources
            .iter()
            .cloned()
            .collect::<Vec<String>>()
    });
    assert_eq!(
        held, discovered,
        "the resource set must hold the discovered resource, got {held:?}"
    );
}
