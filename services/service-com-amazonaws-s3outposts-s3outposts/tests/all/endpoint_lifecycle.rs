// SPDX-License-Identifier: CC0-1.0

//! The S3 on Outposts endpoint lifecycle, end to end over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rule under test is the one this control plane states for its two
//! resources: an endpoint's members are whatever `CreateEndpoint` stored for it, the
//! Outposts are the ones the store holds, and every operation computes its answer from
//! those records.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_s3outposts_s3outposts::state::{
    EndpointRecord, OutpostRecord, ACCESS_TYPE_PRIVATE, ENDPOINT_STATUS_AVAILABLE,
    ENDPOINT_TRANSITION, PAGE_LIMIT,
};
use service_com_amazonaws_s3outposts_s3outposts::ComAmazonawsS3outpostsS3outpostsState;
use service_com_amazonaws_s3outposts_s3outposts::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region most cases sign for; the ARN cases vary it.
const REGION: &str = "us-east-1";

/// One tebibyte (1024^4), the unit the capacities below are stated in
/// (`Outpost$CapacityInBytes` is "The Amazon S3 capacity of the outpost in bytes").
const TEBIBYTE: i64 = 1024 * 1024 * 1024 * 1024;

/// An account that is NOT the one this emulator serves: the owner of the Outpost these
/// cases have shared with the caller through Resource Access Manager. A synthetic,
/// non-routable placeholder in the same all-zero family as `engine::emulator_account_id`;
/// no real account id appears anywhere.
const SHARING_ACCOUNT_ID: &str = "000000000001";

/// The Outpost [`SHARING_ACCOUNT_ID`] has shared with the caller.
pub(crate) const SHARED_OUTPOST_ID: &str = "op-2c3d4e5f607182930";

/// One emulator over a store NOBODY has provisioned anything in — what a new Amazon Web
/// Services account is: no Outposts, no endpoints.
///
/// An Outpost is hardware Amazon installs, and this API models no operation that
/// provisions one, so this is the store every engine starts with.
pub(crate) fn unprovisioned_emulator_with_engine(
) -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine.clone()));
    (router, engine)
}

/// One emulator over a store NOBODY has provisioned anything in.
pub(crate) fn unprovisioned_emulator() -> MultiServiceRouter {
    unprovisioned_emulator_with_engine().0
}

/// One emulator for an account that DOES have S3 on Outposts capacity: two Outposts of
/// its own and one another account shares with it, plus that account's own endpoint on
/// the Outpost it shared — the endpoint `ListSharedEndpoints` reports and `ListEndpoints`
/// does not.
///
/// None of those three facts is something an operation of this API creates, so the
/// capacity is stood up through the store's own provisioning seam — the same seam
/// `S3_OUTPOSTS_PROVISIONED_OUTPOSTS` reaches in a running emulator — and every answer
/// the cases below assert on is still computed by a handler from those records.
///
/// Returned beside the router for the cases that have to look at the store itself.
fn emulator_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let (router, engine) = unprovisioned_emulator_with_engine();
    let state = engine.state::<ComAmazonawsS3outpostsS3outpostsState>();
    let mut store = state.lock();

    let owner = engine::emulator_account_id().to_string();
    for (outpost_id, owner_id, capacity) in [
        ("op-0a1b2c3d4e5f60718", owner.clone(), 48 * TEBIBYTE),
        ("op-1b2c3d4e5f6071829", owner, 96 * TEBIBYTE),
        (
            SHARED_OUTPOST_ID,
            SHARING_ACCOUNT_ID.to_string(),
            24 * TEBIBYTE,
        ),
    ] {
        store.provision_outpost(
            OutpostRecord::provisioned(outpost_id, &owner_id, capacity).unwrap_or_else(|| {
                panic!("{outpost_id} owned by {owner_id} must be a provisionable Outpost")
            }),
        );
    }

    // The sharing account's own endpoint on the Outpost it shared, minted through the
    // same counters a created endpoint uses, so no `CreateEndpoint` can mint a colliding
    // id, CIDR or interface. It was provisioned before this engine existed, so the clock
    // owes it nothing.
    let endpoint_id = store.mint_endpoint_id();
    let subnet_id = "subnet-0a1b2c3d4e5f60718".to_string();
    let vpc_id = store.vpc_for_subnet(&subnet_id);
    let network_interface_id = store.mint_network_interface_id();
    let cidr_block = store
        .allocate_cidr_block()
        .expect("the first /26 of the reserved block is free in an empty store");
    store.endpoints.insert(
        (SHARED_OUTPOST_ID.to_string(), endpoint_id.clone()),
        EndpointRecord {
            endpoint_id,
            outpost_id: SHARED_OUTPOST_ID.to_string(),
            cidr_block,
            status: ENDPOINT_STATUS_AVAILABLE.to_string(),
            pending: None,
            creation_time: SystemTime::now(),
            network_interfaces: vec![network_interface_id],
            vpc_id,
            subnet_id,
            security_group_id: "sg-0a1b2c3d4e5f60718".to_string(),
            access_type: ACCESS_TYPE_PRIVATE.to_string(),
            customer_owned_ipv4_pool: None,
            owner_account: SHARING_ACCOUNT_ID.to_string(),
        },
    );
    drop(store);

    (router, engine)
}

/// One emulator for an account that has S3 on Outposts capacity (see
/// [`emulator_with_engine`]).
pub(crate) fn emulator() -> MultiServiceRouter {
    emulator_with_engine().0
}

/// What the emulator answered: the HTTP status and the JSON body.
pub(crate) struct Answer {
    pub(crate) status: u16,
    pub(crate) body: Value,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    pub(crate) fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    /// The `Message` an error payload carries, or the empty string when it
    /// carries none.
    ///
    /// A modelled error renders its own member name (`Message`), while the
    /// boundary validator renders the protocol's generic `message`; a client
    /// reads whichever is present, so this does too.
    pub(crate) fn message(&self) -> &str {
        self.body["Message"]
            .as_str()
            .or_else(|| self.body["message"].as_str())
            .unwrap_or_default()
    }

    /// The `Endpoints` list an endpoint listing carries.
    pub(crate) fn endpoints(&self) -> &[Value] {
        self.body["Endpoints"]
            .as_array()
            .unwrap_or_else(|| panic!("an endpoint listing must carry Endpoints: {:?}", self.body))
            .as_slice()
    }

    /// The `Outposts` list `ListOutpostsWithS3` carries.
    pub(crate) fn outposts(&self) -> &[Value] {
        self.body["Outposts"]
            .as_array()
            .unwrap_or_else(|| panic!("ListOutpostsWithS3 must carry Outposts: {:?}", self.body))
            .as_slice()
    }

    /// The `NextToken` the answer carries, if it carries one.
    pub(crate) fn next_token(&self) -> Option<String> {
        self.body["NextToken"].as_str().map(str::to_string)
    }
}

/// A SigV4 credential scope naming this service, signed for `region`. It is what routes a
/// request to the s3-outposts service, and its Region is what every ARN in the answer
/// names.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/s3-outposts/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

pub(crate) async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("s3-outposts.{region}.amazonaws.com"))
        .header("authorization", authorization(region));
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

async fn create_in(router: &MultiServiceRouter, region: &str, request: Value) -> Answer {
    call_in(
        router,
        region,
        "POST",
        "/S3Outposts/CreateEndpoint",
        Some(request),
    )
    .await
}

pub(crate) async fn create(router: &MultiServiceRouter, request: Value) -> Answer {
    create_in(router, REGION, request).await
}

async fn list_endpoints_in(router: &MultiServiceRouter, region: &str, query: &str) -> Answer {
    call_in(
        router,
        region,
        "GET",
        &format!("/S3Outposts/ListEndpoints{query}"),
        None,
    )
    .await
}

pub(crate) async fn list_endpoints(router: &MultiServiceRouter, query: &str) -> Answer {
    list_endpoints_in(router, REGION, query).await
}

pub(crate) async fn list_outposts_in(
    router: &MultiServiceRouter,
    region: &str,
    query: &str,
) -> Answer {
    call_in(
        router,
        region,
        "GET",
        &format!("/S3Outposts/ListOutpostsWithS3{query}"),
        None,
    )
    .await
}

pub(crate) async fn list_outposts(router: &MultiServiceRouter, query: &str) -> Answer {
    list_outposts_in(router, REGION, query).await
}

pub(crate) async fn list_shared(router: &MultiServiceRouter, query: &str) -> Answer {
    call_in(
        router,
        REGION,
        "GET",
        &format!("/S3Outposts/ListSharedEndpoints{query}"),
        None,
    )
    .await
}

pub(crate) async fn delete_endpoint(
    router: &MultiServiceRouter,
    outpost_id: &str,
    endpoint_id: &str,
) -> Answer {
    call_in(
        router,
        REGION,
        "DELETE",
        &format!("/S3Outposts/DeleteEndpoint?endpointId={endpoint_id}&outpostId={outpost_id}"),
        None,
    )
    .await
}

/// The `(OutpostId, OwnerId)` pairs the emulator presents, in the order
/// `ListOutpostsWithS3` reports them. Nothing in these cases hardcodes a provisioned id:
/// the Outposts are discovered through the API, the way a client discovers them.
async fn all_outposts(router: &MultiServiceRouter) -> Vec<(String, String)> {
    let answer = list_outposts(router, "?maxResults=100").await;
    assert_eq!(
        answer.status, 200,
        "ListOutpostsWithS3 must answer 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer
        .outposts()
        .iter()
        .map(|outpost| {
            (
                string_at(outpost, "OutpostId"),
                string_at(outpost, "OwnerId"),
            )
        })
        .collect()
}

/// The Outposts this caller owns.
pub(crate) async fn owned_outposts(router: &MultiServiceRouter) -> Vec<String> {
    all_outposts(router)
        .await
        .into_iter()
        .filter(|(_, owner)| owner == engine::emulator_account_id())
        .map(|(id, _)| id)
        .collect()
}

/// An Outpost another account shared with this caller.
pub(crate) async fn shared_outpost(router: &MultiServiceRouter) -> String {
    all_outposts(router)
        .await
        .into_iter()
        .find(|(_, owner)| owner != engine::emulator_account_id())
        .map(|(id, _)| id)
        .expect("the emulator presents an Outpost shared by another account")
}

/// The string member `name` of `value`, or a panic naming what was there instead.
pub(crate) fn string_at(value: &Value, name: &str) -> String {
    value[name]
        .as_str()
        .unwrap_or_else(|| panic!("member {name} must be a string in {value:?}"))
        .to_string()
}

/// The endpoint in `answer` whose `EndpointArn` is `arn`.
fn endpoint_with_arn(answer: &Answer, arn: &str) -> Value {
    answer
        .endpoints()
        .iter()
        .find(|endpoint| endpoint["EndpointArn"].as_str() == Some(arn))
        .unwrap_or_else(|| panic!("no endpoint with ARN {arn} in {:?}", answer.body))
        .clone()
}

/// `EndpointId`'s `^[a-zA-Z0-9]{19}$`, checked here rather than through the crate's own
/// predicate so a wrong predicate cannot make this test pass.
fn matches_endpoint_id(value: &str) -> bool {
    value.len() == 19 && value.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// The endpoint id an `EndpointArn` ends with, per `EndpointArn`'s pattern
/// `^arn:(aws|…):s3-outposts:[a-z\-0-9]*:[0-9]{12}:outpost/(op-[a-f0-9]{17}|ec2)/endpoint/[a-zA-Z0-9]{19}$`.
pub(crate) fn endpoint_id_of(arn: &str) -> String {
    let (prefix, endpoint_id) = arn
        .rsplit_once("/endpoint/")
        .unwrap_or_else(|| panic!("{arn} is not an EndpointArn"));
    assert!(
        matches_endpoint_id(endpoint_id),
        "the id in {arn} must be 19 alphanumeric characters, got {endpoint_id:?}"
    );
    assert!(
        prefix.contains(":s3-outposts:"),
        "{arn} must be an s3-outposts ARN"
    );
    endpoint_id.to_string()
}

/// An `OutpostId` the model's `^(op-[a-f0-9]{17}|\d{12}|ec2)$` admits but the store does
/// not hold: 17 hex digits, as the pattern requires.
const UNKNOWN_OUTPOST_ID: &str = "op-fffffffffffffffff";

/// A create request with only the three required members.
pub(crate) fn minimal_create(outpost_id: &str) -> Value {
    json!({
        "OutpostId": outpost_id,
        "SubnetId": "subnet-00000000000000001",
        "SecurityGroupId": "sg-00000000000000001",
    })
}

/// `CreateEndpoint` persists what it was given and the endpoint listings report that
/// record: for every input member the `Endpoint` structure carries, the listed value is
/// the created one, and the members S3 on Outposts provisions itself are filled in.
///
/// The rows differ along what the rule generalises over — which Outpost, both id widths
/// `SubnetId` / `SecurityGroupId` admit, each `EndpointAccessType` value, whether the
/// access type is named at all, and whether a CoIP pool is carried.
#[tokio::test]
async fn a_created_endpoint_is_listed_from_what_was_stored() {
    let router = emulator();
    let outposts = owned_outposts(&router).await;
    assert!(
        outposts.len() >= 2,
        "the emulator must present at least two owned Outposts, got {outposts:?}"
    );

    let cases = vec![
        (
            "only the required members, 17-hex ids",
            json!({
                "OutpostId": outposts[0],
                "SubnetId": "subnet-0a1b2c3d4e5f60711",
                "SecurityGroupId": "sg-0a1b2c3d4e5f60711",
            }),
            "Private",
            Value::Null,
        ),
        (
            "the short id width the patterns admit",
            json!({
                "OutpostId": outposts[0],
                "SubnetId": "subnet-0a1b2c3d",
                "SecurityGroupId": "sg-0a1b2c3d",
            }),
            "Private",
            Value::Null,
        ),
        (
            "Private named explicitly, on the second Outpost",
            json!({
                "OutpostId": outposts[1],
                "SubnetId": "subnet-0a1b2c3d4e5f60712",
                "SecurityGroupId": "sg-0a1b2c3d4e5f60712",
                "AccessType": "Private",
            }),
            "Private",
            Value::Null,
        ),
        (
            "CustomerOwnedIp with its CoIP pool",
            json!({
                "OutpostId": outposts[1],
                "SubnetId": "subnet-0a1b2c3d4e5f60713",
                "SecurityGroupId": "sg-0a1b2c3d4e5f60713",
                "AccessType": "CustomerOwnedIp",
                "CustomerOwnedIpv4Pool": "ipv4pool-coip-0a1b2c3d4e5f60713",
            }),
            "CustomerOwnedIp",
            json!("ipv4pool-coip-0a1b2c3d4e5f60713"),
        ),
    ];

    for (case, request, expected_access_type, expected_pool) in cases {
        let created = create(&router, request.clone()).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreateEndpoint must answer 200, got {} with {:?}",
            created.status, created.body
        );
        let arn = string_at(&created.body, "EndpointArn");
        let endpoint_id = endpoint_id_of(&arn);

        let listed = list_endpoints(&router, "?maxResults=100").await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: ListEndpoints must answer 200, got {} with {:?}",
            listed.status, listed.body
        );
        let endpoint = endpoint_with_arn(&listed, &arn);

        assert_eq!(
            endpoint["OutpostsId"], request["OutpostId"],
            "case {case:?}: OutpostsId must be the Outpost the endpoint was created on, got {:?}",
            endpoint["OutpostsId"]
        );
        assert_eq!(
            endpoint["SubnetId"], request["SubnetId"],
            "case {case:?}: SubnetId must be the stored one, got {:?}",
            endpoint["SubnetId"]
        );
        assert_eq!(
            endpoint["SecurityGroupId"], request["SecurityGroupId"],
            "case {case:?}: SecurityGroupId must be the stored one, got {:?}",
            endpoint["SecurityGroupId"]
        );
        assert_eq!(
            endpoint["AccessType"],
            json!(expected_access_type),
            "case {case:?}: AccessType must be the stored one, got {:?}",
            endpoint["AccessType"]
        );
        assert_eq!(
            endpoint["CustomerOwnedIpv4Pool"], expected_pool,
            "case {case:?}: CustomerOwnedIpv4Pool must be the stored one, got {:?}",
            endpoint["CustomerOwnedIpv4Pool"]
        );
        assert_eq!(
            endpoint["EndpointArn"],
            json!(arn),
            "case {case:?}: the listed ARN must be the created one, got {:?}",
            endpoint["EndpointArn"]
        );

        // The members S3 on Outposts provisions rather than the caller: each is reported,
        // and the endpoint id the ARN carries is the one DeleteEndpoint names.
        assert!(
            endpoint["CreationTime"].is_number(),
            "case {case:?}: CreationTime must be the stored creation instant, got {:?}",
            endpoint["CreationTime"]
        );
        assert!(
            string_at(&endpoint, "CidrBlock").ends_with("/26"),
            "case {case:?}: CidrBlock must be the committed VPC range, got {:?}",
            endpoint["CidrBlock"]
        );
        assert!(
            string_at(&endpoint, "VpcId").starts_with("vpc-"),
            "case {case:?}: VpcId must name the subnet's VPC, got {:?}",
            endpoint["VpcId"]
        );
        let interfaces = endpoint["NetworkInterfaces"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: NetworkInterfaces must be a list"))
            .clone();
        assert!(
            !interfaces.is_empty()
                && interfaces
                    .iter()
                    .all(|nic| string_at(nic, "NetworkInterfaceId").starts_with("eni-")),
            "case {case:?}: every network interface must carry an id, got {interfaces:?}"
        );
        assert!(
            matches_endpoint_id(&endpoint_id),
            "case {case:?}: the minted id must satisfy EndpointId, got {endpoint_id:?}"
        );
        // `FailedReason` belongs to a failed create or delete; this one succeeded.
        assert_eq!(
            endpoint["FailedReason"],
            Value::Null,
            "case {case:?}: a stored endpoint carries no failure reason, got {:?}",
            endpoint["FailedReason"]
        );
    }
}

/// Every ARN in an answer names the Region the request was signed for, the account that
/// owns the resource, and the resource's own ids — so an endpoint's ARN and its parent
/// Outpost's ARN always agree on the Region.
///
/// The rows are Regions in different partitions' naming shapes; each gets its own
/// emulator so the created endpoint is the only one in the answer.
#[tokio::test]
async fn every_arn_names_the_region_the_request_was_signed_for() {
    let account = engine::emulator_account_id();
    for region in ["us-east-1", "eu-west-1", "ap-southeast-2", "me-central-1"] {
        let router = emulator();
        let outpost_id = owned_outposts(&router)
            .await
            .first()
            .expect("the emulator presents an owned Outpost")
            .clone();

        let created = create_in(&router, region, minimal_create(&outpost_id)).await;
        assert_eq!(
            created.status, 200,
            "case {region:?}: CreateEndpoint must answer 200, got {} with {:?}",
            created.status, created.body
        );
        let arn = string_at(&created.body, "EndpointArn");
        let endpoint_id = endpoint_id_of(&arn);
        assert_eq!(
            arn,
            format!("arn:aws:s3-outposts:{region}:{account}:outpost/{outpost_id}/endpoint/{endpoint_id}"),
            "case {region:?}: the EndpointArn must be built from the request's Region, the \
             owning account and the stored ids, got {arn:?}"
        );

        let listed = list_endpoints_in(&router, region, "?maxResults=100").await;
        assert_eq!(
            string_at(&endpoint_with_arn(&listed, &arn), "EndpointArn"),
            arn,
            "case {region:?}: the listed ARN must be the created one"
        );

        let outposts = list_outposts_in(&router, region, "?maxResults=100").await;
        let outpost = outposts
            .outposts()
            .iter()
            .find(|outpost| outpost["OutpostId"].as_str() == Some(outpost_id.as_str()))
            .unwrap_or_else(|| panic!("case {region:?}: the Outpost must still be listed"))
            .clone();
        assert_eq!(
            string_at(&outpost, "OutpostArn"),
            format!("arn:aws:outposts:{region}:{account}:outpost/{outpost_id}"),
            "case {region:?}: the OutpostArn must name the same Region"
        );
        assert_eq!(
            string_at(&outpost, "S3OutpostArn"),
            format!("arn:aws:s3-outposts:{region}:{account}:outpost/{outpost_id}/s3"),
            "case {region:?}: the S3OutpostArn must name the same Region"
        );
        assert!(
            outpost["CapacityInBytes"].as_i64().unwrap_or_default() > 0,
            "case {region:?}: the Outpost must report its S3 capacity, got {:?}",
            outpost["CapacityInBytes"]
        );
    }
}

/// `ListOutpostsWithS3` is `@paginated(items: Outposts)`: whatever `MaxResults` it is
/// given, following `NextToken` to the end enumerates every Outpost exactly once in one
/// stable order, no page is longer than was asked for, and the last page carries no token.
///
/// The rows are page sizes: each bound `MaxResults` publishes (`@range(min: 0, max: 100)`,
/// where 0 is "no preference"), the sizes around the number of Outposts, and no
/// `MaxResults` member at all.
#[tokio::test]
async fn list_outposts_with_s3_pages_over_one_stable_order() {
    let router = emulator();
    let expected: Vec<String> = all_outposts(&router)
        .await
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    assert!(
        expected.len() >= 2,
        "pagination needs at least two Outposts, got {expected:?}"
    );

    let cases: Vec<(&str, String)> = vec![
        ("one Outpost per page", "?maxResults=1".to_string()),
        ("two per page", "?maxResults=2".to_string()),
        (
            "a page as long as the store",
            format!("?maxResults={}", expected.len()),
        ),
        (
            "the largest page the model admits",
            format!("?maxResults={PAGE_LIMIT}"),
        ),
        ("no preference", "?maxResults=0".to_string()),
        ("no MaxResults member at all", String::new()),
    ];

    for (case, query) in cases {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let uri = match (&token, query.is_empty()) {
                (None, _) => query.clone(),
                (Some(token), true) => format!("?nextToken={token}"),
                (Some(token), false) => format!("{query}&nextToken={token}"),
            };
            let answer = list_outposts(&router, &uri).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: page {pages} must answer 200, got {} with {:?}",
                answer.status, answer.body
            );
            let page: Vec<String> = answer
                .outposts()
                .iter()
                .map(|outpost| string_at(outpost, "OutpostId"))
                .collect();
            assert!(
                !page.is_empty(),
                "case {case:?}: page {pages} must not be empty while a token is served"
            );
            seen.extend(page);
            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "case {case:?}: pagination must terminate, saw {seen:?}"
            );
            token = answer.next_token();
            if token.is_none() {
                break;
            }
        }

        assert_eq!(
            seen, expected,
            "case {case:?}: paging must enumerate every Outpost exactly once in one order"
        );
    }

    // A page shorter than the store is served with a token; the model's own ceiling is
    // enough for the whole store, so that page carries none.
    let first = list_outposts(&router, "?maxResults=1").await;
    assert_eq!(
        first.outposts().len(),
        1,
        "a page must not be longer than MaxResults, got {:?}",
        first.body
    );
    assert!(
        first.next_token().is_some(),
        "a page that does not reach the end must carry a NextToken, got {:?}",
        first.body
    );
    assert!(
        list_outposts(&router, &format!("?maxResults={PAGE_LIMIT}"))
            .await
            .next_token()
            .is_none(),
        "the last page must carry no NextToken"
    );
}

/// `ListEndpoints` is `@paginated(items: Endpoints)`: following `NextToken` enumerates
/// every endpoint of this account exactly once in one stable order, and a token this
/// service never issued is refused rather than served as an arbitrary page.
#[tokio::test]
async fn list_endpoints_pages_over_one_stable_order() {
    let router = emulator();
    let outposts = owned_outposts(&router).await;
    let mut created = Vec::new();
    for index in 0..7u32 {
        let outpost_id = &outposts[index as usize % outposts.len()];
        let request = json!({
            "OutpostId": outpost_id,
            "SubnetId": format!("subnet-0000000000000{index:04x}"),
            "SecurityGroupId": format!("sg-0000000000000{index:04x}"),
        });
        let answer = create(&router, request).await;
        assert_eq!(
            answer.status, 200,
            "create {index} must answer 200, got {} with {:?}",
            answer.status, answer.body
        );
        created.push(string_at(&answer.body, "EndpointArn"));
    }

    let whole: Vec<String> = list_endpoints(&router, "?maxResults=100")
        .await
        .endpoints()
        .iter()
        .map(|endpoint| string_at(endpoint, "EndpointArn"))
        .collect();
    assert_eq!(
        whole.len(),
        created.len(),
        "ListEndpoints must report exactly the endpoints this account created, got {whole:?}"
    );
    for arn in &created {
        assert!(
            whole.contains(arn),
            "ListEndpoints must report {arn}, got {whole:?}"
        );
    }

    for page_size in [1usize, 2, 3, 7, 100] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let uri = match &token {
                None => format!("?maxResults={page_size}"),
                Some(token) => format!("?maxResults={page_size}&nextToken={token}"),
            };
            let answer = list_endpoints(&router, &uri).await;
            assert_eq!(
                answer.status, 200,
                "case {page_size}: a page must answer 200, got {} with {:?}",
                answer.status, answer.body
            );
            assert!(
                answer.endpoints().len() <= page_size,
                "case {page_size}: a page must not be longer than MaxResults, got {:?}",
                answer.body
            );
            seen.extend(
                answer
                    .endpoints()
                    .iter()
                    .map(|endpoint| string_at(endpoint, "EndpointArn")),
            );
            token = answer.next_token();
            if token.is_none() {
                break;
            }
            assert!(
                seen.len() <= whole.len(),
                "case {page_size}: pagination must terminate, saw {seen:?}"
            );
        }
        assert_eq!(
            seen, whole,
            "case {page_size}: paging must enumerate every endpoint exactly once in one order"
        );
    }

    for bad in ["not-a-cursor", "op-0a1b2c3d4e5f60718", "aaaa/bbbb"] {
        let refused = list_endpoints(&router, &format!("?maxResults=1&nextToken={bad}")).await;
        assert_eq!(
            (refused.status, refused.error_code()),
            (400, "ValidationException"),
            "case {bad:?}: a token this service never issued must be refused, got {} with {:?}",
            refused.status,
            refused.body
        );
    }
}

/// A new endpoint is not usable the instant the create returns: it reports `Pending` to a
/// caller listing straight away, and the clock owes it `Available` without any further
/// call.
#[tokio::test]
async fn a_new_endpoint_reports_pending_until_the_clock_owes_it_available() {
    let router = emulator();
    let outpost_id = owned_outposts(&router).await[0].clone();

    let started = Instant::now();
    let created = create(&router, minimal_create(&outpost_id)).await;
    let arn = string_at(&created.body, "EndpointArn");

    let immediately = list_endpoints(&router, "?maxResults=100").await;
    if started.elapsed() < ENDPOINT_TRANSITION {
        assert_eq!(
            endpoint_with_arn(&immediately, &arn)["Status"],
            json!("Pending"),
            "a caller acting inside the transition span still sees Pending, got {:?}",
            endpoint_with_arn(&immediately, &arn)["Status"]
        );
    }

    // The transition is owed by the clock, not by a call: nothing is done here but wait.
    std::thread::sleep(ENDPOINT_TRANSITION + Duration::from_millis(200));

    let settled = list_endpoints(&router, "?maxResults=100").await;
    assert_eq!(
        endpoint_with_arn(&settled, &arn)["Status"],
        json!("Available"),
        "once the transition span has passed the endpoint is Available, got {:?}",
        endpoint_with_arn(&settled, &arn)["Status"]
    );
    assert_eq!(
        endpoint_with_arn(&settled, &arn)["CreationTime"],
        endpoint_with_arn(&immediately, &arn)["CreationTime"],
        "the creation instant does not move when the status settles"
    );
}

/// `DeleteEndpoint` removes the record: the endpoint is gone from `ListEndpoints`, the
/// endpoints beside it are untouched, and a pair naming no endpoint of this account — a
/// deleted one included — is the modelled 404.
#[tokio::test]
async fn deleting_an_endpoint_removes_it_and_leaves_the_others() {
    let router = emulator();
    let outposts = owned_outposts(&router).await;
    for index in 0..3u32 {
        let request = json!({
            "OutpostId": outposts[index as usize % outposts.len()],
            "SubnetId": format!("subnet-0000000000000a{index:03x}"),
            "SecurityGroupId": format!("sg-0000000000000a{index:03x}"),
        });
        assert_eq!(
            create(&router, request).await.status,
            200,
            "endpoint {index} must be created"
        );
    }

    // The order the listing serves, which is the order the store holds — not the order the
    // creates happened in.
    let ordered: Vec<String> = list_endpoints(&router, "?maxResults=100")
        .await
        .endpoints()
        .iter()
        .map(|endpoint| string_at(endpoint, "EndpointArn"))
        .collect();

    for (index, arn) in ordered.clone().into_iter().enumerate() {
        let endpoint = endpoint_with_arn(&list_endpoints(&router, "?maxResults=100").await, &arn);
        let outpost_id = string_at(&endpoint, "OutpostsId");
        let endpoint_id = endpoint_id_of(&arn);

        let deleted = delete_endpoint(&router, &outpost_id, &endpoint_id).await;
        assert_eq!(
            deleted.status, 200,
            "case {index}: DeleteEndpoint must answer 200, got {} with {:?}",
            deleted.status, deleted.body
        );

        let listed = list_endpoints(&router, "?maxResults=100").await;
        let remaining: Vec<String> = listed
            .endpoints()
            .iter()
            .map(|endpoint| string_at(endpoint, "EndpointArn"))
            .collect();
        assert_eq!(
            remaining,
            ordered[index + 1..].to_vec(),
            "case {index}: ListEndpoints must report exactly the endpoints still stored"
        );

        let again = delete_endpoint(&router, &outpost_id, &endpoint_id).await;
        assert_eq!(
            (again.status, again.error_code()),
            (404, "ResourceNotFoundException"),
            "case {index}: a repeated delete must answer 404, got {} with {:?}",
            again.status,
            again.body
        );
    }
}

/// A pair that names no endpoint of this account is a 404, whichever half of it is wrong —
/// including an endpoint that exists on another Outpost, and one another account owns.
#[tokio::test]
async fn an_endpoint_no_caller_of_this_account_owns_is_not_found() {
    let router = emulator();
    let outposts = owned_outposts(&router).await;
    let shared = shared_outpost(&router).await;
    let arn = string_at(
        &create(&router, minimal_create(&outposts[0])).await.body,
        "EndpointArn",
    );
    let endpoint_id = endpoint_id_of(&arn);

    // The endpoint the sharing account owns on the shared Outpost: visible through
    // ListSharedEndpoints, and not this account's to delete.
    let shared_endpoint_id = endpoint_id_of(&string_at(
        &list_shared(&router, &format!("?outpostId={shared}&maxResults=100"))
            .await
            .endpoints()[0],
        "EndpointArn",
    ));

    let cases = [
        ("the endpoint exists, but on another Outpost", outposts[1].clone(), endpoint_id.clone()),
        ("the Outpost exists, the endpoint does not", outposts[0].clone(), "0123456789abcdefghi".to_string()),
        ("neither exists", UNKNOWN_OUTPOST_ID.to_string(), "0123456789abcdefghi".to_string()),
        ("an Outpost id of the 12-digit form", "000000000000".to_string(), endpoint_id.clone()),
        ("an endpoint another account owns", shared.clone(), shared_endpoint_id),
    ];

    for (case, outpost_id, candidate) in cases {
        let answer = delete_endpoint(&router, &outpost_id, &candidate).await;
        assert_eq!(
            (answer.status, answer.error_code()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: must answer 404 ResourceNotFoundException, got {} with {:?}",
            answer.status,
            answer.body
        );
    }

    // None of those refusals touched the stored endpoint.
    let listed = list_endpoints(&router, "?maxResults=100").await;
    assert_eq!(
        listed.endpoints().len(),
        1,
        "the stored endpoint must still be listed, got {:?}",
        listed.body
    );
}

/// `CreateEndpoint` hangs the endpoint off an existing Outpost: an `OutpostId` the model's
/// pattern admits but the store does not hold is the modelled 404, and nothing is stored
/// for it.
#[tokio::test]
async fn create_endpoint_refuses_an_outpost_the_store_does_not_hold() {
    let router = emulator();
    // Every form `OutpostId`'s `^(op-[a-f0-9]{17}|\d{12}|ec2)$` admits, none of them in
    // the store.
    let cases = [
        ("an op- id nothing has", UNKNOWN_OUTPOST_ID),
        ("another op- id nothing has", "op-00000000000000000"),
        ("the 12-digit form", "123456789012"),
        ("the ec2 form", "ec2"),
    ];

    for (case, outpost_id) in cases {
        let answer = create(&router, minimal_create(outpost_id)).await;
        assert_eq!(
            (answer.status, answer.error_code()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: must answer 404 ResourceNotFoundException, got {} with {:?}",
            answer.status,
            answer.body
        );
    }

    assert!(
        list_endpoints(&router, "?maxResults=100")
            .await
            .endpoints()
            .is_empty(),
        "a refused create must store nothing"
    );
}

/// One endpoint per (Outpost, subnet, security group): a create that repeats all three is
/// the modelled conflict, and a create that differs in any one of them is accepted.
#[tokio::test]
async fn a_repeated_subnet_and_security_group_on_one_outpost_conflicts() {
    let router = emulator();
    let outposts = owned_outposts(&router).await;
    let baseline = json!({
        "OutpostId": outposts[0],
        "SubnetId": "subnet-0b1c2d3e4f5061728",
        "SecurityGroupId": "sg-0b1c2d3e4f5061728",
    });
    assert_eq!(
        create(&router, baseline.clone()).await.status,
        200,
        "the baseline endpoint must be created"
    );

    let cases = vec![
        ("the same three members again", baseline.clone(), 409),
        (
            "another subnet, the same security group",
            json!({
                "OutpostId": outposts[0],
                "SubnetId": "subnet-0b1c2d3e4f5061729",
                "SecurityGroupId": "sg-0b1c2d3e4f5061728",
            }),
            200,
        ),
        (
            "the same subnet, another security group",
            json!({
                "OutpostId": outposts[0],
                "SubnetId": "subnet-0b1c2d3e4f5061728",
                "SecurityGroupId": "sg-0b1c2d3e4f5061729",
            }),
            200,
        ),
        (
            "the same subnet and security group, another Outpost",
            json!({
                "OutpostId": outposts[1],
                "SubnetId": "subnet-0b1c2d3e4f5061728",
                "SecurityGroupId": "sg-0b1c2d3e4f5061728",
            }),
            200,
        ),
    ];

    let mut stored = 1;
    for (case, request, expected_status) in cases {
        let answer = create(&router, request).await;
        assert_eq!(
            answer.status, expected_status,
            "case {case:?}: expected {expected_status}, got {} with {:?}",
            answer.status, answer.body
        );
        if expected_status == 200 {
            stored += 1;
        } else {
            assert_eq!(
                answer.error_code(),
                "ConflictException",
                "case {case:?}: a duplicate must be the modelled conflict, got {:?}",
                answer.body
            );
        }
        assert_eq!(
            list_endpoints(&router, "?maxResults=100").await.endpoints().len(),
            stored,
            "case {case:?}: a refused create stores nothing and an accepted one stores exactly one"
        );
    }
}

/// `AccessType: CustomerOwnedIp` documents that the CoIP pool must be named with it; a
/// create that leaves it out is refused rather than stored.
#[tokio::test]
async fn customer_owned_ip_without_a_coip_pool_is_refused() {
    let router = emulator();
    let outpost_id = owned_outposts(&router).await[0].clone();

    let cases = vec![
        ("CustomerOwnedIp with no pool", Some("CustomerOwnedIp"), None, 400),
        (
            "CustomerOwnedIp with its pool",
            Some("CustomerOwnedIp"),
            Some("ipv4pool-coip-0c1d2e3f405162738"),
            200,
        ),
        ("Private needs no pool", Some("Private"), None, 200),
        ("no AccessType at all defaults to Private", None, None, 200),
    ];

    for (index, (case, access_type, pool, expected_status)) in cases.into_iter().enumerate() {
        let mut request = json!({
            "OutpostId": outpost_id,
            "SubnetId": format!("subnet-0000000000000b{index:03x}"),
            "SecurityGroupId": format!("sg-0000000000000b{index:03x}"),
        });
        if let Some(access_type) = access_type {
            request["AccessType"] = json!(access_type);
        }
        if let Some(pool) = pool {
            request["CustomerOwnedIpv4Pool"] = json!(pool);
        }

        let answer = create(&router, request).await;
        assert_eq!(
            answer.status, expected_status,
            "case {case:?}: expected {expected_status}, got {} with {:?}",
            answer.status, answer.body
        );
        if expected_status == 400 {
            assert_eq!(
                answer.error_code(),
                "ValidationException",
                "case {case:?}: must be refused as a validation failure, got {:?}",
                answer.body
            );
            continue;
        }

        let arn = string_at(&answer.body, "EndpointArn");
        let listed = list_endpoints(&router, "?maxResults=100").await;
        let endpoint = endpoint_with_arn(&listed, &arn);
        assert_eq!(
            endpoint["AccessType"],
            json!(access_type.unwrap_or("Private")),
            "case {case:?}: AccessType must be the stored one, got {:?}",
            endpoint["AccessType"]
        );
    }
}

/// `ListSharedEndpoints` reports the endpoints of the named Outpost that another account
/// owns — never this account's own, which `ListEndpoints` reports — and an Outpost the
/// store does not hold is the modelled 404.
#[tokio::test]
async fn list_shared_endpoints_reports_only_what_another_account_owns() {
    let router = emulator();
    let owned = owned_outposts(&router).await;
    let shared = shared_outpost(&router).await;
    let account = engine::emulator_account_id();

    // An endpoint of this account on every Outpost, the shared one included.
    let mut own_arns = Vec::new();
    for (index, outpost_id) in owned.iter().chain(std::iter::once(&shared)).enumerate() {
        let request = json!({
            "OutpostId": outpost_id,
            "SubnetId": format!("subnet-0000000000000c{index:03x}"),
            "SecurityGroupId": format!("sg-0000000000000c{index:03x}"),
        });
        let answer = create(&router, request).await;
        assert_eq!(
            answer.status, 200,
            "an endpoint must be creatable on Outpost {outpost_id}, got {} with {:?}",
            answer.status, answer.body
        );
        own_arns.push(string_at(&answer.body, "EndpointArn"));
    }

    // Every Outpost the store holds: only the shared one has an endpoint of another
    // account, and the caller's own endpoints never appear.
    for outpost_id in owned.iter().chain(std::iter::once(&shared)) {
        let answer = list_shared(&router, &format!("?outpostId={outpost_id}&maxResults=100")).await;
        assert_eq!(
            answer.status, 200,
            "case {outpost_id:?}: ListSharedEndpoints must answer 200, got {} with {:?}",
            answer.status, answer.body
        );
        let arns: Vec<String> = answer
            .endpoints()
            .iter()
            .map(|endpoint| string_at(endpoint, "EndpointArn"))
            .collect();
        assert!(
            arns.iter().all(|arn| !own_arns.contains(arn)),
            "case {outpost_id:?}: this account's own endpoints are not shared with it, got {arns:?}"
        );
        assert!(
            arns.iter()
                .all(|arn| !arn.contains(&format!(":{account}:"))),
            "case {outpost_id:?}: every shared endpoint is owned by another account, got {arns:?}"
        );
        for endpoint in answer.endpoints() {
            assert_eq!(
                endpoint["OutpostsId"],
                json!(outpost_id),
                "case {outpost_id:?}: every endpoint reported is on the named Outpost, got {:?}",
                endpoint["OutpostsId"]
            );
        }
        let expected = usize::from(outpost_id == &shared);
        assert_eq!(
            arns.len(),
            expected,
            "case {outpost_id:?}: expected {expected} shared endpoint(s), got {arns:?}"
        );
    }

    // Every form `OutpostId` admits, for an Outpost the store does not hold.
    for unknown in [UNKNOWN_OUTPOST_ID, "123456789012", "ec2"] {
        let answer = list_shared(&router, &format!("?outpostId={unknown}&maxResults=100")).await;
        assert_eq!(
            (answer.status, answer.error_code()),
            (404, "ResourceNotFoundException"),
            "case {unknown:?}: an unknown Outpost must answer 404, got {} with {:?}",
            answer.status,
            answer.body
        );
    }
}

/// The members S3 on Outposts derives rather than echoes keep their invariants: endpoints
/// in one subnet report one VPC, endpoints in different subnets report different VPCs, and
/// no two live endpoints are committed the same CIDR.
#[tokio::test]
async fn derived_endpoint_members_keep_their_invariants() {
    let router = emulator();
    let outposts = owned_outposts(&router).await;
    // Rows differ in whether the subnet repeats and whether the Outpost does.
    let cases = [
        ("first endpoint in subnet one", 0, "subnet-0d1e2f3041526374a"),
        ("second endpoint in subnet one, other Outpost", 1, "subnet-0d1e2f3041526374a"),
        ("an endpoint in subnet two", 0, "subnet-0d1e2f3041526374b"),
        ("an endpoint in subnet three", 1, "subnet-0d1e2f3041526374c"),
    ];

    let mut by_subnet: Vec<(String, String)> = Vec::new();
    let mut cidrs: Vec<String> = Vec::new();
    for (index, (case, outpost, subnet_id)) in cases.into_iter().enumerate() {
        let request = json!({
            "OutpostId": outposts[outpost % outposts.len()],
            "SubnetId": subnet_id,
            "SecurityGroupId": format!("sg-0000000000000d{index:03x}"),
        });
        let answer = create(&router, request).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: CreateEndpoint must answer 200, got {} with {:?}",
            answer.status, answer.body
        );
        let arn = string_at(&answer.body, "EndpointArn");
        let listed = list_endpoints(&router, "?maxResults=100").await;
        let endpoint = endpoint_with_arn(&listed, &arn);
        let vpc_id = string_at(&endpoint, "VpcId");
        let cidr = string_at(&endpoint, "CidrBlock");

        for (seen_subnet, seen_vpc) in &by_subnet {
            if seen_subnet == subnet_id {
                assert_eq!(
                    &vpc_id, seen_vpc,
                    "case {case:?}: endpoints in subnet {subnet_id} must report one VPC, got \
                     {vpc_id} after {seen_vpc}"
                );
            } else {
                assert_ne!(
                    &vpc_id, seen_vpc,
                    "case {case:?}: subnet {subnet_id} must not report the VPC of {seen_subnet}"
                );
            }
        }
        assert!(
            !cidrs.contains(&cidr),
            "case {case:?}: no two live endpoints may be committed {cidr}, already in {cidrs:?}"
        );

        by_subnet.push((subnet_id.to_string(), vpc_id));
        cidrs.push(cidr);
    }
}

/// Nothing a client can repeat grows the store without end: the side table that maps a
/// subnet to its VPC holds an entry only for a subnet a stored endpoint sits in, so a
/// create/delete sequence with a fresh subnet every time leaves nothing behind, and the
/// CIDR a deleted endpoint committed is offered again.
///
/// This case looks at the store directly, because what it pins is the absence of a
/// leak — which no response can show.
#[tokio::test]
async fn a_create_delete_sequence_leaves_no_subnet_mapping_behind() {
    let (router, engine) = emulator_with_engine();
    let outposts = owned_outposts(&router).await;
    let store = engine.state::<ComAmazonawsS3outpostsS3outpostsState>();

    // Whatever the seed holds is the floor: the sequence below must come back to it.
    let seeded_mappings = store.lock().subnet_vpcs.len();
    let seeded_endpoints = store.lock().endpoints.len();

    // Rows differ in the subnet's id width and in how many endpoints share it, so a case
    // that releases on the first delete and one that releases only on the last are both
    // covered.
    let cases: [(&str, &str, usize); 4] = [
        ("one endpoint, 17-hex subnet", "subnet-0e1f20314253647a0", 1),
        ("two endpoints in one subnet", "subnet-0e1f20314253647a1", 2),
        ("one endpoint, 8-hex subnet", "subnet-0e1f2031", 1),
        ("three endpoints in one subnet", "subnet-0e1f20314253647a2", 3),
    ];

    for (case, subnet_id, count) in cases {
        let mut live = Vec::new();
        for index in 0..count {
            let request = json!({
                "OutpostId": outposts[index % outposts.len()],
                "SubnetId": subnet_id,
                "SecurityGroupId": format!("sg-0000000000000e{index:03x}"),
            });
            let answer = create(&router, request).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: endpoint {index} must be created, got {} with {:?}",
                answer.status, answer.body
            );
            let arn = string_at(&answer.body, "EndpointArn");
            let endpoint =
                endpoint_with_arn(&list_endpoints(&router, "?maxResults=100").await, &arn);
            live.push((string_at(&endpoint, "OutpostsId"), endpoint_id_of(&arn)));
        }
        assert_eq!(
            store.lock().subnet_vpcs.len(),
            seeded_mappings + 1,
            "case {case:?}: {count} endpoint(s) in one subnet need exactly one mapping"
        );

        for (index, (outpost_id, endpoint_id)) in live.iter().enumerate() {
            assert_eq!(
                delete_endpoint(&router, outpost_id, endpoint_id).await.status,
                200,
                "case {case:?}: delete {index} must answer 200"
            );
            let expected = seeded_mappings + usize::from(index + 1 < live.len());
            assert_eq!(
                store.lock().subnet_vpcs.len(),
                expected,
                "case {case:?}: after delete {index} the mapping must survive only while an \
                 endpoint is left in {subnet_id}"
            );
        }

        assert_eq!(
            store.lock().endpoints.len(),
            seeded_endpoints,
            "case {case:?}: every endpoint the case created must be gone"
        );
    }

    assert_eq!(
        store.lock().subnet_vpcs.len(),
        seeded_mappings,
        "the whole sequence must leave the store as it found it"
    );
}
