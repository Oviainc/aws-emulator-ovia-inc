// SPDX-License-Identifier: CC0-1.0

//! The encryption-policy PRECONDITION a collection create carries, end to end over the
//! wire: a create whose collection has no encryption setting at all is refused with
//! `ValidationException`, nothing is stored for it, and creating the covering `encryption`
//! security policy — and only that — lets the identical create through to `ACTIVE`.
//!
//! Every case drives the real request path — `MultiServiceRouter::dispatch` with `POST /`,
//! `X-Amz-Target: OpenSearchServerless.<Operation>` and an `awsJson1_0` body (the wire
//! contract in this crate's `OPERATIONS.md`) — so the boundary validator, the protocol
//! codec and the handler all take part, exactly as they do for an SDK client.
//!
//! The tables below span what the rule generalises over: the collection name (both ends of
//! `CollectionName`'s modelled `@length(3..64)`, and a name carrying digits and hyphens),
//! the collection type (every `CollectionType` value), the other members the create may
//! carry, and the FORM of the rule that covers the name once the policy exists (an exact
//! resource, a trailing wildcard, the catch-all) — beside a table of near-miss policies
//! that leave the name uncovered.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::collection_lifecycle::{at, call, emulator, text, CLIENT_ERROR};

/// The error code the refusal carries (`CreateCollection` errors ->
/// `ValidationException`, `@httpError` 400).
const VALIDATION: &str = "ValidationException";

/// The `errorCode` `BatchGetCollection` reports for a selector naming no stored
/// collection, from the model's `CollectionErrorDetail`.
const NOT_FOUND: &str = "NOT_FOUND";

/// The non-terminal status a collection is created in, and the terminal one it settles
/// into on its own (`CollectionStatus` -> `CREATING` … `ACTIVE`).
const CREATING: &str = "CREATING";
const ACTIVE: &str = "ACTIVE";

/// The `kmsKeyArn` a collection encrypted with the AWS-owned key reports.
const AWS_OWNED_KEY: &str = "auto";

/// How this refusal class is worded: the whole sentence pair is fixed text, and only the
/// collection name varies — it is the request's own `name` member, verbatim.
fn missing_encryption_policy_message(name: &str) -> String {
    format!(
        "No matching security policy of encryption type found for collection name: {name}. \
         Please create security policy of encryption type for this collection."
    )
}

/// An `encryption` policy document covering `resources`, assigning the AWS-owned key.
///
/// The shape is the one AWS's "Encryption policies for Amazon OpenSearch Serverless"
/// documents: `Rules` of `ResourceType` `collection` over `collection/<name>` patterns,
/// beside `AWSOwnedKey` or `KmsARN`.
fn encryption_policy(resources: &[String]) -> Value {
    json!({
        "Rules": [{ "ResourceType": "collection", "Resource": resources }],
        "AWSOwnedKey": true,
    })
}

/// Create a security policy of `kind`, asserting it was accepted.
async fn create_policy(router: &MultiServiceRouter, kind: &str, name: &str, policy: Value) {
    let (status, code, answer) = call(
        router,
        "CreateSecurityPolicy",
        json!({ "type": kind, "name": name, "policy": policy.to_string() }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case CreateSecurityPolicy({kind}/{name}): expected 200, got {status} {code} with {answer}"
    );
}

/// The whole `BatchGetCollection` answer for the single `names` selector `name`.
async fn batch_get_by_name(router: &MultiServiceRouter, name: &str) -> Value {
    let (status, code, answer) = call(router, "BatchGetCollection", json!({ "names": [name] })).await;
    assert_eq!(
        status, 200,
        "case BatchGetCollection(names=[{name}]): expected 200, got {status} {code} with {answer}"
    );
    answer
}

/// Every collection name `ListCollections` reports.
async fn listed_names(router: &MultiServiceRouter) -> Vec<String> {
    let (status, code, answer) = call(router, "ListCollections", json!({})).await;
    assert_eq!(
        status, 200,
        "case ListCollections(): expected 200, got {status} {code} with {answer}"
    );
    at(&answer, "collectionSummaries")
        .as_array()
        .map(|summaries| {
            summaries
                .iter()
                .map(|summary| text(summary, "name"))
                .collect()
        })
        .unwrap_or_default()
}

/// Assert that `name` names no collection at all: neither batch read nor listing reports
/// one, and the batch read answers the modelled `NOT_FOUND` detail for the selector.
async fn assert_nothing_created(router: &MultiServiceRouter, case: &str, name: &str) {
    let answer = batch_get_by_name(router, name).await;
    let details = at(&answer, "collectionDetails")
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        details.is_empty(),
        "case {case}: the refused create stored nothing, so BatchGetCollection(names=[{name}]) \
         must report no collectionDetails, got {details:?}"
    );
    let errors = at(&answer, "collectionErrorDetails")
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        errors
            .iter()
            .any(|error| text(error, "name") == name && text(error, "errorCode") == NOT_FOUND),
        "case {case}: BatchGetCollection(names=[{name}]) must report the {NOT_FOUND} detail for \
         the name, got {errors:?}"
    );
    let listed = listed_names(router).await;
    assert!(
        !listed.contains(&name.to_string()),
        "case {case}: ListCollections must not report {name}, got {listed:?}"
    );
}

/// Call `CreateCollection` with `request` and assert it was refused exactly as this class
/// of refusal is worded.
async fn assert_refused(router: &MultiServiceRouter, case: &str, name: &str, request: Value) {
    let (status, code, answer) = call(router, "CreateCollection", request).await;
    assert_eq!(
        (status, code.as_str()),
        (CLIENT_ERROR, VALIDATION),
        "case {case}: expected {CLIENT_ERROR} {VALIDATION}, got {status} {code} with {answer}"
    );
    assert_eq!(
        text(&answer, "message"),
        missing_encryption_policy_message(name),
        "case {case}: the refusal's message must name the requested collection, got {answer}"
    );
    assert_nothing_created(router, case, name).await;
}

/// Poll `BatchGetCollection` until `name` reports `ACTIVE`, answering its detail. The
/// transition is owed by the clock, so this waits for the wall clock rather than for a
/// call.
async fn await_active(router: &MultiServiceRouter, case: &str, name: &str) -> Value {
    for _ in 0..20 {
        let answer = batch_get_by_name(router, name).await;
        let detail = at(&answer, "collectionDetails")
            .as_array()
            .and_then(|details| details.first())
            .cloned()
            .unwrap_or(Value::Null);
        if text(&detail, "status") == ACTIVE {
            return detail;
        }
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    }
    panic!("case {case}: the collection {name} never reached {ACTIVE}");
}

/// A name at `CollectionName`'s modelled `@length` maximum of 64, satisfying its
/// `^[a-z][a-z0-9-]+$` pattern.
fn longest_name() -> String {
    let name = format!("z{}", "a".repeat(63));
    assert_eq!(
        name.len(),
        64,
        "the longest-name row must sit on CollectionName's modelled maximum, got {name:?}"
    );
    name
}

/// One row of the precondition table: the create request the row drives twice, and the
/// `Resource` patterns of the encryption policy created between the two calls.
struct PreconditionCase {
    case: &'static str,
    name: String,
    /// Members the create carries beside `name`, so the precondition is shown to hold
    /// whatever else the request says.
    extra: Value,
    covering: Vec<String>,
}

#[tokio::test]
async fn a_create_is_refused_until_an_encryption_policy_covers_its_name() {
    let longest = longest_name();
    let cases = vec![
        PreconditionCase {
            case: "a VECTORSEARCH collection covered by an exact resource",
            name: "kb-vectors".to_string(),
            extra: json!({ "type": "VECTORSEARCH" }),
            covering: vec!["collection/kb-vectors".to_string()],
        },
        PreconditionCase {
            case: "the shortest name the model allows, covered by a trailing wildcard",
            name: "abc".to_string(),
            extra: json!({ "type": "SEARCH" }),
            covering: vec!["collection/ab*".to_string()],
        },
        PreconditionCase {
            case: "the longest name the model allows, covered by the catch-all",
            name: longest.clone(),
            extra: json!({ "type": "TIMESERIES", "description": "a long-named collection" }),
            covering: vec!["collection/*".to_string()],
        },
        PreconditionCase {
            case: "a name carrying digits and hyphens, covered by a prefix wildcard",
            name: "logs-2026-q1".to_string(),
            extra: json!({
                "type": "SEARCH",
                "standbyReplicas": "DISABLED",
                "deletionProtection": "DISABLED",
                "tags": [{ "key": "team", "value": "search" }],
            }),
            covering: vec!["collection/logs-*".to_string()],
        },
        PreconditionCase {
            case: "a name covered by the second resource of a two-resource rule",
            name: "vectors-two".to_string(),
            extra: json!({ "type": "VECTORSEARCH", "vectorOptions": { "ServerlessVectorAcceleration": "ENABLED" } }),
            covering: vec![
                "collection/vectors-one".to_string(),
                "collection/vectors-two".to_string(),
            ],
        },
    ];

    for PreconditionCase {
        case,
        name,
        extra,
        covering,
    } in cases
    {
        // Each row gets its own store, so one row's covering policy cannot admit another's
        // create.
        let router = emulator();
        let mut request = json!({ "name": name });
        for (member, value) in extra.as_object().expect("the extra members are an object") {
            request[member] = value.clone();
        }

        assert_refused(&router, case, &name, request.clone()).await;

        // The encryption policy is the ONLY thing added between the two identical calls.
        create_policy(
            &router,
            "encryption",
            "covering-policy",
            encryption_policy(&covering),
        )
        .await;

        let (status, code, answer) = call(&router, "CreateCollection", request).await;
        assert_eq!(
            status, 200,
            "case {case}: with the covering encryption policy the identical create is \
             accepted, got {status} {code} with {answer}"
        );
        let detail = at(&answer, "createCollectionDetail").clone();
        assert_eq!(
            text(&detail, "name"),
            name,
            "case {case}: the created collection carries the requested name, got {detail}"
        );
        assert_eq!(
            text(&detail, "status"),
            CREATING,
            "case {case}: a fresh collection is reported in its non-terminal status, got {detail}"
        );
        assert_eq!(
            text(&detail, "kmsKeyArn"),
            AWS_OWNED_KEY,
            "case {case}: the covering policy assigns the AWS-owned key, got {detail}"
        );

        let settled = await_active(&router, case, &name).await;
        assert_eq!(
            text(&settled, "id"),
            text(&detail, "id"),
            "case {case}: the settled collection is the one the create minted, got {settled}"
        );
        assert_eq!(
            text(&settled, "kmsKeyArn"),
            AWS_OWNED_KEY,
            "case {case}: the read-back reports the same key, got {settled}"
        );
    }
}

/// One row of the near-miss table: a policy the store holds that does NOT cover
/// `collection/<name>`, so the create is still refused and still stores nothing.
struct NearMissCase {
    case: &'static str,
    /// The `SecurityPolicyType` of the stored policy (`encryption` | `network`).
    kind: &'static str,
    policy: Value,
}

#[tokio::test]
async fn a_policy_that_does_not_cover_the_name_leaves_the_create_refused() {
    // The collection every row below asks for; no row's policy denotes it.
    let name = "kb-vectors";
    let cases = vec![
        NearMissCase {
            case: "an encryption policy naming a different collection",
            kind: "encryption",
            policy: encryption_policy(&["collection/other-collection".to_string()]),
        },
        NearMissCase {
            case: "an encryption policy whose wildcard starts elsewhere",
            kind: "encryption",
            policy: encryption_policy(&["collection/logs-*".to_string()]),
        },
        NearMissCase {
            case: "an encryption policy whose rule is about indexes",
            kind: "encryption",
            policy: json!({
                "Rules": [{ "ResourceType": "index", "Resource": [format!("index/{name}/*")] }],
                "AWSOwnedKey": true,
            }),
        },
        NearMissCase {
            case: "an encryption policy with no rules at all",
            kind: "encryption",
            policy: json!({ "Rules": [], "AWSOwnedKey": true }),
        },
        NearMissCase {
            case: "a NETWORK policy covering the very name",
            kind: "network",
            policy: json!([{
                "Rules": [{ "ResourceType": "collection", "Resource": [format!("collection/{name}")] }],
                "AllowFromPublic": true,
            }]),
        },
    ];

    for NearMissCase { case, kind, policy } in cases {
        let router = emulator();
        create_policy(&router, kind, "near-miss-policy", policy).await;
        assert_refused(
            &router,
            case,
            name,
            json!({ "name": name, "type": "VECTORSEARCH" }),
        )
        .await;
    }
}
