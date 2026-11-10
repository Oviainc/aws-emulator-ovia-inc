// SPDX-License-Identifier: CC0-1.0

//! `UpdateUser` end to end over HTTP: what an update changes on a stored user object, and
//! what it leaves exactly as it stands.
//!
//! The rule under test: an update writes the members and attributes the request NAMES onto
//! the stored record — holding the values it carries for `ADD` and `REPLACE`, and holding
//! none for `REMOVE`, which `UpdateUserRequest$UpdateType` documents as the type that
//! clears a value — and touches nothing else about the object. A member the request leaves
//! out is not a member it asked to change, the naming members (the account name, its SID and
//! the names the directory derived for it) are the directory's own and not updatable, and
//! every later read answers from that one record.
//!
//! The rows vary what the rule generalises over: which `UpdateType` the request names and
//! whether it names one at all, which members it carries (the structural ones, the
//! attributes, both at once), and the case the attribute name is written in.
//!
//! A name the directory holds no user object under is `ResourceNotFoundException` — the
//! refusal's own wording is the `object_naming` suite's.

use serde_json::{json, Value};
use service_com_amazonaws_directoryservicedata_directoryservicedata::state::MAX_OBJECT_ATTRIBUTES;
use smithy_server_runtime::MultiServiceRouter;

use super::validation_envelope::{call, emulator, Answer, DIRECTORY};

/// `Attributes`' `@length` maximum — how many attributes ONE request may carry
/// (`smithy_query(query="shape", shape="Attributes")`).
const ATTRIBUTES_PER_REQUEST: usize = 25;

/// The `ValidationExceptionReason` a write refused for its size carries
/// (`smithy_query(query="shape", shape="ValidationExceptionReason")`).
const LDAP_SIZE_LIMIT_EXCEEDED: &str = "LDAP_SIZE_LIMIT_EXCEEDED";

/// The attributes every row's describe asks for: the two the create stored and one it did
/// not, so an attribute an update adds is reported and one it clears is gone.
const ASKED_ATTRIBUTES: [&str; 3] = ["Description", "Co", "Title"];

async fn create_user(router: &MultiServiceRouter, body: &Value) -> Answer {
    call(router, "/Users/CreateUser", DIRECTORY, body).await
}

async fn update_user(router: &MultiServiceRouter, body: &Value) -> Answer {
    call(router, "/Users/UpdateUser", DIRECTORY, body).await
}

async fn describe_user(router: &MultiServiceRouter, name: &str) -> Answer {
    call(
        router,
        "/Users/DescribeUser",
        DIRECTORY,
        &json!({"SAMAccountName": name, "OtherAttributes": ASKED_ATTRIBUTES}),
    )
    .await
}

/// The user every row starts from: every member an update can change, carrying a value.
fn created(name: &str) -> Value {
    json!({
        "SAMAccountName": name,
        "EmailAddress": "ada@corp.example.com",
        "GivenName": "Ada",
        "Surname": "Lovelace",
        "OtherAttributes": {
            "description": {"S": "a stored object"},
            "co": {"S": "Antarctica"},
        },
    })
}

/// One row: the update a request sends, and what the object holds afterwards.
struct Row {
    case: &'static str,
    /// The `UpdateUser` body, without the `SAMAccountName` the row's user is named by.
    update: fn() -> Value,
    /// `EmailAddress`, `GivenName` and `Surname` as the describe must report them
    /// afterwards; `null` for a member holding no value.
    expected_members: fn() -> Value,
    /// The `OtherAttributes` the describe must report afterwards, among the names
    /// [`ASKED_ATTRIBUTES`] asks for; `null` when it must report none.
    expected_attributes: fn() -> Value,
}

/// The table: one row per form an update takes.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a REPLACE of one structural member",
            update: || json!({"UpdateType": "REPLACE", "GivenName": "Grace"}),
            expected_members: || {
                json!({
                    "EmailAddress": "ada@corp.example.com",
                    "GivenName": "Grace",
                    "Surname": "Lovelace",
                })
            },
            expected_attributes: || {
                json!({"Description": {"S": "a stored object"}, "Co": {"S": "Antarctica"}})
            },
        },
        Row {
            case: "an update naming no UpdateType at all",
            update: || json!({"GivenName": "Grace"}),
            expected_members: || {
                json!({
                    "EmailAddress": "ada@corp.example.com",
                    "GivenName": "Grace",
                    "Surname": "Lovelace",
                })
            },
            expected_attributes: || {
                json!({"Description": {"S": "a stored object"}, "Co": {"S": "Antarctica"}})
            },
        },
        Row {
            case: "a REPLACE of every structural member",
            update: || {
                json!({
                    "UpdateType": "REPLACE",
                    "EmailAddress": "grace@corp.example.com",
                    "GivenName": "Grace",
                    "Surname": "Hopper",
                })
            },
            expected_members: || {
                json!({
                    "EmailAddress": "grace@corp.example.com",
                    "GivenName": "Grace",
                    "Surname": "Hopper",
                })
            },
            expected_attributes: || {
                json!({"Description": {"S": "a stored object"}, "Co": {"S": "Antarctica"}})
            },
        },
        Row {
            case: "a REMOVE of one structural member",
            update: || json!({"UpdateType": "REMOVE", "Surname": "Lovelace"}),
            expected_members: || {
                json!({
                    "EmailAddress": "ada@corp.example.com",
                    "GivenName": "Ada",
                    "Surname": null,
                })
            },
            expected_attributes: || {
                json!({"Description": {"S": "a stored object"}, "Co": {"S": "Antarctica"}})
            },
        },
        Row {
            case: "a REMOVE of every structural member",
            update: || {
                json!({
                    "UpdateType": "REMOVE",
                    "EmailAddress": "ada@corp.example.com",
                    "GivenName": "Ada",
                    "Surname": "Lovelace",
                })
            },
            expected_members: || {
                json!({"EmailAddress": null, "GivenName": null, "Surname": null})
            },
            expected_attributes: || {
                json!({"Description": {"S": "a stored object"}, "Co": {"S": "Antarctica"}})
            },
        },
        Row {
            case: "an ADD of an attribute the object held none of",
            update: || json!({"UpdateType": "ADD", "OtherAttributes": {"title": {"S": "Engineer"}}}),
            expected_members: || {
                json!({
                    "EmailAddress": "ada@corp.example.com",
                    "GivenName": "Ada",
                    "Surname": "Lovelace",
                })
            },
            expected_attributes: || {
                json!({
                    "Description": {"S": "a stored object"},
                    "Co": {"S": "Antarctica"},
                    "Title": {"S": "Engineer"},
                })
            },
        },
        Row {
            case: "a REPLACE of an attribute the object holds",
            update: || {
                json!({
                    "UpdateType": "REPLACE",
                    "OtherAttributes": {"description": {"S": "a changed object"}},
                })
            },
            expected_members: || {
                json!({
                    "EmailAddress": "ada@corp.example.com",
                    "GivenName": "Ada",
                    "Surname": "Lovelace",
                })
            },
            expected_attributes: || {
                json!({"Description": {"S": "a changed object"}, "Co": {"S": "Antarctica"}})
            },
        },
        Row {
            case: "a REPLACE naming the attribute in another case",
            update: || {
                json!({
                    "UpdateType": "REPLACE",
                    "OtherAttributes": {"DESCRIPTION": {"S": "a changed object"}},
                })
            },
            expected_members: || {
                json!({
                    "EmailAddress": "ada@corp.example.com",
                    "GivenName": "Ada",
                    "Surname": "Lovelace",
                })
            },
            expected_attributes: || {
                json!({"Description": {"S": "a changed object"}, "Co": {"S": "Antarctica"}})
            },
        },
        Row {
            case: "a REMOVE of one attribute",
            update: || json!({"UpdateType": "REMOVE", "OtherAttributes": {"co": {"S": "any"}}}),
            expected_members: || {
                json!({
                    "EmailAddress": "ada@corp.example.com",
                    "GivenName": "Ada",
                    "Surname": "Lovelace",
                })
            },
            expected_attributes: || json!({"Description": {"S": "a stored object"}}),
        },
        Row {
            case: "a REMOVE of every attribute the object holds",
            update: || {
                json!({
                    "UpdateType": "REMOVE",
                    "OtherAttributes": {"CO": {"S": "any"}, "Description": {"S": "any"}},
                })
            },
            expected_members: || {
                json!({
                    "EmailAddress": "ada@corp.example.com",
                    "GivenName": "Ada",
                    "Surname": "Lovelace",
                })
            },
            expected_attributes: || json!(null),
        },
        Row {
            case: "a structural member and an attribute at once",
            update: || {
                json!({
                    "GivenName": "Grace",
                    "OtherAttributes": {"title": {"S": "Engineer"}},
                })
            },
            expected_members: || {
                json!({
                    "EmailAddress": "ada@corp.example.com",
                    "GivenName": "Grace",
                    "Surname": "Lovelace",
                })
            },
            expected_attributes: || {
                json!({
                    "Description": {"S": "a stored object"},
                    "Co": {"S": "Antarctica"},
                    "Title": {"S": "Engineer"},
                })
            },
        },
    ]
}

/// RULE: an update writes what it names and leaves the rest of the object as it stands.
#[tokio::test]
async fn an_update_changes_the_members_it_names_and_nothing_else() {
    let router = emulator();

    for (index, row) in rows().into_iter().enumerate() {
        let case = row.case;
        // One user per row, inside `UserName`'s `@length(min: 1, max: 20)`, so no row can
        // read the object another row changed.
        let name = format!("updated{index}");
        let created = create_user(&router, &created(&name)).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreateUser must be served, got {} with {:?}",
            created.status, created.body
        );

        // What the directory derived for the object, which no update may change.
        let before = describe_user(&router, &name).await;
        assert_eq!(
            before.status, 200,
            "case {case:?}: DescribeUser must be served, got {} with {:?}",
            before.status, before.body
        );

        let mut body = (row.update)();
        body["SAMAccountName"] = json!(name);
        let updated = update_user(&router, &body).await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: UpdateUser must be served, got {} with {:?}",
            updated.status, updated.body
        );

        let after = describe_user(&router, &name).await;
        assert_eq!(
            after.status, 200,
            "case {case:?}: DescribeUser must be served after the update, got {} with {:?}",
            after.status, after.body
        );

        let expected_members = (row.expected_members)();
        for member in ["EmailAddress", "GivenName", "Surname"] {
            assert_eq!(
                after.body[member],
                expected_members[member],
                "case {case:?}: {member} must be {:?} after the update, got {:?}",
                expected_members[member],
                after.body
            );
        }
        assert_eq!(
            after.body["OtherAttributes"],
            (row.expected_attributes)(),
            "case {case:?}: expected the attributes {:?}, got {:?}",
            (row.expected_attributes)(),
            after.body
        );
        // The names the directory derived, the SID it minted and whether the account is
        // active are no part of what an update carries.
        for member in [
            "SAMAccountName",
            "SID",
            "DistinguishedName",
            "UserPrincipalName",
            "Realm",
            "Enabled",
        ] {
            assert_eq!(
                after.body[member], before.body[member],
                "case {case:?}: {member} must be what it was before the update, got {:?}",
                after.body
            );
        }
    }
}

/// `ClientToken` is this operation's `@idempotencyToken`: the same token with the same
/// parameters answers as the call it repeats, and with different ones is a conflict.
#[tokio::test]
async fn a_client_token_makes_a_repeat_answer_as_the_call_it_repeats() {
    let router = emulator();
    let name = "tokenUser";
    create_user(&router, &created(name)).await;

    let token = "6f8a0f2e-1f3c-4a2b-9c4d-0a1b2c3d4e5f";
    let first = update_user(
        &router,
        &json!({"SAMAccountName": name, "GivenName": "Grace", "ClientToken": token}),
    )
    .await;
    assert_eq!(
        first.status, 200,
        "the first update must be served, got {} with {:?}",
        first.status, first.body
    );

    let repeated = update_user(
        &router,
        &json!({"SAMAccountName": name, "GivenName": "Grace", "ClientToken": token}),
    )
    .await;
    assert_eq!(
        repeated.status, 200,
        "case a repeat of the same call: expected the answer of the call it repeats, got {} \
         with {:?}",
        repeated.status, repeated.body
    );

    let conflicting = update_user(
        &router,
        &json!({"SAMAccountName": name, "GivenName": "Ada", "ClientToken": token}),
    )
    .await;
    assert_eq!(
        conflicting.status, 409,
        "case the same token with other parameters: expected HTTP 409, got {} with {:?}",
        conflicting.status, conflicting.body
    );
    assert_eq!(
        conflicting.error_code(),
        "ConflictException",
        "case the same token with other parameters: expected ConflictException, got {:?}",
        conflicting.body
    );

    // The object holds what the served update wrote, and the conflict wrote nothing.
    let described = describe_user(&router, name).await;
    assert_eq!(
        described.body["GivenName"],
        json!("Grace"),
        "the object must hold what the served update wrote, got {:?}",
        described.body
    );
}

/// One attribute name per `index`, inside `LdapDisplayName`'s
/// `^[A-Za-z*][A-Za-z-*]*$` (letters and hyphens only, so no digits), distinct for every
/// index this suite uses.
fn generated_attribute(index: usize) -> String {
    let high = char::from(b'a' + u8::try_from(index / 26).expect("the suite stays inside a-z"));
    let low = char::from(b'a' + u8::try_from(index % 26).expect("a remainder below 26"));
    format!("gen{high}{low}")
}

/// RULE: one object's record holds at most `MAX_OBJECT_ATTRIBUTES` attributes — the
/// emulator's own ceiling on a store a caller can grow update after update — and a write
/// that would pass it is refused with the modelled size-limit reason, leaving the record as
/// it stands.
#[tokio::test]
async fn an_update_cannot_grow_one_record_past_the_attribute_ceiling() {
    let router = emulator();
    let name = "filledUser";
    create_user(&router, &created(name)).await;

    // The record starts with the two attributes the create stored, so the writes below fill
    // it to the ceiling exactly; each request carries at most what `Attributes` admits.
    let mut held = 2;
    let mut index = 0;
    while held < MAX_OBJECT_ATTRIBUTES {
        let wanted = (MAX_OBJECT_ATTRIBUTES - held).min(ATTRIBUTES_PER_REQUEST);
        let mut attributes = serde_json::Map::new();
        for _ in 0..wanted {
            attributes.insert(generated_attribute(index), json!({"S": "filled"}));
            index += 1;
        }
        let filled = update_user(
            &router,
            &json!({
                "SAMAccountName": name,
                "UpdateType": "ADD",
                "OtherAttributes": Value::Object(attributes),
            }),
        )
        .await;
        assert_eq!(
            filled.status, 200,
            "case {held} attributes held: a write inside the ceiling must be served, got {} \
             with {:?}",
            filled.status, filled.body
        );
        held += wanted;
    }

    // One more NAME is one attribute past the ceiling.
    let refused = update_user(
        &router,
        &json!({
            "SAMAccountName": name,
            "UpdateType": "ADD",
            "OtherAttributes": {generated_attribute(index): {"S": "one too many"}},
        }),
    )
    .await;
    assert_eq!(
        refused.status, 400,
        "case one attribute past the ceiling: expected HTTP 400, got {} with {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "ValidationException",
        "case one attribute past the ceiling: expected ValidationException, got {:?}",
        refused.body
    );
    assert_eq!(
        refused.body["Reason"],
        json!(LDAP_SIZE_LIMIT_EXCEEDED),
        "case one attribute past the ceiling: expected the size-limit reason, got {:?}",
        refused.body
    );
    assert!(
        refused
            .message()
            .is_some_and(|prose| prose.contains(&MAX_OBJECT_ATTRIBUTES.to_string())),
        "case one attribute past the ceiling: the refusal must state the ceiling, got {:?}",
        refused.body
    );

    // The refusal wrote nothing: the attribute it named is not held, and the ones the
    // record held are untouched.
    let described = call(
        &router,
        "/Users/DescribeUser",
        DIRECTORY,
        &json!({
            "SAMAccountName": name,
            "OtherAttributes": [generated_attribute(index), "Description"],
        }),
    )
    .await;
    assert_eq!(
        described.body["OtherAttributes"],
        json!({"Description": {"S": "a stored object"}}),
        "the refused write must leave the record as it was, got {:?}",
        described.body
    );

    // A write that replaces a name the record already holds grows nothing, so it is served
    // at the ceiling; so is one that clears a name, and after it there is room again.
    let replaced = update_user(
        &router,
        &json!({
            "SAMAccountName": name,
            "UpdateType": "REPLACE",
            "OtherAttributes": {"Description": {"S": "replaced at the ceiling"}},
        }),
    )
    .await;
    assert_eq!(
        replaced.status, 200,
        "case a replace at the ceiling: expected it to be served, got {} with {:?}",
        replaced.status, replaced.body
    );
    let cleared = update_user(
        &router,
        &json!({
            "SAMAccountName": name,
            "UpdateType": "REMOVE",
            "OtherAttributes": {generated_attribute(0): {"S": "any"}},
        }),
    )
    .await;
    assert_eq!(
        cleared.status, 200,
        "case a removal at the ceiling: expected it to be served, got {} with {:?}",
        cleared.status, cleared.body
    );
    let after_room = update_user(
        &router,
        &json!({
            "SAMAccountName": name,
            "UpdateType": "ADD",
            "OtherAttributes": {generated_attribute(index): {"S": "back inside"}},
        }),
    )
    .await;
    assert_eq!(
        after_room.status, 200,
        "case one attribute cleared: a write back inside the ceiling must be served, got {} \
         with {:?}",
        after_room.status, after_room.body
    );
}
