// SPDX-License-Identifier: CC0-1.0

//! The HTTP status this service answers a modelled error with, end to end over the wire.
//!
//! Every case drives the real request path — `MultiServiceRouter::dispatch` with `POST /`,
//! `X-Amz-Target: OpenSearchServerless.<Operation>` and an `awsJson1_0` body (the wire
//! contract in this crate's `OPERATIONS.md`) — so the protocol codec and the handler both
//! take part, exactly as they do for an SDK client. The harness is the one
//! `collection_lifecycle` owns; nothing is rebuilt here.
//!
//! The rule under test: on this `awsJson1_0` service the response status is not how a
//! client discriminates an error — the body's `__type` is — so EVERY modelled client error
//! is serialized with one status, [`CLIENT_ERROR`] (HTTP 400), whatever `@httpError` its
//! shape carries, while the body still names the error's own shape. A non-client answer
//! keeps its own status, so the collapse is about the client-error class and not about
//! every failure.
//!
//! The tables span what the rule generalises over: the whole modelled client-error catalog
//! (`ConflictException` 409, `ResourceNotFoundException` 404, `ServiceQuotaExceededException`
//! 402, `ValidationException` 400), the operations that raise each one — creates, reads,
//! deletes, a write and a quota — and both the request-time and the state-time refusals, not
//! the two operations the issue quoted.

use serde_json::{json, Value};
use service_com_amazonaws_opensearchserverless_opensearchserverless::state::MAX_SECURITY_POLICIES_PER_TYPE;
use smithy_server_runtime::MultiServiceRouter;

use crate::collection_lifecycle::{at, call, emulator, text, CLIENT_ERROR};

/// Every client error this service models, with the `@httpError` status its own shape
/// carries, read off
/// `smithy_query(query="shape", shape="ConflictException,ResourceNotFoundException,ValidationException,ServiceQuotaExceededException,OcuLimitExceededException")`
/// — each reported `errorKind: "client"`. The rule under test is that the answer is
/// [`CLIENT_ERROR`] whatever the second column says.
const MODELED_CLIENT_ERRORS: [(&str, u16); 5] = [
    ("ConflictException", 409),
    ("OcuLimitExceededException", 402),
    ("ResourceNotFoundException", 404),
    ("ServiceQuotaExceededException", 402),
    ("ValidationException", 400),
];

/// The `@httpError` status `code`'s shape carries, from [`MODELED_CLIENT_ERRORS`].
fn modeled_status(code: &str) -> u16 {
    MODELED_CLIENT_ERRORS
        .iter()
        .find(|(name, _)| *name == code)
        .map(|(_, status)| *status)
        .unwrap_or_else(|| panic!("{code} is not one of this service's modelled client errors"))
}

/// A well-formed `PolicyVersion` nothing ever minted: inside `@length(min: 20, max: 36)`
/// and matching the modelled `^([0-9a-zA-Z+/]{4})*(([0-9a-zA-Z+/]{2}==)|([0-9a-zA-Z+/]{3}=))?$`.
const UNMINTED_VERSION: &str = "MTAwMDAwMDAwMDAwMDAwMA==";

/// An `encryption` / `data` / `retention` policy document the handlers accept.
const EMPTY_RULES: &str = r#"{"Rules":[]}"#;

/// A `network` policy document the handler accepts.
const NETWORK_RULES: &str = r#"{"Rules":[],"AllowFromPublic":true}"#;

/// A `retention` lifecycle document at the documented 24h minimum window.
const RETENTION_RULES: &str =
    r#"{"Rules":[{"ResourceType":"index","Resource":["index/demo/*"],"MinIndexRetention":"24h"}]}"#;

/// One row: what the case is, the call that raises the error, and the error shape the body
/// must name.
struct Row {
    what: &'static str,
    operation: &'static str,
    request: Value,
    code: &'static str,
}

/// Call `operation` with `body`, asserting it was accepted.
async fn accepted(router: &MultiServiceRouter, operation: &str, body: Value) -> Value {
    let case = body.to_string();
    let (status, code, answer) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {operation}({case}): expected 200, got {status} {code} with {answer}"
    );
    answer
}

/// Assert `row` is answered with [`CLIENT_ERROR`] and a body naming `row.code`.
async fn assert_client_error(router: &MultiServiceRouter, row: &Row) -> Value {
    let modeled = modeled_status(row.code);
    let (status, code, answer) = call(router, row.operation, row.request.clone()).await;
    assert_eq!(
        (status, code.as_str()),
        (CLIENT_ERROR, row.code),
        "case {} ({}): expected {CLIENT_ERROR} {} \
         (the shape's @httpError says {modeled}), got {status} {code} with {answer}",
        row.what,
        row.operation,
        row.code
    );
    answer
}

// ---------------------------------------------------------------------------
// The whole modelled client-error catalog, over the operations that raise it.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_modeled_client_error_answers_the_single_client_error_status() {
    let router = emulator();

    // The stored records the conflict rows collide with, and the group the delete refusal
    // needs non-empty.
    accepted(
        &router,
        "CreateAccessPolicy",
        json!({ "type": "data", "name": "stored-access", "policy": EMPTY_RULES }),
    )
    .await;
    accepted(
        &router,
        "CreateSecurityPolicy",
        json!({ "type": "encryption", "name": "stored-encryption", "policy": EMPTY_RULES }),
    )
    .await;
    accepted(
        &router,
        "CreateLifecyclePolicy",
        json!({ "type": "retention", "name": "stored-retention", "policy": RETENTION_RULES }),
    )
    .await;
    let group = accepted(
        &router,
        "CreateCollectionGroup",
        json!({ "name": "stored-group", "standbyReplicas": "ENABLED" }),
    )
    .await;
    let group_id = text(at(&group, "createCollectionGroupDetail"), "id");
    accepted(
        &router,
        "CreateCollection",
        // A collection carries its own encryption setting: one has to come from the
        // request or from a covering encryption security policy, and this suite is about
        // the refusal STATUS, not about where that setting came from.
        json!({
            "name": "stored-collection",
            "collectionGroupName": "stored-group",
            "encryptionConfig": { "aWSOwnedKey": true },
        }),
    )
    .await;

    // The id a security configuration of a type and name carries is `<type>/<account>/<name>`,
    // so this one is well-formed and names nothing.
    let absent_config_id = format!("saml/{}/never-created", engine::emulator_account_id());

    let rows: Vec<Row> = vec![
        Row {
            what: "a data access policy pair already stored",
            operation: "CreateAccessPolicy",
            request: json!({ "type": "data", "name": "stored-access", "policy": EMPTY_RULES }),
            code: "ConflictException",
        },
        Row {
            what: "a collection name already taken",
            operation: "CreateCollection",
            request: json!({
                "name": "stored-collection",
                "encryptionConfig": { "aWSOwnedKey": true },
            }),
            code: "ConflictException",
        },
        Row {
            what: "a collection group name already taken",
            operation: "CreateCollectionGroup",
            request: json!({ "name": "stored-group", "standbyReplicas": "DISABLED" }),
            code: "ConflictException",
        },
        Row {
            what: "a security policy pair already stored",
            operation: "CreateSecurityPolicy",
            request: json!({ "type": "encryption", "name": "stored-encryption", "policy": EMPTY_RULES }),
            code: "ConflictException",
        },
        Row {
            what: "a lifecycle policy pair already stored",
            operation: "CreateLifecyclePolicy",
            request: json!({ "type": "retention", "name": "stored-retention", "policy": RETENTION_RULES }),
            code: "ConflictException",
        },
        Row {
            what: "a collection group that still holds a collection",
            operation: "DeleteCollectionGroup",
            request: json!({ "id": group_id.clone() }),
            code: "ValidationException",
        },
        Row {
            what: "a write carrying a policyVersion nothing minted",
            operation: "UpdateAccessPolicy",
            request: json!({
                "type": "data",
                "name": "stored-access",
                "policyVersion": UNMINTED_VERSION,
                "description": "this write must not land",
            }),
            code: "ValidationException",
        },
        Row {
            what: "a read of an access policy pair never created",
            operation: "GetAccessPolicy",
            request: json!({ "type": "data", "name": "never-created" }),
            code: "ResourceNotFoundException",
        },
        Row {
            what: "a read of an encryption policy pair never created",
            operation: "GetSecurityPolicy",
            request: json!({ "type": "encryption", "name": "never-created" }),
            code: "ResourceNotFoundException",
        },
        Row {
            what: "a read of a network policy pair never created",
            operation: "GetSecurityPolicy",
            request: json!({ "type": "network", "name": "never-created" }),
            code: "ResourceNotFoundException",
        },
        Row {
            what: "a read of a security configuration id that names nothing",
            operation: "GetSecurityConfig",
            request: json!({ "id": absent_config_id }),
            code: "ResourceNotFoundException",
        },
        Row {
            what: "a delete of a collection id that names nothing",
            operation: "DeleteCollection",
            request: json!({ "id": "aaaaaaaaaaaaaaaaaaaaaaaa" }),
            code: "ResourceNotFoundException",
        },
        Row {
            what: "a delete of an access policy pair never created",
            operation: "DeleteAccessPolicy",
            request: json!({ "type": "data", "name": "never-created" }),
            code: "ResourceNotFoundException",
        },
        Row {
            what: "a delete of a security policy pair never created",
            operation: "DeleteSecurityPolicy",
            request: json!({ "type": "network", "name": "never-created" }),
            code: "ResourceNotFoundException",
        },
        Row {
            what: "a policy type outside the modelled value list",
            operation: "GetAccessPolicy",
            request: json!({ "type": "Data", "name": "stored-access" }),
            code: "ValidationException",
        },
        Row {
            what: "a collection name one short of the modelled minimum",
            operation: "CreateCollection",
            request: json!({ "name": "ab" }),
            code: "ValidationException",
        },
        Row {
            what: "a collection id outside the modelled pattern",
            operation: "DeleteCollection",
            request: json!({ "id": "NOT-AN-ID" }),
            code: "ValidationException",
        },
    ];

    for row in &rows {
        let answer = assert_client_error(&router, row).await;
        // The status carries no information now, so the body must: the error names its own
        // shape, and says something about the request.
        assert!(
            at(&answer, "__type").is_string(),
            "case {} ({}): the body must name the error shape, got {answer}",
            row.what,
            row.operation
        );
        assert!(
            !text(&answer, "message").is_empty(),
            "case {} ({}): the error must carry a message, got {answer}",
            row.what,
            row.operation
        );
    }

    // Every row above was a refusal, so the stored records are untouched: the collection
    // group the delete refused is still readable, with its collection still counted.
    let read = accepted(
        &router,
        "BatchGetCollectionGroup",
        json!({ "ids": [group_id] }),
    )
    .await;
    assert_eq!(
        at(&read, "collectionGroupDetails")
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        1,
        "case after the refusals: the refused delete left the group stored, got {read}"
    );
}

// ---------------------------------------------------------------------------
// The two client errors a refusal can only reach through stored state: the
// transient-status conflict, and the per-type quota.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_state_time_client_errors_answer_it_too() {
    let router = emulator();

    // A collection is returned in `CREATING` and reaches `ACTIVE` on its own a moment
    // later, so the delete refusal is observable to a caller acting straight after the
    // create. `ConflictException`'s own documentation: "When deleting a resource, thrown
    // when the resource is not in the ACTIVE, FAILED, or UPDATE_FAILED state."
    let created = accepted(
        &router,
        "CreateCollection",
        json!({
            "name": "transient-collection",
            "encryptionConfig": { "aWSOwnedKey": true },
        }),
    )
    .await;
    let id = text(at(&created, "createCollectionDetail"), "id");
    assert_client_error(
        &router,
        &Row {
            what: "a delete of a collection still in CREATING",
            operation: "DeleteCollection",
            request: json!({ "id": id }),
            code: "ConflictException",
        },
    )
    .await;

    // The per-type security-policy quota: creates are accepted up to it, and the one past
    // it is the quota error — whose `@httpError` is 402, a status the issue never recorded
    // and which this service still answers as a plain client error.
    for index in 0..MAX_SECURITY_POLICIES_PER_TYPE {
        accepted(
            &router,
            "CreateSecurityPolicy",
            json!({ "type": "network", "name": format!("quota-policy-{index}"), "policy": NETWORK_RULES }),
        )
        .await;
    }
    let answer = assert_client_error(
        &router,
        &Row {
            what: "a create one past the per-type security policy quota",
            operation: "CreateSecurityPolicy",
            request: json!({ "type": "network", "name": "quota-policy-too-many", "policy": NETWORK_RULES }),
            code: "ServiceQuotaExceededException",
        },
    )
    .await;
    assert!(
        !text(&answer, "message").is_empty(),
        "case one past the quota: the quota error carries its required message, got {answer}"
    );
}

// ---------------------------------------------------------------------------
// The collapse is about the CLIENT-error class: an answer that is not one keeps
// its own status.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_answer_that_is_not_a_client_error_keeps_its_own_status() {
    let router = emulator();

    // A scaffolded operation answers the framework's 501, not a client error: collapsing
    // it would tell a client its request was at fault. No handler in this crate carries
    // the scaffold marker any more — `ListCollectionGroups` was the last, and it now
    // reads the stored collection groups — so the operation that once answered 501 is
    // checked here to be served rather than refused.
    let (status, code, answer) = call(&router, "ListCollectionGroups", json!({})).await;
    assert_eq!(
        (status, code.as_str()),
        (200, ""),
        "case ListCollectionGroups (implemented): expected 200, got {status} {code} with {answer}"
    );

    // And a request this service does not model at all is the protocol's own routing
    // answer, which is not a modelled client error either.
    let (status, _, answer) = call(&router, "NoSuchOperation", json!({})).await;
    assert_ne!(
        status, CLIENT_ERROR,
        "case NoSuchOperation: an unrouted request is not a modelled client error, got {status} with {answer}"
    );
}

// ---------------------------------------------------------------------------
// What a boundary refusal SAYS: the clause names the member, the value and the
// constraint — with a `@pattern` reported the way the service reports it.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_pattern_refusal_states_the_regex_without_the_traits_anchors() {
    let router = emulator();
    accepted(
        &router,
        "CreateAccessPolicy",
        json!({ "type": "data", "name": "stored-access", "policy": EMPTY_RULES }),
    )
    .await;

    // (case, operation, request, member, the offending value, the regex the clause states)
    //
    // The third column of each row is the member's `@pattern` with its `^`/`$` anchors
    // dropped: `smithy_query(query="shape", shape="CollectionGroupName,PolicyName,PolicyVersion")`
    // gives `^[a-z][a-z0-9-]+$` for the two name shapes and the base64 pattern below for
    // `PolicyVersion`. The rows span three shapes and two members, so the rule is about a
    // pattern clause and not about one member. A member typed `CollectionName` reports the
    // class its own validator applies instead, which `tests/all/reported_pattern.rs` owns.
    let rows: [(&str, &str, Value, &str, &str, &str); 4] = [
        (
            "a collection group name beside a collection's own name",
            "CreateCollection",
            json!({ "name": "valid-collection", "collectionGroupName": "1invalid-group" }),
            "collectionGroupName",
            "1invalid-group",
            "[a-z][a-z0-9-]+",
        ),
        (
            "a collection group name outside CollectionGroupName",
            "CreateCollectionGroup",
            json!({ "name": "1cd", "standbyReplicas": "ENABLED" }),
            "name",
            "1cd",
            "[a-z][a-z0-9-]+",
        ),
        (
            "a policy name outside PolicyName",
            "CreateAccessPolicy",
            json!({ "type": "data", "name": "1bad-policy", "policy": EMPTY_RULES }),
            "name",
            "1bad-policy",
            "[a-z][a-z0-9-]+",
        ),
        (
            "a policyVersion outside PolicyVersion",
            "UpdateAccessPolicy",
            json!({
                "type": "data",
                "name": "stored-access",
                "policyVersion": "AAAAAAAAAAAAAAAAAAAA==",
                "description": "this write must not land",
            }),
            "policyVersion",
            "AAAAAAAAAAAAAAAAAAAA==",
            "([0-9a-zA-Z+/]{4})*(([0-9a-zA-Z+/]{2}==)|([0-9a-zA-Z+/]{3}=))?",
        ),
    ];

    for (what, operation, request, member, value, pattern) in rows {
        let (status, code, answer) = call(&router, operation, request).await;
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_ERROR, "ValidationException"),
            "case {what}: expected {CLIENT_ERROR} ValidationException, got {status} {code} with {answer}"
        );
        assert_eq!(
            text(&answer, "message"),
            format!(
                "1 validation error detected: Value '{value}' at '{member}' failed to satisfy \
                 constraint: Member must satisfy regular expression pattern: {pattern}"
            ),
            "case {what}: the clause must state the pattern without its anchors, got {answer}"
        );
    }

    // A refusal that is not a pattern failure keeps the framework's own clause, so only
    // the pattern rendering is this service's own.
    for (what, request, clause) in [
        (
            "a name one short of CollectionName's minimum",
            json!({ "name": "ab" }),
            "Value 'ab' at 'name' failed to satisfy constraint: Member must have length greater than or equal to 3",
        ),
        (
            "a required member absent",
            json!({ "description": "no name" }),
            "Value null at 'name' failed to satisfy constraint: Member must not be null",
        ),
    ] {
        let (status, code, answer) = call(&router, "CreateCollection", request).await;
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_ERROR, "ValidationException"),
            "case {what}: expected {CLIENT_ERROR} ValidationException, got {status} {code} with {answer}"
        );
        assert_eq!(
            text(&answer, "message"),
            format!("1 validation error detected: {clause}"),
            "case {what}: the framework's clause is kept, got {answer}"
        );
    }
}


#[tokio::test]
async fn a_conflict_names_the_requested_name_and_no_minted_id() {
    let router = emulator();

    // Rows differ in the `name` the request carries, along `CollectionName`'s
    // @length(min: 3, max: 64) and `^[a-z][a-z0-9-]+$`: the minimum, a mixed
    // letter/digit/dash name, and the maximum.
    for name in ["abc", "a-b-c-1-2-3", &"a".repeat(64)] {
        let created = accepted(
            &router,
            "CreateCollection",
            json!({ "name": name, "encryptionConfig": { "aWSOwnedKey": true } }),
        )
        .await;
        let id = text(at(&created, "createCollectionDetail"), "id");
        let (status, code, answer) = call(
            &router,
            "CreateCollection",
            json!({ "name": name, "encryptionConfig": { "aWSOwnedKey": true } }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_ERROR, "ConflictException"),
            "case CreateCollection({name}, duplicate): expected {CLIENT_ERROR} ConflictException, got {status} {code} with {answer}"
        );
        assert_eq!(
            text(&answer, "message"),
            format!("A collection with name '{name}' already exists."),
            "case CreateCollection({name}, duplicate): the refusal names the requested name, got {answer}"
        );
        assert!(
            !text(&answer, "message").contains(&id),
            "case CreateCollection({name}, duplicate): the refusal must not report the minted id {id}, got {answer}"
        );
    }

    // The same, along `CollectionGroupName`'s @length(min: 3, max: 32) and the same
    // pattern, and over both `standbyReplicas` values so the refusal is about the name.
    for (name, replicas) in [
        ("abc", "ENABLED"),
        ("g-1-2-3", "DISABLED"),
        ("gaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "ENABLED"),
    ] {
        let created = accepted(
            &router,
            "CreateCollectionGroup",
            json!({ "name": name, "standbyReplicas": replicas }),
        )
        .await;
        let id = text(at(&created, "createCollectionGroupDetail"), "id");
        let (status, code, answer) = call(
            &router,
            "CreateCollectionGroup",
            json!({ "name": name, "standbyReplicas": replicas }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_ERROR, "ConflictException"),
            "case CreateCollectionGroup({name}, duplicate): expected {CLIENT_ERROR} ConflictException, got {status} {code} with {answer}"
        );
        assert_eq!(
            text(&answer, "message"),
            format!("A collection group with name '{name}' already exists."),
            "case CreateCollectionGroup({name}, duplicate): the refusal names the requested name, got {answer}"
        );
        assert!(
            !text(&answer, "message").contains(&id),
            "case CreateCollectionGroup({name}, duplicate): the refusal must not report the minted id {id}, got {answer}"
        );
    }

    // The delete refusal states the rule `ConflictException` documents — the statuses a
    // collection may be deleted from — rather than the record it was asked about.
    let created = accepted(
        &router,
        "CreateCollection",
        json!({
            "name": "not-yet-deletable",
            "encryptionConfig": { "aWSOwnedKey": true },
        }),
    )
    .await;
    let id = text(at(&created, "createCollectionDetail"), "id");
    let (status, code, answer) = call(&router, "DeleteCollection", json!({ "id": id.clone() })).await;
    assert_eq!(
        (status, code.as_str()),
        (CLIENT_ERROR, "ConflictException"),
        "case DeleteCollection(still CREATING): expected {CLIENT_ERROR} ConflictException, got {status} {code} with {answer}"
    );
    assert_eq!(
        text(&answer, "message"),
        "Collection cannot be deleted because it is not in 'ACTIVE', 'FAILED', or 'UPDATE_FAILED' status.",
        "case DeleteCollection(still CREATING): the refusal states the documented rule, got {answer}"
    );
    assert!(
        !text(&answer, "message").contains(&id),
        "case DeleteCollection(still CREATING): the refusal must not report the id {id}, got {answer}"
    );
}
