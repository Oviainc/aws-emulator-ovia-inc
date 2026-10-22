// SPDX-License-Identifier: CC0-1.0

//! `GetCreditAllocationHistory`, end to end over the awsJson1_0 wire.
//!
//! Every assertion here is about the RULE the operation follows: it REPORTS the
//! credit allocations persisted in
//! `ComAmazonawsBillingAwsbillingState::credit_allocations` — those of the
//! account asked about, whose `billingMonth` lies in the inclusive range of
//! billing months the two request timestamps fall in, and whose credit the
//! optional `creditId` names — newest billing month first, paged by
//! `maxResults`/`nextToken`, and it writes nothing.
//!
//! The emulator runs no billing pipeline, so nothing applies a credit to a bill
//! and the store is empty on a fresh emulator: like the view-store cap test in
//! `billing_views.rs`, the tables below put the records in state directly (the
//! same shared engine the router hands every handler) and then assert what the
//! API reports for them. The rows differ along everything the rule generalises
//! over: billing month, account, credit id (including one written with leading
//! zeros), service name, currency, whether a description is carried, whether the
//! bill is estimated, the position of the range bounds inside their months, the
//! width of the range, and the page size.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_billing_awsbilling::state::{CreditAllocation, CreditAmount};
use service_com_amazonaws_billing_awsbilling::{
    ComAmazonawsBillingAwsbillingState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;

/// The (`billingMonth`, `creditId`, `appliedServiceName`) triple that names one
/// allocation entry in an expectation: together they identify one application
/// of one credit for one account.
type EntryId = (String, String, String);

/// One row of the date-range table: the case, `startDate`, `endDate`, and the
/// entries that range must select.
type RangeCase = (&'static str, i64, i64, Vec<EntryId>);

/// One row of the `creditId` table: the case, the filter, and the entries it
/// must select.
type CreditCase = (&'static str, i64, Vec<EntryId>);

/// One row of the refusal table: the case, the request, and the member the
/// refusal must name in its `fieldList` (`None` for a modeled constraint the
/// boundary validator refuses with its own wording).
type RefusalCase = (&'static str, Value, Option<&'static str>);

/// The account id every generated emulator presents (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT_ID: &str = "000000000000";

/// A second 12-digit account, used to pin that the history of one account never
/// leaks into another's.
const OTHER_ACCOUNT_ID: &str = "123456789012";

/// One router over ONE shared engine — what `core/router` builds in production,
/// so every call in a test observes the same state.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// Send one awsJson1_0 call and return its HTTP status and decoded body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("AWSBilling.{operation}"))
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/billing/aws4_request, \
             SignedHeaders=host, Signature=0",
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, value)
}

/// The `__type` an awsJson error body carries, without its namespace prefix.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit(['#', ':'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// A request `Timestamp`: the Unix epoch seconds of a UTC civil instant.
///
/// Days-from-civil, the standard proleptic-Gregorian conversion, so the test
/// needs no date dependency the crate under test does not itself need.
fn at(year: i64, month: i64, day: i64, hour: i64) -> i64 {
    let shifted_year = if month <= 2 { year - 1 } else { year };
    let era = if shifted_year >= 0 {
        shifted_year
    } else {
        shifted_year - 399
    } / 400;
    let year_of_era = shifted_year - era * 400;
    let shifted_month = (month + 9) % 12;
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days_since_epoch = era * 146_097 + day_of_era - 719_468;
    days_since_epoch * 86_400 + hour * 3_600
}

/// One persisted allocation the API must report back unchanged.
struct Seed {
    credit_id: &'static str,
    currency_code: &'static str,
    currency_amount: &'static str,
    description: Option<&'static str>,
    account_id: &'static str,
    applied_service_name: &'static str,
    billing_month: &'static str,
    is_estimated_bill: bool,
}

impl Seed {
    fn allocation(&self) -> CreditAllocation {
        CreditAllocation {
            credit_id: self.credit_id.to_string(),
            credit_amount: CreditAmount {
                currency_code: self.currency_code.to_string(),
                currency_amount: self.currency_amount.to_string(),
            },
            description: self.description.map(str::to_string),
            account_id: self.account_id.to_string(),
            applied_service_name: self.applied_service_name.to_string(),
            billing_month: self.billing_month.to_string(),
            is_estimated_bill: self.is_estimated_bill,
        }
    }

    /// This allocation's [`EntryId`].
    fn id(&self) -> EntryId {
        (
            self.billing_month.to_string(),
            self.credit_id.to_string(),
            self.applied_service_name.to_string(),
        )
    }
}

/// The seeded allocations. Every month lies inside one 24-billing-month window
/// so a single request can span them all.
///
/// Service names are real Amazon Web Services service names and the amounts are
/// modeled `CurrencyAmount` decimal strings (`^[0-9]+(.[0-9]+)$`).
fn seeds() -> Vec<Seed> {
    vec![
        Seed {
            credit_id: "7",
            currency_code: "USD",
            currency_amount: "743.21",
            description: Some("January EC2 allocation"),
            account_id: ACCOUNT_ID,
            applied_service_name: "Amazon Elastic Compute Cloud",
            billing_month: "2025-01",
            is_estimated_bill: false,
        },
        Seed {
            credit_id: "7",
            currency_code: "USD",
            currency_amount: "0.01",
            description: None,
            account_id: ACCOUNT_ID,
            applied_service_name: "AWS Lambda",
            billing_month: "2025-02",
            is_estimated_bill: false,
        },
        // A credit id written with leading zeros: the `creditId` filter is a
        // Long, so it names this credit as the number 42.
        Seed {
            credit_id: "0042",
            currency_code: "USD",
            currency_amount: "12.00",
            description: Some("shared credit, storage"),
            account_id: ACCOUNT_ID,
            applied_service_name: "Amazon Simple Storage Service",
            billing_month: "2025-02",
            is_estimated_bill: false,
        },
        Seed {
            credit_id: "7",
            currency_code: "USD",
            currency_amount: "2.34",
            description: None,
            account_id: ACCOUNT_ID,
            applied_service_name: "AmazonCloudWatch",
            billing_month: "2025-12",
            is_estimated_bill: false,
        },
        Seed {
            credit_id: "900000000000000001",
            currency_code: "EUR",
            currency_amount: "5.50",
            description: Some("euro-denominated credit"),
            account_id: ACCOUNT_ID,
            applied_service_name: "Amazon Relational Database Service",
            billing_month: "2026-01",
            is_estimated_bill: false,
        },
        // An in-flight bill: the entry is estimated.
        Seed {
            credit_id: "7",
            currency_code: "USD",
            currency_amount: "100.00",
            description: Some("in-flight bill"),
            account_id: ACCOUNT_ID,
            applied_service_name: "Amazon Elastic Compute Cloud",
            billing_month: "2026-06",
            is_estimated_bill: true,
        },
        // Another account's allocation: never reported for ACCOUNT_ID.
        Seed {
            credit_id: "7",
            currency_code: "USD",
            currency_amount: "9.99",
            description: None,
            account_id: OTHER_ACCOUNT_ID,
            applied_service_name: "AWS Lambda",
            billing_month: "2025-06",
            is_estimated_bill: false,
        },
    ]
}

/// Persist every seeded allocation in the shared engine's store.
fn persist(engine: &Arc<engine::InMemoryEngine>, seeds: &[Seed]) {
    let state = engine.state::<ComAmazonawsBillingAwsbillingState>();
    let mut state = state.lock();
    for seed in seeds {
        assert!(
            state.record_credit_allocation(seed.allocation()),
            "case {:?}: the seeded allocation must fit in the store",
            seed.id()
        );
    }
}

/// How many allocations are resident, so a read can be shown not to write.
fn resident(engine: &Arc<engine::InMemoryEngine>) -> usize {
    let state = engine.state::<ComAmazonawsBillingAwsbillingState>();
    let state = state.lock();
    state.credit_allocations.len()
}

/// The (`billingMonth`, `creditId`, `appliedServiceName`) triples a response
/// reports, in the order it reports them.
fn reported(body: &Value) -> Vec<EntryId> {
    body["creditAllocationHistoryList"]
        .as_array()
        .expect("creditAllocationHistoryList is a list")
        .iter()
        .map(|entry| {
            (
                entry["billingMonth"].as_str().unwrap_or_default().to_string(),
                entry["creditId"].as_str().unwrap_or_default().to_string(),
                entry["appliedServiceName"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect()
}

/// A request covering every seeded month for `account`.
fn whole_range(account: &str) -> Value {
    json!({
        "accountId": account,
        "startDate": at(2025, 1, 1, 0),
        "endDate": at(2026, 6, 30, 23),
    })
}

/// The seeded entries of `account`, newest billing month first — and, inside one
/// month, highest credit id first, which is the store's own key order.
fn expected_for(account: &str) -> Vec<EntryId> {
    let seeds = seeds();
    let mut ids: Vec<EntryId> = seeds
        .iter()
        .filter(|seed| seed.account_id == account)
        .map(Seed::id)
        .collect();
    ids.sort_by(|left, right| {
        right.0.cmp(&left.0).then_with(|| {
            right
                .1
                .parse::<i128>()
                .unwrap_or_default()
                .cmp(&left.1.parse::<i128>().unwrap_or_default())
        })
    });
    ids
}

#[tokio::test]
async fn every_persisted_allocation_is_reported_newest_billing_month_first() {
    let (router, engine) = emulator();
    let seeds = seeds();
    persist(&engine, &seeds);

    let (status, body) = call(
        &router,
        "GetCreditAllocationHistory",
        &whole_range(ACCOUNT_ID),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"whole range\": expected 200, got {status} {body:?}"
    );
    assert_eq!(
        reported(&body),
        expected_for(ACCOUNT_ID),
        "case \"whole range\": expected the account's entries newest month first, got {body:?}"
    );
    assert_eq!(
        body["partialResults"],
        json!(false),
        "case \"whole range\": every month is answered from the store, got {:?}",
        body["partialResults"]
    );
    assert_eq!(
        body["failedMonths"],
        json!([]),
        "case \"whole range\": failedMonths is non-empty only when partialResults is true, got {:?}",
        body["failedMonths"]
    );
    assert!(
        body.get("nextToken").is_none_or(Value::is_null),
        "case \"whole range\": one page holds every entry, got {:?}",
        body.get("nextToken")
    );

    // billingMonth descending, as the output member documents.
    let months: Vec<String> = reported(&body).into_iter().map(|id| id.0).collect();
    let mut sorted = months.clone();
    sorted.sort_by(|left, right| right.cmp(left));
    assert_eq!(
        months, sorted,
        "case \"whole range\": entries must be sorted by billingMonth descending, got {months:?}"
    );

    // Every member of every entry is the persisted one.
    let entries = body["creditAllocationHistoryList"]
        .as_array()
        .expect("creditAllocationHistoryList is a list");
    for entry in entries {
        let id = (
            entry["billingMonth"].as_str().unwrap_or_default().to_string(),
            entry["creditId"].as_str().unwrap_or_default().to_string(),
            entry["appliedServiceName"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        );
        let seed = seeds
            .iter()
            .find(|seed| seed.account_id == ACCOUNT_ID && seed.id() == id)
            .unwrap_or_else(|| panic!("case {id:?}: reported an entry nothing persisted"));
        assert_eq!(
            entry["accountId"].as_str(),
            Some(seed.account_id),
            "case {id:?}: expected accountId {:?}, got {:?}",
            seed.account_id,
            entry["accountId"]
        );
        assert_eq!(
            entry["creditAmount"],
            json!({ "currencyCode": seed.currency_code, "currencyAmount": seed.currency_amount }),
            "case {id:?}: expected the persisted amount, got {:?}",
            entry["creditAmount"]
        );
        assert_eq!(
            entry.get("description").and_then(Value::as_str),
            seed.description,
            "case {id:?}: expected description {:?}, got {:?}",
            seed.description,
            entry.get("description")
        );
        assert_eq!(
            entry["isEstimatedBill"],
            json!(seed.is_estimated_bill),
            "case {id:?}: expected isEstimatedBill {}, got {:?}",
            seed.is_estimated_bill,
            entry["isEstimatedBill"]
        );
    }

    // The other account's own history is its own entry, and only that.
    let (status, other) = call(
        &router,
        "GetCreditAllocationHistory",
        &whole_range(OTHER_ACCOUNT_ID),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"other account\": expected 200, got {status} {other:?}"
    );
    assert_eq!(
        reported(&other),
        expected_for(OTHER_ACCOUNT_ID),
        "case \"other account\": an account sees only its own allocations, got {other:?}"
    );
}

#[tokio::test]
async fn a_date_range_selects_the_billing_months_it_covers() {
    let (router, engine) = emulator();
    persist(&engine, &seeds());

    // (case, startDate, endDate, the entries the range must select)
    let cases: Vec<RangeCase> = vec![
        (
            "one month, bounds on the month's edges",
            at(2025, 2, 1, 0),
            at(2025, 2, 28, 23),
            vec![
                ("2025-02".into(), "0042".into(), "Amazon Simple Storage Service".into()),
                ("2025-02".into(), "7".into(), "AWS Lambda".into()),
            ],
        ),
        (
            "one month, both bounds mid-month: a billing month is the unit",
            at(2025, 2, 14, 12),
            at(2025, 2, 14, 12),
            vec![
                ("2025-02".into(), "0042".into(), "Amazon Simple Storage Service".into()),
                ("2025-02".into(), "7".into(), "AWS Lambda".into()),
            ],
        ),
        (
            "two months, both bounds mid-month",
            at(2025, 1, 15, 6),
            at(2025, 2, 3, 1),
            vec![
                ("2025-02".into(), "0042".into(), "Amazon Simple Storage Service".into()),
                ("2025-02".into(), "7".into(), "AWS Lambda".into()),
                ("2025-01".into(), "7".into(), "Amazon Elastic Compute Cloud".into()),
            ],
        ),
        (
            "the latest month only",
            at(2026, 6, 1, 0),
            at(2026, 6, 30, 23),
            vec![(
                "2026-06".into(),
                "7".into(),
                "Amazon Elastic Compute Cloud".into(),
            )],
        ),
        (
            "entirely before every persisted month",
            at(2024, 1, 1, 0),
            at(2024, 12, 31, 23),
            vec![],
        ),
        (
            "entirely after every persisted month",
            at(2026, 7, 1, 0),
            at(2026, 8, 1, 0),
            vec![],
        ),
        (
            // The published bound: "the range from startDate to endDate cannot
            // exceed 24 billing months".
            "the widest accepted range, 24 billing months apart",
            at(2024, 6, 15, 0),
            at(2026, 6, 15, 0),
            expected_for(ACCOUNT_ID),
        ),
    ];

    for (case, start, end, expected) in cases {
        let request = json!({ "accountId": ACCOUNT_ID, "startDate": start, "endDate": end });
        let (status, body) = call(&router, "GetCreditAllocationHistory", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body:?}"
        );
        assert_eq!(
            reported(&body),
            expected,
            "case {case:?}: expected {expected:?}, got {body:?}"
        );
    }
}

#[tokio::test]
async fn the_credit_id_filter_selects_one_credit() {
    let (router, engine) = emulator();
    persist(&engine, &seeds());

    // (case, the creditId filter, the entries it must select)
    let cases: Vec<CreditCase> = vec![
        (
            "a credit applied in several months and services",
            7,
            expected_for(ACCOUNT_ID)
                .into_iter()
                .filter(|id| id.1 == "7")
                .collect(),
        ),
        (
            // The stored id is "0042": a Long filter names a credit by number.
            "a credit whose id is persisted with leading zeros",
            42,
            vec![(
                "2025-02".into(),
                "0042".into(),
                "Amazon Simple Storage Service".into(),
            )],
        ),
        (
            "a long credit id",
            900_000_000_000_000_001,
            vec![(
                "2026-01".into(),
                "900000000000000001".into(),
                "Amazon Relational Database Service".into(),
            )],
        ),
        ("a credit with no allocations", 999, vec![]),
    ];

    for (case, credit_id, expected) in cases {
        let mut request = whole_range(ACCOUNT_ID);
        request["creditId"] = json!(credit_id);
        let (status, body) = call(&router, "GetCreditAllocationHistory", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body:?}"
        );
        assert_eq!(
            reported(&body),
            expected,
            "case {case:?}: expected {expected:?}, got {body:?}"
        );
    }
}

#[tokio::test]
async fn pagination_returns_every_entry_exactly_once_for_every_page_size() {
    let (router, engine) = emulator();
    persist(&engine, &seeds());
    let expected = expected_for(ACCOUNT_ID);

    for page_size in [1_usize, 2, 3, 5, 6, 1000] {
        let case = format!("maxResults {page_size}");
        let mut walked: Vec<EntryId> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = whole_range(ACCOUNT_ID);
            request["maxResults"] = json!(page_size);
            if let Some(token) = &next_token {
                request["nextToken"] = json!(token);
            }
            let (status, body) = call(&router, "GetCreditAllocationHistory", &request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} {body:?}"
            );
            let page = reported(&body);
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page must hold at most maxResults entries, got {page:?}"
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "case {case:?}: the walk must terminate, {pages} pages so far holding {walked:?}"
            );
            next_token = body
                .get("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if next_token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, expected,
            "case {case:?}: the pages must concatenate to the whole history in order, got {walked:?}"
        );
    }

    // A nextToken is a position in the DESCENDING walk, so the same token used
    // with a narrower range reports the entries of that range that follow it:
    // a token from the newest month leaves every older month still to come...
    let mut first = whole_range(ACCOUNT_ID);
    first["maxResults"] = json!(1);
    let (status, page) = call(&router, "GetCreditAllocationHistory", &first).await;
    assert_eq!(
        status, 200,
        "case \"token from a later month\": expected 200, got {status} {page:?}"
    );
    let newest_token = page["nextToken"]
        .as_str()
        .expect("a first page of one entry has a nextToken")
        .to_string();
    let oldest_month = json!({
        "accountId": ACCOUNT_ID,
        "startDate": at(2025, 1, 1, 0),
        "endDate": at(2025, 1, 31, 23),
        "nextToken": newest_token,
    });
    let (status, body) = call(&router, "GetCreditAllocationHistory", &oldest_month).await;
    assert_eq!(
        status, 200,
        "case \"token from a later month\": expected 200, got {status} {body:?}"
    );
    assert_eq!(
        reported(&body),
        vec![(
            "2025-01".to_string(),
            "7".to_string(),
            "Amazon Elastic Compute Cloud".to_string(),
        )],
        "case \"token from a later month\": the older month still follows the token, got {body:?}"
    );

    // ...and a token from an older month leaves nothing before it in a later
    // month's range, so that request answers with an empty page.
    let mut fifth = whole_range(ACCOUNT_ID);
    fifth["maxResults"] = json!(5);
    let (_, fifth_page) = call(&router, "GetCreditAllocationHistory", &fifth).await;
    let older_token = fifth_page["nextToken"]
        .as_str()
        .expect("a five-entry page of a six-entry history has a nextToken")
        .to_string();
    let later_month = json!({
        "accountId": ACCOUNT_ID,
        "startDate": at(2026, 6, 1, 0),
        "endDate": at(2026, 6, 30, 23),
        "nextToken": older_token,
    });
    let (status, body) = call(&router, "GetCreditAllocationHistory", &later_month).await;
    assert_eq!(
        status, 200,
        "case \"token from an earlier month\": expected 200, got {status} {body:?}"
    );
    assert_eq!(
        reported(&body),
        Vec::new(),
        "case \"token from an earlier month\": nothing in that range follows the token, got {body:?}"
    );
}

#[tokio::test]
async fn an_account_with_no_persisted_allocations_gets_an_empty_list() {
    // Nothing is persisted: the emulator applies no credit to a bill, so a
    // fresh account has no allocation history — and that is a 200 with an empty
    // list, not a refusal.
    let (router, _engine) = emulator();

    for account in [ACCOUNT_ID, OTHER_ACCOUNT_ID, "999999999999"] {
        let (status, body) = call(
            &router,
            "GetCreditAllocationHistory",
            &whole_range(account),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {account:?}: expected 200, got {status} {body:?}"
        );
        assert_eq!(
            body["creditAllocationHistoryList"],
            json!([]),
            "case {account:?}: expected an empty list, got {body:?}"
        );
        assert_eq!(
            body["partialResults"],
            json!(false),
            "case {account:?}: nothing failed to be retrieved, got {body:?}"
        );
        assert!(
            body.get("nextToken").is_none_or(Value::is_null),
            "case {account:?}: there is no further page, got {:?}",
            body.get("nextToken")
        );
    }
}

#[tokio::test]
async fn a_request_outside_the_documented_rules_is_refused_and_persists_nothing() {
    let (router, engine) = emulator();
    persist(&engine, &seeds());
    let before = resident(&engine);

    // (case, request, the member the refusal must name)
    let cases: Vec<RefusalCase> = vec![
        (
            // "Must be on or before endDate."
            "startDate after endDate",
            json!({
                "accountId": ACCOUNT_ID,
                "startDate": at(2026, 2, 1, 0),
                "endDate": at(2025, 2, 1, 0),
            }),
            Some("startDate"),
        ),
        (
            // The same rule one second apart, inside one month: the order of
            // the two instants is what is refused, not their distance.
            "startDate one second after endDate",
            json!({
                "accountId": ACCOUNT_ID,
                "startDate": at(2025, 2, 10, 12) + 1,
                "endDate": at(2025, 2, 10, 12),
            }),
            Some("startDate"),
        ),
        (
            // "The range from startDate to endDate cannot exceed 24 billing
            // months": 25 months apart is one past the bound.
            "a range 25 billing months wide",
            json!({
                "accountId": ACCOUNT_ID,
                "startDate": at(2024, 5, 1, 0),
                "endDate": at(2026, 6, 1, 0),
            }),
            Some("endDate"),
        ),
        (
            "a range ten years wide",
            json!({
                "accountId": ACCOUNT_ID,
                "startDate": at(2016, 1, 1, 0),
                "endDate": at(2026, 1, 1, 0),
            }),
            Some("endDate"),
        ),
        (
            "a nextToken this service never issued",
            json!({
                "accountId": ACCOUNT_ID,
                "startDate": at(2025, 1, 1, 0),
                "endDate": at(2026, 1, 1, 0),
                "nextToken": "not-a-token-this-service-issued",
            }),
            Some("nextToken"),
        ),
        (
            // Epoch seconds so far from 1970 that they name no civil date, so
            // no billing month can be derived from them.
            "a startDate outside the representable calendar",
            json!({
                "accountId": ACCOUNT_ID,
                "startDate": i64::MIN,
                "endDate": at(2026, 1, 1, 0),
            }),
            Some("startDate"),
        ),
        (
            "an endDate outside the representable calendar",
            json!({
                "accountId": ACCOUNT_ID,
                "startDate": at(2025, 1, 1, 0),
                "endDate": i64::MAX,
            }),
            Some("endDate"),
        ),
        (
            // Modeled constraints, refused at the boundary: `accountId`
            // ^[0-9]{12}$ and `maxResults` 1..1000.
            "an accountId that is not 12 digits",
            json!({
                "accountId": "12345",
                "startDate": at(2025, 1, 1, 0),
                "endDate": at(2026, 1, 1, 0),
            }),
            None,
        ),
        (
            "maxResults below the modeled range",
            json!({
                "accountId": ACCOUNT_ID,
                "startDate": at(2025, 1, 1, 0),
                "endDate": at(2026, 1, 1, 0),
                "maxResults": 0,
            }),
            None,
        ),
        (
            "maxResults above the modeled range",
            json!({
                "accountId": ACCOUNT_ID,
                "startDate": at(2025, 1, 1, 0),
                "endDate": at(2026, 1, 1, 0),
                "maxResults": 1001,
            }),
            None,
        ),
    ];

    for (case, request, member) in cases {
        let (status, body) = call(&router, "GetCreditAllocationHistory", &request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected a 400 ValidationException, got {status} {body:?}"
        );
        // A refusal this handler words itself names the member it is about,
        // both in the message and in the modeled `fieldList`.
        if let Some(member) = member {
            let message = body["message"].as_str().unwrap_or_default();
            assert!(
                message.contains(member),
                "case {case:?}: the message must name {member:?}, got {message:?}"
            );
            assert_eq!(
                body["fieldList"][0]["name"].as_str(),
                Some(member),
                "case {case:?}: expected fieldList to name {member:?}, got {:?}",
                body["fieldList"]
            );
            assert!(
                !body["fieldList"][0]["message"]
                    .as_str()
                    .unwrap_or_default()
                    .is_empty(),
                "case {case:?}: the field entry must say why it failed, got {:?}",
                body["fieldList"]
            );
        }
    }

    assert_eq!(
        resident(&engine),
        before,
        "case \"after every refusal\": a refused read must not change the store"
    );
}

#[tokio::test]
async fn reading_the_history_does_not_change_what_a_later_caller_sees() {
    let (router, engine) = emulator();
    persist(&engine, &seeds());
    let before = resident(&engine);

    let (_, first) = call(
        &router,
        "GetCreditAllocationHistory",
        &whole_range(ACCOUNT_ID),
    )
    .await;
    let mut paged = whole_range(ACCOUNT_ID);
    paged["maxResults"] = json!(2);
    let (_, first_page) = call(&router, "GetCreditAllocationHistory", &paged).await;

    for attempt in 1..=3 {
        let case = format!("repeat {attempt}");
        let (_, again) = call(
            &router,
            "GetCreditAllocationHistory",
            &whole_range(ACCOUNT_ID),
        )
        .await;
        assert_eq!(
            again, first,
            "case {case:?}: the operation reports, so it must answer identically, got {again:?}"
        );
        let (_, page_again) = call(&router, "GetCreditAllocationHistory", &paged).await;
        assert_eq!(
            page_again, first_page,
            "case {case:?}: a first page must not consume anything, got {page_again:?}"
        );
    }

    assert_eq!(
        resident(&engine),
        before,
        "case \"after seven reads\": a read must not add or remove a record"
    );
}
