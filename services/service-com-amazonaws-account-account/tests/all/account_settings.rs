// SPDX-License-Identifier: CC0-1.0

//! The AWS Account settings slice — account name and alternate contacts — end to
//! end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted store and serialization: the same
//! path a client takes. The rule under test is the one Account Management states
//! for these resources: a put stores a record against the resolved account (the
//! `AccountId` the request names, else the caller's own), a get reports exactly
//! what was stored, a contact type is keyed so a second put of that type
//! overwrites it, a delete removes it, and a type with nothing stored is a
//! `ResourceNotFoundException` rather than an empty success.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_account_account::state::{default_account_id, MAX_ACCOUNTS};
use service_com_amazonaws_account_account::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The values `AlternateContactType` admits, from
/// `smithy_query(query="shape", shape="AlternateContactType")` — `BILLING`,
/// `OPERATIONS`, `SECURITY`. Spelled out here, not imported, so a wrong set in
/// the implementation cannot make this test pass.
const ALTERNATE_CONTACT_TYPES: [&str; 3] = ["BILLING", "OPERATIONS", "SECURITY"];

/// The `AccountState` a live account reports, from
/// `smithy_query(query="shape", shape="AccountState")`
/// (`PENDING_ACTIVATION | ACTIVE | SUSPENDED | CLOSED`).
const ACTIVE: &str = "ACTIVE";

/// `AccountName`'s modelled `@length(min: 1, max: 50)`, from
/// `smithy_query(query="operation", operation="PutAccountName")`.
const ACCOUNT_NAME_MIN: usize = 1;
const ACCOUNT_NAME_MAX: usize = 50;

/// A SigV4 credential scope naming this service: it is what routes a request to
/// the `account` service (its sigv4 signing name).
const SIGNED_FOR_US_EAST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/account/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// `AccountId`'s `^\d{12}$`, checked here rather than through the crate's own
/// code so a wrong account id cannot pass unnoticed.
fn is_account_id(value: &str) -> bool {
    value.len() == 12 && value.bytes().all(|b| b.is_ascii_digit())
}

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    /// The `AlternateContact` structure `GetAlternateContact` returns.
    fn contact(&self) -> &Value {
        &self.body["AlternateContact"]
    }

    /// One member of this answer's body, as a string (empty when absent).
    fn member(&self, name: &str) -> &str {
        self.body[name].as_str().unwrap_or_default()
    }

    /// The error `message`, or the empty string when there is none.
    fn message(&self) -> &str {
        self.body["message"].as_str().unwrap_or_default()
    }
}

async fn call(router: &MultiServiceRouter, uri: &str, body: Value) -> Answer {
    let payload = body.to_string();
    let request = ::http::Request::builder()
        .method("POST")
        .uri(uri)
        .header("host", "account.us-east-1.amazonaws.com")
        .header("authorization", SIGNED_FOR_US_EAST_1)
        .header("content-type", "application/json")
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "POST {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

async fn put_account_name(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "/putAccountName", request).await
}

async fn get_account_information(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "/getAccountInformation", request).await
}

async fn put_alternate_contact(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "/putAlternateContact", request).await
}

async fn get_alternate_contact(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "/getAlternateContact", request).await
}

async fn delete_alternate_contact(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "/deleteAlternateContact", request).await
}

/// A contact request body for `contact_type`, with members derived from `tag` so
/// every case in a table carries its own distinguishable values.
fn contact_request(contact_type: &str, tag: &str) -> Value {
    json!({
        "AlternateContactType": contact_type,
        "Name": format!("{tag} Name"),
        "Title": format!("{tag} Title"),
        "EmailAddress": format!("{}@example.com", tag.to_ascii_lowercase().replace(' ', ".")),
        "PhoneNumber": "+1 206-555-0100",
    })
}

/// Assert `answer` is the contact `request` asked to store, member for member.
fn assert_contact_matches(case: &str, request: &Value, answer: &Answer) {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: GetAlternateContact must answer 200, got {} with {:?}",
        answer.status, answer.body
    );
    for member in ["Name", "Title", "EmailAddress", "PhoneNumber", "AlternateContactType"] {
        assert_eq!(
            answer.contact()[member],
            request[member],
            "case {case:?}: {member} must be read back as stored, got {:?}",
            answer.contact()[member]
        );
    }
}

/// Every alternate contact type is its own record: a put of one type is readable
/// back member for member, and leaves the other types exactly as they were.
#[tokio::test]
async fn each_contact_type_round_trips_independently() {
    let router = emulator();

    // Nothing is stored yet, so every type is a 404 rather than an empty success.
    for contact_type in ALTERNATE_CONTACT_TYPES {
        let answer =
            get_alternate_contact(&router, json!({"AlternateContactType": contact_type})).await;
        assert_eq!(
            answer.status, 404,
            "case {contact_type:?}: an unset contact must answer 404, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ResourceNotFoundException",
            "case {contact_type:?}: expected ResourceNotFoundException, got {:?}",
            answer.body
        );
    }

    // Store one contact per type, each with its own values.
    let mut stored: Vec<(&str, Value)> = Vec::new();
    for contact_type in ALTERNATE_CONTACT_TYPES {
        let request = contact_request(contact_type, contact_type);
        let put = put_alternate_contact(&router, request.clone()).await;
        assert_eq!(
            put.status, 200,
            "case {contact_type:?}: PutAlternateContact must answer 200, got {} with {:?}",
            put.status, put.body
        );
        stored.push((contact_type, request));
    }

    // Each reads back as itself — no type's write landed on another's record.
    for (contact_type, request) in &stored {
        let answer =
            get_alternate_contact(&router, json!({"AlternateContactType": contact_type})).await;
        assert_contact_matches(contact_type, request, &answer);
    }
}

/// `PutAlternateContact` is `@idempotent`: a second put of the same type
/// overwrites the one stored record instead of adding a second one, and touches
/// no other type.
#[tokio::test]
async fn a_second_put_of_one_type_overwrites_that_record_only() {
    let router = emulator();

    for contact_type in ALTERNATE_CONTACT_TYPES {
        put_alternate_contact(&router, contact_request(contact_type, "First")).await;
    }

    // Rewrite each type in turn, with values that differ in every member.
    for (index, contact_type) in ALTERNATE_CONTACT_TYPES.into_iter().enumerate() {
        let rewrite = contact_request(contact_type, &format!("Rewrite {index}"));
        let put = put_alternate_contact(&router, rewrite.clone()).await;
        assert_eq!(
            put.status, 200,
            "case {contact_type:?}: a repeated put must answer 200, got {} with {:?}",
            put.status, put.body
        );

        let answer =
            get_alternate_contact(&router, json!({"AlternateContactType": contact_type})).await;
        assert_contact_matches(contact_type, &rewrite, &answer);

        // A delete of that type now empties it: proof the account holds ONE
        // record of this type, not the original plus the rewrite.
        let first_delete =
            delete_alternate_contact(&router, json!({"AlternateContactType": contact_type})).await;
        assert_eq!(
            first_delete.status, 200,
            "case {contact_type:?}: deleting the stored contact must answer 200, got {} with {:?}",
            first_delete.status, first_delete.body
        );
        let after = get_alternate_contact(&router, json!({"AlternateContactType": contact_type}))
            .await;
        assert_eq!(
            after.status, 404,
            "case {contact_type:?}: one delete must leave no record of the type, got {} with {:?}",
            after.status, after.body
        );

        // Every type after this one still carries its first write.
        for untouched in ALTERNATE_CONTACT_TYPES.into_iter().skip(index + 1) {
            let answer =
                get_alternate_contact(&router, json!({"AlternateContactType": untouched})).await;
            assert_contact_matches(untouched, &contact_request(untouched, "First"), &answer);
        }
    }
}

/// `DeleteAlternateContact` removes the record the type names; deleting a type
/// that is not set is a `ResourceNotFoundException`, both for a type never
/// written and for a second delete of one already removed.
#[tokio::test]
async fn deleting_an_unset_contact_type_is_not_found() {
    let router = emulator();

    // Only BILLING is stored, so the other two types are unset.
    let billing = contact_request("BILLING", "Ada");
    put_alternate_contact(&router, billing.clone()).await;

    for contact_type in ALTERNATE_CONTACT_TYPES.into_iter().filter(|t| *t != "BILLING") {
        let answer =
            delete_alternate_contact(&router, json!({"AlternateContactType": contact_type})).await;
        assert_eq!(
            answer.status, 404,
            "case {contact_type:?}: deleting an unset type must answer 404, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ResourceNotFoundException",
            "case {contact_type:?}: expected ResourceNotFoundException, got {:?}",
            answer.body
        );
    }

    // A refused delete of another type left BILLING alone.
    assert_contact_matches(
        "BILLING",
        &billing,
        &get_alternate_contact(&router, json!({"AlternateContactType": "BILLING"})).await,
    );

    // Deleting BILLING twice: the first removes the record, the second finds
    // nothing to remove.
    for (attempt, expected_status) in [(1, 200), (2, 404)] {
        let answer =
            delete_alternate_contact(&router, json!({"AlternateContactType": "BILLING"})).await;
        assert_eq!(
            answer.status, expected_status,
            "case delete attempt {attempt}: expected HTTP {expected_status}, got {} with {:?}",
            answer.status, answer.body
        );
        if expected_status == 404 {
            assert_eq!(
                answer.error_code(),
                "ResourceNotFoundException",
                "case delete attempt {attempt}: expected ResourceNotFoundException, got {:?}",
                answer.body
            );
        }
    }
}

/// The one sentence Account Management states for an alternate contact that is
/// not set. Spelled out here rather than imported from the crate so a
/// per-request message cannot make this test pass.
const ALTERNATE_CONTACT_NOT_FOUND: &str = "No contact of the inputted alternate contact type found.";

/// A member account id (`^\d{12}$`) that is not the caller's own account: a
/// not-found message must be the same literal whichever account resolves.
const MEMBER_ACCOUNT: &str = "123456789012";

/// The not-found message is ONE CONSTANT: invariant of the
/// `AlternateContactType` asked for, of the account the call resolved to, and of
/// whether `GetAlternateContact` or `DeleteAlternateContact` asked — and whether
/// the type was never set or was just deleted. It names neither the contact type
/// nor any account id.
#[tokio::test]
async fn an_unset_alternate_contact_states_one_constant_message() {
    let router = emulator();

    // The class: every contact type, both operations, both ways of being unset,
    // and both ways of resolving the account.
    for account in [None, Some(MEMBER_ACCOUNT)] {
        let request = |contact_type: &str| match account {
            Some(id) => json!({"AlternateContactType": contact_type, "AccountId": id}),
            None => json!({"AlternateContactType": contact_type}),
        };
        for contact_type in ALTERNATE_CONTACT_TYPES {
            // (how the type is unset, which operation asks)
            let mut answers: Vec<(String, Answer)> = Vec::new();
            answers.push((
                format!("{contact_type} never set, read by Get, account {account:?}"),
                get_alternate_contact(&router, request(contact_type)).await,
            ));
            answers.push((
                format!("{contact_type} never set, removed by Delete, account {account:?}"),
                delete_alternate_contact(&router, request(contact_type)).await,
            ));

            // Now set it and delete it, so the SAME type is unset by a delete.
            let mut stored = contact_request(contact_type, "Ada");
            if let Some(id) = account {
                stored["AccountId"] = json!(id);
            }
            let put = put_alternate_contact(&router, stored).await;
            assert_eq!(
                put.status, 200,
                "case {contact_type:?}: the put must answer 200, got {} with {:?}",
                put.status, put.body
            );
            let deleted = delete_alternate_contact(&router, request(contact_type)).await;
            assert_eq!(
                deleted.status, 200,
                "case {contact_type:?}: the delete must answer 200, got {} with {:?}",
                deleted.status, deleted.body
            );
            answers.push((
                format!("{contact_type} after a delete, read by Get, account {account:?}"),
                get_alternate_contact(&router, request(contact_type)).await,
            ));
            answers.push((
                format!("{contact_type} after a delete, removed again, account {account:?}"),
                delete_alternate_contact(&router, request(contact_type)).await,
            ));

            for (case, answer) in answers {
                assert_eq!(
                    answer.status, 404,
                    "case {case:?}: expected HTTP 404, got {} with {:?}",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.error_code(),
                    "ResourceNotFoundException",
                    "case {case:?}: expected ResourceNotFoundException, got {:?}",
                    answer.body
                );
                assert_eq!(
                    answer.message(),
                    ALTERNATE_CONTACT_NOT_FOUND,
                    "case {case:?}: expected the constant not-found message, got {:?}",
                    answer.message()
                );
                assert!(
                    !answer.message().contains(contact_type),
                    "case {case:?}: the message must not name the contact type, got {:?}",
                    answer.message()
                );
                for id in [default_account_id(), MEMBER_ACCOUNT] {
                    assert!(
                        !answer.message().contains(id),
                        "case {case:?}: the message must not name an account id, got {:?}",
                        answer.message()
                    );
                }
            }
        }
    }
}

/// `GetAccountInformation` reports the name `PutAccountName` last stored, for
/// every name the model admits; the created date is set once and never moves,
/// and the account is ACTIVE with a 12-digit id.
#[tokio::test]
async fn account_name_round_trips_and_the_created_date_is_stable() {
    // Names spanning what `AccountName`'s length bound and pattern admit: both
    // published bounds, and values differing in length and in the punctuation the
    // pattern `^[ -;=?-~]+$` allows.
    let names = [
        ("the shortest name the bound admits", "A".repeat(ACCOUNT_NAME_MIN)),
        ("the longest name the bound admits", "N".repeat(ACCOUNT_NAME_MAX)),
        ("a name with spaces", "Demo Account".to_string()),
        ("a name with admitted punctuation", "Acct #1 (prod) ?+-:;@~".to_string()),
        ("a name of digits", "1234567890".to_string()),
    ];

    let router = emulator();

    let first = get_account_information(&router, json!({})).await;
    assert_eq!(
        first.status, 200,
        "the caller's own account must be reportable before any write, got {} with {:?}",
        first.status, first.body
    );
    let own_id = first.member("AccountId").to_string();
    assert!(
        is_account_id(&own_id),
        "AccountId must satisfy ^\\d{{12}}$, got {own_id:?}"
    );
    assert_eq!(
        own_id,
        default_account_id(),
        "an absent AccountId must resolve to the caller's own account"
    );
    assert_eq!(
        first.member("AccountState"),
        ACTIVE,
        "a known account must report {ACTIVE}, got {:?}",
        first.body["AccountState"]
    );
    let created = first.body["AccountCreatedDate"].clone();
    assert!(
        !created.is_null(),
        "AccountCreatedDate must be reported, got {:?}",
        first.body
    );

    for (case, name) in &names {
        let put = put_account_name(&router, json!({"AccountName": name})).await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: PutAccountName must answer 200 for {name:?}, got {} with {:?}",
            put.status, put.body
        );

        let answer = get_account_information(&router, json!({})).await;
        assert_eq!(
            answer.member("AccountName"),
            name.as_str(),
            "case {case:?}: AccountName must be read back as stored, got {:?}",
            answer.body["AccountName"]
        );
        assert_eq!(
            answer.member("AccountId"),
            own_id,
            "case {case:?}: the account id must not change, got {:?}",
            answer.body["AccountId"]
        );
        assert_eq!(
            answer.member("AccountState"),
            ACTIVE,
            "case {case:?}: AccountState must stay {ACTIVE}, got {:?}",
            answer.body["AccountState"]
        );
        assert_eq!(
            answer.body["AccountCreatedDate"], created,
            "case {case:?}: AccountCreatedDate is set once per account, so it must not be \
             regenerated per call, got {:?}",
            answer.body["AccountCreatedDate"]
        );
    }

    // Reading twice in a row reports the same record: a report changes nothing.
    let again = get_account_information(&router, json!({})).await;
    let last_name = names.last().expect("the table is not empty").1.clone();
    assert_eq!(
        again.member("AccountName"),
        last_name,
        "the last write must be what a later read reports, got {:?}",
        again.body
    );
    assert_eq!(
        again.body["AccountCreatedDate"], created,
        "a second read must report the same created date, got {:?}",
        again.body["AccountCreatedDate"]
    );
}

/// The resolved account id is what keys every record: naming the caller's own
/// account explicitly is the same record as naming none, and another account id
/// is a record of its own.
#[tokio::test]
async fn the_resolved_account_id_keys_the_record() {
    let own = default_account_id().to_string();
    let other = "210987654321";
    assert!(
        is_account_id(other) && other != own,
        "the test's second account id must be a different well-formed id"
    );

    let router = emulator();

    // An account this emulator holds no record of is outside the caller's
    // organization: the modelled refusal is AccessDeniedException.
    let unknown = get_account_information(&router, json!({"AccountId": other})).await;
    assert_eq!(
        unknown.status, 403,
        "an account with no record must be refused, got {} with {:?}",
        unknown.status, unknown.body
    );
    assert_eq!(
        unknown.error_code(),
        "AccessDeniedException",
        "expected AccessDeniedException, got {:?}",
        unknown.body
    );

    // Writing with the own id named explicitly, reading with it absent: one record.
    put_account_name(&router, json!({"AccountName": "Named By Id", "AccountId": own})).await;
    for (case, request) in [
        ("AccountId absent", json!({})),
        ("AccountId named explicitly", json!({"AccountId": own})),
    ] {
        let answer = get_account_information(&router, request).await;
        assert_eq!(
            answer.member("AccountName"),
            "Named By Id",
            "case {case:?}: both forms must land on the caller's own record, got {:?}",
            answer.body
        );
    }

    // A second account is its own record: its name and contacts are separate.
    put_account_name(&router, json!({"AccountName": "Member Account", "AccountId": other})).await;
    let mut contact = contact_request("BILLING", "Member");
    contact["AccountId"] = json!(other);
    put_alternate_contact(&router, contact.clone()).await;

    for (case, account, expected_name) in [
        ("the caller's own account", json!({}), "Named By Id"),
        (
            "the member account",
            json!({"AccountId": other}),
            "Member Account",
        ),
    ] {
        let answer = get_account_information(&router, account).await;
        assert_eq!(
            answer.member("AccountName"),
            expected_name,
            "case {case:?}: each account id keys its own name, got {:?}",
            answer.body
        );
    }

    let on_other =
        get_alternate_contact(&router, json!({"AccountId": other, "AlternateContactType": "BILLING"}))
            .await;
    assert_contact_matches("the member account's BILLING contact", &contact, &on_other);

    let on_own = get_alternate_contact(&router, json!({"AlternateContactType": "BILLING"})).await;
    assert_eq!(
        on_own.status, 404,
        "a contact stored on another account must not appear on the caller's own, got {} with {:?}",
        on_own.status, on_own.body
    );
}

/// The store holds at most `MAX_ACCOUNTS` records: a write that would ADD one
/// past the bound is refused, while every account already stored stays writable.
#[tokio::test]
async fn a_write_that_would_add_an_account_past_the_bound_is_refused() {
    let router = emulator();

    // The caller's own account is already stored, so this fills the rest.
    for index in 1..MAX_ACCOUNTS {
        let account_id = format!("{index:012}");
        assert!(
            is_account_id(&account_id),
            "case {index}: the generated account id must satisfy ^\\d{{12}}$, got {account_id:?}"
        );
        let answer =
            put_account_name(&router, json!({"AccountName": "Member", "AccountId": account_id}))
                .await;
        assert_eq!(
            answer.status, 200,
            "case {index}: a write within the bound must answer 200, got {} with {:?}",
            answer.status, answer.body
        );
    }

    // One more NEW id is past the bound, on either mutating operation.
    let past_bound = format!("{MAX_ACCOUNTS:012}");
    for (case, uri, request) in [
        (
            "PutAccountName for an account past the bound",
            "/putAccountName",
            json!({"AccountName": "One Too Many", "AccountId": past_bound}),
        ),
        (
            "PutAlternateContact for an account past the bound",
            "/putAlternateContact",
            json!({
                "AccountId": past_bound,
                "AlternateContactType": "BILLING",
                "Name": "Ada",
                "Title": "CFO",
                "EmailAddress": "ada@example.com",
                "PhoneNumber": "+1 206-555-0100",
            }),
        ),
    ] {
        let answer = call(&router, uri, request).await;
        assert_eq!(
            answer.status, 403,
            "case {case:?}: expected HTTP 403, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "AccessDeniedException",
            "case {case:?}: expected AccessDeniedException, got {:?}",
            answer.body
        );
    }

    // The refused writes stored nothing: that id is still unknown.
    let unknown = get_account_information(&router, json!({"AccountId": past_bound})).await;
    assert_eq!(
        unknown.status, 403,
        "a refused write must leave the account unknown, got {} with {:?}",
        unknown.status, unknown.body
    );

    // Accounts already in the store are unaffected by the bound.
    for (case, account_id) in [
        ("the caller's own account", default_account_id().to_string()),
        ("an account stored earlier", format!("{:012}", 1)),
    ] {
        let put = put_account_name(
            &router,
            json!({"AccountName": "Renamed", "AccountId": account_id}),
        )
        .await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: a stored account must stay writable, got {} with {:?}",
            put.status, put.body
        );
        let answer = get_account_information(&router, json!({"AccountId": account_id})).await;
        assert_eq!(
            answer.member("AccountName"),
            "Renamed",
            "case {case:?}: the update must be what a later read reports, got {:?}",
            answer.body
        );
    }
}

/// An input outside a modelled constraint is refused with HTTP 400
/// `ValidationException`, and a refused call stores nothing.
#[tokio::test]
async fn inputs_outside_the_modelled_constraints_are_refused() {
    // One long local part, so the email cases can sit exactly on the modelled
    // `@length(min: 1, max: 254)` bound and one past it.
    let at_email_bound = format!("{}@example.com", "a".repeat(254 - "@example.com".len()));
    let past_email_bound = format!("{}@example.com", "a".repeat(255 - "@example.com".len()));

    let cases: Vec<(&str, &str, Value)> = vec![
        // AccountId: ^\d{12}$
        (
            "an account id of too few digits",
            "/getAccountInformation",
            json!({"AccountId": "12345"}),
        ),
        (
            "an account id of too many digits",
            "/getAccountInformation",
            json!({"AccountId": "0000000000000"}),
        ),
        (
            "an account id that is not digits",
            "/getAccountInformation",
            json!({"AccountId": "abcdefghijkl"}),
        ),
        (
            "an account id on a write",
            "/putAccountName",
            json!({"AccountName": "Demo", "AccountId": "12345"}),
        ),
        // AccountName: @length(1..50), pattern ^[ -;=?-~]+$
        (
            "an account name below the length bound",
            "/putAccountName",
            json!({"AccountName": ""}),
        ),
        (
            "an account name past the length bound",
            "/putAccountName",
            json!({"AccountName": "N".repeat(ACCOUNT_NAME_MAX + 1)}),
        ),
        (
            "an account name outside the pattern",
            "/putAccountName",
            json!({"AccountName": "Demo<Account>"}),
        ),
        (
            "an account name with a tab",
            "/putAccountName",
            json!({"AccountName": "Demo\tAccount"}),
        ),
        (
            "no account name at all",
            "/putAccountName",
            json!({}),
        ),
        // AlternateContactType: BILLING | OPERATIONS | SECURITY
        (
            "a contact type the enum does not admit",
            "/putAlternateContact",
            contact_request("MARKETING", "Ada"),
        ),
        (
            "a contact type in the wrong case",
            "/getAlternateContact",
            json!({"AlternateContactType": "billing"}),
        ),
        (
            "no contact type on a delete",
            "/deleteAlternateContact",
            json!({}),
        ),
        // The contact members and their lengths and patterns.
        (
            "a missing required contact member",
            "/putAlternateContact",
            json!({
                "AlternateContactType": "BILLING",
                "Title": "CFO",
                "EmailAddress": "ada@example.com",
                "PhoneNumber": "+1 206-555-0100",
            }),
        ),
        (
            "an email address with no at sign",
            "/putAlternateContact",
            json!({
                "AlternateContactType": "BILLING",
                "Name": "Ada",
                "Title": "CFO",
                "EmailAddress": "ada.example.com",
                "PhoneNumber": "+1 206-555-0100",
            }),
        ),
        (
            "an email address past its length bound",
            "/putAlternateContact",
            json!({
                "AlternateContactType": "BILLING",
                "Name": "Ada",
                "Title": "CFO",
                "EmailAddress": past_email_bound,
                "PhoneNumber": "+1 206-555-0100",
            }),
        ),
        (
            "a phone number with letters",
            "/putAlternateContact",
            json!({
                "AlternateContactType": "BILLING",
                "Name": "Ada",
                "Title": "CFO",
                "EmailAddress": "ada@example.com",
                "PhoneNumber": "call me",
            }),
        ),
        (
            "a phone number past its length bound",
            "/putAlternateContact",
            json!({
                "AlternateContactType": "BILLING",
                "Name": "Ada",
                "Title": "CFO",
                "EmailAddress": "ada@example.com",
                "PhoneNumber": "+".repeat(26),
            }),
        ),
        (
            "a name past its length bound",
            "/putAlternateContact",
            json!({
                "AlternateContactType": "BILLING",
                "Name": "A".repeat(65),
                "Title": "CFO",
                "EmailAddress": "ada@example.com",
                "PhoneNumber": "+1 206-555-0100",
            }),
        ),
        (
            "a title past its length bound",
            "/putAlternateContact",
            json!({
                "AlternateContactType": "BILLING",
                "Name": "Ada",
                "Title": "T".repeat(51),
                "EmailAddress": "ada@example.com",
                "PhoneNumber": "+1 206-555-0100",
            }),
        ),
    ];

    let router = emulator();
    let before = get_account_information(&router, json!({})).await;

    for (case, uri, request) in cases {
        let answer = call(&router, uri, request).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: {uri} must answer 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            answer.body
        );
    }

    // Not one refused call stored anything.
    let after = get_account_information(&router, json!({})).await;
    assert_eq!(
        after.body, before.body,
        "a refused request must leave the account record untouched"
    );
    for contact_type in ALTERNATE_CONTACT_TYPES {
        let answer =
            get_alternate_contact(&router, json!({"AlternateContactType": contact_type})).await;
        assert_eq!(
            answer.status, 404,
            "case {contact_type:?}: a refused put must store no contact, got {} with {:?}",
            answer.status, answer.body
        );
    }

    // The same members ON the bounds are accepted, so the refusals above are the
    // bound and not a blanket rejection.
    let accepted = json!({
        "AlternateContactType": "BILLING",
        "Name": "A".repeat(64),
        "Title": "T".repeat(50),
        "EmailAddress": at_email_bound,
        "PhoneNumber": "+".repeat(25),
    });
    let put = put_alternate_contact(&router, accepted.clone()).await;
    assert_eq!(
        put.status, 200,
        "case \"every contact member exactly on its bound\": expected 200, got {} with {:?}",
        put.status, put.body
    );
    assert_contact_matches(
        "every contact member exactly on its bound",
        &accepted,
        &get_alternate_contact(&router, json!({"AlternateContactType": "BILLING"})).await,
    );
}
