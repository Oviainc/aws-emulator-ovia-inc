// SPDX-License-Identifier: CC0-1.0

//! What a `NextToken` of this service NAMES: a position in the listing's own order.
//!
//! Every call here is a real `POST /` with `X-Amz-Target:
//! ServiceQuotasV20190624.<Op>` dispatched through the same `MultiServiceRouter` the
//! emulator serves with, over ONE shared engine, so what is exercised is the wire
//! behaviour. The harnesses are the ones `quota_workflow.rs` and
//! `quota_request_template.rs` own (the template plane's gate reads the account's
//! organization, so that listing is driven on the emulator mounting AWS Organizations
//! beside Service Quotas).
//!
//! The RULE, asserted over a TABLE of mutations rather than over one recorded request:
//! a page token names the position the page stopped at in the listing's ORDER — the
//! order key of the last record it reported — so a record stored or removed anywhere
//! around that position between two pages leaves the rest of the walk reporting exactly
//! the records that sit AFTER that position, each once and in order. Counting records
//! ahead of the page cannot do that: a record stored before the boundary reports one
//! record twice, and a record removed before it hides one from the walk altogether.
//!
//! The rows span the listings whose record set changes while a caller walks them — the
//! quota request template, both change histories, and the applied-quota listing's
//! resource plane — and the mutations the rule generalises over: a record stored before
//! the boundary and between the boundary and the next record, and a record removed
//! before the boundary, AT it, and after it.
//!
//! `pagination_token_scope.rs` pins which listing a token is good for; what this suite
//! adds is what the token means inside that listing while the listing moves.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Value};

use crate::quota_request_template::{Emulator, HOME_REGION};
use crate::quota_workflow::{client, emulator, Client, ACCOUNT};
use crate::resource_quota_context::quota_code_for;

/// Comfortably past BOTH steps the clock owes a quota increase request
/// (`crate::state::SUPPORT_CASE_OPEN_DELAY`, then
/// `crate::state::REQUEST_DECISION_DELAY` once its support case is open), so a request
/// made before this wait is decided afterwards and its value is in force.
const PAST_DECISION: Duration = Duration::from_millis(1_800);

/// The account-wide change history, whose name is too long to repeat in every row.
const HISTORY: &str = "ListRequestedServiceQuotaChangeHistory";

/// The per-quota change history.
const HISTORY_BY_QUOTA: &str = "ListRequestedServiceQuotaChangeHistoryByQuota";

/// The quota whose per-resource plane the applied-quota row walks: AWS publishes it as
/// adjustable at either level, so it HAS a resource plane (`src/state.rs::es_quotas`).
const RESOURCE_QUOTA_NAME: &str = "Instances per domain";

/// A caller of one listing.
///
/// The template plane is served only to an account in an organization with all
/// features, so its rows run on the two-service emulator; every other listing needs
/// Service Quotas alone.
enum Caller {
    Template(Emulator),
    Quotas(Client),
}

impl Caller {
    /// A caller with nothing stored yet, for a listing that does or does not stand
    /// behind the template plane's organization gate.
    async fn new(needs_organization: bool) -> Self {
        if needs_organization {
            return Self::Template(Emulator::in_organization("ALL").await);
        }
        let engine = emulator();
        Self::Quotas(client(&engine, HOME_REGION))
    }

    /// Dispatch one call that must answer 200, and return its body.
    async fn ok(&self, operation: &str, body: Value) -> Value {
        match self {
            Self::Template(template) => template.ok(operation, body).await,
            Self::Quotas(quotas) => quotas.ok(operation, body).await,
        }
    }
}

/// One step a row takes before a walk or in the middle of one: a call the caller must
/// answer 200, or the wait the clock needs for what the calls asked for to reach the
/// listing.
enum Step {
    Call(&'static str, Value),
    Settle(Duration),
}

/// One row of the table: a listing, the steps that seed the records it reports, and the
/// mutation made between its first page and the rest of the walk.
struct Row {
    /// Why this row is in the table.
    case: String,
    /// The listing's operation.
    operation: &'static str,
    /// The output member carrying its records.
    items: &'static str,
    /// The selection walked.
    body: Value,
    /// The members that identify one of its records, a nested one by its path.
    identity: &'static [&'static str],
    /// The page size the walk asks for.
    page_size: i64,
    /// What stores the records the walk starts out over.
    setup: Vec<Step>,
    /// What changes the record set once the first page has been served.
    mutation: Vec<Step>,
    /// Whether this listing stands behind the template plane's organization gate.
    needs_organization: bool,
}

/// A template entry of the one quota request template, as the members that identify it
/// plus the `DesiredValue` to store.
///
/// The quotas are seeded rows of this service's own catalog
/// (`src/state.rs::vpc_quotas`), each named by the `QuotaCode` AWS publishes for it,
/// with a value above its published default. Held in ONE Region and ONE service, so the
/// template's key order — `(ServiceCode, AwsRegion, QuotaCode)` — is the `QuotaCode`
/// order, which is what lets a row place its mutation before or after a page boundary.
struct Entry {
    quota_code: &'static str,
    desired: f64,
}

/// The template entries the rows draw on, in the template's own key order.
const ENTRIES: &[Entry] = &[
    // Network interfaces per Region, published default 5,000.
    Entry {
        quota_code: "L-045E6E9C",
        desired: 6_000.0,
    },
    // Subnets per VPC, published default 200.
    Entry {
        quota_code: "L-407747CB",
        desired: 400.0,
    },
    // Route tables per VPC, published default 200.
    Entry {
        quota_code: "L-589F43AA",
        desired: 400.0,
    },
    // Active VPC peering connections per VPC, published default 50.
    Entry {
        quota_code: "L-7E9ECCDB",
        desired: 100.0,
    },
    // VPCs per Region, published default 5.
    Entry {
        quota_code: "L-F678F1CE",
        desired: 10.0,
    },
];

/// The `(ServiceCode, QuotaCode, AwsRegion)` body a read or a delete of one entry takes.
fn triple(entry: &Entry) -> Value {
    json!({
        "ServiceCode": "vpc",
        "QuotaCode": entry.quota_code,
        "AwsRegion": HOME_REGION,
    })
}

/// A put of one entry.
fn put(entry: &Entry) -> Step {
    let mut body = triple(entry);
    body["DesiredValue"] = json!(entry.desired);
    Step::Call("PutServiceQuotaIncreaseRequestIntoTemplate", body)
}

/// A delete of one entry.
fn delete(entry: &Entry) -> Step {
    Step::Call(
        "DeleteServiceQuotaIncreaseRequestFromTemplate",
        triple(entry),
    )
}

/// An increase asked for on one quota, optionally on one resource's plane.
fn raise(service_code: &str, quota_code: &str, desired: f64, context: Option<&str>) -> Step {
    let mut body = json!({
        "ServiceCode": service_code,
        "QuotaCode": quota_code,
        "DesiredValue": desired,
    });
    if let Some(context) = context {
        body["ContextId"] = json!(context);
    }
    Step::Call("RequestServiceQuotaIncrease", body)
}

/// A domain ARN of this account, as a `ContextId` naming one resource.
fn domain(name: &str) -> String {
    format!("arn:aws:es:{HOME_REGION}:{ACCOUNT}:domain/{name}")
}

/// The template rows: the one listing the issue was filed against, with its mutation
/// placed at each position relative to the page boundary the token names.
fn template_rows() -> Vec<Row> {
    let body = json!({});
    let identity: &[&str] = &["ServiceCode", "AwsRegion", "QuotaCode"];
    vec![
        Row {
            case: "an entry stored BEFORE the page boundary".to_string(),
            operation: "ListServiceQuotaIncreaseRequestsInTemplate",
            items: "ServiceQuotaIncreaseRequestInTemplateList",
            body: body.clone(),
            identity,
            page_size: 1,
            setup: vec![put(&ENTRIES[1]), put(&ENTRIES[2]), put(&ENTRIES[4])],
            mutation: vec![put(&ENTRIES[0])],
            needs_organization: true,
        },
        Row {
            case: "an entry stored between the boundary and the next entry".to_string(),
            operation: "ListServiceQuotaIncreaseRequestsInTemplate",
            items: "ServiceQuotaIncreaseRequestInTemplateList",
            body: body.clone(),
            identity,
            page_size: 1,
            setup: vec![put(&ENTRIES[1]), put(&ENTRIES[4])],
            mutation: vec![put(&ENTRIES[2])],
            needs_organization: true,
        },
        Row {
            case: "an entry deleted BEFORE the page boundary".to_string(),
            operation: "ListServiceQuotaIncreaseRequestsInTemplate",
            items: "ServiceQuotaIncreaseRequestInTemplateList",
            body: body.clone(),
            identity,
            page_size: 2,
            setup: vec![
                put(&ENTRIES[0]),
                put(&ENTRIES[1]),
                put(&ENTRIES[2]),
                put(&ENTRIES[4]),
            ],
            mutation: vec![delete(&ENTRIES[0])],
            needs_organization: true,
        },
        Row {
            case: "the entry AT the page boundary deleted".to_string(),
            operation: "ListServiceQuotaIncreaseRequestsInTemplate",
            items: "ServiceQuotaIncreaseRequestInTemplateList",
            body: body.clone(),
            identity,
            page_size: 1,
            setup: vec![put(&ENTRIES[1]), put(&ENTRIES[2]), put(&ENTRIES[4])],
            mutation: vec![delete(&ENTRIES[1])],
            needs_organization: true,
        },
        Row {
            case: "an entry deleted AFTER the page boundary".to_string(),
            operation: "ListServiceQuotaIncreaseRequestsInTemplate",
            items: "ServiceQuotaIncreaseRequestInTemplateList",
            body,
            identity,
            page_size: 1,
            setup: vec![put(&ENTRIES[1]), put(&ENTRIES[2]), put(&ENTRIES[4])],
            mutation: vec![delete(&ENTRIES[4])],
            needs_organization: true,
        },
    ]
}

/// The change-history rows: the listings this crate keeps NEWEST FIRST, so a further
/// increase requested mid-walk is stored ahead of every record already reported.
fn history_rows() -> Vec<Row> {
    let identity: &[&str] = &["Id"];
    vec![
        Row {
            case: "an increase requested while the account's history is walked".to_string(),
            operation: HISTORY,
            items: "RequestedQuotas",
            body: json!({}),
            identity,
            page_size: 1,
            setup: vec![
                raise("vpc", "L-F678F1CE", 10.0, None),
                raise("vpc", "L-A4707A72", 10.0, None),
                raise("vpc", "L-045E6E9C", 6_000.0, None),
                raise("vpc", "L-407747CB", 400.0, None),
            ],
            mutation: vec![raise("vpc", "L-589F43AA", 400.0, None)],
            needs_organization: false,
        },
        Row {
            case: "an increase requested while ONE quota's history is walked".to_string(),
            operation: HISTORY_BY_QUOTA,
            items: "RequestedQuotas",
            body: json!({"ServiceCode": "vpc", "QuotaCode": "L-F678F1CE"}),
            identity,
            page_size: 1,
            // One quota holds at most one OPEN request, so each further increase waits
            // for the clock to decide the one before it.
            setup: vec![
                raise("vpc", "L-F678F1CE", 10.0, None),
                Step::Settle(PAST_DECISION),
                raise("vpc", "L-F678F1CE", 20.0, None),
                Step::Settle(PAST_DECISION),
            ],
            mutation: vec![raise("vpc", "L-F678F1CE", 30.0, None)],
            needs_organization: false,
        },
    ]
}

/// The applied-quota row: the RESOURCE plane's records appear as increases on a
/// resource's own plane are decided, and the every-resource context `*` sorts ahead of
/// a resource ARN, so this one lands ahead of the boundary too.
fn applied_quota_row(quota_code: &str) -> Row {
    Row {
        case: "a resource plane applied while the applied-quota listing is walked".to_string(),
        operation: "ListServiceQuotas",
        items: "Quotas",
        body: json!({
            "ServiceCode": "es",
            "QuotaCode": quota_code,
            "QuotaAppliedAtLevel": "RESOURCE",
        }),
        identity: &["QuotaCode", "QuotaContext.ContextId"],
        page_size: 1,
        setup: vec![
            raise("es", quota_code, 120.0, Some(&domain("beta"))),
            raise("es", quota_code, 140.0, Some(&domain("gamma"))),
            Step::Settle(PAST_DECISION),
        ],
        mutation: vec![
            raise("es", quota_code, 90.0, Some("*")),
            Step::Settle(PAST_DECISION),
        ],
        needs_organization: false,
    }
}

/// What identifies one record of `row`'s listing, as one comparable string.
fn identity(row: &Row, record: &Value) -> String {
    row.identity
        .iter()
        .map(|path| {
            let mut value = record;
            for member in path.split('.') {
                value = &value[member];
            }
            format!("{path}={value}")
        })
        .collect::<Vec<String>>()
        .join(" ")
}

/// The identities of the records `response` carries for `row`.
fn identities(row: &Row, response: &Value) -> Vec<String> {
    response[row.items]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|record| identity(row, record))
        .collect()
}

/// Run one row's steps.
async fn run(caller: &Caller, steps: &[Step]) {
    for step in steps {
        match step {
            Step::Call(operation, body) => {
                caller.ok(operation, body.clone()).await;
            }
            Step::Settle(wait) => tokio::time::sleep(*wait).await,
        }
    }
}

/// The whole selection in one body, which is what a call naming no `MaxResults` serves.
async fn whole_listing(caller: &Caller, row: &Row) -> Vec<String> {
    let served = caller.ok(row.operation, row.body.clone()).await;
    identities(row, &served)
}

/// Walk `row`'s listing, making its mutation between the first page and the rest, and
/// assert the rule on what the walk reported.
async fn assert_the_walk_resumes_at_the_position_it_stopped_at(row: &Row) {
    let case = &row.case;
    let caller = Caller::new(row.needs_organization).await;
    run(&caller, &row.setup).await;

    // What the listing holds when the walk starts, and its first page.
    let before = whole_listing(&caller, row).await;
    assert!(
        before.len() as i64 > row.page_size,
        "case {case}: the row must seed more records than one page holds, got {before:?}"
    );
    let mut asked = row.body.clone();
    asked["MaxResults"] = json!(row.page_size);
    let first = caller.ok(row.operation, asked).await;
    let page_one = identities(row, &first);
    assert_eq!(
        page_one,
        before[..row.page_size as usize].to_vec(),
        "case {case}: the first page must be the first {} records of the listing, got {page_one:?}",
        row.page_size
    );
    let mut token = first["NextToken"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| {
            panic!("case {case}: records remain after this page, so a NextToken must follow it, got {first}")
        });

    // The record set moves under the walk, and the listing is read whole again so the
    // expectation is the ORDER the rest of the walk has to report.
    run(&caller, &row.mutation).await;
    let after = whole_listing(&caller, row).await;

    let mut seen = page_one.clone();
    let mut ended = false;
    for _ in 0..256 {
        let mut resume = row.body.clone();
        resume["MaxResults"] = json!(row.page_size);
        resume["NextToken"] = json!(&token);
        let page = caller.ok(row.operation, resume).await;
        let served = identities(row, &page);
        assert!(
            served.len() as i64 <= row.page_size,
            "case {case}: a page must hold at most {} records, got {served:?}",
            row.page_size
        );
        seen.extend(served);
        match page["NextToken"].as_str() {
            Some(next) => token = next.to_string(),
            None => {
                ended = true;
                break;
            }
        }
    }
    assert!(
        ended,
        "case {case}: the walk must reach a page with no NextToken, got {seen:?}"
    );

    // The rule: the walk reports the first page, then exactly the records that sit
    // after the position its token named — in the listing's order, as the listing holds
    // it NOW.
    let boundary = page_one
        .last()
        .unwrap_or_else(|| panic!("case {case}: the first page must hold a record"));
    let tail: Vec<String> = match after.iter().position(|held| held == boundary) {
        Some(at) => after[at + 1..].to_vec(),
        // The mutation removed the very record the token names: what sits after that
        // position is then the records the listing still holds out of the tail it held
        // when the page was cut.
        None => {
            let unreported = &before[page_one.len()..];
            after
                .iter()
                .filter(|held| unreported.contains(held))
                .cloned()
                .collect()
        }
    };
    let expected: Vec<String> = page_one.iter().chain(tail.iter()).cloned().collect();
    assert_eq!(
        seen, expected,
        "case {case}: the walk must report the first page and then the records after the \
         position its token named, got {seen:?}"
    );

    // Named on its own, because it is the symptom an offset token has: the records
    // ahead of the boundary shift and one record is served on two pages.
    let distinct: BTreeSet<&String> = seen.iter().collect();
    assert_eq!(
        distinct.len(),
        seen.len(),
        "case {case}: no record may be reported twice in one walk, got {seen:?}"
    );
}

/// RULE: a token names a position in the template's key order, so an entry put into or
/// deleted from the template around the boundary leaves the walk reporting every entry
/// after that position exactly once.
#[tokio::test]
async fn the_template_listing_resumes_at_the_key_position_its_token_named() {
    for row in template_rows() {
        assert_the_walk_resumes_at_the_position_it_stopped_at(&row).await;
    }
}

/// RULE, the same one on the listings this crate keeps newest-first: an increase
/// requested mid-walk is stored ahead of every record already reported and repeats none
/// of them.
#[tokio::test]
async fn a_change_history_resumes_at_the_position_its_token_named() {
    for row in history_rows() {
        assert_the_walk_resumes_at_the_position_it_stopped_at(&row).await;
    }
}

/// RULE, the same one on the applied-quota listing: a resource plane that appears
/// mid-walk ahead of the boundary repeats no record and hides none.
#[tokio::test]
async fn the_applied_quota_listing_resumes_at_the_position_its_token_named() {
    let engine = emulator();
    let caller = client(&engine, HOME_REGION);
    // Looked up rather than written down, because the catalog derives this quota's code.
    let quota_code = quota_code_for(&caller, "es", RESOURCE_QUOTA_NAME).await;
    assert_the_walk_resumes_at_the_position_it_stopped_at(&applied_quota_row(&quota_code)).await;
}
