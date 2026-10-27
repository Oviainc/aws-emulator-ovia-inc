// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Region an identity source's IAM Identity Center parameters
//! name.
//!
//! The rule under test: a member documented to carry an Amazon Web Services Region has
//! to name a Region that exists, so `CreateIdentitySource` refuses a value naming none
//! with `ValidationException` (400) and records nothing —
//! `IamIdentityCenter$Region` is modelled as a plain string (`@required`,
//! `@length(max: 1000)`, no `@pattern` and no value set:
//! `smithy_query(query="shape",shape="IamIdentityCenter")`), so the constraint boundary
//! cannot decide it and the handler is where it is decided.
//!
//! The tables span the class the rule generalises over:
//!
//! * the ACCEPTED set is every entry of the Region catalog this emulator shares
//!   (`emulator_telemetry::policy::KNOWN_REGIONS`, "every AWS region the wire may carry,
//!   sorted for binary search" — the same catalog `state::is_aws_region` looks a value up
//!   in), so the test cites no hand-written list of its own and covers Regions no other
//!   suite of this crate names (`ap-east-2`, `mx-central-1`, `eu-isoe-west-1`, …);
//! * the REFUSED rows differ along HOW the value misses being a Region: it is empty or
//!   blank, it is a Region spelled in another case, it carries a Region plus surrounding
//!   punctuation or whitespace, it is a Region's prefix or an ordinal no Region carries,
//!   it is an availability zone rather than a Region, it is a well-formed code of a
//!   partition that does not exist, it is a hostname or an ARN rather than a Region, and
//!   it is the longest value the modelled `@length(max: 1000)` admits.
//!
//! Every request goes over the wire through the `MultiServiceRouter` the harness in
//! `approval_team_lifecycle.rs` mounts, so the route, the JSON envelope and the HTTP
//! status are all exercised, and an accepted Region is read back through
//! `GetIdentitySource`.

use serde_json::{json, Value};
use service_com_amazonaws_mpa_awsfluffycoreservice::state;

use crate::approval_team_lifecycle::{encode_label, error_code, text, Harness, REGION};

/// `ValidationException$Message` is modelled `@length(max: 1000)`, which a refusal
/// quoting a `Region` of up to 1000 characters still has to fit.
const MAX_VALIDATION_MESSAGE: usize = 1000;

/// `IamIdentityCenter$Region` is modelled `@length(max: 1000)`, so this is the longest
/// value the constraint boundary admits and therefore the longest one that reaches the
/// handler.
const MAX_REGION: usize = 1000;

/// An IAM Identity Center instance ARN satisfying `IdcInstanceArn`'s modelled pattern
/// `^arn:.+:sso:::instance/(?:sso)?ins-[a-zA-Z0-9-.]{16}$` and its
/// `@length(min: 20, max: 2048)`: the instance id's last 16 characters come from
/// `[a-zA-Z0-9-.]`.
fn instance_arn(id: &str) -> String {
    assert_eq!(
        id.len(),
        16,
        "IdcInstanceArn's pattern fixes the instance id at 16 characters, {id:?} has {}",
        id.len()
    );
    format!("arn:aws:sso:::instance/ssoins-{id}")
}

/// The instance every row names, so the only thing a row varies is the Region.
fn row_instance() -> String {
    instance_arn("1234567890abcdef")
}

/// A `CreateIdentitySource` body naming `region` as the instance's Region.
fn create_body(region: &str, client_token: Option<&str>) -> Value {
    let mut body = json!({
        "IdentitySourceParameters": {
            "IamIdentityCenter": {
                "InstanceArn": row_instance(),
                "Region": region,
            }
        }
    });
    if let Some(token) = client_token {
        body["ClientToken"] = json!(token);
    }
    body
}

/// `POST /identity-sources`.
async fn create(harness: &Harness, region: &str, client_token: Option<&str>) -> (u16, Value) {
    harness
        .call(
            "POST",
            "/identity-sources",
            Some(create_body(region, client_token)),
        )
        .await
}

/// `GET /identity-sources/{IdentitySourceArn}` — the label is non-greedy, so the ARN is
/// percent-encoded into one path segment.
async fn get_source(harness: &Harness, arn: &str) -> (u16, Value) {
    harness
        .call("GET", &format!("/identity-sources/{}", encode_label(arn)), None)
        .await
}

/// How many identity sources the store holds.
fn stored_sources(harness: &Harness) -> usize {
    let store = state::service_state(&*harness.engine).expect("the service store");
    let count = store.lock().identity_sources.len();
    count
}

/// How many identity sources `ListIdentitySources` reports.
async fn listed_sources(harness: &Harness, case: &str) -> usize {
    let (status, body) = harness.call("POST", "/identity-sources/?List", None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListIdentitySources returned {status} with {body:?}"
    );
    body.get("IdentitySources")
        .and_then(Value::as_array)
        .map_or(0, Vec::len)
}

/// The refused rows: `(case, the value the request names as the instance's Region)`.
/// Every row satisfies `IamIdentityCenter$Region`'s `@length(max: 1000)`, so each one
/// reaches the handler rather than the constraint boundary.
fn regions_that_name_no_region() -> Vec<(&'static str, String)> {
    vec![
        // `Region` carries no modelled length MINIMUM, so the empty string is a value the
        // boundary admits — and names no Region.
        ("the empty string", String::new()),
        ("a single space", " ".to_string()),
        ("whitespace only", "   ".to_string()),
        ("a Region in upper case", "US-WEST-2".to_string()),
        ("a Region in mixed case", "Us-West-2".to_string()),
        ("a Region with a trailing space", "us-west-2 ".to_string()),
        ("a Region with a leading space", " us-west-2".to_string()),
        ("a Region with a trailing dot", "us-west-2.".to_string()),
        ("a Region with its separators removed", "useast1".to_string()),
        (
            "a Region with underscores for its separators",
            "us_west_2".to_string(),
        ),
        ("a Region's prefix without its ordinal", "us-west".to_string()),
        (
            "a Region prefix with an ordinal no Region carries",
            "us-west-9".to_string(),
        ),
        (
            "an availability zone rather than a Region",
            "us-west-2a".to_string(),
        ),
        (
            "a local zone rather than a Region",
            "us-west-2-lax-1a".to_string(),
        ),
        (
            "a well-formed code of a partition that does not exist",
            "eu-gov-west-1".to_string(),
        ),
        (
            "a well-formed code naming no Region at all",
            "mars-central-9".to_string(),
        ),
        ("free text", "not a region!!".to_string()),
        (
            "this service's regional hostname rather than a Region",
            format!("mpa.{REGION}.amazonaws.com"),
        ),
        (
            "the instance's ARN rather than a Region",
            row_instance(),
        ),
        (
            "an ARN's region field, quoted with its colons",
            format!(":{REGION}:"),
        ),
        (
            "the longest value the modelled length maximum (1000) admits",
            "r".repeat(MAX_REGION),
        ),
    ]
}

/// A value that names no Amazon Web Services Region is refused with
/// `ValidationException` (400), in this service's one wording for that class, and
/// nothing is recorded: no source in the store, none in the listing.
#[tokio::test]
async fn a_region_that_names_no_aws_region_is_refused_and_records_nothing() {
    for (case, region) in regions_that_name_no_region() {
        // A store of its own per row, so what one row does cannot carry into the next.
        let harness = Harness::new();
        let (status, body) = create(&harness, &region, None).await;

        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for Region of {} character(s), got {status} \
             with {body:?}",
            region.len()
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected error code ValidationException, got {body:?}"
        );
        let message = text(&body, "Message");
        assert_eq!(
            message,
            state::invalid_region("Region", &region).message,
            "case {case:?}: wrong refusal message, got {message:?}"
        );
        assert!(
            message.len() <= MAX_VALIDATION_MESSAGE,
            "case {case:?}: the refusal is {} characters, past the modelled maximum of \
             {MAX_VALIDATION_MESSAGE}: {message:?}",
            message.len()
        );

        let stored = stored_sources(&harness);
        assert_eq!(
            stored, 0,
            "case {case:?}: the store holds {stored} identity source(s) after a refused \
             create"
        );
        let listed = listed_sources(&harness, case).await;
        assert_eq!(
            listed, 0,
            "case {case:?}: the listing reports {listed} identity source(s) after a \
             refused create"
        );
    }
}

/// Every Region of the catalog this emulator shares is accepted, recorded and reported
/// back by the read — so the rule refuses values naming no Region without narrowing the
/// set of Regions a caller may name.
#[tokio::test]
async fn every_region_the_catalog_names_is_accepted_and_reads_back() {
    // The catalog `state::is_aws_region` looks a value up in, driven whole rather than
    // sampled, so no entry can be accepted by the predicate and refused on the wire.
    for region in emulator_telemetry::policy::KNOWN_REGIONS {
        // `state::MAX_IDENTITY_SOURCES` bounds one store, and the catalog is longer than
        // that, so each Region is created in a store of its own.
        let harness = Harness::new();
        let (status, created) = create(&harness, region, None).await;
        assert_eq!(
            status, 200,
            "case {region:?}: a Region the catalog names was refused: {created:?}"
        );
        let arn = text(&created, "IdentitySourceArn");
        assert!(
            !arn.is_empty(),
            "case {region:?}: create reported no IdentitySourceArn: {created:?}"
        );

        let (status, read) = get_source(&harness, &arn).await;
        assert_eq!(
            status, 200,
            "case {region:?}: GetIdentitySource returned {status} with {read:?}"
        );
        let reported = text(
            &read["IdentitySourceParameters"]["IamIdentityCenter"],
            "Region",
        );
        assert_eq!(
            reported, *region,
            "case {region:?}: the read reports Region {reported:?}"
        );
        assert_eq!(
            text(
                &read["IdentitySourceParameters"]["IamIdentityCenter"],
                "InstanceArn"
            ),
            row_instance(),
            "case {region:?}: the read reports another instance: {read:?}"
        );
    }
}

/// The predicate itself, over both tables: exactly the catalog's entries are Regions.
#[test]
fn the_region_rule_covers_exactly_the_catalog() {
    for region in emulator_telemetry::policy::KNOWN_REGIONS {
        assert!(
            state::is_aws_region(region),
            "case {region:?}: a catalog entry must be an Amazon Web Services Region"
        );
    }
    for (case, region) in regions_that_name_no_region() {
        assert!(
            !state::is_aws_region(&region),
            "case {case:?}: expected is_aws_region of a {}-character value to be false",
            region.len()
        );
    }
}

/// A refused create consumes nothing a later call needs: the `@idempotencyToken` the
/// refused request carried is still free, so the same token with a Region that names one
/// creates the source — a refusal is not recorded as a completed create.
#[tokio::test]
async fn a_refused_region_leaves_the_client_token_free() {
    let harness = Harness::new();
    let token = "region-rule-token";

    let (status, body) = create(&harness, "mars-central-9", Some(token)).await;
    assert_eq!(status, 400, "the refused create answered {status}: {body:?}");

    let (status, created) = create(&harness, REGION, Some(token)).await;
    assert_eq!(
        status, 200,
        "the token the refusal carried was not free: {created:?}"
    );
    let arn = text(&created, "IdentitySourceArn");

    // The same token now replays that create rather than minting a second source.
    let (status, replayed) = create(&harness, REGION, Some(token)).await;
    assert_eq!(status, 200, "the replay answered {status}: {replayed:?}");
    assert_eq!(
        text(&replayed, "IdentitySourceArn"),
        arn,
        "the replay minted a second source: {replayed:?}"
    );
    let stored = stored_sources(&harness);
    assert_eq!(
        stored, 1,
        "the store holds {stored} identity source(s) after one accepted create"
    );
}

/// The Region rule does not move the organization gate: a caller the store does not
/// record as the organization's management account is still refused with 403
/// `AccessDeniedException`, whatever its Region names.
#[tokio::test]
async fn the_organization_gate_still_comes_before_the_region_rule() {
    for (case, region) in regions_that_name_no_region() {
        let harness = Harness::without_organization();
        let (status, body) = create(&harness, &region, None).await;
        assert_eq!(
            status, 403,
            "case {case:?}: expected HTTP 403 for an unauthorized caller, got {status} \
             with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "AccessDeniedException",
            "case {case:?}: expected error code AccessDeniedException, got {body:?}"
        );
    }
}
