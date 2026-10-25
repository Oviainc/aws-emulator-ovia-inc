// SPDX-License-Identifier: CC0-1.0

//! `ListPagesByContact`, end to end over the wire.
//!
//! Every case drives the production `MultiServiceRouter` with a buffered awsJson1_1 request
//! (`POST /`, `X-Amz-Target: SSMContacts.<Operation>`) through the harness the contact suite
//! builds, so what is asserted is what a signing AWS client would see.
//!
//! The rules under test, each asserted over a TABLE whose rows differ along what the rule
//! generalises over rather than over one call:
//!
//! 1. a page belongs to the contact whose contact channel was engaged, so the listing of a
//!    contact reports exactly the pages the engagements left on THAT contact's channels, in
//!    the order they were sent, each carrying the engagement it belongs to and that contact's
//!    ARN. The rows span one and several targets in a stage, targets spread over stages, a
//!    channel targeted twice, a stage whose wait has not run out, a stage with no targets at
//!    all and a plan with no stages;
//! 2. the pages an ESCALATION plan's stage produces are the pages of the contact the stage
//!    named, not of the escalation plan that was engaged: the engaged plan's own listing is
//!    empty and the named contact's reports them;
//! 3. a contact's channels are engaged by every engagement that reaches them, so repeated
//!    starts accumulate pages on the one contact, in send order, each naming its own
//!    engagement — and a listing reports the pages of the contact it NAMES and no other's;
//! 4. a page the clock has not reached is not reported, whichever side of now its send
//!    instant falls on;
//! 5. the listing is paginated: no page of results holds more than the `MaxResults` the
//!    request named, an opaque `NextToken` resumes exactly after the last entry, the final
//!    page carries none, and paging to exhaustion reports every page once in send order. The
//!    rows span `MaxResults` absent, its published minimum (0, which names no page size at
//!    all), sizes below, equal to and above the number of pages, and its published maximum;
//! 6. a request that cannot be served is refused with the modelled error: a contact the store
//!    does not hold with `ResourceNotFoundException` (HTTP 404), a `NextToken` this listing
//!    did not issue — including one a sibling listing issued — with `ValidationException`
//!    (HTTP 400), and, before the `ContactId` is resolved at all, an account with no ACTIVE
//!    Incident Manager replication set with `ValidationException` (HTTP 400).
//!
//! The listing is also a READ, which the first suite asserts for every row: repeating it
//! reports the same pages and leaves the store exactly as it was.
//!
//! A page summary carries no contact-channel ARN — the modelled `Page` does not have one — so
//! which channel a listed page engaged is read from the store, exactly as
//! `tests/all/page_acknowledgement.rs` reads a page's accept code.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_ssmcontacts_ssmcontacts::state::{
    encode_next_token, service_state, TOKEN_SCOPE_CONTACTS, TOKEN_SCOPE_PAGES_BY_ENGAGEMENT,
};

use crate::contact_lifecycle::{error_code, Emulator, ACCOUNT, REGION};
use crate::contact_updates::set_replication_set;
// A page's send instant is the plan's own schedule, which no modelled call moves, so the stop
// suite's store writer and its clock offset are what a row uses to place one.
use crate::engagement_stop_pages::{from_now, set_sent_time};
use crate::page_acknowledgement::{channel_target, describe_page, set_plan, CHANNELS};

/// The `Sender` every engagement this suite starts carries.
const SENDER: &str = "incident-manager";

/// The `IncidentId` every engagement this suite starts carries, so a page's own `IncidentId`
/// is asserted against a value that was not defaulted.
const INCIDENT: &str = "incident-2043";

// ---------------------------------------------------------------------------
// Fixture helpers
// ---------------------------------------------------------------------------

/// Create a contact with `channels` contact channels, answering its `ContactArn` and the ARNs
/// of its channels in creation order.
///
/// The channel catalog is the one the acknowledgement suite spans the model's `ChannelType`
/// set with, so a row asking for three channels gets one of each kind.
async fn contact_with_channels(
    emulator: &Emulator,
    alias: &str,
    channels: usize,
    case: &str,
) -> (String, Vec<String>) {
    let contact = emulator
        .create_contact(
            json!({"Alias": alias, "Type": "PERSONAL", "Plan": {"Stages": []}}),
            case,
        )
        .await;
    let mut arns: Vec<String> = Vec::new();
    for index in 0..channels {
        let (name, channel_type, address) = CHANNELS[index % CHANNELS.len()];
        arns.push(
            emulator
                .create_channel(
                    json!({
                        "ContactId": contact,
                        "Name": format!("{name}-{index}"),
                        "Type": channel_type,
                        "DeliveryAddress": {"SimpleAddress": address},
                    }),
                    case,
                )
                .await,
        );
    }
    (contact, arns)
}

/// A plan whose stages are `stages`: each entry is a `DurationInMinutes` and the indices into
/// `channels` its `Targets` name.
fn plan_of(stages: &[(i64, &[usize])], channels: &[String]) -> Value {
    let rendered: Vec<Value> = stages
        .iter()
        .map(|(duration, targets)| {
            json!({
                "DurationInMinutes": duration,
                "Targets": targets
                    .iter()
                    .map(|index| channel_target(&channels[*index]))
                    .collect::<Vec<Value>>(),
            })
        })
        .collect();
    json!({"Stages": rendered})
}

/// Start an engagement of `contact`, asserting it succeeded, and answer its `EngagementArn`.
async fn start_engagement(emulator: &Emulator, contact: &str, case: &str) -> String {
    let (status, body) = emulator
        .call(
            "StartEngagement",
            json!({
                "ContactId": contact,
                "Sender": SENDER,
                "Subject": "Database latency",
                "Content": "p99 write latency is over budget.",
                "IncidentId": INCIDENT,
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

/// `PageArn` -> the contact channel that page engaged, as the store holds it.
///
/// The modelled `Page` carries no channel ARN, so this is how a row says which channels the
/// listed pages engaged.
fn paged_channel_by_page(emulator: &Emulator) -> BTreeMap<String, String> {
    let state = service_state(&*emulator.engine).expect("the engine holds this service's state");
    let state = state.lock();
    state
        .pages
        .values()
        .map(|page| (page.arn.clone(), page.contact_channel_arn.clone()))
        .collect()
}

/// How many pages the store holds, and how many it has ever sent — the pair a read must
/// leave exactly as it found it.
fn page_counters(emulator: &Emulator) -> (usize, u64) {
    let state = service_state(&*emulator.engine).expect("the engine holds this service's state");
    let state = state.lock();
    (state.pages.len(), state.pages_sent)
}

/// `ListPagesByContact`, answering the status and the body.
async fn list_pages(emulator: &Emulator, request: Value) -> (u16, Value) {
    emulator.call("ListPagesByContact", request).await
}

/// The pages `ListPagesByContact` reports for `contact`, asserting it answered 200.
async fn listed_pages(emulator: &Emulator, contact: &str, case: &str) -> Vec<Value> {
    let (status, body) = list_pages(emulator, json!({"ContactId": contact})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListPagesByContact to answer 200, got {status} {body:?}"
    );
    assert!(
        body["NextToken"].as_str().is_none(),
        "case {case:?}: an unpaginated listing of at most a handful of pages must be the \
         final page, got {body:?}"
    );
    body["Pages"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected a Pages list, got {body:?}"))
}

/// The `PageArn`s a listing reported, in the order it reported them.
fn page_arns(pages: &[Value], case: &str) -> Vec<String> {
    pages
        .iter()
        .map(|page| {
            page["PageArn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a PageArn, got {page:?}"))
                .to_string()
        })
        .collect()
}

/// A well-formed `SsmContactsArn` for a contact nothing ever created.
fn absent_contact_arn() -> String {
    format!("arn:aws:ssm-contacts:{REGION}:{ACCOUNT}:contact/no-such-contact")
}

// ---------------------------------------------------------------------------
// 1. the pages of a contact are the pages its own channels were engaged with
// ---------------------------------------------------------------------------

/// One plan shape, and which of the contact's channels engaging it pages right away.
struct PlanCase {
    case: &'static str,
    /// How many channels the engaged contact holds.
    channels: usize,
    /// The plan's stages: a `DurationInMinutes` (inside the modelled `StageDurationInMins`
    /// range 0..30) and the channel indices the stage targets.
    stages: &'static [(i64, &'static [usize])],
    /// The channel indices the listing must report, in the order it must report them. A
    /// target in a stage whose wait has not run out is NOT here: its page is not sent yet.
    expected: &'static [usize],
}

const PLAN_CASES: &[PlanCase] = &[
    PlanCase {
        case: "one stage targeting one channel",
        channels: 1,
        stages: &[(0, &[0])],
        expected: &[0],
    },
    PlanCase {
        case: "one stage targeting three channels",
        channels: 3,
        stages: &[(0, &[0, 1, 2])],
        expected: &[0, 1, 2],
    },
    PlanCase {
        case: "two stages that begin at the same instant",
        channels: 2,
        stages: &[(0, &[0]), (0, &[1])],
        expected: &[0, 1],
    },
    PlanCase {
        case: "three stages that begin at the same instant",
        channels: 3,
        stages: &[(0, &[2]), (0, &[1]), (0, &[0])],
        expected: &[2, 1, 0],
    },
    PlanCase {
        case: "a second stage whose wait has not run out",
        channels: 2,
        stages: &[(5, &[0]), (0, &[1])],
        expected: &[0],
    },
    PlanCase {
        case: "a stage half an hour out, the model's longest wait",
        channels: 2,
        stages: &[(30, &[0]), (0, &[1])],
        expected: &[0],
    },
    PlanCase {
        case: "the same channel targeted by two stages",
        channels: 1,
        stages: &[(0, &[0]), (0, &[0])],
        expected: &[0],
    },
    PlanCase {
        case: "a stage with no targets",
        channels: 1,
        stages: &[(0, &[])],
        expected: &[],
    },
    PlanCase {
        case: "a plan with no stages at all",
        channels: 2,
        stages: &[],
        expected: &[],
    },
];

#[tokio::test]
async fn pages_by_contact_reports_the_pages_the_plan_engaged() {
    for row in PLAN_CASES {
        let emulator = Emulator::new();
        let (contact, channels) =
            contact_with_channels(&emulator, "oncall-primary", row.channels, row.case).await;
        if !row.stages.is_empty() {
            set_plan(
                &emulator,
                &contact,
                plan_of(row.stages, &channels),
                row.case,
            )
            .await;
        }

        let engagement = start_engagement(&emulator, &contact, row.case).await;
        let before = page_counters(&emulator);
        let pages = listed_pages(&emulator, &contact, row.case).await;

        let channel_of = paged_channel_by_page(&emulator);
        let engaged: Vec<String> = page_arns(&pages, row.case)
            .into_iter()
            .map(|arn| {
                channel_of
                    .get(&arn)
                    .cloned()
                    .unwrap_or_else(|| panic!("case {:?}: the store holds no page {arn:?}", row.case))
            })
            .collect();
        let expected: Vec<String> = row
            .expected
            .iter()
            .map(|index| channels[*index].clone())
            .collect();
        assert_eq!(
            engaged, expected,
            "case {:?}: expected the listing to report the pages of {expected:?}, got {engaged:?}",
            row.case
        );

        for page in &pages {
            assert_eq!(
                page["EngagementArn"].as_str(),
                Some(engagement.as_str()),
                "case {:?}: expected every page to name the engagement that sent it, got {page:?}",
                row.case
            );
            assert_eq!(
                page["ContactArn"].as_str(),
                Some(contact.as_str()),
                "case {:?}: expected every page to name the contact whose channel it engaged, \
                 got {page:?}",
                row.case
            );
            assert_eq!(
                page["Sender"].as_str(),
                Some(SENDER),
                "case {:?}: expected the engagement's sender, got {page:?}",
                row.case
            );
            assert_eq!(
                page["IncidentId"].as_str(),
                Some(INCIDENT),
                "case {:?}: expected the engagement's incident, got {page:?}",
                row.case
            );
            assert!(
                page.get("SentTime").and_then(Value::as_f64).is_some(),
                "case {:?}: expected a sent page to carry a SentTime, got {page:?}",
                row.case
            );
            assert!(
                page.get("DeliveryTime").is_none() && page.get("ReadTime").is_none(),
                "case {:?}: a page nobody answered carries neither acknowledgement instant, \
                 got {page:?}",
                row.case
            );

            // Round trip: every listed page is readable through `DescribePage`, which is the
            // reader that reports the message bodies the summary leaves out.
            let page_arn = page["PageArn"].as_str().unwrap_or_default();
            let (status, described) = describe_page(&emulator, page_arn).await;
            assert_eq!(
                status, 200,
                "case {:?}: expected DescribePage to answer 200 for a listed page, got \
                 {status} {described:?}",
                row.case
            );
            assert_eq!(
                described["EngagementArn"].as_str(),
                Some(engagement.as_str()),
                "case {:?}: expected the described page to name the same engagement, got \
                 {described:?}",
                row.case
            );
            assert_eq!(
                described["ContactArn"].as_str(),
                Some(contact.as_str()),
                "case {:?}: expected the described page to name the same contact, got \
                 {described:?}",
                row.case
            );
        }

        // The listing is a READ: repeating it reports the same pages and writes nothing.
        let again = listed_pages(&emulator, &contact, row.case).await;
        assert_eq!(
            page_arns(&again, row.case),
            page_arns(&pages, row.case),
            "case {:?}: repeating the listing must report the same pages",
            row.case
        );
        assert_eq!(
            page_counters(&emulator),
            before,
            "case {:?}: listing a contact's pages must leave the store as it was",
            row.case
        );
    }
}

// ---------------------------------------------------------------------------
// 2. an escalation stage pages the contact it names
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pages_by_contact_follows_an_escalation_stage_to_the_paged_contact() {
    let case = "an escalation plan engaging a personal contact";
    let emulator = Emulator::new();
    let (primary, channels) = contact_with_channels(&emulator, "oncall-primary", 2, case).await;
    set_plan(
        &emulator,
        &primary,
        plan_of(&[(0, &[0, 1])], &channels),
        case,
    )
    .await;

    let escalation = emulator
        .create_contact(
            json!({
                "Alias": "escalation-tier-1",
                "Type": "ESCALATION",
                "Plan": {"Stages": [{
                    "DurationInMinutes": 0,
                    "Targets": [{"ContactTargetInfo": {
                        "ContactId": primary,
                        "IsEssential": true,
                    }}],
                }]},
            }),
            case,
        )
        .await;

    let engagement = start_engagement(&emulator, &escalation, case).await;

    let escalated = listed_pages(&emulator, &escalation, case).await;
    assert!(
        escalated.is_empty(),
        "case {case:?}: an escalation plan holds no contact channel of its own, so its own \
         listing reports nothing, got {escalated:?}"
    );

    let paged = listed_pages(&emulator, &primary, case).await;
    assert_eq!(
        paged.len(),
        2,
        "case {case:?}: expected the escalation stage to page both of the named contact's \
         channels, got {paged:?}"
    );
    for page in &paged {
        assert_eq!(
            page["ContactArn"].as_str(),
            Some(primary.as_str()),
            "case {case:?}: a page belongs to the contact whose channel it engaged, got {page:?}"
        );
        assert_eq!(
            page["EngagementArn"].as_str(),
            Some(engagement.as_str()),
            "case {case:?}: expected the escalation's own engagement, got {page:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// 3. pages accumulate per contact, and a listing reports only that contact's
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pages_by_contact_accumulates_over_engagements_and_isolates_contacts() {
    let case = "two contacts engaged a different number of times";
    let emulator = Emulator::new();
    let (primary, primary_channels) =
        contact_with_channels(&emulator, "oncall-primary", 2, case).await;
    set_plan(
        &emulator,
        &primary,
        plan_of(&[(0, &[0, 1])], &primary_channels),
        case,
    )
    .await;
    let (secondary, secondary_channels) =
        contact_with_channels(&emulator, "oncall-secondary", 1, case).await;
    set_plan(
        &emulator,
        &secondary,
        plan_of(&[(0, &[0])], &secondary_channels),
        case,
    )
    .await;

    let first = start_engagement(&emulator, &primary, case).await;
    let second = start_engagement(&emulator, &primary, case).await;
    let other = start_engagement(&emulator, &secondary, case).await;
    assert_ne!(
        first, second,
        "case {case:?}: two starts of one contact are two engagements"
    );

    let paged = listed_pages(&emulator, &primary, case).await;
    let engagements: Vec<&str> = paged
        .iter()
        .map(|page| page["EngagementArn"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        engagements,
        vec![
            first.as_str(),
            first.as_str(),
            second.as_str(),
            second.as_str()
        ],
        "case {case:?}: expected both channels paged by each start, in send order, got {paged:?}"
    );

    let other_pages = listed_pages(&emulator, &secondary, case).await;
    assert_eq!(
        other_pages.len(),
        1,
        "case {case:?}: expected the other contact's single channel to hold one page, got \
         {other_pages:?}"
    );
    assert_eq!(
        other_pages[0]["EngagementArn"].as_str(),
        Some(other.as_str()),
        "case {case:?}: expected the other contact's own engagement, got {other_pages:?}"
    );

    let primary_arns = page_arns(&paged, case);
    for arn in page_arns(&other_pages, case) {
        assert!(
            !primary_arns.contains(&arn),
            "case {case:?}: no page belongs to two contacts, {arn:?} is in both listings"
        );
    }
}

// ---------------------------------------------------------------------------
// 4. a page the clock has not reached is not reported
// ---------------------------------------------------------------------------

/// Where a page's send instant sits relative to now, and whether the listing reports it.
const SEND_INSTANTS: &[(i64, bool)] = &[
    (-3600, true),
    (-5, true),
    (0, true),
    (5, false),
    (3600, false),
];

#[tokio::test]
async fn pages_by_contact_reports_only_pages_already_sent() {
    for (offset, reported) in SEND_INSTANTS {
        let case = format!("a page sent {offset}s from now");
        let emulator = Emulator::new();
        let (contact, channels) = contact_with_channels(&emulator, "oncall-primary", 1, &case).await;
        set_plan(&emulator, &contact, plan_of(&[(0, &[0])], &channels), &case).await;
        start_engagement(&emulator, &contact, &case).await;

        let pages = listed_pages(&emulator, &contact, &case).await;
        let page_arn = page_arns(&pages, &case)
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("case {case:?}: the start must have sent one page"));
        set_sent_time(&emulator, &page_arn, from_now(*offset));

        let after = listed_pages(&emulator, &contact, &case).await;
        assert_eq!(
            !after.is_empty(),
            *reported,
            "case {case:?}: expected the page to be reported = {reported}, got {after:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// 5. pagination
// ---------------------------------------------------------------------------

/// The `MaxResults` values the pagination table spans: absent, the published minimum (0),
/// below, equal to and above the number of pages, and the published maximum.
const PAGE_SIZES: &[Option<i64>] = &[None, Some(0), Some(1), Some(2), Some(3), Some(4), Some(1024)];

#[tokio::test]
async fn pages_by_contact_pages_through_every_page_once() {
    for max_results in PAGE_SIZES {
        let case = format!("MaxResults={max_results:?}");
        let emulator = Emulator::new();
        let (contact, channels) = contact_with_channels(&emulator, "oncall-primary", 3, &case).await;
        set_plan(
            &emulator,
            &contact,
            plan_of(&[(0, &[0, 1, 2])], &channels),
            &case,
        )
        .await;
        start_engagement(&emulator, &contact, &case).await;

        let expected = page_arns(&listed_pages(&emulator, &contact, &case).await, &case);
        assert_eq!(
            expected.len(),
            3,
            "case {case:?}: the fixture pages three channels, got {expected:?}"
        );

        let mut request = json!({"ContactId": contact});
        if let Some(limit) = max_results {
            request["MaxResults"] = json!(limit);
        }
        let mut collected: Vec<String> = Vec::new();
        loop {
            let (status, body) = list_pages(&emulator, request.clone()).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected ListPagesByContact to answer 200, got {status} {body:?}"
            );
            let page = body["Pages"].as_array().cloned().unwrap_or_default();
            if let Some(limit) = max_results.filter(|limit| *limit > 0) {
                assert!(
                    page.len() as i64 <= limit,
                    "case {case:?}: a page of results held {} entries, got {body:?}",
                    page.len()
                );
            }
            collected.extend(page_arns(&page, &case));
            match body["NextToken"].as_str() {
                Some(token) => {
                    assert!(
                        collected.len() <= 16,
                        "case {case:?}: paging did not terminate, collected {collected:?}"
                    );
                    request["NextToken"] = json!(token);
                }
                None => break,
            }
        }
        assert_eq!(
            collected, expected,
            "case {case:?}: paging to exhaustion must report every page once, in send order"
        );
    }
}

// ---------------------------------------------------------------------------
// 6. refusals
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pages_by_contact_refuses_a_request_it_cannot_serve() {
    let emulator = Emulator::new();
    let (contact, channels) =
        contact_with_channels(&emulator, "oncall-primary", 1, "the refusal fixture").await;
    set_plan(
        &emulator,
        &contact,
        plan_of(&[(0, &[0])], &channels),
        "the refusal fixture",
    )
    .await;
    start_engagement(&emulator, &contact, "the refusal fixture").await;

    let (status, body) = list_pages(&emulator, json!({"ContactId": absent_contact_arn()})).await;
    assert_eq!(
        status, 404,
        "case \"a contact the store does not hold\": expected 404, got {status} {body:?}"
    );
    assert!(
        error_code(&body).ends_with("ResourceNotFoundException"),
        "case \"a contact the store does not hold\": expected ResourceNotFoundException, got \
         {body:?}"
    );

    // A token this listing did not issue: one that is not a token at all, one a SIBLING
    // listing issued, and one scoped to another list entirely.
    let foreign_tokens = [
        "nottoken".to_string(),
        encode_next_token(TOKEN_SCOPE_PAGES_BY_ENGAGEMENT, "0"),
        encode_next_token(TOKEN_SCOPE_CONTACTS, "oncall-primary"),
    ];
    for token in foreign_tokens {
        let (status, body) = list_pages(
            &emulator,
            json!({"ContactId": contact, "NextToken": token}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {token:?}: expected a token this listing did not issue to be refused 400, \
             got {status} {body:?}"
        );
        assert!(
            error_code(&body).ends_with("ValidationException"),
            "case {token:?}: expected ValidationException, got {body:?}"
        );
    }

    // An EMPTY `NextToken` is inside the modelled length and names no resume point, so it is
    // served exactly as an absent one.
    let (status, body) = list_pages(&emulator, json!({"ContactId": contact, "NextToken": ""})).await;
    assert_eq!(
        status, 200,
        "case \"an empty NextToken\": expected 200, got {status} {body:?}"
    );
    assert_eq!(
        body["Pages"].as_array().map(Vec::len),
        Some(1),
        "case \"an empty NextToken\": expected the first page of results, got {body:?}"
    );

    // The account gate is evaluated BEFORE the `ContactId` is resolved, so a contact the
    // store DOES hold is refused the same way once the account's replication set is gone.
    set_replication_set(&emulator, None);
    for named in [contact.clone(), absent_contact_arn()] {
        let (status, body) = list_pages(&emulator, json!({"ContactId": named})).await;
        assert_eq!(
            status, 400,
            "case {named:?}: an account with no ACTIVE replication set is refused 400, got \
             {status} {body:?}"
        );
        assert!(
            error_code(&body).ends_with("ValidationException"),
            "case {named:?}: expected ValidationException, got {body:?}"
        );
    }
}
