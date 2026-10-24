// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the organization service-access slice —
//! `StartOrganizationServiceAccessUpdate` and `ListOrganizationServiceAccessStatus` —
//! driven over HTTP through the real router (the harness lives in
//! `crate::global_network_lifecycle`).
//!
//! The rule under test: an account's Network Manager organization service-access state is
//! one stored record. `StartOrganizationServiceAccessUpdate` is the only writer — it
//! records `ENABLED` or `DISABLED` for the `Action` its documented catalog allows, mints
//! the organization id once and keeps it, and on enabling starts a service-linked-role
//! (SLR) deployment that is `IN_PROGRESS` until the settle span the clock owes has passed
//! and `SUCCEEDED` after. `ListOrganizationServiceAccessStatus` reports exactly that
//! record — no `OrganizationStatus` at all while none exists — with one `AccountStatus` per
//! account the deployment covers, paged by `MaxResults`/`NextToken`, and reading never
//! changes what a later reader sees. An `Action` outside the catalog and a `NextToken` this
//! service did not mint are refused with `ValidationException` (HTTP 400) and write
//! nothing.
//!
//! The tables below span that class: every value of the documented `Action` catalog and
//! values differing from it in case, in surrounding whitespace, in length (including at
//! and one past the modelled bound) and in being another word entirely; sequences of
//! catalog actions differing in which action mints the record, in how many updates they
//! apply and in their ordering, plus a retry of the enabling call repeated while the
//! deployment it started is still owed; `MaxResults` at each published bound and one past
//! each; and tokens differing in alphabet, in length and in whether they decode at all.
//!
//! Nothing here reaches into `crate::state`: the record is only ever read back through
//! `ListOrganizationServiceAccessStatus`.

use serde_json::{json, Value};

use crate::global_network_lifecycle::{error_code, Emulator, PAST_SETTLE};

/// The one URI both operations are bound to (`smithy_query query="operation"
/// operation="ListOrganizationServiceAccessStatus,StartOrganizationServiceAccessUpdate"`:
/// `GET` and `POST /organizations/service-access`).
const SERVICE_ACCESS_URI: &str = "/organizations/service-access";

/// The values `Action` accepts, from the member's own documentation ("This can be either
/// ENABLE or DISABLE", `smithy_query query="doc"
/// shape="StartOrganizationServiceAccessUpdateRequest" member="Action"`), each paired with
/// the `OrganizationAwsServiceAccessStatus` it leaves behind ("This will be ENABLED or
/// DISABLED", `smithy_query query="doc" shape="OrganizationStatus"
/// member="OrganizationAwsServiceAccessStatus"`).
const ACTION_CATALOG: [(&str, &str); 2] = [("ENABLE", "ENABLED"), ("DISABLE", "DISABLED")];

/// The two `SLRDeploymentStatus` values ("either SUCCEEDED or IN_PROGRESS", `smithy_query
/// query="doc" shape="OrganizationStatus" member="SLRDeploymentStatus"`).
const IN_PROGRESS: &str = "IN_PROGRESS";
/// See [`IN_PROGRESS`].
const SUCCEEDED: &str = "SUCCEEDED";

/// `StartOrganizationServiceAccessUpdate` with `action`.
async fn start_update(emulator: &Emulator, action: &str) -> (u16, Value) {
    emulator
        .post(SERVICE_ACCESS_URI, json!({ "Action": action }))
        .await
}

/// `StartOrganizationServiceAccessUpdate`, asserting it was accepted, and answering the
/// `OrganizationStatus` it reports.
async fn start_update_ok(emulator: &Emulator, case: &str, action: &str) -> Value {
    let (status, body) = start_update(emulator, action).await;
    assert_eq!(
        status, 200,
        "case {case}: StartOrganizationServiceAccessUpdate({action:?}) expected 200, got {status} with {body}"
    );
    body["OrganizationStatus"].clone()
}

/// `ListOrganizationServiceAccessStatus` with an optional query string, answering the
/// status and the whole body.
async fn list_status(emulator: &Emulator, query: &str) -> (u16, Value) {
    let uri = if query.is_empty() {
        SERVICE_ACCESS_URI.to_string()
    } else {
        format!("{SERVICE_ACCESS_URI}?{query}")
    };
    emulator.get(&uri).await
}

/// `ListOrganizationServiceAccessStatus`, asserting it was answered, and returning the
/// reported `OrganizationStatus` (`Value::Null` when none is reported at all).
async fn reported_status(emulator: &Emulator, case: &str) -> Value {
    let (status, body) = list_status(emulator, "").await;
    assert_eq!(
        status, 200,
        "case {case}: ListOrganizationServiceAccessStatus expected 200, got {status} with {body}"
    );
    assert!(
        body["NextToken"].is_null(),
        "case {case}: one account fits one page, so no NextToken is due, got {body}"
    );
    body["OrganizationStatus"].clone()
}

/// The id shape an Amazon Web Services Organization carries: `o-` and 8..17 lowercase hex
/// digits (`OrganizationId` pattern `^o-([0-9a-f]{8,17})$`, `smithy_query
/// shape=OrganizationId`).
fn is_organization_id(id: &str) -> bool {
    match id.strip_prefix("o-") {
        Some(suffix) => {
            (8..=17).contains(&suffix.len())
                && suffix
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        }
        None => false,
    }
}

/// Assert `status` reports `access_status` and `slr_status` (the latter absent when
/// `None`), with one `AccountStatus` per account of the organization carrying the same SLR
/// status — and answer the organization id it carries.
fn assert_status(case: &str, status: &Value, access_status: &str, slr_status: Option<&str>) -> String {
    let organization_id = status["OrganizationId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected an OrganizationId in {status}"))
        .to_string();
    assert!(
        is_organization_id(&organization_id),
        "case {case}: expected an o-<hex> organization id, got {organization_id:?}"
    );
    assert_eq!(
        status["OrganizationAwsServiceAccessStatus"],
        json!(access_status),
        "case {case}: expected service access {access_status}, got {status}"
    );
    assert_eq!(
        status["SLRDeploymentStatus"],
        slr_status.map_or(Value::Null, |value| json!(value)),
        "case {case}: expected SLR deployment {slr_status:?}, got {status}"
    );
    let expected_accounts = match slr_status {
        // An enabled organization deploys the SLR into its accounts; this emulator
        // presents exactly one account, so that is the whole membership.
        Some(deployment) => json!([{
            "AccountId": engine::emulator_account_id(),
            "SLRDeploymentStatus": deployment,
        }]),
        // Nothing is deployed while access is disabled, so no account has a deployment.
        None => json!([]),
    };
    assert_eq!(
        status["AccountStatusList"], expected_accounts,
        "case {case}: expected the account statuses {expected_accounts}, got {status}"
    );
    organization_id
}

#[tokio::test]
async fn organization_service_access_round_trips_through_stored_state() {
    let emulator = Emulator::new();

    // Nothing has ever been updated: this service knows of no organization, so there is no
    // status to report — not an empty one.
    let before = reported_status(&emulator, "no record").await;
    assert!(
        before.is_null(),
        "case no record: expected no OrganizationStatus at all, got {before}"
    );

    // ENABLE: the organization id is minted here, and the SLR deployment starts.
    let enabled = start_update_ok(&emulator, "enable", "ENABLE").await;
    let organization_id = assert_status(
        "enable",
        &enabled,
        ACTION_CATALOG[0].1,
        Some(IN_PROGRESS),
    );

    // The transient is observable: a caller reading straight after the update still sees
    // the deployment IN_PROGRESS, and sees the stored record rather than a fresh one.
    let reading = reported_status(&emulator, "enable read-back").await;
    assert_eq!(
        assert_status(
            "enable read-back",
            &reading,
            ACTION_CATALOG[0].1,
            Some(IN_PROGRESS)
        ),
        organization_id,
        "case enable read-back: the organization id must be the stored one, got {reading}"
    );

    // The transition the clock owes becomes due without anything polling for it.
    tokio::time::sleep(PAST_SETTLE).await;
    let settled = reported_status(&emulator, "settled").await;
    assert_eq!(
        assert_status("settled", &settled, ACTION_CATALOG[0].1, Some(SUCCEEDED)),
        organization_id,
        "case settled: the organization id must not change, got {settled}"
    );
    // A read does not change what the next reader sees.
    let again = reported_status(&emulator, "settled twice").await;
    assert_eq!(
        again, settled,
        "case settled twice: a read must not change the record, got {again}"
    );

    // ENABLE again: already enabled, so nothing is restarted — the deployment stays
    // SUCCEEDED and the organization keeps its id.
    let re_enabled = start_update_ok(&emulator, "enable again", "ENABLE").await;
    assert_eq!(
        assert_status(
            "enable again",
            &re_enabled,
            ACTION_CATALOG[0].1,
            Some(SUCCEEDED)
        ),
        organization_id,
        "case enable again: the organization id must not change, got {re_enabled}"
    );

    // DISABLE: no SLR is deployed any more, so no deployment status is reported for the
    // organization or for any account in it; the organization itself is unchanged.
    let disabled = start_update_ok(&emulator, "disable", "DISABLE").await;
    assert_eq!(
        assert_status("disable", &disabled, ACTION_CATALOG[1].1, None),
        organization_id,
        "case disable: the organization id must not change, got {disabled}"
    );
    let disabled_read = reported_status(&emulator, "disable read-back").await;
    assert_eq!(
        assert_status("disable read-back", &disabled_read, ACTION_CATALOG[1].1, None),
        organization_id,
        "case disable read-back: expected the stored record, got {disabled_read}"
    );

    // Re-ENABLE: a fresh deployment, again IN_PROGRESS before it settles.
    let re_enabled = start_update_ok(&emulator, "re-enable", "ENABLE").await;
    assert_eq!(
        assert_status(
            "re-enable",
            &re_enabled,
            ACTION_CATALOG[0].1,
            Some(IN_PROGRESS)
        ),
        organization_id,
        "case re-enable: the organization id must not change, got {re_enabled}"
    );
    tokio::time::sleep(PAST_SETTLE).await;
    let re_settled = reported_status(&emulator, "re-enable settled").await;
    assert_eq!(
        assert_status(
            "re-enable settled",
            &re_settled,
            ACTION_CATALOG[0].1,
            Some(SUCCEEDED)
        ),
        organization_id,
        "case re-enable settled: the organization id must not change, got {re_settled}"
    );
}

/// The access status the documented catalog pairs with `action`.
fn expected_access_status(action: &str) -> &'static str {
    ACTION_CATALOG
        .into_iter()
        .find_map(|(catalog_action, status)| (catalog_action == action).then_some(status))
        .unwrap_or_else(|| panic!("{action:?} is not a value of the documented Action catalog"))
}

/// Assert `status` reports `access_status` with the accounts that status implies, WITHOUT
/// pinning which side of the settle span the SLR deployment is on: an enabled organization
/// deploys the SLR into the one account this emulator presents and reports one of the two
/// documented `SLRDeploymentStatus` values, for the organization and for that account
/// alike; a disabled one deploys nothing, so it reports no deployment status at all.
///
/// Answers the organization id the record carries, so a caller can assert it is kept.
fn assert_access(case: &str, status: &Value, access_status: &str) -> String {
    let organization_id = status["OrganizationId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected an OrganizationId in {status}"))
        .to_string();
    assert!(
        is_organization_id(&organization_id),
        "case {case}: expected an o-<hex> organization id, got {organization_id:?}"
    );
    assert_eq!(
        status["OrganizationAwsServiceAccessStatus"],
        json!(access_status),
        "case {case}: expected service access {access_status}, got {status}"
    );
    if access_status == ACTION_CATALOG[0].1 {
        let deployment = status["SLRDeploymentStatus"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: expected an SLRDeploymentStatus in {status}"));
        assert!(
            deployment == IN_PROGRESS || deployment == SUCCEEDED,
            "case {case}: expected one of {IN_PROGRESS}/{SUCCEEDED}, got {deployment:?}"
        );
        assert_eq!(
            status["AccountStatusList"],
            json!([{
                "AccountId": engine::emulator_account_id(),
                "SLRDeploymentStatus": deployment,
            }]),
            "case {case}: expected the organization's one account to carry {deployment}, got {status}"
        );
    } else {
        assert!(
            status["SLRDeploymentStatus"].is_null(),
            "case {case}: a disabled organization deploys no SLR, got {status}"
        );
        assert_eq!(
            status["AccountStatusList"],
            json!([]),
            "case {case}: a disabled organization has no account deployments, got {status}"
        );
    }
    organization_id
}

/// Sequences of catalog actions applied to a FRESH emulator, differing in which action
/// mints the record, in how many updates are applied and in their ordering. The rule they
/// span: the update is the only writer, it mints the organization id once and keeps it
/// however often access is turned on and off, and the record it leaves behind is the one
/// the next reader reports.
const ACTION_SEQUENCES: [(&str, &[&str]); 7] = [
    ("disable first", &["DISABLE"]),
    ("disable twice", &["DISABLE", "DISABLE"]),
    ("enable then disable", &["ENABLE", "DISABLE"]),
    ("disable then enable", &["DISABLE", "ENABLE"]),
    ("enable thrice", &["ENABLE", "ENABLE", "ENABLE"]),
    (
        "alternating from enabled",
        &["ENABLE", "DISABLE", "ENABLE", "DISABLE"],
    ),
    (
        "alternating from disabled",
        &["DISABLE", "ENABLE", "DISABLE", "ENABLE"],
    ),
];

#[tokio::test]
async fn every_update_sequence_leaves_one_record_the_reader_reports() {
    for (case, actions) in ACTION_SEQUENCES {
        let emulator = Emulator::new();
        let mut minted: Option<String> = None;
        for action in actions {
            let written = start_update_ok(&emulator, case, action).await;
            let organization_id = assert_access(case, &written, expected_access_status(action));
            // The id is minted by the FIRST update, whichever action that is, and every
            // later update of the same organization keeps it.
            match &minted {
                Some(held) => assert_eq!(
                    &organization_id, held,
                    "case {case}: {action} must keep the organization id {held}, got {written}"
                ),
                None => minted = Some(organization_id.clone()),
            }
            // The reader reports the record this update wrote, not a fresh one.
            let read_back = reported_status(&emulator, case).await;
            assert_eq!(
                assert_access(case, &read_back, expected_access_status(action)),
                organization_id,
                "case {case}: after {action} the reader must report the stored record, got {read_back}"
            );
        }
    }
}

#[tokio::test]
async fn repeating_enable_while_the_deployment_is_in_flight_does_not_restart_it() {
    let emulator = Emulator::new();
    let started = std::time::Instant::now();
    let first = start_update_ok(&emulator, "in-flight first", "ENABLE").await;
    let organization_id = assert_status(
        "in-flight first",
        &first,
        ACTION_CATALOG[0].1,
        Some(IN_PROGRESS),
    );

    // Retry the enabling call again and again while the deployment the first call started
    // is still owed. Each retry reports the SAME organization and the same enabled access:
    // nothing is re-minted.
    let retry_window = PAST_SETTLE + std::time::Duration::from_millis(100);
    let mut retries = 0_u32;
    while started.elapsed() < retry_window {
        let retried = start_update_ok(&emulator, "in-flight retry", "ENABLE").await;
        assert_eq!(
            assert_access("in-flight retry", &retried, ACTION_CATALOG[0].1),
            organization_id,
            "case in-flight retry: a retry must keep the organization id, got {retried}"
        );
        retries += 1;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(
        retries > 1,
        "case in-flight retry: the retry window must cover more than one retry, got {retries}"
    );

    // The deployment the FIRST call started was still due at its own instant, so by now it
    // has succeeded: had any retry restarted it, the last one — taken less than a settle
    // span ago — would still be reported IN_PROGRESS.
    let settled = reported_status(&emulator, "in-flight settled").await;
    assert_eq!(
        assert_status(
            "in-flight settled",
            &settled,
            ACTION_CATALOG[0].1,
            Some(SUCCEEDED)
        ),
        organization_id,
        "case in-flight settled: the organization id must not change, got {settled}"
    );
}

#[tokio::test]
async fn action_outside_the_documented_catalog_is_refused_and_writes_nothing() {
    let emulator = Emulator::new();

    // Every value of the catalog is accepted, in both orders, and each leaves its own
    // access status behind.
    for (action, expected) in ACTION_CATALOG {
        let reported = start_update_ok(&emulator, action, action).await;
        let slr_status = (expected == "ENABLED").then_some(IN_PROGRESS);
        assert_status(action, &reported, expected, slr_status);
        let read_back = reported_status(&emulator, action).await;
        assert_status(action, &read_back, expected, slr_status);
    }
    // The record the refusals below must leave untouched: the last accepted action.
    let held = reported_status(&emulator, "before refusals").await;

    // Values differing from the catalog in case, in surrounding whitespace, in length — at
    // the modelled `Action` bound of 50 characters and one past it — and in being another
    // word entirely. `names_the_member` is false only where the modelled length bound is
    // what refuses the value, because the boundary validator reports the wire member name
    // rather than this service's `Fields` entry.
    let refused: [(&str, &str, bool); 10] = [
        ("lowercase", "enable", true),
        ("mixed case", "Enable", true),
        ("trailing space", "ENABLE ", true),
        ("leading space", " DISABLE", true),
        ("empty", "", true),
        ("the status, not the action", "ENABLED", true),
        ("another word", "DELETE", true),
        ("punctuation", "ENABLE,DISABLE", true),
        ("at the modelled length bound", "A", true),
        ("one past the modelled length bound", "A", false),
    ];
    for (case, seed, names_the_member) in refused {
        // The last two rows are the length bound itself: 50 characters is inside the
        // modelled `length 0..50` and refused by the catalog, 51 is refused by the model.
        let action = match case {
            "at the modelled length bound" => seed.repeat(50),
            "one past the modelled length bound" => seed.repeat(51),
            _ => seed.to_string(),
        };
        let (status, body) = start_update(&emulator, &action).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400 for Action {action:?}, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case}: expected ValidationException for Action {action:?}, got {body}"
        );
        if names_the_member {
            assert_eq!(
                body["Fields"][0]["Name"],
                json!("Action"),
                "case {case}: the refusal must name the Action member, got {body}"
            );
        }
        let after = reported_status(&emulator, case).await;
        assert_eq!(
            after, held,
            "case {case}: a refused update must write nothing, got {after}"
        );
    }
}

#[tokio::test]
async fn paging_bounds_and_tokens_are_enforced_over_the_account_list() {
    let emulator = Emulator::new();
    start_update_ok(&emulator, "setup", "ENABLE").await;
    let whole = reported_status(&emulator, "no paging").await;

    // `MaxResults` is modelled `range 1..500`: each published bound is accepted and one
    // past each is refused. The organization holds one account, so every accepted page
    // carries it and none is followed by a token.
    for max_results in [1, 2, 499, 500] {
        let (status, body) = list_status(&emulator, &format!("maxResults={max_results}")).await;
        assert_eq!(
            status, 200,
            "case maxResults={max_results}: expected 200, got {status} with {body}"
        );
        assert!(
            body["NextToken"].is_null(),
            "case maxResults={max_results}: one account fits one page, got {body}"
        );
        assert_eq!(
            body["OrganizationStatus"], whole,
            "case maxResults={max_results}: expected the whole record, got {body}"
        );
    }
    for max_results in ["0", "-1", "501", "1000"] {
        let (status, body) = list_status(&emulator, &format!("maxResults={max_results}")).await;
        assert_eq!(
            status, 400,
            "case maxResults={max_results}: expected 400 outside the modelled range 1..500, got {status} with {body}"
        );
    }

    // An EMPTY `NextToken` satisfies the modelled `length 0..2048` and can only name the
    // first page, so it is read as "no token" rather than refused.
    let (status, body) = list_status(&emulator, "nextToken=").await;
    assert_eq!(
        status, 200,
        "case empty token: expected 200, got {status} with {body}"
    );
    assert_eq!(
        body["OrganizationStatus"], whole,
        "case empty token: expected the first page, got {body}"
    );
    assert!(
        body["NextToken"].is_null(),
        "case empty token: one account fits one page, got {body}"
    );

    // A `NextToken` this service did not mint is refused rather than silently ignored.
    // The rows differ in alphabet, in length (including the modelled 2048-character bound
    // and one past it) and in whether they decode as hex at all.
    let tokens: [(&str, String); 5] = [
        ("not hex", "not-a-token".to_string()),
        ("odd-length hex", "abc".to_string()),
        // Hex of a string this service never prefixes its tokens with.
        ("hex of foreign text", "6e6f7065".to_string()),
        ("at the modelled length bound", "a".repeat(2048)),
        ("one past the modelled length bound", "a".repeat(2049)),
    ];
    for (case, token) in tokens {
        let (status, body) = list_status(&emulator, &format!("nextToken={token}")).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400 for a token this service did not issue, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case}: expected ValidationException, got {body}"
        );
    }

    // None of those refusals disturbed the record.
    let after = reported_status(&emulator, "after refusals").await;
    assert_eq!(
        after, whole,
        "case after refusals: a refused listing must change nothing, got {after}"
    );
}
