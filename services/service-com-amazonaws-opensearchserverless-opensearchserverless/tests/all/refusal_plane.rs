// SPDX-License-Identifier: CC0-1.0

//! What this service REFUSES, and how: the error class each precondition carries and the
//! sentence it is worded with, end to end over the wire.
//!
//! Every case drives the real request path (`MultiServiceRouter::dispatch` with `POST /`,
//! `X-Amz-Target: OpenSearchServerless.<Operation>` and an `awsJson1_0` body) through the
//! harness `collection_lifecycle` owns, so the boundary validator, the protocol codec and
//! the handler all take part exactly as they do for an SDK client.
//!
//! The rules under test, each asserted over a TABLE rather than over one request:
//!
//! * a refusal's message is a template whose fixed text is the same for every request of
//!   its class and whose variable parts are read from the request and from the stored
//!   record — the collection NAME a deletion-protected delete names, the resource KIND and
//!   id an unresolvable `resourceArn` names, the `id` a security configuration is addressed
//!   by, the capacity members and their float values an impossible range names;
//! * a precondition that never clears by retrying the same request is `ValidationException`
//!   (400), not the `ConflictException` (409) a client retries: a consumed
//!   policy/configuration version, and a collection group that still holds collections;
//! * the tagging plane addresses a RESOURCE, so it resolves every kind of taggable
//!   resource this service stores, not only collections;
//! * a limit past a quota, and a policy document whose statement names an empty required
//!   list, are refused and store nothing.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_opensearchserverless_opensearchserverless::state::MAX_ACCOUNT_CAPACITY_IN_OCU;
use smithy_server_runtime::MultiServiceRouter;

use crate::collection_lifecycle::{at, call, emulator, text, ACCOUNT, CLIENT_ERROR};
use crate::security_configs::{create_config, saml_options};

/// Long enough for a collection's `CREATING -> ACTIVE` transition to fall due; the
/// emulator's span is 500 ms and nothing sleeps for it inside a handler.
const SETTLE_WAIT: Duration = Duration::from_millis(700);

/// A well-formed `PolicyVersion` nothing ever minted: inside `@length(min: 20, max: 36)`
/// and matching `PolicyVersion`'s own
/// `^([0-9a-zA-Z+/]{4})*(([0-9a-zA-Z+/]{2}==)|([0-9a-zA-Z+/]{3}=))?$`.
const UNMINTED_VERSION: &str = "MTAwMDAwMDAwMDAwMDAwMA==";

/// The one sentence every optimistic-lock refusal carries, whichever resource's version
/// was consumed.
const STALE_VERSION_MESSAGE: &str =
    "Policy version specified in the request refers to an older version and policy has since \
     changed";

/// A `retention` lifecycle policy document at the documented 24h minimum window.
const RETENTION_RULES: &str =
    r#"{"Rules":[{"ResourceType":"index","Resource":["index/demo/*"],"MinIndexRetention":"24h"}]}"#;

/// A data access policy document naming one principal, so it is a document the handlers
/// store.
const DATA_POLICY: &str = r#"[{"Rules":[{"ResourceType":"index","Resource":["index/demo/*"],"Permission":["aoss:ReadDocument"]}],"Principal":["arn:aws:iam::000000000000:role/Reader"]}]"#;

/// The `message` an error body carries.
fn message(body: &Value) -> String {
    text(body, "message")
}

/// Create the collection `body` describes, asserting it was accepted, and answer its
/// `createCollectionDetail`.
async fn create_collection(router: &MultiServiceRouter, body: Value) -> Value {
    let case = body.to_string();
    let (status, code, answer) = call(router, "CreateCollection", body).await;
    assert_eq!(
        status, 200,
        "case CreateCollection({case}): expected 200, got {status} {code} with {answer}"
    );
    at(&answer, "createCollectionDetail").clone()
}

/// Create the collection group `body` describes, asserting it was accepted, and answer its
/// `createCollectionGroupDetail`.
async fn create_group(router: &MultiServiceRouter, body: Value) -> Value {
    let case = body.to_string();
    let (status, code, answer) = call(router, "CreateCollectionGroup", body).await;
    assert_eq!(
        status, 200,
        "case CreateCollectionGroup({case}): expected 200, got {status} {code} with {answer}"
    );
    at(&answer, "createCollectionGroupDetail").clone()
}

// ---------------------------------------------------------------------------
// CreateCollection: the encryption setting a collection must have.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_collection_with_no_encryption_setting_names_the_policy_it_needs() {
    let router = emulator();

    // The rows differ along what the rule generalises over: the `name` the refusal's one
    // variable part is taken from — at the modelled minimum length, with digits and
    // hyphens, and long — and the collection `type` beside it, which the rule says nothing
    // about.
    let cases: Vec<(&str, &str, Option<&str>)> = vec![
        ("a name at the modelled minimum length", "abc", None),
        (
            "a name with digits and hyphens",
            "sim-enc-bare-42",
            Some("SEARCH"),
        ),
        (
            "a long name",
            "a-collection-name-nothing-covers-at-all",
            Some("TIMESERIES"),
        ),
        ("a name of only letters", "vectorsearch", Some("VECTORSEARCH")),
    ];

    for (case, name, kind) in cases {
        let mut request = json!({ "name": name });
        if let Some(kind) = kind {
            request["type"] = json!(kind);
        }
        let (status, code, answer) = call(&router, "CreateCollection", request).await;
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_ERROR, "ValidationException"),
            "case {case:?}: expected {CLIENT_ERROR} ValidationException, got {status} {code} with \
             {answer}"
        );
        assert_eq!(
            message(&answer),
            format!(
                "No matching security policy of encryption type found for collection name: \
                 {name}. Please create security policy of encryption type for this collection."
            ),
            "case {case:?}: the refusal must name the collection's own name and nothing else, \
             got {answer}"
        );
    }

    // And the two ways a collection DOES get an encryption setting are still accepted, so
    // the refusal is about the missing setting rather than about the request's shape: the
    // request's own `encryptionConfig`, and an `encryption` policy whose rules cover the
    // name.
    create_collection(
        &router,
        json!({ "name": "has-encryption-config", "encryptionConfig": { "aWSOwnedKey": true } }),
    )
    .await;
    let covered = "covered-by-policy";
    let (status, code, answer) = call(
        &router,
        "CreateSecurityPolicy",
        json!({
            "type": "encryption",
            "name": "covering-policy",
            "policy": format!(
                r#"{{"Rules":[{{"ResourceType":"collection","Resource":["collection/{covered}"]}}],"AWSOwnedKey":true}}"#
            ),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case CreateSecurityPolicy(covering policy): expected 200, got {status} {code} with \
         {answer}"
    );
    create_collection(&router, json!({ "name": covered })).await;
}

// ---------------------------------------------------------------------------
// DeleteCollection: the deletion-protection refusal names the collection by NAME.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_deletion_protection_refusal_names_the_collection_by_its_stored_name() {
    let router = emulator();

    // The rows differ along the name the message's variable part is read from and the
    // collection type beside it; every row addresses the delete by the minted `id`, which
    // the message must NOT carry.
    for (case, name, kind) in [
        ("a short name", "abc", "TIMESERIES"),
        ("a name with digits and hyphens", "protected-coll-7", "SEARCH"),
        ("a long name", "a-protected-collection-with-a-long-name", "SEARCH"),
    ] {
        let detail = create_collection(
            &router,
            json!({
                "name": name,
                "type": kind,
                "deletionProtection": "ENABLED",
                "encryptionConfig": { "aWSOwnedKey": true },
            }),
        )
        .await;
        let id = text(&detail, "id");

        let (status, code, answer) =
            call(&router, "DeleteCollection", json!({ "id": &id })).await;
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_ERROR, "ValidationException"),
            "case {case:?}: expected {CLIENT_ERROR} ValidationException, got {status} {code} with \
             {answer}"
        );
        assert_eq!(
            message(&answer),
            format!(
                "Collection {name} cannot be deleted because deletion protection is enabled. \
                 Disable deletion protection before deleting the collection."
            ),
            "case {case:?}: the refusal must name the collection's NAME, got {answer}"
        );
        assert!(
            !message(&answer).contains(&id),
            "case {case:?}: the refusal must not name the id {id}, got {answer}"
        );

        // The refusal removed nothing: the collection is still there to be read.
        let (status, code, answer) =
            call(&router, "BatchGetCollection", json!({ "ids": [&id] })).await;
        assert_eq!(
            status, 200,
            "case {case:?} read back: expected 200, got {status} {code} with {answer}"
        );
        assert_eq!(
            at(&answer, "collectionDetails")
                .as_array()
                .map(Vec::len)
                .unwrap_or_default(),
            1,
            "case {case:?}: the refused delete must leave the collection stored, got {answer}"
        );
    }

    // The control: the same delete on a collection whose protection is DISABLED is
    // accepted once the collection has settled, so the refusal is about the SETTING.
    let detail = create_collection(
        &router,
        json!({
            "name": "unprotected-collection",
            "deletionProtection": "DISABLED",
            "encryptionConfig": { "aWSOwnedKey": true },
        }),
    )
    .await;
    tokio::time::sleep(SETTLE_WAIT).await;
    let (status, code, answer) = call(
        &router,
        "DeleteCollection",
        json!({ "id": text(&detail, "id") }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case DeleteCollection(protection DISABLED): expected 200, got {status} {code} with \
         {answer}"
    );
}

// ---------------------------------------------------------------------------
// The security-configuration not-found refusal: the READ path names the id it was
// given, the WRITE paths name the policy the id encodes.
// ---------------------------------------------------------------------------

/// The request `operation` needs to address the configuration `id`.
fn security_config_request(operation: &str, id: &str) -> Value {
    match operation {
        // `configVersion` is `@required`, so a not-found read of an update still carries
        // one; the value is a well-formed version nothing minted.
        "UpdateSecurityConfig" => json!({ "id": id, "configVersion": UNMINTED_VERSION }),
        _ => json!({ "id": id }),
    }
}

/// How `operation` words the configuration `id` names nothing under: the read path names
/// the id, the write paths name the configuration as the `Policy` this service calls it,
/// by the type and the name the id's first and last segments carry. An id carrying no
/// separator encodes neither, so it is named as the id it is.
fn security_config_absent_message(operation: &str, id: &str) -> String {
    let encoded = id.split_once('/').map(|(kind, rest)| {
        let name = rest.rsplit_once('/').map_or(rest, |(_, name)| name);
        (kind, name)
    });
    match (operation, encoded) {
        ("GetSecurityConfig", _) | (_, None) => format!("Security config with id: {id} does not exist"),
        (_, Some((kind, name))) => format!("Policy with name {name} and type {kind} is not found"),
    }
}

#[tokio::test]
async fn every_operation_that_takes_a_security_config_id_words_the_404_for_its_own_class() {
    let router = emulator();

    // The rows differ along the `SecurityConfigType` the id names (every modelled value,
    // `smithy_query(query="shape", shape="SecurityConfigType")`), the name it ends with,
    // and the operation that addresses it.
    let ids: Vec<(&str, String)> = vec![
        (
            "a saml id nothing created",
            format!("saml/{ACCOUNT}/never-created"),
        ),
        (
            "an iamidentitycenter id nothing created",
            format!("iamidentitycenter/{ACCOUNT}/never-created-either"),
        ),
        (
            "an iamfederation id nothing created",
            format!("iamfederation/{ACCOUNT}/other-name"),
        ),
        ("an id of no modelled shape at all", "not-an-id".to_string()),
    ];
    for operation in [
        "GetSecurityConfig",
        "UpdateSecurityConfig",
        "DeleteSecurityConfig",
    ] {
        for (case, id) in &ids {
            let (status, code, answer) =
                call(&router, operation, security_config_request(operation, id)).await;
            assert_eq!(
                (status, code.as_str()),
                (CLIENT_ERROR, "ResourceNotFoundException"),
                "case {operation}({case:?}): expected {CLIENT_ERROR} ResourceNotFoundException, \
                 got {status} {code} with {answer}"
            );
            assert_eq!(
                message(&answer),
                security_config_absent_message(operation, id),
                "case {operation}({case:?}): the refusal must name what it was given, got \
                 {answer}"
            );
        }
    }

    // The same wording for an id that DID name a configuration until it was deleted, which
    // is the other way a client reaches this refusal.
    let created = create_config(
        &router,
        json!({ "type": "saml", "name": "doomed-config", "samlOptions": saml_options() }),
    )
    .await;
    let id = text(&created, "id");
    let (status, code, answer) =
        call(&router, "DeleteSecurityConfig", json!({ "id": &id })).await;
    assert_eq!(
        status, 200,
        "case DeleteSecurityConfig(stored): expected 200, got {status} {code} with {answer}"
    );
    for operation in [
        "GetSecurityConfig",
        "UpdateSecurityConfig",
        "DeleteSecurityConfig",
    ] {
        let (status, code, answer) =
            call(&router, operation, security_config_request(operation, &id)).await;
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_ERROR, "ResourceNotFoundException"),
            "case {operation}(after the delete): expected {CLIENT_ERROR} \
             ResourceNotFoundException, got {status} {code} with {answer}"
        );
        assert_eq!(
            message(&answer),
            security_config_absent_message(operation, &id),
            "case {operation}(after the delete): the refusal must name the deleted \
             configuration, got {answer}"
        );
    }
}

// ---------------------------------------------------------------------------
// The tagging plane: every taggable resource KIND, and the refusal for an ARN
// that resolves to none.
// ---------------------------------------------------------------------------

/// The tags `ListTagsForResource` reports for `arn`, sorted by key.
async fn listed_tags(router: &MultiServiceRouter, arn: &str) -> Vec<(String, String)> {
    let (status, code, answer) =
        call(router, "ListTagsForResource", json!({ "resourceArn": arn })).await;
    assert_eq!(
        status, 200,
        "case ListTagsForResource({arn}): expected 200, got {status} {code} with {answer}"
    );
    let mut tags: Vec<(String, String)> = at(&answer, "tags")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|tag| (text(tag, "key"), text(tag, "value")))
        .collect();
    tags.sort();
    tags
}

#[tokio::test]
async fn the_tagging_plane_resolves_every_taggable_resource_kind() {
    let router = emulator();

    let collection = create_collection(
        &router,
        json!({ "name": "tagged-coll", "encryptionConfig": { "aWSOwnedKey": true } }),
    )
    .await;
    let group = create_group(
        &router,
        json!({ "name": "tagged-group", "standbyReplicas": "ENABLED" }),
    )
    .await;

    // The rows differ along the KIND of resource the ARN addresses and the tag VALUE
    // written: plain ASCII, multi-byte UTF-8 (which must survive byte for byte) and the
    // empty value the model's `@length(min: 0)` admits.
    let resources: Vec<(&str, String)> = vec![
        ("a collection ARN", text(&collection, "arn")),
        ("a collection group ARN", text(&group, "arn")),
    ];
    let values: Vec<(&str, &str)> = vec![
        ("plain ASCII", "search"),
        ("multi-byte UTF-8", "ベクトル検索 🌐 — naïve"),
        ("the empty value", ""),
    ];

    for (resource_case, arn) in &resources {
        assert_eq!(
            listed_tags(&router, arn).await,
            Vec::new(),
            "case {resource_case}: a resource created without tags carries none"
        );
        for (value_case, value) in &values {
            let case = format!("{resource_case} tagged with {value_case}");
            let (status, code, answer) = call(
                &router,
                "TagResource",
                json!({ "resourceArn": arn, "tags": [{ "key": "team", "value": value }] }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case}: expected 200, got {status} {code} with {answer}"
            );
            assert_eq!(
                listed_tags(&router, arn).await,
                vec![("team".to_string(), (*value).to_string())],
                "case {case}: the stored value must be the one written, byte for byte"
            );
        }

        // And the removal: a key the resource carries is gone, a key it does not carry is
        // not an error.
        let (status, code, answer) = call(
            &router,
            "UntagResource",
            json!({ "resourceArn": arn, "tagKeys": ["team", "never-set"] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {resource_case} untagged: expected 200, got {status} {code} with {answer}"
        );
        assert_eq!(
            listed_tags(&router, arn).await,
            Vec::new(),
            "case {resource_case} untagged: the removed key must be gone"
        );
    }

    // A tag written on a collection group is the group's own state, so the read of the
    // group reports it too.
    let group_arn = text(&group, "arn");
    let (status, code, answer) = call(
        &router,
        "TagResource",
        json!({ "resourceArn": &group_arn, "tags": [{ "key": "owner", "value": "データ" }] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case TagResource(group, round trip): expected 200, got {status} {code} with {answer}"
    );
    let (status, code, answer) = call(
        &router,
        "BatchGetCollectionGroup",
        json!({ "ids": [text(&group, "id")] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case BatchGetCollectionGroup(after the tag): expected 200, got {status} {code} with \
         {answer}"
    );
    let read = at(&answer, "collectionGroupDetails")
        .as_array()
        .and_then(|details| details.first())
        .cloned()
        .unwrap_or(Value::Null);
    assert_eq!(
        at(&read, "tags"),
        &json!([{ "key": "owner", "value": "データ" }]),
        "case BatchGetCollectionGroup(after the tag): the group must report the written tag, got \
         {read}"
    );
}

#[tokio::test]
async fn an_unresolvable_resource_arn_names_the_kind_and_the_id_it_could_not_find() {
    let router = emulator();

    // The rows differ along the resource KIND the ARN names, how the ARN misses (an id
    // nothing carries, another Region) and the resource half that names no addressable
    // resource at all.
    let absent_id = "0".repeat(24);
    let cases: Vec<(&str, String, String)> = vec![
        (
            "a collection id nothing carries",
            format!("arn:aws:aoss:us-east-1:{ACCOUNT}:collection/{absent_id}"),
            format!("Collection ID {absent_id} is not found"),
        ),
        (
            "a collection group id nothing carries",
            format!("arn:aws:aoss:us-east-1:{ACCOUNT}:collection-group/{absent_id}"),
            format!("Collection Group ID {absent_id} is not found"),
        ),
        (
            "a collection of another Region",
            format!("arn:aws:aoss:eu-west-1:{ACCOUNT}:collection/{absent_id}"),
            format!("Collection ID {absent_id} is not found"),
        ),
        (
            "a collection group of another Region",
            format!("arn:aws:aoss:eu-west-1:{ACCOUNT}:collection-group/{absent_id}"),
            format!("Collection Group ID {absent_id} is not found"),
        ),
    ];
    // An ARN whose resource half names no resource this service addresses carries no id to
    // name, so the refusal names the ARN itself.
    let unaddressable = format!("arn:aws:aoss:us-east-1:{ACCOUNT}:vpc-endpoint/vpce-0123456789");

    let operations: Vec<(&str, Value)> = vec![
        ("TagResource", json!({ "tags": [{ "key": "k", "value": "v" }] })),
        ("UntagResource", json!({ "tagKeys": ["k"] })),
        ("ListTagsForResource", json!({})),
    ];

    for (operation, extra) in &operations {
        for (case, arn, expected) in &cases {
            let mut request = extra.clone();
            request["resourceArn"] = json!(arn);
            let (status, code, answer) = call(&router, operation, request).await;
            assert_eq!(
                (status, code.as_str()),
                (CLIENT_ERROR, "ResourceNotFoundException"),
                "case {operation}({case:?}): expected {CLIENT_ERROR} ResourceNotFoundException, \
                 got {status} {code} with {answer}"
            );
            assert_eq!(
                &message(&answer),
                expected,
                "case {operation}({case:?}): the refusal must name the resource kind and its id, \
                 got {answer}"
            );
        }

        let mut request = extra.clone();
        request["resourceArn"] = json!(&unaddressable);
        let (status, code, answer) = call(&router, operation, request).await;
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_ERROR, "ResourceNotFoundException"),
            "case {operation}(an unaddressable resource half): expected {CLIENT_ERROR} \
             ResourceNotFoundException, got {status} {code} with {answer}"
        );
        assert_eq!(
            message(&answer),
            format!("Resource {unaddressable} is not found"),
            "case {operation}(an unaddressable resource half): the refusal must name the ARN, \
             got {answer}"
        );
    }
}

// ---------------------------------------------------------------------------
// The optimistic lock: a consumed version is invalid input, not a conflict.
// ---------------------------------------------------------------------------

/// One policy/configuration family's write path: the operations, the detail shape they
/// answer with, and the member the version travels in.
struct VersionedFamily {
    case: &'static str,
    update: &'static str,
    detail: &'static str,
    version_member: &'static str,
    /// The identity members every request of this family carries.
    identity: fn() -> Value,
    /// The create request, including the identity members.
    create: fn() -> (&'static str, Value),
}

/// The four families whose update takes a version, from the model: three policy types plus
/// the security configuration (`UpdateAccessPolicy`, `UpdateSecurityPolicy`,
/// `UpdateLifecyclePolicy` carry `policyVersion`; `UpdateSecurityConfig` carries
/// `configVersion`).
fn versioned_families() -> Vec<VersionedFamily> {
    vec![
        VersionedFamily {
            case: "a data access policy",
            update: "UpdateAccessPolicy",
            detail: "accessPolicyDetail",
            version_member: "policyVersion",
            identity: || json!({ "type": "data", "name": "locked-access" }),
            create: || {
                (
                    "CreateAccessPolicy",
                    json!({ "type": "data", "name": "locked-access", "policy": DATA_POLICY }),
                )
            },
        },
        VersionedFamily {
            case: "an encryption security policy",
            update: "UpdateSecurityPolicy",
            detail: "securityPolicyDetail",
            version_member: "policyVersion",
            identity: || json!({ "type": "encryption", "name": "locked-encryption" }),
            create: || {
                (
                    "CreateSecurityPolicy",
                    json!({
                        "type": "encryption",
                        "name": "locked-encryption",
                        "policy": r#"{"Rules":[{"ResourceType":"collection","Resource":["collection/locked*"]}],"AWSOwnedKey":true}"#,
                    }),
                )
            },
        },
        VersionedFamily {
            case: "a retention lifecycle policy",
            update: "UpdateLifecyclePolicy",
            detail: "lifecyclePolicyDetail",
            version_member: "policyVersion",
            identity: || json!({ "type": "retention", "name": "locked-retention" }),
            create: || {
                (
                    "CreateLifecyclePolicy",
                    json!({
                        "type": "retention",
                        "name": "locked-retention",
                        "policy": RETENTION_RULES,
                    }),
                )
            },
        },
        VersionedFamily {
            case: "a saml security configuration",
            update: "UpdateSecurityConfig",
            detail: "securityConfigDetail",
            version_member: "configVersion",
            identity: || json!({}),
            create: || {
                (
                    "CreateSecurityConfig",
                    json!({
                        "type": "saml",
                        "name": "locked-saml",
                        "samlOptions": { "metadata": "<EntityDescriptor entityID=\"https://idp.example.com/metadata\"></EntityDescriptor>" },
                    }),
                )
            },
        },
    ]
}

#[tokio::test]
async fn a_consumed_version_is_refused_as_invalid_input_on_every_versioned_update() {
    let router = emulator();

    for family in versioned_families() {
        let case = family.case;
        let (create_operation, create_body) = (family.create)();
        let (status, code, answer) = call(&router, create_operation, create_body).await;
        assert_eq!(
            status, 200,
            "case {case}: {create_operation} expected 200, got {status} {code} with {answer}"
        );
        let created = at(&answer, family.detail).clone();

        // A security configuration is addressed by its minted id; a policy by its
        // `(type, name)` pair. Either way the identity members ride on every request.
        let mut identity = (family.identity)();
        if family.update == "UpdateSecurityConfig" {
            identity["id"] = json!(text(&created, "id"));
        }
        let versions = text(&created, family.version_member);

        // The first update consumes the creation version and mints a fresh one.
        let mut request = identity.clone();
        request[family.version_member] = json!(versions.clone());
        request["description"] = json!("the winner");
        let (status, code, answer) = call(&router, family.update, request).await;
        assert_eq!(
            status, 200,
            "case {case}: the first update expected 200, got {status} {code} with {answer}"
        );
        let after_first = at(&answer, family.detail).clone();
        let current = text(&after_first, family.version_member);
        assert_ne!(
            current, versions,
            "case {case}: the accepted update must mint a fresh version, got {after_first}"
        );

        // Every version that is not the stored one is refused the same way: the one the
        // first update consumed, and one nothing ever minted.
        for (what, version) in [
            ("the version the first update consumed", versions.clone()),
            ("a version nothing ever minted", UNMINTED_VERSION.to_string()),
        ] {
            let mut request = identity.clone();
            request[family.version_member] = json!(version);
            request["description"] = json!("this write must not land");
            let (status, code, answer) = call(&router, family.update, request).await;
            assert_eq!(
                (status, code.as_str()),
                (CLIENT_ERROR, "ValidationException"),
                "case {case} with {what}: expected {CLIENT_ERROR} ValidationException, got \
                 {status} {code} with {answer}"
            );
            assert_eq!(
                message(&answer),
                STALE_VERSION_MESSAGE,
                "case {case} with {what}: the refusal must state that the version is older than \
                 the stored one, got {answer}"
            );
        }

        // The refused writes stored nothing: the resource is exactly what the accepted
        // update left.
        let mut request = identity.clone();
        request[family.version_member] = json!(current.clone());
        request["description"] = json!("the second winner");
        let (status, code, answer) = call(&router, family.update, request).await;
        assert_eq!(
            status, 200,
            "case {case}: the current version must still be accepted, got {status} {code} with \
             {answer}"
        );
        assert_eq!(
            text(at(&answer, family.detail), "description"),
            "the second winner",
            "case {case}: the accepted write is the one that lands, got {answer}"
        );
    }
}

// ---------------------------------------------------------------------------
// DeleteCollectionGroup: a group that still holds collections.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_collection_group_holding_collections_is_refused_as_invalid_input() {
    let router = emulator();

    // The rows differ along how many collections the group holds, so the refusal is about
    // emptiness rather than about one count.
    for (case, group, collections) in [
        ("one collection", "held-one", vec!["held-collection-a"]),
        (
            "three collections",
            "held-three",
            vec!["held-collection-b", "held-collection-c", "held-collection-d"],
        ),
    ] {
        let detail = create_group(
            &router,
            json!({ "name": group, "standbyReplicas": "DISABLED" }),
        )
        .await;
        let id = text(&detail, "id");
        for collection in &collections {
            create_collection(
                &router,
                json!({
                    "name": collection,
                    "collectionGroupName": group,
                    "encryptionConfig": { "aWSOwnedKey": true },
                }),
            )
            .await;
        }

        let (status, code, answer) =
            call(&router, "DeleteCollectionGroup", json!({ "id": &id })).await;
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_ERROR, "ValidationException"),
            "case {case:?}: expected {CLIENT_ERROR} ValidationException, got {status} {code} with \
             {answer}"
        );
        assert_eq!(
            message(&answer),
            format!("CollectionGroup with ID {id}, has collections associated and cannot be deleted"),
            "case {case:?}: the refusal must name the group id, got {answer}"
        );
    }

    // The control: an EMPTY group is deleted, so the refusal is about the members.
    let empty = create_group(
        &router,
        json!({ "name": "held-none", "standbyReplicas": "DISABLED" }),
    )
    .await;
    let (status, code, answer) = call(
        &router,
        "DeleteCollectionGroup",
        json!({ "id": text(&empty, "id") }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case an empty group: expected 200, got {status} {code} with {answer}"
    );
}

// ---------------------------------------------------------------------------
// UpdateAccountSettings: the account's own OCU quota.
// ---------------------------------------------------------------------------

/// The `capacityLimits` pair `GetAccountSettings` reports.
async fn stored_limits(router: &MultiServiceRouter) -> (Option<i64>, Option<i64>) {
    let (status, code, answer) = call(router, "GetAccountSettings", json!({})).await;
    assert_eq!(
        status, 200,
        "case GetAccountSettings: expected 200, got {status} {code} with {answer}"
    );
    let limits = at(&answer, "accountSettingsDetail.capacityLimits");
    (
        at(limits, "maxIndexingCapacityInOCU").as_i64(),
        at(limits, "maxSearchCapacityInOCU").as_i64(),
    )
}

#[tokio::test]
async fn a_capacity_limit_past_the_account_quota_is_refused_and_stores_nothing() {
    let router = emulator();

    let quota = i64::from(MAX_ACCOUNT_CAPACITY_IN_OCU);
    let before = stored_limits(&router).await;

    // The rows differ along the capacity type the request names, how far past the quota it
    // asks for (one past, far past, the largest the modelled `integer` carries) and the
    // word the refusal uses for that type.
    let cases: Vec<(&str, &str, &str, i64)> = vec![
        (
            "the indexing ceiling one past the quota",
            "maxIndexingCapacityInOCU",
            "indexing",
            quota + 1,
        ),
        (
            "the search ceiling one past the quota",
            "maxSearchCapacityInOCU",
            "search",
            quota + 1,
        ),
        (
            "the indexing ceiling far past the quota",
            "maxIndexingCapacityInOCU",
            "indexing",
            quota * 3,
        ),
        (
            "the search ceiling at the modelled maximum",
            "maxSearchCapacityInOCU",
            "search",
            i64::from(i32::MAX),
        ),
    ];

    for (case, member, word, asked) in cases {
        let (status, code, answer) = call(
            &router,
            "UpdateAccountSettings",
            json!({ "capacityLimits": { member: asked } }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_ERROR, "ServiceQuotaExceededException"),
            "case {case:?}: expected {CLIENT_ERROR} ServiceQuotaExceededException, got {status} \
             {code} with {answer}"
        );
        let current = if member == "maxIndexingCapacityInOCU" {
            before.0.unwrap_or_default()
        } else {
            before.1.unwrap_or_default()
        };
        assert_eq!(
            message(&answer),
            format!(
                "The requested total {word} capacity of {asked}.0 OCUs exceeds your maximum \
                 {word} capacity service quota limit of {quota} OCUs. Current settings: \
                 {current} OCUs for collections that are not part of any collection group, 0 \
                 OCUs for collection groups. New request of {asked} OCUs would bring total to \
                 {asked}.0 OCUs."
            ),
            "case {case:?}: the refusal must name the request's figure, the quota and the \
             account's own, got {answer}"
        );
        assert_eq!(
            stored_limits(&router).await,
            before,
            "case {case:?}: a refused update must store nothing"
        );
    }

    // The quota itself is accepted on both members, so the refusal is about being PAST it.
    let (status, code, answer) = call(
        &router,
        "UpdateAccountSettings",
        json!({ "capacityLimits": {
            "maxIndexingCapacityInOCU": quota,
            "maxSearchCapacityInOCU": quota,
        }}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case both members AT the quota: expected 200, got {status} {code} with {answer}"
    );
    assert_eq!(
        stored_limits(&router).await,
        (Some(quota), Some(quota)),
        "case both members AT the quota: the stored limits must be the ones named"
    );
}

#[tokio::test]
async fn the_account_quota_counts_the_capacity_the_collection_groups_commit() {
    let router = emulator();

    // A group commits its own ceilings, which the account's total counts: with this group
    // stored, the ceiling an update may ask for is the quota LESS what the group commits.
    let committed_ocu: i64 = 100;
    create_group(
        &router,
        json!({
            "name": "committing-group",
            "standbyReplicas": "ENABLED",
            "capacityLimits": {
                "maxIndexingCapacityInOCU": 100.0,
                "maxSearchCapacityInOCU": 100.0,
            },
        }),
    )
    .await;

    let quota = i64::from(MAX_ACCOUNT_CAPACITY_IN_OCU);
    let before = stored_limits(&router).await;

    // One past the room the group leaves is refused, and the refusal names the group's
    // committed capacity and the total it would reach.
    let asked = quota - committed_ocu + 1;
    let (status, code, answer) = call(
        &router,
        "UpdateAccountSettings",
        json!({ "capacityLimits": { "maxIndexingCapacityInOCU": asked } }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (CLIENT_ERROR, "ServiceQuotaExceededException"),
        "case one past the room the group leaves: expected {CLIENT_ERROR} \
         ServiceQuotaExceededException, got {status} {code} with {answer}"
    );
    assert_eq!(
        message(&answer),
        format!(
            "The requested total indexing capacity of {asked}.0 OCUs exceeds your maximum \
             indexing capacity service quota limit of {quota} OCUs. Current settings: {} OCUs \
             for collections that are not part of any collection group, {committed_ocu} OCUs for \
             collection groups. New request of {asked} OCUs would bring total to {}.0 OCUs.",
            before.0.unwrap_or_default(),
            asked + committed_ocu
        ),
        "case one past the room the group leaves: the refusal must count the group's committed \
         capacity, got {answer}"
    );
    assert_eq!(
        stored_limits(&router).await,
        before,
        "case one past the room the group leaves: a refused update must store nothing"
    );

    // Exactly the room it leaves is accepted.
    let fits = quota - committed_ocu;
    let (status, code, answer) = call(
        &router,
        "UpdateAccountSettings",
        json!({ "capacityLimits": { "maxIndexingCapacityInOCU": fits } }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case exactly the room the group leaves: expected 200, got {status} {code} with {answer}"
    );
    assert_eq!(
        stored_limits(&router).await.0,
        Some(fits),
        "case exactly the room the group leaves: the limit must be stored"
    );
}

// ---------------------------------------------------------------------------
// The data access policy document: a statement that names no principal.
// ---------------------------------------------------------------------------

/// A data access policy statement granting one permission to `principals`.
fn statement(principals: Value) -> Value {
    json!({
        "Rules": [{
            "ResourceType": "index",
            "Resource": ["index/demo/*"],
            "Permission": ["aoss:ReadDocument"],
        }],
        "Principal": principals,
    })
}

#[tokio::test]
async fn a_statement_naming_no_principal_is_refused_and_stores_nothing() {
    let router = emulator();

    // The rows differ along the document's spelling (one statement object, a list of
    // statements), how many statements it carries and WHICH of them names no principal —
    // the index the refusal's JSON path carries.
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "a single statement in a list",
            json!([statement(json!([]))]),
            vec!["$[0].Principal"],
        ),
        (
            "a single statement written as an object",
            statement(json!([])),
            vec!["$[0].Principal"],
        ),
        (
            "the second of two statements",
            json!([
                statement(json!(["arn:aws:iam::000000000000:role/Reader"])),
                statement(json!([])),
            ]),
            vec!["$[1].Principal"],
        ),
        (
            "both statements of two",
            json!([statement(json!([])), statement(json!([]))]),
            vec!["$[0].Principal", "$[1].Principal"],
        ),
    ];

    for (index, (case, document, paths)) in cases.into_iter().enumerate() {
        let name = format!("empty-principal-{index}");
        let expected = format!(
            "Policy json is invalid, error: [{}]",
            paths
                .iter()
                .map(|path| format!("{path}: there must be a minimum of 1 items in the array"))
                .collect::<Vec<String>>()
                .join(", ")
        );
        let (status, code, answer) = call(
            &router,
            "CreateAccessPolicy",
            json!({ "type": "data", "name": &name, "policy": document.to_string() }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_ERROR, "ValidationException"),
            "case {case:?}: expected {CLIENT_ERROR} ValidationException, got {status} {code} with \
             {answer}"
        );
        assert_eq!(
            message(&answer),
            expected,
            "case {case:?}: the refusal must name the JSON path of every empty list, got {answer}"
        );

        // Nothing was stored: the name the refused create used names no policy.
        let (status, code, answer) = call(
            &router,
            "GetAccessPolicy",
            json!({ "type": "data", "name": &name }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_ERROR, "ResourceNotFoundException"),
            "case {case:?} read back: expected {CLIENT_ERROR} ResourceNotFoundException, got \
             {status} {code} with {answer}"
        );
        assert_eq!(
            message(&answer),
            format!("Policy with name {name} and type data is not found"),
            "case {case:?} read back: the refusal must name the pair that was asked for, got \
             {answer}"
        );
    }

    // A statement that DOES name a principal is stored, and the same check holds on the
    // update: a document that would empty the principal list is refused and the stored
    // policy is untouched.
    let good = json!([statement(json!([
        "arn:aws:iam::000000000000:role/Reader",
        "arn:aws:iam::000000000000:role/Writer",
    ]))]);
    let (status, code, answer) = call(
        &router,
        "CreateAccessPolicy",
        json!({ "type": "data", "name": "named-principals", "policy": good.to_string() }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case a statement naming two principals: expected 200, got {status} {code} with {answer}"
    );
    let stored = at(&answer, "accessPolicyDetail").clone();

    let (status, code, answer) = call(
        &router,
        "UpdateAccessPolicy",
        json!({
            "type": "data",
            "name": "named-principals",
            "policyVersion": text(&stored, "policyVersion"),
            "policy": json!([statement(json!([]))]).to_string(),
        }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (CLIENT_ERROR, "ValidationException"),
        "case an update emptying the principal list: expected {CLIENT_ERROR} ValidationException, \
         got {status} {code} with {answer}"
    );
    assert_eq!(
        message(&answer),
        "Policy json is invalid, error: [$[0].Principal: there must be a minimum of 1 items in \
         the array]",
        "case an update emptying the principal list: the refusal must name the JSON path, got \
         {answer}"
    );
    let (status, code, answer) = call(
        &router,
        "GetAccessPolicy",
        json!({ "type": "data", "name": "named-principals" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case the stored policy after the refused update: expected 200, got {status} {code} with \
         {answer}"
    );
    assert_eq!(
        at(&answer, "accessPolicyDetail"),
        &stored,
        "case the stored policy after the refused update: a refused write must change nothing"
    );
}

// ---------------------------------------------------------------------------
// CollectionGroupCapacityLimits: a floor above its own ceiling.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_impossible_capacity_range_names_the_members_and_their_float_values() {
    let router = emulator();

    // The rows differ along which pair contradicts itself, whether the values are whole or
    // fractional, and how far apart they are — the two numbers the message renders as the
    // floats the members are modelled with.
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "whole indexing values",
            json!({ "minIndexingCapacityInOCU": 32.0, "maxIndexingCapacityInOCU": 16.0 }),
            "Invalid capacity range: minIndexingCapacityInOCU (32.0) exceeds \
             maxIndexingCapacityInOCU (16.0)",
        ),
        (
            "whole search values",
            json!({ "minSearchCapacityInOCU": 8.0, "maxSearchCapacityInOCU": 4.0 }),
            "Invalid capacity range: minSearchCapacityInOCU (8.0) exceeds \
             maxSearchCapacityInOCU (4.0)",
        ),
        (
            "fractional indexing values",
            json!({ "minIndexingCapacityInOCU": 2.5, "maxIndexingCapacityInOCU": 1.5 }),
            "Invalid capacity range: minIndexingCapacityInOCU (2.5) exceeds \
             maxIndexingCapacityInOCU (1.5)",
        ),
        (
            "a floor a fraction above its ceiling",
            json!({ "minSearchCapacityInOCU": 16.5, "maxSearchCapacityInOCU": 16.0 }),
            "Invalid capacity range: minSearchCapacityInOCU (16.5) exceeds \
             maxSearchCapacityInOCU (16.0)",
        ),
    ];

    // The group every update row addresses, created with limits nothing contradicts.
    let group = create_group(
        &router,
        json!({
            "name": "capacity-range-group",
            "standbyReplicas": "ENABLED",
            "capacityLimits": { "maxIndexingCapacityInOCU": 64.0, "maxSearchCapacityInOCU": 64.0 },
        }),
    )
    .await;
    let group_id = text(&group, "id");

    for (index, (case, limits, expected)) in cases.into_iter().enumerate() {
        // The same structure is refused by the write that states it for a new group and by
        // the write that restates it for a stored one.
        let writes: Vec<(&str, Value)> = vec![
            (
                "CreateCollectionGroup",
                json!({
                    "name": format!("range-group-{index}"),
                    "standbyReplicas": "ENABLED",
                    "capacityLimits": limits.clone(),
                }),
            ),
            (
                "UpdateCollectionGroup",
                json!({ "id": &group_id, "capacityLimits": limits.clone() }),
            ),
        ];
        for (operation, request) in writes {
            let (status, code, answer) = call(&router, operation, request).await;
            assert_eq!(
                (status, code.as_str()),
                (CLIENT_ERROR, "ValidationException"),
                "case {operation}({case:?}): expected {CLIENT_ERROR} ValidationException, got \
                 {status} {code} with {answer}"
            );
            assert_eq!(
                message(&answer),
                expected,
                "case {operation}({case:?}): the refusal must name both members and render their \
                 values as floats, got {answer}"
            );
        }
    }

    // A floor AT its ceiling contradicts nothing, so it is accepted: the refusal is about
    // the floor being ABOVE the ceiling. The floors go to the published minimum of
    // `CollectionGroupMinIndexingCapacityValue` (`@range(min: 0)`) and the ceilings stay at
    // their own (`@range(min: 1)`).
    let accepted_limits = json!({
        "minIndexingCapacityInOCU": 8.0,
        "maxIndexingCapacityInOCU": 8.0,
        "minSearchCapacityInOCU": 0.0,
        "maxSearchCapacityInOCU": 1.0,
    });
    let (status, code, answer) = call(
        &router,
        "UpdateCollectionGroup",
        json!({ "id": &group_id, "capacityLimits": accepted_limits.clone() }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case a floor at its ceiling: expected 200, got {status} {code} with {answer}"
    );

    // And the refusals above stored nothing: the group still carries the limits the last
    // accepted write left.
    let (status, code, answer) = call(
        &router,
        "BatchGetCollectionGroup",
        json!({ "ids": [&group_id] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case the group after the refusals: expected 200, got {status} {code} with {answer}"
    );
    let read = at(&answer, "collectionGroupDetails")
        .as_array()
        .and_then(|details| details.first())
        .cloned()
        .unwrap_or(Value::Null);
    assert_eq!(
        at(&read, "capacityLimits"),
        &accepted_limits,
        "case the group after the refusals: only the accepted write may have landed, got {read}"
    );
}
