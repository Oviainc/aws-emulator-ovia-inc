// SPDX-License-Identifier: CC0-1.0

//! `ListPageReceipts` — the events recorded against one page — end to end over the wire.
//!
//! Every case drives the production `MultiServiceRouter` with a buffered awsJson1_1 request
//! (`POST /`, `X-Amz-Target: SSMContacts.<Operation>`) through the harness the contact suite
//! builds, so what is asserted is what a signing AWS client would see.
//!
//! The rules under test, each as a TABLE whose rows differ along what the rule generalises
//! over rather than a single call:
//!
//! 1. a page reports the acknowledgements recorded against it, oldest first, with the channel
//!    the page engaged, the `Note` each acknowledgement carried and the instant it arrived;
//!    a page nobody has acknowledged reports an EMPTY list rather than a refusal. The rows
//!    span the acknowledgement sequences a page may carry — neither `AcceptType`, each of
//!    them, both, repeated, with and without a `Note`, under both `AcceptCodeValidation`
//!    values;
//! 2. the listing is served in `MaxResults`-sized pages whose `NextToken` resumes after the
//!    last receipt emitted, so paging to exhaustion reports the whole log whatever the page
//!    size. The rows span the member absent, both published bounds, sizes smaller than the
//!    log, and one past the maximum, which the modelled constraint refuses;
//! 3. listing a page's receipts is a READ: repeated listings answer the same thing and the
//!    stored log, `DeliveryTime` and `ReadTime` are untouched;
//! 4. the listing is refused — before the page is looked up — while the account has no ACTIVE
//!    Incident Manager replication set, with `ResourceNotFoundException` for a page the store
//!    does not hold (never an empty list), and with `ValidationException` for a `NextToken`
//!    this list did not issue;
//! 5. every page an engagement sent is listable on its own: the pages come from the plan the
//!    engagement engaged, so the rows span the stage and target shapes a plan takes, and one
//!    page's log is never another's.
//!
//! The onboarded path is reached through the contact suite's own account fixture
//! (`contact_updates::set_replication_set`), which writes the replication set the handlers
//! read: Incident Manager's `CreateReplicationSet` is deprecated and refuses every call, so
//! the store is the only place an onboarded account can come from. A page's `PageArn` and the
//! `AcceptCode` sent with it are read from that same store — `ListPagesByEngagement` is still
//! a scaffold, and AWS never returns an accept code on the wire — which is the test standing
//! in for the contact reading the message their channel received.

use serde_json::{json, Value};
use service_com_amazonaws_ssmcontacts_ssmcontacts::state::{
    encode_next_token, service_state, PageRecord, TOKEN_SCOPE_CONTACTS, TOKEN_SCOPE_PAGE_RECEIPTS,
};

use crate::contact_lifecycle::{error_code, Emulator};
use crate::contact_updates::set_replication_set;

/// The `ReceiptType` an acknowledgement of each `AcceptType` is recorded under: the two
/// enums share these values (`ACCEPT_TYPE_VALUES` / `RECEIPT_TYPE_VALUES` in
/// `src/ops/types/shared_types.rs`).
const DELIVERED: &str = "DELIVERED";
const READ: &str = "READ";

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// Create a contact channel under `contact`, answering its `ContactChannelArn`.
async fn create_channel(emulator: &Emulator, contact: &str, index: usize) -> String {
    let case = format!("channel {index} of {contact}");
    let (status, body) = emulator
        .call(
            "CreateContactChannel",
            json!({
                "ContactId": contact,
                "Name": format!("channel-{index}"),
                "Type": "EMAIL",
                "DeliveryAddress": {"SimpleAddress": format!("contact-{index}@example.com")},
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateContactChannel to answer 200, got {status} {body:?}"
    );
    body["ContactChannelArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a ContactChannelArn, got {body:?}"))
        .to_string()
}

/// Create a contact, answering its `ContactArn`.
async fn create_contact(
    emulator: &Emulator,
    alias: &str,
    contact_type: &str,
    stages: Vec<Value>,
) -> String {
    emulator
        .create_contact(
            json!({"Alias": alias, "Type": contact_type, "Plan": {"Stages": stages}}),
            alias,
        )
        .await
}

/// Replace `contact`'s engagement plan with `stages`.
async fn set_plan(emulator: &Emulator, contact: &str, stages: Vec<Value>, case: &str) {
    let (status, body) = emulator
        .call(
            "UpdateContact",
            json!({"ContactId": contact, "Plan": {"Stages": stages}}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected UpdateContact to answer 200, got {status} {body:?}"
    );
}

/// One stage of a plan: the channels it targets, and how long the engagement waits on it.
fn channel_stage(channels: &[&String], duration_in_minutes: u32) -> Value {
    json!({
        "DurationInMinutes": duration_in_minutes,
        "Targets": channels
            .iter()
            .map(|channel| json!({"ChannelTargetInfo": {"ContactChannelId": channel}}))
            .collect::<Vec<Value>>(),
    })
}

/// `StartEngagement` against `contact`, answering its `EngagementArn`.
async fn start_engagement(emulator: &Emulator, contact: &str, case: &str) -> String {
    let (status, body) = emulator
        .call(
            "StartEngagement",
            json!({
                "ContactId": contact,
                "Sender": "incident-manager",
                "Subject": "disk pressure on prod-1",
                "Content": "a node is out of disk",
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected StartEngagement to answer 200, got {status} {body:?}"
    );
    body["EngagementArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected an EngagementArn, got {body:?}"))
        .to_string()
}

/// The pages the engagement `engagement_arn` sent, in the order it sent them.
///
/// Read from the store because `ListPagesByEngagement` is still a scaffold: a `PageArn` has
/// no other way onto the wire yet.
fn pages_of(emulator: &Emulator, engagement_arn: &str) -> Vec<PageRecord> {
    let store = service_state(&*emulator.engine).expect("the engine holds this service's state");
    let store = store.lock();
    let mut pages: Vec<PageRecord> = store
        .pages
        .values()
        .filter(|page| page.engagement_arn == engagement_arn)
        .cloned()
        .collect();
    pages.sort_by_key(|page| page.sequence);
    pages
}

/// The stored page `page_arn` names, read straight from the store.
fn stored_page(emulator: &Emulator, page_arn: &str) -> PageRecord {
    let store = service_state(&*emulator.engine).expect("the engine holds this service's state");
    let store = store.lock();
    store
        .pages
        .get(page_arn)
        .cloned()
        .unwrap_or_else(|| panic!("the store holds no page under {page_arn}"))
}

/// Every receipt `ListPageReceipts` reports for `request`, paging to exhaustion, asserting
/// no page is larger than the `MaxResults` the request named.
async fn all_receipts(emulator: &Emulator, request: Value, case: &str) -> Vec<Value> {
    let mut request = request;
    let limit = request["MaxResults"].as_i64();
    let mut receipts: Vec<Value> = Vec::new();
    loop {
        let (status, body) = emulator.call("ListPageReceipts", request.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListPageReceipts to answer 200, got {status} {body:?}"
        );
        let page = body["Receipts"].as_array().cloned().unwrap_or_default();
        if let Some(limit) = limit.filter(|limit| *limit > 0) {
            assert!(
                page.len() as i64 <= limit,
                "case {case:?}: a page held {} receipts for MaxResults={limit}",
                page.len()
            );
        }
        receipts.extend(page);
        match body["NextToken"].as_str() {
            Some(token) => {
                assert!(
                    receipts.len() <= 64,
                    "case {case:?}: ListPageReceipts paging did not terminate, collected {} \
                     receipts",
                    receipts.len()
                );
                request["NextToken"] = json!(token);
            }
            None => return receipts,
        }
    }
}

/// The `ReceiptType` of every receipt in `receipts`, in the order they were reported.
fn receipt_types(receipts: &[Value]) -> Vec<String> {
    receipts
        .iter()
        .map(|receipt| receipt["ReceiptType"].as_str().unwrap_or("").to_string())
        .collect()
}

/// A contact with one channel its plan targets, engaged once: answers the single page that
/// engagement sent.
async fn sent_page(emulator: &Emulator, case: &str) -> PageRecord {
    let contact = create_contact(emulator, "oncall-primary", "PERSONAL", Vec::new()).await;
    let channel = create_channel(emulator, &contact, 0).await;
    set_plan(emulator, &contact, vec![channel_stage(&[&channel], 5)], case).await;
    let engagement = start_engagement(emulator, &contact, case).await;
    let mut pages = pages_of(emulator, &engagement);
    assert_eq!(
        pages.len(),
        1,
        "case {case:?}: expected the engagement to send one page, got {pages:?}"
    );
    pages.remove(0)
}

// ---------------------------------------------------------------------------
// 1 + 2. What a page reports, and the pages it is reported in
// ---------------------------------------------------------------------------

/// One acknowledgement of a page: the `AcceptType` it carries, the `Note` it carries, and
/// whether it asks for `AcceptCode` validation to be ignored.
struct AckRow {
    accept_type: &'static str,
    note: Option<&'static str>,
    ignore_code: bool,
}

/// The acknowledgement sequences a page's receipt log is asserted over: no acknowledgement at
/// all, each `AcceptType` the model admits, both, the same one twice, with and without a
/// `Note`, and under both `AcceptCodeValidation` values.
const ACK_SEQUENCES: &[&[AckRow]] = &[
    &[],
    &[AckRow {
        accept_type: DELIVERED,
        note: None,
        ignore_code: false,
    }],
    &[AckRow {
        accept_type: READ,
        note: Some("on it"),
        ignore_code: true,
    }],
    &[
        AckRow {
            accept_type: DELIVERED,
            note: Some("phone buzzed"),
            ignore_code: false,
        },
        AckRow {
            accept_type: READ,
            note: Some("acknowledged from the console"),
            ignore_code: true,
        },
    ],
    &[
        AckRow {
            accept_type: READ,
            note: None,
            ignore_code: false,
        },
        AckRow {
            accept_type: READ,
            note: Some("re-read"),
            ignore_code: false,
        },
        AckRow {
            accept_type: DELIVERED,
            note: None,
            ignore_code: true,
        },
    ],
];

/// The `MaxResults` values every listing is driven with.
///
/// `MaxResults` is `range 0..1024` (model: `ListPageReceipts` input), so the rows span the
/// member absent, both published bounds, two sizes smaller than the log under test, and one
/// past the maximum, which the modelled constraint refuses before the handler runs.
const PAGE_SIZES: &[Option<i64>] = &[None, Some(0), Some(1), Some(2), Some(1024), Some(1025)];

#[tokio::test]
async fn a_page_reports_its_acknowledgements_oldest_first_in_max_results_sized_pages() {
    for sequence in ACK_SEQUENCES {
        let emulator = Emulator::new();
        let case = format!("{} acknowledgement(s)", sequence.len());
        let page = sent_page(&emulator, &case).await;

        // Acknowledge the page the sequence's way, and build the log that must result.
        let mut expected: Vec<(&str, Option<&str>)> = Vec::new();
        for ack in sequence.iter() {
            let mut request = json!({
                "PageId": page.arn,
                "ContactChannelId": page.contact_channel_arn,
                "AcceptType": ack.accept_type,
                "AcceptCode": if ack.ignore_code {
                    "000000".to_string()
                } else {
                    page.accept_code.clone()
                },
            });
            if ack.ignore_code {
                request["AcceptCodeValidation"] = json!("IGNORE");
            }
            if let Some(note) = ack.note {
                request["Note"] = json!(note);
            }
            let (status, body) = emulator.call("AcceptPage", request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected AcceptPage to answer 200, got {status} {body:?}"
            );
            expected.push((ack.accept_type, ack.note));
        }

        for max_results in PAGE_SIZES {
            let mut request = json!({"PageId": page.arn});
            if let Some(max_results) = max_results {
                request["MaxResults"] = json!(max_results);
            }
            let case = format!("{case}, MaxResults={max_results:?}");

            // One past the published maximum is refused by the modelled constraint, before
            // the handler runs.
            if max_results.is_some_and(|value| value > 1024) {
                let (status, body) = emulator.call("ListPageReceipts", request).await;
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected HTTP 400 past the MaxResults maximum, got \
                     {status} {body:?}"
                );
                continue;
            }

            let receipts = all_receipts(&emulator, request, &case).await;
            assert_eq!(
                receipt_types(&receipts),
                expected
                    .iter()
                    .map(|(kind, _)| (*kind).to_string())
                    .collect::<Vec<String>>(),
                "case {case:?}: expected the acknowledgements oldest first, got {receipts:?}"
            );
            for (index, (_, note)) in expected.iter().enumerate() {
                assert_eq!(
                    receipts[index]["ReceiptInfo"].as_str(),
                    *note,
                    "case {case:?}: expected receipt {index} to carry the note the \
                     acknowledgement did, got {:?}",
                    receipts[index]
                );
                assert_eq!(
                    receipts[index]["ContactChannelArn"].as_str(),
                    Some(page.contact_channel_arn.as_str()),
                    "case {case:?}: expected receipt {index} to name the paged channel, got \
                     {:?}",
                    receipts[index]
                );
                assert!(
                    receipts[index]["ReceiptTime"].as_f64().unwrap_or_default() > 1_600_000_000.0,
                    "case {case:?}: expected receipt {index} to carry the instant it \
                     happened, got {:?}",
                    receipts[index]
                );
            }
            // The log is chronological, so the instants never go backwards.
            let times: Vec<f64> = receipts
                .iter()
                .map(|receipt| receipt["ReceiptTime"].as_f64().unwrap_or_default())
                .collect();
            assert!(
                times.windows(2).all(|pair| pair[0] <= pair[1]),
                "case {case:?}: expected non-decreasing receipt times, got {times:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// 3. Listing is a read
// ---------------------------------------------------------------------------

#[tokio::test]
async fn listing_a_pages_receipts_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();
    let case = "a page listed four times";
    let page = sent_page(&emulator, case).await;

    let before = all_receipts(&emulator, json!({"PageId": page.arn}), case).await;
    assert!(
        before.is_empty(),
        "case {case:?}: expected a page nobody acknowledged to report no receipts, got \
         {before:?}"
    );
    for _ in 0..3 {
        let again = all_receipts(&emulator, json!({"PageId": page.arn}), case).await;
        assert_eq!(
            again, before,
            "case {case:?}: expected repeated listings to answer the same receipts"
        );
    }
    let stored = stored_page(&emulator, &page.arn);
    assert!(
        stored.receipts.is_empty(),
        "case {case:?}: expected listing to record no event, got {stored:?}"
    );
    assert!(
        stored.delivery_time.is_none() && stored.read_time.is_none(),
        "case {case:?}: expected listing to leave the engagement cycle where it was, got \
         {stored:?}"
    );
}

// ---------------------------------------------------------------------------
// 4. Refusals
// ---------------------------------------------------------------------------

/// `SsmContactsArn` forms the model's own pattern admits, none of which names a stored page.
///
/// The pattern is `^arn:(aws|aws-cn|aws-us-gov):ssm-contacts:[-\w+=\/,.@]*:[0-9]+:([\w+=\/,.@:-]+)*$`
/// (model: `ListPageReceipts$PageId`), so the rows span all three partitions and a
/// Region-less ARN.
const UNKNOWN_PAGES: &[&str] = &[
    "arn:aws:ssm-contacts:us-east-1:000000000000:page/oncall-primary/not-a-page",
    "arn:aws-cn:ssm-contacts:cn-north-1:000000000000:page/oncall-primary/not-a-page",
    "arn:aws-us-gov:ssm-contacts:us-gov-west-1:000000000000:page/oncall-primary/not-a-page",
    "arn:aws:ssm-contacts::000000000000:page/oncall-primary/not-a-page",
];

#[tokio::test]
async fn a_page_the_store_does_not_hold_is_refused_not_answered_with_an_empty_list() {
    let emulator = Emulator::new();
    for page_id in UNKNOWN_PAGES {
        let case = format!("an unknown page {page_id}");
        let (status, body) = emulator
            .call("ListPageReceipts", json!({"PageId": page_id}))
            .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404, got {status} {body:?}"
        );
        assert!(
            error_code(&body).contains("ResourceNotFoundException"),
            "case {case:?}: expected a ResourceNotFoundException, got {body:?}"
        );

        // The page is resolved before the token is read, so an unknown page carrying a token
        // this list never issued is still reported as the page being missing.
        let (status, body) = emulator
            .call(
                "ListPageReceipts",
                json!({
                    "PageId": page_id,
                    "NextToken": encode_next_token(TOKEN_SCOPE_CONTACTS, "x"),
                }),
            )
            .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404 for an unknown page with a foreign token, got \
             {status} {body:?}"
        );
    }
}

#[tokio::test]
async fn the_account_precondition_is_consulted_before_the_page_is() {
    let emulator = Emulator::new();
    let case = "the account has no replication set";
    let page = sent_page(&emulator, case).await;

    for page_id in [page.arn.as_str(), UNKNOWN_PAGES[0]] {
        for replication_set in [None, Some(not_yet_active())] {
            set_replication_set(&emulator, replication_set.clone());
            let case = format!("{case}: {page_id} with {replication_set:?}");
            let (status, body) = emulator
                .call("ListPageReceipts", json!({"PageId": page_id}))
                .await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} {body:?}"
            );
            assert!(
                error_code(&body).contains("ValidationException"),
                "case {case:?}: expected a ValidationException, got {body:?}"
            );
            assert!(
                body["Message"]
                    .as_str()
                    .unwrap_or_default()
                    .contains(page_id),
                "case {case:?}: expected the refusal to name the ARN the request carried, got \
                 {body:?}"
            );
        }
    }
}

/// A replication set that has not finished replicating: stored, but no home Region yet.
fn not_yet_active() -> service_com_amazonaws_ssmcontacts_ssmcontacts::state::AccountReplicationSet {
    service_com_amazonaws_ssmcontacts_ssmcontacts::state::AccountReplicationSet {
        arn: "arn:aws:ssm-incidents::000000000000:replication-set/emulator-set".to_string(),
        active: false,
    }
}

#[tokio::test]
async fn a_next_token_this_list_did_not_issue_is_refused() {
    let emulator = Emulator::new();
    let case = "a foreign pagination token";
    let page = sent_page(&emulator, case).await;

    // Rows: not hex at all, a token another list issued, and one of this list's own tokens
    // whose payload is not a position.
    let tokens: Vec<String> = vec![
        "not-a-token".to_string(),
        encode_next_token(TOKEN_SCOPE_CONTACTS, "oncall-primary"),
        encode_next_token(TOKEN_SCOPE_PAGE_RECEIPTS, "not-a-position"),
    ];
    for token in tokens {
        let case = format!("{case} {token}");
        let (status, body) = emulator
            .call(
                "ListPageReceipts",
                json!({"PageId": page.arn, "NextToken": token}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body:?}"
        );
        assert!(
            error_code(&body).contains("ValidationException"),
            "case {case:?}: expected a ValidationException, got {body:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// 5. Every page an engagement sent is listable
// ---------------------------------------------------------------------------

/// One plan to engage: the channels the contact owns, which of them each stage targets, and
/// how many pages the engagement must send.
struct PlanRow {
    case: &'static str,
    /// How many channels the contact owns.
    channels: usize,
    /// The stages of the plan, each naming indexes into those channels.
    stages: &'static [&'static [usize]],
    /// The channel indexes that must be paged, in the order the pages are sent.
    paged: &'static [usize],
}

const PLAN_ROWS: &[PlanRow] = &[
    PlanRow {
        case: "one stage, one channel",
        channels: 1,
        stages: &[&[0]],
        paged: &[0],
    },
    PlanRow {
        case: "two stages over three channels",
        channels: 3,
        stages: &[&[0, 1], &[2]],
        paged: &[0, 1, 2],
    },
    PlanRow {
        case: "a plan naming the same channel in two stages",
        channels: 1,
        stages: &[&[0], &[0]],
        paged: &[0],
    },
    PlanRow {
        case: "a plan with a stage but no targets",
        channels: 1,
        stages: &[&[]],
        paged: &[],
    },
    PlanRow {
        case: "a plan whose target order is not the creation order",
        channels: 2,
        stages: &[&[1], &[0]],
        paged: &[1, 0],
    },
];

#[tokio::test]
async fn every_page_an_engagement_sent_reports_its_own_receipt_log() {
    for row in PLAN_ROWS {
        let emulator = Emulator::new();
        let contact = create_contact(&emulator, "oncall-primary", "PERSONAL", Vec::new()).await;
        let mut channels: Vec<String> = Vec::new();
        for index in 0..row.channels {
            channels.push(create_channel(&emulator, &contact, index).await);
        }
        let stages: Vec<Value> = row
            .stages
            .iter()
            .map(|targets| {
                let targeted: Vec<&String> =
                    targets.iter().map(|index| &channels[*index]).collect();
                channel_stage(&targeted, 5)
            })
            .collect();
        set_plan(&emulator, &contact, stages, row.case).await;

        let engagement = start_engagement(&emulator, &contact, row.case).await;
        let pages = pages_of(&emulator, &engagement);
        let paged: Vec<String> = pages
            .iter()
            .map(|page| page.contact_channel_arn.clone())
            .collect();
        let expected: Vec<String> = row
            .paged
            .iter()
            .map(|index| channels[*index].clone())
            .collect();
        assert_eq!(
            paged, expected,
            "case {:?}: expected the engagement to page {expected:?}, got {paged:?}",
            row.case
        );

        // Each page is listable on its own, reports nothing until it is acknowledged, and
        // then reports only ITS own acknowledgement — one page's log is not another's.
        for page in &pages {
            let receipts = all_receipts(&emulator, json!({"PageId": page.arn}), row.case).await;
            assert!(
                receipts.is_empty(),
                "case {:?}: expected an unacknowledged page to report no receipts, got \
                 {receipts:?}",
                row.case
            );
        }
        if let Some(first) = pages.first() {
            let (status, body) = emulator
                .call(
                    "AcceptPage",
                    json!({
                        "PageId": first.arn,
                        "AcceptType": READ,
                        "AcceptCode": first.accept_code,
                        "Note": row.case,
                    }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {:?}: expected AcceptPage to answer 200, got {status} {body:?}",
                row.case
            );
            for page in &pages {
                let receipts = all_receipts(&emulator, json!({"PageId": page.arn}), row.case).await;
                let expected: Vec<String> = if page.arn == first.arn {
                    vec![READ.to_string()]
                } else {
                    Vec::new()
                };
                assert_eq!(
                    receipt_types(&receipts),
                    expected,
                    "case {:?}: expected only the acknowledged page to report a receipt, got \
                     {receipts:?} for {}",
                    row.case,
                    page.arn
                );
            }
        }
    }
}

#[tokio::test]
async fn an_escalation_plans_pages_each_report_their_own_log() {
    let emulator = Emulator::new();
    let case = "an escalation plan over two contacts";
    let primary = create_contact(&emulator, "oncall-primary", "PERSONAL", Vec::new()).await;
    let secondary = create_contact(&emulator, "oncall-secondary", "PERSONAL", Vec::new()).await;
    let primary_channel = create_channel(&emulator, &primary, 0).await;
    let secondary_channel = create_channel(&emulator, &secondary, 1).await;
    set_plan(
        &emulator,
        &primary,
        vec![channel_stage(&[&primary_channel], 5)],
        case,
    )
    .await;
    set_plan(
        &emulator,
        &secondary,
        vec![channel_stage(&[&secondary_channel], 5)],
        case,
    )
    .await;

    // The escalation plan targets the two contacts and — in its last stage — itself, which a
    // plan walk has to terminate on rather than page for ever.
    let escalation = create_contact(&emulator, "escalation-tier-1", "ESCALATION", Vec::new()).await;
    set_plan(
        &emulator,
        &escalation,
        vec![
            json!({
                "DurationInMinutes": 0,
                "Targets": [{"ContactTargetInfo": {"ContactId": primary, "IsEssential": true}}],
            }),
            json!({
                "DurationInMinutes": 10,
                "Targets": [
                    {"ContactTargetInfo": {"ContactId": secondary, "IsEssential": false}},
                    {"ContactTargetInfo": {"ContactId": escalation, "IsEssential": false}},
                ],
            }),
        ],
        case,
    )
    .await;

    let engagement = start_engagement(&emulator, &escalation, case).await;
    let pages = pages_of(&emulator, &engagement);
    let paged: Vec<String> = pages
        .iter()
        .map(|page| page.contact_channel_arn.clone())
        .collect();
    assert_eq!(
        paged,
        vec![primary_channel.clone(), secondary_channel.clone()],
        "case {case:?}: expected the escalation plan to page both contacts' channels once, got \
         {paged:?}"
    );

    // Acknowledging the escalated contact's page puts a receipt on THAT page only.
    let escalated = &pages[1];
    let (status, body) = emulator
        .call(
            "AcceptPage",
            json!({
                "PageId": escalated.arn,
                "ContactChannelId": escalated.contact_channel_arn,
                "AcceptType": DELIVERED,
                "AcceptCode": escalated.accept_code,
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected AcceptPage to answer 200, got {status} {body:?}"
    );
    let escalated_log = all_receipts(&emulator, json!({"PageId": escalated.arn}), case).await;
    assert_eq!(
        receipt_types(&escalated_log),
        vec![DELIVERED.to_string()],
        "case {case:?}: expected the escalated page to report its acknowledgement, got \
         {escalated_log:?}"
    );
    assert_eq!(
        escalated_log[0]["ContactChannelArn"].as_str(),
        Some(secondary_channel.as_str()),
        "case {case:?}: expected the receipt to name the escalated contact's channel, got \
         {escalated_log:?}"
    );
    let first_log = all_receipts(&emulator, json!({"PageId": pages[0].arn}), case).await;
    assert!(
        first_log.is_empty(),
        "case {case:?}: expected the unacknowledged page to report no receipts, got {first_log:?}"
    );
}
