// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeleteVpnConcentrator`, driven over the wire the service actually
//! speaks: `ec2Query`, an `application/x-www-form-urlencoded` POST whose body carries
//! `Action=…`, answered with XML (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rules under test:
//!
//! 1. A DELETE NAMES ONE CONCENTRATOR OF THE ACCOUNT BY ID AND TAKES IT AWAY: the call
//!    answers the envelope's success flag and the record leaves the store, so the id names
//!    nothing afterwards — a repeat of the very same request is refused for that id, and
//!    every other concentrator the account holds is left exactly as it was.
//! 2. AN ID THE ACCOUNT HOLDS NO CONCENTRATOR FOR IS REFUSED, NOT ANSWERED AS A NO-OP:
//!    HTTP 400 `InvalidVpnConcentratorID.NotFound`, whose message names the id the request
//!    asked about, whatever form that id takes; an empty id is a missing parameter rather
//!    than a lookup that failed. Nothing is written either way.
//! 3. `DryRun` CHANGES NOTHING: a delete that would have succeeded answers
//!    `DryRunOperation` and leaves the concentrator stored, while a dry run naming an id
//!    nothing holds is still refused for the id.
//!
//! Each table row is a distinct input along what those rules generalise over: the state the
//! concentrator is in, whether it is attached to a transit gateway, how many tags it
//! carries, how many other concentrators the account holds beside it, and which form the
//! refused id takes. The recorded case the issue was filed about — a well-formed `vcn-` id
//! the account holds no concentrator for — is ONE row of
//! [`delete_refuses_an_id_the_account_holds_no_concentrator_for_and_writes_nothing`].
//!
//! `CreateVpnConcentrator` is still a scaffold under its own issue and so is
//! `DescribeVpnConcentrators`, so the concentrator a row deletes is seeded through the
//! store the way the VPN connection suite seeds a connection
//! ([`super::vpn_connection_routes::seed`]), and what the delete took away is read back
//! both through the API — the repeat delete's refusal for the same id — and off the store
//! itself.

use ec2::ops::types::{Tag, VPN_CONCENTRATOR_TYPE_VALUES, VpnConcentrator};
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, DRY_RUN_OPERATION,
    INVALID_VPN_CONCENTRATOR_ID_NOT_FOUND, MISSING_PARAMETER, VPN_STATE_AVAILABLE,
    VPN_STATE_PENDING,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, error_code, error_message};
use super::wire::{Param, param};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// `VpnConcentrator$State` is "The current state of the VPN concentrator" and is modelled
/// as a plain string (`smithy_query(query="shape",shape="VpnConcentrator")`), so a seeded
/// concentrator is given the states this crate already spells for a VPN resource: the
/// `VpnState` members [`VPN_STATE_AVAILABLE`] and [`VPN_STATE_PENDING`].
///
/// The only `VpnConcentratorType` the model publishes
/// (`smithy_query(query="shape",shape="VpnConcentratorType")`, as the generated
/// [`VPN_CONCENTRATOR_TYPE_VALUES`] catalog spells it).
fn concentrator_type() -> &'static str {
    VPN_CONCENTRATOR_TYPE_VALUES[0]
}

/// One seeded concentrator: the members a concentrator of a transit gateway carries.
fn concentrator(
    state: &str,
    transit_gateway_id: Option<&str>,
    tags: &[(&str, &str)],
) -> VpnConcentrator {
    VpnConcentrator {
        state: Some(state.to_string()),
        transit_gateway_id: transit_gateway_id.map(str::to_string),
        // The attachment exists only where the concentrator is attached to a gateway, so
        // the two members are seeded together.
        transit_gateway_attachment_id: transit_gateway_id
            .map(|_| "tgw-attach-0123456789abcdef0".to_string()),
        r#type: Some(concentrator_type().to_string()),
        tags: Some(
            tags.iter()
                .map(|(key, value)| Tag {
                    key: Some((*key).to_string()),
                    value: Some((*value).to_string()),
                })
                .collect(),
        ),
        ..Default::default()
    }
}

/// Seed one concentrator and answer the `VpnConcentratorId` the store minted for it.
fn seed(
    engine: &engine::InMemoryEngine,
    state: &str,
    transit_gateway_id: Option<&str>,
    tags: &[(&str, &str)],
) -> String {
    engine
        .state::<Ec2State>()
        .lock()
        .insert_vpn_concentrator(concentrator(state, transit_gateway_id, tags))
        .expect("the store holds the seeded VPN concentrator")
}

/// One `action` call carrying `params`, as its HTTP status and XML body.
async fn invoke(router: &MultiServiceRouter, action: &str, params: &[Param]) -> (u16, String) {
    call(router, REGION, action, params).await
}

/// One `DeleteVpnConcentrator` call naming `vpn_concentrator_id`, with any extra `params`.
async fn delete(
    router: &MultiServiceRouter,
    vpn_concentrator_id: &str,
    params: &[Param],
) -> (u16, String) {
    let mut fields = vec![param("VpnConcentratorId", vpn_concentrator_id)];
    fields.extend(params.iter().cloned());
    invoke(router, "DeleteVpnConcentrator", &fields).await
}

/// `DeleteVpnConcentrator` requiring success, and the body it answered.
async fn delete_ok(case: &str, router: &MultiServiceRouter, vpn_concentrator_id: &str) -> String {
    let (status, body) = delete(router, vpn_concentrator_id, &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DeleteVpnConcentrator, got {status} with body \
         {body}"
    );
    assert_eq!(
        element(&body, "return").as_deref(),
        Some("true"),
        "case {case:?}: a delete that succeeded reports the success flag, got {body}"
    );
    body
}

/// The refusal a delete of `vpn_concentrator_id` answers now — its code and its message —
/// which is how the API itself reports that the id names nothing.
async fn refusal(
    case: &str,
    router: &MultiServiceRouter,
    vpn_concentrator_id: &str,
    params: &[Param],
) -> (String, String) {
    let (status, body) = delete(router, vpn_concentrator_id, params).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400 from DeleteVpnConcentrator, got {status} with body \
         {body}"
    );
    (error_code(&body), error_message(&body))
}

/// The state the store holds for `vpn_concentrator_id` right now, read without going
/// through an operation — `None` when the store holds no such concentrator.
fn stored_state(engine: &engine::InMemoryEngine, vpn_concentrator_id: &str) -> Option<String> {
    engine
        .state::<Ec2State>()
        .lock()
        .vpn_concentrator(vpn_concentrator_id)
        .map(|view| view.state.clone().unwrap_or_default())
}

/// How many concentrators the store holds, so a refusal can be shown to have removed none.
fn stored_concentrators(engine: &engine::InMemoryEngine) -> usize {
    engine.state::<Ec2State>().lock().vpn_concentrators.len()
}

// ---------------------------------------------------------------------------
// RULE 1: what a delete does to the concentrator it names
// ---------------------------------------------------------------------------

/// One row of the delete table: what makes it a different input.
struct DeleteCase {
    /// What makes this row a different input.
    case: &'static str,
    /// The state the seeded concentrator is in when the delete reaches it.
    state: &'static str,
    /// The transit gateway it is attached to, where it is attached to one.
    transit_gateway_id: Option<&'static str>,
    /// The tags it carries.
    tags: &'static [(&'static str, &'static str)],
    /// How many FURTHER concentrators the account holds, which the delete must leave alone.
    others: usize,
}

/// The rows: the state the concentrator is deleted in, whether it is attached to a transit
/// gateway, how many tags it carries, and how many other concentrators the account holds.
fn delete_cases() -> Vec<DeleteCase> {
    vec![
        DeleteCase {
            case: "a concentrator in service attached to a transit gateway",
            state: VPN_STATE_AVAILABLE,
            transit_gateway_id: Some("tgw-0123456789abcdef0"),
            tags: &[("Name", "head-office")],
            others: 0,
        },
        DeleteCase {
            case: "a concentrator still coming up, attached to nothing",
            state: VPN_STATE_PENDING,
            transit_gateway_id: None,
            tags: &[],
            others: 0,
        },
        DeleteCase {
            case: "a concentrator carrying several tags",
            state: VPN_STATE_AVAILABLE,
            transit_gateway_id: Some("tgw-00112233445566778"),
            tags: &[("Name", "branch"), ("team", "network"), ("cost-centre", "42")],
            others: 2,
        },
        DeleteCase {
            case: "one of several concentrators the account holds",
            state: VPN_STATE_AVAILABLE,
            transit_gateway_id: None,
            tags: &[],
            others: 3,
        },
    ]
}

#[tokio::test]
async fn delete_vpn_concentrator_takes_the_concentrator_it_names_away() {
    for DeleteCase {
        case,
        state,
        transit_gateway_id,
        tags,
        others,
    } in delete_cases()
    {
        let (router, engine) = super::ec2_query::emulator();
        let vpn_concentrator_id = seed(&engine, state, transit_gateway_id, tags);
        // The concentrators the request says nothing about, which the delete must not touch.
        let untouched: Vec<String> = (0..others)
            .map(|_| seed(&engine, VPN_STATE_AVAILABLE, None, &[]))
            .collect();

        delete_ok(case, &router, &vpn_concentrator_id).await;

        // The record is gone from the store itself, not merely hidden from one reader.
        let actual = stored_state(&engine, &vpn_concentrator_id);
        assert_eq!(
            actual, None,
            "case {case:?}: expected the delete to take the concentrator out of the store, \
             got {actual:?}"
        );
        let actual = stored_concentrators(&engine);
        assert_eq!(
            actual, others,
            "case {case:?}: expected the store to hold the {others} concentrators the delete \
             said nothing about, got {actual}"
        );
        for other in &untouched {
            let actual = stored_state(&engine, other);
            assert_eq!(
                actual.as_deref(),
                Some(VPN_STATE_AVAILABLE),
                "case {case:?}: expected {other} to be untouched, got {actual:?}"
            );
        }

        // And the API itself reports that the id now names nothing: the very same request,
        // repeated, is refused for that id rather than answered as a second deletion.
        let expected = format!("The vpn-concentrator ID '{vpn_concentrator_id}' does not exist");
        let (code, message) = refusal(case, &router, &vpn_concentrator_id, &[]).await;
        assert_eq!(
            code, INVALID_VPN_CONCENTRATOR_ID_NOT_FOUND,
            "case {case:?}: expected the repeat to answer {INVALID_VPN_CONCENTRATOR_ID_NOT_FOUND}, \
             got {code}"
        );
        assert_eq!(
            message, expected,
            "case {case:?}: expected the message {expected:?}, got {message:?}"
        );

        // The others are still deletable afterwards: nothing about the first delete took
        // the id space with it.
        for other in &untouched {
            delete_ok(case, &router, other).await;
        }
        let actual = stored_concentrators(&engine);
        assert_eq!(
            actual, 0,
            "case {case:?}: expected an empty store once every concentrator was deleted, got \
             {actual}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 2: the ids a delete refuses
// ---------------------------------------------------------------------------

/// One row of the refusal table: the id as the request sends it, and the EC2 code it must
/// be refused with.
struct RefusalCase {
    /// What makes this row a different input.
    case: &'static str,
    /// `VpnConcentratorId` as the request sends it.
    value: &'static str,
    /// The `<Code>` EC2 answers.
    code: &'static str,
}

/// The rows: every form an id that resolves to no concentrator of the account can take,
/// plus the empty field, which names no concentrator at all.
fn refusal_cases() -> Vec<RefusalCase> {
    vec![
        // The id the recorded request named has this form: the long (17 hexadecimal digit)
        // id form, nothing holding it.
        RefusalCase {
            case: "a long-form id nothing holds",
            value: "vcn-0ffffffffffffffff",
            code: INVALID_VPN_CONCENTRATOR_ID_NOT_FOUND,
        },
        RefusalCase {
            case: "a short-form id nothing holds",
            value: "vcn-0abcd123",
            code: INVALID_VPN_CONCENTRATOR_ID_NOT_FOUND,
        },
        RefusalCase {
            case: "an id of another resource kind",
            value: "tgw-0123456789abcdef0",
            code: INVALID_VPN_CONCENTRATOR_ID_NOT_FOUND,
        },
        RefusalCase {
            case: "not a resource id at all",
            value: "not-an-id",
            code: INVALID_VPN_CONCENTRATOR_ID_NOT_FOUND,
        },
        RefusalCase {
            case: "an empty id",
            value: "",
            code: MISSING_PARAMETER,
        },
    ]
}

#[tokio::test]
async fn delete_refuses_an_id_the_account_holds_no_concentrator_for_and_writes_nothing() {
    let (router, engine) = super::ec2_query::emulator();
    // One stored concentrator, so a refusal below is about the id the request named rather
    // than about an empty store — and so the delete can be shown to have left it alone.
    let held = seed(
        &engine,
        VPN_STATE_AVAILABLE,
        Some("tgw-0123456789abcdef0"),
        &[("Name", "head-office")],
    );
    let before = stored_concentrators(&engine);

    for RefusalCase { case, value, code } in refusal_cases() {
        let (actual, message) = refusal(case, &router, value, &[]).await;
        assert_eq!(
            actual, code,
            "case {case:?}: expected the code {code}, got {actual}"
        );
        // The refusal says which id it is about, built from the request.
        let expected = if code == MISSING_PARAMETER {
            "The request must contain the parameter vpnConcentratorId".to_string()
        } else {
            format!("The vpn-concentrator ID '{value}' does not exist")
        };
        assert_eq!(
            message, expected,
            "case {case:?}: expected the message {expected:?}, got {message:?}"
        );

        // Nothing was written: the concentrator the account DOES hold is untouched and no
        // record was added or removed for the id that resolved to nothing.
        let actual = stored_state(&engine, &held);
        assert_eq!(
            actual.as_deref(),
            Some(VPN_STATE_AVAILABLE),
            "case {case:?}: the held concentrator must be untouched, got {actual:?}"
        );
        let actual = stored_concentrators(&engine);
        assert_eq!(
            actual, before,
            "case {case:?}: expected the store to hold {before} concentrators, got {actual}"
        );
    }

    // The id that resolves is still deleted after all those refusals.
    delete_ok("the held concentrator", &router, &held).await;
    let actual = stored_state(&engine, &held);
    assert_eq!(
        actual, None,
        "the held concentrator: expected the delete to take it away, got {actual:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE 3: DryRun changes nothing
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_dry_run_reports_the_outcome_and_changes_nothing() {
    let (router, engine) = super::ec2_query::emulator();
    let held = seed(
        &engine,
        VPN_STATE_AVAILABLE,
        Some("tgw-0123456789abcdef0"),
        &[],
    );
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
    let (actual, _) = refusal(case, &router, "vcn-0ffffffffffffffff", &dry_run).await;
    assert_eq!(
        actual, INVALID_VPN_CONCENTRATOR_ID_NOT_FOUND,
        "case {case:?}: expected the code {INVALID_VPN_CONCENTRATOR_ID_NOT_FOUND}, got {actual}"
    );

    // And the delete the dry run asked about still works.
    let case = "the delete the dry run asked about";
    delete_ok(case, &router, &held).await;
    let actual = stored_state(&engine, &held);
    assert_eq!(
        actual, None,
        "case {case:?}: expected the delete to take the concentrator away, got {actual:?}"
    );
}
