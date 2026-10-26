// SPDX-License-Identifier: CC0-1.0

//! What `ListPagesByContact` reports about a page that has been ACKNOWLEDGED, end to end
//! over the wire.
//!
//! Every case drives the production `MultiServiceRouter` with a buffered awsJson1_1 request
//! (`POST /`, `X-Amz-Target: SSMContacts.<Operation>`) through the harness the contact suite
//! builds, so what is asserted is what a signing AWS client would see.
//!
//! The rule under test: a pages listing rebuilds each page from what the store holds for
//! THAT page, so the acknowledgement instants it reports are the ones the acknowledgements
//! recorded — the instant a page was FIRST acknowledged delivered, and the instant it was
//! first acknowledged read, each absent until that acknowledgement happened, neither moved
//! by a repeat and neither reported for a page nobody answered. `tests/all/pages_by_contact.rs`
//! asserts which pages the listing reports and in what order; this suite asserts what it
//! reports ABOUT one, which is the half of a page's record another call can change after the
//! engagement started.
//!
//! The rows span the acknowledgement sequences one caller can send against one page: none at
//! all, each `AcceptType` the model declares once, both in either order, each repeated, and
//! the whole cycle with a repeat of each type. Every sequence is asserted step by step, so a
//! stamped instant is checked against the one the listing first reported rather than only at
//! the end, and the sibling page of the same engagement is asserted untouched throughout.
//!
//! The listing is also a READ: it is repeated on every row and must report the same instants
//! and leave the stored ones exactly as they were — an acknowledgement instant is what
//! `AcceptPage` recorded, never something a listing stamps.

use serde_json::{json, Value};
use service_com_amazonaws_ssmcontacts_ssmcontacts::ops::types::ACCEPT_TYPE_VALUES;
use service_com_amazonaws_ssmcontacts_ssmcontacts::state::PageRecord;

use crate::contact_lifecycle::Emulator;
use crate::page_acknowledgement::{
    describe_page, now_secs, pages_of, set_plan, start_engagement, wire_time,
};
use crate::pages_by_contact::{contact_with_channels, plan_of};

/// The `AcceptType` value that acknowledges a page as read, from the model's own `AcceptType`
/// value set (`ACCEPT_TYPE_VALUES` in `src/ops/types/shared_types.rs`); its other value,
/// `DELIVERED`, stamps the delivery.
const READ: &str = "READ";

/// The `AcceptType` value that acknowledges a page as delivered. See [`READ`].
const DELIVERED: &str = "DELIVERED";

/// One acknowledgement sequence sent against the FIRST page of an engagement.
struct AcknowledgementCase {
    case: &'static str,
    /// The `AcceptType`s acknowledged against that page, in order. Every entry is a value of
    /// the model's own `AcceptType` set, which the test asserts the table spans.
    steps: &'static [&'static str],
    /// Whether the listing must report a `DeliveryTime` once the whole sequence has run.
    delivered: bool,
    /// Whether the listing must report a `ReadTime` once the whole sequence has run.
    read: bool,
}

const ACKNOWLEDGEMENT_CASES: &[AcknowledgementCase] = &[
    AcknowledgementCase {
        case: "a page nobody answered",
        steps: &[],
        delivered: false,
        read: false,
    },
    AcknowledgementCase {
        case: "delivered once",
        steps: &[DELIVERED],
        delivered: true,
        read: false,
    },
    AcknowledgementCase {
        case: "read once, with no delivery ever acknowledged",
        steps: &[READ],
        delivered: false,
        read: true,
    },
    AcknowledgementCase {
        case: "delivered then read",
        steps: &[DELIVERED, READ],
        delivered: true,
        read: true,
    },
    AcknowledgementCase {
        case: "read then delivered",
        steps: &[READ, DELIVERED],
        delivered: true,
        read: true,
    },
    AcknowledgementCase {
        case: "delivered twice",
        steps: &[DELIVERED, DELIVERED],
        delivered: true,
        read: false,
    },
    AcknowledgementCase {
        case: "read twice",
        steps: &[READ, READ],
        delivered: false,
        read: true,
    },
    AcknowledgementCase {
        case: "the whole cycle, with a repeat of each type",
        steps: &[DELIVERED, READ, DELIVERED, READ],
        delivered: true,
        read: true,
    },
];

/// How many channels the fixture contact holds, so every row has a sibling page that the
/// acknowledgements never touch.
const CHANNELS_PER_CONTACT: usize = 2;

#[tokio::test]
async fn pages_by_contact_reports_the_acknowledgement_instants_the_store_holds() {
    // The rows claim to span the model's own value set, so a value added to the model shows
    // up here rather than silently going untested.
    for value in ACCEPT_TYPE_VALUES {
        assert!(
            ACKNOWLEDGEMENT_CASES
                .iter()
                .any(|row| row.steps.contains(value)),
            "case {value:?}: the table must acknowledge every modelled AcceptType, \
             ACCEPT_TYPE_VALUES is {ACCEPT_TYPE_VALUES:?}"
        );
    }

    for row in ACKNOWLEDGEMENT_CASES {
        let case = row.case;
        let emulator = Emulator::new();
        let (contact, channels) =
            contact_with_channels(&emulator, "oncall-primary", CHANNELS_PER_CONTACT, case).await;
        // One stage, due the instant the engagement starts, targeting both channels: the
        // engagement sends one page per channel and the listing reports both right away.
        set_plan(
            &emulator,
            &contact,
            plan_of(&[(0, &[0, 1])], &channels),
            case,
        )
        .await;

        let engagement = start_engagement(&emulator, &contact, case).await;
        let sent = pages_of(&emulator, &engagement);
        assert_eq!(
            sent.len(),
            CHANNELS_PER_CONTACT,
            "case {case:?}: expected one page per targeted channel, got {:?}",
            sent.iter().map(|page| page.arn.clone()).collect::<Vec<_>>()
        );
        let answered = sent[0].clone();
        let untouched = sent[1].clone();

        // Before any acknowledgement neither page carries either instant.
        for page_arn in [&answered.arn, &untouched.arn] {
            let listed = listed_page(&emulator, &contact, page_arn, case).await;
            assert!(
                wire_time(&listed, "DeliveryTime").is_none()
                    && wire_time(&listed, "ReadTime").is_none(),
                "case {case:?}: expected an unanswered page to carry neither acknowledgement \
                 instant, got {listed:?}"
            );
        }

        // What the listing reported the first time each type was acknowledged, which a later
        // acknowledgement of the same type must not move.
        let mut first_delivery: Option<f64> = None;
        let mut first_read: Option<f64> = None;

        for step in row.steps {
            let before = now_secs();
            accept(&emulator, &answered, step, case).await;
            let after = now_secs();

            let listed = listed_page(&emulator, &contact, &answered.arn, case).await;
            let delivery = wire_time(&listed, "DeliveryTime");
            let read = wire_time(&listed, "ReadTime");
            let (reported, other, first_stamped, first_other) = if *step == READ {
                (read, delivery, &mut first_read, &mut first_delivery)
            } else {
                (delivery, read, &mut first_delivery, &mut first_read)
            };

            let at = reported.unwrap_or_else(|| {
                panic!(
                    "case {case:?}: acknowledging {step} must make the listing report that \
                     instant, got {listed:?}"
                )
            });
            match *first_stamped {
                Some(stamped) => assert_eq!(
                    at, stamped,
                    "case {case:?}: acknowledging {step} again must leave the instant the \
                     listing reports where it was, expected {stamped}, got {at}"
                ),
                None => {
                    assert!(
                        at >= before - 1.0 && at <= after + 1.0,
                        "case {case:?}: expected the {step} instant inside \
                         [{before}, {after}], got {at}"
                    );
                    *first_stamped = Some(at);
                }
            }
            assert_eq!(
                other, *first_other,
                "case {case:?}: acknowledging {step} must leave the other instant as it was, \
                 expected {first_other:?}, got {other:?}"
            );

            // The acknowledgement was addressed to ONE page, so the sibling page of the same
            // engagement still carries neither instant.
            let sibling = listed_page(&emulator, &contact, &untouched.arn, case).await;
            assert!(
                wire_time(&sibling, "DeliveryTime").is_none()
                    && wire_time(&sibling, "ReadTime").is_none(),
                "case {case:?}: acknowledging one page must leave the sibling page \
                 unacknowledged, got {sibling:?}"
            );
        }

        // The sequence has run: the listing reports exactly the instants the row expects.
        let listed = listed_page(&emulator, &contact, &answered.arn, case).await;
        assert_eq!(
            wire_time(&listed, "DeliveryTime").is_some(),
            row.delivered,
            "case {case:?}: expected DeliveryTime present={}, got {listed:?}",
            row.delivered
        );
        assert_eq!(
            wire_time(&listed, "ReadTime").is_some(),
            row.read,
            "case {case:?}: expected ReadTime present={}, got {listed:?}",
            row.read
        );

        // Round trip: `DescribePage` is the reader that reports one page in full, and it
        // reports the same instants for the same page, because both rebuild them from the one
        // record.
        let (status, described) = describe_page(&emulator, &answered.arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribePage to answer 200 for a listed page, got \
             {status} {described:?}"
        );
        for field in ["SentTime", "DeliveryTime", "ReadTime"] {
            assert_eq!(
                wire_time(&listed, field),
                wire_time(&described, field),
                "case {case:?}: expected the listing and DescribePage to agree on {field}, \
                 got {listed:?} and {described:?}"
            );
        }
        for field in ["PageArn", "EngagementArn", "ContactArn", "Sender", "IncidentId"] {
            assert_eq!(
                listed.get(field),
                described.get(field),
                "case {case:?}: expected the listing and DescribePage to agree on {field}, \
                 got {listed:?} and {described:?}"
            );
        }

        // The listing is a READ: repeating it reports the same instants and the stored ones
        // are exactly what they were before it ran.
        let stored_before = acknowledgement_instants(&pages_of(&emulator, &engagement));
        let again = listed_page(&emulator, &contact, &answered.arn, case).await;
        assert_eq!(
            again, listed,
            "case {case:?}: repeating the listing must report the same page, got {again:?} \
             and {listed:?}"
        );
        let stored_after = acknowledgement_instants(&pages_of(&emulator, &engagement));
        assert_eq!(
            stored_after, stored_before,
            "case {case:?}: listing a contact's pages must stamp nothing, got \
             {stored_after:?} after {stored_before:?}"
        );
    }
}

/// Acknowledge `page` with `accept_type`, presenting the accept code Incident Manager sent
/// with that page, and assert the call succeeded.
///
/// The code is read from the store because AWS delivers it to the channel out of band and
/// puts it on no response, exactly as `tests/all/page_acknowledgement.rs` reads it.
async fn accept(emulator: &Emulator, page: &PageRecord, accept_type: &str, case: &str) {
    let (status, body) = emulator
        .call(
            "AcceptPage",
            json!({
                "PageId": page.arn,
                "AcceptType": accept_type,
                "AcceptCode": page.accept_code,
                "Note": format!("acknowledged as {accept_type}"),
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected AcceptPage({accept_type}) to answer 200, got {status} {body:?}"
    );
}

/// The one page `page_arn` names, as `ListPagesByContact` reports it for `contact`.
async fn listed_page(emulator: &Emulator, contact: &str, page_arn: &str, case: &str) -> Value {
    let (status, body) = emulator
        .call("ListPagesByContact", json!({"ContactId": contact}))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListPagesByContact to answer 200, got {status} {body:?}"
    );
    let pages = body["Pages"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected a Pages list, got {body:?}"));
    pages
        .into_iter()
        .find(|page| page["PageArn"].as_str() == Some(page_arn))
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected the listing to report page {page_arn:?}, got {body:?}")
        })
}

/// Every page's stored acknowledgement instants, keyed by `PageArn`: the pair a read must
/// leave exactly as it found it.
fn acknowledgement_instants(
    pages: &[PageRecord],
) -> Vec<(
    String,
    Option<std::time::SystemTime>,
    Option<std::time::SystemTime>,
)> {
    pages
        .iter()
        .map(|page| (page.arn.clone(), page.delivery_time, page.read_time))
        .collect()
}
