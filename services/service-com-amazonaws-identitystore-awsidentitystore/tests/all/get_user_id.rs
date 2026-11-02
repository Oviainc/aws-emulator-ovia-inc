// SPDX-License-Identifier: CC0-1.0

//! `GetUserId`: resolving a user's primary identifiers from a NON-primary unique
//! identifier, driven end to end over HTTP.
//!
//! Every case goes through the same front door a client uses — `POST /` with
//! `X-Amz-Target: AWSIdentityStore.GetUserId` and an `awsJson1_1` body — through the
//! production `MultiServiceRouter`, reusing the harness of
//! [`crate::identity_store_lifecycle`]. Nothing reaches into the state struct, so a
//! response that is not computed from persisted state cannot pass.
//!
//! The suites are TABLES. The rule under test is: `GetUserId` reports the stored
//! `UserId` / `UserArn` of the user whose attribute the request's `AlternateIdentifier`
//! names, refuses an identifier the service resolves no user by with
//! `ValidationException` (400), answers `ResourceNotFoundException` when the
//! identity store holds no such user, and persists nothing. The rows differ along what
//! that rule generalises over: which identifier form is sent, how the `AttributePath`
//! is spelled, the value's length and character class, the `AttributeValue`'s document
//! type, and which identity store is named.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{emulator, ok, refused, string, MISSING_STORE, NOT_FOUND, STORE};
use crate::identity_store_lifecycle::{assert_resource_arn, assert_uuid};

/// Which stored attribute a row resolves its user by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum By {
    UserName,
    EmailValue,
}

/// The `AttributePath` spellings a row sends, with the attribute each one selects.
///
/// The two paths come from the modelled doc of `GetUserIdRequest$AlternateIdentifier`
/// (`smithy_query(query="doc",shape="GetUserIdRequest",member="AlternateIdentifier")`):
/// "For the unique attribute, the only valid paths are `userName` and `emails.value`."
/// The re-cased spellings are rows of the same path, because SCIM attribute NAMES are
/// case-insensitive (RFC 7643 §2.1), the schema these attributes come from.
const PATH_ROWS: &[(&str, By)] = &[
    ("userName", By::UserName),
    ("UserName", By::UserName),
    ("USERNAME", By::UserName),
    ("emails.value", By::EmailValue),
    ("Emails.Value", By::EmailValue),
];

/// The bounds of the modelled `UserName` length (1..128, from
/// `smithy_query(query="shape",shape="UserName")`).
const USER_NAME_MIN_LEN: usize = 1;
const USER_NAME_MAX_LEN: usize = 128;

/// `UserName` values spanning the modelled `UserName` constraints: the shortest value
/// admitted, the longest, and one row per character class the pattern
/// `^[\p{L}\p{M}\p{S}\p{N}\p{P}]+$` admits (letters with marks, symbols, numbers,
/// punctuation). None of them is the `alice` the issue's recording used.
fn user_name_rows() -> Vec<(&'static str, String)> {
    vec![
        ("minimum length", "x".repeat(USER_NAME_MIN_LEN)),
        ("maximum length", "y".repeat(USER_NAME_MAX_LEN)),
        ("letters with marks", "José-Ängström".to_string()),
        ("symbols", "€£+=~".to_string()),
        ("numbers", "0123456789".to_string()),
        ("punctuation", "a.b_c!d%e".to_string()),
    ]
}

/// A `UniqueAttribute` identifier naming `path` = `value`.
fn unique_attribute(path: &str, value: Value) -> Value {
    json!({ "UniqueAttribute": { "AttributePath": path, "AttributeValue": value } })
}

/// A `GetUserId` request body.
fn get_user_id(store: &str, identifier: Value) -> Value {
    json!({ "IdentityStoreId": store, "AlternateIdentifier": identifier })
}

/// Create a user carrying `user_name` and, when given, one email, and return its
/// `(UserId, UserArn)` as `CreateUser` minted them.
///
/// `Emails` is `length 1..1` in the model, so a user carries at most one.
async fn create_user(
    router: &MultiServiceRouter,
    store: &str,
    user_name: &str,
    email: Option<&str>,
    case: &str,
) -> (String, String) {
    let mut body = json!({ "IdentityStoreId": store, "UserName": user_name });
    if let Some(email) = email {
        body["Emails"] = json!([{ "Value": email, "Type": "work", "Primary": true }]);
    }
    let created = ok(router, "CreateUser", body, case).await;
    (
        string(&created, "UserId", case),
        string(&created, "UserArn", case),
    )
}

#[tokio::test]
async fn a_user_is_resolved_by_every_documented_unique_attribute_path() {
    let router = emulator();

    for (index, (value_case, user_name)) in user_name_rows().into_iter().enumerate() {
        // A value for each supported attribute of the SAME user, so a row differs only
        // in which attribute the request names.
        let email = format!("row{index}.{value_case}@example.test").replace(' ', "-");
        let create_case = format!("create a user with a {value_case} UserName");
        let (user_id, user_arn) =
            create_user(&router, STORE, &user_name, Some(&email), &create_case).await;

        for (path, by) in PATH_ROWS {
            let case = format!("{value_case} UserName resolved by {path}");
            let value = match by {
                By::UserName => user_name.clone(),
                By::EmailValue => email.clone(),
            };
            let got = ok(
                &router,
                "GetUserId",
                get_user_id(STORE, unique_attribute(path, json!(value))),
                &case,
            )
            .await;

            let resolved_id = string(&got, "UserId", &case);
            let resolved_arn = string(&got, "UserArn", &case);
            assert_eq!(
                resolved_id, user_id,
                "case {case:?}: expected the stored UserId {user_id:?}, got {resolved_id:?}"
            );
            assert_eq!(
                resolved_arn, user_arn,
                "case {case:?}: expected the stored UserArn {user_arn:?}, got {resolved_arn:?}"
            );
            assert_eq!(
                string(&got, "IdentityStoreId", &case),
                STORE,
                "case {case:?}: expected the identity store that was named, got {got}"
            );
            assert_uuid(&resolved_id, "UserId", &case);
            assert_resource_arn(&resolved_arn, "user", &resolved_id, &case);

            // Round trip: the resolved id names that very user when read back through
            // `DescribeUser`, so the answer is the persisted record and not a value
            // built from the request.
            let described = ok(
                &router,
                "DescribeUser",
                json!({ "IdentityStoreId": STORE, "UserId": resolved_id.clone() }),
                &case,
            )
            .await;
            assert_eq!(
                string(&described, "UserName", &case),
                user_name,
                "case {case:?}: expected the resolved id to name the user that was created, \
                 got {described}"
            );
        }
    }
}

#[tokio::test]
async fn resolution_is_stable_and_scoped_to_one_identity_store() {
    let router = emulator();
    // The answer depends on which store the request names — and repeating a read
    // answers the same thing. A second store of this account is the other side of that
    // scope: the same `UserName` claimed in each resolves to that store's OWN user.
    let user_name = "shared-name";
    let elsewhere_store = "d-1b2c3d4e5f";
    let (here, here_arn) = create_user(
        &router,
        STORE,
        user_name,
        Some("here@example.test"),
        "one store",
    )
    .await;
    let (elsewhere, elsewhere_arn) = create_user(
        &router,
        elsewhere_store,
        user_name,
        Some("elsewhere@example.test"),
        "another store",
    )
    .await;
    assert_ne!(
        here, elsewhere,
        "expected two stores to mint two users for the same UserName, got {here} twice"
    );

    for (store, expected_id, expected_arn) in [
        (STORE, &here, &here_arn),
        (elsewhere_store, &elsewhere, &elsewhere_arn),
    ] {
        for attempt in 0..3 {
            let case = format!("{store} attempt {attempt}");
            let got = ok(
                &router,
                "GetUserId",
                get_user_id(store, unique_attribute("userName", json!(user_name))),
                &case,
            )
            .await;
            assert_eq!(
                &string(&got, "UserId", &case),
                expected_id,
                "case {case:?}: expected the user of that store, got {got}"
            );
            assert_eq!(
                &string(&got, "UserArn", &case),
                expected_arn,
                "case {case:?}: expected the stored UserArn, got {got}"
            );
        }
    }

    // The same lookup addressed to a store this emulator does not serve is refused,
    // every time.
    for attempt in 0..3 {
        let case = format!("another account's store, attempt {attempt}");
        let body = refused(
            &router,
            "GetUserId",
            get_user_id(MISSING_STORE, unique_attribute("userName", json!(user_name))),
            NOT_FOUND,
            "ResourceNotFoundException",
            &case,
        )
        .await;
        assert_eq!(
            body.get("ResourceType").and_then(Value::as_str),
            Some("IDENTITY_STORE"),
            "case {case:?}: expected the store to be the missing resource, got {body}"
        );
    }
}

#[tokio::test]
async fn the_single_winner_of_a_duplicate_user_name_is_the_one_resolved() {
    let router = emulator();
    // A `UserName` is a uniqueness claim, so a second create naming it conflicts and
    // the name still resolves to the FIRST user — the single winner.
    for (index, user_name) in ["winner-one", "winner-two"].into_iter().enumerate() {
        let case = format!("duplicate {user_name}");
        let email = format!("winner{index}@example.test");
        let (winner, winner_arn) =
            create_user(&router, STORE, user_name, Some(&email), &case).await;

        refused(
            &router,
            "CreateUser",
            json!({ "IdentityStoreId": STORE, "UserName": user_name }),
            409,
            "ConflictException",
            &case,
        )
        .await;

        let got = ok(
            &router,
            "GetUserId",
            get_user_id(STORE, unique_attribute("userName", json!(user_name))),
            &case,
        )
        .await;
        assert_eq!(
            string(&got, "UserId", &case),
            winner,
            "case {case:?}: expected the winning create's UserId, got {got}"
        );
        assert_eq!(
            string(&got, "UserArn", &case),
            winner_arn,
            "case {case:?}: expected the winning create's UserArn, got {got}"
        );
    }
}

#[tokio::test]
async fn an_identifier_no_user_carries_is_reported_as_not_found() {
    let router = emulator();
    let case = "setup";
    create_user(&router, STORE, "present", Some("present@example.test"), case).await;

    // Rows: which identifier form misses, and how — an absent user name, an absent
    // email, and an external id (no modelled operation of this service sets
    // `ExternalIds`, so none ever matches).
    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "unknown userName",
            STORE,
            unique_attribute("userName", json!("absent")),
        ),
        (
            "unknown emails.value",
            STORE,
            unique_attribute("emails.value", json!("absent@example.test")),
        ),
        (
            "a userName that differs only in case",
            STORE,
            unique_attribute("userName", json!("PRESENT")),
        ),
        (
            "external id",
            STORE,
            json!({ "ExternalId": { "Issuer": "https://idp.example.test", "Id": "ext-1" } }),
        ),
    ];

    for (case, store, identifier) in rows {
        let body = refused(
            &router,
            "GetUserId",
            get_user_id(store, identifier),
            NOT_FOUND,
            "ResourceNotFoundException",
            case,
        )
        .await;
        // The modelled `ResourceType` for a user, so a client can tell WHAT was missing.
        assert_eq!(
            body.get("ResourceType").and_then(Value::as_str),
            Some("USER"),
            "case {case:?}: expected ResourceType USER on the refusal, got {body}"
        );
    }

    // An identifier naming no store this emulator serves is a different missing resource:
    // the STORE, not the user the identifier names.
    let body = refused(
        &router,
        "GetUserId",
        get_user_id(MISSING_STORE, unique_attribute("userName", json!("present"))),
        NOT_FOUND,
        "ResourceNotFoundException",
        "another account's store",
    )
    .await;
    assert_eq!(
        body.get("ResourceType").and_then(Value::as_str),
        Some("IDENTITY_STORE"),
        "case \"another account's store\": expected the store to be the missing \
         resource, got {body}"
    );
}

#[tokio::test]
async fn an_identifier_the_service_resolves_no_user_by_is_refused() {
    let router = emulator();
    let case = "setup";
    let (user_id, _) =
        create_user(&router, STORE, "refusals", Some("refusals@example.test"), case).await;

    // Rows: an `AttributePath` that is not one of the two documented unique attributes
    // (including a group's path and sub-paths of the supported ones), and an
    // `AttributeValue` whose document type is not the string these attributes hold.
    // Every path below satisfies the modelled `AttributePath` pattern, so each refusal
    // comes from the operation rather than from the boundary validator.
    let rows: Vec<(&str, Value)> = vec![
        ("a group's displayName", unique_attribute("displayName", json!("refusals"))),
        ("the primary identifier", unique_attribute("id", json!(user_id.clone()))),
        ("a name sub-attribute", unique_attribute("name.givenName", json!("refusals"))),
        ("another email sub-attribute", unique_attribute("emails.type", json!("work"))),
        ("an external-id sub-attribute", unique_attribute("externalIds.id", json!("ext-1"))),
        ("the emails list itself", unique_attribute("emails", json!("refusals@example.test"))),
        ("a sub-path of userName", unique_attribute("userName.value", json!("refusals"))),
        ("a numeric userName value", unique_attribute("userName", json!(17))),
        ("a boolean userName value", unique_attribute("userName", json!(true))),
        ("a list userName value", unique_attribute("userName", json!(["refusals"]))),
        (
            "an object emails.value",
            unique_attribute("emails.value", json!({ "Value": "refusals@example.test" })),
        ),
        ("a null emails.value", unique_attribute("emails.value", Value::Null)),
    ];

    for (case, identifier) in rows {
        refused(
            &router,
            "GetUserId",
            get_user_id(STORE, identifier),
            400,
            "ValidationException",
            case,
        )
        .await;
    }
}

#[tokio::test]
async fn resolving_a_user_id_persists_nothing() {
    let router = emulator();
    // `GetUserId` is `@readonly`: a read of a store nothing has written to must not
    // write — it reports the user it holds none of, however often it is made, and the
    // store still holds nothing afterwards (a `ListUsers` of it is empty), so a CREATE
    // addressed to it afterwards is the FIRST user that store holds.
    let probe_stores: Vec<String> = (0..4)
        .map(|index| format!("d-{:010x}", 0xf000_0000u64 + index as u64))
        .collect();
    for store in &probe_stores {
        for attempt in 0..2 {
            let case = format!("read {attempt} of {store}");
            let refusal = refused(
                &router,
                "GetUserId",
                get_user_id(store, unique_attribute("userName", json!("nobody"))),
                NOT_FOUND,
                "ResourceNotFoundException",
                &case,
            )
            .await;
            assert_eq!(
                refusal.get("ResourceType").and_then(Value::as_str),
                Some("USER"),
                "case {case:?}: expected the USER to be the missing resource in a store \
                 that is served, got {refusal}"
            );
        }
        let case = format!("the store {store} still holds nothing");
        let listed = ok(
            &router,
            "ListUsers",
            json!({ "IdentityStoreId": store.clone() }),
            &case,
        )
        .await;
        assert_eq!(
            listed["Users"].as_array().map(Vec::len),
            Some(0),
            "case {case:?}: expected the reads to have written nothing, got {listed}"
        );
        let case = format!("create in {store} after the reads");
        let created = ok(
            &router,
            "CreateUser",
            json!({ "IdentityStoreId": store.clone(), "UserName": "first" }),
            &case,
        )
        .await;
        let resolved = ok(
            &router,
            "GetUserId",
            get_user_id(store, unique_attribute("userName", json!("first"))),
            &case,
        )
        .await;
        assert_eq!(
            string(&resolved, "UserId", &case),
            string(&created, "UserId", &case),
            "case {case:?}: expected the created user to resolve in the store it was \
             created in, got {resolved}"
        );
    }

    // A SUCCESSFUL read persists nothing either: the user resolved in the store that
    // DOES exist still resolves to the same ids, and that store still holds just the
    // one user — so a resolution neither minted nor touched a record.
    let store = STORE;
    let case = "a successful read, repeated";
    create_user(&router, store, "first", None, case).await;
    let resolved = ok(
        &router,
        "GetUserId",
        get_user_id(store, unique_attribute("userName", json!("first"))),
        case,
    )
    .await;
    let user_id = string(&resolved, "UserId", case);
    for attempt in 0..2 {
        let case = format!("{case} {attempt}");
        let again = ok(
            &router,
            "GetUserId",
            get_user_id(store, unique_attribute("userName", json!("first"))),
            &case,
        )
        .await;
        assert_eq!(
            string(&again, "UserId", &case),
            user_id,
            "case {case:?}: expected the same UserId on every read, got {again}"
        );
    }
    // Nothing was created by those reads: a second user named `first` in that store is
    // still the first duplicate, so the uniqueness claim is held by exactly one record.
    refused(
        &router,
        "CreateUser",
        json!({ "IdentityStoreId": store, "UserName": "first" }),
        409,
        "ConflictException",
        case,
    )
    .await;
}
