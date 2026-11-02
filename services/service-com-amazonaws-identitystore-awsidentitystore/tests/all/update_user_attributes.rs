// SPDX-License-Identifier: CC0-1.0

//! `UpdateUser`, driven end to end over HTTP.
//!
//! Every case goes through the front door a client uses: `POST /` with
//! `X-Amz-Target: AWSIdentityStore.UpdateUser` and an `awsJson1_1` body
//! (OPERATIONS.md, "Wire protocol"), dispatched by the production
//! [`MultiServiceRouter`], and every assertion about what was stored is read back
//! through `DescribeUser`. Nothing reaches into the state struct, so a response that
//! is not computed from persisted state cannot pass.
//!
//! The suites are TABLES. The rule under test is: each `AttributeOperation` applies to
//! the attribute its `AttributePath` names — WITH an `AttributeValue` it sets it (a
//! multi-valued attribute wholesale), WITHOUT one it removes it — the operations apply
//! in request order, the stored revision moves on, and a path or value the model does
//! not admit is refused without changing anything. The rows differ along the axes that
//! rule generalises over: which attribute, its type (string, enum, object,
//! sub-attribute, list), the entry count of a list, the case of the path and of the
//! members inside a value, and the form of the identifiers.
//!
//! The wire harness (`emulator`, `ok`, `refused`, …) is [`crate::harness`]'s, shared
//! rather than copied: every call still goes through the production
//! [`MultiServiceRouter`] front door and nothing reaches into the state struct.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{emulator, ok, refused, string, MISSING_STORE, NOT_FOUND, STORE};

/// The emulator's account id, as it appears in an identity store's ARN — the other
/// form the modelled `IdentityStoreId` pattern admits.
const ACCOUNT: &str = "000000000000";

/// The longest a `SensitiveStringType` attribute may be (modelled length 1..1024,
/// `smithy_query(query="shape",shape="User")`).
const TEXT_MAX_LEN: usize = 1024;

/// The longest a `UserName` may be (modelled length 1..128).
const USER_NAME_MAX_LEN: usize = 128;

/// Every attribute of a `User` an `UpdateUser` can set, as `DescribeUser` names it in
/// its response (from the modelled `User` shape). A case that updates one of these
/// must leave every other one absent on a user created without it.
///
/// `UserStatus` is deliberately NOT here: every stored user HAS a status (a created one
/// is `ENABLED`), so it is never absent and the rows that change it assert it directly.
const UPDATABLE_MEMBERS: &[&str] = &[
    "UserName",
    "Name",
    "DisplayName",
    "NickName",
    "ProfileUrl",
    "Emails",
    "Addresses",
    "PhoneNumbers",
    "UserType",
    "Title",
    "PreferredLanguage",
    "Locale",
    "Timezone",
    "Photos",
    "Website",
    "Birthdate",
    "Roles",
];

/// Whether `got` carries everything `expected` does.
///
/// Equality for scalars; for an object every key of `expected` must be present in
/// `got` with a value that in turn contains it. A member the model gives a default (an
/// `Email`'s `Primary`) may come back on a response that did not send it, which is why
/// this is containment rather than equality — that nothing was invented is asserted by
/// [`assert_only_member`].
fn contains(expected: &Value, got: &Value) -> bool {
    match (expected, got) {
        (Value::Object(expected), Value::Object(got)) => expected
            .iter()
            .all(|(key, value)| got.get(key).is_some_and(|actual| contains(value, actual))),
        (Value::Array(expected), Value::Array(got)) => {
            expected.len() == got.len()
                && expected
                    .iter()
                    .zip(got)
                    .all(|(value, actual)| contains(value, actual))
        }
        _ => expected == got,
    }
}

/// Assert `member` of `described` carries `expected`.
fn assert_member(described: &Value, member: &str, expected: &Value, case: &str) {
    let actual = described.get(member).unwrap_or(&Value::Null);
    assert!(
        contains(expected, actual),
        "case {case:?}: expected {member} to be {expected}, got {actual}"
    );
}

/// Assert `member` of `described` is absent.
fn assert_absent(described: &Value, member: &str, case: &str) {
    let actual = described.get(member).unwrap_or(&Value::Null);
    assert!(
        actual.is_null(),
        "case {case:?}: expected no {member}, got {actual}"
    );
}

/// Assert the only attributes the described user carries are `kept`: an update sets
/// what it names and invents nothing else.
fn assert_only_member(described: &Value, kept: &[&str], case: &str) {
    for member in UPDATABLE_MEMBERS {
        if kept.contains(member) {
            continue;
        }
        assert_absent(described, member, case);
    }
}

/// A `UserName` for one row: the modelled `UserName` pattern
/// (`^[\p{L}\p{M}\p{S}\p{N}\p{P}]+$`) admits letters, marks, symbols, numbers and
/// punctuation but NOT whitespace, so a case's spaces become hyphens.
fn user_name(case: &str, index: usize) -> String {
    format!("user-{}-{index}", case.replace(' ', "-"))
}

/// Create a user carrying `members`, and return its `UserId`.
async fn create_user(router: &MultiServiceRouter, members: Value, case: &str) -> String {
    let mut body = members;
    body["IdentityStoreId"] = json!(STORE);
    let created = ok(router, "CreateUser", body, case).await;
    assert_eq!(
        string(&created, "Revision", case),
        "1",
        "case {case:?}: a created user is at its first revision"
    );
    string(&created, "UserId", case)
}

/// `DescribeUser`'s report of `user`.
async fn describe(router: &MultiServiceRouter, user: &str, case: &str) -> Value {
    ok(
        router,
        "DescribeUser",
        json!({ "IdentityStoreId": STORE, "UserId": user }),
        case,
    )
    .await
}

/// One `UpdateUser` that must succeed, returning the revision it reports.
async fn update(
    router: &MultiServiceRouter,
    user: &str,
    operations: Value,
    case: &str,
) -> (String, Value) {
    let updated = ok(
        router,
        "UpdateUser",
        json!({ "IdentityStoreId": STORE, "UserId": user, "Operations": operations }),
        case,
    )
    .await;
    assert_eq!(
        string(&updated, "UserId", case),
        user.to_ascii_lowercase(),
        "case {case:?}: expected the updated user's id back, got {updated}"
    );
    assert_eq!(
        string(&updated, "UserArn", case),
        format!("arn:aws:identitystore:::user/{}", user.to_ascii_lowercase()),
        "case {case:?}: expected the user's ARN back, got {updated}"
    );
    let revision = string(&updated, "Revision", case);
    assert!(
        !revision.is_empty() && revision.bytes().all(|byte| byte.is_ascii_digit()),
        "case {case:?}: expected a decimal ResourceRevision, got {revision:?}"
    );
    (revision, updated)
}

/// One operation setting `path` to `value`.
fn set(path: &str, value: Value) -> Value {
    json!([{ "AttributePath": path, "AttributeValue": value }])
}

/// A string of `len` copies of `fill` — a value at or past a modelled length bound.
fn text(fill: char, len: usize) -> String {
    std::iter::repeat_n(fill, len).collect()
}

/// A set operation stores the attribute its path names, and `DescribeUser` reports it.
///
/// The rows differ along the attribute's type (string, enum, object, sub-attribute,
/// list), the number of entries in a list, the case of the path and of the member names
/// inside the value, and the length of a string against its modelled bounds.
#[tokio::test]
async fn a_set_operation_stores_the_attribute_its_path_names() {
    let router = emulator();
    let name = json!({
        "Formatted": "Ada Lovelace",
        "FamilyName": "Lovelace",
        "GivenName": "Ada",
        "MiddleName": "Byron",
        "HonorificPrefix": "Dr.",
        "HonorificSuffix": "FRS",
    });
    // (case, AttributePath, AttributeValue, the DescribeUser member it lands in, what
    // that member must then carry)
    let rows: Vec<(&str, &str, Value, &str, Value)> = vec![
        (
            "user name",
            "userName",
            json!("renamed-ada"),
            "UserName",
            json!("renamed-ada"),
        ),
        (
            "display name",
            "displayName",
            json!("Ada Lovelace"),
            "DisplayName",
            json!("Ada Lovelace"),
        ),
        (
            "display name, path in the model's case",
            "DisplayName",
            json!("Ada L"),
            "DisplayName",
            json!("Ada L"),
        ),
        (
            "display name, path in lower case",
            "displayname",
            json!("Ada"),
            "DisplayName",
            json!("Ada"),
        ),
        (
            "display name at the modelled minimum length",
            "displayName",
            json!("A"),
            "DisplayName",
            json!("A"),
        ),
        (
            "display name at the modelled maximum length",
            "displayName",
            json!(text('a', TEXT_MAX_LEN)),
            "DisplayName",
            json!(text('a', TEXT_MAX_LEN)),
        ),
        (
            "user name at the modelled maximum length",
            "userName",
            json!(text('u', USER_NAME_MAX_LEN)),
            "UserName",
            json!(text('u', USER_NAME_MAX_LEN)),
        ),
        (
            "nick name",
            "nickName",
            json!("Ada"),
            "NickName",
            json!("Ada"),
        ),
        (
            "profile url",
            "profileUrl",
            json!("https://example.invalid/ada"),
            "ProfileUrl",
            json!("https://example.invalid/ada"),
        ),
        (
            "user type",
            "userType",
            json!("Employee"),
            "UserType",
            json!("Employee"),
        ),
        (
            "title",
            "title",
            json!("Countess"),
            "Title",
            json!("Countess"),
        ),
        (
            "preferred language",
            "preferredLanguage",
            json!("en-GB"),
            "PreferredLanguage",
            json!("en-GB"),
        ),
        (
            "locale",
            "locale",
            json!("en-GB"),
            "Locale",
            json!("en-GB"),
        ),
        (
            "timezone",
            "timezone",
            json!("Europe/London"),
            "Timezone",
            json!("Europe/London"),
        ),
        (
            "website",
            "website",
            json!("https://example.invalid"),
            "Website",
            json!("https://example.invalid"),
        ),
        (
            "birthdate",
            "birthdate",
            json!("1815-12-10"),
            "Birthdate",
            json!("1815-12-10"),
        ),
        // `UserStatus` is a modelled enum, so each of its values is a row
        // (`USER_STATUS_VALUES`: DISABLED, ENABLED).
        (
            "user status, enabled",
            "userStatus",
            json!("ENABLED"),
            "UserStatus",
            json!("ENABLED"),
        ),
        (
            "user status, disabled",
            "userStatus",
            json!("DISABLED"),
            "UserStatus",
            json!("DISABLED"),
        ),
        (
            "name, every sub-attribute at once",
            "name",
            name.clone(),
            "Name",
            name.clone(),
        ),
        (
            "name, member names in lower case",
            "name",
            json!({ "givenname": "Ada", "familyname": "Lovelace" }),
            "Name",
            json!({ "GivenName": "Ada", "FamilyName": "Lovelace" }),
        ),
        (
            "name sub-attribute, given name",
            "name.givenName",
            json!("Ada"),
            "Name",
            json!({ "GivenName": "Ada" }),
        ),
        (
            "name sub-attribute, family name",
            "name.familyName",
            json!("Lovelace"),
            "Name",
            json!({ "FamilyName": "Lovelace" }),
        ),
        (
            "name sub-attribute, honorific prefix",
            "name.honorificPrefix",
            json!("Dr."),
            "Name",
            json!({ "HonorificPrefix": "Dr." }),
        ),
        (
            "emails",
            "emails",
            json!([{ "Value": "ada@example.invalid", "Type": "work", "Primary": true }]),
            "Emails",
            json!([{ "Value": "ada@example.invalid", "Type": "work", "Primary": true }]),
        ),
        (
            "phone numbers",
            "phoneNumbers",
            json!([{ "Value": "+1 (555) 0100", "Type": "mobile", "Primary": true }]),
            "PhoneNumbers",
            json!([{ "Value": "+1 (555) 0100", "Type": "mobile", "Primary": true }]),
        ),
        (
            "phone numbers, member names in lower case",
            "phoneNumbers",
            json!([{ "value": "+1 (555) 0101", "type": "work", "primary": false }]),
            "PhoneNumbers",
            json!([{ "Value": "+1 (555) 0101", "Type": "work", "Primary": false }]),
        ),
        (
            "addresses",
            "addresses",
            json!([{
                "StreetAddress": "12 High Street",
                "Locality": "London",
                "Region": "England",
                "PostalCode": "SW1A-1AA",
                "Country": "GB",
                "Formatted": "12 High Street, London",
                "Type": "home",
                "Primary": true,
            }]),
            "Addresses",
            json!([{
                "StreetAddress": "12 High Street",
                "Locality": "London",
                "Region": "England",
                "PostalCode": "SW1A-1AA",
                "Country": "GB",
                "Formatted": "12 High Street, London",
                "Type": "home",
                "Primary": true,
            }]),
        ),
        (
            "roles",
            "roles",
            json!([{ "Value": "Researcher", "Type": "work", "Primary": true }]),
            "Roles",
            json!([{ "Value": "Researcher", "Type": "work", "Primary": true }]),
        ),
        (
            "photos, one entry",
            "photos",
            json!([{ "Value": "https://example.invalid/ada.png" }]),
            "Photos",
            json!([{ "Value": "https://example.invalid/ada.png" }]),
        ),
        // `User$Photos` is modelled `length 1..3`, so three entries is its upper bound.
        (
            "photos, three entries at the modelled maximum",
            "photos",
            json!([
                { "Value": "https://example.invalid/1.png", "Type": "jpg", "Primary": true },
                { "Value": "https://example.invalid/2.png", "Display": "second" },
                { "Value": "https://example.invalid/3.gif" },
            ]),
            "Photos",
            json!([
                { "Value": "https://example.invalid/1.png", "Type": "jpg", "Primary": true },
                { "Value": "https://example.invalid/2.png", "Display": "second" },
                { "Value": "https://example.invalid/3.gif" },
            ]),
        ),
    ];

    for (index, (case, path, value, member, expected)) in rows.iter().enumerate() {
        let created_name = user_name(case, index);
        let user = create_user(&router, json!({ "UserName": created_name }), case).await;
        let (revision, _) = update(&router, &user, set(path, value.clone()), case).await;
        assert_eq!(
            revision, "2",
            "case {case:?}: expected the revision after one update on a fresh user"
        );

        let described = describe(&router, &user, case).await;
        assert_member(&described, member, expected, case);
        assert_eq!(
            string(&described, "Revision", case),
            revision,
            "case {case:?}: expected DescribeUser to report the revision UpdateUser answered"
        );
        // The user was created with a `UserName` and nothing else, so only that member
        // and the updated one may be present: an update reports state, it does not
        // synthesise a profile.
        assert_only_member(&described, &["UserName", member], case);
        if *member != "UserName" {
            assert_member(&described, "UserName", &json!(created_name), case);
        }
        let created_at = described["CreatedAt"].as_f64().unwrap_or_default();
        let updated_at = described["UpdatedAt"].as_f64().unwrap_or_default();
        assert!(
            updated_at >= created_at,
            "case {case:?}: expected UpdatedAt ({updated_at}) at or after CreatedAt ({created_at})"
        );
    }
}

/// An operation with no `AttributeValue` — or an explicit `null`, or an empty array for
/// a multi-valued attribute — removes the attribute, and `DescribeUser` then omits it.
#[tokio::test]
async fn an_operation_without_a_value_removes_the_attribute() {
    let router = emulator();
    // (case, AttributePath, the value to store first, the removing AttributeValue —
    // `None` for an operation that carries none at all, the DescribeUser member)
    let rows: Vec<(&str, &str, Value, Option<Value>, &str)> = vec![
        (
            "display name, no AttributeValue",
            "displayName",
            json!("Ada Lovelace"),
            None,
            "DisplayName",
        ),
        (
            "display name, explicit null",
            "displayName",
            json!("Ada Lovelace"),
            Some(Value::Null),
            "DisplayName",
        ),
        (
            "nick name",
            "nickName",
            json!("Ada"),
            None,
            "NickName",
        ),
        (
            "user name",
            "userName",
            json!("ada-to-be-removed"),
            None,
            "UserName",
        ),
        (
            "the whole name object",
            "name",
            json!({ "GivenName": "Ada", "FamilyName": "Lovelace" }),
            None,
            "Name",
        ),
        (
            "emails, no AttributeValue",
            "emails",
            json!([{ "Value": "ada@example.invalid" }]),
            None,
            "Emails",
        ),
        (
            "phone numbers, explicit null",
            "phoneNumbers",
            json!([{ "Value": "+1 (555) 0100" }]),
            Some(Value::Null),
            "PhoneNumbers",
        ),
        (
            "photos",
            "photos",
            json!([{ "Value": "https://example.invalid/ada.png" }]),
            None,
            "Photos",
        ),
        (
            "roles",
            "roles",
            json!([{ "Value": "Researcher" }]),
            None,
            "Roles",
        ),
        (
            "addresses",
            "addresses",
            json!([{ "Locality": "London" }]),
            None,
            "Addresses",
        ),
    ];

    for (index, (case, path, stored, removal, member)) in rows.iter().enumerate() {
        let user = create_user(&router, json!({ "UserName": user_name(case, index) }), case).await;
        update(&router, &user, set(path, stored.clone()), case).await;
        let described = describe(&router, &user, case).await;
        assert_member(&described, member, stored, &format!("{case}: stored first"));

        let operation = match removal {
            Some(value) => json!([{ "AttributePath": path, "AttributeValue": value }]),
            None => json!([{ "AttributePath": path }]),
        };
        let (revision, _) = update(&router, &user, operation, case).await;
        assert_eq!(
            revision, "3",
            "case {case:?}: expected a removal to move the revision on like any update"
        );
        let described = describe(&router, &user, case).await;
        assert_absent(&described, member, case);
    }

    // Removing ONE sub-attribute leaves its siblings alone; removing the last one
    // leaves no name at all.
    let case = "sub-attribute removal";
    let user = create_user(&router, json!({ "UserName": user_name(case, 0) }), case).await;
    update(
        &router,
        &user,
        set("name", json!({ "GivenName": "Ada", "FamilyName": "Lovelace" })),
        case,
    )
    .await;
    update(&router, &user, json!([{ "AttributePath": "name.givenName" }]), case).await;
    let described = describe(&router, &user, case).await;
    assert_member(&described, "Name", &json!({ "FamilyName": "Lovelace" }), case);
    assert_absent(&described["Name"], "GivenName", case);

    update(
        &router,
        &user,
        json!([{ "AttributePath": "name.familyName" }]),
        case,
    )
    .await;
    let described = describe(&router, &user, case).await;
    assert_absent(&described, "Name", "the last sub-attribute removed");
}

/// Several operations in one request all apply, in request order.
#[tokio::test]
async fn operations_apply_in_request_order() {
    let router = emulator();
    let case = "a set, an overriding set, a set and a removal";
    let user = create_user(&router, json!({ "UserName": user_name(case, 0) }), case).await;

    let (revision, _) = update(
        &router,
        &user,
        json!([
            { "AttributePath": "displayName", "AttributeValue": "First" },
            { "AttributePath": "nickName", "AttributeValue": "Nick" },
            { "AttributePath": "displayName", "AttributeValue": "Second" },
            { "AttributePath": "nickName" },
            { "AttributePath": "name.givenName", "AttributeValue": "Ada" },
            { "AttributePath": "name.familyName", "AttributeValue": "Lovelace" },
        ]),
        case,
    )
    .await;
    // One call is one revision, however many operations it carries.
    assert_eq!(
        revision, "2",
        "case {case:?}: expected one update to move the revision on once"
    );

    let described = describe(&router, &user, case).await;
    assert_member(&described, "DisplayName", &json!("Second"), case);
    assert_absent(&described, "NickName", case);
    assert_member(
        &described,
        "Name",
        &json!({ "GivenName": "Ada", "FamilyName": "Lovelace" }),
        case,
    );
}

/// `Revision` is a precondition: the update applies only while it matches, and every
/// successful update moves the stored revision on.
#[tokio::test]
async fn the_revision_is_a_precondition_and_moves_on_with_every_update() {
    let router = emulator();
    let case = "revision precondition";
    let user = create_user(&router, json!({ "UserName": user_name(case, 0) }), case).await;

    // A matching revision applies.
    let conditional = ok(
        &router,
        "UpdateUser",
        json!({
            "IdentityStoreId": STORE,
            "UserId": user,
            "Revision": "1",
            "Operations": [{ "AttributePath": "displayName", "AttributeValue": "First" }],
        }),
        case,
    )
    .await;
    assert_eq!(
        string(&conditional, "Revision", case),
        "2",
        "case {case:?}: expected the revision after the update"
    );

    // The same request again is stale: the user is no longer at revision 1.
    for stale in ["1", "9999"] {
        let refusal = refused(
            &router,
            "UpdateUser",
            json!({
                "IdentityStoreId": STORE,
                "UserId": user,
                "Revision": stale,
                "Operations": [{ "AttributePath": "displayName", "AttributeValue": "Second" }],
            }),
            409,
            "ConflictException",
            stale,
        )
        .await;
        assert_eq!(
            refusal.get("Reason").and_then(Value::as_str),
            Some("CONCURRENT_MODIFICATION"),
            "case {stale:?}: expected the modelled ConflictExceptionReason, got {refusal}"
        );
        let described = describe(&router, &user, stale).await;
        assert_member(&described, "DisplayName", &json!("First"), stale);
        assert_eq!(
            string(&described, "Revision", stale),
            "2",
            "case {stale:?}: a refused update changes nothing"
        );
    }

    // No `Revision` at all applies unconditionally, and each update moves it on.
    for expected in ["3", "4", "5"] {
        let (revision, _) = update(
            &router,
            &user,
            set("displayName", json!(format!("Name {expected}"))),
            expected,
        )
        .await;
        assert_eq!(
            revision, expected,
            "case {expected:?}: expected each update to move the revision on by one"
        );
    }

    // And the current revision is a precondition that still holds.
    let conditional = ok(
        &router,
        "UpdateUser",
        json!({
            "IdentityStoreId": STORE,
            "UserId": user,
            "Revision": "5",
            "Operations": [{ "AttributePath": "displayName" }],
        }),
        case,
    )
    .await;
    assert_eq!(
        string(&conditional, "Revision", case),
        "6",
        "case {case:?}: expected the current revision to be an accepted precondition"
    );
    assert_absent(&describe(&router, &user, case).await, "DisplayName", case);
}

/// A path the model does not admit, or a value of the wrong type or outside the length
/// the model gives that member, is refused with `ValidationException` — and the stored
/// user is left exactly as it was, even when an earlier operation of the same request
/// would have applied.
#[tokio::test]
async fn an_invalid_operation_is_refused_and_changes_nothing() {
    let router = emulator();
    let case = "invalid operations";
    let user = create_user(&router, json!({ "UserName": user_name(case, 0) }), case).await;
    update(&router, &user, set("displayName", json!("Ada Lovelace")), case).await;
    let before = describe(&router, &user, case).await;

    // (case, the Operations list)
    let rows: Vec<(&str, Value)> = vec![
        ("a path no user attribute has", set("nosuchattribute", json!("x"))),
        ("a sub-attribute of a list", set("emails.value", json!("x"))),
        // Attributes the identity store owns: the ids, the ARN, the revision and the
        // audit timestamps are minted by the service, never set by the caller.
        ("the user id", set("userId", json!("x"))),
        ("the user ARN", set("userArn", json!("x"))),
        ("the revision", set("revision", json!("7"))),
        ("the identity store id", set("identityStoreId", json!("d-0000000001"))),
        ("the creation timestamp", set("createdAt", json!("2020-01-01"))),
        ("the external ids", set("externalIds", json!([{ "Issuer": "idp", "Id": "x" }]))),
        // The wrong JSON type for the attribute.
        ("a number where a string belongs", set("displayName", json!(7))),
        ("an array where a string belongs", set("displayName", json!(["Ada"]))),
        ("an object where a string belongs", set("displayName", json!({ "x": "y" }))),
        ("a string where an object belongs", set("name", json!("Ada Lovelace"))),
        ("a string where a list belongs", set("emails", json!("ada@example.invalid"))),
        ("a number where a boolean belongs", set("emails", json!([{ "Value": "a@b.invalid", "Primary": 1 }]))),
        // Outside the modelled length of the member.
        ("an empty string", set("displayName", json!(""))),
        (
            "a string one past the modelled maximum",
            set("displayName", json!(text('a', TEXT_MAX_LEN + 1))),
        ),
        (
            "a user name one past the modelled maximum",
            set("userName", json!(text('u', USER_NAME_MAX_LEN + 1))),
        ),
        // Past the modelled maximum number of entries: `User$PhoneNumbers` is
        // `length 1..1` and `User$Photos` is `length 1..3`.
        (
            "two phone numbers where the model allows one",
            set(
                "phoneNumbers",
                json!([{ "Value": "+1 (555) 0100" }, { "Value": "+1 (555) 0101" }]),
            ),
        ),
        (
            "four photos where the model allows three",
            set(
                "photos",
                json!([
                    { "Value": "https://example.invalid/1.png" },
                    { "Value": "https://example.invalid/2.png" },
                    { "Value": "https://example.invalid/3.png" },
                    { "Value": "https://example.invalid/4.png" },
                ]),
            ),
        ),
        // Members the shape does not declare, and a required one left out.
        (
            "a member the entry shape does not declare",
            set("emails", json!([{ "Value": "a@b.invalid", "Nickname": "work" }])),
        ),
        (
            "a member the name shape does not declare",
            set("name", json!({ "GivenName": "Ada", "Surname": "Lovelace" })),
        ),
        (
            "a photo entry without its required Value",
            set("photos", json!([{ "Type": "png" }])),
        ),
        (
            "one entry member spelled twice in different cases",
            set("emails", json!([{ "Value": "a@b.invalid", "value": "c@d.invalid" }])),
        ),
        (
            "a number where a list belongs",
            set("phoneNumbers", json!(7)),
        ),
        // The model types these members as lists, so a lone entry still needs its
        // array.
        (
            "a lone entry with no array around it",
            set("emails", json!({ "Value": "ada@example.invalid" })),
        ),
        // Each of these members is modelled with a minimum length of 1, so an empty
        // array is not a value it admits (removal omits the AttributeValue instead).
        ("an empty array where the model requires one entry", set("emails", json!([]))),
        ("an empty array of photos", set("photos", json!([]))),
        (
            "a boolean where a list belongs",
            set("roles", json!(true)),
        ),
        // Outside the modelled enum.
        ("a user status the model does not declare", set("userStatus", json!("ACTIVE"))),
        // An invalid operation after a valid one: nothing is written.
        (
            "a valid operation followed by an invalid one",
            json!([
                { "AttributePath": "nickName", "AttributeValue": "Ada" },
                { "AttributePath": "displayName", "AttributeValue": 7 },
            ]),
        ),
    ];

    for (row, operations) in &rows {
        refused(
            &router,
            "UpdateUser",
            json!({ "IdentityStoreId": STORE, "UserId": user, "Operations": operations }),
            400,
            "ValidationException",
            row,
        )
        .await;
        let after = describe(&router, &user, row).await;
        assert_eq!(
            after, before,
            "case {row:?}: expected a refused update to leave the user unchanged"
        );
    }
}

/// An update naming a user that store does not hold, or an identifier naming no store
/// this emulator serves, is not found.
#[tokio::test]
async fn an_update_of_a_user_that_does_not_exist_is_not_found() {
    let router = emulator();
    let case = "not found";
    let user = create_user(&router, json!({ "UserName": user_name(case, 0) }), case).await;

    // (case, IdentityStoreId, UserId, the ResourceType of what is missing)
    let rows: Vec<(&str, String, String, &str)> = vec![
        (
            "a user id the store does not hold",
            STORE.to_string(),
            "a1b2c3d4-5678-90ab-cdef-1111eeee2222".to_string(),
            "USER",
        ),
        (
            "that user id in ARN form",
            STORE.to_string(),
            "arn:aws:identitystore:::user/a1b2c3d4-5678-90ab-cdef-1111eeee2222".to_string(),
            "USER",
        ),
        (
            // The identifier names a store of ANOTHER account, which is no store this
            // emulator serves, so THAT is the resource the refusal names.
            "a store of another account",
            MISSING_STORE.to_string(),
            user.clone(),
            "IDENTITY_STORE",
        ),
    ];

    for (row, store, identifier, expected_type) in &rows {
        let refusal = refused(
            &router,
            "UpdateUser",
            json!({
                "IdentityStoreId": store,
                "UserId": identifier,
                "Operations": [{ "AttributePath": "displayName", "AttributeValue": "Ada" }],
            }),
            NOT_FOUND,
            "ResourceNotFoundException",
            row,
        )
        .await;
        assert_eq!(
            refusal.get("ResourceType").and_then(Value::as_str),
            Some(*expected_type),
            "case {row:?}: expected the modelled ResourceType of the missing resource, \
             got {refusal}"
        );
    }

    // The store the refused update named is still no store this emulator serves: a
    // refusal creates nothing.
    refused(
        &router,
        "DescribeUser",
        json!({ "IdentityStoreId": MISSING_STORE, "UserId": user }),
        NOT_FOUND,
        "ResourceNotFoundException",
        "the store is still not served",
    )
    .await;
}

/// `UserName` is a uniqueness claim: an update cannot take one another user holds, and
/// a user that gives its name up releases it.
#[tokio::test]
async fn a_user_name_an_update_takes_stays_unique() {
    let router = emulator();
    let case = "user name uniqueness";
    let held = "ada-lovelace";
    let first = create_user(&router, json!({ "UserName": held }), "first").await;
    let second = create_user(&router, json!({ "UserName": "charles-babbage" }), "second").await;

    let refusal = refused(
        &router,
        "UpdateUser",
        json!({
            "IdentityStoreId": STORE,
            "UserId": second,
            "Operations": [{ "AttributePath": "userName", "AttributeValue": held }],
        }),
        409,
        "ConflictException",
        case,
    )
    .await;
    assert_eq!(
        refusal.get("Reason").and_then(Value::as_str),
        Some("UNIQUENESS_CONSTRAINT_VIOLATION"),
        "case {case:?}: expected the modelled ConflictExceptionReason, got {refusal}"
    );
    let described = describe(&router, &second, case).await;
    assert_member(&described, "UserName", &json!("charles-babbage"), case);
    assert_eq!(
        string(&described, "Revision", case),
        "1",
        "case {case:?}: a refused update changes nothing"
    );

    // Taking the name it already holds is not a conflict with itself.
    update(
        &router,
        &second,
        set("userName", json!("charles-babbage")),
        "its own name",
    )
    .await;

    // Once the first user gives the name up, the second can take it.
    update(&router, &first, json!([{ "AttributePath": "userName" }]), "released").await;
    update(&router, &second, set("userName", json!(held)), "reclaimed").await;
    assert_member(
        &describe(&router, &second, "reclaimed").await,
        "UserName",
        &json!(held),
        "reclaimed",
    );
    assert_absent(
        &describe(&router, &first, "released").await,
        "UserName",
        "released",
    );
}

/// Every form of identifier the modelled patterns admit names the same user and the
/// same store, so an update sent with any of them lands on one record.
#[tokio::test]
async fn any_accepted_identifier_form_names_the_same_user() {
    let router = emulator();
    let case = "identifier forms";
    let user = create_user(&router, json!({ "UserName": user_name(case, 0) }), case).await;

    // (case, IdentityStoreId, UserId, the display name this form sets)
    let rows: Vec<(&str, String, String, &str)> = vec![
        ("bare id", STORE.to_string(), user.clone(), "First"),
        (
            "user ARN",
            STORE.to_string(),
            format!("arn:aws:identitystore:::user/{user}"),
            "Second",
        ),
        (
            "upper-case user id",
            STORE.to_string(),
            user.to_ascii_uppercase(),
            "Third",
        ),
        (
            "identity store ARN",
            format!("arn:aws:identitystore::{ACCOUNT}:identitystore/{STORE}"),
            user.clone(),
            "Fourth",
        ),
    ];

    for (row, store, identifier, display_name) in &rows {
        let updated = ok(
            &router,
            "UpdateUser",
            json!({
                "IdentityStoreId": store,
                "UserId": identifier,
                "Operations": [{ "AttributePath": "displayName", "AttributeValue": display_name }],
            }),
            row,
        )
        .await;
        assert_eq!(
            string(&updated, "UserId", row),
            user,
            "case {row:?}: expected every form to name the one user"
        );
        assert_eq!(
            string(&updated, "IdentityStoreId", row),
            STORE,
            "case {row:?}: expected the store's own id back, got {updated}"
        );
        let described = describe(&router, &user, row).await;
        assert_member(&described, "DisplayName", &json!(display_name), row);
    }

    // Four updates of one user, so its revision moved on four times from the create.
    assert_eq!(
        string(&describe(&router, &user, case).await, "Revision", case),
        "5",
        "case {case:?}: expected every form to have updated the same record"
    );
}
