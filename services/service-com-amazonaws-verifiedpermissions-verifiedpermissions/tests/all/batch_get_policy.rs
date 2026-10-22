// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for `BatchGetPolicy`.
//!
//! The rule under test: a batch read answers EVERY request item on its own and in
//! the request's order — an item whose references resolve is reported in `results`
//! with the persisted policy, and an item whose reference names nothing is reported
//! in `errors` with the `BatchGetPolicyErrorCode` for WHICH of its references
//! missed, so one miss never costs the other items their answer.
//!
//! Everything goes over the wire through the same `MultiServiceRouter` the emulator
//! binary serves, on the shared harness in
//! [`super::policy_store_authorization`], so a value only appears in a response if
//! it really reached state.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::policy_store_authorization::{call, error_code, ok, router, store_with_mode};

/// One policy the fixture stores, and what a batch answer about it must say.
struct Stored {
    store_id: String,
    policy_id: String,
    name: Option<&'static str>,
    description: Option<&'static str>,
    statement: &'static str,
}

/// How one request item addresses its policy — the dimension the rule generalises
/// over.
#[derive(Debug)]
enum Reference {
    /// Both the store and the policy by their minted ids.
    ById(&'static str),
    /// The store by its id, the policy by `name/<policyName>` (the model documents
    /// a policy name as usable in place of the id).
    ByPolicyName(&'static str),
    /// A store id no policy store has.
    UnknownStore,
    /// A real store, and a policy id it does not hold.
    UnknownPolicy(&'static str),
    /// A real store and a real policy — but the policy belongs to the OTHER store.
    OtherStoresPolicy {
        store_of: &'static str,
        policy_of: &'static str,
    },
    /// The named policy's store addressed through an ACTIVE
    /// `policy-store-alias/<name>`, the policy by its id.
    ByActiveAlias(&'static str),
    /// A `policy-store-alias/<name>` that exists but is `PendingDeletion`, which
    /// `DeletePolicyStoreAlias` documents as no longer resolving.
    PendingDeletionAlias,
    /// `policy-store-alias/<name>` naming no alias.
    UnknownAlias,
}

/// What the batch must answer for one request item.
#[derive(Debug)]
enum Expect {
    /// Reported in `results`, as the named fixture policy.
    Hit(&'static str),
    /// Reported in `errors` under this code. The values are the three
    /// `BatchGetPolicyErrorCode` members the model lists.
    Miss(&'static str),
}

/// One row of the fixture table.
struct FixtureRow {
    label: &'static str,
    /// Index into the fixture's two stores.
    store: usize,
    name: Option<&'static str>,
    description: Option<&'static str>,
    statement: &'static str,
}

/// The fixture: two stores, three policies, spanning present/absent `name` and
/// `description` and both Cedar effects.
const FIXTURE: &[FixtureRow] = &[
    FixtureRow {
        label: "named-and-described",
        store: 0,
        name: Some("alpha-policy"),
        description: Some("the first policy"),
        statement:
            "permit(principal == User::\"alice\", action == Action::\"view\", resource == Photo::\"a\");",
    },
    FixtureRow {
        label: "anonymous",
        store: 0,
        name: None,
        description: None,
        statement:
            "forbid(principal == User::\"bob\", action == Action::\"edit\", resource == Photo::\"b\");",
    },
    FixtureRow {
        label: "in-the-second-store",
        store: 1,
        name: Some("beta-policy"),
        description: None,
        statement: "permit(principal, action, resource);",
    },
];

/// Everything one test's fixture holds.
struct Fixture {
    /// The two policy store ids.
    stores: [String; 2],
    /// The stored policies, by [`FixtureRow::label`].
    policies: BTreeMap<&'static str, Stored>,
    /// An `Active` alias naming `stores[0]`.
    active_alias: String,
    /// An alias naming `stores[1]`, soft-deleted so it is `PendingDeletion`.
    pending_alias: String,
}

/// Build the fixture: two stores, the policies above, and the two aliases.
async fn fixture(router: &MultiServiceRouter) -> Fixture {
    let stores = [
        store_with_mode(router, "OFF").await,
        store_with_mode(router, "OFF").await,
    ];
    let mut policies = BTreeMap::new();
    for row in FIXTURE {
        let store_id = stores[row.store].clone();
        let mut definition = json!({ "statement": row.statement });
        if let Some(description) = row.description {
            definition["description"] = json!(description);
        }
        let mut request = json!({
            "policyStoreId": store_id,
            "definition": { "static": definition },
        });
        if let Some(name) = row.name {
            request["name"] = json!(name);
        }
        let created = ok(router, "CreatePolicy", &request).await;
        policies.insert(
            row.label,
            Stored {
                store_id,
                policy_id: created["policyId"]
                    .as_str()
                    .expect("CreatePolicy returns a policyId")
                    .to_string(),
                name: row.name,
                description: row.description,
                statement: row.statement,
            },
        );
    }

    // An alias name carries the `policy-store-alias/` prefix, which is what
    // `CreatePolicyStoreAlias` requires and what a `policyStoreId` reference uses.
    let active_alias = "policy-store-alias/first-store".to_string();
    let pending_alias = "policy-store-alias/second-store".to_string();
    ok(
        router,
        "CreatePolicyStoreAlias",
        &json!({ "aliasName": active_alias, "policyStoreId": stores[0] }),
    )
    .await;
    ok(
        router,
        "CreatePolicyStoreAlias",
        &json!({ "aliasName": pending_alias, "policyStoreId": stores[1] }),
    )
    .await;
    // A soft delete leaves the alias in `PendingDeletion`, which is a state a
    // `policyStoreId` reference no longer resolves through.
    ok(
        router,
        "DeletePolicyStoreAlias",
        &json!({ "aliasName": pending_alias, "deletionMode": "SoftDelete" }),
    )
    .await;

    Fixture {
        stores,
        policies,
        active_alias,
        pending_alias,
    }
}

/// The `{policyStoreId, policyId}` a `Reference` puts on the wire.
fn request_item(reference: &Reference, fixture: &Fixture) -> Value {
    let stored = &fixture.policies;
    let (store_ref, policy_ref) = match reference {
        Reference::ById(label) => (
            stored[label].store_id.clone(),
            stored[label].policy_id.clone(),
        ),
        Reference::ByPolicyName(label) => (
            stored[label].store_id.clone(),
            format!(
                "name/{}",
                stored[label]
                    .name
                    .expect("a ByPolicyName row names a policy that has a name")
            ),
        ),
        Reference::UnknownStore => (
            "PS0000000000000000000000".to_string(),
            "SP0000000000000000000000".to_string(),
        ),
        Reference::UnknownPolicy(label) => (
            stored[label].store_id.clone(),
            "SP0000000000000000000000".to_string(),
        ),
        Reference::OtherStoresPolicy {
            store_of,
            policy_of,
        } => (
            stored[store_of].store_id.clone(),
            stored[policy_of].policy_id.clone(),
        ),
        Reference::ByActiveAlias(label) => (
            fixture.active_alias.clone(),
            stored[label].policy_id.clone(),
        ),
        Reference::PendingDeletionAlias => (
            fixture.pending_alias.clone(),
            "SP0000000000000000000000".to_string(),
        ),
        Reference::UnknownAlias => (
            "policy-store-alias/no-such-alias".to_string(),
            "SP0000000000000000000000".to_string(),
        ),
    };
    json!({ "policyStoreId": store_ref, "policyId": policy_ref })
}

/// Every item is answered on its own, in the request's order, with a hit in
/// `results` and a miss in `errors` under the code for the reference that missed.
#[tokio::test]
async fn batch_get_policy_answers_every_request_item_in_order() {
    // The rows differ along COUNT (one item, several), the proportion of hits to
    // misses, the INTERLEAVING of hits and misses, which reference misses, how a
    // hit addresses its store and policy (ids, an active alias,
    // `name/<policyName>`), whether the items span one store or two, and whether
    // an item is repeated.
    let rows: &[(&str, &[(Reference, Expect)])] = &[
        (
            "a single hit",
            &[(Reference::ById("named-and-described"), Expect::Hit("named-and-described"))],
        ),
        (
            "a single miss",
            &[(Reference::UnknownStore, Expect::Miss("POLICY_STORE_NOT_FOUND"))],
        ),
        (
            "every policy, across both stores",
            &[
                (Reference::ById("named-and-described"), Expect::Hit("named-and-described")),
                (Reference::ById("anonymous"), Expect::Hit("anonymous")),
                (Reference::ById("in-the-second-store"), Expect::Hit("in-the-second-store")),
            ],
        ),
        (
            "a hit addressed by policy name",
            &[
                (Reference::ByPolicyName("named-and-described"), Expect::Hit("named-and-described")),
            ],
        ),
        (
            "miss, hit, miss — the hit still answers",
            &[
                (Reference::UnknownStore, Expect::Miss("POLICY_STORE_NOT_FOUND")),
                (Reference::ById("anonymous"), Expect::Hit("anonymous")),
                (Reference::UnknownPolicy("anonymous"), Expect::Miss("POLICY_NOT_FOUND")),
            ],
        ),
        (
            "hit, miss, hit — order is the request's, not the outcome's",
            &[
                (Reference::ById("in-the-second-store"), Expect::Hit("in-the-second-store")),
                (Reference::UnknownAlias, Expect::Miss("POLICY_STORE_ALIAS_NOT_FOUND")),
                (Reference::ById("named-and-described"), Expect::Hit("named-and-described")),
            ],
        ),
        (
            "each of the three error codes at once",
            &[
                (Reference::UnknownStore, Expect::Miss("POLICY_STORE_NOT_FOUND")),
                (Reference::UnknownPolicy("named-and-described"), Expect::Miss("POLICY_NOT_FOUND")),
                (Reference::UnknownAlias, Expect::Miss("POLICY_STORE_ALIAS_NOT_FOUND")),
            ],
        ),
        (
            "a policy is scoped to its own store",
            &[
                (
                    Reference::OtherStoresPolicy {
                        store_of: "in-the-second-store",
                        policy_of: "anonymous",
                    },
                    Expect::Miss("POLICY_NOT_FOUND"),
                ),
            ],
        ),
        (
            "a hit addressed through an active alias",
            &[
                (Reference::ByActiveAlias("named-and-described"), Expect::Hit("named-and-described")),
                (Reference::ByActiveAlias("anonymous"), Expect::Hit("anonymous")),
            ],
        ),
        (
            "an alias in PendingDeletion no longer resolves",
            &[
                (Reference::PendingDeletionAlias, Expect::Miss("POLICY_STORE_ALIAS_NOT_FOUND")),
            ],
        ),
        (
            "a repeated item is answered once per occurrence",
            &[
                (Reference::ById("anonymous"), Expect::Hit("anonymous")),
                (Reference::ById("anonymous"), Expect::Hit("anonymous")),
                (Reference::UnknownAlias, Expect::Miss("POLICY_STORE_ALIAS_NOT_FOUND")),
                (Reference::UnknownAlias, Expect::Miss("POLICY_STORE_ALIAS_NOT_FOUND")),
            ],
        ),
    ];

    let router = router();
    let fixture = fixture(&router).await;
    let stored = &fixture.policies;

    // Precondition for the "a policy is scoped to its own store" row: the policy
    // it addresses THROUGH the other store must be an id that store really does
    // not hold, or the row would prove nothing.
    let listed = ok(
        &router,
        "ListPolicies",
        &json!({ "policyStoreId": stored["in-the-second-store"].store_id }),
    )
    .await;
    let other_store_holds_it = listed["policies"].as_array().is_some_and(|policies| {
        policies.iter().any(|policy| {
            policy["policyId"].as_str() == Some(stored["anonymous"].policy_id.as_str())
        })
    });
    assert!(
        !other_store_holds_it,
        "precondition: the second store must not hold the first store's policy id, got {listed}"
    );

    for (case, items) in rows {
        let requests: Vec<Value> = items
            .iter()
            .map(|(reference, _)| request_item(reference, &fixture))
            .collect();
        let answer = ok(&router, "BatchGetPolicy", &json!({ "requests": requests })).await;

        // Both members are `required`, so both are always present — an all-hit
        // call still carries an empty `errors` list, and the other way round.
        let results = answer["results"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: results is required, got {answer}"))
            .clone();
        let errors = answer["errors"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: errors is required, got {answer}"))
            .clone();

        let expected_hits: Vec<&&str> = items
            .iter()
            .filter_map(|(_, expect)| match expect {
                Expect::Hit(label) => Some(label),
                Expect::Miss(_) => None,
            })
            .collect();
        let expected_misses: Vec<(&&str, &Value)> = items
            .iter()
            .zip(requests.iter())
            .filter_map(|((_, expect), request)| match expect {
                Expect::Miss(code) => Some((code, request)),
                Expect::Hit(_) => None,
            })
            .collect();

        assert_eq!(
            results.len(),
            expected_hits.len(),
            "case {case:?}: expected {} results, got {answer}",
            expected_hits.len()
        );
        assert_eq!(
            errors.len(),
            expected_misses.len(),
            "case {case:?}: expected {} errors, got {answer}",
            expected_misses.len()
        );

        for (position, (label, result)) in expected_hits.iter().zip(results.iter()).enumerate() {
            let policy = &stored[**label];
            assert_eq!(
                result["policyStoreId"].as_str(),
                Some(policy.store_id.as_str()),
                "case {case:?}: result {position} must report the RESOLVED policyStoreId, got {result}"
            );
            assert_eq!(
                result["policyId"].as_str(),
                Some(policy.policy_id.as_str()),
                "case {case:?}: result {position} must report the RESOLVED policyId, got {result}"
            );
            assert_eq!(
                result["policyType"].as_str(),
                Some("STATIC"),
                "case {case:?}: result {position} has the wrong policyType, got {result}"
            );
            assert_eq!(
                result["definition"]["static"]["statement"].as_str(),
                Some(policy.statement),
                "case {case:?}: result {position} must give the statement back byte-identical, got {result}"
            );
            assert_eq!(
                result["definition"]["static"]["description"].as_str(),
                policy.description,
                "case {case:?}: result {position} has the wrong description, got {result}"
            );
            assert_eq!(
                result["name"].as_str(),
                policy.name,
                "case {case:?}: result {position} has the wrong name, got {result}"
            );
            // `BatchGetPolicyOutputItem` models no `effect`, `principal`,
            // `resource` or `actions`, so a batch answer must not invent them.
            for absent in ["effect", "principal", "resource", "actions"] {
                assert!(
                    result[absent].is_null(),
                    "case {case:?}: result {position} carries {absent}, which the model does not put on a BatchGetPolicyOutputItem, got {result}"
                );
            }
            for required in ["createdDate", "lastUpdatedDate"] {
                assert!(
                    result[required].is_number() || result[required].is_string(),
                    "case {case:?}: result {position} is missing {required}, got {result}"
                );
            }
        }

        for (position, ((code, request), error)) in
            expected_misses.iter().zip(errors.iter()).enumerate()
        {
            assert_eq!(
                error["code"].as_str(),
                Some(**code),
                "case {case:?}: error {position} has the wrong code, got {error}"
            );
            // The error echoes the item's OWN references, so a caller can pair
            // the error with what it asked about.
            assert_eq!(
                error["policyStoreId"], request["policyStoreId"],
                "case {case:?}: error {position} must echo the request's policyStoreId, got {error}"
            );
            assert_eq!(
                error["policyId"], request["policyId"],
                "case {case:?}: error {position} must echo the request's policyId, got {error}"
            );
            assert!(
                !error["message"].as_str().unwrap_or_default().is_empty(),
                "case {case:?}: error {position} must carry a message, got {error}"
            );
        }
    }
}

/// A batch answer agrees with `GetPolicy` on every member the two share, for every
/// policy in the fixture — the batch reads the same persisted policy, it does not
/// recompute one.
#[tokio::test]
async fn batch_get_policy_agrees_with_get_policy() {
    let router = router();
    let stored = fixture(&router).await.policies;

    let requests: Vec<Value> = stored
        .values()
        .map(|policy| json!({ "policyStoreId": policy.store_id, "policyId": policy.policy_id }))
        .collect();
    let answer = ok(&router, "BatchGetPolicy", &json!({ "requests": requests })).await;
    let results = answer["results"]
        .as_array()
        .expect("results is required")
        .clone();
    assert_eq!(
        results.len(),
        stored.len(),
        "every fixture policy must be reported, got {answer}"
    );

    for (policy, result) in stored.values().zip(results.iter()) {
        let case = &policy.policy_id;
        let single = ok(
            &router,
            "GetPolicy",
            &json!({ "policyStoreId": policy.store_id, "policyId": policy.policy_id }),
        )
        .await;
        for member in [
            "policyStoreId",
            "policyId",
            "policyType",
            "definition",
            "createdDate",
            "lastUpdatedDate",
            "name",
        ] {
            assert_eq!(
                result[member], single[member],
                "case {case:?}: BatchGetPolicy and GetPolicy disagree on {member}: {result} vs {single}"
            );
        }
    }
}

/// `requests` is `length 1..100`: both published bounds are served and one past
/// each is refused as a 400 `ValidationException`, never as a partial answer.
#[tokio::test]
async fn batch_get_policy_honours_the_models_request_count_bounds() {
    // Source: `BatchGetPolicyInputList`'s `length {min: 1, max: 100}`, read from
    // the service's Smithy model
    // (`smithy_query(query="operation",operation="BatchGetPolicy")`).
    const MIN_REQUESTS: usize = 1;
    const MAX_REQUESTS: usize = 100;

    let router = router();
    let stored = fixture(&router).await.policies;
    let item = json!({
        "policyStoreId": stored["anonymous"].store_id,
        "policyId": stored["anonymous"].policy_id,
    });

    for count in [MIN_REQUESTS, MAX_REQUESTS] {
        let requests: Vec<Value> = std::iter::repeat_n(item.clone(), count).collect();
        let answer = ok(&router, "BatchGetPolicy", &json!({ "requests": requests })).await;
        assert_eq!(
            answer["results"].as_array().map(Vec::len),
            Some(count),
            "case {count} requests: a request at a published bound is answered item for item, got {answer}"
        );
        assert_eq!(
            answer["errors"].as_array().map(Vec::len),
            Some(0),
            "case {count} requests: no item missed, got {answer}"
        );
    }

    for (case, count) in [("below the minimum", MIN_REQUESTS - 1), ("above the maximum", MAX_REQUESTS + 1)] {
        let requests: Vec<Value> = std::iter::repeat_n(item.clone(), count).collect();
        let (status, body) = call(&router, "BatchGetPolicy", &json!({ "requests": requests })).await;
        assert_eq!(
            status, 400,
            "case {case:?} ({count} requests): expected 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?} ({count} requests): unexpected error code, got {body}"
        );
    }
}

/// `BatchGetPolicy` is a report: no sequence of batch reads — hits, misses, or a
/// mix — changes what a later caller sees.
#[tokio::test]
async fn batch_get_policy_changes_no_state() {
    let router = router();
    let Fixture {
        stores,
        policies: stored,
        active_alias,
        pending_alias,
    } = fixture(&router).await;

    let before: Vec<Value> = stores
        .iter()
        .map(|store_id| json!({ "policyStoreId": store_id }))
        .collect();
    let first = list_each(&router, &before).await;
    for requests in [
        vec![json!({
            "policyStoreId": stored["named-and-described"].store_id,
            "policyId": stored["named-and-described"].policy_id,
        })],
        vec![json!({ "policyStoreId": "PS0000000000000000000000", "policyId": "SP0000000000000000000000" })],
        vec![json!({
            "policyStoreId": active_alias,
            "policyId": stored["anonymous"].policy_id,
        })],
        vec![
            json!({
                "policyStoreId": stored["anonymous"].store_id,
                "policyId": "SP0000000000000000000000",
            }),
            json!({ "policyStoreId": "policy-store-alias/no-such-alias", "policyId": "SP0" }),
            json!({ "policyStoreId": pending_alias, "policyId": "SP0" }),
        ],
    ] {
        ok(&router, "BatchGetPolicy", &json!({ "requests": requests })).await;
    }

    let after = list_each(&router, &before).await;
    assert_eq!(
        first, after,
        "no batch read may change what ListPolicies reports: {first:?} vs {after:?}"
    );
}

/// `ListPolicies` for each request, so two snapshots of the fixture's stores can be
/// compared.
async fn list_each(router: &MultiServiceRouter, requests: &[Value]) -> Vec<Value> {
    let mut listed = Vec::new();
    for request in requests {
        listed.push(ok(router, "ListPolicies", request).await);
    }
    listed
}
