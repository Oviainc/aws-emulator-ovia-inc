// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteFunction` on a QUALIFIED version: "Specify a version to
//! delete. You can't delete a version that an alias references"
//! (`smithy_query(query="operation", operation="DeleteFunction")`).
//!
//! The rule under test: a published version that at least one alias of the function still
//! reaches stays, and the delete is refused with the modelled `ResourceConflictException`
//! (409) naming every alias that reaches it, before anything is removed; a version no
//! alias reaches is deleted. An alias reaches a version both through the version it
//! invokes (`AliasConfiguration$FunctionVersion`) and through the second version its
//! routing configuration shifts part of its traffic to
//! (`AliasRoutingConfiguration$AdditionalVersionWeights`), so either reference holds the
//! version.
//!
//! Every row drives real HTTP requests through the router — over the ONE alias harness
//! this test binary has, [`crate::alias_update`] — and reads the outcome back through
//! `GetFunction`, `GetAlias` and `ListVersionsByFunction`, so a refusal that removed the
//! version anyway, or a delete that answered 204 without removing it, fails here. The rows
//! differ along what the rule generalises over: HOW the alias reaches the version (the
//! version it invokes, a routing weight), HOW MANY aliases reach it, WHICH version number
//! of how many published ones is addressed, whether the qualifier travels as the
//! `Qualifier` parameter or inside `FunctionName`, and the documented `FunctionName` form
//! the request addresses the function by.

use std::sync::Arc;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::alias_update::{
    call, create_alias, create_function, delete_alias, get_alias, router_on, update_alias, Answer,
    ACCOUNT, FUNCTIONS_URI, LATEST, REGION,
};

/// The fixed text of the refusal, as `crate::state::version_referenced_by_aliases_message`
/// builds it: the sentence, then the referencing alias names comma-separated inside one
/// bracketed group.
fn referenced_message(aliases: &[&str]) -> String {
    format!(
        "Unable to delete version because the following aliases reference it: [{}]",
        aliases.join(", ")
    )
}

/// `DeleteFunction` on the function `reference` names, qualified by `qualifier` when one
/// is given.
async fn delete_function(
    router: &MultiServiceRouter,
    reference: &str,
    qualifier: Option<&str>,
) -> Answer {
    let uri = match qualifier {
        Some(qualifier) => format!("{FUNCTIONS_URI}/{reference}?Qualifier={qualifier}"),
        None => format!("{FUNCTIONS_URI}/{reference}"),
    };
    call(router, "DELETE", &uri, &Value::Null).await
}

/// `GetFunction` on one qualifier of `function`.
async fn get_function(router: &MultiServiceRouter, function: &str, qualifier: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("{FUNCTIONS_URI}/{function}?Qualifier={qualifier}"),
        &Value::Null,
    )
    .await
}

/// `ListVersionsByFunction` on `function`.
async fn list_versions(router: &MultiServiceRouter, function: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("{FUNCTIONS_URI}/{function}/versions"),
        &Value::Null,
    )
    .await
}

/// The `Version` labels `ListVersionsByFunction` reports for `function`.
async fn listed_versions(router: &MultiServiceRouter, function: &str) -> Vec<String> {
    let listed = list_versions(router, function).await;
    listed
        .json()
        .get("Versions")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| entry.get("Version").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// `PublishVersion` on `function`, stating `description`.
async fn publish_version(
    router: &MultiServiceRouter,
    function: &str,
    description: &str,
) -> Answer {
    call(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{function}/versions"),
        &json!({ "Description": description }),
    )
    .await
}

/// One alias of the function under test: the name, the version it invokes, and the
/// routing weights it shifts part of its traffic by.
struct AliasSpec {
    name: &'static str,
    invokes: &'static str,
    weights: &'static [(&'static str, f64)],
}

/// One delete row.
struct DeleteCase {
    name: &'static str,
    function: &'static str,
    /// How many versions are published before the delete.
    versions: usize,
    /// The aliases the function holds when the delete arrives.
    aliases: &'static [AliasSpec],
    /// The version the delete addresses.
    qualifier: &'static str,
    /// Whether the qualifier travels as the `Qualifier` query parameter; otherwise it is
    /// appended to `FunctionName` as `name:qualifier`.
    via_query: bool,
    /// How the delete addresses the function, built from the function name.
    reference: fn(&str) -> String,
    /// The alias names the refusal must list, in the order it lists them. Empty when the
    /// delete must succeed.
    blocked_by: &'static [&'static str],
}

/// The rows. Each of the referencing forms blocks, each unreferenced version deletes, and
/// the request forms vary independently of both.
const DELETE_CASES: &[DeleteCase] = &[
    DeleteCase {
        name: "one alias invoking the version, bare name and the Qualifier parameter",
        function: "dfa-held-one",
        versions: 1,
        aliases: &[AliasSpec {
            name: "live",
            invokes: "1",
            weights: &[],
        }],
        qualifier: "1",
        via_query: true,
        reference: |name| name.to_string(),
        blocked_by: &["live"],
    },
    DeleteCase {
        name: "two aliases invoking the version, addressed by the full function arn",
        function: "dfa-held-two",
        versions: 2,
        aliases: &[
            AliasSpec {
                name: "green",
                invokes: "2",
                weights: &[],
            },
            AliasSpec {
                name: "blue",
                invokes: "2",
                weights: &[],
            },
        ],
        qualifier: "2",
        via_query: true,
        reference: |name| format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}"),
        blocked_by: &["blue", "green"],
    },
    DeleteCase {
        name: "an alias shifting part of its traffic to the version, qualified name form",
        function: "dfa-held-weight",
        versions: 3,
        aliases: &[AliasSpec {
            name: "shifting",
            invokes: "1",
            weights: &[("3", 0.25)],
        }],
        qualifier: "3",
        via_query: false,
        reference: |name| name.to_string(),
        blocked_by: &["shifting"],
    },
    DeleteCase {
        name: "an alias that invokes the version and another that shifts traffic to it",
        function: "dfa-held-both",
        versions: 2,
        aliases: &[
            AliasSpec {
                name: "pinned",
                invokes: "2",
                weights: &[],
            },
            AliasSpec {
                name: "canary",
                invokes: "1",
                weights: &[("2", 0.5)],
            },
        ],
        qualifier: "2",
        via_query: true,
        reference: |name| format!("{ACCOUNT}:function:{name}"),
        blocked_by: &["canary", "pinned"],
    },
    DeleteCase {
        name: "a version no alias reaches, the other version being aliased",
        function: "dfa-free-other",
        versions: 2,
        aliases: &[AliasSpec {
            name: "live",
            invokes: "1",
            weights: &[],
        }],
        qualifier: "2",
        via_query: true,
        reference: |name| name.to_string(),
        blocked_by: &[],
    },
    DeleteCase {
        name: "a version whose only alias invokes the unpublished version instead",
        function: "dfa-free-latest",
        versions: 1,
        aliases: &[AliasSpec {
            name: "dev",
            invokes: LATEST,
            weights: &[],
        }],
        qualifier: "1",
        via_query: false,
        reference: |name| format!("function:{name}"),
        blocked_by: &[],
    },
    DeleteCase {
        name: "the newest version of three, which no alias reaches",
        function: "dfa-free-newest",
        versions: 3,
        aliases: &[AliasSpec {
            name: "stage",
            invokes: "1",
            weights: &[("2", 0.1)],
        }],
        qualifier: "3",
        via_query: true,
        reference: |name| format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}"),
        blocked_by: &[],
    },
];

/// Create every alias `case` names on its function.
async fn create_case_aliases(router: &MultiServiceRouter, case: &DeleteCase) {
    for alias in case.aliases {
        let mut body = Map::new();
        body.insert("Name".to_string(), json!(alias.name));
        body.insert("FunctionVersion".to_string(), json!(alias.invokes));
        if !alias.weights.is_empty() {
            let mut weights = Map::new();
            for (version, weight) in alias.weights {
                weights.insert((*version).to_string(), json!(weight));
            }
            body.insert(
                "RoutingConfig".to_string(),
                json!({ "AdditionalVersionWeights": Value::Object(weights) }),
            );
        }
        let created = create_alias(router, case.function, &Value::Object(body)).await;
        assert_eq!(
            created.status,
            201,
            "case {:?}: the alias {:?} must be created, got {} {}",
            case.name,
            alias.name,
            created.status,
            created.text()
        );
    }
}

#[tokio::test]
async fn a_version_an_alias_reaches_is_not_deleted() {
    for case in DELETE_CASES {
        let label = case.name;
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = router_on(&engine);
        create_function(&router, case.function, case.versions).await;
        create_case_aliases(&router, case).await;

        let reference = (case.reference)(case.function);
        let (reference, qualifier) = if case.via_query {
            (reference, Some(case.qualifier))
        } else {
            (format!("{reference}:{}", case.qualifier), None)
        };
        let answer = delete_function(&router, &reference, qualifier).await;

        if case.blocked_by.is_empty() {
            assert_eq!(
                answer.status,
                204,
                "case {label:?}: a version no alias reaches must be deleted, got {} {}",
                answer.status,
                answer.text()
            );
            let read = get_function(&router, case.function, case.qualifier).await;
            assert_eq!(
                read.status,
                404,
                "case {label:?}: the deleted version must be gone, got {} {}",
                read.status,
                read.text()
            );
            let listed = listed_versions(&router, case.function).await;
            assert!(
                !listed.iter().any(|version| version == case.qualifier),
                "case {label:?}: the deleted version must not be listed, got {listed:?}"
            );
            continue;
        }

        assert_eq!(
            answer.status,
            409,
            "case {label:?}: expected the modelled conflict status, got {} {}",
            answer.status,
            answer.text()
        );
        assert!(
            answer.error_code().contains("ResourceConflictException"),
            "case {label:?}: expected ResourceConflictException, got {}",
            answer.error_code()
        );
        let expected = referenced_message(case.blocked_by);
        assert_eq!(
            answer.message(),
            expected,
            "case {label:?}: expected the refusal {expected:?}, got {:?}",
            answer.message()
        );
        assert_eq!(
            answer.string("Type").as_deref(),
            Some("User"),
            "case {label:?}: a refused request is a User fault, got {}",
            answer.text()
        );

        // Nothing was removed: the version, every alias and the whole version listing are
        // where they were.
        let read = get_function(&router, case.function, case.qualifier).await;
        assert_eq!(
            read.status,
            200,
            "case {label:?}: a refused delete leaves the version readable, got {} {}",
            read.status,
            read.text()
        );
        for alias in case.aliases {
            let held = get_alias(&router, case.function, alias.name).await;
            assert_eq!(
                held.status,
                200,
                "case {label:?}: the alias {:?} must survive, got {} {}",
                alias.name,
                held.status,
                held.text()
            );
        }
        let listed = listed_versions(&router, case.function).await;
        let published: Vec<String> = (1..=case.versions).map(|v| v.to_string()).collect();
        assert!(
            published.iter().all(|version| listed.contains(version)),
            "case {label:?}: every published version must still be listed, expected \
             {published:?} among {listed:?}"
        );
    }
}

#[tokio::test]
async fn a_refused_delete_leaves_the_version_counter_where_it_was() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    // One published version, which an alias then pins. `create_function` publishes it
    // with the description this suite re-states below, so the configuration has not
    // changed since.
    create_function(&router, "dfa-counter", 1).await;
    let created = create_alias(
        &router,
        "dfa-counter",
        &json!({ "Name": "live", "FunctionVersion": "1" }),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "the alias must be created, got {} {}",
        created.status,
        created.text()
    );

    let refused = delete_function(&router, "dfa-counter", Some("1")).await;
    assert_eq!(
        refused.status,
        409,
        "the aliased version must not be deleted, got {} {}",
        refused.status,
        refused.text()
    );

    // "Lambda doesn't publish a version if the function's configuration and code haven't
    // changed since the last version": version 1 is still there and still holds this
    // configuration, so the next publish re-reports it rather than minting a number past
    // the version the refused delete did not take.
    let republished = publish_version(&router, "dfa-counter", "version 1").await;
    assert_eq!(
        republished.status,
        201,
        "PublishVersion must succeed, got {} {}",
        republished.status,
        republished.text()
    );
    assert_eq!(
        republished.string("Version").as_deref(),
        Some("1"),
        "expected the publish to report version 1, got {}",
        republished.text()
    );
    let expected_arn = format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:dfa-counter:1");
    assert_eq!(
        republished.string("FunctionArn").as_deref(),
        Some(expected_arn.as_str()),
        "expected the ARN {expected_arn:?}, got {}",
        republished.text()
    );
    assert_eq!(
        listed_versions(&router, "dfa-counter").await,
        vec![LATEST.to_string(), "1".to_string()],
        "the refused delete must leave no extra version behind, got {:?}",
        listed_versions(&router, "dfa-counter").await
    );

    // A change to the configuration gets its own number, and numbers are never reused: a
    // second version, deletable once no alias reaches it.
    let second = publish_version(&router, "dfa-counter", "version 2").await;
    assert_eq!(
        second.string("Version").as_deref(),
        Some("2"),
        "expected the changed configuration to mint version 2, got {}",
        second.text()
    );
    let deleted = delete_function(&router, "dfa-counter", Some("2")).await;
    assert_eq!(
        deleted.status,
        204,
        "an unreferenced version must be deletable, got {} {}",
        deleted.status,
        deleted.text()
    );
    let third = publish_version(&router, "dfa-counter", "version 3").await;
    assert_eq!(
        third.string("Version").as_deref(),
        Some("3"),
        "a deleted number is never reused, got {}",
        third.text()
    );
}

#[tokio::test]
async fn the_version_is_deletable_once_no_alias_reaches_it() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    create_function(&router, "dfa-release", 2).await;
    for (alias, body) in [
        ("pinned", json!({ "Name": "pinned", "FunctionVersion": "1" })),
        (
            "canary",
            json!({
                "Name": "canary",
                "FunctionVersion": "2",
                "RoutingConfig": { "AdditionalVersionWeights": { "1": 0.2 } },
            }),
        ),
    ] {
        let created = create_alias(&router, "dfa-release", &body).await;
        assert_eq!(
            created.status,
            201,
            "the alias {alias:?} must be created, got {} {}",
            created.status,
            created.text()
        );
    }

    // Both aliases reach version 1 — one invokes it, one shifts traffic to it — so both
    // are named, and dropping either one alone is not enough.
    let refused = delete_function(&router, "dfa-release", Some("1")).await;
    assert_eq!(
        refused.message(),
        referenced_message(&["canary", "pinned"]),
        "expected both aliases named, got {:?}",
        refused.message()
    );

    let repointed = update_alias(
        &router,
        "dfa-release",
        "canary",
        &json!({ "FunctionVersion": "2", "RoutingConfig": { "AdditionalVersionWeights": {} } }),
    )
    .await;
    assert_eq!(
        repointed.status,
        200,
        "the routing weight must be removable, got {} {}",
        repointed.status,
        repointed.text()
    );
    let still_refused = delete_function(&router, "dfa-release", Some("1")).await;
    assert_eq!(
        still_refused.message(),
        referenced_message(&["pinned"]),
        "expected the remaining alias named, got {:?}",
        still_refused.message()
    );

    let removed = delete_alias(&router, "dfa-release", "pinned").await;
    assert_eq!(
        removed.status,
        204,
        "the last referencing alias must be deletable, got {} {}",
        removed.status,
        removed.text()
    );
    let deleted = delete_function(&router, "dfa-release", Some("1")).await;
    assert_eq!(
        deleted.status,
        204,
        "the version must be deletable once no alias reaches it, got {} {}",
        deleted.status,
        deleted.text()
    );
    let read = get_function(&router, "dfa-release", "1").await;
    assert_eq!(
        read.status,
        404,
        "the deleted version must be gone, got {} {}",
        read.status,
        read.text()
    );
}

#[tokio::test]
async fn deleting_the_whole_function_takes_its_aliased_versions_with_it() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    create_function(&router, "dfa-whole", 2).await;
    for alias in ["blue", "green"] {
        let created = create_alias(
            &router,
            "dfa-whole",
            &json!({ "Name": alias, "FunctionVersion": "2" }),
        )
        .await;
        assert_eq!(
            created.status,
            201,
            "the alias {alias:?} must be created, got {} {}",
            created.status,
            created.text()
        );
    }

    // "Otherwise, all versions and aliases are deleted": an unqualified delete is not
    // the delete of a version an alias references, so no alias blocks it.
    let deleted = delete_function(&router, "dfa-whole", None).await;
    assert_eq!(
        deleted.status,
        204,
        "an unqualified delete must succeed, got {} {}",
        deleted.status,
        deleted.text()
    );
    let read = get_function(&router, "dfa-whole", LATEST).await;
    assert_eq!(
        read.status,
        404,
        "the function must be gone, got {} {}",
        read.status,
        read.text()
    );
}
