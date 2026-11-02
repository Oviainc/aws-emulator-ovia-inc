// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListInsights`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees. The harness and the baseline `CreateDomain` body come from
//! [`super::domain_lifecycle`]; this suite adds no second copy of them.
//!
//! The rule under test: a listing reports the insights the entity the request names
//! holds — a domain's own for a `DomainName` entity, and the account's own plus every
//! domain's for an `Account` entity — each as the stored record's values, narrowed to
//! the insights raised inside the requested time range, ordered by the instant each
//! was raised in the requested sort order, in pages of at most `MaxResults` resumed by
//! the token the previous page handed out; an entity holding no insight — including a
//! name this account holds no domain under — reports the empty set, while an entity
//! this service cannot resolve at all and a token naming no insight of the listing are
//! refused rather than answered with a page.
//!
//! So the assertions span a SET of inputs rather than one call: a table of insights
//! covering EVERY value of `InsightType`, `InsightPriorityLevel` and `InsightStatus`
//! the model admits, both values of `IsExperimental`, instants before, at and after
//! the call, and two insights raised at the SAME instant; then both sort orders and
//! the absent one, five page sizes and the unbounded case, the published bounds and
//! values past them at either end, six time ranges, four shapes of unresolvable token
//! and five entities this service resolves nothing for.
//!
//! The insights themselves are written into the store directly (through
//! [`super::domain_lifecycle::Emulator::store`]): the service RAISES an insight from
//! what it observes of a domain, so no wire call leaves one. Everything the rule is
//! asserted ON is read back through the wire.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::ops::types::{
    INSIGHT_ENTITY_TYPE_VALUES, INSIGHT_PRIORITY_LEVEL_VALUES, INSIGHT_SORT_ORDER_VALUES,
    INSIGHT_STATUS_VALUES, INSIGHT_TYPE_VALUES,
};
use service_com_amazonaws_opensearch_amazonopensearchservice::state::InsightRecord;

use super::domain_lifecycle::{create_baseline, error_code, Answer, Emulator, PREFIX};

/// One row of the insight table: the last field of its `InsightId`, its display name,
/// its type, its priority, its status, how far from the base instant it was raised,
/// and whether it is experimental.
struct Row {
    id_suffix: &'static str,
    display_name: &'static str,
    insight_type: &'static str,
    priority: &'static str,
    status: &'static str,
    offset: i64,
    experimental: bool,
}

impl Row {
    /// The `GUID` this row's insight is addressed by: `InsightId` is
    /// `@length(36, 36)` with pattern
    /// `^\p{XDigit}{8}-\p{XDigit}{4}-\p{XDigit}{4}-\p{XDigit}{4}-\p{XDigit}{12}$`, so
    /// a row's id is a hyphenated UUID whose last field is what tells it apart.
    fn id(&self) -> String {
        format!("7f1c9b2a-1d4e-4c3b-8a5f-{}", self.id_suffix)
    }
}

/// The insights every case below is asserted over.
///
/// The rows differ along what the rule generalises over: what the insight reports, how
/// urgent it is, where it is in its own lifecycle, whether it is experimental, and
/// whether it was raised before, at or after the base instant. Between them they use
/// every value of `InsightType`, `InsightPriorityLevel` and `InsightStatus` — the
/// catalogs the model declares, as `ops::types`' `INSIGHT_TYPE_VALUES`,
/// `INSIGHT_PRIORITY_LEVEL_VALUES` and `INSIGHT_STATUS_VALUES` (asserted below) — and
/// `000000000004` was raised at the SAME instant as `000000000003`, so the order of
/// two insights that fall together is exercised too.
fn rows() -> Vec<Row> {
    vec![
        Row {
            id_suffix: "000000000001",
            display_name: "Cluster entered red status",
            insight_type: "EVENT",
            priority: "CRITICAL",
            status: "ACTIVE",
            offset: -3_600,
            experimental: false,
        },
        Row {
            id_suffix: "000000000002",
            display_name: "Enable dedicated master nodes",
            insight_type: "RECOMMENDATION",
            priority: "HIGH",
            status: "ACTIVE",
            offset: -1_800,
            experimental: false,
        },
        Row {
            id_suffix: "000000000003",
            display_name: "Shards were rebalanced",
            insight_type: "EVENT",
            priority: "MEDIUM",
            status: "RESOLVED",
            offset: -900,
            experimental: true,
        },
        Row {
            id_suffix: "000000000004",
            display_name: "Move the data volumes to gp3",
            insight_type: "RECOMMENDATION",
            priority: "LOW",
            status: "DISMISSED",
            offset: -900,
            experimental: false,
        },
        Row {
            id_suffix: "000000000005",
            display_name: "JVM memory pressure crossed its threshold",
            insight_type: "EVENT",
            priority: "HIGH",
            status: "DISMISSED",
            offset: 0,
            experimental: true,
        },
        Row {
            id_suffix: "000000000006",
            display_name: "Upgrade to a supported engine version",
            insight_type: "RECOMMENDATION",
            priority: "CRITICAL",
            status: "RESOLVED",
            offset: 600,
            experimental: false,
        },
        Row {
            id_suffix: "000000000007",
            display_name: "Automated snapshot completed",
            insight_type: "EVENT",
            priority: "LOW",
            status: "ACTIVE",
            offset: 1_800,
            experimental: false,
        },
        Row {
            id_suffix: "000000000008",
            display_name: "Right-size the data nodes",
            insight_type: "RECOMMENDATION",
            priority: "MEDIUM",
            status: "ACTIVE",
            offset: 3_600,
            experimental: true,
        },
    ]
}

/// The ids of [`rows`] in the order an ascending listing must report them: by the
/// instant each insight was raised, oldest first, the insight id breaking a tie.
fn ascending_order() -> Vec<String> {
    rows().iter().map(Row::id).collect()
}

/// [`ascending_order`] reversed — the order a descending listing reports.
fn descending_order() -> Vec<String> {
    let mut order = ascending_order();
    order.reverse();
    order
}

/// The instant the offsets of [`rows`] are measured from, rounded down to the second
/// so the millisecond count a case computes for itself is exact.
fn base_instant() -> SystemTime {
    let since = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the test clock is after the epoch");
    UNIX_EPOCH + Duration::from_secs(since.as_secs())
}

/// `base` as an epoch-second count, computed here rather than taken from the emulator,
/// so a case asserts the UNIT the wire carries a timestamp in and not just its value.
fn base_seconds(base: SystemTime) -> f64 {
    #[allow(clippy::cast_precision_loss)]
    let seconds = base
        .duration_since(UNIX_EPOCH)
        .expect("the test clock is after the epoch")
        .as_secs() as f64;
    seconds
}

/// `base` as the epoch-millisecond count `InsightTimeRange`'s members are stated in.
fn base_millis(base: SystemTime) -> i64 {
    i64::try_from(
        base.duration_since(UNIX_EPOCH)
            .expect("the test clock is after the epoch")
            .as_millis(),
    )
    .expect("an epoch-millisecond count of this era fits a Long")
}

/// `at` shifted by `offset` seconds, in either direction.
fn shifted(at: SystemTime, offset: i64) -> SystemTime {
    if offset < 0 {
        at - Duration::from_secs(offset.unsigned_abs())
    } else {
        at + Duration::from_secs(offset.unsigned_abs())
    }
}

/// `row` as the record the store holds, raised `row.offset` seconds from `base`. The
/// insight was last changed when it was raised, so both instants are the same: nothing
/// has moved it since.
fn stored(row: &Row, base: SystemTime) -> InsightRecord {
    let raised = shifted(base, row.offset);
    InsightRecord {
        insight_id: row.id(),
        display_name: row.display_name.to_string(),
        insight_type: row.insight_type.to_string(),
        priority: row.priority.to_string(),
        status: row.status.to_string(),
        created_at: raised,
        updated_at: raised,
        is_experimental: row.experimental,
        // This suite asserts the LISTING, which reports an insight's own members and
        // none of its detail fields; the detailed view has its own suite
        // ([`super::insight_details`]).
        details: Vec::new(),
        feedback: None,
    }
}

/// Put the whole [`rows`] table into `name`'s store entry, measured from `base`.
fn seed(emulator: &Emulator, name: &str, base: SystemTime) {
    let store = emulator.store();
    let mut guard = store.lock();
    let domain = guard
        .domains
        .get_mut(name)
        .unwrap_or_else(|| panic!("case seed({name:?}): the domain must have been created"));
    for row in rows() {
        let record = stored(&row, base);
        domain.insights.insert(record.insight_id.clone(), record);
    }
}

/// A domain with the whole table raised on it, and the base instant the offsets were
/// measured from.
async fn seeded(emulator: &Emulator, name: &str) -> SystemTime {
    create_baseline(emulator, name).await;
    let base = base_instant();
    seed(emulator, name, base);
    base
}

/// An `InsightEntity` naming the domain `name`.
fn domain_entity(name: &str) -> Value {
    json!({"Type": "DomainName", "Value": name})
}

/// `POST /2021-01-01/opensearch/insights` with `body`.
async fn list(emulator: &Emulator, body: Value) -> Answer {
    emulator
        .call("POST", &format!("{PREFIX}/opensearch/insights"), Some(body))
        .await
}

/// The listing of `name`, with whatever extra members `extra` carries merged in.
///
/// Reached by a sibling suite as `super::insights::list_domain`, so the crate drives
/// this listing through ONE request helper.
pub(crate) async fn list_domain(emulator: &Emulator, name: &str, extra: Value) -> Answer {
    let mut body = json!({"Entity": domain_entity(name)});
    if let Some(members) = extra.as_object() {
        for (member, value) in members {
            body[member] = value.clone();
        }
    }
    list(emulator, body).await
}

/// The `Insights` of an accepted listing. Reached by a sibling suite on the same rule
/// as [`list_domain`].
pub(crate) fn insights(answer: &Answer, case: &str) -> Vec<Value> {
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["Insights"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected an Insights list, got {:?}", answer.body))
        .clone()
}

/// The `InsightId` of every insight of a page, in the order the page reports them.
fn ids(page: &[Value]) -> Vec<String> {
    page.iter()
        .map(|insight| insight["InsightId"].as_str().unwrap_or_default().to_owned())
        .collect()
}

/// A refused insight request's status and modelled error code, with the case named on
/// failure. Shared with [`super::insight_details`], which refuses the same entities.
pub(crate) fn refused(answer: &Answer, case: &str, status: u16, code: &str) {
    assert_eq!(
        answer.status, status,
        "case {case}: expected HTTP {status}, got {} {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        error_code(answer),
        code,
        "case {case}: expected {code}, got {:?}",
        answer.body
    );
}

#[tokio::test]
async fn a_listing_reports_every_insight_of_the_entity_as_its_stored_record() {
    let emulator = Emulator::new();
    let base = seeded(&emulator, "insight-report").await;

    let answer = list_domain(&emulator, "insight-report", json!({})).await;
    let page = insights(&answer, "whole listing");
    assert_eq!(
        ids(&page),
        descending_order(),
        "case whole listing: expected the insights newest first, got {:?}",
        ids(&page)
    );

    for row in rows() {
        let case = format!("Insight({:?})", row.id_suffix);
        // The catalogs these rows draw on are the model's own enums, as generated into
        // `ops::types`: a row naming a value outside one would assert nothing about
        // this service's wire surface.
        assert!(
            INSIGHT_TYPE_VALUES.contains(&row.insight_type)
                && INSIGHT_PRIORITY_LEVEL_VALUES.contains(&row.priority)
                && INSIGHT_STATUS_VALUES.contains(&row.status),
            "case {case}: the row must use modelled values, got {:?}/{:?}/{:?}",
            row.insight_type,
            row.priority,
            row.status
        );

        let reported = page
            .iter()
            .find(|insight| insight["InsightId"].as_str() == Some(row.id().as_str()))
            .unwrap_or_else(|| panic!("case {case}: expected the insight to be reported"));
        assert_eq!(
            reported["DisplayName"].as_str(),
            Some(row.display_name),
            "case {case}: expected the stored display name, got {:?}",
            reported["DisplayName"]
        );
        assert_eq!(
            reported["Type"].as_str(),
            Some(row.insight_type),
            "case {case}: expected the stored type, got {:?}",
            reported["Type"]
        );
        assert_eq!(
            reported["Priority"].as_str(),
            Some(row.priority),
            "case {case}: expected the stored priority, got {:?}",
            reported["Priority"]
        );
        assert_eq!(
            reported["Status"].as_str(),
            Some(row.status),
            "case {case}: expected the stored status, got {:?}",
            reported["Status"]
        );
        #[allow(clippy::cast_precision_loss)]
        let raised = base_seconds(base) + row.offset as f64;
        for member in ["CreationTime", "UpdateTime"] {
            let reported_at = reported[member].as_f64().unwrap_or_else(|| {
                panic!("case {case}: expected a numeric {member}, got {reported:?}")
            });
            assert!(
                (reported_at - raised).abs() < f64::EPSILON,
                "case {case}: expected {member} {raised}, got {reported_at}"
            );
        }
        assert_eq!(
            reported["IsExperimental"].as_bool(),
            Some(row.experimental),
            "case {case}: expected the stored IsExperimental, got {:?}",
            reported["IsExperimental"]
        );
    }

    // A listing is a report: a second one answers exactly the same, so reading the
    // insights moved none of them.
    let again = list_domain(&emulator, "insight-report", json!({})).await;
    assert_eq!(
        again.body, answer.body,
        "case whole listing(twice): a read must leave the insights as they were, got {:?}",
        again.body
    );
}

#[tokio::test]
async fn the_sort_order_decides_which_end_of_the_listing_comes_first() {
    let emulator = Emulator::new();
    seeded(&emulator, "insight-order").await;

    // Both values the model admits, and the request that names none.
    for (sort_order, expected) in [
        (Some("ASC"), ascending_order()),
        (Some("DESC"), descending_order()),
        (None, descending_order()),
    ] {
        let case = format!("SortOrder {sort_order:?}");
        let extra = match sort_order {
            None => json!({}),
            Some(value) => {
                assert!(
                    INSIGHT_SORT_ORDER_VALUES.contains(&value),
                    "case {case}: the row must use a modelled sort order"
                );
                json!({"SortOrder": value})
            }
        };
        let answer = list_domain(&emulator, "insight-order", extra).await;
        assert_eq!(
            ids(&insights(&answer, &case)),
            expected,
            "case {case}: expected the listing in that order, got {:?}",
            answer.body
        );
    }

    // A value outside `InsightSortOrder` is no sort order at all: it is refused rather
    // than reinterpreted as one of the two.
    for not_an_order in ["ascending", "asc", "", "DESCENDING"] {
        let answer =
            list_domain(&emulator, "insight-order", json!({"SortOrder": not_an_order})).await;
        refused(
            &answer,
            &format!("SortOrder {not_an_order:?}"),
            400,
            "ValidationException",
        );
    }
}

#[tokio::test]
async fn pages_partition_the_listing_of_an_entity() {
    let emulator = Emulator::new();
    seeded(&emulator, "insight-paging").await;
    let whole = descending_order();

    // One row per page size: the smallest, two that do not divide the listing, the
    // listing's own size and the largest the modelled range allows.
    for page_size in [1_i64, 2, 3, 8, 500] {
        let case = format!("page size {page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0_usize;
        loop {
            let extra = match &token {
                None => json!({"MaxResults": page_size}),
                Some(token) => json!({"MaxResults": page_size, "NextToken": token}),
            };
            let answer = list_domain(&emulator, "insight-paging", extra).await;
            let page = insights(&answer, &case);
            assert!(
                i64::try_from(page.len()).unwrap_or(i64::MAX) <= page_size,
                "case {case}: a page holds at most MaxResults insights, got {}",
                page.len()
            );
            assert!(
                !page.is_empty(),
                "case {case}: a page the service issued a token for reports insights, got none"
            );
            walked.extend(ids(&page));
            pages += 1;
            token = answer.body["NextToken"].as_str().map(str::to_owned);
            if token.is_none() {
                break;
            }
            assert!(
                walked.len() <= whole.len(),
                "case {case}: paging kept issuing tokens past the end of the listing"
            );
        }
        assert_eq!(
            walked, whole,
            "case {case}: expected the pages to concatenate back to the whole listing in order, \
             got {walked:?} over {pages} pages"
        );
        let expected_pages = whole
            .len()
            .div_ceil(usize::try_from(page_size).unwrap_or(1));
        assert_eq!(
            pages, expected_pages,
            "case {case}: expected {expected_pages} pages, got {pages}"
        );
    }

    // A request that names no page size asks for no bound and gets the whole listing
    // in one page, with no token to resume from.
    let unbounded = list_domain(&emulator, "insight-paging", json!({})).await;
    assert_eq!(
        ids(&insights(&unbounded, "unbounded")),
        whole,
        "case unbounded: expected the whole listing in one page"
    );
    assert!(
        unbounded.body["NextToken"].is_null(),
        "case unbounded: expected no NextToken, got {:?}",
        unbounded.body["NextToken"]
    );

    // Both published bounds are accepted; a page size past either end is refused.
    for at_bound in [1_i64, 500] {
        let answer =
            list_domain(&emulator, "insight-paging", json!({"MaxResults": at_bound})).await;
        assert_eq!(
            answer.status, 200,
            "case MaxResults={at_bound}: the published bound is accepted, got {} {:?}",
            answer.status, answer.body
        );
    }
    for out_of_bound in [0_i64, 501, 1_000, -1] {
        let answer =
            list_domain(&emulator, "insight-paging", json!({"MaxResults": out_of_bound})).await;
        refused(
            &answer,
            &format!("MaxResults={out_of_bound}"),
            400,
            "ValidationException",
        );
    }
}

#[tokio::test]
async fn the_time_range_narrows_the_listing_to_the_insights_raised_inside_it() {
    let emulator = Emulator::new();
    let base = seeded(&emulator, "insight-window").await;
    let millis = base_millis(base);
    let suffixes = |reported: &[Value]| -> Vec<String> {
        ids(reported)
            .iter()
            .map(|id| id.split('-').next_back().unwrap_or_default().to_owned())
            .collect()
    };

    // The rows differ along where the window falls against the table: the whole set,
    // a window that ends before anything was raised, one that begins after everything
    // was, one single instant two insights share, and two windows that cut the set in
    // the middle — each bound INCLUDED, so a window ending exactly at an insight's
    // instant still reports it.
    for (label, from, to, expected) in [
        (
            "the whole set",
            millis - 7_200_000,
            millis + 7_200_000,
            vec![
                "000000000008",
                "000000000007",
                "000000000006",
                "000000000005",
                "000000000004",
                "000000000003",
                "000000000002",
                "000000000001",
            ],
        ),
        ("before the oldest", millis - 86_400_000, millis - 7_200_000, vec![]),
        ("after the newest", millis + 7_200_000, millis + 86_400_000, vec![]),
        (
            "one shared instant",
            millis - 900_000,
            millis - 900_000,
            vec!["000000000004", "000000000003"],
        ),
        (
            "the older half",
            millis - 3_600_000,
            millis - 1_800_000,
            vec!["000000000002", "000000000001"],
        ),
        (
            "the newer half",
            millis,
            millis + 3_600_000,
            vec!["000000000008", "000000000007", "000000000006", "000000000005"],
        ),
    ] {
        let case = format!("TimeRange {label}");
        let answer = list_domain(
            &emulator,
            "insight-window",
            json!({"TimeRange": {"From": from, "To": to}}),
        )
        .await;
        let page = insights(&answer, &case);
        assert_eq!(
            suffixes(&page),
            expected
                .iter()
                .map(|suffix| (*suffix).to_string())
                .collect::<Vec<String>>(),
            "case {case}: expected the insights raised inside the window, got {:?}",
            ids(&page)
        );
    }

    // A window that ends before it starts holds no instant at all, so it is a
    // malformed request rather than an empty page.
    for (from, to) in [(millis, millis - 1), (millis + 7_200_000, millis - 7_200_000)] {
        let answer = list_domain(
            &emulator,
            "insight-window",
            json!({"TimeRange": {"From": from, "To": to}}),
        )
        .await;
        refused(
            &answer,
            &format!("TimeRange {from}..{to}"),
            400,
            "ValidationException",
        );
    }
}

#[tokio::test]
async fn a_token_that_names_no_insight_of_the_listing_is_refused() {
    let emulator = Emulator::new();
    let base = seeded(&emulator, "insight-token").await;
    create_baseline(&emulator, "insight-other").await;
    let released = rows()[2].id();
    // An insight of ANOTHER entity's listing, so a token carried over from it names
    // nothing in this one.
    let elsewhere = InsightRecord {
        insight_id: "6cc3df5e-4a71-4f8c-9d9e-300000000001".to_string(),
        display_name: "Another domain is under-provisioned".to_string(),
        insight_type: "RECOMMENDATION".to_string(),
        priority: "LOW".to_string(),
        status: "ACTIVE".to_string(),
        created_at: base,
        updated_at: base,
        is_experimental: false,
        details: Vec::new(),
        feedback: None,
    };
    {
        let store = emulator.store();
        let mut guard = store.lock();
        guard
            .domains
            .get_mut("insight-other")
            .expect("the sibling domain must have been created")
            .insights
            .insert(elsewhere.insight_id.clone(), elsewhere.clone());
        let domain = guard
            .domains
            .get_mut("insight-token")
            .expect("the domain must have been created");
        domain.insights.remove(&released);
    }

    // A token is only ever a value this operation handed out FOR THIS listing: one
    // invented, one belonging to an insight of another entity's listing, one whose
    // insight has since been released, and one naming an insight the request's own
    // time range filters out.
    for (token, extra, label) in [
        ("not-a-token".to_string(), json!({}), "an invented token"),
        (
            elsewhere.insight_id.clone(),
            json!({}),
            "a token from another entity's listing",
        ),
        (
            released.clone(),
            json!({}),
            "a token whose insight was released",
        ),
        (
            rows()[7].id(),
            json!({"TimeRange": {
                "From": base_millis(base) - 7_200_000,
                "To": base_millis(base) - 1_800_000,
            }}),
            "a token outside the requested time range",
        ),
    ] {
        let case = format!("{label} ({token:?})");
        let mut body = extra;
        body["NextToken"] = json!(token);
        let answer = list_domain(&emulator, "insight-token", body).await;
        refused(&answer, &case, 400, "ValidationException");
    }
}

#[tokio::test]
async fn an_entity_this_account_does_not_hold_has_an_empty_listing() {
    let emulator = Emulator::new();
    seeded(&emulator, "insight-present").await;
    create_baseline(&emulator, "insight-deleted").await;
    let deleted = emulator
        .call(
            "DELETE",
            &format!("{PREFIX}/opensearch/domain/insight-deleted"),
            None,
        )
        .await;
    assert_eq!(
        deleted.status, 200,
        "case DeleteDomain(insight-deleted): expected 200, got {} with {:?}",
        deleted.status, deleted.body
    );

    // Rows differ along the shape of the name: one never created, one at the shortest
    // the model admits, one at its longest, and one the account held until it was
    // deleted. A name no domain is held under holds no insight either, so the listing
    // reports that empty set rather than refusing the name.
    for name in [
        "never-created",
        "abc",
        "a-domain-name-at-its-upper-b",
        "insight-deleted",
    ] {
        let case = format!("ListInsights(DomainName {name:?})");
        let answer = list_domain(&emulator, name, json!({})).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected the listing to be served, got {} with {:?}",
            answer.status, answer.body
        );
        assert!(
            insights(&answer, &case).is_empty(),
            "case {case}: expected an empty Insights, got {:?}",
            answer.body
        );
        assert!(
            answer.body["NextToken"].is_null(),
            "case {case}: expected no token past the end of an empty set, got {:?}",
            answer.body["NextToken"]
        );
    }

    // A `DomainName` entity names its domain in `Value`, so one that names none
    // resolves to no entity rather than to every domain.
    for entity in [
        json!({"Type": "DomainName"}),
        json!({"Type": "DomainName", "Value": null}),
    ] {
        let answer = list(&emulator, json!({"Entity": entity})).await;
        refused(
            &answer,
            &format!("ListInsights({entity:?})"),
            400,
            "ValidationException",
        );
    }

    // An `Account` entity's `Value` is "the Amazon Web Services account ID", and the
    // only account this emulator serves is the one the request was made for.
    for foreign in ["123456789012", "210987654321"] {
        assert_ne!(
            foreign,
            engine::emulator_account_id(),
            "case ListInsights(Account {foreign:?}): the row must name another account"
        );
        let answer = list(
            &emulator,
            json!({"Entity": {"Type": "Account", "Value": foreign}}),
        )
        .await;
        refused(
            &answer,
            &format!("ListInsights(Account {foreign:?})"),
            400,
            "ValidationException",
        );
    }
}

#[tokio::test]
async fn an_account_listing_reports_the_insights_of_the_account_and_of_its_domains() {
    let emulator = Emulator::new();
    let base = seeded(&emulator, "insight-account-a").await;
    create_baseline(&emulator, "insight-account-b").await;

    // One insight on a sibling domain and one held for the account itself: an
    // `Account` entity reports the account's own insights and those of every domain it
    // holds, and a `DomainName` entity reports only that domain's.
    let sibling = InsightRecord {
        insight_id: "8aa1bd3c-2e5f-4d6a-9b7c-100000000001".to_string(),
        display_name: "Sibling domain is under-provisioned".to_string(),
        insight_type: "RECOMMENDATION".to_string(),
        priority: "HIGH".to_string(),
        status: "ACTIVE".to_string(),
        created_at: shifted(base, 7_200),
        updated_at: shifted(base, 7_200),
        is_experimental: false,
        details: Vec::new(),
        feedback: None,
    };
    let account_wide = InsightRecord {
        insight_id: "9bb2ce4d-3f60-4e7b-8c8d-200000000001".to_string(),
        display_name: "Reserved Instance coverage is low for this account".to_string(),
        insight_type: "RECOMMENDATION".to_string(),
        priority: "MEDIUM".to_string(),
        status: "ACTIVE".to_string(),
        created_at: shifted(base, 10_800),
        updated_at: shifted(base, 10_800),
        is_experimental: false,
        details: Vec::new(),
        feedback: None,
    };
    {
        let store = emulator.store();
        let mut guard = store.lock();
        guard
            .domains
            .get_mut("insight-account-b")
            .expect("the sibling domain must have been created")
            .insights
            .insert(sibling.insight_id.clone(), sibling.clone());
        guard
            .account_insights
            .insert(account_wide.insight_id.clone(), account_wide.clone());
    }

    // The entity types the model admits, with the account named both ways a caller can
    // name it.
    for (entity, expected) in [
        (
            json!({"Type": "Account"}),
            {
                let mut all = ascending_order();
                all.push(sibling.insight_id.clone());
                all.push(account_wide.insight_id.clone());
                all.reverse();
                all
            },
        ),
        (
            json!({"Type": "Account", "Value": engine::emulator_account_id()}),
            {
                let mut all = ascending_order();
                all.push(sibling.insight_id.clone());
                all.push(account_wide.insight_id.clone());
                all.reverse();
                all
            },
        ),
        (domain_entity("insight-account-a"), descending_order()),
        (
            domain_entity("insight-account-b"),
            vec![sibling.insight_id.clone()],
        ),
    ] {
        let case = format!("ListInsights({entity:?})");
        assert!(
            INSIGHT_ENTITY_TYPE_VALUES.contains(&entity["Type"].as_str().unwrap_or_default()),
            "case {case}: the row must name a modelled entity type"
        );
        let answer = list(&emulator, json!({"Entity": entity})).await;
        assert_eq!(
            ids(&insights(&answer, &case)),
            expected,
            "case {case}: expected the entity's own insights, got {:?}",
            answer.body
        );
    }

    // A domain of the account that holds no insight reports an empty listing rather
    // than the account's set.
    create_baseline(&emulator, "insight-account-c").await;
    let empty = list_domain(&emulator, "insight-account-c", json!({})).await;
    assert!(
        insights(&empty, "empty listing").is_empty(),
        "case empty listing: expected no insights, got {:?}",
        empty.body["Insights"]
    );
    assert!(
        empty.body["NextToken"].is_null(),
        "case empty listing: expected no NextToken, got {:?}",
        empty.body["NextToken"]
    );
}
