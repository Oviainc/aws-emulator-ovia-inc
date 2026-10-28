// SPDX-License-Identifier: CC0-1.0

//! An entitlement's `Subscribers` are ACCOUNTS, end to end over HTTP: every entry of the
//! list has to be able to be an Amazon Web Services account ID, on each of the three paths
//! that write it — CreateFlow's `Entitlements`, GrantFlowEntitlements' `Entitlements` and
//! UpdateFlowEntitlement's `Subscribers`.
//!
//! The rule is asserted over a TABLE spanning the class rather than the one value that
//! exposed the gap: the rows differ along the FORM of the entry (a name, an ARN, digits
//! with a non-digit mixed in, non-ASCII digits, whitespace), its LENGTH either side of the
//! fixed width, the COUNT of entries and the POSITION of the unusable one in the list, with
//! accepted rows at both published ends of the 12-digit range.
//!
//! Every case drives the real request path (routing, restJson1 deserialization, constraint
//! validation, handler, serialization) through the harness
//! [`crate::flow_lifecycle::Emulator`], so what these tests assert is what a MediaConnect
//! client sees on the wire.

use serde_json::{json, Value};

use crate::flow_entitlements::{entitlements, grant};
use crate::flow_lifecycle::Emulator;

/// An account ID used wherever a row needs a usable subscriber: "the Amazon Web Services
/// account IDs that you want to share your content with"
/// (`smithy_query(query="doc",shape="GrantEntitlementRequest",member="Subscribers")`).
const SUBSCRIBER: &str = "111122223333";

/// One subscriber list, and whether the service stores an entitlement carrying it.
struct SubscriberCase {
    case: &'static str,
    subscribers: Vec<&'static str>,
    accepted: bool,
}

/// The subscriber lists asserted below.
///
/// The accepted rows span the whole 12-digit account-ID range — its lowest and highest
/// values — and the list sizes the member admits, including the empty list (an entitlement
/// shared with nobody is a value, not a malformed one). The refused rows span the forms a
/// string can take that no account ID does, and place the unusable entry first, last and
/// alone so the check cannot be passing only on the first element.
fn subscriber_cases() -> Vec<SubscriberCase> {
    vec![
        SubscriberCase {
            case: "one account ID",
            subscribers: vec![SUBSCRIBER],
            accepted: true,
        },
        SubscriberCase {
            case: "the lowest and the highest 12-digit account ID",
            subscribers: vec!["000000000000", "999999999999"],
            accepted: true,
        },
        SubscriberCase {
            case: "three distinct account IDs",
            subscribers: vec![SUBSCRIBER, "222233334444", "333344445555"],
            accepted: true,
        },
        SubscriberCase {
            case: "no subscribers at all: shared with nobody",
            subscribers: vec![],
            accepted: true,
        },
        SubscriberCase {
            case: "a name where an account ID belongs",
            subscribers: vec!["not-an-account"],
            accepted: false,
        },
        SubscriberCase {
            case: "far fewer digits than an account ID has",
            subscribers: vec!["12345"],
            accepted: false,
        },
        SubscriberCase {
            case: "eleven digits, one short of the width",
            subscribers: vec!["11112222333"],
            accepted: false,
        },
        SubscriberCase {
            case: "thirteen digits, one past the width",
            subscribers: vec!["1111222233334"],
            accepted: false,
        },
        SubscriberCase {
            case: "the empty string",
            subscribers: vec![""],
            accepted: false,
        },
        SubscriberCase {
            case: "the account's ARN rather than its ID",
            subscribers: vec!["arn:aws:iam::111122223333:root"],
            accepted: false,
        },
        SubscriberCase {
            case: "twelve characters with a letter among the digits",
            subscribers: vec!["11112222333a"],
            accepted: false,
        },
        SubscriberCase {
            case: "an account ID padded with whitespace",
            subscribers: vec![" 111122223333 "],
            accepted: false,
        },
        SubscriberCase {
            case: "twelve non-ASCII digits",
            subscribers: vec!["１１１１２２２２３３３３"],
            accepted: false,
        },
        SubscriberCase {
            case: "an unusable entry after a usable one",
            subscribers: vec![SUBSCRIBER, "nope"],
            accepted: false,
        },
        SubscriberCase {
            case: "an unusable entry before a usable one",
            subscribers: vec!["nope", SUBSCRIBER],
            accepted: false,
        },
    ]
}

/// The subscriber list of a row, as the JSON a request carries.
fn list(subscribers: &[&'static str]) -> Value {
    json!(subscribers)
}

/// Assert a refusal names the member that was unusable: a `BadRequestException` is "thrown
/// if the request contains a semantic error. The precise meaning depends on the API, and is
/// documented in the error message".
fn assert_names_the_member(case: &str, body: &Value) {
    let message = body["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("Subscribers"),
        "case {case:?}: the refusal message names the Subscribers member, got {body}"
    );
}

/// The `name` of every flow ListFlows reports.
async fn listed_flow_names(emulator: &Emulator) -> Vec<String> {
    let (status, code, body) = emulator.list_flows("").await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "listing flows expected 200, got {status} {code:?}: {body}"
    );
    body["flows"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|flow| flow["name"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// CreateFlow's `Entitlements` writes the same subscriber list a grant does: a row the
/// service refuses creates NO flow at all, and a row it accepts is reported back by
/// DescribeFlow exactly as it was given.
#[tokio::test]
async fn create_flow_judges_every_entitlement_subscriber() {
    let emulator = Emulator::new();

    for (row, SubscriberCase { case, subscribers, accepted }) in
        subscriber_cases().into_iter().enumerate()
    {
        let name = format!("subscriber-create-{row}");
        let (status, code, body) = emulator
            .create_flow(json!({
                "name": name,
                "source": { "name": "src", "protocol": "rtp", "whitelistCidr": "10.0.0.0/16" },
                "entitlements": [{ "name": "e1", "subscribers": list(&subscribers) }],
            }))
            .await;

        if accepted {
            assert_eq!(
                (status, code.as_deref()),
                (201, None),
                "case {case:?}: creating expected 201, got {status} {code:?}: {body}"
            );
            let flow_arn = body["flow"]["flowArn"].as_str().unwrap_or_default().to_string();
            let (status, code, described) = emulator.describe_flow(&flow_arn).await;
            assert_eq!(
                (status, code.as_deref()),
                (200, None),
                "case {case:?}: describing expected 200, got {status} {code:?}: {described}"
            );
            let stored = entitlements(&described["flow"]);
            assert_eq!(
                stored.len(),
                1,
                "case {case:?}: the created flow carries its one entitlement, got {described}"
            );
            assert_eq!(
                stored[0]["subscribers"],
                list(&subscribers),
                "case {case:?}: the stored subscribers, got {described}"
            );
        } else {
            assert_eq!(
                (status, code.as_deref()),
                (400, Some("BadRequestException")),
                "case {case:?}: creating expected 400 BadRequestException, got {status} {code:?}: {body}"
            );
            assert_names_the_member(case, &body);
            assert!(
                !listed_flow_names(&emulator).await.contains(&name),
                "case {case:?}: a refused create stores no flow, got {name:?} in the listing"
            );
        }
    }
}

/// GrantFlowEntitlements judges the same list on an EXISTING flow: a refused grant leaves
/// the flow's entitlements exactly as they were, and an accepted one persists the list the
/// request carried.
#[tokio::test]
async fn grant_flow_entitlements_judges_every_entitlement_subscriber() {
    let emulator = Emulator::new();
    let (status, code, body) = emulator
        .create_flow(json!({
            "name": "subscriber-grant-flow",
            "source": { "name": "src", "protocol": "rtp", "whitelistCidr": "10.0.0.0/16" },
        }))
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "creating the grant fixture expected 201, got {status} {code:?}: {body}"
    );
    let flow_arn = body["flow"]["flowArn"].as_str().unwrap_or_default().to_string();

    let mut stored_names: Vec<String> = Vec::new();
    for (row, SubscriberCase { case, subscribers, accepted }) in
        subscriber_cases().into_iter().enumerate()
    {
        let name = format!("granted-{row}");
        let (status, code, body) = grant(
            &emulator,
            &flow_arn,
            json!({ "entitlements": [{ "name": name, "subscribers": list(&subscribers) }] }),
        )
        .await;

        if accepted {
            assert_eq!(
                (status, code.as_deref()),
                (201, None),
                "case {case:?}: granting expected 201, got {status} {code:?}: {body}"
            );
            assert_eq!(
                entitlements(&body)[0]["subscribers"],
                list(&subscribers),
                "case {case:?}: the granted subscribers, got {body}"
            );
            stored_names.push(name);
        } else {
            assert_eq!(
                (status, code.as_deref()),
                (400, Some("BadRequestException")),
                "case {case:?}: granting expected 400 BadRequestException, got {status} {code:?}: {body}"
            );
            assert_names_the_member(case, &body);
        }

        // Whatever the row, the flow carries exactly the entitlements the ACCEPTED rows
        // granted: a refused grant stores nothing.
        let (status, code, described) = emulator.describe_flow(&flow_arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "case {case:?}: describing expected 200, got {status} {code:?}: {described}"
        );
        let held: Vec<String> = entitlements(&described["flow"])
            .iter()
            .map(|entitlement| entitlement["name"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            held, stored_names,
            "case {case:?}: the entitlements the flow holds, got {described}"
        );
    }
}

/// UpdateFlowEntitlement REPLACES the whole list, so it judges it the same way: a refused
/// update leaves the stored subscribers untouched, and an accepted one replaces them.
#[tokio::test]
async fn update_flow_entitlement_judges_every_entitlement_subscriber() {
    let emulator = Emulator::new();
    let (status, code, body) = emulator
        .create_flow(json!({
            "name": "subscriber-update-flow",
            "source": { "name": "src", "protocol": "rtp", "whitelistCidr": "10.0.0.0/16" },
            "entitlements": [{ "name": "updated", "subscribers": [SUBSCRIBER] }],
        }))
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "creating the update fixture expected 201, got {status} {code:?}: {body}"
    );
    let flow_arn = body["flow"]["flowArn"].as_str().unwrap_or_default().to_string();
    let entitlement_arn = entitlements(&body["flow"])[0]["entitlementArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // What the entitlement holds right now, so each row can assert the stored list either
    // moved to what it asked for or did not move at all.
    let mut held: Value = json!([SUBSCRIBER]);
    for SubscriberCase { case, subscribers, accepted } in subscriber_cases() {
        let (status, code, body) = emulator
            .send(
                "PUT",
                &format!("/v1/flows/{flow_arn}/entitlements/{entitlement_arn}"),
                Some(json!({ "subscribers": list(&subscribers) })),
            )
            .await;

        if accepted {
            assert_eq!(
                (status, code.as_deref()),
                (202, None),
                "case {case:?}: updating expected 202, got {status} {code:?}: {body}"
            );
            held = list(&subscribers);
            assert_eq!(
                body["entitlement"]["subscribers"], held,
                "case {case:?}: the updated subscribers, got {body}"
            );
        } else {
            assert_eq!(
                (status, code.as_deref()),
                (400, Some("BadRequestException")),
                "case {case:?}: updating expected 400 BadRequestException, got {status} {code:?}: {body}"
            );
            assert_names_the_member(case, &body);
        }

        let (status, code, described) = emulator.describe_flow(&flow_arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "case {case:?}: describing expected 200, got {status} {code:?}: {described}"
        );
        assert_eq!(
            entitlements(&described["flow"])[0]["subscribers"], held,
            "case {case:?}: the stored subscribers, got {described}"
        );
    }
}

/// The crate's own encoding of what an account ID is, asserted directly on the same rows:
/// the HTTP cases above go through it, and this pins the predicate itself so a future
/// caller reads one rule.
#[test]
fn account_id_predicate_matches_the_refusals() {
    use service_com_amazonaws_mediaconnect_mediaconnect::state::is_account_id;

    for SubscriberCase { case, subscribers, accepted } in subscriber_cases() {
        let every_entry_is_an_account = subscribers
            .iter()
            .all(|subscriber| is_account_id(subscriber));
        assert_eq!(
            every_entry_is_an_account, accepted,
            "case {case:?}: every entry of {subscribers:?} an account ID expected {accepted}, \
             got {every_entry_is_an_account}"
        );
    }
}
