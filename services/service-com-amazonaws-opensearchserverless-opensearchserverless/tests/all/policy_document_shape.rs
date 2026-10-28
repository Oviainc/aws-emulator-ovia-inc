// SPDX-License-Identifier: CC0-1.0

//! A stored security policy document is read ONE way, wherever this service reads it.
//!
//! Every case drives the real request path through [`crate::collection_lifecycle::call`] —
//! `MultiServiceRouter::dispatch` with `POST /`, `X-Amz-Target:
//! OpenSearchServerless.<Operation>` and an `awsJson1_0` body — so the boundary validator,
//! the protocol codec and the handler all take part, exactly as they do for an SDK client.
//!
//! The rule under test: a security policy document's list-valued positions — the document
//! itself (one statement object, or a list of them), a statement's `Rules`, and a rule's
//! `Resource` — may each be written as a list or as the single value that list would hold,
//! and the single value denotes the one-element list. So the resources
//! `ListSecurityPolicies`' `resource` filter narrows on are the SAME resources the
//! encryption lookup `CreateCollection` performs reads out of that document: for one
//! stored policy, the filtered listing reports it exactly when its rules name a resource
//! the filter's patterns overlap, and a collection create with no `encryptionConfig` is
//! accepted exactly when a rule about collections names `collection/<name>`.
//!
//! The table spans what that rule generalises over: both spellings of each of the three
//! positions and their combinations, one statement and a list of statements, an exact and
//! a wildcard resource pattern, a rule about collections and one about indexes, the
//! AWS-owned key and a customer managed key, both `SecurityPolicyType` values, and a
//! `Rules` holding a value that is no rule at all.

use serde_json::{json, Value};

use crate::collection_lifecycle::{at, call, emulator, text};

/// A collection name no row's rules name, so every row owes the same answer for it: the
/// filtered listing leaves the policy out, and the create is refused.
const UNCOVERED_COLLECTION: &str = "elsewhere-coll";

/// The `kmsKeyArn` a collection reports when its encryption setting is the AWS-owned key.
const AWS_OWNED_KEY: &str = "auto";

/// A customer managed key ARN satisfying `EncryptionConfig$kmsKeyArn`'s modelled pattern.
fn customer_key() -> String {
    "arn:aws:kms:us-east-1:000000000000:key/12345678-1234-1234-1234-123456789012".to_string()
}

/// Every policy name `ListSecurityPolicies` reports for `kind` under the `resource` filter,
/// across pages.
async fn listed(
    router: &smithy_server_runtime::MultiServiceRouter,
    kind: &str,
    resource: Option<&[String]>,
) -> Vec<String> {
    let mut names = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut request = json!({ "type": kind, "maxResults": 100 });
        if let Some(resource) = resource {
            request["resource"] = json!(resource);
        }
        if let Some(token) = &token {
            request["nextToken"] = json!(token);
        }
        let case = request.to_string();
        let (status, code, answer) = call(router, "ListSecurityPolicies", request).await;
        assert_eq!(
            status, 200,
            "case ListSecurityPolicies({case}): expected 200, got {status} {code} with {answer}"
        );
        names.extend(
            at(&answer, "securityPolicySummaries")
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|summary| text(summary, "name")),
        );
        match at(&answer, "nextToken").as_str() {
            Some(next) => token = Some(next.to_string()),
            None => return names,
        }
    }
}

/// One row: a policy document spelling, the filter entries that must and must not select
/// it, and what the encryption lookup owes for the collections its rules do and do not
/// name.
struct ShapeCase {
    /// The spelling under test, named in every assertion of the row.
    case: &'static str,
    /// The `SecurityPolicyType` the policy is stored under.
    kind: &'static str,
    /// The policy document, as a caller writes it.
    policy: Value,
    /// Filter entries each of which must select this policy on its own.
    selecting: Vec<String>,
    /// Filter entries none of which may select it.
    rejecting: Vec<String>,
    /// A collection the policy's rules cover and the `kmsKeyArn` it must report, when the
    /// policy covers one for encryption.
    covered: Option<(&'static str, String)>,
}

#[tokio::test]
async fn one_policy_document_is_read_the_same_way_by_the_listing_filter_and_the_encryption_lookup()
{
    let key = customer_key();

    // The resource forms a rule names come from AWS's "Encryption policies for Amazon
    // OpenSearch Serverless" and "Network access for Amazon OpenSearch Serverless":
    // `collection/<name>` and `index/<collection>/<index>`, `*` standing for any run of
    // characters. Row 1 is the fully-listed spelling those pages write; every later row
    // writes a single value in one of the three list-valued positions.
    let cases: Vec<ShapeCase> = vec![
        ShapeCase {
            case: "Rules a list and Resource a list",
            kind: "encryption",
            policy: json!({
                "Rules": [{ "ResourceType": "collection", "Resource": ["collection/shape-both"] }],
                "AWSOwnedKey": true,
            }),
            selecting: vec![
                "collection/shape-both".to_string(),
                "collection/shape-b*".to_string(),
                "collection/*".to_string(),
            ],
            rejecting: vec!["collection/shape-bothx".to_string()],
            covered: Some(("shape-both", AWS_OWNED_KEY.to_string())),
        },
        ShapeCase {
            case: "Resource a single resource pattern",
            kind: "encryption",
            policy: json!({
                "Rules": [{ "ResourceType": "collection", "Resource": "collection/shape-scalar" }],
                "AWSOwnedKey": true,
            }),
            selecting: vec![
                "collection/shape-scalar".to_string(),
                "collection/*".to_string(),
            ],
            rejecting: vec!["collection/shape-scalarx".to_string()],
            covered: Some(("shape-scalar", AWS_OWNED_KEY.to_string())),
        },
        ShapeCase {
            case: "Rules a single rule object",
            kind: "encryption",
            policy: json!({
                "Rules": { "ResourceType": "collection", "Resource": ["collection/shape-rule"] },
                "AWSOwnedKey": true,
            }),
            selecting: vec![
                "collection/shape-rule".to_string(),
                "collection/shape-*".to_string(),
            ],
            rejecting: vec!["collection/other-rule".to_string()],
            covered: Some(("shape-rule", AWS_OWNED_KEY.to_string())),
        },
        ShapeCase {
            case: "Rules a single rule object and Resource a single pattern",
            kind: "encryption",
            policy: json!({
                "Rules": { "ResourceType": "collection", "Resource": "collection/shape-single" },
                "AWSOwnedKey": true,
            }),
            selecting: vec![
                "collection/shape-single".to_string(),
                "collection/*".to_string(),
            ],
            rejecting: vec!["collection/shape-singlex".to_string()],
            covered: Some(("shape-single", AWS_OWNED_KEY.to_string())),
        },
        ShapeCase {
            case: "the list document form with a single Resource pattern",
            kind: "encryption",
            policy: json!([{
                "Rules": [{ "ResourceType": "collection", "Resource": "collection/shape-list" }],
                "AWSOwnedKey": true,
            }]),
            selecting: vec![
                "collection/shape-list".to_string(),
                "collection/shape-l*".to_string(),
            ],
            rejecting: vec!["collection/shape-listx".to_string()],
            covered: Some(("shape-list", AWS_OWNED_KEY.to_string())),
        },
        ShapeCase {
            case: "a single wildcard Resource pattern assigning a customer managed key",
            kind: "encryption",
            policy: json!({
                "Rules": { "ResourceType": "collection", "Resource": "collection/shape-wild-*" },
                "KmsARN": key.clone(),
            }),
            selecting: vec![
                "collection/shape-wild-one".to_string(),
                "collection/shape-wild-*".to_string(),
                "collection/*".to_string(),
            ],
            rejecting: vec!["collection/shape-tame".to_string()],
            covered: Some(("shape-wild-one", key.clone())),
        },
        ShapeCase {
            case: "two statements, the second naming a single Resource pattern",
            kind: "encryption",
            policy: json!([
                {
                    "Rules": [{ "ResourceType": "collection", "Resource": ["collection/shape-first"] }],
                    "AWSOwnedKey": true,
                },
                {
                    "Rules": { "ResourceType": "collection", "Resource": "collection/shape-second" },
                    "KmsARN": key.clone(),
                },
            ]),
            selecting: vec![
                "collection/shape-first".to_string(),
                "collection/shape-second".to_string(),
                "collection/shape-*".to_string(),
            ],
            rejecting: vec!["collection/shape-third".to_string()],
            // The key sits on the statement that matched, not on the first statement.
            covered: Some(("shape-second", key.clone())),
        },
        ShapeCase {
            case: "a single index rule naming a single Resource pattern",
            kind: "encryption",
            policy: json!({
                "Rules": { "ResourceType": "index", "Resource": "index/shape-idx/app-*" },
                "AWSOwnedKey": true,
            }),
            selecting: vec![
                "index/shape-idx/app-2026".to_string(),
                "index/shape-idx/*".to_string(),
            ],
            // A rule about indexes names no collection, so a collection resource does not
            // meet it — in the listing or in the encryption lookup.
            rejecting: vec!["collection/shape-idx".to_string()],
            covered: None,
        },
        ShapeCase {
            case: "Rules holding a value that is no rule",
            kind: "encryption",
            policy: json!({
                "Rules": "collection/shape-nonsense",
                "AWSOwnedKey": true,
            }),
            selecting: Vec::new(),
            rejecting: vec![
                "collection/shape-nonsense".to_string(),
                "collection/*".to_string(),
            ],
            covered: None,
        },
        ShapeCase {
            case: "a network policy naming a single Resource pattern",
            kind: "network",
            policy: json!({
                "Rules": { "ResourceType": "collection", "Resource": "collection/shape-net" },
                "AllowFromPublic": true,
            }),
            selecting: vec![
                "collection/shape-net".to_string(),
                "collection/*".to_string(),
            ],
            rejecting: vec!["collection/shape-netx".to_string()],
            // A network policy says nothing about encryption, whichever spelling it uses.
            covered: None,
        },
    ];

    for ShapeCase {
        case,
        kind,
        policy,
        selecting,
        rejecting,
        covered,
    } in cases
    {
        // One engine per row, so a row's policy is the only stored policy and the answers
        // below are that document's alone.
        let router = emulator();
        let (status, code, answer) = call(
            &router,
            "CreateSecurityPolicy",
            json!({ "type": kind, "name": "shape-policy", "policy": policy.to_string() }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case CreateSecurityPolicy({case}): expected 200, got {status} {code} with {answer}"
        );

        // An unfiltered listing reports the policy whatever its document says.
        assert_eq!(
            listed(&router, kind, None).await,
            vec!["shape-policy".to_string()],
            "case ListSecurityPolicies({case}, no filter): the stored policy is reported"
        );

        for wanted in &selecting {
            let filter = [wanted.clone()];
            assert_eq!(
                listed(&router, kind, Some(&filter)).await,
                vec!["shape-policy".to_string()],
                "case ListSecurityPolicies({case}, resource=[{wanted}]): the policy's rules name a resource this filter overlaps, so it must be reported"
            );
        }
        // Every entry together selects it too, exactly when at least one does alone.
        if !selecting.is_empty() {
            assert_eq!(
                listed(&router, kind, Some(&selecting)).await,
                vec!["shape-policy".to_string()],
                "case ListSecurityPolicies({case}, resource={selecting:?}): a filter naming every matching resource reports the policy"
            );
        }

        let mut unmatched = rejecting;
        unmatched.push(format!("collection/{UNCOVERED_COLLECTION}"));
        for wanted in &unmatched {
            let filter = [wanted.clone()];
            assert!(
                listed(&router, kind, Some(&filter)).await.is_empty(),
                "case ListSecurityPolicies({case}, resource=[{wanted}]): the policy's rules name no resource this filter overlaps, so it must be left out"
            );
        }

        // The encryption lookup reads the same document: a create naming no
        // `encryptionConfig` is accepted exactly for a collection a rule about collections
        // names, and reports the key that rule's statement assigns.
        if let Some((name, expected_key)) = covered {
            let (status, code, answer) =
                call(&router, "CreateCollection", json!({ "name": name })).await;
            assert_eq!(
                status, 200,
                "case CreateCollection({case}, {name}): the listing reports the policy for this resource, so the create it covers must be accepted; got {status} {code} with {answer}"
            );
            let detail = at(&answer, "createCollectionDetail").clone();
            assert_eq!(
                text(&detail, "kmsKeyArn"),
                expected_key,
                "case CreateCollection({case}, {name}): expected kmsKeyArn {expected_key}, got {detail}"
            );
            // The stored record answers the same key, so it is not a rendering of this one
            // request.
            let (status, code, read) = call(
                &router,
                "BatchGetCollection",
                json!({ "names": [name] }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case BatchGetCollection({case}, {name}): expected 200, got {status} {code} with {read}"
            );
            let stored = at(&read, "collectionDetails")
                .as_array()
                .and_then(|details| details.first())
                .cloned()
                .unwrap_or(Value::Null);
            assert_eq!(
                text(&stored, "kmsKeyArn"),
                expected_key,
                "case BatchGetCollection({case}, {name}): the read-back reports the same key, got {stored}"
            );
        }

        // Every concrete `collection/<name>` the filtered listing left this policy out of
        // is a collection the encryption lookup does not cover either, so the create is
        // refused — the two readings agree on what the document does NOT name as well. A
        // wildcard entry names no single collection, so it is not one of these.
        let refused: Vec<String> = unmatched
            .iter()
            .filter(|entry| !entry.contains('*'))
            .filter_map(|entry| entry.strip_prefix("collection/").map(str::to_string))
            .collect();
        for name in refused {
            let (status, code, answer) =
                call(&router, "CreateCollection", json!({ "name": name })).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "ValidationException"),
                "case CreateCollection({case}, {name}): no stored policy names this collection, so the create must be refused; got {status} {code} with {answer}"
            );
        }
    }
}
