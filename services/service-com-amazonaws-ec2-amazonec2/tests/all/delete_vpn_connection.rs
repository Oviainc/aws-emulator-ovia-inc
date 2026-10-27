// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeleteVpnConnection`, read back through `DescribeVpnConnections`
//! and through the `CreateVpnConnectionRoute` that resolves the same connection, driven over
//! the wire the service actually speaks: `ec2Query`, an
//! `application/x-www-form-urlencoded` POST whose body carries `Action=…`, answered with XML
//! (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rules under test:
//!
//! 1. A DELETE NAMES ONE CONNECTION OF THE ACCOUNT BY ID AND STARTS ITS TEARDOWN: the
//!    connection reaches the non-terminal `deleting` state — `VpnState` carries both
//!    `deleting` and `deleted`, so it keeps being reported while it goes away — and the
//!    `deleted` state falls due on the clock rather than at the next call. Repeating the
//!    request neither reports a second deletion nor postpones the first.
//! 2. AN ID THE ACCOUNT HOLDS NO CONNECTION FOR IS REFUSED, NOT ANSWERED AS A NO-OP: HTTP
//!    400 `InvalidVpnConnectionID.NotFound`, whose message names the id the request asked
//!    about, whatever form that id takes; an empty id is a missing parameter rather than a
//!    lookup that failed. Nothing is written either way.
//! 3. `DryRun` CHANGES NOTHING: a delete that would have succeeded answers
//!    `DryRunOperation` and leaves the connection exactly as it was, while a dry run naming
//!    an id nothing holds is still refused for the id.
//!
//! Each table row is a distinct input along what those rules generalise over: the `VpnState`
//! the connection is deleted in, how many static routes it carries, whether the delete is
//! repeated, and which form the refused id takes. The recorded case the issue was filed
//! about — a well-formed `vpn-` id the account holds no connection for — is ONE row of
//! [`delete_refuses_an_id_the_account_holds_no_connection_for_and_writes_nothing`].
//!
//! `CreateVpnConnection` is still a scaffold under its own issue, so the connection a row
//! deletes is seeded through the store, with the same helper the static route suite seeds
//! one with ([`super::vpn_connection_routes::seed`]).

use std::time::Duration;

use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, DRY_RUN_OPERATION, INCORRECT_STATE,
    INVALID_VPN_CONNECTION_ID_NOT_FOUND, MISSING_PARAMETER, TRANSITION_DELAY,
    VPN_STATE_AVAILABLE, VPN_STATE_DELETED, VPN_STATE_DELETING, VPN_STATE_PENDING,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, error_code, error_message};
use super::vpn_connection_routes::seed;
use super::wire::{child, child_text, document, param, Param};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// Long enough past [`TRANSITION_DELAY`] that a move the clock owes is certainly due.
fn past_the_transition() -> Duration {
    TRANSITION_DELAY + Duration::from_millis(250)
}

/// One `action` call carrying `params`, as its HTTP status and XML body.
async fn invoke(router: &MultiServiceRouter, action: &str, params: &[Param]) -> (u16, String) {
    call(router, REGION, action, params).await
}

/// One `DeleteVpnConnection` call naming `vpn_connection_id`, with any extra `params`.
async fn delete(
    router: &MultiServiceRouter,
    vpn_connection_id: &str,
    params: &[Param],
) -> (u16, String) {
    let mut fields = vec![param("VpnConnectionId", vpn_connection_id)];
    fields.extend(params.iter().cloned());
    invoke(router, "DeleteVpnConnection", &fields).await
}

/// `DeleteVpnConnection` requiring success, and the body it answered.
async fn delete_ok(case: &str, router: &MultiServiceRouter, vpn_connection_id: &str) -> String {
    let (status, body) = delete(router, vpn_connection_id, &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DeleteVpnConnection, got {status} with body \
         {body}"
    );
    assert_eq!(
        element(&body, "return").as_deref(),
        Some("true"),
        "case {case:?}: a mutation that reports no result of its own carries the envelope's \
         success flag, got {body}"
    );
    body
}

/// One `CreateVpnConnectionRoute` call adding `destination_cidr_block` to a connection.
async fn add_route(
    router: &MultiServiceRouter,
    vpn_connection_id: &str,
    destination_cidr_block: &str,
) -> (u16, String) {
    let fields = vec![
        param("VpnConnectionId", vpn_connection_id),
        param("DestinationCidrBlock", destination_cidr_block),
    ];
    invoke(router, "CreateVpnConnectionRoute", &fields).await
}

/// One connection as `DescribeVpnConnections` reports it.
#[derive(Debug, PartialEq, Eq)]
struct Described {
    /// The connection's `VpnState`.
    state: String,
    /// Whether the report carries the customer gateway configuration — "it's present in the
    /// `DescribeVpnConnections` response only if the VPN connection is in the `pending` or
    /// `available` state" (`VpnConnection$CustomerGatewayConfiguration`).
    has_configuration: bool,
    /// The destination block of each static route the connection carries, in report order.
    routes: Vec<String>,
}

/// The connection `vpn_connection_id` names, as one `DescribeVpnConnections` body reports
/// it — or `None` when that body reports no such connection.
fn reported(case: &str, body: &str, vpn_connection_id: &str) -> Option<Described> {
    let parsed = document(case, body);
    parsed
        .descendants()
        .filter(|node| node.is_element())
        .find(|node| child_text(*node, "vpnConnectionId").as_deref() == Some(vpn_connection_id))
        .map(|node| Described {
            state: child_text(node, "state").unwrap_or_default(),
            has_configuration: child_text(node, "customerGatewayConfiguration").is_some(),
            routes: child(node, "routes")
                .map(|routes| {
                    routes
                        .children()
                        .filter(|item| item.is_element() && item.tag_name().name() == "item")
                        .map(|item| child_text(item, "destinationCidrBlock").unwrap_or_default())
                        .collect()
                })
                .unwrap_or_default(),
        })
}

/// `DescribeVpnConnections` narrowed to one id, requiring the connection to still be
/// reported.
async fn describe_one(case: &str, router: &MultiServiceRouter, vpn_connection_id: &str) -> Described {
    let params = vec![param("VpnConnectionId.1", vpn_connection_id)];
    let (status, body) = invoke(router, "DescribeVpnConnections", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeVpnConnections, got {status} with body \
         {body}"
    );
    reported(case, &body, vpn_connection_id).unwrap_or_else(|| {
        panic!("case {case:?}: no {vpn_connection_id} in {body}")
    })
}

/// The `VpnState` the store holds for `vpn_connection_id` right now, read without going
/// through an operation — `None` when the store holds no such connection.
fn stored_state(engine: &engine::InMemoryEngine, vpn_connection_id: &str) -> Option<String> {
    engine
        .state::<Ec2State>()
        .lock()
        .vpn_connection(vpn_connection_id)
        .map(|record| record.state().to_string())
}

/// How many connections the store holds, so a refusal can be shown to have added none.
fn stored_connections(engine: &engine::InMemoryEngine) -> usize {
    engine.state::<Ec2State>().lock().vpn_connections.len()
}

// ---------------------------------------------------------------------------
// RULE 1: what a delete does to the connection it names
// ---------------------------------------------------------------------------

/// One row of the delete table: what makes it a different input.
struct DeleteCase {
    /// What makes this row a different input.
    case: &'static str,
    /// The `VpnState` the seeded connection is in when the delete reaches it.
    seeded: &'static str,
    /// The static routes the connection carries first, as `CreateVpnConnectionRoute` adds
    /// them.
    routes: &'static [&'static str],
    /// Whether the delete is asked for twice.
    twice: bool,
}

/// The rows: the state the connection is deleted in — one still coming up, one in service —
/// how many static routes it carries, and a delete repeated on a connection already going
/// away.
fn delete_cases() -> Vec<DeleteCase> {
    vec![
        DeleteCase {
            case: "a connection in service carrying no static route",
            seeded: VPN_STATE_AVAILABLE,
            routes: &[],
            twice: false,
        },
        DeleteCase {
            case: "a connection still coming up",
            seeded: VPN_STATE_PENDING,
            routes: &[],
            twice: false,
        },
        DeleteCase {
            case: "a connection carrying one static route",
            seeded: VPN_STATE_AVAILABLE,
            routes: &["192.168.10.0/24"],
            twice: false,
        },
        DeleteCase {
            case: "a connection carrying routes of both address families",
            seeded: VPN_STATE_AVAILABLE,
            routes: &["10.11.0.0/16", "2001:db8:abcd::/48"],
            twice: false,
        },
        DeleteCase {
            case: "a delete repeated on a connection already going away",
            seeded: VPN_STATE_AVAILABLE,
            routes: &["203.0.113.0/24"],
            twice: true,
        },
    ]
}

#[tokio::test]
async fn delete_vpn_connection_moves_the_connection_through_deleting_to_deleted() {
    for DeleteCase {
        case,
        seeded,
        routes,
        twice,
    } in delete_cases()
    {
        let (router, engine) = super::ec2_query::emulator();
        let vpn_connection_id = seed(&engine, seeded);
        for destination_cidr_block in routes {
            let (status, body) = add_route(&router, &vpn_connection_id, destination_cidr_block).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from CreateVpnConnectionRoute for \
                 {destination_cidr_block}, got {status} with body {body}"
            );
        }

        delete_ok(case, &router, &vpn_connection_id).await;
        // When the FIRST delete's clock-owed move falls due. A repeat must not push this
        // back, so the `deleted` state below is asserted at this instant rather than at one
        // measured from the last call.
        let deleted_due_at = tokio::time::Instant::now() + past_the_transition();

        // The connection is still reported, in the non-terminal state a delete leaves it in,
        // and it keeps every static route it was carrying.
        let expected = Described {
            state: VPN_STATE_DELETING.to_string(),
            has_configuration: false,
            routes: routes.iter().map(|block| (*block).to_string()).collect(),
        };
        let actual = describe_one(case, &router, &vpn_connection_id).await;
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the describe to report {expected:?} straight after the \
             delete, got {actual:?}"
        );

        if twice {
            delete_ok(case, &router, &vpn_connection_id).await;
            let actual = describe_one(case, &router, &vpn_connection_id).await;
            assert_eq!(
                actual.state, VPN_STATE_DELETING,
                "case {case:?}: a repeat must leave the deleting state alone, got {actual:?}"
            );
        }

        // Only as far as the FIRST delete's due instant: a repeat that had restarted the
        // clock would still report `deleting` here. Nothing but the clock has to happen for
        // the move to fall due.
        tokio::time::sleep_until(deleted_due_at).await;

        let actual = describe_one(case, &router, &vpn_connection_id).await;
        assert_eq!(
            actual.state, VPN_STATE_DELETED,
            "case {case:?}: expected the clock-owed deleted state, got {actual:?}"
        );
        let actual = stored_state(&engine, &vpn_connection_id);
        assert_eq!(
            actual.as_deref(),
            Some(VPN_STATE_DELETED),
            "case {case:?}: the store itself holds the connection in its terminal state, got \
             {actual:?}"
        );
    }
}

/// The delete's write is what every other operation that resolves the connection then sees:
/// a connection on its way out has nothing left for a static route to be programmed into, so
/// the route create that succeeded before the delete is refused after it.
#[tokio::test]
async fn a_connection_a_delete_is_taking_away_no_longer_admits_a_static_route() {
    let case = "a static route create across a delete";
    let (router, engine) = super::ec2_query::emulator();
    let vpn_connection_id = seed(&engine, VPN_STATE_AVAILABLE);

    let (status, body) = add_route(&router, &vpn_connection_id, "198.51.100.0/24").await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 before the delete, got {status} with body {body}"
    );

    delete_ok(case, &router, &vpn_connection_id).await;

    let (status, body) = add_route(&router, &vpn_connection_id, "198.51.100.0/24").await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400 after the delete, got {status} with body {body}"
    );
    let actual = error_code(&body);
    assert_eq!(
        actual, INCORRECT_STATE,
        "case {case:?}: expected the code {INCORRECT_STATE}, got {actual} in {body}"
    );
}

// ---------------------------------------------------------------------------
// RULE 2: the ids a delete refuses
// ---------------------------------------------------------------------------

/// One row of the refusal table: the id as the request sends it, and the EC2 code it must be
/// refused with.
struct RefusalCase {
    /// What makes this row a different input.
    case: &'static str,
    /// `VpnConnectionId` as the request sends it.
    value: &'static str,
    /// The `<Code>` EC2 answers.
    code: &'static str,
}

/// The rows: every form an id that resolves to no connection of the account can take, plus
/// the empty field, which names no connection at all.
fn refusal_cases() -> Vec<RefusalCase> {
    vec![
        // The id the recorded request named has this form: the long (17 hexadecimal digit)
        // id form, nothing holding it.
        RefusalCase {
            case: "a long-form id nothing holds",
            value: "vpn-0ffffffffffffffff",
            code: INVALID_VPN_CONNECTION_ID_NOT_FOUND,
        },
        RefusalCase {
            case: "a short-form id nothing holds",
            value: "vpn-0abcd123",
            code: INVALID_VPN_CONNECTION_ID_NOT_FOUND,
        },
        RefusalCase {
            case: "an id of another resource kind",
            value: "vgw-0123456789abcdef0",
            code: INVALID_VPN_CONNECTION_ID_NOT_FOUND,
        },
        RefusalCase {
            case: "not a resource id at all",
            value: "not-an-id",
            code: INVALID_VPN_CONNECTION_ID_NOT_FOUND,
        },
        RefusalCase {
            case: "an empty id",
            value: "",
            code: MISSING_PARAMETER,
        },
    ]
}

#[tokio::test]
async fn delete_refuses_an_id_the_account_holds_no_connection_for_and_writes_nothing() {
    let (router, engine) = super::ec2_query::emulator();
    // One stored connection, so a refusal below is about the id the request named rather
    // than about an empty store — and so the delete can be shown to have left it alone.
    let held = seed(&engine, VPN_STATE_AVAILABLE);
    let before = stored_connections(&engine);

    for RefusalCase { case, value, code } in refusal_cases() {
        let (status, body) = delete(&router, value, &[]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from DeleteVpnConnection, got {status} with \
             body {body}"
        );
        let actual = error_code(&body);
        assert_eq!(
            actual, code,
            "case {case:?}: expected the code {code}, got {actual} in {body}"
        );
        // The refusal says which id it is about, built from the request.
        let expected = if code == MISSING_PARAMETER {
            "The request must contain the parameter vpnConnectionId".to_string()
        } else {
            format!("The vpnConnection ID '{value}' does not exist")
        };
        let actual = error_message(&body);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the message {expected:?}, got {actual:?} in {body}"
        );

        // Nothing was written: the connection the account DOES hold is untouched and no
        // record was added for the id that resolved to nothing.
        let actual = stored_state(&engine, &held);
        assert_eq!(
            actual.as_deref(),
            Some(VPN_STATE_AVAILABLE),
            "case {case:?}: the held connection must be untouched, got {actual:?}"
        );
        let actual = stored_connections(&engine);
        assert_eq!(
            actual, before,
            "case {case:?}: expected the store to hold {before} connections, got {actual}"
        );
    }

    // The id that resolves is still deleted after all those refusals.
    delete_ok("the held connection", &router, &held).await;
    let actual = describe_one("the held connection", &router, &held).await;
    assert_eq!(
        actual.state, VPN_STATE_DELETING,
        "the held connection: expected the delete to start, got {actual:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE 3: DryRun changes nothing
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_dry_run_reports_the_outcome_and_changes_nothing() {
    let (router, engine) = super::ec2_query::emulator();
    let held = seed(&engine, VPN_STATE_AVAILABLE);
    let dry_run = [param("DryRun", "true")];

    // A delete that would have succeeded.
    let case = "a dry run of a delete that would have succeeded";
    let (status, body) = delete(&router, &held, &dry_run).await;
    assert_eq!(
        status, 412,
        "case {case:?}: expected HTTP 412, got {status} with body {body}"
    );
    let actual = error_code(&body);
    assert_eq!(
        actual, DRY_RUN_OPERATION,
        "case {case:?}: expected the code {DRY_RUN_OPERATION}, got {actual} in {body}"
    );
    let actual = stored_state(&engine, &held);
    assert_eq!(
        actual.as_deref(),
        Some(VPN_STATE_AVAILABLE),
        "case {case:?}: a dry run writes nothing, got {actual:?}"
    );

    // A dry run that names an id nothing holds is refused for the ID: the flag asks whether
    // the caller is permitted, not whether the request is about something that exists.
    let case = "a dry run naming an id nothing holds";
    let (status, body) = delete(&router, "vpn-0ffffffffffffffff", &dry_run).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with body {body}"
    );
    let actual = error_code(&body);
    assert_eq!(
        actual, INVALID_VPN_CONNECTION_ID_NOT_FOUND,
        "case {case:?}: expected the code {INVALID_VPN_CONNECTION_ID_NOT_FOUND}, got {actual} \
         in {body}"
    );

    // And the delete the dry run asked about still works.
    let case = "the delete the dry run asked about";
    delete_ok(case, &router, &held).await;
    let actual = describe_one(case, &router, &held).await;
    assert_eq!(
        actual.state, VPN_STATE_DELETING,
        "case {case:?}: expected the delete to start, got {actual:?}"
    );
}
