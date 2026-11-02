// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the ADDRESS rule on the GROWTH path — `CreateSubscriber`, the
//! one write that adds a single subscriber to a notification a budget already holds —
//! driven over the wire through the same `MultiServiceRouter` the production router
//! serves from.
//!
//! The RULE: a `Subscriber` "consists of a subscription type and either an Amazon SNS
//! topic or an email address" (`Subscriber`), and its `Address` is "the address that
//! Amazon Web Services sends budget notifications to, either an SNS topic or an email"
//! which, "when you create a subscriber, … can't contain line breaks"
//! (`Subscriber$Address`) — so a write takes an address only in the form that
//! subscriber's own `SubscriptionType` names and refuses any other with
//! `InvalidParameterException` (HTTP 400), storing nothing.
//!
//! `subscriber_address_validation.rs` asserts that refusal across every write that
//! establishes a subscriber; this suite asserts what the rule has to leave behind on the
//! path that grows a STORED set one subscriber at a time, which the whole-set writes
//! cannot show:
//!
//! 1. A refused `CreateSubscriber` leaves the notification exactly as writable as it was:
//!    the stored set is untouched, and the per-type capacity the refused subscriber would
//!    have taken ("one SNS subscriber and up to 10 email subscribers") is still free, so
//!    a well-formed subscriber of that same type is still accepted afterwards. Each
//!    notification is seeded to leave exactly ONE slot of the refused subscriber's own
//!    type free, so that acceptance fails if the refusal took the slot, and the create
//!    AFTER it — the one past the bound — is refused as the limit it is.
//! 2. Every address form each subscription type names is accepted on that path and read
//!    back as supplied, appended after the subscriber the notification already held — so
//!    the rule refuses malformed addresses without narrowing the forms a type admits.
//!    The persistence MECHANICS of that path (ordering, the notification a subscriber
//!    lands on, the duplicate and the per-type limits) are
//!    `subscriber_creation.rs`'s own subject, over one address form per type; what this
//!    test adds is the set of forms, so neither suite needs to re-derive the other.
//!
//! Both tables are the address-validation suite's own, so the two suites cannot drift
//! about which addresses are malformed and which are not.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_budgets_awsbudgetservicegateway::state::{
    MAX_EMAIL_SUBSCRIBERS_PER_NOTIFICATION, MAX_SNS_SUBSCRIBERS_PER_NOTIFICATION,
};

use super::budget_action_execution::seed_budget;
use super::budget_lifecycle::{caller_account, call, is_error, router_with_engine, same};
use super::subscriber_address_validation::{
    assert_catalog_spanned, assert_refused, malformed_rows, notification, reported_subscribers,
    seed_subscriber, stored_subscribers, subscriber, well_formed_rows,
};
use super::subscriber_creation::{create_subscriber, pair};

/// Create `budget_name` holding ONE notification, carrying `subscribers` — the set each
/// table below grows onto (`Subscribers` is `@length(min: 1)`, so a notification cannot be
/// created without one).
async fn seed_notified(
    router: &MultiServiceRouter,
    account_id: &str,
    budget_name: &str,
    alerts_on: &Value,
    subscribers: &[Value],
) {
    seed_budget(router, account_id, budget_name).await;
    let (status, body) = call(
        router,
        "CreateNotification",
        &json!({
            "AccountId": account_id,
            "BudgetName": budget_name,
            "Notification": alerts_on,
            "Subscribers": subscribers
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup {budget_name:?}: expected CreateNotification to answer 200, got {status} {body}"
    );
}

/// A well-formed subscriber of `subscription_type` — the address form THAT type names, so
/// a test can follow a refused subscriber with an accepted one of the same type and show
/// the refusal took none of that type's capacity. `label` keeps each one distinct from
/// every other subscriber the suite establishes.
///
/// The arms are the `SubscriptionType` catalog [`assert_catalog_spanned`] pins, so a
/// catalog that grew a third type fails here instead of going untested.
fn well_formed_of(subscription_type: &str, label: &str) -> Value {
    match subscription_type {
        "EMAIL" => subscriber("EMAIL", &format!("{label}@example.com")),
        "SNS" => subscriber(
            "SNS",
            &format!("arn:aws:sns:us-east-1:{}:{label}", caller_account()),
        ),
        other => panic!("no well-formed address form is known for subscription type {other:?}"),
    }
}

/// The set a refusal row's notification is seeded with: enough subscribers to leave
/// exactly ONE slot of `subscription_type` free, so the create that follows the refusal
/// succeeds only if the refused subscriber took no capacity.
///
/// The seats come from the per-type limit the crate publishes, so a limit that moves
/// moves this seeding with it rather than leaving the capacity claim resting on the
/// value the constant happens to hold.
fn seeded_set(subscription_type: &str) -> Vec<Value> {
    let limit = match subscription_type {
        "EMAIL" => MAX_EMAIL_SUBSCRIBERS_PER_NOTIFICATION,
        "SNS" => MAX_SNS_SUBSCRIBERS_PER_NOTIFICATION,
        other => panic!("no per-type subscriber limit is known for subscription type {other:?}"),
    };
    let mut set: Vec<Value> = (1..limit)
        .map(|seat| well_formed_of(subscription_type, &format!("seat-{seat}")))
        .collect();
    if set.is_empty() {
        // `Subscribers` is `@length(min: 1)`, so a type whose half admits a single slot
        // is seeded with a subscriber of the OTHER type — which leaves that slot free.
        set.push(seed_subscriber());
    }
    set
}

/// The subscribers of one notification as the store holds them, for a budget holding a
/// single notification.
fn held_subscribers(
    engine: &std::sync::Arc<engine::InMemoryEngine>,
    account_id: &str,
    budget_name: &str,
) -> Vec<(String, String)> {
    stored_subscribers(engine, account_id, budget_name)
        .into_iter()
        .next()
        .unwrap_or_default()
}

/// RULE 1: a `CreateSubscriber` whose address is not what its `SubscriptionType` names is
/// refused, stores nothing, and consumes none of that type's subscriber capacity — the
/// notification is left exactly as writable as it was.
#[tokio::test]
async fn a_refused_growth_leaves_the_notification_as_writable_as_it_was() {
    assert_catalog_spanned();
    let (router, engine) = router_with_engine();
    let account_id = caller_account();

    for (index, row) in malformed_rows().into_iter().enumerate() {
        let case = row.case;
        let address = row.subscriber["Address"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let subscription_type = row.subscriber["SubscriptionType"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let budget_name = format!("growth-refused-{index}");
        let alerts_on = notification(index);
        let seeded_subscribers = seeded_set(&subscription_type);
        seed_notified(
            &router,
            &account_id,
            &budget_name,
            &alerts_on,
            &seeded_subscribers,
        )
        .await;
        let seeded: Vec<(String, String)> = seeded_subscribers.iter().map(pair).collect();

        let (status, body) = create_subscriber(
            &router,
            &account_id,
            &budget_name,
            &alerts_on,
            &row.subscriber,
        )
        .await;
        assert_refused(case, &address, status, &body);
        let held = held_subscribers(&engine, &account_id, &budget_name);
        assert_eq!(
            held, seeded,
            "case {case:?}: expected the refused growth to leave the store at {seeded:?}, \
             got {held:?}"
        );

        // The ONE slot of its own type the refused subscriber would have taken is still
        // free: a well-formed subscriber of that type is accepted next, and lands after
        // the subscribers the notification already held.
        let accepted = well_formed_of(&subscription_type, &format!("accepted-{index}"));
        let (status, body) =
            create_subscriber(&router, &account_id, &budget_name, &alerts_on, &accepted).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a well-formed {subscription_type} subscriber to be \
             accepted after the refusal, got {status} {body}"
        );
        let held = held_subscribers(&engine, &account_id, &budget_name);
        let expected: Vec<(String, String)> = seeded
            .iter()
            .cloned()
            .chain(::std::iter::once(pair(&accepted)))
            .collect();
        assert_eq!(
            held, expected,
            "case {case:?}: expected the store to hold {expected:?}, got {held:?}"
        );

        // That slot was the LAST of its type, so the next one is past the bound — the
        // limit refusal, not the address refusal, which is what makes the acceptance
        // above a statement about capacity at all.
        let past_the_bound = well_formed_of(&subscription_type, &format!("past-bound-{index}"));
        let (status, body) = create_subscriber(
            &router,
            &account_id,
            &budget_name,
            &alerts_on,
            &past_the_bound,
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected one {subscription_type} subscriber past the bound to be \
             refused, got {status} {body}"
        );
        assert!(
            is_error(&body, "CreationLimitExceededException"),
            "case {case:?}: expected one {subscription_type} subscriber past the bound to be \
             refused as the limit, got {status} {body}"
        );
        let held = held_subscribers(&engine, &account_id, &budget_name);
        assert_eq!(
            held, expected,
            "case {case:?}: expected the limit refusal to leave the store at {expected:?}, \
             got {held:?}"
        );
    }
}

/// RULE 2: an address that IS what its subscription type names is accepted on the growth
/// path and read back as supplied, appended after the subscriber the notification already
/// held — over every address form the two types admit.
#[tokio::test]
async fn every_address_form_its_type_names_is_accepted_on_the_growth_path() {
    assert_catalog_spanned();
    let (router, engine) = router_with_engine();
    let account_id = caller_account();

    for (index, (case, row)) in well_formed_rows().into_iter().enumerate() {
        let budget_name = format!("growth-accepted-{index}");
        let alerts_on = notification(index);
        let seeded_subscribers = vec![seed_subscriber()];
        seed_notified(
            &router,
            &account_id,
            &budget_name,
            &alerts_on,
            &seeded_subscribers,
        )
        .await;

        let (status, body) =
            create_subscriber(&router, &account_id, &budget_name, &alerts_on, &row).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateSubscriber({row}) to answer 200, got {status} {body}"
        );

        let reported =
            reported_subscribers(&router, &account_id, &budget_name, &alerts_on).await;
        let expected = json!([seed_subscriber(), &row]);
        assert!(
            same(&json!(reported), &expected),
            "case {case:?}: expected the read to report {expected}, got {reported:?}"
        );
        let held = held_subscribers(&engine, &account_id, &budget_name);
        let expected = vec![pair(&seed_subscriber()), pair(&row)];
        assert_eq!(
            held, expected,
            "case {case:?}: expected the store to hold {expected:?}, got {held:?}"
        );
    }
}
