// SPDX-License-Identifier: CC0-1.0

//! Handing this service a role to assume: whose role it may be, and when that is decided.
//!
//! A `roleArn` on an event destination names a role the SERVICE assumes to write events,
//! so the request is a pass-role and a principal can pass only a role of its OWN account.
//! The rule the rows below assert:
//!
//! 1. a role whose account is not the calling account is refused `AccessDeniedException`
//!    (403) with the service's fixed text, whatever the rest of the request says;
//! 2. the decision is made from the REQUEST alone, so it comes BEFORE the resource the
//!    request names is resolved — an id naming no linked account does not change the
//!    refusal — and before any write, so a refused call leaves the store untouched;
//! 3. a role of the calling account, and a destination carrying no role at all, are
//!    passed, and what was written is what the read returns.
//!
//! Every row drives one emulator through the real router dispatch (the same path a
//! socket-served request takes), on both operations that accept a destination:
//! `PutWhatsAppBusinessAccountEventDestinations` and the `waba` block of
//! `AssociateWhatsAppBusinessAccount`.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    call, console_call, emulator, link, query_escape, start_signup, Answer, ACCOUNT_ID,
};

/// The text the service refuses a cross-account pass-role with
/// (`CROSS_ACCOUNT_PASS_ROLE_MESSAGE` in `src/state.rs`): wholly fixed, so nothing of the
/// request — not the role, not the destination, not the id — reaches it.
const CROSS_ACCOUNT: &str = "Cross-account pass role is not allowed.";

/// A well-formed `LinkedWhatsAppBusinessAccountId` that names no linked account, so a row
/// using it would be a not-found if the pass-role decision were made after the lookup.
const UNLINKED_WABA: &str = "waba-00000000000000000000000000000000";

/// 12-digit account ids that are NOT the calling account (`ACCOUNT_ID`). An account id is
/// exactly 12 digits (the model's `RoleArn` pattern
/// `^arn:.*:iam::\d{12}:role\/[a-zA-Z0-9+=,.@\-_]+$`), so these span the class: the
/// documentation range, one a single digit away from the caller's, and both numeric
/// extremes a 12-digit id can take.
const OTHER_ACCOUNTS: &[&str] = &[
    "111122223333",
    "000000000001",
    "999999999999",
    "100000000000",
];

/// Role names drawn from every character class the model's `RoleArn` pattern admits
/// (`[a-zA-Z0-9+=,.@\-_]+`), plus the shortest name it allows, so the decision is shown
/// not to depend on how the role is NAMED.
const ROLE_NAMES: &[&str] = &["event-writer", "Events_2", "a+b=c,d.e@f-g_h", "A"];

/// Destination ARNs in the CALLING account, one per service an event destination plausibly
/// names, in both resource forms (`<type>/<id>` and `<type>:<id>`) the model's
/// `EventDestinationArn` pattern `^arn:.*:[a-z-]+([/:](.*))?$` admits.
fn destination_arns() -> Vec<String> {
    vec![
        format!("arn:aws:sns:us-east-1:{ACCOUNT_ID}:social-messaging-events"),
        format!("arn:aws:sqs:us-east-1:{ACCOUNT_ID}:events-queue"),
        format!("arn:aws:firehose:us-east-1:{ACCOUNT_ID}:deliverystream/events"),
        format!("arn:aws:kinesis:us-west-2:{ACCOUNT_ID}:stream/events"),
    ]
}

fn role(account: &str, name: &str) -> String {
    format!("arn:aws:iam::{account}:role/{name}")
}

async fn put_destinations(router: &MultiServiceRouter, id: &str, destinations: Value) -> Answer {
    call(
        router,
        "PUT",
        "/v1/whatsapp/waba/eventdestinations",
        Some(json!({ "id": id, "eventDestinations": destinations })),
    )
    .await
}

async fn read_destinations(router: &MultiServiceRouter, id: &str) -> Value {
    let uri = format!("/v1/whatsapp/waba/details?id={}", query_escape(id));
    call(router, "GET", &uri, None)
        .await
        .at("/account/eventDestinations")
        .clone()
}

/// Finalize `token` into a new linked account whose `waba` block carries `destinations`.
async fn finalize_with_destinations(
    router: &MultiServiceRouter,
    token: &str,
    destinations: Value,
) -> Answer {
    console_call(
        router,
        "POST",
        "/v1/whatsapp/signup",
        Some(json!({
            "setupFinalization": {
                "associateInProgressToken": token,
                "phoneNumbers": [{ "id": "phone-number-id-aaaa", "twoFactorPin": "123456" }],
                "waba": {
                    "id": "waba-11111111111111111111111111111111",
                    "eventDestinations": destinations,
                },
            }
        })),
    )
    .await
}

/// Assert one answer is the fixed 403 refusal.
fn expect_refusal(case: &str, answer: &Answer) {
    assert_eq!(
        (answer.status, answer.error_type.as_str()),
        (403, "AccessDeniedException"),
        "case {case:?}: expected 403 AccessDeniedException, got {} {:?} {:?}",
        answer.status,
        answer.error_type,
        answer.body
    );
    let actual = answer.message();
    assert_eq!(
        actual, CROSS_ACCOUNT,
        "case {case:?}: expected the message {CROSS_ACCOUNT:?}, got {actual:?}"
    );
}

/// RULES 1 and 2: every role of another account is refused with the fixed 403, and the
/// refusal does not depend on whether the `id` names a linked account — so it is reached
/// before the lookup — nor does it change what a later read sees.
#[tokio::test]
async fn a_role_of_another_account_is_refused_whatever_the_rest_of_the_request_names() {
    let router = emulator();
    // One account exists and already carries a destination of its own, so a refusal is
    // shown to leave a WRITTEN value alone rather than an absent one.
    let linked = link(
        &router,
        "waba-55555555555555555555555555555555",
        &["phone-number-id-bbbb"],
    )
    .await;
    let kept = json!([{
        "eventDestinationArn": destination_arns()[0],
        "roleArn": role(ACCOUNT_ID, "already-written"),
    }]);
    let seeded = put_destinations(&router, &linked, kept.clone()).await;
    assert_eq!(
        seeded.status, 200,
        "setup: expected 200 from the seeding put, got {} {:?}",
        seeded.status, seeded.body
    );

    for (index, account) in OTHER_ACCOUNTS.iter().enumerate() {
        // Every account against every role name, so the table is the cross product of the
        // two axes rather than a diagonal through them; the destination ARN rotates so
        // each row also names a different destination.
        for (offset, name) in ROLE_NAMES.iter().enumerate() {
            let destinations = destination_arns();
            let destination = &destinations[(index + offset) % destinations.len()];
            let body = json!([{
                "eventDestinationArn": destination,
                "roleArn": role(account, name),
            }]);

            // On the id of a LINKED account: the resource resolves, and the pass-role
            // refusal is still what answers.
            let on_linked = put_destinations(&router, &linked, body.clone()).await;
            expect_refusal(
                &format!("role {name:?} in {account} on a linked waba"),
                &on_linked,
            );

            // On an id that names NO account: were the lookup first, this would be the
            // operation's `InvalidParametersException` instead.
            let on_unlinked = put_destinations(&router, UNLINKED_WABA, body.clone()).await;
            expect_refusal(
                &format!("role {name:?} in {account} on an unlinked waba"),
                &on_unlinked,
            );

            // And nothing was written: the account still reports what it carried.
            let after = read_destinations(&router, &linked).await;
            assert_eq!(
                after, kept,
                "case role {name:?} in {account:?}: a refused put must leave the \
                 destinations alone, got {after:?}"
            );
        }
    }
}

/// RULE 3: a role of the CALLING account is passed, and so is a destination that carries
/// no role at all; what was written is what the read returns.
#[tokio::test]
async fn a_role_of_the_calling_account_is_passed_and_read_back() {
    let router = emulator();
    let linked = link(
        &router,
        "waba-66666666666666666666666666666666",
        &["phone-number-id-cccc"],
    )
    .await;

    let mut cases: Vec<(String, Value)> = Vec::new();
    for (index, destination) in destination_arns().into_iter().enumerate() {
        let name = ROLE_NAMES[index % ROLE_NAMES.len()];
        cases.push((
            format!("role {name:?} on {destination}"),
            json!([{ "eventDestinationArn": destination, "roleArn": role(ACCOUNT_ID, name) }]),
        ));
    }
    // The member is optional, so a destination with no role passes no role at all and
    // there is nothing to authorize.
    cases.push((
        "a destination with no roleArn".to_string(),
        json!([{ "eventDestinationArn": destination_arns()[0] }]),
    ));
    // An empty list clears the destinations, and passes no role either.
    cases.push(("an empty destination list".to_string(), json!([])));

    for (case, destinations) in cases {
        let answer = put_destinations(&router, &linked, destinations.clone()).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {} {:?}",
            answer.status, answer.body
        );
        let after = read_destinations(&router, &linked).await;
        assert_eq!(
            after, destinations,
            "case {case:?}: expected the read to return {destinations:?}, got {after:?}"
        );
    }
}

/// The INPUT constraint is still decided first: a list past the modelled maximum of one is
/// the framework's `ValidationException` even when an entry also passes a foreign role,
/// because a request that does not satisfy its own shape is never admitted to the handler.
#[tokio::test]
async fn a_list_past_its_modelled_maximum_is_still_the_validation_refusal() {
    let router = emulator();
    let destinations = destination_arns();
    let answer = put_destinations(
        &router,
        UNLINKED_WABA,
        json!([
            {
                "eventDestinationArn": destinations[0],
                "roleArn": role(OTHER_ACCOUNTS[0], ROLE_NAMES[0]),
            },
            { "eventDestinationArn": destinations[1] },
        ]),
    )
    .await;
    assert_eq!(
        (answer.status, answer.error_type.as_str()),
        (400, "ValidationException"),
        "two destinations: expected 400 ValidationException, got {} {:?} {:?}",
        answer.status,
        answer.error_type,
        answer.body
    );
    let actual = answer.message();
    assert!(
        actual.ends_with(
            "at 'eventDestinations' failed to satisfy constraint: Member must have length \
             less than or equal to 1"
        ),
        "two destinations: expected the max-1 clause, got {actual:?}"
    );
}

/// The same rule on the other operation that accepts a destination: a `waba` block passing
/// a foreign role is refused with the same fixed 403, and nothing is linked and no signup
/// consumed — the same token still finalizes once the role is one the caller may pass.
#[tokio::test]
async fn the_association_refuses_a_foreign_role_in_its_waba_block() {
    let router = emulator();
    let token = start_signup(&router).await;
    let destinations = destination_arns();

    for (index, account) in OTHER_ACCOUNTS.iter().enumerate() {
        let name = ROLE_NAMES[index % ROLE_NAMES.len()];
        let answer = finalize_with_destinations(
            &router,
            &token,
            json!([{
                "eventDestinationArn": destinations[index % destinations.len()],
                "roleArn": role(account, name),
            }]),
        )
        .await;
        expect_refusal(&format!("waba block passing a role in {account}"), &answer);
        let listed = call(&router, "GET", "/v1/whatsapp/waba/list", None).await;
        assert_eq!(
            listed.at("/linkedAccounts"),
            &json!([]),
            "case {account:?}: a refused finalization must link no account, got {:?}",
            listed.at("/linkedAccounts")
        );
    }

    // The signup was never consumed, so the corrected request still finalizes.
    let own = json!([{
        "eventDestinationArn": destinations[0],
        "roleArn": role(ACCOUNT_ID, ROLE_NAMES[0]),
    }]);
    let accepted = finalize_with_destinations(&router, &token, own.clone()).await;
    assert_eq!(
        accepted.status, 200,
        "the corrected finalization: expected 200, got {} {:?}",
        accepted.status, accepted.body
    );
    let id = accepted.string_at("/linkedWhatsAppBusinessAccountId");
    let after = read_destinations(&router, &id).await;
    assert_eq!(
        after, own,
        "the corrected finalization: expected the read to return {own:?}, got {after:?}"
    );
}
