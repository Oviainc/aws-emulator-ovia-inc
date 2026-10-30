// SPDX-License-Identifier: CC0-1.0

//! The ONE projection of a stored subscription, driven through every operation that
//! reports one — the create and the four readers/transitions.
//!
//! The rule: a stored subscription is reported by projecting the record held in state,
//! so every operation that answers with one answers the same eight members with the same
//! values for the same record — `CreateSubscription`'s output and the
//! `SubscriptionDescription` the other four return carry the identical member set
//! (`smithy_query(query="shape", shape="CreateSubscriptionOutput,SubscriptionDescription",
//! model_path="services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json")`:
//! `domainId`, `subscriptionId`, `arn`, `status`, `createdAt`, `lastUpdatedAt`, with the
//! optional `activatedAt` / `deactivatedAt`), so a create response and a read of the
//! record it created cannot disagree about any of them.
//!
//! `CreateSubscription` used to write that projection out by hand while the other four
//! went through the single-homed one in `crate::state`; both now compute it in one place,
//! and this suite drives the shared path through an operation from each former copy: the
//! create, `GetSubscription`, `ListSubscriptions`, `ActivateSubscription` and
//! `DeactivateSubscription`.
//!
//! The rows differ along what the rule generalises over: how many subscriptions the
//! domain holds, which of them is read back, the sequence of transitions the record has
//! taken (so which status it is in and which stamps it carries), whether its domain owns
//! a web application, and the Region the requests are signed for.
//!
//! The harness (`Emulator`, `string_at`, `web_app_setup`) is the sibling suites': one way
//! of driving this service, not two.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::arn_resolution_during_domain_deletion::web_app_setup;
use crate::domain_subscription_lifecycle::{string_at, Emulator};

/// Every member `CreateSubscriptionOutput` and `SubscriptionDescription` declare — the
/// two shapes the one projection fills, member for member
/// (`smithy_query(query="shape", shape="CreateSubscriptionOutput,SubscriptionDescription")`).
/// `activatedAt` and `deactivatedAt` are the two optional ones, absent until the
/// transition each records has run.
const REQUIRED_MEMBERS: [&str; 6] = [
    "domainId",
    "subscriptionId",
    "arn",
    "status",
    "createdAt",
    "lastUpdatedAt",
];
const OPTIONAL_MEMBERS: [&str; 2] = ["activatedAt", "deactivatedAt"];

/// One transition a row may take, as its URI segment and the status it moves the record
/// to (`smithy_query(query="operation", operation="ActivateSubscription,DeactivateSubscription")`
/// for the URIs, `shape="SubscriptionStatus"` for the values).
const ACTIVATE: (&str, &str) = ("activate", "ACTIVE");
const DEACTIVATE: (&str, &str) = ("deactivate", "INACTIVE");

/// The members of a reported subscription.
fn members(body: &Value) -> BTreeSet<String> {
    body.as_object()
        .unwrap_or_else(|| panic!("a reported subscription is a JSON object, got {body:?}"))
        .keys()
        .cloned()
        .collect()
}

/// Assert `reported` carries exactly the projection's member set: every required member,
/// and no member outside the modelled eight.
fn assert_member_set(case: &str, what: &str, reported: &Value) {
    let present = members(reported);
    for member in REQUIRED_MEMBERS {
        assert!(
            present.contains(member),
            "case {case:?}: {what} must report {member}, got {present:?} in {reported:?}"
        );
    }
    let modelled: BTreeSet<&str> = REQUIRED_MEMBERS
        .into_iter()
        .chain(OPTIONAL_MEMBERS)
        .collect();
    for member in &present {
        assert!(
            modelled.contains(member.as_str()),
            "case {case:?}: {what} must report only the modelled members {modelled:?}, \
             got {member:?} in {reported:?}"
        );
    }
}

/// `GetSubscription`'s view of one record.
async fn get_subscription(
    emulator: &Emulator,
    case: &str,
    domain_id: &str,
    subscription_id: &str,
) -> Value {
    let (status, body) = emulator
        .send(
            "GET",
            &format!("/domains/{domain_id}/subscriptions/{subscription_id}"),
            None,
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetSubscription got {status} with {body:?}"
    );
    body.get("subscription")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: a subscription member in {body:?}"))
}

/// `ListSubscriptions`'s view of one record, picked out of the page by its id.
async fn listed_subscription(
    emulator: &Emulator,
    case: &str,
    domain_id: &str,
    subscription_id: &str,
) -> Value {
    let (status, body) = emulator
        .send("GET", &format!("/domains/{domain_id}/subscriptions"), None)
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListSubscriptions got {status} with {body:?}"
    );
    let page = body
        .get("subscriptions")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: a subscriptions list in {body:?}"));
    page.iter()
        .find(|entry| entry.get("subscriptionId").and_then(Value::as_str) == Some(subscription_id))
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: {subscription_id} on the page {page:?}"))
}

/// Take one transition and answer the record it reported.
async fn transition(
    emulator: &Emulator,
    case: &str,
    domain_id: &str,
    subscription_id: &str,
    segment: &str,
) -> Value {
    let (status, body) = emulator
        .send(
            "POST",
            &format!("/domains/{domain_id}/subscriptions/{subscription_id}/{segment}"),
            Some(&json!({})),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the {segment} transition got {status} with {body:?}"
    );
    body.get("subscription")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: a subscription member in {body:?}"))
}

/// One row of the table: a domain holding `count` subscriptions in `region`, owning a
/// web application when `web_app`, with `sequence` transitions taken on subscription
/// number `index`.
struct Row {
    case: &'static str,
    count: usize,
    index: usize,
    sequence: &'static [(&'static str, &'static str)],
    web_app: bool,
    region: &'static str,
}

#[tokio::test]
async fn every_subscription_responder_reports_one_projection_of_the_record() {
    let rows = [
        Row {
            case: "one fresh subscription",
            count: 1,
            index: 0,
            sequence: &[],
            web_app: false,
            region: "us-east-1",
        },
        Row {
            case: "one subscription, deactivated",
            count: 1,
            index: 0,
            sequence: &[DEACTIVATE],
            web_app: false,
            region: "us-west-2",
        },
        Row {
            case: "three subscriptions, the middle one re-activated",
            count: 3,
            index: 1,
            sequence: &[DEACTIVATE, ACTIVATE],
            web_app: false,
            region: "eu-west-1",
        },
        Row {
            case: "two subscriptions, the last one activated from the start",
            count: 2,
            index: 1,
            sequence: &[ACTIVATE],
            web_app: true,
            region: "eu-central-1",
        },
        Row {
            case: "four subscriptions, the first one alternated four times",
            count: 4,
            index: 0,
            sequence: &[ACTIVATE, DEACTIVATE, ACTIVATE, DEACTIVATE],
            web_app: false,
            region: "ap-south-1",
        },
        Row {
            case: "five subscriptions, the last one untouched under a web-app domain",
            count: 5,
            index: 4,
            sequence: &[],
            web_app: true,
            region: "ca-central-1",
        },
    ];

    for Row {
        case,
        count,
        index,
        sequence,
        web_app,
        region,
    } in rows
    {
        let emulator = Emulator::new(region);
        let mut request = json!({ "name": "clinic-projection" });
        if web_app {
            request["webAppSetupConfiguration"] = web_app_setup();
        }
        let domain_id = string_at(&emulator.create_domain(&request).await, "domainId");

        // Every create's own response, as the create projected the record it stored.
        let mut created = Vec::new();
        for position in 0..count {
            let (status, body) = emulator
                .send(
                    "POST",
                    &format!("/domains/{domain_id}/subscriptions"),
                    Some(&json!({})),
                )
                .await;
            assert_eq!(
                status, 201,
                "case {case:?}: CreateSubscription #{position} got {status} with {body:?}"
            );
            assert_member_set(case, "CreateSubscription", &body);
            created.push(body);
        }

        // The create's projection and each reader's are one computation: for EVERY
        // subscription in the domain, the create response is byte-for-byte the record
        // `GetSubscription` and `ListSubscriptions` report.
        for (position, create_response) in created.iter().enumerate() {
            let subscription_id = string_at(create_response, "subscriptionId");
            let fetched = get_subscription(&emulator, case, &domain_id, &subscription_id).await;
            assert_member_set(case, "GetSubscription", &fetched);
            assert_eq!(
                &fetched, create_response,
                "case {case:?}: GetSubscription of subscription #{position} must report the \
                 record the create projected, expected {create_response:?}, got {fetched:?}"
            );
            let listed = listed_subscription(&emulator, case, &domain_id, &subscription_id).await;
            assert_member_set(case, "ListSubscriptions", &listed);
            assert_eq!(
                &listed, create_response,
                "case {case:?}: the ListSubscriptions entry for subscription #{position} must \
                 report the record the create projected, expected {create_response:?}, got \
                 {listed:?}"
            );
        }

        // And after each transition, every responder still agrees — on the record as the
        // transition left it, with the create's identifying members untouched.
        let followed = &created[index];
        let subscription_id = string_at(followed, "subscriptionId");
        for (step, (segment, target)) in sequence.iter().enumerate() {
            let case = &format!("{case} (step {step}: {segment})");
            let answered = transition(&emulator, case, &domain_id, &subscription_id, segment).await;
            assert_member_set(case, "the transition", &answered);
            assert_eq!(
                answered.get("status").and_then(Value::as_str),
                Some(*target),
                "case {case:?}: the transition must report the status it wrote, got \
                 {answered:?}"
            );
            for member in ["domainId", "subscriptionId", "arn", "createdAt"] {
                assert_eq!(
                    answered.get(member),
                    followed.get(member),
                    "case {case:?}: {member} must stand as the create projected it, expected \
                     {:?}, got {:?}",
                    followed.get(member),
                    answered.get(member)
                );
            }
            let fetched = get_subscription(&emulator, case, &domain_id, &subscription_id).await;
            assert_eq!(
                fetched, answered,
                "case {case:?}: GetSubscription must report the record the transition \
                 projected, expected {answered:?}, got {fetched:?}"
            );
            let listed = listed_subscription(&emulator, case, &domain_id, &subscription_id).await;
            assert_eq!(
                listed, answered,
                "case {case:?}: the ListSubscriptions entry must report the record the \
                 transition projected, expected {answered:?}, got {listed:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_create_response_projects_its_own_record_and_no_neighbours() {
    // Two domains in one store, each holding subscriptions: a create's response is the
    // record IT stored, so no two creates and no reader report one another's.
    let emulator = Emulator::new("us-east-1");
    let mut per_domain = Vec::new();
    for name in ["clinic-alpha", "clinic-beta"] {
        let domain_id = string_at(
            &emulator.create_domain(&json!({ "name": name })).await,
            "domainId",
        );
        let mut created = Vec::new();
        for _ in 0..3 {
            let (status, body) = emulator
                .send(
                    "POST",
                    &format!("/domains/{domain_id}/subscriptions"),
                    Some(&json!({})),
                )
                .await;
            assert_eq!(
                status, 201,
                "domain {name}: CreateSubscription got {status} with {body:?}"
            );
            created.push(body);
        }
        per_domain.push((name, domain_id, created));
    }

    for (name, domain_id, created) in &per_domain {
        for response in created {
            let case = format!("{name}/{}", string_at(response, "subscriptionId"));
            assert_eq!(
                response.get("domainId").and_then(Value::as_str),
                Some(domain_id.as_str()),
                "case {case:?}: the create must report the domain it created under, got \
                 {response:?}"
            );
            let fetched = get_subscription(
                &emulator,
                &case,
                domain_id,
                &string_at(response, "subscriptionId"),
            )
            .await;
            assert_eq!(
                &fetched, response,
                "case {case:?}: the reader must report this subscription's own record, \
                 expected {response:?}, got {fetched:?}"
            );
        }
        // Each create minted a distinct record, so the projections are distinct too.
        let ids: BTreeSet<String> = created
            .iter()
            .map(|response| string_at(response, "subscriptionId"))
            .collect();
        assert_eq!(
            ids.len(),
            created.len(),
            "domain {name}: every create must project its own subscription, got {ids:?}"
        );
    }
}
