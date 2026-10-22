// SPDX-License-Identifier: CC0-1.0

//! Billing adjustment requests, end to end over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the
//! same path a client takes. The wire helpers come from the suite beside this
//! one (`agreement_lifecycle`), so there is one harness for the crate.
//!
//! The rule under test: a billing adjustment request is the record one accepted
//! entry of `BatchCreateBillingAdjustmentRequest` stored against an invoice of
//! an agreement, and `ListBillingAdjustmentRequests` reports every stored
//! request matching EVERY filter the call names, in creation order and one page
//! at a time — each summary computed from that one record, never echoed from
//! the listing call's own input.

use std::collections::BTreeSet;
use std::thread::sleep;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::agreement_lifecycle::{agreement_over, call, emulator, is_next_token, term, Answer};

/// The catalog every agreement of this service is created in, spelled out here
/// rather than imported so a handler reporting another one fails this test.
const CATALOG: &str = "AWSMarketplace";

/// The `agreementType` every agreement this crate mints carries.
const AGREEMENT_TYPE: &str = "PurchaseAgreement";

/// The largest batch `BatchCreateBillingAdjustmentRequest` accepts, which is the
/// emulator's own documented bound (`state::BILLING_ADJUSTMENT_BATCH_LIMIT`),
/// spelled out so a change to it has to be made deliberately.
const BATCH_LIMIT: usize = 50;

/// Longer than the span a submitted request stays `PENDING`
/// (`state::BILLING_ADJUSTMENT_SETTLEMENT`), so waiting this long makes the
/// status change the clock owes due.
const PAST_SETTLEMENT: Duration = Duration::from_millis(900);

/// `BillingAdjustmentRequestId`'s `^ba-[a-zA-Z0-9]+$`, checked here rather than
/// through the crate's own code so a wrong minter cannot make this test pass.
fn is_billing_adjustment_id(value: &str) -> bool {
    value
        .strip_prefix("ba-")
        .is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_alphanumeric()))
}

/// One entry of a batch, with the members
/// `BatchCreateBillingAdjustmentRequestEntry` requires.
///
/// `pub(crate)` because `error_bodies.rs` stores adjustment requests through
/// the same wire helpers rather than building a second harness.
pub(crate) fn entry(agreement: &str, invoice: &str, amount: &str, token: &str) -> Value {
    json!({
        "agreementId": agreement,
        "originalInvoiceId": invoice,
        "adjustmentAmount": amount,
        "currencyCode": "USD",
        // One of `BillingAdjustmentReasonCode`'s values, from
        // smithy_query(query="shape", shape="BillingAdjustmentReasonCode").
        "adjustmentReasonCode": "INCORRECT_METERING",
        "clientToken": token,
    })
}

pub(crate) async fn submit(router: &MultiServiceRouter, entries: Vec<Value>) -> Answer {
    call(
        router,
        "BatchCreateBillingAdjustmentRequest",
        json!({"billingAdjustmentRequestEntries": entries}),
    )
    .await
}

async fn list(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "ListBillingAdjustmentRequests", request).await
}

pub(crate) async fn get(router: &MultiServiceRouter, agreement: &str, id: &str) -> Answer {
    call(
        router,
        "GetBillingAdjustmentRequest",
        json!({"agreementId": agreement, "billingAdjustmentRequestId": id}),
    )
    .await
}

/// The `items` of a list or batch answer.
fn items(answer: &Answer) -> Vec<Value> {
    answer.body["items"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("items is a required member, got {:?}", answer.body))
}

/// The `billingAdjustmentRequestId` of every item, in the order answered.
fn item_ids(answer: &Answer) -> Vec<String> {
    items(answer)
        .iter()
        .map(|item| {
            item["billingAdjustmentRequestId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The `errors` of a batch answer.
fn batch_errors(answer: &Answer) -> Vec<Value> {
    answer.body["errors"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("errors is a required member, got {:?}", answer.body))
}

/// One created request's `createdAt`, as the stored record reports it.
async fn created_at(router: &MultiServiceRouter, agreement: &str, id: &str) -> f64 {
    let answer = get(router, agreement, id).await;
    answer.body["createdAt"]
        .as_f64()
        .unwrap_or_else(|| panic!("createdAt is a required timestamp, got {:?}", answer.body))
}

/// Submit `invoices` as one batch against `agreement`, answering the created
/// ids in entry order.
async fn submit_all(
    router: &MultiServiceRouter,
    agreement: &str,
    invoices: &[(&str, &str)],
    token_prefix: &str,
) -> Vec<String> {
    let entries: Vec<Value> = invoices
        .iter()
        .enumerate()
        .map(|(index, (invoice, amount))| {
            entry(
                agreement,
                invoice,
                amount,
                &format!("{token_prefix}-{index}"),
            )
        })
        .collect();
    let answer = submit(router, entries).await;
    assert_eq!(
        answer.status, 200,
        "submitting {invoices:?} must succeed, got {} with {:?}",
        answer.status, answer.body
    );
    assert!(
        batch_errors(&answer).is_empty(),
        "submitting {invoices:?} against an existing agreement must report no per-entry \
         error, got {:?}",
        answer.body
    );
    let ids = item_ids(&answer);
    assert_eq!(
        ids.len(),
        invoices.len(),
        "one item per accepted entry: expected {}, got {:?}",
        invoices.len(),
        answer.body
    );
    for id in &ids {
        assert!(
            is_billing_adjustment_id(id),
            "a created id must match ^ba-[a-zA-Z0-9]+$, got {id:?}"
        );
    }
    ids
}

// ---------------------------------------------------------------------------
// ListBillingAdjustmentRequests
// ---------------------------------------------------------------------------

/// One row of the listing table: the query, and the requests it selects.
struct Listing {
    case: String,
    request: Value,
    expected: Vec<String>,
}

/// Every member of `ListBillingAdjustmentRequestsInput` selects on the STORED
/// request, and several of them are an AND.
///
/// The rows differ along what the rule generalises over: which member filters,
/// whether its value matches a stored request or nothing, how many members are
/// combined, and — for the two instant bounds — a value either side of a
/// published `createdAt` and the published value itself.
#[tokio::test]
async fn every_filter_selects_on_the_stored_request() {
    let router = emulator();
    let first = agreement_over(&router, "ap-adjust1", vec![term("eula.v1")]).await;
    let second = agreement_over(&router, "ap-adjust2", vec![term("eula.v1")]).await;

    let early = submit_all(
        &router,
        &first,
        &[("inv-1", "10"), ("inv-2", "2.5")],
        "tok-early",
    )
    .await;
    // Two batches, far enough apart that their `createdAt` differ by more than
    // the resolution the wire carries a timestamp at.
    sleep(Duration::from_millis(5));
    let late = submit_all(&router, &second, &[("inv-3", "0.00000001")], "tok-late").await;

    let every: Vec<String> = early.iter().chain(late.iter()).cloned().collect();
    let first_created = created_at(&router, &first, &early[0]).await;
    let late_created = created_at(&router, &second, &late[0]).await;
    assert!(
        late_created > first_created,
        "the later batch must carry the later createdAt, got {late_created} after {first_created}"
    );
    // The bounds the rows use sit either side of the two batches rather than on
    // a reported `createdAt` itself: a timestamp crosses the wire at the
    // precision the protocol carries, so only an instant strictly between the
    // two batches tells the two sides of the bound apart.
    let between = (first_created + late_created) / 2.0;

    let cases = vec![
        Listing {
            case: "no filter lists every stored request in creation order".to_string(),
            request: json!({}),
            expected: every.clone(),
        },
        Listing {
            case: "agreementId selects one agreement's requests".to_string(),
            request: json!({"agreementId": first}),
            expected: early.clone(),
        },
        Listing {
            case: "agreementId selects the other agreement's requests".to_string(),
            request: json!({"agreementId": second}),
            expected: late.clone(),
        },
        Listing {
            case: "catalog selects the catalog the agreements were created in".to_string(),
            request: json!({"catalog": CATALOG}),
            expected: every.clone(),
        },
        Listing {
            case: "another catalog selects nothing".to_string(),
            request: json!({"catalog": "SomeOtherCatalog"}),
            expected: vec![],
        },
        Listing {
            case: "agreementType selects the type the agreements carry".to_string(),
            request: json!({"agreementType": AGREEMENT_TYPE}),
            expected: every.clone(),
        },
        Listing {
            case: "another agreementType selects nothing".to_string(),
            request: json!({"agreementType": "VendorInsightsAgreement"}),
            expected: vec![],
        },
        Listing {
            case: "createdAfter a second before the first request selects every one".to_string(),
            request: json!({"createdAfter": first_created - 1.0}),
            expected: every.clone(),
        },
        Listing {
            case: "createdAfter an instant between the two batches excludes the earlier"
                .to_string(),
            request: json!({"createdAfter": between}),
            expected: late.clone(),
        },
        Listing {
            case: "createdAfter a second past the last request selects nothing".to_string(),
            request: json!({"createdAfter": late_created + 1.0}),
            expected: vec![],
        },
        Listing {
            case: "createdBefore a second after the last request selects every one".to_string(),
            request: json!({"createdBefore": late_created + 1.0}),
            expected: every.clone(),
        },
        Listing {
            case: "createdBefore an instant between the two batches excludes the later"
                .to_string(),
            request: json!({"createdBefore": between}),
            expected: early.clone(),
        },
        Listing {
            case: "createdBefore a second before the first request selects nothing".to_string(),
            request: json!({"createdBefore": first_created - 1.0}),
            expected: vec![],
        },
        Listing {
            case: "the two bounds together select the batch between them".to_string(),
            request: json!({
                "createdAfter": first_created - 1.0,
                "createdBefore": between,
            }),
            expected: early.clone(),
        },
        Listing {
            case: "agreementId and agreementType are an AND that both match".to_string(),
            request: json!({"agreementId": first, "agreementType": AGREEMENT_TYPE}),
            expected: early.clone(),
        },
        Listing {
            case: "agreementId and a mismatching catalog select nothing".to_string(),
            request: json!({"agreementId": first, "catalog": "SomeOtherCatalog"}),
            expected: vec![],
        },
        Listing {
            case: "maxResults bounds the page to the first requests".to_string(),
            request: json!({"maxResults": 2}),
            expected: early.clone(),
        },
    ];

    for case in cases {
        let answer = list(&router, case.request.clone()).await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: listing must succeed, got {} with {:?}",
            case.case, answer.status, answer.body
        );
        assert_eq!(
            item_ids(&answer),
            case.expected,
            "case {:?}: expected {:?}, got {:?}",
            case.case,
            case.expected,
            answer.body
        );
    }

    // Every summary is computed from the stored record: the invoice, amount and
    // currency the entry carried, and the agreement's own type and catalog —
    // which the listing call never names.
    let answer = list(&router, json!({"agreementId": first})).await;
    let summaries = items(&answer);
    let expected = [("inv-1", "10"), ("inv-2", "2.5")];
    for (summary, (invoice, amount)) in summaries.iter().zip(expected) {
        assert_eq!(
            summary["originalInvoiceId"].as_str(),
            Some(invoice),
            "case {invoice:?}: the summary reports the invoice the entry named, got {summary:?}"
        );
        assert_eq!(
            summary["adjustmentAmount"].as_str(),
            Some(amount),
            "case {invoice:?}: the summary reports the amount the entry named, got {summary:?}"
        );
        assert_eq!(
            summary["currencyCode"].as_str(),
            Some("USD"),
            "case {invoice:?}: the summary reports the currency the entry named, got {summary:?}"
        );
        assert_eq!(
            summary["agreementId"].as_str(),
            Some(first.as_str()),
            "case {invoice:?}: the summary reports the agreement adjusted, got {summary:?}"
        );
        assert_eq!(
            summary["agreementType"].as_str(),
            Some(AGREEMENT_TYPE),
            "case {invoice:?}: the summary reports the agreement's type, got {summary:?}"
        );
        assert_eq!(
            summary["catalog"].as_str(),
            Some(CATALOG),
            "case {invoice:?}: the summary reports the agreement's catalog, got {summary:?}"
        );
        assert!(
            summary["createdAt"].as_f64().is_some() && summary["updatedAt"].as_f64().is_some(),
            "case {invoice:?}: createdAt and updatedAt are required timestamps, got {summary:?}"
        );
    }
}

/// A page token resumes the listing it was minted for, and nothing else.
#[tokio::test]
async fn a_page_token_resumes_the_same_listing() {
    let router = emulator();
    let agreement = agreement_over(&router, "ap-pages", vec![term("eula.v1")]).await;
    let invoices: Vec<(String, String)> = (0..7)
        .map(|index| (format!("inv-{index}"), format!("{}", index + 1)))
        .collect();
    let borrowed: Vec<(&str, &str)> = invoices
        .iter()
        .map(|(invoice, amount)| (invoice.as_str(), amount.as_str()))
        .collect();
    let created = submit_all(&router, &agreement, &borrowed, "tok-page").await;

    // Three pages of at most three, in the same total order the unpaged listing
    // reports.
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    for page in 1..=3 {
        let mut request = json!({"maxResults": 3});
        if let Some(token) = token.as_deref() {
            request["nextToken"] = json!(token);
        }
        let answer = list(&router, request.clone()).await;
        assert_eq!(
            answer.status, 200,
            "case page {page}: listing must succeed, got {} with {:?}",
            answer.status, answer.body
        );
        let expected_len = if page == 3 { 1 } else { 3 };
        let ids = item_ids(&answer);
        assert_eq!(
            ids.len(),
            expected_len,
            "case page {page}: expected {expected_len} items, got {:?}",
            answer.body
        );
        seen.extend(ids);
        token = answer.body["nextToken"].as_str().map(str::to_string);
        if page == 3 {
            assert!(
                token.is_none(),
                "case page {page}: the last page carries no nextToken, got {token:?}"
            );
        } else {
            let minted = token
                .as_deref()
                .unwrap_or_else(|| panic!("case page {page}: a further page needs a nextToken"));
            assert!(
                is_next_token(minted),
                "case page {page}: a token must match NextToken's pattern, got {minted:?}"
            );
        }
    }
    assert_eq!(
        seen, created,
        "the pages together must report every request once, in creation order: expected {created:?}, got {seen:?}"
    );
    assert_eq!(
        seen.iter().collect::<BTreeSet<_>>().len(),
        created.len(),
        "no request may be reported on two pages: {seen:?}"
    );

    // A token belongs to the query it was minted for.
    let first_page = list(&router, json!({"maxResults": 3})).await;
    let minted = first_page.str_at("/nextToken");
    let refusals = vec![
        (
            "a token minted for the unfiltered listing cannot resume a filtered one",
            json!({"maxResults": 3, "agreementId": agreement, "nextToken": minted}),
        ),
        (
            "a token this service never minted is refused",
            json!({"nextToken": "not-a-token"}),
        ),
    ];
    for (case, request) in refusals {
        let answer = list(&router, request).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.reason(),
            "INVALID_NEXT_TOKEN",
            "case {case:?}: expected reason INVALID_NEXT_TOKEN, got {:?}",
            answer.body
        );
    }
}

// ---------------------------------------------------------------------------
// The clock
// ---------------------------------------------------------------------------

/// A submitted request is `PENDING` until the clock owes it its terminal
/// `COMPLETED`, and the status a listing filters on is the stored one.
#[tokio::test]
async fn a_submitted_request_is_pending_until_the_clock_completes_it() {
    let router = emulator();
    let agreement = agreement_over(&router, "ap-clock", vec![term("eula.v1")]).await;
    let created = submit_all(&router, &agreement, &[("inv-clock", "42")], "tok-clock").await;
    let id = created[0].clone();

    // A caller acting straight after the submit still sees the transient state.
    let pending = get(&router, &agreement, &id).await;
    assert_eq!(
        pending.str_at("/status"),
        "PENDING",
        "case right after the submit: expected PENDING, got {:?}",
        pending.body
    );
    assert!(
        pending.body["statusMessage"].is_null(),
        "case right after the submit: statusMessage is populated only when the status is \
         VALIDATION_FAILED, got {:?}",
        pending.body
    );
    assert_eq!(
        item_ids(&list(&router, json!({"status": "PENDING"})).await),
        vec![id.clone()],
        "case right after the submit: a PENDING filter selects it, got {:?}",
        pending.body
    );
    assert!(
        item_ids(&list(&router, json!({"status": "COMPLETED"})).await).is_empty(),
        "case right after the submit: a COMPLETED filter selects nothing"
    );

    sleep(PAST_SETTLEMENT);

    let completed = get(&router, &agreement, &id).await;
    assert_eq!(
        completed.str_at("/status"),
        "COMPLETED",
        "case past the settlement span: expected COMPLETED, got {:?}",
        completed.body
    );
    let created_instant = pending.body["createdAt"].as_f64().unwrap_or_default();
    let updated_instant = completed.body["updatedAt"].as_f64().unwrap_or_default();
    assert!(
        updated_instant > created_instant,
        "case past the settlement span: updatedAt {updated_instant} must be after createdAt \
         {created_instant}"
    );
    assert_eq!(
        completed.body["createdAt"].as_f64(),
        Some(created_instant),
        "case past the settlement span: createdAt never moves, got {:?}",
        completed.body
    );
    let again = get(&router, &agreement, &id).await;
    assert_eq!(
        again.body["updatedAt"].as_f64(),
        Some(updated_instant),
        "case read twice: updatedAt is the instant the change was due, so it is stable across \
         reads, got {:?}",
        again.body
    );

    for (case, status, expected) in [
        ("COMPLETED selects the settled request", "COMPLETED", vec![id.clone()]),
        ("PENDING selects nothing once it settled", "PENDING", vec![]),
        ("VALIDATION_FAILED selects nothing", "VALIDATION_FAILED", vec![]),
    ] {
        let answer = list(&router, json!({"status": status})).await;
        assert_eq!(
            item_ids(&answer),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            answer.body
        );
    }
}

// ---------------------------------------------------------------------------
// BatchCreateBillingAdjustmentRequest
// ---------------------------------------------------------------------------

/// An entry the service cannot accept is reported on the per-entry `errors`
/// channel — "a list of errors for entries that failed validation" — and stores
/// nothing.
///
/// The rows differ along the kind of entry: one naming a resource that does not
/// exist, and three whose amount satisfies `PositiveAmountUpto8Decimals`'s
/// pattern without being positive.
#[tokio::test]
async fn an_entry_that_cannot_be_accepted_is_reported_as_an_error() {
    let router = emulator();
    let agreement = agreement_over(&router, "ap-errors", vec![term("eula.v1")]).await;

    let cases = vec![
        (
            "an agreement that does not exist",
            entry("agmt-absent", "inv-x", "10", "tok-absent"),
            "RESOURCE_NOT_FOUND_EXCEPTION",
        ),
        (
            "a zero amount",
            entry(&agreement, "inv-x", "0", "tok-zero"),
            "VALIDATION_EXCEPTION",
        ),
        (
            "a zero amount with decimals",
            entry(&agreement, "inv-x", "0.00", "tok-zero-decimals"),
            "VALIDATION_EXCEPTION",
        ),
        (
            "an empty amount",
            entry(&agreement, "inv-x", "", "tok-empty"),
            "VALIDATION_EXCEPTION",
        ),
    ];

    for (case, bad, expected_code) in cases {
        let token = bad["clientToken"].as_str().unwrap_or_default().to_string();
        let answer = submit(&router, vec![bad.clone()]).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: a per-entry failure is reported in the body, not as an HTTP error, \
             got {} with {:?}",
            answer.status, answer.body
        );
        assert!(
            items(&answer).is_empty(),
            "case {case:?}: a refused entry creates nothing, got {:?}",
            answer.body
        );
        let errors = batch_errors(&answer);
        assert_eq!(
            errors.len(),
            1,
            "case {case:?}: expected one error, got {:?}",
            answer.body
        );
        assert_eq!(
            errors[0]["code"].as_str(),
            Some(expected_code),
            "case {case:?}: expected code {expected_code}, got {:?}",
            answer.body
        );
        assert_eq!(
            errors[0]["clientToken"].as_str(),
            Some(token.as_str()),
            "case {case:?}: the error names the entry's clientToken, got {:?}",
            answer.body
        );
        assert!(
            !errors[0]["message"].as_str().unwrap_or_default().is_empty(),
            "case {case:?}: message is a required member, got {:?}",
            answer.body
        );
    }

    let answer = list(&router, json!({})).await;
    assert!(
        items(&answer).is_empty(),
        "no refused entry may be stored, got {:?}",
        answer.body
    );
}

/// A malformed batch is refused as a whole, before anything is stored.
///
/// The rows differ along which rule about the batch it breaks: no entry at all,
/// more entries than one batch accepts, entries naming more than one agreement,
/// and two entries sharing the `clientToken` that is meant to be unique.
#[tokio::test]
async fn a_malformed_batch_is_refused_as_a_whole() {
    let agreement_placeholder = "agmt-whatever";
    let over_the_limit: Vec<Value> = (0..=BATCH_LIMIT)
        .map(|index| {
            entry(
                agreement_placeholder,
                &format!("inv-{index}"),
                "1",
                &format!("tok-{index}"),
            )
        })
        .collect();

    let cases = vec![
        (
            "a batch with no entry",
            Vec::new(),
            "MISSING_BILLING_ADJUSTMENTS",
        ),
        (
            "a batch one entry past the limit",
            over_the_limit,
            "BILLING_ADJUSTMENTS_LIMIT_EXCEEDED",
        ),
        (
            "a batch naming two agreements",
            vec![
                entry("agmt-one", "inv-1", "1", "tok-1"),
                entry("agmt-two", "inv-2", "1", "tok-2"),
            ],
            "MULTIPLE_AGREEMENT_IDS",
        ),
        (
            "two entries sharing one clientToken",
            vec![
                entry(agreement_placeholder, "inv-1", "1", "tok-same"),
                entry(agreement_placeholder, "inv-2", "1", "tok-same"),
            ],
            "INVALID_CLIENT_TOKEN",
        ),
    ];

    for (case, entries, expected_reason) in cases {
        let router = emulator();
        let answer = submit(&router, entries).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.reason(),
            expected_reason,
            "case {case:?}: expected reason {expected_reason}, got {:?}",
            answer.body
        );
        let listed = list(&router, json!({})).await;
        assert!(
            items(&listed).is_empty(),
            "case {case:?}: a refused batch stores nothing, got {:?}",
            listed.body
        );
    }

    // The bound itself is accepted: a batch of exactly the limit.
    let router = emulator();
    let agreement = agreement_over(&router, "ap-limit", vec![term("eula.v1")]).await;
    let invoices: Vec<(String, String)> = (0..BATCH_LIMIT)
        .map(|index| (format!("inv-{index}"), format!("{}", index + 1)))
        .collect();
    let borrowed: Vec<(&str, &str)> = invoices
        .iter()
        .map(|(invoice, amount)| (invoice.as_str(), amount.as_str()))
        .collect();
    let created = submit_all(&router, &agreement, &borrowed, "tok-limit").await;
    assert_eq!(
        created.len(),
        BATCH_LIMIT,
        "case a batch of exactly the limit: expected {BATCH_LIMIT} items, got {}",
        created.len()
    );
    let listed = list(&router, json!({"maxResults": BATCH_LIMIT})).await;
    assert_eq!(
        item_ids(&listed),
        created,
        "case a batch of exactly the limit: every entry must be stored, got {:?}",
        listed.body
    );
}

/// A created request reads back through `GetBillingAdjustmentRequest` as it was
/// submitted, and the `clientToken` is what makes a replayed submit report it
/// rather than create a second one.
///
/// The rows differ along the reason code and the description the entry carried;
/// the codes are `BillingAdjustmentReasonCode`'s values, from
/// smithy_query(query="shape", shape="BillingAdjustmentReasonCode").
#[tokio::test]
async fn a_created_request_reads_back_through_get() {
    let router = emulator();
    let agreement = agreement_over(&router, "ap-roundtrip", vec![term("eula.v1")]).await;

    let cases = vec![
        (
            "a described test-environment adjustment",
            "TEST_ENVIRONMENT_CHARGES",
            Some("charges incurred while testing the integration"),
            "12345.67890123",
        ),
        (
            "an undescribed renewal adjustment",
            "UNINTENDED_RENEWAL",
            None,
            "1",
        ),
        (
            "an adjustment of the smallest amount the model admits",
            "BUYER_DISSATISFACTION",
            Some("one hundred millionth"),
            "0.00000001",
        ),
    ];

    for (index, (case, reason, description, amount)) in cases.into_iter().enumerate() {
        let invoice = format!("inv-rt-{index}");
        let token = format!("tok-rt-{index}");
        let mut submitted = entry(&agreement, &invoice, amount, &token);
        submitted["adjustmentReasonCode"] = json!(reason);
        if let Some(description) = description {
            submitted["description"] = json!(description);
        }

        let answer = submit(&router, vec![submitted.clone()]).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: submitting must succeed, got {} with {:?}",
            answer.status, answer.body
        );
        let created = item_ids(&answer);
        assert_eq!(
            created.len(),
            1,
            "case {case:?}: expected one created item, got {:?}",
            answer.body
        );
        let id = created[0].clone();

        let read = get(&router, &agreement, &id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: reading {id} back must succeed, got {} with {:?}",
            read.status, read.body
        );
        for member in [
            "originalInvoiceId",
            "adjustmentAmount",
            "currencyCode",
            "adjustmentReasonCode",
            "agreementId",
        ] {
            let expected = if member == "agreementId" {
                Some(agreement.as_str())
            } else {
                submitted[member].as_str()
            };
            assert_eq!(
                read.body[member].as_str(),
                expected,
                "case {case:?}: {member} must read back as submitted, got {:?}",
                read.body
            );
        }
        assert_eq!(
            read.body["description"].as_str(),
            description,
            "case {case:?}: description must read back as submitted, got {:?}",
            read.body
        );
        assert_eq!(
            read.str_at("/billingAdjustmentRequestId"),
            id,
            "case {case:?}: the report names the request read, got {:?}",
            read.body
        );

        // The pair of ids selects: the request belongs to its own agreement.
        let other = agreement_over(
            &router,
            &format!("ap-other{index}"),
            vec![term("eula.v1")],
        )
        .await;
        for (not_found_case, agreement_id, request_id) in [
            ("another agreement's id", other.as_str(), id.as_str()),
            ("a request id that names none", agreement.as_str(), "ba-absent"),
        ] {
            let missing = get(&router, agreement_id, request_id).await;
            assert_eq!(
                // This service answers a lookup that found nothing with the
                // modelled `ResourceNotFoundException` body at HTTP 400, for
                // every resource it holds — see `crate::state::NOT_FOUND_STATUS`
                // for the recorded evidence that its model's `@httpError(404)`
                // understates the wire.
                missing.status,
                400,
                "case {case:?} / {not_found_case:?}: expected 400, got {} with {:?}",
                missing.status,
                missing.body
            );
            assert_eq!(
                missing.error_code(),
                "ResourceNotFoundException",
                "case {case:?} / {not_found_case:?}: expected a ResourceNotFoundException, got {:?}",
                missing.body
            );
        }

        // Replaying the token reports the request it created; reusing it for
        // different details is a conflict, and neither creates a second record.
        let replayed = submit(&router, vec![submitted.clone()]).await;
        assert_eq!(
            item_ids(&replayed),
            vec![id.clone()],
            "case {case:?}: a replayed submit reports the request the token created, got {:?}",
            replayed.body
        );
        let mut changed = submitted.clone();
        changed["adjustmentAmount"] = json!("999");
        let conflict = submit(&router, vec![changed]).await;
        assert!(
            items(&conflict).is_empty(),
            "case {case:?}: a reused token creates nothing, got {:?}",
            conflict.body
        );
        assert_eq!(
            batch_errors(&conflict)
                .first()
                .and_then(|error| error["code"].as_str()),
            Some("CONFLICT_EXCEPTION"),
            "case {case:?}: a token reused for different details is a conflict, got {:?}",
            conflict.body
        );

        let listed = list(&router, json!({"agreementId": agreement})).await;
        assert_eq!(
            item_ids(&listed).iter().filter(|listed| **listed == id).count(),
            1,
            "case {case:?}: {id} must be stored exactly once, got {:?}",
            listed.body
        );
    }
}

// ---------------------------------------------------------------------------
// A listing's agreementId names an agreement
// ---------------------------------------------------------------------------

/// `ListBillingAdjustmentRequestsInput$agreementId` must name an agreement this
/// service holds: an id it holds none for is refused as an invalid value for
/// the member, not accepted as a filter that selects nothing.
///
/// The rows differ along the form of the id that names no agreement, and along
/// which other members the call combines it with — the value is refused before
/// anything is filtered. The last case is the agreement that DOES exist, which
/// still lists.
#[tokio::test]
async fn a_listing_agreement_id_must_name_a_stored_agreement() {
    let router = emulator();
    let agreement = agreement_over(&router, "ap-listagreement", vec![term("eula.v1")]).await;
    let created = submit_all(&router, &agreement, &[("inv-1", "1")], "tok-listagreement").await;

    let absent: Vec<(&str, Value)> = vec![
        ("a short id", json!({"agreementId": "agmt-absent"})),
        (
            "an id shaped like one this service mints",
            json!({"agreementId": "agmt-0000000000000000"}),
        ),
        (
            "an id using every character AgreementId's pattern allows",
            json!({"agreementId": "A_z/0-9"}),
        ),
        (
            "an absent id beside a status filter",
            json!({"agreementId": "agmt-absent", "status": "PENDING"}),
        ),
        (
            "an absent id beside a page size",
            json!({"agreementId": "agmt-absent", "maxResults": 1}),
        ),
    ];

    for (case, request) in &absent {
        let answer = list(&router, request.clone()).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.reason(),
            "INVALID_AGREEMENT_ID",
            "case {case:?}: expected reason INVALID_AGREEMENT_ID, got {:?}",
            answer.body
        );
    }

    let answer = list(&router, json!({"agreementId": agreement})).await;
    assert_eq!(
        answer.status, 200,
        "case \"the agreement that exists\": expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        item_ids(&answer),
        created,
        "case \"the agreement that exists\": expected its own requests, got {:?}",
        answer.body
    );
}

// ---------------------------------------------------------------------------
// The bound on the store
// ---------------------------------------------------------------------------
/// The largest number of billing adjustment requests the store holds, which is
/// the emulator's own documented bound (`state::BILLING_ADJUSTMENT_REQUEST_QUOTA`),
/// spelled out so a change to it has to be made deliberately.
const STORE_QUOTA: usize = 1_000;

/// However many calls are made, the store holds at most [`STORE_QUOTA`]
/// requests: `BATCH_LIMIT` bounds ONE call, and this bounds the store across
/// all of them.
///
/// The rows differ along WHERE the bound falls relative to the call: a batch
/// that crosses it partway through (the entries that still fit are stored, the
/// rest are refused on the batch's own per-entry channel), and a call made when
/// the store is already full (every entry refused, nothing created).
#[tokio::test]
async fn the_store_holds_at_most_its_quota_however_many_calls_are_made() {
    let router = emulator();
    let agreement = agreement_over(&router, "ap-quota", vec![term("eula.v1")]).await;

    // Fill the store to one slot short of the bound, in batches of the largest
    // size one call accepts.
    let mut stored = 0usize;
    let mut first_id = String::new();
    while stored < STORE_QUOTA - 1 {
        let count = BATCH_LIMIT.min(STORE_QUOTA - 1 - stored);
        let entries: Vec<Value> = (0..count)
            .map(|index| {
                entry(
                    &agreement,
                    &format!("inv-{stored}-{index}"),
                    "1",
                    &format!("tok-fill-{stored}-{index}"),
                )
            })
            .collect();
        let answer = submit(&router, entries).await;
        assert_eq!(
            answer.status, 200,
            "case \"filling the store at {stored}\": expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        let ids = item_ids(&answer);
        assert_eq!(
            ids.len(),
            count,
            "case \"filling the store at {stored}\": expected {count} items, got {:?}",
            answer.body
        );
        if first_id.is_empty() {
            first_id = ids[0].clone();
        }
        stored += count;
    }

    // A batch that crosses the bound: one slot is free, so one entry is stored
    // and the other two are refused, each under its own clientToken.
    let crossing: Vec<Value> = (0..3)
        .map(|index| {
            entry(
                &agreement,
                &format!("inv-cross-{index}"),
                "1",
                &format!("tok-cross-{index}"),
            )
        })
        .collect();
    let crossed = submit(&router, crossing).await;
    assert_eq!(
        crossed.status, 200,
        "case \"a batch crossing the bound\": expected 200, got {} with {:?}",
        crossed.status, crossed.body
    );
    assert_eq!(
        item_ids(&crossed).len(),
        1,
        "case \"a batch crossing the bound\": only the entry that still fits is stored, got {:?}",
        crossed.body
    );
    let refusals = batch_errors(&crossed);
    assert_eq!(
        refusals.len(),
        2,
        "case \"a batch crossing the bound\": expected one error per entry that does not fit, \
         got {:?}",
        crossed.body
    );
    for refusal in &refusals {
        assert_eq!(
            refusal["code"].as_str(),
            Some("VALIDATION_EXCEPTION"),
            "case \"a batch crossing the bound\": expected VALIDATION_EXCEPTION, got {refusal:?}"
        );
    }

    // The store is now full, so no entry of a further call is accepted: each is
    // refused under its own clientToken, and the call creates nothing.
    let refused = submit(
        &router,
        vec![entry(&agreement, "inv-full", "1", "tok-full")],
    )
    .await;
    assert_eq!(
        refused.status, 200,
        "case \"a call against a full store\": the batch is answered per entry, got {} with {:?}",
        refused.status, refused.body
    );
    assert!(
        items(&refused).is_empty(),
        "case \"a call against a full store\": nothing may be created, got {:?}",
        refused.body
    );
    assert_eq!(
        batch_errors(&refused)
            .first()
            .and_then(|error| error["code"].as_str()),
        Some("VALIDATION_EXCEPTION"),
        "case \"a call against a full store\": expected VALIDATION_EXCEPTION, got {:?}",
        refused.body
    );

    // And nothing already stored was lost to either refusal.
    let still_there = get(&router, &agreement, &first_id).await;
    assert_eq!(
        still_there.status, 200,
        "case \"after the refusals\": {first_id} must still read back, got {} with {:?}",
        still_there.status, still_there.body
    );
}

