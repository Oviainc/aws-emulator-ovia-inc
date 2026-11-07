// SPDX-License-Identifier: CC0-1.0

//! The two layers every Directory Service Data operation passes before its handler, end to
//! end over HTTP: which ATTRIBUTE VALUES this API takes, and whether the caller may reach
//! the DIRECTORY a request names.
//!
//! Four rules are under test, each over the set it generalises across rather than over the
//! one request that showed it:
//!
//! 1. **A `DirectoryId` the account does not hold is refused, never served and never
//!    reported missing.** This data plane creates no directory — the Directory Service
//!    CONTROL plane does, and records it in the inventory this gate reads — so a
//!    syntactically valid `DirectoryId` the account does not hold is
//!    `AccessDeniedException`, HTTP 403, `Reason` `IAM_AUTH`, with a message naming the
//!    calling principal and nothing about the directory: the same answer from every one of
//!    the 17 operations, including the eleven that also declare `ResourceNotFoundException`.
//!    The refusal precedes pagination-token parsing, `ClientToken` semantics and every
//!    object lookup, and nothing of the refused call is persisted.
//! 2. **Only the `S` member of the `AttributeValue` union is taken.** An attribute sent as
//!    `N`, `BOOL` or `SS` is `ValidationException`, HTTP 400, `Reason`
//!    `INVALID_ATTRIBUTE_VALUE`, with the message `Invalid value for attribute:
//!    '<attribute>'.` — and that layer runs BEFORE the gate, so an unreachable directory
//!    does not hide it.
//! 3. **A `@sensitive` member's value is withheld from a constraint clause**, while a
//!    member the model does not mark is still echoed — the decision is made per member
//!    inside ONE message.
//! 4. **The clauses of one refusal are stated in this service's fixed order**, by the
//!    member path each is stated at, whichever order the request wrote its members in.
//!
//! Every case goes through the router, so each exercises routing, deserialization, the
//! layers above and serialization — the same path a client takes. The harness (the
//! emulator, the request walk and the clause envelopes) is `validation_envelope`'s.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_directoryservicedata_directoryservicedata::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use super::validation_envelope::{
    call, hold_directories, one_violation, one_withheld_violation, Answer, DIRECTORY,
    LDAP_DISPLAY_NAME_CEILING, LDAP_DISPLAY_NAME_FLOOR, VALIDATION_EXCEPTION,
};

/// The error code the gate refuses with: the shape
/// `com.amazonaws.directoryservicedata#AccessDeniedException`, `@httpError` 403, which all
/// 17 operations declare.
const ACCESS_DENIED_EXCEPTION: &str = "AccessDeniedException";

/// The `AccessDeniedReason` such a refusal carries
/// (`smithy_query(query="shape", shape="AccessDeniedReason")` — `IAM_AUTH`,
/// `DIRECTORY_AUTH`, `DATA_DISABLED`).
const IAM_AUTH: &str = "IAM_AUTH";

/// The `ValidationExceptionReason` the attribute layer carries
/// (`smithy_query(query="shape", shape="ValidationExceptionReason")` includes it).
const INVALID_ATTRIBUTE_VALUE: &str = "INVALID_ATTRIBUTE_VALUE";

/// `DirectoryId`s inside `^d-[0-9a-f]{10}$` that NO account here holds — the gate's own
/// case, varied along the digits so no row can pass on the shape of an id.
const UNHELD_DIRECTORIES: [&str; 4] = [
    "d-0000000000",
    "d-ffffffffff",
    "d-4f8a1c07e2",
    "d-63be20d149",
];

/// `SearchString`'s `@length` maximum, from `SearchUsers`' input
/// (`smithy_query(query="operation", operation="SearchUsers")` — len 1..64, `@sensitive`).
const SEARCH_STRING_CEILING: usize = 64;

/// `GivenName`'s and `Surname`'s `@length` maximum, both `@sensitive`
/// (`smithy_query(query="operation", operation="CreateUser")` — len 1..64).
const GIVEN_NAME_CEILING: usize = 64;

/// `EmailAddress`'s `@length` maximum, `@sensitive` (same call — len 1..256).
const EMAIL_ADDRESS_CEILING: usize = 256;

/// `NextToken`'s `@length` maximum, `@sensitive`
/// (`smithy_query(query="operation", operation="ListUsers")` — len 1..6144).
const NEXT_TOKEN_CEILING: usize = 6144;

/// `UserName`'s `@length` maximum — `CreateUser`'s `SAMAccountName`, which the model does
/// NOT mark `@sensitive` (same call — len 1..20).
const USER_NAME_CEILING: usize = 20;

/// `Realm`'s `@pattern`, as a clause states it: the model declares
/// `^([a-zA-Z0-9]+[\\.-])+([a-zA-Z0-9])+[.]?$`
/// (`smithy_query(query="shape", shape="Realm")`) and the anchors bracketing a whole-value
/// match are not part of the stated expression.
const REALM_PATTERN: &str = r"([a-zA-Z0-9]+[\\.-])+([a-zA-Z0-9])+[.]?";

/// `LdapDisplayName`'s `@pattern`, stated the same way — the shape an attribute NAME is
/// (`smithy_query(query="shape", shape="Attributes")`: the map's key, len 1..63, pattern
/// `^[A-Za-z*][A-Za-z-*]*$`).
const LDAP_DISPLAY_NAME_PATTERN: &str = "[A-Za-z*][A-Za-z-*]*";

/// Every route this service binds, with the `@required` members of each inside their own
/// constraints — one row per operation (17, as `OPERATIONS.md` lists them), so a rule that
/// holds for the service has to hold for every row.
fn every_operation() -> Vec<(&'static str, Value)> {
    let group = json!({"SAMAccountName": "engineering"});
    let user = json!({"SAMAccountName": "jdoe"});
    let membership = json!({"GroupName": "engineering", "MemberName": "jdoe"});
    let search = json!({"SearchString": "j", "SearchAttributes": ["cn"]});
    vec![
        ("/GroupMemberships/AddGroupMember", membership.clone()),
        ("/GroupMemberships/ListGroupMembers", group.clone()),
        ("/GroupMemberships/ListGroupsForMember", user.clone()),
        ("/GroupMemberships/RemoveGroupMember", membership),
        ("/Groups/CreateGroup", group.clone()),
        ("/Groups/DeleteGroup", group.clone()),
        ("/Groups/DescribeGroup", group.clone()),
        ("/Groups/ListGroups", json!({})),
        ("/Groups/SearchGroups", search.clone()),
        ("/Groups/UpdateGroup", group),
        ("/Users/CreateUser", user.clone()),
        ("/Users/DeleteUser", user.clone()),
        ("/Users/DescribeUser", user.clone()),
        ("/Users/DisableUser", user.clone()),
        ("/Users/ListUsers", json!({})),
        ("/Users/SearchUsers", search),
        ("/Users/UpdateUser", user),
    ]
}

/// The operations whose input carries an attribute map (`OtherAttributes`, targeting
/// `Attributes`), with the `@required` members each needs — read off
/// `smithy_query(query="operation")`: four of the seventeen.
fn every_operation_carrying_attributes() -> [(&'static str, Value); 4] {
    [
        ("/Users/CreateUser", json!({"SAMAccountName": "jdoe"})),
        ("/Users/UpdateUser", json!({"SAMAccountName": "jdoe"})),
        (
            "/Groups/CreateGroup",
            json!({"SAMAccountName": "engineering"}),
        ),
        (
            "/Groups/UpdateGroup",
            json!({"SAMAccountName": "engineering"}),
        ),
    ]
}

/// The emulator over an engine the caller keeps, so a case can record a directory or drop
/// one mid-test the way the control plane's create and delete do.
fn emulator_over(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// Drop the account's record of `directory`, as the control plane's `DeleteDirectory` does.
fn drop_directory(engine: &engine::InMemoryEngine, directory: &str) {
    let inventory = service_com_amazonaws_directoryservicedata_directoryservicedata::state::
        ad_directory_inventory(engine)
        .expect("the shared engine holds the AD directory inventory");
    inventory.lock().forget(directory);
}

/// The message a denial states: the calling principal, and nothing else. The principal is
/// the one this emulator's credential speaks for, read from the shared definition STS and
/// IAM answer for it from rather than spelled again here.
fn not_authorized() -> String {
    format!(
        "User: {principal} is not authorized to access this resource",
        principal = service_common::caller_principal::ambient_user_arn()
    )
}

/// Assert one call was refused by the gate: 403 `AccessDeniedException` in the body and the
/// header, `Reason` `IAM_AUTH`, the principal-only message, and nothing of the directory or
/// the object the request named anywhere in the body.
fn assert_denied(case: &str, answer: &Answer, directory: &str) {
    assert_eq!(
        answer.status, 403,
        "case {case:?}: expected HTTP 403, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        ACCESS_DENIED_EXCEPTION,
        "case {case:?}: expected error code {ACCESS_DENIED_EXCEPTION}, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.error_type, ACCESS_DENIED_EXCEPTION,
        "case {case:?}: expected x-amzn-errortype {ACCESS_DENIED_EXCEPTION}, got {:?}",
        answer.error_type
    );
    assert_eq!(
        answer.body.get("Reason").and_then(Value::as_str),
        Some(IAM_AUTH),
        "case {case:?}: expected Reason {IAM_AUTH}, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.message(),
        Some(not_authorized().as_str()),
        "case {case:?}: expected Message {:?}, got {:?}",
        not_authorized(),
        answer.body
    );
    let rendered = answer.body.to_string();
    assert!(
        !rendered.contains(directory),
        "case {case:?}: a denial must not name the directory, got {rendered:?}"
    );
}

/// RULE 1: every operation answers a well-formed `DirectoryId` the account does not hold
/// with the one denial — whichever operation, and whatever the id.
///
/// The rows are the whole surface (17 routes) crossed with ids differing along their
/// digits, so neither a per-operation answer nor one keyed to a particular id passes.
#[tokio::test]
async fn every_operation_denies_a_directory_the_account_does_not_hold() {
    let router = super::validation_envelope::emulator();

    for (route, body) in every_operation() {
        for directory in UNHELD_DIRECTORIES {
            let answer = call(&router, route, directory, &body).await;
            assert_denied(&format!("{route} on {directory}"), &answer, directory);
        }
    }
}

/// The same request is SERVED for a directory the account does hold, so the denial above is
/// the gate's doing and not the request's: no row answers 403, and the reads report that
/// directory's (empty) inventory.
#[tokio::test]
async fn every_operation_is_served_for_a_directory_the_account_holds() {
    let router = super::validation_envelope::emulator();

    for (route, body) in every_operation() {
        let answer = call(&router, route, DIRECTORY, &body).await;
        assert_ne!(
            answer.status, 403,
            "route {route:?}: a directory the account holds must not be denied, got {:?}",
            answer.body
        );
        assert_ne!(
            answer.error_code(),
            ACCESS_DENIED_EXCEPTION,
            "route {route:?}: a directory the account holds must not be denied, got {:?}",
            answer.body
        );
    }
}

/// RULE 1, the layer ORDER: the denial precedes pagination-token parsing and `ClientToken`
/// semantics, and the modelled constraints precede the denial.
///
/// Each row sends the SAME member twice — once to a directory the account does not hold
/// (where the answer is the denial) and once to one it holds (where the layer that owns
/// that member answers instead), so the order of the two layers is what the pair pins.
#[tokio::test]
async fn the_denial_precedes_token_and_idempotency_handling() {
    let router = super::validation_envelope::emulator();
    let unheld = UNHELD_DIRECTORIES[0];

    // (case, route, body, the error code the HELD directory answers)
    let rows: [(&str, &str, Value, &str); 3] = [
        (
            "a NextToken this listing never issued",
            "/Users/ListUsers",
            json!({"NextToken": "not-a-token-this-listing-issued"}),
            VALIDATION_EXCEPTION,
        ),
        (
            "a fabricated NextToken on a group listing",
            "/Groups/ListGroups",
            json!({"NextToken": "also-not-a-token"}),
            VALIDATION_EXCEPTION,
        ),
        (
            "a well-formed ClientToken on a create",
            "/Users/CreateUser",
            json!({"SAMAccountName": "sim2real", "ClientToken": "a-client-token-01"}),
            // The create is SERVED for a directory the account holds, so there is no
            // error code at all.
            "",
        ),
    ];

    for (case, route, body, served_code) in rows {
        let denied = call(&router, route, unheld, &body).await;
        assert_denied(case, &denied, unheld);

        let served = call(&router, route, DIRECTORY, &body).await;
        assert_eq!(
            served.error_code(),
            served_code,
            "case {case:?}: for a directory the account holds the layer that owns the member \
             must answer, expected {served_code:?}, got {:?}",
            served.body
        );
    }
}

/// A value outside a modelled constraint is refused BEFORE the gate, so a caller on an
/// unreachable directory is still told what is wrong with its request.
///
/// The rows differ along the constraint class (a `@pattern`, a `@range` ceiling, a
/// `@length` ceiling, an absent `@required` member) and the operation, and every one is
/// sent to a directory the account does not hold.
#[tokio::test]
async fn the_modelled_constraints_are_refused_before_the_gate() {
    let router = super::validation_envelope::emulator();
    let unheld = UNHELD_DIRECTORIES[1];

    let rows: [(&str, &str, Value); 4] = [
        (
            "a Realm outside its @pattern",
            "/Groups/ListGroups",
            json!({"Realm": "not a realm!"}),
        ),
        (
            "a MaxResults past its @range ceiling",
            "/Users/ListUsers",
            json!({"MaxResults": 251}),
        ),
        (
            "a SearchString past its @length ceiling",
            "/Users/SearchUsers",
            json!({
                "SearchString": "s".repeat(SEARCH_STRING_CEILING + 1),
                "SearchAttributes": ["Cn"],
            }),
        ),
        (
            "a @required member absent altogether",
            "/Users/CreateUser",
            json!({}),
        ),
    ];

    for (case, route, body) in rows {
        let refused = call(&router, route, unheld, &body).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            VALIDATION_EXCEPTION,
            "case {case:?}: the constraint layer must answer before the gate, got {:?}",
            refused.body
        );
    }
}

/// RULE 1, information hiding and persistence: a denied call writes nothing, so the
/// directory holds exactly nothing once the account does hold it.
#[tokio::test]
async fn a_denied_call_persists_nothing() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_over(&engine);
    let directory = UNHELD_DIRECTORIES[2];

    for (case, route, body) in [
        (
            "CreateUser",
            "/Users/CreateUser",
            json!({"SAMAccountName": "sim2real", "ClientToken": "client-token-01"}),
        ),
        (
            "CreateGroup",
            "/Groups/CreateGroup",
            json!({"SAMAccountName": "sim2real-group"}),
        ),
        (
            "AddGroupMember",
            "/GroupMemberships/AddGroupMember",
            json!({"GroupName": "sim2real-group", "MemberName": "sim2real"}),
        ),
    ] {
        let denied = call(&router, route, directory, &body).await;
        assert_denied(case, &denied, directory);
    }

    // The account now holds the directory, as a control-plane create would have recorded:
    // every write above was refused, so it holds no object of its own.
    hold_directories(&engine, &[directory]);
    let listed = call(&router, "/Users/ListUsers", directory, &json!({})).await;
    assert_eq!(
        listed.status, 200,
        "a directory the account holds must be served, got {} with {:?}",
        listed.status, listed.body
    );
    assert_eq!(
        listed.body["Users"],
        json!([]),
        "a denied create must have persisted nothing, got {:?}",
        listed.body
    );
    let described = call(
        &router,
        "/Users/DescribeUser",
        directory,
        &json!({"SAMAccountName": "sim2real"}),
    )
    .await;
    assert_eq!(
        described.error_code(),
        "ResourceNotFoundException",
        "a denied create must have persisted no user, got {:?}",
        described.body
    );
}

/// RULE 1, where the answer comes from: the gate follows the account's own inventory, so a
/// directory the control plane records is served and one it drops is denied again.
#[tokio::test]
async fn the_gate_follows_the_accounts_inventory() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_over(&engine);
    let directory = UNHELD_DIRECTORIES[3];

    let before = call(&router, "/Groups/ListGroups", directory, &json!({})).await;
    assert_denied("before the account holds it", &before, directory);

    hold_directories(&engine, &[directory]);
    let served = call(&router, "/Groups/ListGroups", directory, &json!({})).await;
    assert_eq!(
        served.status, 200,
        "once the account holds the directory it must be served, got {} with {:?}",
        served.status, served.body
    );

    drop_directory(&engine, directory);
    let after = call(&router, "/Groups/ListGroups", directory, &json!({})).await;
    assert_denied("after the account stops holding it", &after, directory);
}

/// RULE 2: the `AttributeValue` member the request sent decides — `S` is taken, `N`, `BOOL`
/// and `SS` are refused with the attribute layer's own envelope.
///
/// The rows cross every operation that carries an attribute map with the three members the
/// API does not take, varying the attribute name and the value, and every row is sent to a
/// directory the account does NOT hold: the refusal is the attribute layer's, which runs
/// before the gate.
#[tokio::test]
async fn only_the_string_member_of_an_attribute_value_is_taken() {
    let router = super::validation_envelope::emulator();
    let unheld = UNHELD_DIRECTORIES[0];

    // (case, the attribute name, the value as the request sends it)
    let refused_values: [(&str, &str, Value); 5] = [
        ("the N member", "codePage", json!({"N": 16})),
        ("the N member, another attribute", "Floor", json!({"N": -1})),
        ("the BOOL member", "isCriticalSystemObject", json!({"BOOL": true})),
        ("the SS member", "servicePrincipalName", json!({"SS": ["a", "b"]})),
        ("the SS member, empty", "otherTelephone", json!({"SS": []})),
    ];

    for (route, required) in every_operation_carrying_attributes() {
        for (case, attribute, value) in &refused_values {
            let mut body = required.clone();
            body["OtherAttributes"] = json!({(*attribute): value});
            let refused = call(&router, route, unheld, &body).await;
            let what = format!("{case} on {route}");

            assert_eq!(
                refused.status, 400,
                "case {what:?}: expected HTTP 400, got {} with {:?}",
                refused.status, refused.body
            );
            assert_eq!(
                refused.error_code(),
                VALIDATION_EXCEPTION,
                "case {what:?}: expected {VALIDATION_EXCEPTION}, got {:?}",
                refused.body
            );
            assert_eq!(
                refused.body.get("Reason").and_then(Value::as_str),
                Some(INVALID_ATTRIBUTE_VALUE),
                "case {what:?}: expected Reason {INVALID_ATTRIBUTE_VALUE}, got {:?}",
                refused.body
            );
            assert_eq!(
                refused.message(),
                Some(format!("Invalid value for attribute: '{attribute}'.").as_str()),
                "case {what:?}: expected the attribute named, got {:?}",
                refused.body
            );
            // The members of the union are every one `@sensitive`, so no value is echoed.
            assert!(
                !refused.message().unwrap_or_default().contains("16"),
                "case {what:?}: the sentence states no value, got {:?}",
                refused.body
            );
        }
    }
}

/// RULE 2's other half: an attribute NAME outside the constraints the model declares for one
/// is refused in the constraint layer's envelope — the clause stated at the MAP, over the
/// set its keys have to satisfy, with no `Reason`.
///
/// An attribute name is a key of the `Attributes` map, whose key shape is `LdapDisplayName`
/// (`smithy_query(query="shape", shape="Attributes")` — len 1..63, pattern
/// `^[A-Za-z*][A-Za-z-*]*$`). The rows differ along which of those constraints the name
/// breaks and along the operation carrying the map, and every one is sent to a directory the
/// account does NOT hold: this layer runs before the gate.
#[tokio::test]
async fn an_attribute_name_outside_the_modelled_constraints_is_refused() {
    let router = super::validation_envelope::emulator();
    let unheld = UNHELD_DIRECTORIES[2];

    // Each name breaks a different one of the key shape's constraints; the clause is the
    // same either way, because it is about the set a key must satisfy and not about the
    // constraint one key happened to break.
    let refused_names: [(&str, String); 4] = [
        (
            "a name whose first character the @pattern excludes",
            "1BadAttribute".to_string(),
        ),
        (
            "a name carrying a character the @pattern excludes",
            "code Page".to_string(),
        ),
        (
            "a name under the @length floor, which its @pattern also excludes",
            String::new(),
        ),
        (
            "a name one character past the @length ceiling",
            "A".repeat(LDAP_DISPLAY_NAME_CEILING + 1),
        ),
    ];

    for (route, required) in every_operation_carrying_attributes() {
        for (case, name) in &refused_names {
            let mut body = required.clone();
            body["OtherAttributes"] = json!({name.as_str(): {"S": "a taken value"}});
            let refused = call(&router, route, unheld, &body).await;
            let what = format!("{case} on {route}");

            assert_eq!(
                refused.status, 400,
                "case {what:?}: expected HTTP 400, got {} with {:?}",
                refused.status, refused.body
            );
            assert_eq!(
                refused.error_code(),
                VALIDATION_EXCEPTION,
                "case {what:?}: expected {VALIDATION_EXCEPTION}, got {:?}",
                refused.body
            );
            // The clause is stated at the map, quoting the entry the request carried —
            // the key disclosed, the `@sensitive` value withheld behind its marker — and
            // naming the whole constraint set a key has to satisfy.
            let expected = format!(
                "1 validation error detected: \
                 Value '{{{name}=AttributeValue(s=*** REDACTED ***)}}' at 'otherAttributes' \
                 failed to satisfy constraint: Map keys must satisfy constraint: \
                 [Member must have length less than or equal to {LDAP_DISPLAY_NAME_CEILING}, \
                 Member must have length greater than or equal to {LDAP_DISPLAY_NAME_FLOOR}, \
                 Member must satisfy regular expression pattern: {LDAP_DISPLAY_NAME_PATTERN}]"
            );
            assert_eq!(
                refused.message(),
                Some(expected.as_str()),
                "case {what:?}: expected Message {expected:?}, got {:?}",
                refused.body
            );
            assert!(
                !refused.message().unwrap_or_default().contains("a taken value"),
                "case {what:?}: the @sensitive value must be withheld, got {:?}",
                refused.body
            );
            assert!(
                refused.body.get("Reason").is_none(),
                "case {what:?}: a constraint refusal states no Reason, got {:?}",
                refused.body
            );
        }
    }
}

/// A name INSIDE every one of those constraints is not refused by that layer: each published
/// bound of `LdapDisplayName`, and a name carrying every character class its `@pattern`
/// admits, passes and reaches the gate instead.
#[tokio::test]
async fn an_attribute_name_inside_every_bound_passes_the_layer() {
    let router = super::validation_envelope::emulator();
    let unheld = UNHELD_DIRECTORIES[3];

    for (case, name) in [
        ("at the @length floor", "A".repeat(LDAP_DISPLAY_NAME_FLOOR)),
        (
            "at the @length ceiling",
            "A".repeat(LDAP_DISPLAY_NAME_CEILING),
        ),
        (
            "every character class the @pattern admits",
            "*Code-Page*".to_string(),
        ),
        ("an ordinary attribute name", "codePage".to_string()),
    ] {
        let body = json!({
            "SAMAccountName": "jdoe",
            "OtherAttributes": {name.as_str(): {"S": "a taken value"}},
        });
        let answer = call(&router, "/Users/CreateUser", unheld, &body).await;
        assert_denied(&format!("a name {case}"), &answer, unheld);
    }
}

/// The same attribute sent as the `S` member passes that layer: on a directory the account
/// does not hold it reaches the gate (403, not 400), and on one it holds it is served.
#[tokio::test]
async fn the_string_member_of_an_attribute_value_passes_the_layer() {
    let router = super::validation_envelope::emulator();
    let unheld = UNHELD_DIRECTORIES[1];

    for (route, required) in every_operation_carrying_attributes() {
        let mut body = required.clone();
        body["OtherAttributes"] = json!({"description": {"S": "a taken value"}});

        let denied = call(&router, route, unheld, &body).await;
        assert_denied(&format!("the S member on {route}"), &denied, unheld);

        let served = call(&router, route, DIRECTORY, &body).await;
        assert_ne!(
            served.error_code(),
            VALIDATION_EXCEPTION,
            "route {route:?}: the S member must pass the attribute layer, got {:?}",
            served.body
        );
    }
}

/// RULE 3: a constraint clause about a member the model marks `@sensitive` states no value,
/// and a clause about a member it does not mark states the value the request carried.
///
/// The rows differ along the member, its operation, the constraint class and the bound —
/// including a member the issue that filed this rule did not name (`nextToken`) — so a rule
/// keyed to one member or one operation fails here.
#[tokio::test]
async fn a_sensitive_members_value_is_withheld_from_its_clause() {
    let router = super::validation_envelope::emulator();

    // (case, route, body, the clause the refusal states)
    let rows: Vec<(&str, &str, Value, String)> = vec![
        (
            "searchString, @sensitive, past its @length ceiling",
            "/Users/SearchUsers",
            json!({
                "SearchString": "s".repeat(SEARCH_STRING_CEILING + 1),
                "SearchAttributes": ["Cn"],
            }),
            one_withheld_violation(
                "searchString",
                &format!("Member must have length less than or equal to {SEARCH_STRING_CEILING}"),
            ),
        ),
        (
            "the same member reached through SearchGroups",
            "/Groups/SearchGroups",
            json!({
                "SearchString": "s".repeat(SEARCH_STRING_CEILING * 3),
                "SearchAttributes": ["Cn"],
            }),
            one_withheld_violation(
                "searchString",
                &format!("Member must have length less than or equal to {SEARCH_STRING_CEILING}"),
            ),
        ),
        (
            "givenName, @sensitive, past its @length ceiling",
            "/Users/CreateUser",
            json!({"SAMAccountName": "jdoe", "GivenName": "g".repeat(GIVEN_NAME_CEILING + 1)}),
            one_withheld_violation(
                "givenName",
                &format!("Member must have length less than or equal to {GIVEN_NAME_CEILING}"),
            ),
        ),
        (
            "surname, @sensitive, through UpdateUser",
            "/Users/UpdateUser",
            json!({"SAMAccountName": "jdoe", "Surname": "s".repeat(GIVEN_NAME_CEILING + 1)}),
            one_withheld_violation(
                "surname",
                &format!("Member must have length less than or equal to {GIVEN_NAME_CEILING}"),
            ),
        ),
        (
            "emailAddress, @sensitive, past its own @length ceiling",
            "/Users/CreateUser",
            json!({
                "SAMAccountName": "jdoe",
                "EmailAddress": format!("{}@corp.example.com", "e".repeat(EMAIL_ADDRESS_CEILING)),
            }),
            one_withheld_violation(
                "emailAddress",
                &format!("Member must have length less than or equal to {EMAIL_ADDRESS_CEILING}"),
            ),
        ),
        (
            "nextToken, @sensitive, past its @length ceiling",
            "/Users/ListUsers",
            json!({"NextToken": "t".repeat(NEXT_TOKEN_CEILING + 1)}),
            one_withheld_violation(
                "nextToken",
                &format!("Member must have length less than or equal to {NEXT_TOKEN_CEILING}"),
            ),
        ),
        (
            "sAMAccountName, NOT @sensitive, past its @length ceiling",
            "/Users/CreateUser",
            json!({"SAMAccountName": "u".repeat(USER_NAME_CEILING + 1)}),
            one_violation(
                &format!("'{}'", "u".repeat(USER_NAME_CEILING + 1)),
                "sAMAccountName",
                &format!("Member must have length less than or equal to {USER_NAME_CEILING}"),
            ),
        ),
        (
            "realm, NOT @sensitive, outside its @pattern",
            "/Groups/ListGroups",
            json!({"Realm": "not a realm!"}),
            one_violation(
                "'not a realm!'",
                "realm",
                &format!("Member must satisfy regular expression pattern: {REALM_PATTERN}"),
            ),
        ),
    ];

    for (case, route, body, expected) in rows {
        let refused = call(&router, route, DIRECTORY, &body).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.message(),
            Some(expected.as_str()),
            "case {case:?}: expected Message {expected:?}, got {:?}",
            refused.body
        );
    }
}

/// The decision is made per MEMBER inside ONE message: a request breaking a `@sensitive`
/// member's constraint and a non-sensitive member's echoes one value and withholds the
/// other.
#[tokio::test]
async fn one_message_withholds_one_value_and_echoes_the_other() {
    let router = super::validation_envelope::emulator();

    let name = "u".repeat(USER_NAME_CEILING + 1);
    let given = "g".repeat(GIVEN_NAME_CEILING + 1);
    let refused = call(
        &router,
        "/Users/CreateUser",
        DIRECTORY,
        &json!({"SAMAccountName": name, "GivenName": given}),
    )
    .await;

    let expected = two_violations(
        &format!(
            "Value at 'givenName' failed to satisfy constraint: Member must have length less \
             than or equal to {GIVEN_NAME_CEILING}"
        ),
        &format!(
            "Value '{name}' at 'sAMAccountName' failed to satisfy constraint: Member must have \
             length less than or equal to {USER_NAME_CEILING}"
        ),
    );
    assert_eq!(
        refused.message(),
        Some(expected.as_str()),
        "expected Message {expected:?}, got {:?}",
        refused.body
    );
    assert!(
        !refused.message().unwrap_or_default().contains(&given),
        "the @sensitive value must not be echoed, got {:?}",
        refused.body
    );
}

/// The envelope of a two-clause refusal, in the order the clauses are stated in.
fn two_violations(first: &str, second: &str) -> String {
    format!("2 validation errors detected: {first}; {second}")
}

/// RULE 4: the clauses of one refusal are stated in this service's own fixed order — by the
/// member path each clause is stated at — whichever order the request wrote its members in.
///
/// The rows differ along the pair of members, the operation and the order the body writes
/// them in; each pair is sent BOTH ways round and must read back the one same message.
#[tokio::test]
async fn the_clauses_of_one_refusal_are_stated_in_the_services_fixed_order() {
    let router = super::validation_envelope::emulator();
    let realm_requirement =
        format!("Member must satisfy regular expression pattern: {REALM_PATTERN}");

    // (case, route, the body one way round, the body the other way round, the message)
    let rows: Vec<(&str, &str, Value, Value, String)> = vec![
        (
            "memberRealm and realm, both outside Realm's @pattern",
            "/GroupMemberships/ListGroupMembers",
            json!({
                "SAMAccountName": "engineering",
                "Realm": "not a realm!",
                "MemberRealm": "also not a realm!",
            }),
            json!({
                "SAMAccountName": "engineering",
                "MemberRealm": "also not a realm!",
                "Realm": "not a realm!",
            }),
            two_violations(
                &format!(
                    "Value 'also not a realm!' at 'memberRealm' failed to satisfy constraint: \
                     {realm_requirement}"
                ),
                &format!(
                    "Value 'not a realm!' at 'realm' failed to satisfy constraint: \
                     {realm_requirement}"
                ),
            ),
        ),
        (
            "the same pair through ListGroupsForMember",
            "/GroupMemberships/ListGroupsForMember",
            json!({
                "SAMAccountName": "jdoe",
                "Realm": "corp example com",
                "MemberRealm": "member realm",
            }),
            json!({
                "SAMAccountName": "jdoe",
                "MemberRealm": "member realm",
                "Realm": "corp example com",
            }),
            two_violations(
                &format!(
                    "Value 'member realm' at 'memberRealm' failed to satisfy constraint: \
                     {realm_requirement}"
                ),
                &format!(
                    "Value 'corp example com' at 'realm' failed to satisfy constraint: \
                     {realm_requirement}"
                ),
            ),
        ),
        (
            "realm and sAMAccountName, two different constraint classes",
            "/Users/DescribeUser",
            json!({
                "SAMAccountName": "u".repeat(USER_NAME_CEILING + 1),
                "Realm": "not a realm!",
            }),
            json!({
                "Realm": "not a realm!",
                "SAMAccountName": "u".repeat(USER_NAME_CEILING + 1),
            }),
            two_violations(
                &format!(
                    "Value 'not a realm!' at 'realm' failed to satisfy constraint: \
                     {realm_requirement}"
                ),
                &format!(
                    "Value '{}' at 'sAMAccountName' failed to satisfy constraint: Member must \
                     have length less than or equal to {USER_NAME_CEILING}",
                    "u".repeat(USER_NAME_CEILING + 1)
                ),
            ),
        ),
    ];

    for (case, route, one_way, other_way, expected) in rows {
        for (written, body) in [("as written", one_way), ("the other way round", other_way)] {
            let refused = call(&router, route, DIRECTORY, &body).await;
            assert_eq!(
                refused.message(),
                Some(expected.as_str()),
                "case {case:?} ({written}): expected Message {expected:?}, got {:?}",
                refused.body
            );
        }
    }
}
