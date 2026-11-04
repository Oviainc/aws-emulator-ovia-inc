// SPDX-License-Identifier: CC0-1.0

//! What a refused IP-access-control-group or connection-alias call SAYS, over the
//! awsJson1_1 wire.
//!
//! The rule this suite pins:
//!
//! > a refusal's `message` is the sentence this service fixes for that ERROR CLASS
//! > and resource kind — never the error SHAPE's documentation sentence, and never
//! > a value this emulator generated. Its fixed part is identical for every request
//! > in the class, its only variable part is taken from the request, and the
//! > identity of a resource that could not be found travels in
//! > `ResourceNotFoundException`'s own `ResourceId` member instead.
//!
//! Three classes are covered, so each table varies what the rule generalises over
//! (the operation, the identifier's form and length, the name's form, size and
//! script, how the resource came to be absent):
//!
//! * a connection alias this service does not hold — one request-independent
//!   sentence, whatever `ConnectionAliasId` the caller named and however the alias
//!   came to be gone;
//! * an IP access control group this service does not hold — one
//!   request-independent sentence, the same one for every operation that resolves a
//!   `GroupId`;
//! * a `GroupName` another stored group already holds — a fixed prefix plus the
//!   name the request asked for, verbatim, and nothing else: in particular not the
//!   `GroupId` of the group that holds the name, which is a value this emulator
//!   minted rather than one the caller sent.
//!
//! Every case drives a real `MultiServiceRouter` with this service mounted on one
//! shared engine (the harness is [`crate::workspace_lifecycle`]'s), so each
//! assertion is about what a client would see.

use serde_json::{json, Value};
use service_com_amazonaws_workspaces_workspacesservice::state::{
    message_ip_group_name_taken, MAX_IP_GROUP_TEXT_CHARS, MESSAGE_CONNECTION_ALIAS_NOT_FOUND,
    MESSAGE_IP_GROUP_NOT_FOUND,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::account_links::error_code;
use crate::workspace_lifecycle::{call, emulator_with_engine, settle};

/// The `message` member of an error body, as the JSON protocols carry it.
fn error_message(body: &Value) -> String {
    body["message"]
        .as_str()
        .or_else(|| body["Message"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// The generic sentence the model documents for the error SHAPES used here
/// (`ResourceNotFoundException` and `ResourceAlreadyExistsException` are both
/// documented "The resource could not be found." / "The specified resource already
/// exists."). A shape's documentation is not the wire message of any one refusal,
/// so no case below may answer it.
const SHAPE_DOC_SENTENCES: [&str; 2] = [
    "The resource could not be found.",
    "The specified resource already exists.",
];

/// The fixed part of the duplicate-group-name refusal, up to and including the
/// final separator; the one variable part that follows it is the requested
/// `GroupName`.
const NAME_TAKEN_PREFIX: &str =
    "Encountered an exception while creating IP Group - group already exists for given group name - ";

/// Assert a refusal answered HTTP 400 with `code`, and return its message.
fn refusal_message(case: &str, code: &str, status: u16, body: &Value) -> String {
    assert_eq!(
        status, 400,
        "case {case:?}: the refusal is an HTTP 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(body),
        code,
        "case {case:?}: the refusal is a {code}, got {body}"
    );
    let message = error_message(body);
    for doc_sentence in SHAPE_DOC_SENTENCES {
        assert_ne!(
            message, doc_sentence,
            "case {case:?}: the message must be the sentence for this error class, \
             not the error shape's documentation, got {message:?}"
        );
    }
    message
}

/// Create one IP access control group and return the `GroupId` it minted.
async fn create_group(router: &MultiServiceRouter, request: &Value) -> String {
    let (status, body) = call(router, "CreateIpGroup", request).await;
    assert_eq!(
        status, 200,
        "case {request}: CreateIpGroup must answer 200, got {status} with {body}"
    );
    body["GroupId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {request}: a minted GroupId, got {body}"))
        .to_string()
}

/// Create one connection alias and return the `AliasId` it minted.
async fn create_alias(router: &MultiServiceRouter, connection_string: &str) -> String {
    let (status, body) = call(
        router,
        "CreateConnectionAlias",
        &json!({ "ConnectionString": connection_string }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {connection_string:?}: CreateConnectionAlias must answer 200, got {status} with {body}"
    );
    body["AliasId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {connection_string:?}: a minted AliasId, got {body}"))
        .to_string()
}

/// The sentences the corpus recorded, pinned as literals here so the constants the
/// handlers serve cannot drift away from them unnoticed. The rule the other tests
/// assert is that the text belongs to the CLASS; these rows keep the recorded
/// examples anchored.
#[test]
fn the_class_sentences_are_the_recorded_ones() {
    assert_eq!(
        MESSAGE_CONNECTION_ALIAS_NOT_FOUND,
        "The connection alias couldn\u{2019}t be found. Make sure that you have the \
         correct connection alias, and then try again.",
        "case \"a connection alias that is not held\": the class sentence must stay the \
         recorded one, got {MESSAGE_CONNECTION_ALIAS_NOT_FOUND:?}"
    );
    // The apostrophe is the typographic U+2019 the recording carries, not an ASCII
    // one: a client comparing the text byte for byte sees the difference.
    assert!(
        MESSAGE_CONNECTION_ALIAS_NOT_FOUND.contains('\u{2019}')
            && !MESSAGE_CONNECTION_ALIAS_NOT_FOUND.contains('\''),
        "case \"the apostrophe\": the sentence spells it U+2019, got \
         {MESSAGE_CONNECTION_ALIAS_NOT_FOUND:?}"
    );
    assert_eq!(
        MESSAGE_IP_GROUP_NOT_FOUND, "Ip Group Id is not found.",
        "case \"an IP group that is not held\": the class sentence must stay the recorded \
         one, got {MESSAGE_IP_GROUP_NOT_FOUND:?}"
    );
    // One row per shape of name, to show the template is prefix + the name and
    // nothing else.
    for (case, group_name) in [
        ("a plain name", "sim2real-wsipg-dup"),
        ("a name with spaces", "office network"),
        ("a name that looks like an id", "wsipg-ffffffff"),
    ] {
        let expected = format!("{NAME_TAKEN_PREFIX}{group_name}");
        let actual = message_ip_group_name_taken(group_name);
        assert_eq!(
            actual, expected,
            "case {case:?}: the duplicate-name sentence is the fixed prefix plus the \
             requested name, got {actual:?}"
        );
    }
}

/// Every connection alias the not-found table asks about, with how it came to be
/// absent.
///
/// The ids the service never minted span what
/// `smithy_query(query="operation", operation="DeleteConnectionAlias")` admits for
/// `AliasId` — `len 13..68`, `pattern ^wsca-[0-9a-z]{8,63}$` — at both published
/// bounds and in between, with digits, letters and a mix.
fn absent_alias_ids() -> Vec<(String, String)> {
    vec![
        (
            "the shortest id the model admits".to_string(),
            "wsca-00000000".to_string(),
        ),
        (
            "the longest id the model admits".to_string(),
            format!("wsca-{}", "z".repeat(63)),
        ),
        (
            "digits and letters".to_string(),
            "wsca-0a1b2c3d4e5f".to_string(),
        ),
        (
            "an id this emulator never minted".to_string(),
            "wsca-deadbeef99".to_string(),
        ),
    ]
}

#[tokio::test]
async fn a_connection_alias_that_is_not_held_is_refused_with_the_class_sentence() {
    let (router, engine) = emulator_with_engine();

    let mut cases = absent_alias_ids();

    // An alias that was created and then deleted is absent for a second reason, and
    // is refused with exactly the same sentence: the text is the class's, not the
    // request's history.
    let deleted = create_alias(&router, "deleted.example.com").await;
    let (status, body) = call(
        &router,
        "DeleteConnectionAlias",
        &json!({ "AliasId": deleted }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"deleting a stored alias\": must answer 200, got {status} with {body}"
    );
    settle(&engine);
    cases.push(("an alias already deleted".to_string(), deleted));

    for (case, alias_id) in cases {
        let (status, body) = call(
            &router,
            "DeleteConnectionAlias",
            &json!({ "AliasId": alias_id }),
        )
        .await;
        let message = refusal_message(&case, "ResourceNotFoundException", status, &body);
        assert_eq!(
            message, MESSAGE_CONNECTION_ALIAS_NOT_FOUND,
            "case {case:?}: every alias this service does not hold is refused with the \
             one class sentence, got {message:?}"
        );
        // Request-independent: the identifier the caller sent reaches the shape's
        // `ResourceId` member, never the prose.
        assert!(
            !message.contains(&alias_id),
            "case {case:?}: the message must not carry the requested AliasId, got {message:?}"
        );
        assert_eq!(
            body["ResourceId"].as_str(),
            Some(alias_id.as_str()),
            "case {case:?}: ResourceId names the alias the caller asked for, got {body}"
        );
    }
}

/// Every request a rule-or-delete operation can make about a `GroupId`, so one
/// absent group is asked about through each operation that resolves one.
fn ip_group_requests(group_id: &str) -> Vec<(&'static str, Value)> {
    vec![
        ("DeleteIpGroup", json!({ "GroupId": group_id })),
        (
            "AuthorizeIpRules",
            json!({ "GroupId": group_id, "UserRules": [{ "ipRule": "192.0.2.0/24" }] }),
        ),
        (
            "RevokeIpRules",
            json!({ "GroupId": group_id, "UserRules": ["192.0.2.0/24"] }),
        ),
        (
            "UpdateRulesOfIpGroup",
            json!({ "GroupId": group_id, "UserRules": [{ "ipRule": "198.51.100.0/24" }] }),
        ),
    ]
}

#[tokio::test]
async fn an_ip_group_that_is_not_held_is_refused_with_the_class_sentence() {
    let (router, _engine) = emulator_with_engine();

    // A stored group no case names, to show a refusal of another id changes nothing
    // about it.
    let bystander = create_group(
        &router,
        &json!({
            "GroupName": "grp-bystander",
            "GroupDesc": "the group no case names",
            "UserRules": [{ "ipRule": "10.0.0.0/16", "ruleDesc": "office" }],
        }),
    )
    .await;

    // Ids the service never minted, spanning what `IpGroupId`'s
    // `pattern ^wsipg-[0-9a-z]{8,63}$` admits (shortest suffix, longest suffix, a
    // mix of digits and letters), plus a group that WAS stored and was deleted.
    let deleted = create_group(&router, &json!({ "GroupName": "grp-doomed" })).await;
    let (status, body) = call(&router, "DeleteIpGroup", &json!({ "GroupId": deleted })).await;
    assert_eq!(
        status, 200,
        "case \"deleting a stored group\": must answer 200, got {status} with {body}"
    );

    let absent = vec![
        ("the shortest suffix".to_string(), "wsipg-00000000".to_string()),
        (
            "the longest suffix".to_string(),
            format!("wsipg-{}", "a".repeat(63)),
        ),
        (
            "digits and letters".to_string(),
            "wsipg-deadbeef99".to_string(),
        ),
        ("a group already deleted".to_string(), deleted),
    ];

    for (how, group_id) in &absent {
        for (operation, request) in ip_group_requests(group_id) {
            let case = format!("{operation} on {how} ({group_id})");
            let (status, body) = call(&router, operation, &request).await;
            let message = refusal_message(&case, "ResourceNotFoundException", status, &body);
            assert_eq!(
                message, MESSAGE_IP_GROUP_NOT_FOUND,
                "case {case:?}: every operation that resolves a GroupId refuses an absent \
                 group with the one class sentence, got {message:?}"
            );
            assert!(
                !message.contains(group_id.as_str()),
                "case {case:?}: the message must not carry the requested GroupId, got \
                 {message:?}"
            );
            assert_eq!(
                body["ResourceId"].as_str(),
                Some(group_id.as_str()),
                "case {case:?}: ResourceId names the group the caller asked for, got {body}"
            );
        }
    }

    // None of those refusals touched the group that IS stored.
    let (status, body) = call(
        &router,
        "DescribeIpGroups",
        &json!({ "GroupIds": [bystander.clone()] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the bystander survives\": DescribeIpGroups must answer 200, got {status} with {body}"
    );
    assert_eq!(
        body["Result"],
        json!([{
            "groupId": bystander,
            "groupName": "grp-bystander",
            "groupDesc": "the group no case names",
            "userRules": [{ "ipRule": "10.0.0.0/16", "ruleDesc": "office" }],
        }]),
        "case \"the bystander survives\": the stored group is exactly as it was, got {body}"
    );
}

/// Every `GroupName` the duplicate-name table takes, varying what the rule
/// generalises over: length (one character, and the longest this emulator stores),
/// script, spacing, punctuation — `IpGroupName` carries neither a `@length` nor a
/// `@pattern` trait (from
/// `smithy_query(query="shape", shape="IpGroupName")`), so any of these is a name a
/// client can send.
fn duplicate_name_table() -> Vec<(&'static str, String)> {
    vec![
        ("a plain name", "grp-plain".to_string()),
        ("one character", "x".to_string()),
        ("mixed case", "Office-NETWORK-02".to_string()),
        ("spaces and punctuation", "office network (eu), floor 3".to_string()),
        (
            "the separator the message itself uses",
            "group - already - here".to_string(),
        ),
        ("non-ASCII", "bürö-東京".to_string()),
        ("a name shaped like a group id", "wsipg-ffffffff".to_string()),
        (
            "the longest name this emulator stores",
            "n".repeat(MAX_IP_GROUP_TEXT_CHARS),
        ),
    ]
}

#[tokio::test]
async fn a_duplicate_group_name_is_refused_with_the_requested_name_and_no_minted_id() {
    let (router, _engine) = emulator_with_engine();

    for (case, group_name) in duplicate_name_table() {
        let holder = create_group(
            &router,
            &json!({ "GroupName": group_name, "GroupDesc": "the group that holds the name" }),
        )
        .await;

        // The second create differs from the first in everything but the name, so
        // nothing but the name can be what the refusal reports.
        let (status, body) = call(
            &router,
            "CreateIpGroup",
            &json!({
                "GroupName": group_name,
                "GroupDesc": "a different description",
                "UserRules": [{ "ipRule": "203.0.113.0/24" }],
            }),
        )
        .await;
        let message = refusal_message(case, "ResourceAlreadyExistsException", status, &body);
        assert_eq!(
            message,
            format!("{NAME_TAKEN_PREFIX}{group_name}"),
            "case {case:?}: the refusal is the fixed prefix plus the requested GroupName, \
             got {message:?}"
        );
        assert!(
            !message.contains(holder.as_str()),
            "case {case:?}: the message must not carry the minted GroupId {holder:?} of the \
             group that holds the name, got {message:?}"
        );
        assert!(
            !message.ends_with('.'),
            "case {case:?}: the class sentence ends with the requested name, not a period, \
             got {message:?}"
        );

        // Exactly one group still holds the name, and it is the first one.
        let (status, body) = call(&router, "DescribeIpGroups", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeIpGroups must answer 200, got {status} with {body}"
        );
        let holding: Vec<&Value> = body["Result"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: a Result list, got {body}"))
            .iter()
            .filter(|group| group["groupName"] == json!(group_name))
            .collect();
        assert_eq!(
            holding.len(),
            1,
            "case {case:?}: exactly one stored group holds the name, got {holding:?}"
        );
        assert_eq!(
            holding[0]["groupId"],
            json!(holder),
            "case {case:?}: the group holding the name is the first one created, got \
             {holding:?}"
        );

        // Leave the registry empty for the next row, so each case is judged on the
        // groups it created itself.
        let (status, body) = call(&router, "DeleteIpGroup", &json!({ "GroupId": holder })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: deleting the holding group must answer 200, got {status} with {body}"
        );
    }
}
