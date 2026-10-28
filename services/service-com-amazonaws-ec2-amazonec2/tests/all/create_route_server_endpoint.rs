// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `CreateRouteServerEndpoint`: the resource ids it resolves before it
//! writes anything, the `ClientToken` that replays one create instead of storing a second
//! endpoint, and the `DryRun` probe that performs nothing — driven over the wire the service
//! actually speaks: `ec2Query`, an `application/x-www-form-urlencoded` POST whose body
//! carries `Action=CreateRouteServerEndpoint`, answered with XML (README.md /
//! OPERATIONS.md "Wire protocol"). ec2Query is selected by the `ec2` signing name in the
//! request's SigV4 credential scope, so every call here carries one.
//!
//! The rule under test: a create RESOLVES EVERY RESOURCE ID ITS REQUEST NAMES against the
//! store before it writes — an id the account holds no such resource for is refused
//! `Invalid<Resource>Id.NotFound` and, where the service tells the two apart (the route
//! server reference does, through this crate's one route server lookup), a value that is not
//! an id of that kind at all is refused `Invalid<Resource>Id.Malformed` first; both HTTP 400,
//! naming the resource and the id the request sent, and a refused create leaves no record
//! behind;
//! a create whose references all resolve stores exactly one endpoint, answers it as it was
//! stored (so a later describe reports the same members), and answers a repeated
//! idempotency token with that same endpoint rather than a second one.
//!
//! Every table row is a distinct input along what the rule generalises over — which of the
//! two referenced resources the unresolvable id sits in, which id form it takes (the short
//! and the long EC2 form, a non-hex suffix, another resource kind's prefix, a length that is
//! neither form, a value carrying characters that are significant in XML), and, for the token
//! rows, how long the token is (one character, a uuid-shaped one, the longest this emulator
//! accepts, one past it) — so the single case a recording happens to contain is one row among
//! many rather than the specification.

use std::collections::BTreeMap;

use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, error_message};
use super::wire::{Param, child_text, document, is_long_id, param};

/// The error codes EC2 answers these refusals with, spelled as they go over the wire. Written
/// out here rather than read from the service's own constants, so a code this emulator
/// misspells cannot pass this suite.
const CODE_ROUTE_SERVER_NOT_FOUND: &str = "InvalidRouteServerId.NotFound";
const CODE_ROUTE_SERVER_MALFORMED: &str = "InvalidRouteServerId.Malformed";
const CODE_SUBNET_NOT_FOUND: &str = "InvalidSubnetID.NotFound";
const CODE_DRY_RUN_OPERATION: &str = "DryRunOperation";
const CODE_INVALID_PARAMETER_VALUE: &str = "InvalidParameterValue";

/// The non-terminal member of `RouteServerEndpointState` a create answers in
/// (`smithy_query(query="shape",shape="RouteServerEndpointState")`).
const STATE_PENDING: &str = "pending";

/// The longest idempotency token this service accepts, in characters.
const MAX_CLIENT_TOKEN_CHARS: usize = 128;

/// The Region every call here is signed for: the default VPC's subnets — the subnets an
/// endpoint is placed into below — are seeded for the emulator's default Region.
const REGION: &str = "us-east-1";

/// The emulator the tests drive, as [`super::ec2_query::emulator`] builds it. The store is
/// reached only through the wire here, so the engine handle is not needed.
fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// One call of `action` carrying `params`, as its HTTP status and XML body.
async fn call(router: &MultiServiceRouter, action: &str, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, action, params).await
}

/// One call of `action` requiring success, and the body it answered.
async fn call_ok(case: &str, router: &MultiServiceRouter, action: &str, params: &[Param]) -> String {
    let (status, body) = call(router, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// One subnet of the account, read over the wire rather than out of the store.
async fn a_subnet(router: &MultiServiceRouter) -> String {
    let body = call_ok("the account's subnets", router, "DescribeSubnets", &[]).await;
    let subnets = elements(&body, "subnetId");
    subnets
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("the seeded account must hold a subnet, got body {body}"))
}

/// A route server with `amazon_side_asn`, and the id it was minted with.
async fn a_route_server(router: &MultiServiceRouter, amazon_side_asn: &str) -> String {
    let body = call_ok(
        "the route server the endpoints hang off",
        router,
        "CreateRouteServer",
        &[param("AmazonSideAsn", amazon_side_asn)],
    )
    .await;
    element(&body, "routeServerId")
        .unwrap_or_else(|| panic!("no routeServerId in {body}"))
}

/// The form fields of one `CreateRouteServerEndpoint` request: the two references it must
/// resolve, plus whatever `extra` the row adds.
fn create_params(route_server_id: &str, subnet_id: &str, extra: &[Param]) -> Vec<Param> {
    let mut params = vec![
        param("RouteServerId", route_server_id),
        param("SubnetId", subnet_id),
    ];
    params.extend(extra.iter().cloned());
    params
}

/// One `CreateRouteServerEndpoint` call carrying `params`.
async fn create(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    call(router, "CreateRouteServerEndpoint", params).await
}

/// Every route server endpoint the store holds, in the order the describe reports them.
async fn stored_ids(case: &str, router: &MultiServiceRouter) -> Vec<String> {
    let body = call_ok(case, router, "DescribeRouteServerEndpoints", &[]).await;
    elements(&body, "routeServerEndpointId")
}

/// The members of `RouteServerEndpoint` every stored endpoint reports, in the ec2Query
/// spelling the wire uses (`smithy_query(query="shape",shape="RouteServerEndpoint")`).
/// `failureReason` is not among them: nothing in this emulator fails an endpoint. `state` is
/// not either — it is the one member the clock moves, so a create and a later describe are
/// allowed to disagree on it and on nothing else.
const ENDPOINT_MEMBERS: [&str; 6] = [
    "routeServerEndpointId",
    "routeServerId",
    "vpcId",
    "subnetId",
    "eniId",
    "eniAddress",
];

/// The scalar members a response reports for the endpoint `id`, read through the XML tree so
/// a member of the endpoint is never confused with one of the same name elsewhere in the
/// envelope. Every member of [`ENDPOINT_MEMBERS`] must be among them, so a comparison of two
/// of these maps cannot pass by both sides reporting nothing.
fn endpoint_fields(case: &str, body: &str, id: &str) -> BTreeMap<String, String> {
    let parsed = document(case, body);
    let endpoint = parsed
        .descendants()
        .find(|node| {
            node.is_element() && child_text(*node, "routeServerEndpointId").as_deref() == Some(id)
        })
        .unwrap_or_else(|| panic!("case {case:?}: no route server endpoint {id} in {body}"));
    let fields: BTreeMap<String, String> = endpoint
        .children()
        .filter(|node| node.is_element() && node.tag_name().name() != "state")
        .filter_map(|node| {
            if node.children().any(|inner| inner.is_element()) {
                return None;
            }
            let text = node.text()?;
            Some((node.tag_name().name().to_string(), text.to_string()))
        })
        .collect();
    for member in ENDPOINT_MEMBERS {
        assert!(
            fields.contains_key(member),
            "case {case:?}: expected the endpoint {id} to report {member}, got {fields:?}"
        );
    }
    fields
}

/// The `state` a response reports for the endpoint `id`.
fn endpoint_state(case: &str, body: &str, id: &str) -> String {
    let parsed = document(case, body);
    let endpoint = parsed
        .descendants()
        .find(|node| {
            node.is_element() && child_text(*node, "routeServerEndpointId").as_deref() == Some(id)
        })
        .unwrap_or_else(|| panic!("case {case:?}: no route server endpoint {id} in {body}"));
    child_text(endpoint, "state")
        .unwrap_or_else(|| panic!("case {case:?}: no <state> on the endpoint {id} in {body}"))
}

/// One row of the unresolvable-reference table: the two ids the request names, and the
/// refusal the service owes it.
struct RefusalRow {
    case: &'static str,
    route_server_id: String,
    subnet_id: String,
    code: &'static str,
    message: String,
}

// ---------------------------------------------------------------------------
// The references a create must resolve
// ---------------------------------------------------------------------------

/// The rule: every resource id the request names is resolved against the store BEFORE the
/// create writes — malformed before unknown, the route server before the subnet it is placed
/// in — and a refused create stores nothing.
///
/// The rows differ in which reference carries the unresolvable id and in the form that id
/// takes: the long and the short EC2 id form of an id nothing was created for, a suffix that
/// is not hexadecimal, another resource kind's prefix, a length that is neither id form, a
/// value carrying characters that are significant in XML, and a request in which BOTH
/// references are unresolvable.
#[tokio::test]
async fn create_route_server_endpoint_resolves_the_ids_the_request_names() {
    let router = emulator();
    let subnet = a_subnet(&router).await;
    let route_server = a_route_server(&router, "64512").await;

    let rows: Vec<RefusalRow> = vec![
        RefusalRow {
            case: "a long-form route server id nothing was created for",
            route_server_id: "rs-0123456789abcdef0".to_string(),
            subnet_id: subnet.clone(),
            code: CODE_ROUTE_SERVER_NOT_FOUND,
            message: "The route-server ID 'rs-0123456789abcdef0' does not exist".to_string(),
        },
        RefusalRow {
            case: "the short route server id form, unknown",
            route_server_id: "rs-1a2b3c4d".to_string(),
            subnet_id: subnet.clone(),
            code: CODE_ROUTE_SERVER_NOT_FOUND,
            message: "The route-server ID 'rs-1a2b3c4d' does not exist".to_string(),
        },
        RefusalRow {
            case: "a route server id with a non-hex suffix",
            route_server_id: "rs-zzzzzzzz".to_string(),
            subnet_id: subnet.clone(),
            code: CODE_ROUTE_SERVER_MALFORMED,
            message: "Invalid id: 'rs-zzzzzzzz'".to_string(),
        },
        RefusalRow {
            case: "another resource kind's id in the route server member",
            route_server_id: "vpc-00000000000000001".to_string(),
            subnet_id: subnet.clone(),
            code: CODE_ROUTE_SERVER_MALFORMED,
            message: "Invalid id: 'vpc-00000000000000001'".to_string(),
        },
        RefusalRow {
            case: "a route server id of neither published length",
            route_server_id: "rs-0123456789abcde".to_string(),
            subnet_id: subnet.clone(),
            code: CODE_ROUTE_SERVER_MALFORMED,
            message: "Invalid id: 'rs-0123456789abcde'".to_string(),
        },
        RefusalRow {
            // The id the request sent is reported back inside an XML envelope, so a value
            // carrying characters that are significant in XML is one of the forms the rule
            // has to hold for.
            case: "a route server id carrying XML metacharacters",
            route_server_id: "rs-<&'\"zz".to_string(),
            subnet_id: subnet.clone(),
            code: CODE_ROUTE_SERVER_MALFORMED,
            message: "Invalid id: 'rs-<&'\"zz'".to_string(),
        },
        RefusalRow {
            case: "a long-form subnet id nothing was created for",
            route_server_id: route_server.clone(),
            subnet_id: "subnet-0123456789abcdef0".to_string(),
            code: CODE_SUBNET_NOT_FOUND,
            message: "The subnet ID 'subnet-0123456789abcdef0' does not exist".to_string(),
        },
        RefusalRow {
            case: "the short subnet id form, unknown",
            route_server_id: route_server.clone(),
            subnet_id: "subnet-1a2b3c4d".to_string(),
            code: CODE_SUBNET_NOT_FOUND,
            message: "The subnet ID 'subnet-1a2b3c4d' does not exist".to_string(),
        },
        RefusalRow {
            case: "both references unresolvable: the route server is resolved first",
            route_server_id: "rs-0123456789abcdef0".to_string(),
            subnet_id: "subnet-0123456789abcdef0".to_string(),
            code: CODE_ROUTE_SERVER_NOT_FOUND,
            message: "The route-server ID 'rs-0123456789abcdef0' does not exist".to_string(),
        },
    ];

    for RefusalRow {
        case,
        route_server_id,
        subnet_id,
        code,
        message,
    } in &rows
    {
        let (status, body) = create(
            &router,
            &create_params(route_server_id, subnet_id, &[]),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        let actual_code = error_code(&body);
        assert_eq!(
            &actual_code, code,
            "case {case:?}: expected {code}, got {actual_code:?} with body {body}"
        );
        let actual_message = error_message(&body);
        assert_eq!(
            &actual_message, message,
            "case {case:?}: expected the message {message:?}, got {actual_message:?}"
        );
        // Whatever the id carried, the refusal is ONE well-formed XML envelope: the value is
        // escaped on the way out and reads back unescaped above.
        let parsed = document(case, &body);
        assert!(
            parsed.root_element().has_children(),
            "case {case:?}: expected a well-formed XML envelope, got {body}"
        );
    }

    // Not one of the refusals above wrote an endpoint.
    let stored = stored_ids("after the refusals", &router).await;
    assert!(
        stored.is_empty(),
        "case \"after the refusals\": expected no stored endpoint, got {stored:?}"
    );

    // And the refusals did not break the create that CAN resolve both of its references:
    // the endpoint it stores reads back through the describe exactly as it was answered.
    let case = "both references resolve";
    let created = call_ok(
        case,
        &router,
        "CreateRouteServerEndpoint",
        &create_params(&route_server, &subnet, &[]),
    )
    .await;
    let id = element(&created, "routeServerEndpointId")
        .unwrap_or_else(|| panic!("case {case:?}: no routeServerEndpointId in {created}"));
    assert!(
        is_long_id("rse", &id),
        "case {case:?}: expected a long route server endpoint id, got {id:?}"
    );
    let created_state = endpoint_state(case, &created, &id);
    assert_eq!(
        created_state, STATE_PENDING,
        "case {case:?}: expected a created endpoint in {STATE_PENDING}, got {created_state:?}"
    );
    let fields = endpoint_fields(case, &created, &id);
    assert_eq!(
        fields.get("routeServerId").map(String::as_str),
        Some(route_server.as_str()),
        "case {case:?}: expected the endpoint on {route_server}, got {fields:?}"
    );
    assert_eq!(
        fields.get("subnetId").map(String::as_str),
        Some(subnet.as_str()),
        "case {case:?}: expected the endpoint in {subnet}, got {fields:?}"
    );
    let describe_body = call_ok(case, &router, "DescribeRouteServerEndpoints", &[]).await;
    assert_eq!(
        elements(&describe_body, "routeServerEndpointId"),
        vec![id.clone()],
        "case {case:?}: expected only {id} stored, got body {describe_body}"
    );
    let reported = endpoint_fields(case, &describe_body, &id);
    assert_eq!(
        reported, fields,
        "case {case:?}: expected the stored members {fields:?}, got {reported:?}"
    );
}

// ---------------------------------------------------------------------------
// The idempotency token, and the dry run
// ---------------------------------------------------------------------------

/// The rule: a `DryRun` probe from a caller holding the action's permissions is answered
/// `DryRunOperation` (HTTP 412) and performs nothing — unless the request would have been
/// refused anyway, in which case it reports THAT refusal — and a repeated `ClientToken`
/// answers the endpoint the first call stored rather than storing a second one, while a
/// token longer than this emulator accepts is refused.
///
/// The token rows differ in how long the token is and which characters it uses: one
/// character, a uuid-shaped token, the longest this emulator accepts, and one character past
/// it.
#[tokio::test]
async fn create_route_server_endpoint_replays_a_client_token_and_performs_no_dry_run() {
    let router = emulator();
    let subnet = a_subnet(&router).await;
    let route_server = a_route_server(&router, "4200000000").await;

    let case = "a dry run of a request that would have succeeded";
    let (status, body) = create(
        &router,
        &create_params(&route_server, &subnet, &[param("DryRun", "true")]),
    )
    .await;
    assert_eq!(
        status, 412,
        "case {case:?}: expected HTTP 412, got {status} with body {body}"
    );
    let code = error_code(&body);
    assert_eq!(
        code, CODE_DRY_RUN_OPERATION,
        "case {case:?}: expected {CODE_DRY_RUN_OPERATION}, got {code:?} with body {body}"
    );

    let case = "a dry run of a request that would have been refused";
    let (status, body) = create(
        &router,
        &create_params("rs-0123456789abcdef0", &subnet, &[param("DryRun", "true")]),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with body {body}"
    );
    let code = error_code(&body);
    assert_eq!(
        code, CODE_ROUTE_SERVER_NOT_FOUND,
        "case {case:?}: expected {CODE_ROUTE_SERVER_NOT_FOUND}, got {code:?} with body {body}"
    );

    let stored = stored_ids("after the dry runs", &router).await;
    assert!(
        stored.is_empty(),
        "case \"after the dry runs\": expected no stored endpoint, got {stored:?}"
    );

    // One token per row, each sent twice: the second call is a replay of the first create,
    // never a second endpoint.
    let longest = "t".repeat(MAX_CLIENT_TOKEN_CHARS);
    let tokens: Vec<(&str, String)> = vec![
        ("a one-character token", "a".to_string()),
        (
            "a uuid-shaped token",
            "6f0c3d2e-7a1b-4c5d-8e9f-0123456789ab".to_string(),
        ),
        (
            "a token as long as this emulator accepts",
            longest.clone(),
        ),
    ];

    let mut minted: Vec<(&str, String, BTreeMap<String, String>)> = Vec::new();
    for (case, token) in &tokens {
        let params = create_params(&route_server, &subnet, &[param("ClientToken", token)]);
        let created = call_ok(case, &router, "CreateRouteServerEndpoint", &params).await;
        let id = element(&created, "routeServerEndpointId")
            .unwrap_or_else(|| panic!("case {case:?}: no routeServerEndpointId in {created}"));
        let fields = endpoint_fields(case, &created, &id);

        let replayed = call_ok(case, &router, "CreateRouteServerEndpoint", &params).await;
        let replayed_id = element(&replayed, "routeServerEndpointId")
            .unwrap_or_else(|| panic!("case {case:?}: no routeServerEndpointId in {replayed}"));
        assert_eq!(
            replayed_id, id,
            "case {case:?}: expected the replay to answer {id}, got {replayed_id:?}"
        );
        let replayed_fields = endpoint_fields(case, &replayed, &id);
        assert_eq!(
            replayed_fields, fields,
            "case {case:?}: expected the replay to answer the stored members {fields:?}, \
             got {replayed_fields:?}"
        );
        minted.push((case, id, fields));
    }

    // A distinct token is a distinct endpoint, and the store holds exactly those: one per
    // token, whatever the token looked like, and none for the replays.
    let mut expected_ids: Vec<String> = minted.iter().map(|(_, id, _)| id.clone()).collect();
    expected_ids.sort();
    let stored = stored_ids("after the tokens", &router).await;
    assert_eq!(
        stored, expected_ids,
        "case \"after the tokens\": expected one endpoint per token {expected_ids:?}, \
         got {stored:?}"
    );

    let describe_body = call_ok(
        "every stored endpoint",
        &router,
        "DescribeRouteServerEndpoints",
        &[],
    )
    .await;
    for (case, id, fields) in &minted {
        let reported = endpoint_fields(case, &describe_body, id);
        assert_eq!(
            &reported, fields,
            "case {case:?}: expected the describe to report the stored members {fields:?}, \
             got {reported:?}"
        );
    }

    // One character past the bound is refused, and that refusal writes nothing either.
    let case = "a token one character past the longest this emulator accepts";
    let too_long = "t".repeat(MAX_CLIENT_TOKEN_CHARS + 1);
    let (status, body) = create(
        &router,
        &create_params(&route_server, &subnet, &[param("ClientToken", &too_long)]),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with body {body}"
    );
    let code = error_code(&body);
    assert_eq!(
        code, CODE_INVALID_PARAMETER_VALUE,
        "case {case:?}: expected {CODE_INVALID_PARAMETER_VALUE}, got {code:?} with body {body}"
    );
    // The bound EC2 publishes for an idempotency token is 128 characters, which is what this
    // crate's `MAX_CLIENT_TOKEN_CHARS` holds: the wording is pinned literally here and the
    // bound itself by the rows above (a token OF that length is accepted, one past it is not).
    let expected_message = "clientToken accepts at most 128 characters";
    let actual_message = error_message(&body);
    assert_eq!(
        actual_message, expected_message,
        "case {case:?}: expected the message {expected_message:?}, got {actual_message:?}"
    );
    let stored = stored_ids("after the over-long token", &router).await;
    assert_eq!(
        stored, expected_ids,
        "case {case:?}: expected the store unchanged at {expected_ids:?}, got {stored:?}"
    );
}
