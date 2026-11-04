// SPDX-License-Identifier: CC0-1.0

//! `ListRecoveryPointsByLegalHold`: which recovery points a legal hold is on,
//! driven as real HTTP through the router.
//!
//! The rules under test:
//!
//! * a hold is ON the stored recovery points its stored `RecoveryPointSelection`
//!   names: each member the selection states (`VaultNames`,
//!   `ResourceIdentifiers`, `DateRange`) narrows the set, one it states none of
//!   restricts nothing, and a recovery point created after the hold was placed
//!   is on none of them;
//! * what a hold covers is what it RESTRAINS: while the hold is `ACTIVE`,
//!   deleting a recovery point it is on is refused and deletes nothing, while a
//!   point it does not cover still deletes, and a cancellation releases what the
//!   hold held;
//! * a page is `MaxResults` of those recovery points plus an opaque cursor to
//!   the next one, and walking the cursors visits every held point exactly once;
//! * the read changes nothing a later reader sees;
//! * a request outside what the operation admits — a page size outside the
//!   modelled range, a cursor from no result set, a `LegalHoldId` the account
//!   holds nothing under — is refused with the modelled
//!   `InvalidParameterValueException`, which with the missing-parameter one is
//!   the only client error this operation models (`smithy_query` operation
//!   `ListRecoveryPointsByLegalHold`).
//!
//! The rows of the coverage table differ in which members the selection states,
//! how many values each names, whether those values name anything the account
//! holds, and whether the criteria intersect — which is what the coverage rule
//! generalises over.
//!
//! The request harness (`router_with_engine`, `call`, `error_code`) and the
//! recovery-point setup (`start_point`, `age_point`) are the ones the sibling
//! suites in this test binary expose, so there is ONE wire path and ONE
//! recovery-point seam for the service's suites.

use std::collections::BTreeSet;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::recovery_point_deletion::{age_point, listed_points, router_with_engine, start_point};
use super::vault_and_backup_job::{
    call, create_vault, epoch_seconds, error_code, string_at, ACCOUNT, PAST_SETTLE, REGION,
};

/// Seconds in a day — the unit the fixture ages a stored recovery point by, and
/// the unit a `DateRange` bound is derived from.
const SECONDS_PER_DAY: i64 = 24 * 60 * 60;

/// The vaults the fixture stores recovery points in.
const PRIMARY_VAULT: &str = "hold-primary";
const SECOND_VAULT: &str = "hold-second";

/// How many days before now the fixture's aged recovery point was created, so a
/// `DateRange` can select it and leave the fresh ones out.
const AGED_DAYS: u64 = 15;

/// The ends of the modelled `MaxResults` range (`smithy_query` shape
/// `MaxResults`: `@range` 1..1000).
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 1000;

/// The error shape every refusal of this operation names.
const INVALID_PARAMETER: &str = "InvalidParameterValueException";

/// The sentence this service states for a member whose value it cannot accept,
/// which it words once for every such member (`crate::state`'s
/// `rejected_parameter`).
const INVALID_LEGAL_HOLD_ID: &str = "Parameter LegalHoldId is invalid.";

/// A legal hold id of the form this service mints that no fresh account holds.
const UNHELD_HOLD_ID: &str = "00000000-0000-0000-0000-000000000000";

/// Longer than a legal hold's settle span (`crate::state`'s
/// `LEGAL_HOLD_SETTLE_AFTER`), so a call after this wait sees the hold in the
/// `ACTIVE` status the clock has already made due.
///
/// A blocking sleep rather than `tokio::time::sleep`, for the reason given on
/// `vault_and_backup_job::PAST_SETTLE`.
const PAST_LEGAL_HOLD_SETTLE: Duration = Duration::from_millis(3_400);

/// The `LegalHoldStatus` a cancelled hold reports (`smithy_query` shape
/// `LegalHoldStatus`: `CREATING | ACTIVE | CANCELING | CANCELED`).
const STATUS_CANCELED: &str = "CANCELED";

/// The error shape a restraint on deleting a stored recovery point names
/// (`smithy_query` operation `DeleteRecoveryPoint`: `InvalidRequestException`,
/// `httpError` 400).
const INVALID_REQUEST: &str = "InvalidRequestException";

/// The sentence AWS states for "an active legal hold is on this recovery point"
/// (`crate::state`'s `MESSAGE_RECOVERY_POINT_HELD`) — fixed text naming the
/// condition, not the request.
const RECOVERY_POINT_HELD: &str =
    "The recovery point cannot be deleted because it has active legal hold(s).";

/// `POST /legal-holds` — `CreateLegalHold`'s HTTP binding.
async fn create_legal_hold(router: &MultiServiceRouter, request: &Value) -> (u16, Value) {
    call(router, "POST", "/legal-holds", Some(&request.to_string())).await
}

/// `GET /legal-holds/{LegalHoldId}/recovery-points` —
/// `ListRecoveryPointsByLegalHold`'s HTTP binding, with the query string the
/// caller's page parameters bind to.
pub(crate) async fn held_points(
    router: &MultiServiceRouter,
    hold_id: &str,
    query: &str,
) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/legal-holds/{hold_id}/recovery-points{query}"),
        None,
    )
    .await
}

/// `DELETE /legal-holds/{LegalHoldId}` — `CancelLegalHold`'s HTTP binding, with
/// the query string its `cancelDescription` and `retainRecordInDays` bind to.
async fn cancel_legal_hold(
    router: &MultiServiceRouter,
    hold_id: &str,
    query: &str,
) -> (u16, Value) {
    call(
        router,
        "DELETE",
        &format!("/legal-holds/{hold_id}{query}"),
        None,
    )
    .await
}

/// `DELETE /backup-vaults/{BackupVaultName}/recovery-points/{RecoveryPointArn}`
/// — `DeleteRecoveryPoint`'s HTTP binding.
async fn delete_point(router: &MultiServiceRouter, vault: &str, arn: &str) -> (u16, Value) {
    call(
        router,
        "DELETE",
        &format!("/backup-vaults/{vault}/recovery-points/{arn}"),
        None,
    )
    .await
}

/// The `RecoveryPoints` array of a page.
fn listed(body: &Value) -> Vec<Value> {
    body.get("RecoveryPoints")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The `RecoveryPointArn`s a page reports, as a set.
pub(crate) fn arns_of(page: &Value) -> BTreeSet<String> {
    listed(page)
        .iter()
        .map(|point| string_at(point, "RecoveryPointArn"))
        .collect()
}

/// The ARNs named, as the set a page is compared against.
fn set_of<'a>(arns: impl IntoIterator<Item = &'a String>) -> BTreeSet<String> {
    arns.into_iter().cloned().collect()
}

/// Seconds since the epoch `days` before now — the Unix form the model's
/// timestamps take, which a `DateRange` bound is sent as.
///
/// Whole days back, so the bound is nowhere near the creation instant of any
/// point the fixture has just stored. A bound that ends at the CURRENT instant
/// is [`now_epoch_seconds`] instead, for the reason stated there.
fn days_ago(days: u64) -> i64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("a clock after the epoch")
        .as_secs() as i64;
    now - i64::try_from(days).unwrap_or_default() * SECONDS_PER_DAY
}

/// Now as fractional seconds since the epoch — the form a `DateRange` bound
/// takes when it has to name the current instant rather than a day boundary.
///
/// A window closes on an INSTANT, "accurate to milliseconds" (`smithy_query`
/// shape `DateRange`), so a bound truncated to a whole second is EARLIER than
/// every instant inside that second: it would fall before the sub-second
/// creation instants of the points the fixture stored moments ago, which the
/// hold would then rightly leave out. Stated at the precision the bound
/// carries, through the harness's own conversion, the window provably contains
/// them whatever the clock reads.
fn now_epoch_seconds() -> f64 {
    epoch_seconds(SystemTime::now())
}

/// Create a legal hold over `selection` and return the id it was stored under.
pub(crate) async fn hold_over(router: &MultiServiceRouter, title: &str, selection: Value) -> String {
    let (status, created) = create_legal_hold(
        router,
        &json!({
            "Title": title,
            "Description": "the hold whose coverage is read back",
            "RecoveryPointSelection": selection,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup {title:?}: expected CreateLegalHold 200, got {status} {created:?}"
    );
    string_at(&created, "LegalHoldId")
}

/// The recovery points every case is read against: two in one vault (one of them
/// created [`AGED_DAYS`] ago) and one in another, each backing up a protected
/// resource of a different service, all settled before any hold is placed.
struct Fixture {
    router: MultiServiceRouter,
    /// The aged point in [`PRIMARY_VAULT`].
    aged: String,
    /// The fresh point in [`PRIMARY_VAULT`].
    fresh: String,
    /// The point in [`SECOND_VAULT`].
    other_vault: String,
    aged_resource: String,
    fresh_resource: String,
    other_resource: String,
}

impl Fixture {
    async fn new() -> Self {
        let (router, engine) = router_with_engine();
        for vault in [PRIMARY_VAULT, SECOND_VAULT] {
            let (status, created) = create_vault(&router, vault, "{}").await;
            assert_eq!(
                status, 200,
                "setup: expected CreateBackupVault 200 for {vault:?}, got {status} {created:?}"
            );
        }
        let aged_resource = format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/ledger");
        let fresh_resource = "arn:aws:s3:::held-bucket".to_string();
        let other_resource = format!("arn:aws:ec2:{REGION}:{ACCOUNT}:volume/vol-0abc");
        let aged = start_point(&router, PRIMARY_VAULT, &aged_resource, None).await;
        let fresh = start_point(&router, PRIMARY_VAULT, &fresh_resource, None).await;
        let other_vault = start_point(&router, SECOND_VAULT, &other_resource, None).await;
        age_point(&engine, &aged, AGED_DAYS);
        // Every fixture point's backup has finished before a hold is placed, as
        // it has in the flow a hold is placed in.
        std::thread::sleep(PAST_SETTLE);
        Self {
            router,
            aged,
            fresh,
            other_vault,
            aged_resource,
            fresh_resource,
            other_resource,
        }
    }
}

/// A hold covers the stored recovery points its selection names, member by
/// member: the table spans each selection member on its own, several values of
/// one member, the members combined (including a combination nothing satisfies)
/// and values naming nothing the account holds.
#[tokio::test]
async fn a_hold_covers_the_recovery_points_its_selection_names() {
    let fixture = Fixture::new().await;
    let aged = &fixture.aged;
    let fresh = &fixture.fresh;
    let other_vault = &fixture.other_vault;

    let cases: Vec<(&str, Value, BTreeSet<String>)> = vec![
        (
            "one vault named",
            json!({"VaultNames": [PRIMARY_VAULT]}),
            set_of([aged, fresh]),
        ),
        (
            "both vaults named",
            json!({"VaultNames": [PRIMARY_VAULT, SECOND_VAULT]}),
            set_of([aged, fresh, other_vault]),
        ),
        (
            "the other vault alone",
            json!({"VaultNames": [SECOND_VAULT]}),
            set_of([other_vault]),
        ),
        (
            "a vault the account does not hold",
            json!({"VaultNames": ["no-such-vault"]}),
            BTreeSet::new(),
        ),
        (
            "one protected resource named",
            json!({"ResourceIdentifiers": [fixture.fresh_resource]}),
            set_of([fresh]),
        ),
        (
            "two protected resources in different vaults",
            json!({"ResourceIdentifiers": [fixture.aged_resource, fixture.other_resource]}),
            set_of([aged, other_vault]),
        ),
        (
            "a protected resource nothing was backed up for",
            json!({"ResourceIdentifiers": ["arn:aws:s3:::never-backed-up"]}),
            BTreeSet::new(),
        ),
        (
            "a window around the aged point only",
            json!({"DateRange": {
                "FromDate": days_ago(AGED_DAYS + 5),
                "ToDate": days_ago(AGED_DAYS - 5),
            }}),
            set_of([aged]),
        ),
        (
            "a window reaching from before the aged point up to now",
            json!({"DateRange": {
                "FromDate": days_ago(AGED_DAYS + 5),
                "ToDate": now_epoch_seconds(),
            }}),
            set_of([aged, fresh, other_vault]),
        ),
        (
            "a window closed before anything was backed up",
            json!({"DateRange": {"FromDate": days_ago(90), "ToDate": days_ago(60)}}),
            BTreeSet::new(),
        ),
        (
            "a vault and a window intersecting in one point",
            json!({
                "VaultNames": [PRIMARY_VAULT],
                "DateRange": {
                    "FromDate": days_ago(AGED_DAYS + 5),
                    "ToDate": days_ago(AGED_DAYS - 5),
                },
            }),
            set_of([aged]),
        ),
        (
            "a vault and a resource intersecting in nothing",
            json!({
                "VaultNames": [PRIMARY_VAULT],
                "ResourceIdentifiers": [fixture.other_resource],
            }),
            BTreeSet::new(),
        ),
        (
            "every member at once",
            json!({
                "VaultNames": [PRIMARY_VAULT, SECOND_VAULT],
                "ResourceIdentifiers": [
                    fixture.aged_resource,
                    fixture.fresh_resource,
                    fixture.other_resource,
                ],
                "DateRange": {
                    "FromDate": days_ago(AGED_DAYS + 5),
                    "ToDate": now_epoch_seconds(),
                },
            }),
            set_of([aged, fresh, other_vault]),
        ),
    ];

    for (case, selection, expected) in cases {
        let id = hold_over(&fixture.router, case, selection).await;
        let (status, page) = held_points(&fixture.router, &id, "").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListRecoveryPointsByLegalHold 200, got {status} {page:?}"
        );
        assert_eq!(
            arns_of(&page),
            expected,
            "case {case:?}: expected the hold to cover exactly those recovery points, \
             got {page:?}"
        );
        assert!(
            page.get("NextToken").is_none(),
            "case {case:?}: a complete page carries no cursor, got {page:?}"
        );
        // The read changes nothing a later reader sees.
        let (status, again) = held_points(&fixture.router, &id, "").await;
        assert_eq!(
            (status, arns_of(&again)),
            (200, expected),
            "case {case:?}: two reads of one hold must agree, got {status} {again:?}"
        );
    }
}

/// Every member the model declares for a held recovery point is reported from
/// the stored point, and a recovery point created AFTER the hold was placed is
/// not one the hold is on.
#[tokio::test]
async fn a_held_point_reports_its_stored_members_and_a_later_point_is_not_held() {
    let fixture = Fixture::new().await;
    let id = hold_over(
        &fixture.router,
        "whole vault",
        json!({"VaultNames": [PRIMARY_VAULT]}),
    )
    .await;

    let (status, page) = held_points(&fixture.router, &id, "").await;
    assert_eq!(
        status, 200,
        "expected ListRecoveryPointsByLegalHold 200, got {status} {page:?}"
    );
    let reported = listed(&page);
    assert_eq!(
        reported.len(),
        2,
        "the hold covers the two points stored in its vault, got {page:?}"
    );
    for point in &reported {
        let arn = string_at(point, "RecoveryPointArn");
        let aged = arn == fixture.aged;
        let expected_resource = if aged {
            &fixture.aged_resource
        } else {
            &fixture.fresh_resource
        };
        // `ResourceType` is derived from the protected resource's ARN by the one
        // projection every recovery-point read answers from.
        let expected_type = if aged { "DynamoDB" } else { "S3" };
        assert_eq!(
            (
                string_at(point, "ResourceArn").as_str(),
                string_at(point, "BackupVaultName").as_str(),
                string_at(point, "ResourceType").as_str()
            ),
            (expected_resource.as_str(), PRIMARY_VAULT, expected_type),
            "case {arn:?}: expected the stored point's members, got {point:?}"
        );
    }

    // A point backed up into the same vault after the hold was placed is not one
    // the hold is on: the hold was placed on what its selection named then.
    std::thread::sleep(Duration::from_millis(20));
    let later = start_point(
        &fixture.router,
        PRIMARY_VAULT,
        &format!("arn:aws:rds:{REGION}:{ACCOUNT}:db:later"),
        None,
    )
    .await;
    std::thread::sleep(PAST_SETTLE);
    let (status, page) = held_points(&fixture.router, &id, "").await;
    assert_eq!(
        (status, arns_of(&page).contains(&later)),
        (200, false),
        "a recovery point created after the hold was placed is not held, got {status} {page:?}"
    );
    assert_eq!(
        arns_of(&page),
        set_of([&fixture.aged, &fixture.fresh]),
        "the hold still covers exactly the points it was placed on, got {page:?}"
    );
}

/// The held points are paged: a page is at most `MaxResults` of them plus a
/// cursor to the next, and walking the cursors visits every held point exactly
/// once — over page sizes smaller than, equal to and larger than the set.
#[tokio::test]
async fn the_recovery_points_of_a_hold_are_paged_with_a_cursor() {
    let fixture = Fixture::new().await;
    let id = hold_over(
        &fixture.router,
        "every vault",
        json!({"VaultNames": [PRIMARY_VAULT, SECOND_VAULT]}),
    )
    .await;
    let whole = set_of([&fixture.aged, &fixture.fresh, &fixture.other_vault]);

    for page_size in [1_usize, 2, 3, 4] {
        let mut seen: Vec<String> = Vec::new();
        let mut query = format!("?maxResults={page_size}");
        loop {
            let (status, page) = held_points(&fixture.router, &id, &query).await;
            assert_eq!(
                status, 200,
                "case page size {page_size}: expected 200, got {status} {page:?}"
            );
            let points = listed(&page);
            assert!(
                points.len() <= page_size,
                "case page size {page_size}: a page reports at most that many, got {page:?}"
            );
            seen.extend(
                points
                    .iter()
                    .map(|point| string_at(point, "RecoveryPointArn")),
            );
            match page.get("NextToken").and_then(Value::as_str) {
                Some(token) => query = format!("?maxResults={page_size}&nextToken={token}"),
                None => break,
            }
        }
        let unique: BTreeSet<String> = seen.iter().cloned().collect();
        assert_eq!(
            (seen.len(), unique),
            (whole.len(), whole.clone()),
            "case page size {page_size}: the cursor walk must visit every held point once, \
             got {seen:?}"
        );
    }
}

/// Every request this operation refuses answers its modelled
/// `InvalidParameterValueException`: the operation models no not-found error, so
/// a `LegalHoldId` the account holds nothing under is refused as the value it
/// is, and a refused request reports no recovery points.
#[tokio::test]
async fn a_refused_coverage_read_answers_the_modelled_invalid_parameter() {
    let fixture = Fixture::new().await;
    let id = hold_over(
        &fixture.router,
        "refusals",
        json!({"VaultNames": [PRIMARY_VAULT]}),
    )
    .await;
    let cursor_outside =
        encoded_cursor(&format!("arn:aws:backup:{REGION}:{ACCOUNT}:recovery-point:absent"));

    // (case, the hold id read, the query string)
    let cases: Vec<(&str, &str, String)> = vec![
        (
            "a page size one below the modelled minimum",
            id.as_str(),
            format!("?maxResults={}", MAX_RESULTS_MIN - 1),
        ),
        (
            "a page size one above the modelled maximum",
            id.as_str(),
            format!("?maxResults={}", MAX_RESULTS_MAX + 1),
        ),
        (
            "a negative page size",
            id.as_str(),
            "?maxResults=-1".to_string(),
        ),
        (
            "a cursor this service never minted",
            id.as_str(),
            "?nextToken=not-a-cursor".to_string(),
        ),
        (
            "a cursor naming an item outside this result set",
            id.as_str(),
            format!("?nextToken={cursor_outside}"),
        ),
        (
            "a legal hold id the account holds nothing under",
            UNHELD_HOLD_ID,
            String::new(),
        ),
    ];
    for (case, hold_id, query) in cases {
        let (status, refused) = held_points(&fixture.router, hold_id, &query).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, INVALID_PARAMETER),
            "case {case:?}: expected 400 {INVALID_PARAMETER}, got {status} {refused:?}"
        );
        assert!(
            refused.get("RecoveryPoints").is_none(),
            "case {case:?}: a refusal reports no recovery points, got {refused:?}"
        );
    }

    // The refusal for an unheld id names the member a client has to fix.
    let (_, refused) = held_points(&fixture.router, UNHELD_HOLD_ID, "").await;
    assert_eq!(
        string_at(&refused, "Message"),
        INVALID_LEGAL_HOLD_ID,
        "the refusal names the member that named the hold, got {refused:?}"
    );

    // The control: both ends of the modelled page-size range are served.
    for page_size in [MAX_RESULTS_MIN, MAX_RESULTS_MAX] {
        let (status, page) =
            held_points(&fixture.router, &id, &format!("?maxResults={page_size}")).await;
        assert_eq!(
            status, 200,
            "case page size {page_size}: a size the model admits is served, got {status} {page:?}"
        );
    }
}

/// A cancellation RELEASES what the hold held: the hold reports the status,
/// reason and record retention the cancellation recorded, the recovery point it
/// held deletes, and cancelling it again is refused for the state it is now in.
#[tokio::test]
async fn a_canceled_hold_releases_the_recovery_points_it_held() {
    let fixture = Fixture::new().await;
    let id = hold_over(
        &fixture.router,
        "released",
        json!({"ResourceIdentifiers": [fixture.fresh_resource]}),
    )
    .await;
    std::thread::sleep(PAST_LEGAL_HOLD_SETTLE);

    // While the hold is in force the point cannot go.
    let (status, refused) = delete_point(&fixture.router, PRIMARY_VAULT, &fixture.fresh).await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (400, INVALID_REQUEST),
        "setup: the held point's delete must be refused, got {status} {refused:?}"
    );

    let (status, canceled) = cancel_legal_hold(
        &fixture.router,
        &id,
        "?cancelDescription=audit-complete&retainRecordInDays=7",
    )
    .await;
    assert_eq!(
        status, 201,
        "expected CancelLegalHold 201, got {status} {canceled:?}"
    );

    // The hold reports what the cancellation recorded.
    let (status, read) = call(&fixture.router, "GET", &format!("/legal-holds/{id}"), None).await;
    assert_eq!(
        (
            status,
            string_at(&read, "Status").as_str(),
            string_at(&read, "CancelDescription").as_str()
        ),
        (200, STATUS_CANCELED, "audit-complete"),
        "expected the cancelled hold's recorded reason and status, got {status} {read:?}"
    );
    for member in ["CancellationDate", "RetainRecordUntil"] {
        assert!(
            read[member].is_number(),
            "expected the cancellation to record {member}, got {read:?}"
        );
    }

    // Released: the point the hold held now deletes.
    let (status, deleted) = delete_point(&fixture.router, PRIMARY_VAULT, &fixture.fresh).await;
    assert_eq!(
        status, 200,
        "a released point deletes, got {status} {deleted:?}"
    );

    // A hold a cancellation already released is not one another can release.
    let (status, refused) = cancel_legal_hold(
        &fixture.router,
        &id,
        "?cancelDescription=audit-complete-again",
    )
    .await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (400, "InvalidResourceStateException"),
        "expected a second cancellation to be refused for the hold's state, \
         got {status} {refused:?}"
    );

    // A legal hold id the account holds nothing under is not found.
    let (status, refused) = cancel_legal_hold(
        &fixture.router,
        UNHELD_HOLD_ID,
        "?cancelDescription=nothing-to-cancel",
    )
    .await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (400, "ResourceNotFoundException"),
        "expected an unheld id to be refused as absent, got {status} {refused:?}"
    );
}

/// A cursor as this service mints one, from the service's OWN encoder
/// (`crate::state::encode_page_token`), so the test cannot drift from the
/// cursor form the handler hands out.
fn encoded_cursor(key: &str) -> String {
    service_com_amazonaws_backup_cryocontrollerusermanager::state::encode_page_token(key)
}

/// What a hold covers is what it RESTRAINS: once the hold is `ACTIVE`, deleting
/// a recovery point it is on is refused and the point stays stored, while a
/// point the same hold does not cover still deletes.
///
/// "Any actions to delete or disassociate a recovery point will fail with an
/// error if one or more active legal holds are on the recovery point" (model doc
/// for `CreateLegalHold`).
#[tokio::test]
async fn an_active_hold_restrains_deleting_the_recovery_points_it_covers() {
    let fixture = Fixture::new().await;
    let id = hold_over(
        &fixture.router,
        "one resource",
        json!({"ResourceIdentifiers": [fixture.fresh_resource]}),
    )
    .await;
    // The hold is in force only once its creation has settled.
    std::thread::sleep(PAST_LEGAL_HOLD_SETTLE);

    let (status, page) = held_points(&fixture.router, &id, "").await;
    assert_eq!(
        (status, arns_of(&page)),
        (200, set_of([&fixture.fresh])),
        "the hold covers the point of the resource it names, got {status} {page:?}"
    );

    // Deleting the held point is refused, twice over, and deletes nothing.
    for attempt in 0..2 {
        let (status, refused) = delete_point(&fixture.router, PRIMARY_VAULT, &fixture.fresh).await;
        assert_eq!(
            (
                status,
                error_code(&refused).as_str(),
                string_at(&refused, "Message").as_str()
            ),
            (400, INVALID_REQUEST, RECOVERY_POINT_HELD),
            "attempt {attempt}: expected the held point's delete to be refused, \
             got {status} {refused:?}"
        );
    }
    let stored: BTreeSet<String> = listed_points(&fixture.router, PRIMARY_VAULT)
        .await
        .iter()
        .map(|point| string_at(point, "RecoveryPointArn"))
        .collect();
    assert_eq!(
        stored,
        set_of([&fixture.aged, &fixture.fresh]),
        "a refused delete removes nothing, got {stored:?}"
    );

    // The control: a point in the same vault that the hold does not cover is not
    // restrained, so the refusal follows the hold's coverage rather than the
    // vault.
    let (status, deleted) = delete_point(&fixture.router, PRIMARY_VAULT, &fixture.aged).await;
    assert_eq!(
        status, 200,
        "a point no hold is on still deletes, got {status} {deleted:?}"
    );
    let (status, page) = held_points(&fixture.router, &id, "").await;
    assert_eq!(
        (status, arns_of(&page)),
        (200, set_of([&fixture.fresh])),
        "the hold still covers the point it is on, got {status} {page:?}"
    );
}
