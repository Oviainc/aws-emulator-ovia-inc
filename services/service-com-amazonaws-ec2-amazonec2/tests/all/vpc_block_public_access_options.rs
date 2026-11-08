// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the VPC Block Public Access OPTIONS slice —
//! `ModifyVpcBlockPublicAccessOptions` and `DescribeVpcBlockPublicAccessOptions` — driven
//! over the wire the service actually speaks: `ec2Query`, through the crate's shared
//! [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. VPC BPA is ONE SETTING OF ONE REGION: the describe reports the options of the Region
//!    the request was signed for, a Region no modify has been made for reports the documented
//!    default (`off`, `default-state`), and a modify in one Region leaves every other
//!    Region's options where they were;
//! 2. a modify PUTS that Region's mode to the one it names and the describe REPORTS THAT
//!    STORE: the mode round-trips for every member of the modelled catalog, the last write
//!    wins, and the members the service owns (the account id, the Region, the state, the
//!    managing entity, whether exclusions are allowed and the update's timestamp) are derived
//!    rather than taken;
//! 3. the CLOCK, not a caller, moves the options `update-in-progress` -> `update-complete`;
//! 4. a mode outside the modelled catalog is refused and nothing is written, and `DryRun`
//!    reports the permitted outcome of either operation without writing or disclosing
//!    anything.
//!
//! Each table row is a distinct input along what the rule generalises over — which Region the
//! call is signed for, which mode (every member of the modelled catalog), how many modifies
//! have been made, which operation carries the dry run — so no row is the specification.

use std::time::Duration;

use ec2::ops::types::INTERNET_GATEWAY_BLOCK_MODE_VALUES;
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, INTERNET_GATEWAY_BLOCK_MODE_OFF,
    MANAGED_BY_ACCOUNT, TRANSITION_DELAY, VPC_BPA_EXCLUSIONS_ALLOWED, VPC_BPA_STATE_DEFAULT,
    VPC_BPA_STATE_UPDATE_COMPLETE, VPC_BPA_STATE_UPDATE_IN_PROGRESS,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{as_timestamp, call, element, error_code};
use super::wire::{param, Param};

/// The Region most calls here are signed for. Rule 1's rows name others: the options are per
/// Region, so the Region a call carries is one of the inputs the rules generalise over.
const REGION: &str = "us-west-2";

/// How long a clock-owed transition takes to fall due: the service's own span, plus a margin
/// so a slow machine cannot read the state before it is due.
async fn past_the_transition() {
    tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(400)).await;
}

/// One `action` call signed for `region`, as its HTTP status and XML body.
async fn api(
    router: &MultiServiceRouter,
    region: &str,
    action: &str,
    params: &[Param],
) -> (u16, String) {
    call(router, region, action, params).await
}

/// One `action` call that has to succeed, and the body it answered.
async fn ok(
    case: &str,
    router: &MultiServiceRouter,
    region: &str,
    action: &str,
    params: &[Param],
) -> String {
    let (status, body) = api(router, region, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// The VPC BPA options a response reports, as its members.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Options {
    aws_account_id: Option<String>,
    aws_region: Option<String>,
    state: Option<String>,
    internet_gateway_block_mode: Option<String>,
    reason: Option<String>,
    last_update_timestamp: Option<String>,
    managed_by: Option<String>,
    exclusions_allowed: Option<String>,
}

/// The options element of `body`, read member by member.
fn reported(case: &str, body: &str) -> Options {
    // The element carries the response's XML namespace, so it is matched by its open tag
    // rather than by `<name>`.
    assert!(
        body.contains("<vpcBlockPublicAccessOptions"),
        "case {case:?}: no <vpcBlockPublicAccessOptions> in {body}"
    );
    Options {
        aws_account_id: element(body, "awsAccountId"),
        aws_region: element(body, "awsRegion"),
        state: element(body, "state"),
        internet_gateway_block_mode: element(body, "internetGatewayBlockMode"),
        reason: element(body, "reason"),
        last_update_timestamp: element(body, "lastUpdateTimestamp"),
        managed_by: element(body, "managedBy"),
        exclusions_allowed: element(body, "exclusionsAllowed"),
    }
}

/// One `DescribeVpcBlockPublicAccessOptions` call signed for `region`, as the options it
/// reported.
async fn describe(case: &str, router: &MultiServiceRouter, region: &str) -> Options {
    let body = ok(
        case,
        router,
        region,
        "DescribeVpcBlockPublicAccessOptions",
        &[],
    )
    .await;
    reported(case, &body)
}

/// One `ModifyVpcBlockPublicAccessOptions` call signed for `region`, as the options it
/// answered.
async fn modify(case: &str, router: &MultiServiceRouter, region: &str, mode: &str) -> Options {
    let body = ok(
        case,
        router,
        region,
        "ModifyVpcBlockPublicAccessOptions",
        &[param("InternetGatewayBlockMode", mode)],
    )
    .await;
    reported(case, &body)
}

/// The Regions the store holds VPC BPA options for.
fn stored_regions(engine: &engine::InMemoryEngine) -> Vec<String> {
    engine
        .state::<Ec2State>()
        .lock()
        .vpc_block_public_access_options
        .keys()
        .cloned()
        .collect()
}

/// Assert the members no request sets: the account this emulator serves, the Region the call
/// was signed for, the managing entity and whether exclusions are allowed.
fn assert_derived_members(case: &str, region: &str, options: &Options) {
    assert_eq!(
        options.aws_account_id.as_deref(),
        Some(engine::emulator_account_id()),
        "case {case:?}: expected the emulator's account id, got {:?}",
        options.aws_account_id
    );
    assert_eq!(
        options.aws_region.as_deref(),
        Some(region),
        "case {case:?}: expected the signed Region {region}, got {:?}",
        options.aws_region
    );
    assert_eq!(
        options.managed_by.as_deref(),
        Some(MANAGED_BY_ACCOUNT),
        "case {case:?}: expected the account to manage the state, got {:?}",
        options.managed_by
    );
    assert_eq!(
        options.exclusions_allowed.as_deref(),
        Some(VPC_BPA_EXCLUSIONS_ALLOWED),
        "case {case:?}: expected exclusions to be allowed, got {:?}",
        options.exclusions_allowed
    );
    assert_eq!(
        options.reason, None,
        "case {case:?}: an account's own update has no reason to report, got {:?}",
        options.reason
    );
}

// ---------------------------------------------------------------------------
// Rule 1: one setting per Region, defaulting to `off` / `default-state`
// ---------------------------------------------------------------------------

/// RULE 1: a Region no modify has been made for reports the documented default — VPC BPA
/// `off` and the `default-state` — and the read stores nothing.
///
/// The rows are Regions of different shapes and lengths, so no one Region name is the
/// specification.
#[tokio::test]
async fn a_region_no_modify_was_made_for_reports_the_default_options() {
    let (router, engine) = super::ec2_query::emulator();

    // Regions of the enabled-by-default set ([`ec2::state::REGIONS_ENABLED_BY_DEFAULT`]),
    // which is what this endpoint serves at all, differing in partition prefix and length.
    for region in ["us-west-2", "eu-central-1", "ap-southeast-2", "sa-east-1"] {
        let case = format!("the default options of {region}");
        let options = describe(&case, &router, region).await;
        assert_derived_members(&case, region, &options);
        assert_eq!(
            options.internet_gateway_block_mode.as_deref(),
            Some(INTERNET_GATEWAY_BLOCK_MODE_OFF),
            "case {case:?}: expected VPC BPA to be off, got {:?}",
            options.internet_gateway_block_mode
        );
        assert_eq!(
            options.state.as_deref(),
            Some(VPC_BPA_STATE_DEFAULT),
            "case {case:?}: expected the default state, got {:?}",
            options.state
        );
        assert_eq!(
            options.last_update_timestamp, None,
            "case {case:?}: a mode that was never updated has no update timestamp, got {:?}",
            options.last_update_timestamp
        );
    }

    let held = stored_regions(&engine);
    assert!(
        held.is_empty(),
        "a describe must not write: expected no stored Region, got {held:?}"
    );
}

/// RULE 1: the options are a setting of ONE Region: a modify signed for one Region is
/// reported by a describe signed for that Region and changes no other Region's options.
#[tokio::test]
async fn a_modify_changes_only_the_region_it_was_signed_for() {
    let (router, engine) = super::ec2_query::emulator();

    // (the Region modified, the mode it is put into)
    let modified: [(&str, &str); 2] = [
        ("us-west-2", "block-bidirectional"),
        ("eu-west-1", "block-ingress"),
    ];
    for (region, mode) in modified {
        let case = format!("{region} -> {mode}");
        modify(&case, &router, region, mode).await;
    }

    for (region, mode) in modified {
        let case = format!("{region} reports {mode}");
        let options = describe(&case, &router, region).await;
        assert_derived_members(&case, region, &options);
        assert_eq!(
            options.internet_gateway_block_mode.as_deref(),
            Some(mode),
            "case {case:?}: expected the mode this Region was put into, got {:?}",
            options.internet_gateway_block_mode
        );
    }

    // A Region neither modify named is still at its default.
    let untouched = "ap-northeast-1";
    let case = format!("{untouched} was never modified");
    let options = describe(&case, &router, untouched).await;
    assert_eq!(
        (
            options.internet_gateway_block_mode.as_deref(),
            options.state.as_deref()
        ),
        (
            Some(INTERNET_GATEWAY_BLOCK_MODE_OFF),
            Some(VPC_BPA_STATE_DEFAULT)
        ),
        "case {case:?}: expected the default options, got {options:?}"
    );

    let mut held = stored_regions(&engine);
    held.sort();
    assert_eq!(
        held,
        vec!["eu-west-1".to_string(), "us-west-2".to_string()],
        "only the Regions a modify named may be stored, got {held:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 2: a modify writes the mode, and the describe reports that store
// ---------------------------------------------------------------------------

/// RULE 2: every member of the modelled `InternetGatewayBlockMode` catalog is accepted, and
/// the mode a modify sets is the mode the describe reports.
///
/// The catalog is the generated [`ec2::ops::types::INTERNET_GATEWAY_BLOCK_MODE_VALUES`], so a
/// member added to the model is covered here without this test being edited — including the
/// `off` member, which is how an account turns VPC BPA back off.
#[tokio::test]
async fn every_documented_block_mode_round_trips_through_the_store() {
    for mode in INTERNET_GATEWAY_BLOCK_MODE_VALUES {
        // One emulator per row, so each row's Region starts from the default rather than
        // from the row before it.
        let (router, engine) = super::ec2_query::emulator();
        let case = format!("mode {mode}");

        let answered = modify(&case, &router, REGION, mode).await;
        assert_derived_members(&case, REGION, &answered);
        assert_eq!(
            answered.internet_gateway_block_mode.as_deref(),
            Some(*mode),
            "case {case:?}: expected the modify to answer the mode it set, got {:?}",
            answered.internet_gateway_block_mode
        );
        assert_eq!(
            answered.state.as_deref(),
            Some(VPC_BPA_STATE_UPDATE_IN_PROGRESS),
            "case {case:?}: expected the update to be in progress, got {:?}",
            answered.state
        );
        let updated_at = answered
            .last_update_timestamp
            .as_deref()
            .and_then(as_timestamp);
        assert!(
            updated_at.is_some(),
            "case {case:?}: expected an update timestamp, got {:?}",
            answered.last_update_timestamp
        );

        // What the describe reports is what the modify answered: one store, one projection.
        let described = describe(&case, &router, REGION).await;
        assert_eq!(
            described, answered,
            "case {case:?}: the describe must report what the modify answered, got {described:?}"
        );
        assert_eq!(
            stored_regions(&engine),
            vec![REGION.to_string()],
            "case {case:?}: the modify must store exactly the Region it was signed for"
        );
    }
}

/// RULE 2: the LAST modify of a Region wins, and its timestamp does not go backwards — the
/// mode in force is the one the most recent call set, whichever modes came before it.
///
/// The row is the whole modelled catalog applied in order and then `off` again, so the
/// sequence covers a change between two blocking modes and a change back out of one.
#[tokio::test]
async fn the_last_modify_of_a_region_is_the_one_reported() {
    let (router, _engine) = super::ec2_query::emulator();

    let mut sequence: Vec<&str> = INTERNET_GATEWAY_BLOCK_MODE_VALUES.to_vec();
    sequence.push(INTERNET_GATEWAY_BLOCK_MODE_OFF);

    let mut previous: Option<f64> = None;
    for mode in sequence {
        let case = format!("after a modify to {mode}");
        let answered = modify(&case, &router, REGION, mode).await;
        let described = describe(&case, &router, REGION).await;
        assert_eq!(
            described.internet_gateway_block_mode.as_deref(),
            Some(mode),
            "case {case:?}: expected the mode the last modify set, got {:?}",
            described.internet_gateway_block_mode
        );
        assert_eq!(
            described.internet_gateway_block_mode, answered.internet_gateway_block_mode,
            "case {case:?}: the describe must agree with the modify, got {described:?}"
        );
        let updated_at = described
            .last_update_timestamp
            .as_deref()
            .and_then(as_timestamp)
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected an update timestamp, got {:?}",
                    described.last_update_timestamp
                )
            });
        if let Some(earlier) = previous {
            assert!(
                updated_at >= earlier,
                "case {case:?}: the update timestamp must not go backwards, got \
                 {updated_at} after {earlier}"
            );
        }
        previous = Some(updated_at);
    }
}

// ---------------------------------------------------------------------------
// Rule 3: the clock completes the update
// ---------------------------------------------------------------------------

/// RULE 3: a modify leaves the options `update-in-progress` — the span is never zero, so a
/// caller reading straight after it still sees the update in flight — and the clock alone
/// moves them to `update-complete`, with the mode and the update timestamp untouched.
///
/// The rows are the blocking modes and the `off` mode, so the move does not depend on which
/// mode the update set.
#[tokio::test]
async fn the_clock_moves_the_update_to_update_complete() {
    for mode in INTERNET_GATEWAY_BLOCK_MODE_VALUES {
        let (router, _engine) = super::ec2_query::emulator();
        let case = format!("the clock completes the update to {mode}");

        let answered = modify(&case, &router, REGION, mode).await;
        assert_eq!(
            answered.state.as_deref(),
            Some(VPC_BPA_STATE_UPDATE_IN_PROGRESS),
            "case {case:?}: expected the update in progress straight after the modify, got {:?}",
            answered.state
        );

        past_the_transition().await;

        let described = describe(&case, &router, REGION).await;
        assert_eq!(
            described.state.as_deref(),
            Some(VPC_BPA_STATE_UPDATE_COMPLETE),
            "case {case:?}: expected the update to have completed, got {:?}",
            described.state
        );
        assert_eq!(
            described.internet_gateway_block_mode.as_deref(),
            Some(*mode),
            "case {case:?}: the completed update must keep its mode, got {:?}",
            described.internet_gateway_block_mode
        );
        assert_eq!(
            described.last_update_timestamp, answered.last_update_timestamp,
            "case {case:?}: completing the update does not update the mode again, got {:?}",
            described.last_update_timestamp
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 4: what is refused, and what a dry run does not write
// ---------------------------------------------------------------------------

/// RULE 4: a mode this API does not document is refused and nothing is written.
///
/// The rows are the ways a mode can be outside the modelled
/// `InternetGatewayBlockMode` catalog: another word, a member in the wrong case, a member of
/// the EXCLUSION mode catalog, an empty value, and no mode at all. The member is modelled as
/// that enum and is required, so the boundary refuses every one of them before the handler
/// runs and the code is the boundary's rather than the handler's.
#[tokio::test]
async fn a_mode_outside_the_modelled_catalog_is_refused_and_writes_nothing() {
    let (router, engine) = super::ec2_query::emulator();

    // (case, the request's fields, the expected error code)
    let cases: [(&str, Vec<Param>, &str); 5] = [
        (
            "a word the catalog does not hold",
            vec![param("InternetGatewayBlockMode", "block-everything")],
            "ValidationError",
        ),
        (
            "a catalog member in the wrong case",
            vec![param("InternetGatewayBlockMode", "OFF")],
            "ValidationError",
        ),
        (
            // `allow-bidirectional` is a member of the EXCLUSION mode catalog, not of this
            // one.
            "a member of another catalog",
            vec![param("InternetGatewayBlockMode", "allow-bidirectional")],
            "ValidationError",
        ),
        (
            "an empty mode",
            vec![param("InternetGatewayBlockMode", "")],
            "ValidationError",
        ),
        // The required member absent altogether is a malformed request rather than a bad
        // value, so the boundary says so.
        ("no mode at all", vec![], "MalformedInput"),
    ];

    for (case, params, expected_code) in cases {
        let (status, body) = api(
            &router,
            REGION,
            "ModifyVpcBlockPublicAccessOptions",
            &params,
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
        let held = stored_regions(&engine);
        assert!(
            held.is_empty(),
            "case {case:?}: a refused modify must store nothing, got {held:?}"
        );
    }

    // The Region is still at its default after every refusal above.
    let options = describe("after the refusals", &router, REGION).await;
    assert_eq!(
        (
            options.internet_gateway_block_mode.as_deref(),
            options.state.as_deref()
        ),
        (
            Some(INTERNET_GATEWAY_BLOCK_MODE_OFF),
            Some(VPC_BPA_STATE_DEFAULT)
        ),
        "the refusals must leave the Region at its default, got {options:?}"
    );
}

/// RULE 4: `DryRun` reports the permitted outcome — `DryRunOperation`, HTTP 412 — on either
/// operation, writes nothing and discloses no options; `DryRun=false` is an ordinary call.
#[tokio::test]
async fn dry_run_reports_the_permitted_outcome_without_writing() {
    // (case, the action, its parameters without the dry run)
    let cases: [(&str, &str, Vec<Param>); 2] = [
        (
            "a dry-run modify",
            "ModifyVpcBlockPublicAccessOptions",
            vec![param("InternetGatewayBlockMode", "block-ingress")],
        ),
        (
            "a dry-run describe",
            "DescribeVpcBlockPublicAccessOptions",
            Vec::new(),
        ),
    ];

    for (case, action, base) in cases {
        // One emulator per row, so the ordinary call that ends the row before cannot be
        // what this row's store assertions see.
        let (router, engine) = super::ec2_query::emulator();
        let mut params = base.clone();
        params.push(param("DryRun", "true"));
        let (status, body) = api(&router, REGION, action, &params).await;
        assert_eq!(
            status, 412,
            "case {case:?}: expected HTTP 412, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "DryRunOperation",
            "case {case:?}: expected DryRunOperation, got {body}"
        );
        assert!(
            !body.contains("<vpcBlockPublicAccessOptions"),
            "case {case:?}: a dry run reports no options, got {body}"
        );
        let held = stored_regions(&engine);
        assert!(
            held.is_empty(),
            "case {case:?}: a dry run must store nothing, got {held:?}"
        );

        // The same call with the flag off is an ordinary call.
        let mut params = base;
        params.push(param("DryRun", "false"));
        let body = ok(case, &router, REGION, action, &params).await;
        assert!(
            body.contains("<vpcBlockPublicAccessOptions"),
            "case {case:?}: DryRun=false must be an ordinary call, got {body}"
        );
    }
}
