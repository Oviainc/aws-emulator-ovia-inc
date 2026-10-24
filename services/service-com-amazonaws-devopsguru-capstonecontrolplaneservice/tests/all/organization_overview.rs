// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DescribeOrganizationOverview`.
//!
//! The harness (`Emulator`, `Reply`, the organization-administration seam and the
//! insight seeder) is the one `configuration_lifecycle` already provides: one buffered
//! HTTP request through the same [`smithy_server_runtime::MultiServiceRouter`] the
//! emulator boots, so every assertion is about wire behaviour.
//!
//! No modelled operation MINTS an insight — DevOps Guru derives one from anomalies in the
//! covered resources — so a test that needs insights seeds the store's `insights` map the
//! way that analysis would and then asks the service over HTTP.
//!
//! RULES under test:
//!
//! 1. An organization-wide overview belongs to the account that administers DevOps Guru
//!    for that organization, so any other caller is refused with the operation's HTTP 403
//!    `AccessDeniedException` — which is every caller a client of this emulator can
//!    present, because nothing registers an administrator.
//! 2. For that administering caller the overview counts the OPEN insights of the scoped
//!    member accounts, by `InsightType`, among those CREATED inside the requested range,
//!    whose two bounds are DAYS: each is floored to the start of its UTC day and the
//!    `ToTime` day counts in full, with an absent `ToTime` meaning the current day.
//! 3. `AccountIds` and `OrganizationalUnitIds` NARROW that overview to the member
//!    accounts they name; a filter naming none restricts nothing.
//! 4. The overview is a pure read: it leaves the store exactly as it was.

use serde_json::{json, Value};

use service_com_amazonaws_devopsguru_capstonecontrolplaneservice::state::{
    now_millis, InsightRecord, MILLIS_PER_DAY,
};

use crate::configuration_lifecycle::{Emulator, Reply};

/// `DescribeOrganizationOverview`'s HTTP binding, from
/// `smithy_query(query="operation",operation="DescribeOrganizationOverview")`:
/// `POST /organization/overview` with every member in the body.
async fn describe_overview(emulator: &Emulator, body: &Value) -> Reply {
    emulator.call("POST", "/organization/overview", Some(body)).await
}

/// One hour, in milliseconds — the step inside a day the rows are placed at.
const HOUR_MILLIS: i64 = 3_600_000;

/// A UTC midnight well inside the range a `Timestamp` can carry: 2023-11-14T00:00:00Z, in
/// epoch milliseconds. It is an exact multiple of [`MILLIS_PER_DAY`], so every row below
/// is placed on a known day boundary and no row depends on the clock.
const DAY_0: i64 = 1_699_920_000_000;

/// `InsightType`'s and `InsightStatus`'s values, from
/// `smithy_query(query="shape",shape="InsightType,InsightStatus")`.
const PROACTIVE: &str = "PROACTIVE";
const REACTIVE: &str = "REACTIVE";
const ONGOING: &str = "ONGOING";
const CLOSED: &str = "CLOSED";

/// A timestamp as restJson1 reads one: epoch SECONDS.
fn epoch_seconds(millis: i64) -> f64 {
    millis as f64 / 1_000.0
}

/// One seeded insight: the members these rows vary — type, status and the instant the
/// insight was created — with the rest left at a fixed value, so a difference in the
/// counts can only come from these.
fn insight(id: &str, insight_type: &str, status: &str, started_millis: i64) -> InsightRecord {
    InsightRecord {
        id: id.to_string(),
        name: format!("DevOps Guru insight {id}"),
        insight_type: insight_type.to_string(),
        severity: "HIGH".to_string(),
        status: status.to_string(),
        start_time_millis: started_millis,
        // A closed insight's behaviour ended an hour after it started; an ongoing one has
        // no end.
        end_time_millis: (status == CLOSED).then_some(started_millis + HOUR_MILLIS),
        ..InsightRecord::default()
    }
}

/// The insights every counting row is asserted against: both types, both statuses, and
/// creation instants spread over a known grid of days and hours within them.
fn fixture() -> Vec<InsightRecord> {
    vec![
        insight("p-day0-early", PROACTIVE, ONGOING, DAY_0 + HOUR_MILLIS),
        insight("r-day0-closed", REACTIVE, CLOSED, DAY_0 + 2 * HOUR_MILLIS),
        insight(
            "r-day1-morning",
            REACTIVE,
            ONGOING,
            DAY_0 + MILLIS_PER_DAY + 3 * HOUR_MILLIS,
        ),
        insight(
            "p-day1-closed",
            PROACTIVE,
            CLOSED,
            DAY_0 + MILLIS_PER_DAY + 5 * HOUR_MILLIS,
        ),
        insight(
            "p-day1-noon",
            PROACTIVE,
            ONGOING,
            DAY_0 + MILLIS_PER_DAY + 12 * HOUR_MILLIS,
        ),
        insight(
            "r-day2-last-hour",
            REACTIVE,
            ONGOING,
            DAY_0 + 2 * MILLIS_PER_DAY + 23 * HOUR_MILLIS,
        ),
        insight(
            "p-day5-midnight",
            PROACTIVE,
            ONGOING,
            DAY_0 + 5 * MILLIS_PER_DAY,
        ),
    ]
}

/// The two counts one reply reports, as a client reads them. Both members are modelled
/// required, so an absent one is a failure rather than a zero.
fn counts(reply: &Reply) -> (Option<i64>, Option<i64>) {
    (
        reply.at("/ProactiveInsights").and_then(Value::as_i64),
        reply.at("/ReactiveInsights").and_then(Value::as_i64),
    )
}

/// The account this emulator serves, which is the one member account whose insights the
/// store holds. `AwsAccountId` is modelled `^\d{12}$`.
fn served_account() -> String {
    engine::emulator_account_id().to_string()
}

/// Twelve-digit account ids that are NOT the served account — `AwsAccountId`'s pattern,
/// from `smithy_query(query="shape",shape="AccountIdList")`.
const OTHER_ACCOUNTS: [&str; 4] = ["210987654321", "111122223333", "444455556666", "777788889999"];

/// Organizational unit ids matching `OrganizationalUnitId`'s pattern
/// `^ou-[0-9a-z]{4,32}-[a-z0-9]{8,32}$`, from
/// `smithy_query(query="shape",shape="OrganizationalUnitIdList")`.
const ORGANIZATIONAL_UNITS: [&str; 2] = ["ou-abc1-12345678", "ou-9xyz-abcdef0123456789"];

/// An emulator whose organization is administered by the account it serves, holding the
/// fixture insights: the only state in which the REPORTING path of an organization-wide
/// read is reachable.
fn administered_emulator() -> Emulator {
    let emulator = Emulator::new();
    emulator.delegate_organization_administration(&served_account());
    emulator.seed_insights(fixture());
    emulator
}

/// RULE 1: an organization-wide overview is available to the account that administers
/// DevOps Guru for the organization alone, so a caller that administers none is refused
/// with HTTP 403 `AccessDeniedException` — whatever time range or filters the request
/// carries, and whether or not the store holds insights to count.
///
/// The rows differ along the time range, whether `ToTime` is sent, which filters are sent
/// and whether the store was seeded.
#[tokio::test]
async fn an_overview_is_refused_to_a_caller_that_administers_no_organization() {
    // (case, seed the store, request body)
    let cases: Vec<(&str, bool, Value)> = vec![
        (
            "the simplest well-formed request",
            false,
            json!({ "FromTime": epoch_seconds(DAY_0) }),
        ),
        (
            "a store that holds insights to count",
            true,
            json!({ "FromTime": epoch_seconds(DAY_0) }),
        ),
        (
            "an explicit time range",
            true,
            json!({
                "FromTime": epoch_seconds(DAY_0),
                "ToTime": epoch_seconds(DAY_0 + 5 * MILLIS_PER_DAY),
            }),
        ),
        (
            "the served account named explicitly",
            true,
            json!({
                "FromTime": epoch_seconds(DAY_0),
                "AccountIds": [served_account()],
            }),
        ),
        (
            "an organizational unit filter",
            true,
            json!({
                "FromTime": epoch_seconds(DAY_0),
                "OrganizationalUnitIds": [ORGANIZATIONAL_UNITS[0]],
            }),
        ),
        (
            "a range in which nothing was created",
            true,
            json!({
                "FromTime": epoch_seconds(DAY_0 - 30 * MILLIS_PER_DAY),
                "ToTime": epoch_seconds(DAY_0 - 29 * MILLIS_PER_DAY),
            }),
        ),
    ];

    for (case, seed, body) in cases {
        let emulator = Emulator::new();
        if seed {
            emulator.seed_insights(fixture());
        }

        let reply = describe_overview(&emulator, &body).await;
        assert_eq!(
            reply.status, 403,
            "case {case:?}: expected HTTP 403 for a caller that administers no organization, got {} {}",
            reply.status, reply.raw
        );
        assert!(
            reply.names_error("AccessDeniedException"),
            "case {case:?}: expected AccessDeniedException, got {}",
            reply.raw
        );
        // The refusal reports nothing about what the organization holds.
        assert_eq!(
            counts(&reply),
            (None, None),
            "case {case:?}: a refusal must carry no counts, got {}",
            reply.raw
        );
    }
}

/// RULE 2: the overview counts the OPEN insights of the store, by `InsightType`, among
/// those whose creation instant lies in the requested range — a range of whole UTC DAYS,
/// each bound floored to the start of its day and the `ToTime` day counted in full.
///
/// The rows differ along which days the range spans, how many days that is, where inside
/// its day each bound falls, which types and statuses the range catches, and whether the
/// range holds any instant at all.
#[tokio::test]
async fn the_counts_are_the_open_insights_created_in_the_requested_day_range() {
    // (case, FromTime, ToTime, expected open proactive, expected open reactive)
    let cases: Vec<(&str, i64, Option<i64>, i64, i64)> = vec![
        ("the from day alone", DAY_0, Some(DAY_0), 1, 0),
        (
            "a FromTime late in its day still starts at that day's floor",
            DAY_0 + 23 * HOUR_MILLIS,
            Some(DAY_0 + 23 * HOUR_MILLIS),
            1,
            0,
        ),
        (
            "the second day alone: one open insight of each type, the closed one not counted",
            DAY_0 + MILLIS_PER_DAY,
            Some(DAY_0 + MILLIS_PER_DAY),
            1,
            1,
        ),
        (
            "a ToTime early in its day still counts that whole day",
            DAY_0 + 2 * MILLIS_PER_DAY,
            Some(DAY_0 + 2 * MILLIS_PER_DAY + HOUR_MILLIS),
            0,
            1,
        ),
        (
            "three days at once",
            DAY_0,
            Some(DAY_0 + 2 * MILLIS_PER_DAY),
            2,
            2,
        ),
        (
            "every seeded day",
            DAY_0,
            Some(DAY_0 + 5 * MILLIS_PER_DAY),
            3,
            2,
        ),
        (
            "a day nothing was created on",
            DAY_0 + 3 * MILLIS_PER_DAY,
            Some(DAY_0 + 4 * MILLIS_PER_DAY),
            0,
            0,
        ),
        (
            "a range entirely before the insights",
            DAY_0 - 10 * MILLIS_PER_DAY,
            Some(DAY_0 - MILLIS_PER_DAY),
            0,
            0,
        ),
        (
            "a range whose end precedes its start holds no instant",
            DAY_0 + 2 * MILLIS_PER_DAY,
            Some(DAY_0),
            0,
            0,
        ),
        (
            "an instant before the epoch is a day like any other",
            -3 * MILLIS_PER_DAY,
            Some(-2 * MILLIS_PER_DAY),
            0,
            0,
        ),
    ];

    for (case, from, to, proactive, reactive) in cases {
        let emulator = administered_emulator();
        let mut body = json!({ "FromTime": epoch_seconds(from) });
        if let Some(to) = to {
            body["ToTime"] = json!(epoch_seconds(to));
        }

        let reply = describe_overview(&emulator, &body).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: the overview must succeed for the administering caller, got {} {}",
            reply.status, reply.raw
        );
        assert_eq!(
            counts(&reply),
            (Some(proactive), Some(reactive)),
            "case {case:?}: expected ({proactive}, {reactive}) open proactive/reactive insights, got {}",
            reply.raw
        );
    }
}

/// RULE 2, its default: an absent `ToTime` means "the current day", so an insight created
/// moments ago is counted, while the same request with an explicit `ToTime` two days back
/// ends at that day and counts nothing.
#[tokio::test]
async fn an_absent_to_time_means_the_current_day() {
    let emulator = Emulator::new();
    emulator.delegate_organization_administration(&served_account());
    let created = now_millis() - HOUR_MILLIS;
    emulator.seed_insights(vec![
        insight("open-now", PROACTIVE, ONGOING, created),
        insight("closed-now", REACTIVE, CLOSED, created),
    ]);
    let from = now_millis() - 2 * MILLIS_PER_DAY;

    let open_ended = describe_overview(&emulator, &json!({ "FromTime": epoch_seconds(from) })).await;
    assert_eq!(
        open_ended.status, 200,
        "case \"no ToTime\": the overview must succeed, got {} {}",
        open_ended.status, open_ended.raw
    );
    assert_eq!(
        counts(&open_ended),
        (Some(1), Some(0)),
        "case \"no ToTime\": today's open proactive insight must be counted, got {}",
        open_ended.raw
    );

    let bounded = describe_overview(
        &emulator,
        &json!({ "FromTime": epoch_seconds(from), "ToTime": epoch_seconds(from) }),
    )
    .await;
    assert_eq!(
        counts(&bounded),
        (Some(0), Some(0)),
        "case \"ToTime two days back\": an insight created today is outside that day, got {}",
        bounded.raw
    );
}

/// One row of the filter table: the case name, the `AccountIds` and the
/// `OrganizationalUnitIds` the request sends (`None` meaning the member is absent), and
/// the open proactive and open reactive counts expected.
type FilterCase<'a> = (
    &'a str,
    Option<Vec<String>>,
    Option<Vec<String>>,
    i64,
    i64,
);

/// RULE 3: `AccountIds` and `OrganizationalUnitIds` NARROW the overview to the member
/// accounts they name — an account is in scope exactly while every filter the request
/// carries selects it — and a filter that names nothing restricts nothing. The store
/// records no organizational-unit membership for the account it serves, so a unit filter
/// leaves no account in scope.
///
/// The rows differ along which filter is sent, how many ids it names, whether the served
/// account is among them, and whether the list is empty.
#[tokio::test]
async fn the_account_and_unit_filters_narrow_the_overview() {
    let served = served_account();
    // (case, AccountIds, OrganizationalUnitIds, expected open proactive, open reactive)
    let cases: Vec<FilterCase> = vec![
        ("no filter at all", None, None, 3, 2),
        (
            "AccountIds naming the served account",
            Some(vec![served.clone()]),
            None,
            3,
            2,
        ),
        (
            "AccountIds naming another account",
            Some(vec![OTHER_ACCOUNTS[0].to_string()]),
            None,
            0,
            0,
        ),
        (
            "AccountIds naming four other accounts",
            Some(OTHER_ACCOUNTS.iter().map(ToString::to_string).collect()),
            None,
            0,
            0,
        ),
        (
            "AccountIds naming the served account among others, at the modelled maximum of 5",
            Some(
                OTHER_ACCOUNTS
                    .iter()
                    .map(ToString::to_string)
                    .chain([served.clone()])
                    .collect(),
            ),
            None,
            3,
            2,
        ),
        ("an empty AccountIds list restricts nothing", Some(vec![]), None, 3, 2),
        (
            "OrganizationalUnitIds naming a unit the member belongs to none of",
            None,
            Some(vec![ORGANIZATIONAL_UNITS[0].to_string()]),
            0,
            0,
        ),
        (
            "OrganizationalUnitIds naming two units",
            None,
            Some(ORGANIZATIONAL_UNITS.iter().map(ToString::to_string).collect()),
            0,
            0,
        ),
        (
            "an empty OrganizationalUnitIds list restricts nothing",
            None,
            Some(vec![]),
            3,
            2,
        ),
        (
            "both filters: the account matches, the unit does not",
            Some(vec![served.clone()]),
            Some(vec![ORGANIZATIONAL_UNITS[1].to_string()]),
            0,
            0,
        ),
        (
            "both filters, both naming nothing",
            Some(vec![]),
            Some(vec![]),
            3,
            2,
        ),
    ];

    for (case, account_ids, organizational_unit_ids, proactive, reactive) in cases {
        let emulator = administered_emulator();
        let mut body = json!({
            "FromTime": epoch_seconds(DAY_0),
            "ToTime": epoch_seconds(DAY_0 + 5 * MILLIS_PER_DAY),
        });
        if let Some(account_ids) = account_ids {
            body["AccountIds"] = json!(account_ids);
        }
        if let Some(organizational_unit_ids) = organizational_unit_ids {
            body["OrganizationalUnitIds"] = json!(organizational_unit_ids);
        }

        let reply = describe_overview(&emulator, &body).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: the overview must succeed, got {} {}",
            reply.status, reply.raw
        );
        assert_eq!(
            counts(&reply),
            (Some(proactive), Some(reactive)),
            "case {case:?}: expected ({proactive}, {reactive}), got {}",
            reply.raw
        );
    }
}

/// RULE 4: the overview is a pure read, so reading it changes nothing a later caller
/// sees: repeating the same request answers the same counts, a read of one range does not
/// disturb another's, and the insights themselves still list unchanged afterwards.
#[tokio::test]
async fn reading_the_overview_changes_nothing_a_later_caller_sees() {
    let emulator = administered_emulator();
    let whole_span = json!({
        "FromTime": epoch_seconds(DAY_0),
        "ToTime": epoch_seconds(DAY_0 + 5 * MILLIS_PER_DAY),
    });
    let one_day = json!({
        "FromTime": epoch_seconds(DAY_0 + MILLIS_PER_DAY),
        "ToTime": epoch_seconds(DAY_0 + MILLIS_PER_DAY),
    });

    let first = describe_overview(&emulator, &whole_span).await;
    let narrow = describe_overview(&emulator, &one_day).await;
    let again = describe_overview(&emulator, &whole_span).await;

    assert_eq!(
        counts(&first),
        (Some(3), Some(2)),
        "case \"first read\": expected (3, 2), got {}",
        first.raw
    );
    assert_eq!(
        counts(&narrow),
        (Some(1), Some(1)),
        "case \"narrower range\": expected (1, 1), got {}",
        narrow.raw
    );
    assert_eq!(
        counts(&again),
        counts(&first),
        "case \"repeated read\": expected {:?}, got {}",
        counts(&first),
        again.raw
    );

    // `ListInsights` over the same store: every seeded insight is still there, so no
    // count was consumed and no status was moved by the reads above.
    let listed = emulator
        .call(
            "POST",
            "/insights",
            Some(&json!({ "StatusFilter": { "Any": { "Type": PROACTIVE, "StartTimeRange": {} } } })),
        )
        .await;
    assert_eq!(
        listed.status, 200,
        "case \"listing afterwards\": the listing must succeed, got {} {}",
        listed.status, listed.raw
    );
    let mut ids: Vec<String> = listed
        .at("/ProactiveInsights")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    item.get("Id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default();
    ids.sort();
    assert_eq!(
        ids,
        vec![
            "p-day0-early".to_string(),
            "p-day1-closed".to_string(),
            "p-day1-noon".to_string(),
            "p-day5-midnight".to_string(),
        ],
        "case \"listing afterwards\": every seeded proactive insight must still be stored, got {}",
        listed.raw
    );
}

/// One row of the extreme-bound table: the case name, `FromTime` and the optional
/// `ToTime` in epoch seconds, and the counts expected if the bound is accepted.
type ExtremeCase<'a> = (&'a str, f64, Option<f64>, (i64, i64));

/// RULE: a bound at the extreme of what a `Timestamp` can carry is answered by the
/// modelled contract, never by a fault: the day floor SATURATES at the earliest and
/// latest instant this service keeps, so such a request is either a 200 whose range is
/// the saturated span or the modelled 400 `ValidationException` — never a 5xx and never
/// the scaffold's 501.
///
/// The rows differ along which bound is extreme and in which direction.
#[tokio::test]
async fn a_time_bound_at_the_extreme_is_answered_by_the_modelled_contract() {
    // (case, FromTime in epoch seconds, ToTime in epoch seconds, the counts expected if
    // the bound is accepted). `i64::MIN` milliseconds is about -9_223_372_036_854_775
    // epoch seconds, so these bounds sit just inside the representable range and their
    // day floors are what would overflow an unchecked multiplication.
    let cases: [ExtremeCase; 4] = [
        // The range then starts at the earliest instant and ends at the end of today, so
        // every seeded insight was created inside it.
        (
            "a FromTime at the negative extreme",
            -9_223_372_036_850_000.0,
            None,
            (3, 2),
        ),
        // Both bounds floor to the same earliest day, so the range is that one day.
        (
            "both bounds at the negative extreme",
            -9_223_372_036_850_000.0,
            Some(-9_223_372_036_850_000.0),
            (0, 0),
        ),
        // A range that starts after the last day holds no instant.
        (
            "a FromTime at the positive extreme",
            9_223_372_036_850_000.0,
            None,
            (0, 0),
        ),
        (
            "a ToTime at the positive extreme spans everything from the from day",
            epoch_seconds(DAY_0),
            Some(9_223_372_036_850_000.0),
            (3, 2),
        ),
    ];

    let mut accepted = 0usize;
    for (case, from, to, (proactive, reactive)) in cases {
        let emulator = administered_emulator();
        let mut body = json!({ "FromTime": from });
        if let Some(to) = to {
            body["ToTime"] = json!(to);
        }

        let reply = describe_overview(&emulator, &body).await;
        assert!(
            reply.status == 200
                || (reply.status == 400 && reply.names_error("ValidationException")),
            "case {case:?}: expected HTTP 200 or the modelled 400 ValidationException, got {} {}",
            reply.status,
            reply.raw
        );
        if reply.status == 200 {
            accepted += 1;
            assert_eq!(
                counts(&reply),
                (Some(proactive), Some(reactive)),
                "case {case:?}: expected ({proactive}, {reactive}) over the saturated range, got {}",
                reply.raw
            );
        }
    }

    // The saturating floor is what makes an extreme bound answerable at all, so at least
    // one row must have been ANSWERED rather than refused: otherwise the rows above
    // would assert nothing about it.
    assert!(
        accepted > 0,
        "at least one extreme bound must be answered with a computed overview, none was"
    );
}

/// RULE: `FromTime` is modelled required, so a request without it is refused with the
/// operation's HTTP 400 `ValidationException` — never the scaffold's 501 and never a
/// count over some substituted range.
#[tokio::test]
async fn a_request_without_the_required_from_time_is_refused() {
    for (case, body) in [
        ("no member at all", json!({})),
        (
            "only the filters",
            json!({ "AccountIds": [served_account()] }),
        ),
        (
            "a ToTime but no FromTime",
            json!({ "ToTime": epoch_seconds(DAY_0) }),
        ),
    ] {
        let emulator = administered_emulator();
        let reply = describe_overview(&emulator, &body).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected HTTP 400 for a missing required FromTime, got {} {}",
            reply.status, reply.raw
        );
        assert!(
            reply.names_error("ValidationException"),
            "case {case:?}: expected ValidationException, got {}",
            reply.raw
        );
    }
}
