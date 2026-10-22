// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Resolver endpoint slice: `CreateResolverEndpoint`,
//! `GetResolverEndpoint`, `ListResolverEndpoints`, `UpdateResolverEndpoint`,
//! `ListResolverEndpointIpAddresses` and `DeleteResolverEndpoint`.
//!
//! Every case goes over the wire the service actually speaks (awsJson1_1,
//! `POST /` with `X-Amz-Target: Route53Resolver.<OperationName>`), through the
//! same router the emulator mounts, so what is asserted is the HTTP behaviour
//! and not a handler called directly.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_route53resolver_route53resolver::state::{
    mint_page_token, MAX_FILTERS, MAX_FILTER_VALUES, MAX_SECURITY_GROUP_IDS, MAX_STORED_ENDPOINTS,
    PROVISIONING_DELAY,
};
use service_com_amazonaws_route53resolver_route53resolver::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// The region every request below is signed for; it is where the ARNs the
/// service mints must be rooted.
const REGION: &str = "us-west-2";

/// The canonical placeholder account id every emulator ARN carries.
const ACCOUNT: &str = "000000000000";

/// A fresh emulator: one shared engine, this one service mounted on it.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(engine));
    router
}

/// Call one operation and return `(http status, parsed body)`.
async fn call(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("Route53Resolver.{operation}"))
        // The SigV4 credential scope is what routes the request to this service
        // and what names the region its ARNs are minted in.
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20261021/{REGION}/route53resolver/aws4_request, \
                 SignedHeaders=host;x-amz-date, Signature=not-verified-in-this-test"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Call an operation that must succeed, and return its body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: Value) -> Value {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(status, 200, "{operation} must succeed, got {status}: {parsed}");
    parsed
}

/// The `__type` of an error response, which carries the modelled error's shape
/// name on the awsJson protocols.
fn error_type(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Assert an operation failed with `expected` as its modelled error.
async fn expect_error(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
    case: &str,
    expected: &str,
) {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: {operation} must answer HTTP 400, got {status}: {parsed}"
    );
    assert!(
        error_type(&parsed).contains(expected),
        "case {case:?}: {operation} must answer {expected}, got {parsed}"
    );
}

/// A create request body with `count` IP addresses in distinct subnets.
fn create_body(creator: &str, name: Option<&str>, direction: &str, count: usize) -> Value {
    let ip_addresses: Vec<Value> = (0..count)
        .map(|index| {
            json!({ "SubnetId": format!("subnet-{creator}-{index}"), "Ip": format!("10.0.{index}.10") })
        })
        .collect();
    let mut body = json!({
        "CreatorRequestId": creator,
        "Direction": direction,
        "SecurityGroupIds": ["sg-00000000000000001"],
        "IpAddresses": ip_addresses,
    });
    if let Some(name) = name {
        body["Name"] = json!(name);
    }
    body
}

/// The `ResolverEndpoint` of a create/get/update/delete response.
fn endpoint(body: &Value) -> &Value {
    body.get("ResolverEndpoint")
        .expect("the response carries a ResolverEndpoint")
}

fn string_member(value: &Value, member: &str) -> String {
    value
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The same object with `Status` dropped, so two reads can be compared without
/// the member the clock is allowed to move.
fn without_status(value: &Value) -> Value {
    let mut cloned = value.clone();
    if let Some(object) = cloned.as_object_mut() {
        object.remove("Status");
    }
    cloned
}

/// Parse a modelled `Rfc3339TimeString` (length 20..40) into a point in time, so
/// two timestamps are compared numerically rather than as text.
fn parse_time(case: &str, member: &str, raw: &str) -> aws_smithy_types::DateTime {
    assert!(
        raw.len() >= 20 && raw.len() <= 40,
        "case {case:?}: {member} must satisfy Rfc3339TimeString's length 20..40, got {raw:?}"
    );
    aws_smithy_types::DateTime::from_str(raw, aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_else(|error| {
            panic!("case {case:?}: {member} must be an RFC 3339 time, got {raw:?}: {error}")
        })
}

/// One row of the create table: the shape of the request, and what the stored
/// endpoint must then report.
#[derive(Debug)]
struct CreateCase {
    case: &'static str,
    body: Value,
    /// `Id` prefix: AWS names an inbound endpoint `rslvr-in-…` and an outbound
    /// one `rslvr-out-…`.
    id_prefix: &'static str,
    ip_count: i64,
    /// `Protocols`, which defaults to `["Do53"]` when the request names none.
    protocols: Vec<&'static str>,
}

/// A create is persisted: every member of the response is computed from the
/// stored record, a `GetResolverEndpoint` returns the same record, and the IP
/// addresses supplied come back with emulator-assigned `IpId`s.
///
/// The table spans what the rule generalises over: direction (all three the
/// model's `ResolverEndpointDirection` enum declares), the number of IP
/// addresses (the modelled 2..20 and a value in between), address family
/// (IPv4, IPv6, both), an absent `Name`, and `Protocols` present or defaulted.
#[tokio::test]
async fn a_created_endpoint_is_read_back_from_state() {
    let cases = vec![
        CreateCase {
            case: "outbound, two IPv4 addresses, named",
            body: create_body("req-outbound", Some("my-outbound"), "OUTBOUND", 2),
            id_prefix: "rslvr-out-",
            ip_count: 2,
            protocols: vec!["Do53"],
        },
        CreateCase {
            case: "inbound, five IPv4 addresses, named",
            body: create_body("req-inbound", Some("my-inbound"), "INBOUND", 5),
            id_prefix: "rslvr-in-",
            ip_count: 5,
            protocols: vec!["Do53"],
        },
        CreateCase {
            case: "inbound delegation, no Name",
            body: create_body("req-delegation", None, "INBOUND_DELEGATION", 2),
            id_prefix: "rslvr-in-",
            ip_count: 2,
            protocols: vec!["Do53"],
        },
        CreateCase {
            case: "twenty IP addresses, the modelled maximum",
            body: create_body("req-max", Some("twenty-ips"), "OUTBOUND", 20),
            id_prefix: "rslvr-out-",
            ip_count: 20,
            protocols: vec!["Do53"],
        },
        CreateCase {
            case: "two protocols, explicitly requested",
            body: {
                let mut body = create_body("req-protocols", Some("doh"), "INBOUND", 2);
                body["Protocols"] = json!(["Do53", "DoH"]);
                body["ResolverEndpointType"] = json!("DUALSTACK");
                body
            },
            id_prefix: "rslvr-in-",
            ip_count: 2,
            protocols: vec!["Do53", "DoH"],
        },
        CreateCase {
            case: "IPv6-only addresses",
            body: json!({
                "CreatorRequestId": "req-ipv6",
                "Direction": "OUTBOUND",
                "SecurityGroupIds": ["sg-1", "sg-2"],
                "ResolverEndpointType": "IPV6",
                "IpAddresses": [
                    { "SubnetId": "subnet-v6-a", "Ipv6": "2001:db8::a" },
                    { "SubnetId": "subnet-v6-b", "Ipv6": "2001:db8::b" },
                ],
            }),
            id_prefix: "rslvr-out-",
            ip_count: 2,
            protocols: vec!["Do53"],
        },
        CreateCase {
            case: "dual-stack addresses and tags",
            body: json!({
                "CreatorRequestId": "req-dual",
                "Direction": "INBOUND",
                "SecurityGroupIds": ["sg-dual"],
                "ResolverEndpointType": "DUALSTACK",
                "Tags": [{ "Key": "team", "Value": "networking" }],
                "IpAddresses": [
                    { "SubnetId": "subnet-d-a", "Ip": "10.1.0.10", "Ipv6": "2001:db8:1::a" },
                    { "SubnetId": "subnet-d-b", "Ip": "10.1.1.10", "Ipv6": "2001:db8:1::b" },
                ],
            }),
            id_prefix: "rslvr-in-",
            ip_count: 2,
            protocols: vec!["Do53"],
        },
    ];

    for CreateCase {
        case,
        body,
        id_prefix,
        ip_count,
        protocols,
    } in cases
    {
        // Each row gets its own emulator, so the store holds exactly this row.
        let router = emulator();
        let requested = body.clone();

        let created = ok(&router, "CreateResolverEndpoint", body).await;
        let created = endpoint(&created).clone();

        let id = string_member(&created, "Id");
        assert!(
            id.starts_with(id_prefix),
            "case {case:?}: Id must start with {id_prefix:?}, got {id:?}"
        );
        assert_eq!(
            string_member(&created, "Arn"),
            format!("arn:aws:route53resolver:{REGION}:{ACCOUNT}:resolver-endpoint/{id}"),
            "case {case:?}: the Arn must be rooted in the signed region and the emulator account"
        );
        assert_eq!(
            created.get("IpAddressCount").and_then(Value::as_i64),
            Some(ip_count),
            "case {case:?}: IpAddressCount must be the number of IpAddresses supplied, got {created}"
        );
        assert_eq!(
            string_member(&created, "Status"),
            "CREATING",
            "case {case:?}: a just-created endpoint is still CREATING, got {created}"
        );
        assert_eq!(
            created.get("Protocols"),
            Some(&json!(protocols)),
            "case {case:?}: Protocols must be the requested list, or Do53 when none was named, got {created}"
        );
        assert_eq!(
            created.get("CreatorRequestId"),
            requested.get("CreatorRequestId"),
            "case {case:?}: CreatorRequestId must be the one that created the endpoint, got {created}"
        );
        assert_eq!(
            created.get("Name"),
            requested.get("Name"),
            "case {case:?}: Name must be the one requested, got {created}"
        );
        assert_eq!(
            created.get("Direction"),
            requested.get("Direction"),
            "case {case:?}: Direction must be the one requested, got {created}"
        );
        assert_eq!(
            created.get("SecurityGroupIds"),
            requested.get("SecurityGroupIds"),
            "case {case:?}: SecurityGroupIds must be the ones requested, got {created}"
        );
        assert_eq!(
            created.get("ResolverEndpointType"),
            requested.get("ResolverEndpointType"),
            "case {case:?}: ResolverEndpointType must be the one requested, got {created}"
        );
        let creation = parse_time(case, "CreationTime", &string_member(&created, "CreationTime"));
        let modification = parse_time(
            case,
            "ModificationTime",
            &string_member(&created, "ModificationTime"),
        );
        assert_eq!(
            creation, modification,
            "case {case:?}: a never-updated endpoint was modified when it was created, got {created}"
        );
        let host_vpc = string_member(&created, "HostVPCId");
        assert!(
            host_vpc.starts_with("vpc-"),
            "case {case:?}: HostVPCId must be a VPC id derived from the subnets, got {host_vpc:?}"
        );

        // The read-back: same record, through the implemented reader.
        let fetched = ok(
            &router,
            "GetResolverEndpoint",
            json!({ "ResolverEndpointId": id }),
        )
        .await;
        let fetched = endpoint(&fetched).clone();
        assert_eq!(
            without_status(&fetched),
            without_status(&created),
            "case {case:?}: GetResolverEndpoint must return the record CreateResolverEndpoint stored"
        );

        // And the IP addresses, with the ids the emulator assigned.
        let listed = ok(
            &router,
            "ListResolverEndpointIpAddresses",
            json!({ "ResolverEndpointId": id }),
        )
        .await;
        let addresses = listed
            .get("IpAddresses")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let requested_addresses = requested
            .get("IpAddresses")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            addresses.len(),
            requested_addresses.len(),
            "case {case:?}: every IP address supplied must be stored, got {listed}"
        );
        let mut seen_ids = std::collections::HashSet::new();
        for (index, (stored, asked)) in addresses.iter().zip(&requested_addresses).enumerate() {
            assert_eq!(
                stored.get("SubnetId"),
                asked.get("SubnetId"),
                "case {case:?}: address {index} must keep its SubnetId, got {stored}"
            );
            assert_eq!(
                stored.get("Ip"),
                asked.get("Ip"),
                "case {case:?}: address {index} must keep its Ip, got {stored}"
            );
            assert_eq!(
                stored.get("Ipv6"),
                asked.get("Ipv6"),
                "case {case:?}: address {index} must keep its Ipv6, got {stored}"
            );
            let ip_id = string_member(stored, "IpId");
            assert!(
                ip_id.starts_with("rni-"),
                "case {case:?}: address {index} must carry an emulator-assigned IpId, got {ip_id:?}"
            );
            assert!(
                seen_ids.insert(ip_id.clone()),
                "case {case:?}: IpId {ip_id:?} was assigned to two addresses"
            );
            parse_time(case, "IP CreationTime", &string_member(stored, "CreationTime"));
        }

        // The endpoint is in the store, so the list operation reports it too.
        let all = ok(&router, "ListResolverEndpoints", json!({})).await;
        let ids: Vec<String> = all
            .get("ResolverEndpoints")
            .and_then(Value::as_array)
            .map(|items| items.iter().map(|item| string_member(item, "Id")).collect())
            .unwrap_or_default();
        assert_eq!(
            ids,
            vec![id.clone()],
            "case {case:?}: ListResolverEndpoints must report exactly the stored endpoint"
        );
    }
}

/// A retried create with the same `CreatorRequestId` resolves to the endpoint
/// that request already made: "CreatorRequestId ... allows failed requests to be
/// retried without the risk of running the operation twice."
#[tokio::test]
async fn a_repeated_creator_request_id_does_not_create_a_second_endpoint() {
    let router = emulator();
    let body = create_body("req-retried", Some("once-only"), "OUTBOUND", 2);

    let first = ok(&router, "CreateResolverEndpoint", body.clone()).await;
    let second = ok(&router, "CreateResolverEndpoint", body).await;

    assert_eq!(
        without_status(endpoint(&first)),
        without_status(endpoint(&second)),
        "a retried create must return the endpoint the first one made"
    );

    let all = ok(&router, "ListResolverEndpoints", json!({})).await;
    let count = all
        .get("ResolverEndpoints")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    assert_eq!(
        count, 1,
        "a retried create must not store a second endpoint, got {all}"
    );
}

/// The endpoint AWS answers `CREATING` reaches `OPERATIONAL` on its own, and so
/// does each of its IP addresses (`CREATING` -> `ATTACHED`). The transition is
/// owed by the clock, not by the read: the caller that reads immediately still
/// sees the transient status.
#[tokio::test]
async fn the_creating_status_settles_without_anyone_asking() {
    let router = emulator();
    let created = ok(
        &router,
        "CreateResolverEndpoint",
        create_body("req-settles", Some("settling"), "OUTBOUND", 2),
    )
    .await;
    let id = string_member(endpoint(&created), "Id");

    let immediate = ok(
        &router,
        "GetResolverEndpoint",
        json!({ "ResolverEndpointId": id }),
    )
    .await;
    assert_eq!(
        string_member(endpoint(&immediate), "Status"),
        "CREATING",
        "a read taken straight after the create must still see CREATING, got {immediate}"
    );
    let immediate_ips = ok(
        &router,
        "ListResolverEndpointIpAddresses",
        json!({ "ResolverEndpointId": id }),
    )
    .await;
    for address in immediate_ips
        .get("IpAddresses")
        .and_then(Value::as_array)
        .unwrap_or(&vec![])
    {
        assert_eq!(
            string_member(address, "Status"),
            "CREATING",
            "case \"immediately after create\": an IP address must still be CREATING, got {address}"
        );
    }

    tokio::time::sleep(PROVISIONING_DELAY + Duration::from_millis(250)).await;

    let settled = ok(
        &router,
        "GetResolverEndpoint",
        json!({ "ResolverEndpointId": id }),
    )
    .await;
    assert_eq!(
        string_member(endpoint(&settled), "Status"),
        "OPERATIONAL",
        "once the owed transition falls due the endpoint must report OPERATIONAL, got {settled}"
    );
    let settled_ips = ok(
        &router,
        "ListResolverEndpointIpAddresses",
        json!({ "ResolverEndpointId": id }),
    )
    .await;
    for address in settled_ips
        .get("IpAddresses")
        .and_then(Value::as_array)
        .unwrap_or(&vec![])
    {
        assert_eq!(
            string_member(address, "Status"),
            "ATTACHED",
            "case \"after the owed transition\": an IP address must be ATTACHED, got {address}"
        );
    }
}

/// Create `count` endpoints and return their ids in creation order.
async fn seed_endpoints(router: &MultiServiceRouter, count: usize) -> Vec<String> {
    let mut ids = Vec::with_capacity(count);
    for index in 0..count {
        let created = ok(
            router,
            "CreateResolverEndpoint",
            create_body(&format!("req-seed-{index}"), None, "OUTBOUND", 2),
        )
        .await;
        ids.push(string_member(endpoint(&created), "Id"));
    }
    ids
}

/// Paging walks every stored endpoint exactly once, whatever the page size: the
/// `NextToken` resumes where the previous page stopped, and the last page
/// carries none.
#[tokio::test]
async fn paging_walks_every_endpoint_exactly_once() {
    let router = emulator();
    let expected = seed_endpoints(&router, 5).await;

    // Page sizes spanning the modelled MaxResults range (1..100): the minimum,
    // sizes that do and do not divide the collection, exactly the collection
    // size, the maximum, and MaxResults omitted altogether.
    for max_results in [Some(1), Some(2), Some(3), Some(5), Some(100), None] {
        let case = format!("MaxResults={max_results:?}");
        let mut walked: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;

        loop {
            let mut request = json!({});
            if let Some(size) = max_results {
                request["MaxResults"] = json!(size);
            }
            if let Some(token) = &next_token {
                request["NextToken"] = json!(token);
            }
            let page = ok(&router, "ListResolverEndpoints", request).await;
            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "case {case:?}: paging did not terminate after {pages} pages"
            );

            let items = page
                .get("ResolverEndpoints")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if let Some(size) = max_results {
                assert!(
                    i64::try_from(items.len()).unwrap_or(i64::MAX) <= i64::from(size),
                    "case {case:?}: a page must hold at most MaxResults items, got {page}"
                );
            }
            assert_eq!(
                page.get("MaxResults").and_then(Value::as_i64),
                max_results.map(i64::from),
                "case {case:?}: MaxResults must echo the value the request specified, got {page}"
            );
            walked.extend(items.iter().map(|item| string_member(item, "Id")));

            next_token = page
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if next_token.is_none() {
                break;
            }
        }

        assert_eq!(
            walked, expected,
            "case {case:?}: paging must yield every stored endpoint exactly once, in creation order"
        );
    }
}

/// A `NextToken` this service did not mint is refused with
/// `InvalidNextTokenException`, rather than silently read as "start over".
#[tokio::test]
async fn a_next_token_this_service_did_not_mint_is_refused() {
    let router = emulator();
    let ids = seed_endpoints(&router, 3).await;

    // A cursor minted for the per-endpoint IP collection is not a cursor the
    // endpoint list accepts, and vice versa. The endpoint list's own scope also
    // carries its filter set, so a bare "ListResolverEndpoints" scope is one no
    // query ever mints.
    let foreign_scope = mint_page_token(
        &format!("ListResolverEndpointIpAddresses/{}", ids[0]),
        1,
    );
    let scope_no_query_mints = mint_page_token("ListResolverEndpoints", 1);

    for (case, token) in [
        ("not hex at all", "next-please".to_string()),
        ("empty", String::new()),
        ("hex, but not a cursor", hex::encode("ListResolverEndpoints:1")),
        (
            "right payload, wrong checksum",
            hex::encode("ListResolverEndpoints:1:0000000000000000"),
        ),
        ("a cursor for another collection", foreign_scope.clone()),
        ("a scope no query mints", scope_no_query_mints.clone()),
    ] {
        expect_error(
            &router,
            "ListResolverEndpoints",
            json!({ "NextToken": token }),
            case,
            "InvalidNextTokenException",
        )
        .await;
    }

    for (case, token) in [
        ("a cursor for the endpoint list", scope_no_query_mints),
        ("not hex at all", "next-please".to_string()),
    ] {
        expect_error(
            &router,
            "ListResolverEndpointIpAddresses",
            json!({ "ResolverEndpointId": ids[0].clone(), "NextToken": token }),
            case,
            "InvalidNextTokenException",
        )
        .await;
    }

    // The cursor the IP list itself minted is accepted, so the refusals above
    // are about provenance and not about rejecting every token.
    let first_page = ok(
        &router,
        "ListResolverEndpointIpAddresses",
        json!({ "ResolverEndpointId": ids[0].clone(), "MaxResults": 1 }),
    )
    .await;
    let token = first_page
        .get("NextToken")
        .and_then(Value::as_str)
        .expect("a two-address endpoint paged at 1 has a second page")
        .to_string();
    let second_page = ok(
        &router,
        "ListResolverEndpointIpAddresses",
        json!({ "ResolverEndpointId": ids[0].clone(), "MaxResults": 1, "NextToken": token }),
    )
    .await;
    let second = second_page
        .get("IpAddresses")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        second.len(),
        1,
        "the cursor this collection minted must resume it, got {second_page}"
    );
    assert_ne!(
        string_member(&second[0], "IpId"),
        first_page
            .get("IpAddresses")
            .and_then(Value::as_array)
            .map(|items| string_member(&items[0], "IpId"))
            .unwrap_or_default(),
        "the second page must resume after the first, got {second_page}"
    );
}

/// A `ListResolverEndpoints` cursor indexes the FILTERED collection, so it is
/// only accepted by the query that minted it: resuming it under a different
/// `Filters` is `InvalidNextTokenException`, not a page cut at the wrong place.
#[tokio::test]
async fn an_endpoint_list_cursor_belongs_to_the_query_that_minted_it() {
    let router = emulator();
    for index in 0..4 {
        // Two OUTBOUND and two INBOUND endpoints, so a filtered walk and an
        // unfiltered one have different lengths.
        let direction = if index % 2 == 0 { "OUTBOUND" } else { "INBOUND" };
        ok(
            &router,
            "CreateResolverEndpoint",
            create_body(&format!("req-scope-{index}"), None, direction, 2),
        )
        .await;
    }

    let outbound = json!([{ "Name": "Direction", "Values": ["OUTBOUND"] }]);
    let first = ok(
        &router,
        "ListResolverEndpoints",
        json!({ "MaxResults": 1, "Filters": outbound }),
    )
    .await;
    let token = first
        .get("NextToken")
        .and_then(Value::as_str)
        .expect("two OUTBOUND endpoints paged at 1 have a second page")
        .to_string();

    // Each row resumes that cursor under a different query.
    for (case, filters) in [
        ("no filters at all", json!([])),
        (
            "a different value",
            json!([{ "Name": "Direction", "Values": ["INBOUND"] }]),
        ),
        (
            "a different member",
            json!([{ "Name": "Name", "Values": ["OUTBOUND"] }]),
        ),
        (
            "an extra filter",
            json!([
                { "Name": "Direction", "Values": ["OUTBOUND"] },
                { "Name": "Status", "Values": ["CREATING"] },
            ]),
        ),
    ] {
        expect_error(
            &router,
            "ListResolverEndpoints",
            json!({ "MaxResults": 1, "Filters": filters, "NextToken": token.clone() }),
            case,
            "InvalidNextTokenException",
        )
        .await;
    }

    // The query that minted it resumes it, and the scope is canonical: the same
    // filter named with the early upper-snake spelling is the same query.
    for (case, filters) in [
        ("the same filter set", outbound.clone()),
        (
            "the same filter set, the early spelling",
            json!([{ "Name": "DIRECTION", "Values": ["OUTBOUND"] }]),
        ),
    ] {
        let second = ok(
            &router,
            "ListResolverEndpoints",
            json!({ "MaxResults": 1, "Filters": filters, "NextToken": token.clone() }),
        )
        .await;
        let items = second
            .get("ResolverEndpoints")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            items.len(),
            1,
            "case {case:?}: the cursor its own query minted must resume it, got {second}"
        );
        assert_ne!(
            string_member(&items[0], "Id"),
            first
                .get("ResolverEndpoints")
                .and_then(Value::as_array)
                .map(|page| string_member(&page[0], "Id"))
                .unwrap_or_default(),
            "case {case:?}: the second page must resume after the first, got {second}"
        );
    }
}

/// `ListResolverEndpoints` filters restrict the page to the endpoints whose
/// named member matches one of `Values`.
///
/// The names are exactly the catalog the model documents for this operation
/// (`smithy_query(query="doc",shape="Filter",member="Name")`, section
/// **ListResolverEndpoints**): `CreatorRequestId`, `Direction`, `HostVPCId`,
/// `IpAddressCount`, `Name`, `SecurityGroupIds`, `Status` — each exercised here,
/// in both the documented spelling and the early upper-snake one the same doc
/// says is "still supported".
#[tokio::test]
async fn filters_select_on_every_documented_member() {
    let router = emulator();

    let alpha = ok(
        &router,
        "CreateResolverEndpoint",
        json!({
            "CreatorRequestId": "req-alpha",
            "Name": "alpha",
            "Direction": "OUTBOUND",
            "SecurityGroupIds": ["sg-shared", "sg-alpha"],
            "IpAddresses": [
                { "SubnetId": "subnet-alpha-1", "Ip": "10.0.0.1" },
                { "SubnetId": "subnet-alpha-2", "Ip": "10.0.0.2" },
            ],
        }),
    )
    .await;
    let alpha = endpoint(&alpha).clone();
    let beta = ok(
        &router,
        "CreateResolverEndpoint",
        json!({
            "CreatorRequestId": "req-beta",
            "Name": "beta",
            "Direction": "INBOUND",
            "SecurityGroupIds": ["sg-shared"],
            "IpAddresses": [
                { "SubnetId": "subnet-beta-1", "Ip": "10.0.1.1" },
                { "SubnetId": "subnet-beta-2", "Ip": "10.0.1.2" },
            ],
        }),
    )
    .await;
    let beta = endpoint(&beta).clone();
    let gamma = ok(
        &router,
        "CreateResolverEndpoint",
        json!({
            "CreatorRequestId": "req-gamma",
            "Name": "gamma",
            "Direction": "INBOUND_DELEGATION",
            "SecurityGroupIds": ["sg-gamma"],
            "IpAddresses": [
                { "SubnetId": "subnet-gamma-1", "Ip": "10.0.2.1" },
                { "SubnetId": "subnet-gamma-2", "Ip": "10.0.2.2" },
                { "SubnetId": "subnet-gamma-3", "Ip": "10.0.2.3" },
            ],
        }),
    )
    .await;
    let gamma = endpoint(&gamma).clone();

    let alpha_id = string_member(&alpha, "Id");
    let beta_id = string_member(&beta, "Id");
    let gamma_id = string_member(&gamma, "Id");
    let alpha_vpc = string_member(&alpha, "HostVPCId");

    let cases: Vec<(&str, Value, Vec<String>)> = vec![
        (
            "CreatorRequestId",
            json!([{ "Name": "CreatorRequestId", "Values": ["req-beta"] }]),
            vec![beta_id.clone()],
        ),
        (
            "CREATOR_REQUEST_ID, the early spelling",
            json!([{ "Name": "CREATOR_REQUEST_ID", "Values": ["req-beta"] }]),
            vec![beta_id.clone()],
        ),
        (
            "Direction, one value",
            json!([{ "Name": "Direction", "Values": ["INBOUND_DELEGATION"] }]),
            vec![gamma_id.clone()],
        ),
        (
            "DIRECTION, two values",
            json!([{ "Name": "DIRECTION", "Values": ["OUTBOUND", "INBOUND"] }]),
            vec![alpha_id.clone(), beta_id.clone()],
        ),
        (
            "Name",
            json!([{ "Name": "Name", "Values": ["gamma"] }]),
            vec![gamma_id.clone()],
        ),
        (
            "HostVPCId",
            json!([{ "Name": "HostVPCId", "Values": [alpha_vpc.clone()] }]),
            vec![alpha_id.clone()],
        ),
        (
            "HOST_VPC_ID, the early spelling",
            json!([{ "Name": "HOST_VPC_ID", "Values": [alpha_vpc] }]),
            vec![alpha_id.clone()],
        ),
        (
            "IpAddressCount",
            json!([{ "Name": "IpAddressCount", "Values": ["3"] }]),
            vec![gamma_id.clone()],
        ),
        (
            "IP_ADDRESS_COUNT, the early spelling",
            json!([{ "Name": "IP_ADDRESS_COUNT", "Values": ["2"] }]),
            vec![alpha_id.clone(), beta_id.clone()],
        ),
        (
            "SecurityGroupIds, matching one of several",
            json!([{ "Name": "SecurityGroupIds", "Values": ["sg-shared"] }]),
            vec![alpha_id.clone(), beta_id.clone()],
        ),
        (
            "SECURITY_GROUP_IDS, the early spelling",
            json!([{ "Name": "SECURITY_GROUP_IDS", "Values": ["sg-gamma"] }]),
            vec![gamma_id.clone()],
        ),
        (
            "Status, the status every fresh endpoint has",
            json!([{ "Name": "Status", "Values": ["CREATING"] }]),
            vec![alpha_id.clone(), beta_id.clone(), gamma_id.clone()],
        ),
        (
            "Status, one no endpoint has yet",
            json!([{ "Name": "Status", "Values": ["ACTION_NEEDED"] }]),
            vec![],
        ),
        (
            "two filters combine with AND, both matching",
            json!([
                { "Name": "Direction", "Values": ["INBOUND"] },
                { "Name": "Name", "Values": ["beta"] },
            ]),
            vec![beta_id.clone()],
        ),
        (
            "two filters combine with AND, disagreeing",
            json!([
                { "Name": "Direction", "Values": ["INBOUND"] },
                { "Name": "Name", "Values": ["alpha"] },
            ]),
            vec![],
        ),
        (
            "an empty value list matches nothing",
            json!([{ "Name": "Name", "Values": [] }]),
            vec![],
        ),
        ("no filters at all", json!([]), vec![
            alpha_id.clone(),
            beta_id.clone(),
            gamma_id.clone(),
        ]),
    ];

    for (case, filters, expected) in cases {
        let listed = ok(
            &router,
            "ListResolverEndpoints",
            json!({ "Filters": filters }),
        )
        .await;
        let got: Vec<String> = listed
            .get("ResolverEndpoints")
            .and_then(Value::as_array)
            .map(|items| items.iter().map(|item| string_member(item, "Id")).collect())
            .unwrap_or_default();
        assert_eq!(
            got, expected,
            "case {case:?}: the filtered page is wrong, got {listed}"
        );
    }

    // A name outside the documented catalog is refused, not ignored. (The empty
    // name is not in the table: `FilterName`'s modelled length 1..64 refuses it
    // at the boundary, before this handler runs.)
    for case in ["Arn", "VPCId", "DomainName", "Id", "Host_VPC"] {
        expect_error(
            &router,
            "ListResolverEndpoints",
            json!({ "Filters": [{ "Name": case, "Values": ["anything"] }] }),
            case,
            "InvalidParameterException",
        )
        .await;
    }
}

/// An update writes only the members the request carries, bumps
/// `ModificationTime`, and the next read observes it.
#[tokio::test]
async fn an_update_writes_only_the_members_the_request_carries() {
    // Each row: the update body, then the members that must change.
    /// What one update row asserts.
    struct UpdateCase {
        case: &'static str,
        update: Value,
        changes: Vec<(&'static str, Value)>,
    }
    let cases = vec![
        UpdateCase {
            case: "Name alone",
            update: json!({ "Name": "renamed" }),
            changes: vec![("Name", json!("renamed"))],
        },
        UpdateCase {
            case: "Protocols alone",
            update: json!({ "Protocols": ["Do53", "DoH"] }),
            changes: vec![("Protocols", json!(["Do53", "DoH"]))],
        },
        UpdateCase {
            case: "ResolverEndpointType alone",
            update: json!({ "ResolverEndpointType": "DUALSTACK" }),
            changes: vec![("ResolverEndpointType", json!("DUALSTACK"))],
        },
        UpdateCase {
            case: "a boolean alone",
            update: json!({ "Dns64Enabled": true }),
            changes: vec![("Dns64Enabled", json!(true))],
        },
        UpdateCase {
            case: "Name and Protocols together",
            update: json!({ "Name": "renamed-again", "Protocols": ["DoH"] }),
            changes: vec![
                ("Name", json!("renamed-again")),
                ("Protocols", json!(["DoH"])),
            ],
        },
        UpdateCase {
            case: "no optional member at all",
            update: json!({}),
            changes: vec![],
        },
    ];

    for UpdateCase {
        case,
        update,
        changes,
    } in cases
    {
        let router = emulator();
        let created = ok(
            &router,
            "CreateResolverEndpoint",
            {
                let mut body = create_body("req-update", Some("original"), "INBOUND", 2);
                body["ResolverEndpointType"] = json!("IPV4");
                body
            },
        )
        .await;
        let before = endpoint(&created).clone();
        let id = string_member(&before, "Id");

        // Real elapsed time, so a bumped ModificationTime is observable.
        tokio::time::sleep(Duration::from_millis(20)).await;

        let mut request = update.clone();
        request["ResolverEndpointId"] = json!(id);
        let updated = ok(&router, "UpdateResolverEndpoint", request).await;
        let after = endpoint(&updated).clone();

        let fetched = ok(
            &router,
            "GetResolverEndpoint",
            json!({ "ResolverEndpointId": id }),
        )
        .await;
        assert_eq!(
            without_status(endpoint(&fetched)),
            without_status(&after),
            "case {case:?}: the read after the update must return what the update stored"
        );

        for (member, expected) in &changes {
            assert_eq!(
                after.get(*member),
                Some(expected),
                "case {case:?}: {member} must be the updated value, got {after}"
            );
        }

        // Every member the request did not carry keeps the value the create set.
        for member in [
            "Id",
            "Arn",
            "CreatorRequestId",
            "Direction",
            "SecurityGroupIds",
            "IpAddressCount",
            "HostVPCId",
            "CreationTime",
            "Name",
            "Protocols",
            "ResolverEndpointType",
            "Dns64Enabled",
        ] {
            if changes.iter().any(|(changed, _)| *changed == member) {
                continue;
            }
            assert_eq!(
                after.get(member),
                before.get(member),
                "case {case:?}: {member} was not in the request, so it must not change"
            );
        }

        let created_at = parse_time(case, "CreationTime", &string_member(&after, "CreationTime"));
        let modified_at = parse_time(
            case,
            "ModificationTime",
            &string_member(&after, "ModificationTime"),
        );
        if changes.is_empty() {
            // An update that writes nothing is not a modification: it must not move
            // the timestamp or drive the endpoint out of the status it was in.
            assert_eq!(
                modified_at, created_at,
                "case {case:?}: an update that wrote nothing must not move ModificationTime, got {after}"
            );
            assert_eq!(
                string_member(&after, "Status"),
                string_member(&before, "Status"),
                "case {case:?}: an update that wrote nothing must not move Status, got {after}"
            );
        } else {
            assert!(
                modified_at.secs() > created_at.secs()
                    || (modified_at.secs() == created_at.secs()
                        && modified_at.subsec_nanos() > created_at.subsec_nanos()),
                "case {case:?}: an update must bump ModificationTime past CreationTime, got {after}"
            );
            assert_eq!(
                string_member(&after, "Status"),
                "UPDATING",
                "case {case:?}: an endpoint that was just updated reports UPDATING, got {after}"
            );
        }
    }
}

/// `UpdateIpAddresses` names one stored IP address by its `IpId` and sets its
/// `Ipv6`; an `IpId` the endpoint does not hold is refused and nothing is
/// written.
#[tokio::test]
async fn updating_an_ip_address_writes_that_address_only() {
    let router = emulator();
    let created = ok(
        &router,
        "CreateResolverEndpoint",
        create_body("req-ip-update", Some("ips"), "OUTBOUND", 3),
    )
    .await;
    let id = string_member(endpoint(&created), "Id");

    let listed = ok(
        &router,
        "ListResolverEndpointIpAddresses",
        json!({ "ResolverEndpointId": id }),
    )
    .await;
    let before = listed
        .get("IpAddresses")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let target = string_member(&before[1], "IpId");

    expect_error(
        &router,
        "UpdateResolverEndpoint",
        json!({
            "ResolverEndpointId": id,
            "UpdateIpAddresses": [{ "IpId": "rni-0000000000000000", "Ipv6": "2001:db8::1" }],
        }),
        "an IpId this endpoint does not hold",
        "ResourceNotFoundException",
    )
    .await;

    let unchanged = ok(
        &router,
        "ListResolverEndpointIpAddresses",
        json!({ "ResolverEndpointId": id }),
    )
    .await;
    assert_eq!(
        unchanged.get("IpAddresses"),
        listed.get("IpAddresses"),
        "a refused update must leave every stored IP address alone, got {unchanged}"
    );

    ok(
        &router,
        "UpdateResolverEndpoint",
        json!({
            "ResolverEndpointId": id,
            "UpdateIpAddresses": [{ "IpId": target.clone(), "Ipv6": "2001:db8::beef" }],
        }),
    )
    .await;

    let after = ok(
        &router,
        "ListResolverEndpointIpAddresses",
        json!({ "ResolverEndpointId": id }),
    )
    .await;
    let after = after
        .get("IpAddresses")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for (index, (stored, original)) in after.iter().zip(&before).enumerate() {
        let case = format!("address {index}");
        if string_member(stored, "IpId") == target {
            assert_eq!(
                stored.get("Ipv6"),
                Some(&json!("2001:db8::beef")),
                "case {case:?}: the named address must carry the new Ipv6, got {stored}"
            );
            assert_eq!(
                string_member(stored, "Status"),
                "UPDATING",
                "case {case:?}: the named address is being updated, got {stored}"
            );
        } else {
            assert_eq!(
                stored, original,
                "case {case:?}: an address the request did not name must not change"
            );
        }
    }
}

/// Documented: "You can only update between IPV4 and DUALSTACK, IPV6 endpoint
/// type can't be updated to other type."
#[tokio::test]
async fn the_endpoint_type_can_only_move_between_ipv4_and_dualstack() {
    // Each row: the type the endpoint was created with, the type requested, and
    // whether the update is allowed.
    let cases = [
        ("IPV4", "DUALSTACK", true),
        ("DUALSTACK", "IPV4", true),
        ("DUALSTACK", "DUALSTACK", true),
        ("IPV4", "IPV6", false),
        ("DUALSTACK", "IPV6", false),
        ("IPV6", "IPV4", false),
        ("IPV6", "DUALSTACK", false),
        ("IPV6", "IPV6", true),
    ];

    for (from, to, allowed) in cases {
        let case = format!("{from} -> {to}");
        let router = emulator();
        let mut body = create_body("req-type", Some("typed"), "OUTBOUND", 2);
        body["ResolverEndpointType"] = json!(from);
        let created = ok(&router, "CreateResolverEndpoint", body).await;
        let id = string_member(endpoint(&created), "Id");

        let request = json!({ "ResolverEndpointId": id, "ResolverEndpointType": to });
        if allowed {
            let updated = ok(&router, "UpdateResolverEndpoint", request).await;
            assert_eq!(
                string_member(endpoint(&updated), "ResolverEndpointType"),
                to,
                "case {case:?}: the update is allowed, so the type must change, got {updated}"
            );
        } else {
            expect_error(
                &router,
                "UpdateResolverEndpoint",
                request,
                &case,
                "InvalidRequestException",
            )
            .await;
            let fetched = ok(
                &router,
                "GetResolverEndpoint",
                json!({ "ResolverEndpointId": id }),
            )
            .await;
            assert_eq!(
                string_member(endpoint(&fetched), "ResolverEndpointType"),
                from,
                "case {case:?}: a refused update must leave the stored type alone, got {fetched}"
            );
        }
    }
}

/// A delete is observable by the next caller: the endpoint is gone from the
/// store, so a get, a list and a second delete no longer find it.
#[tokio::test]
async fn a_deleted_endpoint_is_gone_for_the_next_caller() {
    let router = emulator();
    let ids = seed_endpoints(&router, 3).await;
    let doomed = ids[1].clone();

    let deleted = ok(
        &router,
        "DeleteResolverEndpoint",
        json!({ "ResolverEndpointId": doomed }),
    )
    .await;
    let deleted = endpoint(&deleted).clone();
    assert_eq!(
        string_member(&deleted, "Id"),
        doomed,
        "the delete must report the endpoint it took, got {deleted}"
    );
    assert_eq!(
        string_member(&deleted, "Status"),
        "DELETING",
        "an endpoint on its way out reports DELETING, got {deleted}"
    );

    let remaining = ok(&router, "ListResolverEndpoints", json!({})).await;
    let listed: Vec<String> = remaining
        .get("ResolverEndpoints")
        .and_then(Value::as_array)
        .map(|items| items.iter().map(|item| string_member(item, "Id")).collect())
        .unwrap_or_default();
    assert_eq!(
        listed,
        vec![ids[0].clone(), ids[2].clone()],
        "the list must no longer carry the deleted endpoint, got {remaining}"
    );

    for operation in [
        "GetResolverEndpoint",
        "DeleteResolverEndpoint",
        "ListResolverEndpointIpAddresses",
    ] {
        expect_error(
            &router,
            operation,
            json!({ "ResolverEndpointId": doomed }),
            "a deleted endpoint",
            "ResourceNotFoundException",
        )
        .await;
    }
}

/// An id that was never created is `ResourceNotFoundException` from every
/// operation that resolves one — never a 501 and never an empty success.
#[tokio::test]
async fn an_unknown_endpoint_id_is_not_found_everywhere() {
    let router = emulator();
    // One real endpoint exists, so "not found" is about the id and not about an
    // empty store.
    seed_endpoints(&router, 1).await;

    for id in [
        "rslvr-out-doesnotexist",
        "rslvr-in-0000000000000000",
        "not-an-endpoint-id",
    ] {
        for operation in [
            "GetResolverEndpoint",
            "DeleteResolverEndpoint",
            "ListResolverEndpointIpAddresses",
        ] {
            expect_error(
                &router,
                operation,
                json!({ "ResolverEndpointId": id }),
                id,
                "ResourceNotFoundException",
            )
            .await;
        }
        expect_error(
            &router,
            "UpdateResolverEndpoint",
            json!({ "ResolverEndpointId": id, "Name": "whatever" }),
            id,
            "ResourceNotFoundException",
        )
        .await;
    }
}

/// `OutpostArn` and `PreferredInstanceType` are documented as a pair: "If you
/// specify this, you must also specify a value for the PreferredInstanceType."
#[tokio::test]
async fn outpost_arn_and_preferred_instance_type_must_be_specified_together() {
    let router = emulator();
    let outpost_arn = "arn:aws:outposts:us-west-2:000000000000:outpost/op-0123456789abcdef0";

    for (case, extra) in [
        ("OutpostArn without PreferredInstanceType", json!({ "OutpostArn": outpost_arn })),
        (
            "PreferredInstanceType without OutpostArn",
            json!({ "PreferredInstanceType": "m5.large" }),
        ),
    ] {
        let mut body = create_body("req-outpost", Some("outpost"), "INBOUND", 2);
        for (key, value) in extra.as_object().expect("an object of extra members") {
            body[key] = value.clone();
        }
        expect_error(
            &router,
            "CreateResolverEndpoint",
            body,
            case,
            "InvalidRequestException",
        )
        .await;
    }

    // Both together is accepted, and both are stored.
    let mut body = create_body("req-outpost-both", Some("outpost-both"), "INBOUND", 2);
    body["OutpostArn"] = json!(outpost_arn);
    body["PreferredInstanceType"] = json!("m5.large");
    let created = ok(&router, "CreateResolverEndpoint", body).await;
    let created = endpoint(&created).clone();
    assert_eq!(
        string_member(&created, "OutpostArn"),
        outpost_arn,
        "the stored OutpostArn must come back, got {created}"
    );
    assert_eq!(
        string_member(&created, "PreferredInstanceType"),
        "m5.large",
        "the stored PreferredInstanceType must come back, got {created}"
    );
}

/// The filter work one request can ask for is bounded: `Filters` and
/// `Filter.Values` carry no modelled length, and the scan cost is the number of
/// stored endpoints multiplied by the filter terms, so a request past either cap
/// is refused with `InvalidParameterException` instead of being served.
#[tokio::test]
async fn the_filter_work_one_request_can_ask_for_is_bounded() {
    let router = emulator();
    seed_endpoints(&router, 2).await;

    let one_filter = json!({ "Name": "Direction", "Values": ["OUTBOUND"] });

    // Each row: how many filters, how many values in each, and whether it is served.
    let cases = [
        ("at the filter cap", MAX_FILTERS, 1, true),
        ("one filter past the cap", MAX_FILTERS + 1, 1, false),
        ("far past the filter cap", MAX_FILTERS * 10, 1, false),
        ("at the value cap", 1, MAX_FILTER_VALUES, true),
        ("one value past the cap", 1, MAX_FILTER_VALUES + 1, false),
        (
            "values spread across filters, past the cap in total",
            MAX_FILTERS,
            MAX_FILTER_VALUES,
            false,
        ),
    ];

    for (case, filter_count, value_count, served) in cases {
        let filters: Vec<Value> = (0..filter_count)
            .map(|_| {
                let mut filter = one_filter.clone();
                filter["Values"] = json!((0..value_count)
                    .map(|index| format!("value-{index}"))
                    .collect::<Vec<String>>());
                filter
            })
            .collect();
        let request = json!({ "Filters": filters });

        if served {
            let listed = ok(&router, "ListResolverEndpoints", request).await;
            assert!(
                listed.get("ResolverEndpoints").is_some(),
                "case {case:?}: a request within the caps must be served, got {listed}"
            );
        } else {
            expect_error(
                &router,
                "ListResolverEndpoints",
                request,
                case,
                "InvalidParameterException",
            )
            .await;
        }
    }
}

/// `SecurityGroupIds` carries no modelled length, and a stored endpoint's groups
/// are walked once per filtered scan, so the list is bounded: a create past the
/// cap is refused with `LimitExceededException` and stores nothing.
#[tokio::test]
async fn the_security_group_list_one_endpoint_can_hold_is_bounded() {
    let router = emulator();

    for (case, count, stored) in [
        ("one group", 1, true),
        ("at the cap", MAX_SECURITY_GROUP_IDS, true),
        ("one past the cap", MAX_SECURITY_GROUP_IDS + 1, false),
    ] {
        let groups: Vec<String> = (0..count).map(|index| format!("sg-{index}")).collect();
        let mut body = create_body(&format!("req-sg-{count}"), None, "OUTBOUND", 2);
        body["SecurityGroupIds"] = json!(groups);

        if stored {
            let created = ok(&router, "CreateResolverEndpoint", body).await;
            assert_eq!(
                endpoint(&created).get("SecurityGroupIds"),
                Some(&json!(groups)),
                "case {case:?}: the stored groups must come back, got {created}"
            );
        } else {
            expect_error(
                &router,
                "CreateResolverEndpoint",
                body,
                case,
                "LimitExceededException",
            )
            .await;
        }
    }
}

/// Walk every page and count the stored endpoints.
async fn count_endpoints(router: &MultiServiceRouter) -> usize {
    let mut total = 0;
    let mut next_token: Option<String> = None;
    loop {
        let mut request = json!({ "MaxResults": 100 });
        if let Some(token) = &next_token {
            request["NextToken"] = json!(token);
        }
        let page = ok(router, "ListResolverEndpoints", request).await;
        total += page
            .get("ResolverEndpoints")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        next_token = page
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if next_token.is_none() {
            return total;
        }
    }
}

/// The store is bounded: once it holds `MAX_STORED_ENDPOINTS` endpoints a create
/// of a NEW one is refused with the modelled `LimitExceededException` and writes
/// nothing, so a caller looping creates cannot grow the emulator without limit.
/// A retry of a `CreatorRequestId` already stored still succeeds (it allocates
/// no record), and deleting one frees a slot.
#[tokio::test]
async fn the_endpoint_store_is_bounded() {
    let router = emulator();
    for index in 0..MAX_STORED_ENDPOINTS {
        ok(
            &router,
            "CreateResolverEndpoint",
            create_body(&format!("req-fill-{index}"), None, "OUTBOUND", 2),
        )
        .await;
    }
    assert_eq!(
        count_endpoints(&router).await,
        MAX_STORED_ENDPOINTS,
        "the store must hold exactly the endpoints that were created"
    );

    expect_error(
        &router,
        "CreateResolverEndpoint",
        create_body("req-one-too-many", None, "OUTBOUND", 2),
        "a new endpoint past the cap",
        "LimitExceededException",
    )
    .await;
    assert_eq!(
        count_endpoints(&router).await,
        MAX_STORED_ENDPOINTS,
        "a refused create must not store anything"
    );

    // A retry of a request id the store already holds is not a new record, so the
    // bound does not refuse it.
    let retried = ok(
        &router,
        "CreateResolverEndpoint",
        create_body("req-fill-0", None, "OUTBOUND", 2),
    )
    .await;
    assert_eq!(
        endpoint(&retried).get("CreatorRequestId"),
        Some(&json!("req-fill-0")),
        "a retry at the cap must return the endpoint that request already made, got {retried}"
    );

    // Freeing a slot lets the next create through.
    let doomed = string_member(endpoint(&retried), "Id");
    ok(
        &router,
        "DeleteResolverEndpoint",
        json!({ "ResolverEndpointId": doomed }),
    )
    .await;
    ok(
        &router,
        "CreateResolverEndpoint",
        create_body("req-after-delete", None, "OUTBOUND", 2),
    )
    .await;
    assert_eq!(
        count_endpoints(&router).await,
        MAX_STORED_ENDPOINTS,
        "a delete frees exactly one slot, so the store is full again"
    );
}

/// Every Resolver endpoint operation that resolves a `ResolverEndpointId`
/// reports an id it holds no endpoint under with Route 53 Resolver's own
/// service error code inside the message: the code in square brackets, the
/// sentence naming the requested id in double quotes, and a terminating period.
///
/// The template is a rule over whatever id the request carried, so the table
/// varies the id along its form and its length: the model puts `ResourceId` at
/// length 1..64
/// (`smithy_query(query="shape",shape="ResourceId")`, no pattern), so both
/// published bounds are covered, as are an id of no Resolver form and one that
/// itself contains the quote character the template uses.
#[tokio::test]
async fn a_missing_endpoint_is_reported_with_its_rslvr_error_code() {
    /// The Route 53 Resolver service error code for "this Resolver endpoint id
    /// is not stored", as recorded on AWS for these operations.
    const CODE: &str = "RSLVR-00501";

    let router = emulator();
    // One real endpoint exists, so each case is about the id in the request and
    // not about an empty store.
    seed_endpoints(&router, 1).await;

    // Each row: what the case varies, and an id no endpoint is stored under.
    let cases: Vec<(&str, String)> = vec![
        (
            "an outbound id of the minted form",
            "rslvr-out-0123456789abcdef0".to_string(),
        ),
        (
            "an inbound id of the minted form",
            "rslvr-in-0000000000000000".to_string(),
        ),
        ("an id of no Resolver form", "not-an-endpoint-id".to_string()),
        ("the shortest ResourceId (length 1)", "r".to_string()),
        ("the longest ResourceId (length 64)", "e".repeat(64)),
        (
            "an id containing the quote character the template uses",
            "rslvr-out-\"quoted\"".to_string(),
        ),
        (
            "an id that differs from a stored one only by case",
            "RSLVR-OUT-0123456789ABCDEF0".to_string(),
        ),
    ];

    for (case, id) in &cases {
        // The operations that take a `ResolverEndpointId` and nothing else
        // required, plus the update, which needs a member to write.
        let requests = [
            ("GetResolverEndpoint", json!({ "ResolverEndpointId": id })),
            ("DeleteResolverEndpoint", json!({ "ResolverEndpointId": id })),
            (
                "ListResolverEndpointIpAddresses",
                json!({ "ResolverEndpointId": id }),
            ),
            (
                "UpdateResolverEndpoint",
                json!({ "ResolverEndpointId": id, "Name": "renamed" }),
            ),
        ];

        for (operation, body) in requests {
            let (status, parsed) = call(&router, operation, body).await;
            assert_eq!(
                status, 400,
                "case {case:?}/{operation}: expected HTTP 400, got {status}: {parsed}"
            );
            assert!(
                error_type(&parsed).contains("ResourceNotFoundException"),
                "case {case:?}/{operation}: expected ResourceNotFoundException, got {parsed}"
            );

            let message = string_member(&parsed, "Message");
            let expected = format!(r#"[{CODE}] Resolver endpoint with ID "{id}" does not exist."#);
            assert_eq!(
                message, expected,
                "case {case:?}/{operation}: expected Message {expected:?}, got {message:?}"
            );
            // The parts the rule is made of, asserted on their own so a future
            // reword cannot drop one of them unnoticed.
            assert!(
                message.starts_with(&format!("[{CODE}] ")),
                "case {case:?}/{operation}: expected the bracketed {CODE} prefix, got {message:?}"
            );
            assert!(
                message.contains(&format!("\"{id}\"")),
                "case {case:?}/{operation}: expected the requested id double-quoted, got {message:?}"
            );
            assert!(
                message.ends_with('.'),
                "case {case:?}/{operation}: expected a terminating period, got {message:?}"
            );
            assert_eq!(
                string_member(&parsed, "ResourceType"),
                "ResolverEndpoint",
                "case {case:?}/{operation}: expected the Resolver endpoint resource type, got {parsed}"
            );
        }
    }

    // The message is built from the request, not fixed text: two different ids
    // yield two different messages.
    let mut seen: Vec<String> = Vec::with_capacity(cases.len());
    for (case, id) in &cases {
        let (_, parsed) = call(
            &router,
            "GetResolverEndpoint",
            json!({ "ResolverEndpointId": id }),
        )
        .await;
        let message = string_member(&parsed, "Message");
        assert!(
            !seen.contains(&message),
            "case {case:?}: every distinct id must give a distinct Message, got {message:?} twice"
        );
        seen.push(message);
    }

    // An endpoint that IS stored is still served, so the not-found path is
    // keyed on the store and not on the shape of the id.
    let stored = seed_endpoints(&router, 1).await;
    let read = ok(
        &router,
        "GetResolverEndpoint",
        json!({ "ResolverEndpointId": stored[0] }),
    )
    .await;
    assert_eq!(
        string_member(endpoint(&read), "Id"),
        stored[0],
        "a stored endpoint must still be returned, got {read}"
    );
}
