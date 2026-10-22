// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the TEXT of this service's modelled exceptions.
//!
//! The rule under test: an exception whose message AWS fixes carries that
//! CONSTANT text, with nothing of the request or of generated state spliced into
//! it, and reports the specifics in the members that model them.
//!
//! Two exceptions of this service have such a message:
//!
//! * `ResourceNotFoundException` models `resourceId` ("The unique ID of the
//!   resource referenced in the failed request") and `resourceType` as required
//!   members, so the identity of the missing resource has its own modelled home
//!   and the message is one fixed sentence — the same whatever operation raised
//!   it, whatever form the reference took (an id, an alias reference, a
//!   `name/` reference, an ARN) and whatever resource type was missing.
//! * `InvalidStateException` models a single `message`, and both its own model
//!   documentation and the recorded wire response carry one fixed
//!   deletion-protection refusal sentence, which names no resource.
//! * `ConflictException` models a REQUIRED `resources` list — "the list of
//!   resources referenced with this failed request", each a
//!   `ResourceConflict{resourceId, resourceType}` — so the collision is
//!   reported there and the message is one fixed summary, the same for every
//!   cause (a contested alias name, a name awaiting deletion, a name already
//!   used inside a store, an idempotency token replayed with other
//!   parameters) and every operation that can conflict.
//! * `TooManyTagsException` models `resourceName`, so the resource at the tag
//!   limit is reported there and neither the count nor the offending key is in
//!   the message.
//!
//! Every assertion goes over the wire through the same `MultiServiceRouter` the
//! emulator binary serves, reusing the shared harness in [`crate::harness`].

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The wire harness (`router`, `call`, `ok`, `store_with_mode`, `error_code`)
// lives once in `tests/all/harness.rs` and is shared with every suite in this
// crate.
use crate::harness::{call, error_code, fault_message, ok, router, store_with_mode};

/// The one message every `ResourceNotFoundException` of this service carries.
///
/// Written out here rather than imported on purpose: this suite pins the
/// constant the wire carries, so a change to the implementation constant has to
/// fail this test instead of moving with it.
const RESOURCE_NOT_FOUND_MESSAGE: &str = "Resource not found";

/// The deletion-protection refusal `DeletePolicyStore` answers, as the wire
/// carries it — the constant `src/ops/mod.rs` owns, which
/// `InvalidStateException`'s own documentation states as the same fixed refusal
/// in slightly different prose. Written out here rather than imported for the
/// same reason as above.
const DELETION_PROTECTION_MESSAGE: &str = "The policy store cannot be deleted because deletion \
     protection is enabled. Deletion protection must be disabled first.";

/// Every value `ResourceType` admits, from
/// `smithy_query(query="shape", shape="ResourceType")`. A `resourceType` this
/// service reports has to be one of them.
///
/// Written out here rather than importing the crate's generated
/// `ops::types::RESOURCE_TYPE_VALUES` on purpose, like the message constants
/// above: this suite pins the list the MODEL publishes, so a change to the
/// generated constant has to fail this test instead of moving with it.
const RESOURCE_TYPE_VALUES: &[&str] = &[
    "IDENTITY_SOURCE",
    "POLICY_STORE",
    "POLICY",
    "POLICY_TEMPLATE",
    "SCHEMA",
    "POLICY_STORE_ALIAS",
];

/// The prefix an alias reference carries, from `CreatePolicyStoreAliasInput$aliasName`.
const ALIAS_PREFIX: &str = "policy-store-alias/";

/// The one message every `ConflictException` of this service carries.
///
/// Written out here rather than imported, for the same reason as the constants
/// above. It is the text the recordings show the service emitting for two
/// structurally different conflict causes on `CreatePolicyStoreAlias`, which is
/// what establishes it as fixed rather than request-derived.
const CONFLICT_MESSAGE: &str = "Conflict occurred during the processing of this request";

/// The one message every `TooManyTagsException` of this service carries.
const TOO_MANY_TAGS_MESSAGE: &str = "The request exceeds the maximum number of user tags allowed";

/// Published Verified Permissions quota: *tags per resource*. The model states
/// it twice — `TagResource`'s documentation ("You can associate as many as 50
/// tags with a resource") and `TooManyTagsException`'s ("the limit (50) has been
/// reached").
const MAX_TAGS_PER_RESOURCE: usize = 50;

/// `PolicyStoreId`'s published upper length bound (`length 1..200`).
const MAX_POLICY_STORE_ID_LEN: usize = 200;

/// One row of the not-found table: a request that names a resource which does
/// not exist, and the identity the exception must report for it.
struct NotFound {
    case: &'static str,
    operation: &'static str,
    request: Value,
    /// What the request named. It must NOT appear in the message.
    reference: String,
    /// The `resourceId` the exception must carry.
    resource_id: String,
    /// The `resourceType` the exception must carry.
    resource_type: &'static str,
}

/// A Cedar policy statement, used wherever a row needs a well-formed
/// `CreatePolicy` body so the row fails on the missing STORE and not on the
/// policy text.
const STATEMENT: &str = "permit(principal, action, resource);";

/// A Cedar policy TEMPLATE statement — a scope carrying a `?principal`
/// placeholder — so a `CreatePolicyTemplate` row fails on the missing store
/// rather than on the template text.
const TEMPLATE_STATEMENT: &str = "permit(principal == ?principal, action, resource);";

/// A well-formed Cognito identity-source configuration, in the account and the
/// region this emulator presents, so a `CreateIdentitySource` row fails on the
/// missing store rather than on its provider reference.
fn cognito_configuration() -> Value {
    json!({
        "cognitoUserPoolConfiguration": {
            "userPoolArn": format!(
                "arn:aws:cognito-idp:us-east-1:{}:userpool/us-east-1_1a2b3c4d5",
                engine::emulator_account_id()
            ),
        },
    })
}

/// A well-formed Cedar schema document, so a `PutSchema` row fails on the
/// missing store rather than on the definition.
fn schema_document() -> String {
    serde_json::to_string(&json!({
        "PhotoApp": {
            "entityTypes": { "User": { "shape": { "type": "Record", "attributes": {} } } },
            "actions": {
                "read": { "appliesTo": { "principalTypes": ["User"], "resourceTypes": ["User"] } },
            },
        },
    }))
    .expect("the schema document serializes")
}

/// The ARN of the store named by `store_id`, as `GetPolicyStore` reports it.
async fn store_arn(router: &MultiServiceRouter, store_id: &str) -> String {
    let store = ok(
        router,
        "GetPolicyStore",
        &json!({ "policyStoreId": store_id }),
    )
    .await;
    store["arn"]
        .as_str()
        .expect("arn is required on the GetPolicyStore response")
        .to_string()
}

/// Every not-found path this service serves answers ONE constant message, with
/// the identity of the missing resource in `resourceId`/`resourceType`.
///
/// The rows differ along the operation, the FORM of the reference that resolves
/// to nothing (a store id, a 200-character id, an id carrying every punctuation
/// character the pattern admits, an alias reference that no longer resolves, a
/// policy id, a `name/` reference, a policy-store ARN, another service's ARN),
/// and the TYPE of resource that is missing (`POLICY_STORE`, `POLICY`,
/// `POLICY_TEMPLATE`, `POLICY_STORE_ALIAS`, `IDENTITY_SOURCE`).
#[tokio::test]
async fn every_not_found_path_answers_the_same_constant_message() {
    let router = router();

    // A live store with one policy in it, so a row can miss the POLICY inside a
    // store that really exists.
    let live_store = store_with_mode(&router, "OFF").await;
    ok(
        &router,
        "CreatePolicy",
        &json!({
            "policyStoreId": live_store,
            "definition": { "static": { "statement": STATEMENT } },
            "name": "live-policy",
        }),
    )
    .await;

    // A store created, then deleted: its ARN is well formed and really was a
    // live resource's.
    let gone_store = store_with_mode(&router, "OFF").await;
    let gone_arn = store_arn(&router, &gone_store).await;
    ok(
        &router,
        "DeletePolicyStore",
        &json!({ "policyStoreId": gone_store }),
    )
    .await;

    // An alias, soft-deleted: `DeletePolicyStoreAlias` documents that a
    // `PendingDeletion` alias used in a `policyStoreId` member "will fail with a
    // `ResourceNotFound` exception".
    let pending_alias = format!("{ALIAS_PREFIX}pending-deletion");
    ok(
        &router,
        "CreatePolicyStoreAlias",
        &json!({ "aliasName": pending_alias, "policyStoreId": live_store }),
    )
    .await;
    ok(
        &router,
        "DeletePolicyStoreAlias",
        &json!({ "aliasName": pending_alias }),
    )
    .await;

    let unknown_store = "PSNONEXISTENT00000000".to_string();
    // Exactly at `PolicyStoreId`'s published length bound.
    let longest_store = format!("PS{}", "a".repeat(MAX_POLICY_STORE_ID_LEN - 2));
    // Every punctuation character `PolicyStoreId`'s pattern `^[a-zA-Z0-9-/_]*$`
    // admits, in one reference.
    let punctuated_store = "PS-store_1/2".to_string();
    let unknown_policy = "SPDOESNOTEXIST000000".to_string();
    let unknown_policy_name = "name/no-such-policy".to_string();
    let unknown_alias = format!("{ALIAS_PREFIX}no-such-alias");
    let unknown_identity_source = "ISDOESNOTEXIST000000".to_string();
    let unknown_template = "PTDOESNOTEXIST000000".to_string();
    let fake_arn =
        "arn:aws:verifiedpermissions::000000000000:policy-store/PSFAKE0000000000000".to_string();
    let other_service_arn = "arn:aws:sqs:us-east-1:000000000000:some-queue".to_string();

    let rows = vec![
        NotFound {
            case: "GetPolicyStore, store id that was never minted",
            operation: "GetPolicyStore",
            request: json!({ "policyStoreId": unknown_store }),
            reference: unknown_store.clone(),
            resource_id: unknown_store.clone(),
            resource_type: "POLICY_STORE",
        },
        NotFound {
            case: "GetPolicyStore, store id at the modelled length bound",
            operation: "GetPolicyStore",
            request: json!({ "policyStoreId": longest_store }),
            reference: longest_store.clone(),
            resource_id: longest_store.clone(),
            resource_type: "POLICY_STORE",
        },
        NotFound {
            case: "GetPolicyStore, store id carrying every admitted punctuation",
            operation: "GetPolicyStore",
            request: json!({ "policyStoreId": punctuated_store }),
            reference: punctuated_store.clone(),
            resource_id: punctuated_store.clone(),
            resource_type: "POLICY_STORE",
        },
        NotFound {
            // The reference named an ALIAS, so the missing resource is the
            // alias, reported by its own name without the `policy-store-alias/`
            // resource path — `resourceId` is "The unique ID of the resource
            // referenced in the failed request".
            case: "GetPolicyStore, alias reference pending deletion",
            operation: "GetPolicyStore",
            request: json!({ "policyStoreId": pending_alias }),
            reference: pending_alias.clone(),
            resource_id: "pending-deletion".to_string(),
            resource_type: "POLICY_STORE_ALIAS",
        },
        NotFound {
            case: "GetPolicyStore, the id of a store that was deleted",
            operation: "GetPolicyStore",
            request: json!({ "policyStoreId": gone_store }),
            reference: gone_store.clone(),
            resource_id: gone_store.clone(),
            resource_type: "POLICY_STORE",
        },
        NotFound {
            case: "GetPolicy, unknown store",
            operation: "GetPolicy",
            request: json!({ "policyStoreId": unknown_store, "policyId": unknown_policy }),
            reference: unknown_store.clone(),
            resource_id: unknown_store.clone(),
            resource_type: "POLICY_STORE",
        },
        NotFound {
            case: "GetPolicy, live store and unknown policy id",
            operation: "GetPolicy",
            request: json!({ "policyStoreId": live_store, "policyId": unknown_policy }),
            reference: unknown_policy.clone(),
            resource_id: unknown_policy.clone(),
            resource_type: "POLICY",
        },
        NotFound {
            // A `name/<policyName>` reference names a POLICY, and its id is the
            // bare policy name — the resource path is not an id a caller can
            // feed back into the API.
            case: "GetPolicy, live store and unknown name/ reference",
            operation: "GetPolicy",
            request: json!({ "policyStoreId": live_store, "policyId": unknown_policy_name }),
            reference: unknown_policy_name.clone(),
            resource_id: unknown_policy_name
                .trim_start_matches("name/")
                .to_string(),
            resource_type: "POLICY",
        },
        NotFound {
            case: "DeletePolicy, live store and unknown policy id",
            operation: "DeletePolicy",
            request: json!({ "policyStoreId": live_store, "policyId": unknown_policy }),
            reference: unknown_policy.clone(),
            resource_id: unknown_policy.clone(),
            resource_type: "POLICY",
        },
        NotFound {
            case: "ListPolicies, unknown store",
            operation: "ListPolicies",
            request: json!({ "policyStoreId": unknown_store }),
            reference: unknown_store.clone(),
            resource_id: unknown_store.clone(),
            resource_type: "POLICY_STORE",
        },
        NotFound {
            case: "CreatePolicy, unknown store",
            operation: "CreatePolicy",
            request: json!({
                "policyStoreId": unknown_store,
                "definition": { "static": { "statement": STATEMENT } },
            }),
            reference: unknown_store.clone(),
            resource_id: unknown_store.clone(),
            resource_type: "POLICY_STORE",
        },
        NotFound {
            case: "PutSchema, unknown store",
            operation: "PutSchema",
            request: json!({
                "policyStoreId": unknown_store,
                "definition": { "cedarJson": schema_document() },
            }),
            reference: unknown_store.clone(),
            resource_id: unknown_store.clone(),
            resource_type: "POLICY_STORE",
        },
        NotFound {
            case: "GetSchema, unknown store",
            operation: "GetSchema",
            request: json!({ "policyStoreId": unknown_store }),
            reference: unknown_store.clone(),
            resource_id: unknown_store.clone(),
            resource_type: "POLICY_STORE",
        },
        NotFound {
            case: "IsAuthorized, unknown store",
            operation: "IsAuthorized",
            request: json!({
                "policyStoreId": unknown_store,
                "principal": { "entityType": "PhotoApp::User", "entityId": "alice" },
                "action": { "actionType": "PhotoApp::Action", "actionId": "view" },
                "resource": { "entityType": "PhotoApp::Photo", "entityId": "vacation" },
            }),
            reference: unknown_store.clone(),
            resource_id: unknown_store.clone(),
            resource_type: "POLICY_STORE",
        },
        NotFound {
            case: "CreatePolicyStoreAlias, unknown store",
            operation: "CreatePolicyStoreAlias",
            request: json!({
                "aliasName": format!("{ALIAS_PREFIX}orphan"),
                "policyStoreId": unknown_store,
            }),
            reference: unknown_store.clone(),
            resource_id: unknown_store.clone(),
            resource_type: "POLICY_STORE",
        },
        NotFound {
            case: "GetPolicyTemplate, unknown store",
            operation: "GetPolicyTemplate",
            request: json!({
                "policyStoreId": unknown_store,
                "policyTemplateId": unknown_template,
            }),
            reference: unknown_store.clone(),
            resource_id: unknown_store.clone(),
            resource_type: "POLICY_STORE",
        },
        NotFound {
            case: "GetPolicyTemplate, live store and unknown template id",
            operation: "GetPolicyTemplate",
            request: json!({
                "policyStoreId": live_store,
                "policyTemplateId": unknown_template,
            }),
            reference: unknown_template.clone(),
            resource_id: unknown_template.clone(),
            resource_type: "POLICY_TEMPLATE",
        },
        NotFound {
            case: "CreatePolicyTemplate, unknown store",
            operation: "CreatePolicyTemplate",
            request: json!({
                "policyStoreId": unknown_store,
                "statement": TEMPLATE_STATEMENT,
            }),
            reference: unknown_store.clone(),
            resource_id: unknown_store.clone(),
            resource_type: "POLICY_STORE",
        },
        NotFound {
            case: "UpdatePolicyStore, unknown store",
            operation: "UpdatePolicyStore",
            request: json!({
                "policyStoreId": unknown_store,
                "validationSettings": { "mode": "OFF" },
            }),
            reference: unknown_store.clone(),
            resource_id: unknown_store.clone(),
            resource_type: "POLICY_STORE",
        },
        NotFound {
            // As above: an alias reference reports the alias, by its bare name.
            case: "UpdatePolicyStore, alias reference pending deletion",
            operation: "UpdatePolicyStore",
            request: json!({
                "policyStoreId": pending_alias,
                "validationSettings": { "mode": "OFF" },
            }),
            reference: pending_alias.clone(),
            resource_id: "pending-deletion".to_string(),
            resource_type: "POLICY_STORE_ALIAS",
        },
        NotFound {
            case: "BatchIsAuthorized, unknown store",
            operation: "BatchIsAuthorized",
            request: json!({
                "policyStoreId": unknown_store,
                "requests": [{
                    "principal": { "entityType": "PhotoApp::User", "entityId": "alice" },
                    "action": { "actionType": "PhotoApp::Action", "actionId": "view" },
                    "resource": { "entityType": "PhotoApp::Photo", "entityId": "vacation" },
                }],
            }),
            reference: unknown_store.clone(),
            resource_id: unknown_store.clone(),
            resource_type: "POLICY_STORE",
        },
        NotFound {
            case: "CreateIdentitySource, unknown store",
            operation: "CreateIdentitySource",
            request: json!({
                "policyStoreId": unknown_store,
                "configuration": cognito_configuration(),
            }),
            reference: unknown_store.clone(),
            resource_id: unknown_store.clone(),
            resource_type: "POLICY_STORE",
        },
        NotFound {
            case: "GetIdentitySource, live store and unknown identity source id",
            operation: "GetIdentitySource",
            request: json!({
                "policyStoreId": live_store,
                "identitySourceId": unknown_identity_source,
            }),
            reference: unknown_identity_source.clone(),
            resource_id: unknown_identity_source.clone(),
            resource_type: "IDENTITY_SOURCE",
        },
        NotFound {
            case: "GetIdentitySource, unknown store",
            operation: "GetIdentitySource",
            request: json!({
                "policyStoreId": unknown_store,
                "identitySourceId": unknown_identity_source,
            }),
            reference: unknown_store.clone(),
            resource_id: unknown_store.clone(),
            resource_type: "POLICY_STORE",
        },
        NotFound {
            // An `aliasName` always names an alias, and its id is that name
            // without the documented `policy-store-alias/` resource path.
            case: "GetPolicyStoreAlias, unknown alias name",
            operation: "GetPolicyStoreAlias",
            request: json!({ "aliasName": unknown_alias }),
            reference: unknown_alias.clone(),
            resource_id: "no-such-alias".to_string(),
            resource_type: "POLICY_STORE_ALIAS",
        },
        NotFound {
            case: "ListTagsForResource, ARN of a store that never existed",
            operation: "ListTagsForResource",
            request: json!({ "resourceArn": fake_arn }),
            reference: fake_arn.clone(),
            resource_id: "PSFAKE0000000000000".to_string(),
            resource_type: "POLICY_STORE",
        },
        NotFound {
            case: "TagResource, ARN of a deleted store",
            operation: "TagResource",
            request: json!({ "resourceArn": gone_arn, "tags": { "env": "test" } }),
            reference: gone_arn.clone(),
            resource_id: gone_store.clone(),
            resource_type: "POLICY_STORE",
        },
        NotFound {
            // The taggable resource of this service is the policy store, so an
            // ARN that names anything else is a missing POLICY_STORE, and
            // `resourceId` is the id the ARN names — its last non-empty `/`- or
            // `:`-separated segment, per `crate::state::resource_id_from_arn`.
            case: "UntagResource, another service's ARN",
            operation: "UntagResource",
            request: json!({ "resourceArn": other_service_arn, "tagKeys": ["env"] }),
            reference: other_service_arn.clone(),
            resource_id: "some-queue".to_string(),
            resource_type: "POLICY_STORE",
        },
    ];

    let mut seen_messages: Vec<(&str, String)> = Vec::new();
    for NotFound {
        case,
        operation,
        request,
        reference,
        resource_id,
        resource_type,
    } in &rows
    {
        let (status, body) = call(&router, operation, request).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: unexpected error code, got {body}"
        );

        let message = fault_message(&body);
        assert_eq!(
            message, RESOURCE_NOT_FOUND_MESSAGE,
            "case {case:?}: expected the constant message {RESOURCE_NOT_FOUND_MESSAGE:?}, got {message:?}"
        );
        assert!(
            !message.contains(reference.as_str()),
            "case {case:?}: the message must not interpolate the reference {reference:?}, got {message:?}"
        );
        assert!(
            !message.contains(resource_id.as_str()),
            "case {case:?}: the message must not interpolate the resource id {resource_id:?}, got {message:?}"
        );

        // The identity of the missing resource is reported in the members that
        // model it, so nothing is lost by the message being a constant.
        assert_eq!(
            body["resourceId"].as_str().unwrap_or_default(),
            resource_id.as_str(),
            "case {case:?}: expected resourceId {resource_id:?}, got {body}"
        );
        assert_eq!(
            body["resourceType"].as_str().unwrap_or_default(),
            *resource_type,
            "case {case:?}: expected resourceType {resource_type:?}, got {body}"
        );
        assert!(
            RESOURCE_TYPE_VALUES.contains(resource_type),
            "case {case:?}: {resource_type:?} is not one of the modelled ResourceType values"
        );

        seen_messages.push((*case, message));
    }

    // One text across every row: the point of the rule is that a client can key
    // on it.
    let distinct: std::collections::BTreeSet<&str> = seen_messages
        .iter()
        .map(|(_, message)| message.as_str())
        .collect();
    assert_eq!(
        distinct.len(),
        1,
        "the not-found message must be ONE constant across every path, got {seen_messages:?}"
    );
}

/// A repeated not-found call answers byte-identically: the message carries
/// nothing that varies between two calls.
#[tokio::test]
async fn the_not_found_message_is_stable_across_repeated_calls() {
    let router = router();
    let request = json!({ "policyStoreId": "PSNONEXISTENT00000000" });

    let (_, first) = call(&router, "GetPolicyStore", &request).await;
    let (_, second) = call(&router, "GetPolicyStore", &request).await;
    assert_eq!(
        fault_message(&first),
        fault_message(&second),
        "case \"repeated GetPolicyStore\": the message must not vary between calls, got {first} then {second}"
    );
    assert_eq!(
        first, second,
        "case \"repeated GetPolicyStore\": the whole fault must be identical, got {first} then {second}"
    );
}

/// The deletion-protection refusal carries `InvalidStateException`'s documented
/// sentence, identifier-free, whatever else the protected store was created with
/// — and a store whose protection is `DISABLED` (explicitly or by default) is
/// deleted instead.
///
/// The rows differ along what else the create carried (description, tags,
/// validation mode) and along how deletion protection was set, which is the only
/// thing that decides the outcome.
#[tokio::test]
async fn the_deletion_protection_refusal_carries_the_documented_constant() {
    /// One row: the extra members a `CreatePolicyStore` carries, how deletion
    /// protection is set, and the status `DeletePolicyStore` must answer.
    struct Row {
        case: &'static str,
        protection: Option<&'static str>,
        extra: Value,
        status: u16,
    }

    let rows = &[
        Row {
            case: "protected, nothing else set",
            protection: Some("ENABLED"),
            extra: json!({}),
            status: 406,
        },
        Row {
            case: "protected, with a description",
            protection: Some("ENABLED"),
            extra: json!({ "description": "photos, production" }),
            status: 406,
        },
        Row {
            case: "protected, with tags",
            protection: Some("ENABLED"),
            extra: json!({ "tags": { "team": "photos", "env": "prod" } }),
            status: 406,
        },
        Row {
            case: "protected, with STRICT validation",
            protection: Some("ENABLED"),
            extra: json!({ "validationSettings": { "mode": "STRICT" } }),
            status: 406,
        },
        Row {
            case: "protection explicitly disabled",
            protection: Some("DISABLED"),
            extra: json!({}),
            status: 200,
        },
        Row {
            case: "protection left absent, which means DISABLED",
            protection: None,
            extra: json!({}),
            status: 200,
        },
    ];

    let router = router();
    let mut refusals: Vec<(&str, String)> = Vec::new();
    for Row {
        case,
        protection,
        extra,
        status: expected_status,
    } in rows
    {
        let mut request = json!({ "validationSettings": { "mode": "OFF" } });
        if let Some(protection) = protection {
            request["deletionProtection"] = json!(protection);
        }
        for (key, value) in extra.as_object().expect("a row's extra is an object") {
            request[key.as_str()] = value.clone();
        }

        let created = ok(&router, "CreatePolicyStore", &request).await;
        let store_id = created["policyStoreId"]
            .as_str()
            .expect("policyStoreId is required on the CreatePolicyStore response")
            .to_string();

        let (status, body) = call(
            &router,
            "DeletePolicyStore",
            &json!({ "policyStoreId": store_id }),
        )
        .await;
        assert_eq!(
            status, *expected_status,
            "case {case:?}: expected HTTP {expected_status} from DeletePolicyStore, got {status} {body}"
        );

        if *expected_status != 406 {
            // The store really went: protection off means the delete happened.
            let (get_status, get_body) = call(
                &router,
                "GetPolicyStore",
                &json!({ "policyStoreId": store_id }),
            )
            .await;
            assert_eq!(
                get_status, 404,
                "case {case:?}: an unprotected store must be gone after the delete, got {get_status} {get_body}"
            );
            continue;
        }

        assert_eq!(
            error_code(&body),
            "InvalidStateException",
            "case {case:?}: unexpected error code, got {body}"
        );
        let message = fault_message(&body);
        assert_eq!(
            message, DELETION_PROTECTION_MESSAGE,
            "case {case:?}: expected the documented refusal {DELETION_PROTECTION_MESSAGE:?}, got {message:?}"
        );
        assert!(
            !message.contains(store_id.as_str()),
            "case {case:?}: the refusal must not interpolate the store id {store_id:?}, got {message:?}"
        );

        // The refusal left the store in place, so the refusal is a refusal.
        let still_there = ok(
            &router,
            "GetPolicyStore",
            &json!({ "policyStoreId": store_id }),
        )
        .await;
        assert_eq!(
            still_there["policyStoreId"].as_str().unwrap_or_default(),
            store_id.as_str(),
            "case {case:?}: a refused delete must leave the store, got {still_there}"
        );

        refusals.push((*case, message));
    }

    let distinct: std::collections::BTreeSet<&str> = refusals
        .iter()
        .map(|(_, message)| message.as_str())
        .collect();
    assert_eq!(
        distinct.len(),
        1,
        "the deletion-protection refusal must be ONE constant across every protected store, got {refusals:?}"
    );
}

/// One conflict row: the request that collides, the values that must NOT be
/// spliced into the message, and the `(resourceId, resourceType)` entries the
/// required `resources` list must report.
struct Conflict {
    case: &'static str,
    operation: &'static str,
    request: Value,
    /// Values of the request, or of the state it collided with, that must not
    /// appear anywhere in the message.
    absent: Vec<String>,
    /// The entries `resources` must carry, in order.
    resources: Vec<(String, &'static str)>,
}

/// The `(resourceId, resourceType)` pairs a fault's `resources` list carries.
fn resource_entries(body: &Value) -> Vec<(String, String)> {
    body["resources"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .map(|entry| {
                    (
                        entry["resourceId"].as_str().unwrap_or_default().to_string(),
                        entry["resourceType"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A second, different Cedar statement, so a row that must collide on a NAME
/// does not also carry the first policy's text.
const OTHER_STATEMENT: &str = "forbid(principal, action, resource);";

/// Every conflicting path of this service answers ONE constant message, with
/// the collision reported in the required `resources` list.
///
/// The rows differ along the operation (`CreatePolicyStoreAlias`,
/// `CreatePolicyStore`, `CreatePolicy`, `CreatePolicyTemplate`,
/// `CreateIdentitySource`), along the CAUSE of the conflict (a name a live
/// alias of another store holds, a name awaiting deletion, a name another
/// resource in the store already carries, an idempotency token replayed with
/// different parameters), and along the TYPE of resource collided with
/// (`POLICY_STORE_ALIAS`, `POLICY_STORE`, `POLICY`, `POLICY_TEMPLATE`,
/// `IDENTITY_SOURCE`).
#[tokio::test]
async fn every_conflict_path_answers_the_same_constant_message() {
    let router = router();
    let store_a = store_with_mode(&router, "OFF").await;
    let store_b = store_with_mode(&router, "OFF").await;

    // A live alias of `store_a`: its name cannot be taken for another store.
    let held_name = "held-by-another-store";
    let held_alias = format!("{ALIAS_PREFIX}{held_name}");
    ok(
        &router,
        "CreatePolicyStoreAlias",
        &json!({ "aliasName": held_alias, "policyStoreId": store_a }),
    )
    .await;

    // An alias soft-deleted, so its name is held in `PendingDeletion`.
    let pending_name = "awaiting-deletion";
    let pending_alias = format!("{ALIAS_PREFIX}{pending_name}");
    ok(
        &router,
        "CreatePolicyStoreAlias",
        &json!({ "aliasName": pending_alias, "policyStoreId": store_a }),
    )
    .await;
    ok(
        &router,
        "DeletePolicyStoreAlias",
        &json!({ "aliasName": pending_alias }),
    )
    .await;

    // A store created under an idempotency token.
    let store_token = "11111111-1111-1111-1111-111111111111";
    let tokened_store = ok(
        &router,
        "CreatePolicyStore",
        &json!({
            "validationSettings": { "mode": "OFF" },
            "description": "first",
            "clientToken": store_token,
        }),
    )
    .await;
    let tokened_store_id = tokened_store["policyStoreId"]
        .as_str()
        .expect("policyStoreId is required on the CreatePolicyStore response")
        .to_string();

    // A named policy, and a policy created under a token, both in `store_a`.
    let policy_name = "duplicated-policy-name";
    let named_policy = ok(
        &router,
        "CreatePolicy",
        &json!({
            "policyStoreId": store_a,
            "name": policy_name,
            "definition": { "static": { "statement": STATEMENT } },
        }),
    )
    .await;
    let named_policy_id = named_policy["policyId"]
        .as_str()
        .expect("policyId is required on the CreatePolicy response")
        .to_string();
    let policy_token = "22222222-2222-2222-2222-222222222222";
    let tokened_policy = ok(
        &router,
        "CreatePolicy",
        &json!({
            "policyStoreId": store_a,
            "clientToken": policy_token,
            "definition": { "static": { "statement": OTHER_STATEMENT, "description": "first" } },
        }),
    )
    .await;
    let tokened_policy_id = tokened_policy["policyId"]
        .as_str()
        .expect("policyId is required on the CreatePolicy response")
        .to_string();

    // A named template, and a template created under a token.
    let template_name = "duplicated-template-name";
    let named_template = ok(
        &router,
        "CreatePolicyTemplate",
        &json!({
            "policyStoreId": store_a,
            "name": template_name,
            "statement": TEMPLATE_STATEMENT,
        }),
    )
    .await;
    let named_template_id = named_template["policyTemplateId"]
        .as_str()
        .expect("policyTemplateId is required on the CreatePolicyTemplate response")
        .to_string();
    let template_token = "33333333-3333-3333-3333-333333333333";
    let tokened_template = ok(
        &router,
        "CreatePolicyTemplate",
        &json!({
            "policyStoreId": store_a,
            "statement": TEMPLATE_STATEMENT,
            "description": "first",
            "clientToken": template_token,
        }),
    )
    .await;
    let tokened_template_id = tokened_template["policyTemplateId"]
        .as_str()
        .expect("policyTemplateId is required on the CreatePolicyTemplate response")
        .to_string();

    // An identity source created under a token.
    let source_token = "44444444-4444-4444-4444-444444444444";
    let tokened_source = ok(
        &router,
        "CreateIdentitySource",
        &json!({
            "policyStoreId": store_a,
            "configuration": cognito_configuration(),
            "principalEntityType": "MyCorp::User",
            "clientToken": source_token,
        }),
    )
    .await;
    let tokened_source_id = tokened_source["identitySourceId"]
        .as_str()
        .expect("identitySourceId is required on the CreateIdentitySource response")
        .to_string();

    let rows = vec![
        Conflict {
            case: "CreatePolicyStoreAlias, a name a live alias of another store holds",
            operation: "CreatePolicyStoreAlias",
            request: json!({ "aliasName": held_alias, "policyStoreId": store_b }),
            absent: vec![
                held_alias.clone(),
                held_name.to_string(),
                store_a.clone(),
                store_b.clone(),
            ],
            resources: vec![
                (held_name.to_string(), "POLICY_STORE_ALIAS"),
                (store_a.clone(), "POLICY_STORE"),
            ],
        },
        Conflict {
            case: "CreatePolicyStoreAlias, a name awaiting deletion, same store",
            operation: "CreatePolicyStoreAlias",
            request: json!({ "aliasName": pending_alias, "policyStoreId": store_a }),
            absent: vec![
                pending_alias.clone(),
                pending_name.to_string(),
                store_a.clone(),
                "PendingDeletion".to_string(),
            ],
            resources: vec![
                (pending_name.to_string(), "POLICY_STORE_ALIAS"),
                (store_a.clone(), "POLICY_STORE"),
            ],
        },
        Conflict {
            case: "CreatePolicyStoreAlias, a name awaiting deletion, another store",
            operation: "CreatePolicyStoreAlias",
            request: json!({ "aliasName": pending_alias, "policyStoreId": store_b }),
            absent: vec![
                pending_alias.clone(),
                pending_name.to_string(),
                store_b.clone(),
                "PendingDeletion".to_string(),
            ],
            resources: vec![
                (pending_name.to_string(), "POLICY_STORE_ALIAS"),
                (store_a.clone(), "POLICY_STORE"),
            ],
        },
        Conflict {
            case: "CreatePolicyStore, token replayed with a different description",
            operation: "CreatePolicyStore",
            request: json!({
                "validationSettings": { "mode": "OFF" },
                "description": "second",
                "clientToken": store_token,
            }),
            absent: vec![store_token.to_string(), tokened_store_id.clone()],
            resources: vec![(tokened_store_id.clone(), "POLICY_STORE")],
        },
        Conflict {
            case: "CreatePolicy, a name another policy in the store already carries",
            operation: "CreatePolicy",
            request: json!({
                "policyStoreId": store_a,
                "name": policy_name,
                "definition": { "static": { "statement": OTHER_STATEMENT } },
            }),
            absent: vec![
                policy_name.to_string(),
                named_policy_id.clone(),
                store_a.clone(),
            ],
            resources: vec![(named_policy_id.clone(), "POLICY")],
        },
        Conflict {
            case: "CreatePolicy, token replayed with a different description",
            operation: "CreatePolicy",
            request: json!({
                "policyStoreId": store_a,
                "clientToken": policy_token,
                "definition": {
                    "static": { "statement": OTHER_STATEMENT, "description": "second" },
                },
            }),
            absent: vec![policy_token.to_string(), tokened_policy_id.clone()],
            resources: vec![(tokened_policy_id.clone(), "POLICY")],
        },
        Conflict {
            case: "CreatePolicyTemplate, a name another template in the store already carries",
            operation: "CreatePolicyTemplate",
            request: json!({
                "policyStoreId": store_a,
                "name": template_name,
                "statement": TEMPLATE_STATEMENT,
            }),
            absent: vec![
                template_name.to_string(),
                named_template_id.clone(),
                store_a.clone(),
            ],
            resources: vec![(named_template_id.clone(), "POLICY_TEMPLATE")],
        },
        Conflict {
            case: "CreatePolicyTemplate, token replayed with a different description",
            operation: "CreatePolicyTemplate",
            request: json!({
                "policyStoreId": store_a,
                "statement": TEMPLATE_STATEMENT,
                "description": "second",
                "clientToken": template_token,
            }),
            absent: vec![template_token.to_string(), tokened_template_id.clone()],
            resources: vec![(tokened_template_id.clone(), "POLICY_TEMPLATE")],
        },
        Conflict {
            case: "CreateIdentitySource, token replayed with another principal entity type",
            operation: "CreateIdentitySource",
            request: json!({
                "policyStoreId": store_a,
                "configuration": cognito_configuration(),
                "principalEntityType": "MyCorp::Employee",
                "clientToken": source_token,
            }),
            absent: vec![source_token.to_string(), tokened_source_id.clone()],
            resources: vec![(tokened_source_id.clone(), "IDENTITY_SOURCE")],
        },
    ];

    let mut seen_messages: Vec<(&str, String)> = Vec::new();
    for Conflict {
        case,
        operation,
        request,
        absent,
        resources,
    } in &rows
    {
        let (status, body) = call(&router, operation, request).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected HTTP 409, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ConflictException",
            "case {case:?}: unexpected error code, got {body}"
        );

        let message = fault_message(&body);
        assert_eq!(
            message, CONFLICT_MESSAGE,
            "case {case:?}: expected the constant message {CONFLICT_MESSAGE:?}, got {message:?}"
        );
        for value in absent {
            assert!(
                !message.contains(value.as_str()),
                "case {case:?}: the message must not interpolate {value:?}, got {message:?}"
            );
        }

        // Nothing is lost by the message being a constant: the collision is in
        // the required `resources` list.
        let entries = resource_entries(&body);
        let expected: Vec<(String, String)> = resources
            .iter()
            .map(|(id, kind)| (id.clone(), (*kind).to_string()))
            .collect();
        assert_eq!(
            entries, expected,
            "case {case:?}: expected resources {expected:?}, got {body}"
        );
        for (resource_id, resource_type) in &entries {
            assert!(
                !resource_id.is_empty(),
                "case {case:?}: a conflict entry must carry a resourceId, got {body}"
            );
            assert!(
                RESOURCE_TYPE_VALUES.contains(&resource_type.as_str()),
                "case {case:?}: {resource_type:?} is not one of the modelled ResourceType values"
            );
        }

        seen_messages.push((*case, message));
    }

    // One text across every row: the point of the rule is that a client keys on
    // the code and reads the specifics from `resources`.
    let distinct: std::collections::BTreeSet<&str> = seen_messages
        .iter()
        .map(|(_, message)| message.as_str())
        .collect();
    assert_eq!(
        distinct.len(),
        1,
        "the conflict message must be ONE constant across every path, got {seen_messages:?}"
    );
}

/// The tag-limit refusal carries `TooManyTagsException`'s constant message and
/// names the resource in the modelled `resourceName` — whatever shape the merge
/// that would cross the limit took — while a merge that lands exactly ON the
/// published limit succeeds.
///
/// The rows differ along how the limit is crossed: in one request on an empty
/// resource (one key past it, and far past it), by one key on a full resource,
/// and by a merge mixing replacements of keys the resource holds with new ones.
#[tokio::test]
async fn the_tag_limit_refusal_carries_the_constant_message_and_names_the_resource() {
    /// One row: how many tags the resource already holds, the tags the request
    /// sends, and how many the merge would leave it with.
    struct Row {
        case: &'static str,
        /// Keys `key0..keyN` seeded on the resource before the request.
        seeded: usize,
        /// The request's tag map.
        tags: Value,
        /// What the merge would leave the resource holding.
        merged: usize,
    }

    /// `n` distinct tags, keyed `<prefix>0..<prefix>n-1`.
    fn tags(prefix: &str, n: usize) -> Value {
        Value::Object(
            (0..n)
                .map(|index| (format!("{prefix}{index}"), json!(format!("v{index}"))))
                .collect(),
        )
    }

    let rows = vec![
        Row {
            case: "one request, one key past the limit, on an empty resource",
            seeded: 0,
            tags: tags("key", MAX_TAGS_PER_RESOURCE + 1),
            merged: MAX_TAGS_PER_RESOURCE + 1,
        },
        Row {
            case: "one request, far past the limit, on an empty resource",
            seeded: 0,
            tags: tags("key", MAX_TAGS_PER_RESOURCE * 2),
            merged: MAX_TAGS_PER_RESOURCE * 2,
        },
        Row {
            case: "one new key on a resource already at the limit",
            seeded: MAX_TAGS_PER_RESOURCE,
            tags: json!({ "onemore": "v" }),
            merged: MAX_TAGS_PER_RESOURCE + 1,
        },
        Row {
            case: "a merge mixing a replacement with two new keys",
            seeded: MAX_TAGS_PER_RESOURCE - 1,
            tags: json!({ "key0": "replaced", "new-one": "v", "new-two": "v" }),
            merged: MAX_TAGS_PER_RESOURCE + 1,
        },
    ];

    let router = router();
    let mut refusals: Vec<(&str, String)> = Vec::new();
    for Row {
        case,
        seeded,
        tags: requested,
        merged,
    } in &rows
    {
        let store_id = store_with_mode(&router, "OFF").await;
        let arn = store_arn(&router, &store_id).await;
        if *seeded > 0 {
            ok(
                &router,
                "TagResource",
                &json!({ "resourceArn": arn, "tags": tags("key", *seeded) }),
            )
            .await;
        }

        let (status, body) = call(
            &router,
            "TagResource",
            &json!({ "resourceArn": arn, "tags": requested }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "TooManyTagsException",
            "case {case:?}: unexpected error code, got {body}"
        );

        let message = fault_message(&body);
        assert_eq!(
            message, TOO_MANY_TAGS_MESSAGE,
            "case {case:?}: expected the constant message {TOO_MANY_TAGS_MESSAGE:?}, \
             got {message:?}"
        );
        assert!(
            !message.contains(&merged.to_string()),
            "case {case:?}: the message must not interpolate the merged count {merged}, \
             got {message:?}"
        );
        assert!(
            !message.contains(arn.as_str()) && !message.contains(store_id.as_str()),
            "case {case:?}: the message must not interpolate the resource {arn:?}, \
             got {message:?}"
        );
        for key in requested
            .as_object()
            .expect("a row's tag map is an object")
            .keys()
        {
            assert!(
                !message.contains(key.as_str()),
                "case {case:?}: the message must not interpolate the key {key:?}, got {message:?}"
            );
        }

        // The resource at the limit is reported in the member that models it.
        assert_eq!(
            body["resourceName"].as_str().unwrap_or_default(),
            arn.as_str(),
            "case {case:?}: expected resourceName {arn:?}, got {body}"
        );

        // And the refusal is a refusal: the stored set is the seeded one.
        let listed = ok(
            &router,
            "ListTagsForResource",
            &json!({ "resourceArn": arn }),
        )
        .await;
        let kept = listed["tags"]
            .as_object()
            .map(serde_json::Map::len)
            .unwrap_or_default();
        assert_eq!(
            kept, *seeded,
            "case {case:?}: a refused merge must leave {seeded} tags, got {listed}"
        );

        refusals.push((*case, message));
    }

    // The published bound itself is accepted, which is what makes the rows above
    // refusals of crossing it rather than of the limit being off by one.
    let at_limit = store_with_mode(&router, "OFF").await;
    let at_limit_arn = store_arn(&router, &at_limit).await;
    let (status, body) = call(
        &router,
        "TagResource",
        &json!({ "resourceArn": at_limit_arn, "tags": tags("key", MAX_TAGS_PER_RESOURCE) }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"exactly at the published limit\": expected HTTP 200, got {status} {body}"
    );

    let distinct: std::collections::BTreeSet<&str> = refusals
        .iter()
        .map(|(_, message)| message.as_str())
        .collect();
    assert_eq!(
        distinct.len(),
        1,
        "the tag-limit refusal must be ONE constant across every merge, got {refusals:?}"
    );
}
