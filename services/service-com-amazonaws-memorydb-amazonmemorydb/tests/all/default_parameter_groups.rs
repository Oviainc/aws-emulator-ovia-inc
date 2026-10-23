// SPDX-License-Identifier: CC0-1.0

//! The parameter groups MemoryDB itself provides, end to end over the wire.
//!
//! Every case drives the SAME path a client does: an `awsJson1_1` `POST /` with
//! `X-Amz-Target: AmazonMemoryDB.<Operation>` through the multi-service router (the
//! shared harness in `cluster_lifecycle`), so what is asserted is the emulator's HTTP
//! behaviour and not a handler called directly.
//!
//! The rule under test: an account already holds one parameter group per parameter group
//! family, named `default.` + the family with its underscores written as hyphens, which
//! carries that family's engine defaults and cannot be modified. The rows are not one
//! recorded name: the families come from `DescribeEngineVersions` itself, for BOTH
//! engines the service publishes, so every family the catalog reports is covered and a
//! family added to the catalog later is covered too.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cluster_lifecycle::{call, error_code, ok, router};

/// The region the suite's requests are signed for (the shared harness signs for it), so
/// the ARN a default group reports must carry it rather than a baked-in region.
const REGION: &str = "us-west-2";

/// The account id every generated emulator presents (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The engines `DescribeEngineVersions$Engine` takes, which is where the families under
/// test come from. Source: `smithy_query query="doc" shape="DescribeEngineVersionsRequest"
/// member="Engine"` — "The name of the engine for which to list versions" — with
/// MemoryDB's two engine names, which this service's own catalog publishes.
const ENGINES: &[&str] = &["redis", "valkey"];

/// Settings every MemoryDB family carries, whatever its engine line: the baseline a
/// client reads a default group for. Source: the engine default parameters this service
/// publishes for every family (`crate::state::ENGINE_PARAMETERS`, entries carried from
/// the first generation on), of which these are three the parameter-group suites do not
/// otherwise assert on a DEFAULT group.
const BASELINE_PARAMETERS: &[&str] = &["maxmemory-policy", "timeout", "activedefrag"];

/// The parameter group families `DescribeEngineVersions` reports, over every engine.
async fn catalog_families(router: &MultiServiceRouter) -> BTreeSet<String> {
    let mut families = BTreeSet::new();
    for engine in ENGINES {
        let case = format!("DescribeEngineVersions(Engine={engine})");
        let body = ok(
            router,
            "DescribeEngineVersions",
            &json!({ "Engine": engine }),
            &case,
        )
        .await;
        let versions = body["EngineVersions"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: EngineVersions is a list, got {body}"));
        assert!(
            !versions.is_empty(),
            "case {case:?}: the catalog must publish a version for {engine}, got {body}"
        );
        for version in versions {
            let family = version["ParameterGroupFamily"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: every version names its family, got {version}")
                });
            families.insert(family.to_string());
        }
    }
    assert!(
        !families.is_empty(),
        "the catalog must report at least one parameter group family"
    );
    families
}

/// The name of the default parameter group of `family`, derived the way a client derives
/// it: `default.` + the family with underscores written as hyphens.
fn default_group_name(family: &str) -> String {
    format!("default.{}", family.replace('_', "-"))
}

/// `DescribeParameters` over every page of `group`, as `(Name, Value)` pairs in answer
/// order. `max_results` is the page size each call asks for, so the same set is read
/// both unpaged and paged.
async fn parameters(
    router: &MultiServiceRouter,
    group: &str,
    max_results: Option<i64>,
    case: &str,
) -> Vec<(String, String)> {
    let mut collected: Vec<(String, String)> = Vec::new();
    let mut token: Option<String> = None;
    // One page per parameter at worst, plus one: a page size of at least 1 always makes
    // progress, so this cannot loop forever.
    for _ in 0..=64 {
        let mut body = json!({ "ParameterGroupName": group });
        if let Some(max_results) = max_results {
            body["MaxResults"] = json!(max_results);
        }
        if let Some(token) = token.as_deref() {
            body["NextToken"] = json!(token);
        }
        let page = ok(router, "DescribeParameters", &body, case).await;
        let listed = page["Parameters"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: Parameters is a list, got {page}"))
            .clone();
        for parameter in &listed {
            let name = parameter["Name"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a parameter has a Name, got {parameter}"))
                .to_string();
            let value = parameter["Value"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: a parameter has a Value, got {parameter}")
                })
                .to_string();
            // Every reported parameter publishes what it is, not just its value.
            for member in ["Description", "DataType", "MinimumEngineVersion"] {
                assert!(
                    parameter.get(member).is_some(),
                    "case {case:?}: expected {member} on {parameter}"
                );
            }
            collected.push((name, value));
        }
        token = page
            .get("NextToken")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        if token.is_none() {
            break;
        }
    }
    assert!(
        token.is_none(),
        "case {case:?}: the listing must end, still holding token {token:?}"
    );
    collected
}

/// Every family the engine catalog reports has its default parameter group WITHOUT
/// anything being created: the group is reported by `DescribeParameterGroups` under the
/// derived name and its own family, and `DescribeParameters` on it answers that family's
/// engine defaults — which is also what a freshly created custom group of the family
/// starts with.
#[tokio::test]
async fn every_catalog_family_has_a_default_parameter_group_nobody_created() {
    let router = router();
    let families = catalog_families(&router).await;

    for (index, family) in families.iter().enumerate() {
        let name = default_group_name(family);
        let case = format!("default group of {family}");

        // The group itself, read one at a time.
        let described = ok(
            &router,
            "DescribeParameterGroups",
            &json!({ "ParameterGroupName": name }),
            &case,
        )
        .await;
        let group = &described["ParameterGroups"][0];
        for (member, expected) in [
            ("Name", json!(name)),
            ("Family", json!(family)),
            (
                "ARN",
                json!(format!(
                    "arn:aws:memorydb:{REGION}:{ACCOUNT}:parametergroup/{name}"
                )),
            ),
        ] {
            assert_eq!(
                group[member], expected,
                "case {case:?}: expected {member} to be {expected}, got {described}"
            );
        }
        assert!(
            group["Description"].is_string(),
            "case {case:?}: a default group reports a description, got {described}"
        );

        // Its parameters: the family's engine defaults, read unpaged and paged.
        let baseline = parameters(&router, &name, None, &case).await;
        assert!(
            !baseline.is_empty(),
            "case {case:?}: the family baseline must carry parameters, got {baseline:?}"
        );
        let names: Vec<&str> = baseline.iter().map(|(name, _)| name.as_str()).collect();
        for expected in BASELINE_PARAMETERS {
            assert!(
                names.contains(expected),
                "case {case:?}: expected the baseline to carry {expected}, got {names:?}"
            );
        }
        let mut sorted = names.clone();
        sorted.sort_unstable();
        assert_eq!(
            names, sorted,
            "case {case:?}: the baseline must be listed in name order, got {names:?}"
        );
        let paged = parameters(&router, &name, Some(3), &case).await;
        assert_eq!(
            paged, baseline,
            "case {case:?}: paging the baseline must yield the same parameters"
        );

        // A group a caller creates in the family STARTS from that baseline, which is
        // what makes the default group the documented starting point for a custom one.
        let custom = format!("custom-group-{index}");
        ok(
            &router,
            "CreateParameterGroup",
            &json!({ "ParameterGroupName": custom, "Family": family }),
            &case,
        )
        .await;
        let fresh = parameters(&router, &custom, None, &case).await;
        assert_eq!(
            fresh, baseline,
            "case {case:?}: a fresh group of the family must start equal to its default"
        );

        // And a read of the default group changes nothing a later caller sees.
        let again = parameters(&router, &name, None, &case).await;
        assert_eq!(
            again, baseline,
            "case {case:?}: reading the baseline must not change it"
        );
    }

    // The unfiltered listing reports every one of them, in name order, from a store
    // nobody wrote a group into.
    let fresh_router = crate::cluster_lifecycle::router();
    let listed = ok(
        &fresh_router,
        "DescribeParameterGroups",
        &json!({}),
        "untouched store",
    )
    .await;
    let reported: Vec<String> = listed["ParameterGroups"]
        .as_array()
        .unwrap_or_else(|| panic!("ParameterGroups is a list, got {listed}"))
        .iter()
        .map(|group| group["Name"].as_str().unwrap_or_default().to_string())
        .collect();
    let mut expected: Vec<String> = families.iter().map(|f| default_group_name(f)).collect();
    expected.sort();
    assert_eq!(
        reported, expected,
        "case {:?}: an untouched store must report exactly the default groups in name \
         order, got {listed}",
        "untouched store"
    );
}

/// A default parameter group is MemoryDB's, not the caller's: an update naming one is
/// refused with the modelled invalid-value error and the group's values are left exactly
/// as the family publishes them — while a custom group of the same family takes the very
/// same update.
#[tokio::test]
async fn a_default_parameter_group_cannot_be_modified() {
    let router = router();
    let families = catalog_families(&router).await;

    // One row per update shape, each a setting every family carries with a value from a
    // different kind of domain (a token set, an integer range).
    let updates: &[&[(&str, &str)]] = &[
        &[("maxmemory-policy", "allkeys-lru")],
        &[("timeout", "120")],
        &[("maxmemory-policy", "volatile-ttl"), ("timeout", "30")],
    ];

    for (index, family) in families.iter().enumerate() {
        let name = default_group_name(family);
        let case = format!("update of {name}");
        let before = parameters(&router, &name, None, &case).await;

        for update in updates {
            let pairs: Vec<Value> = update
                .iter()
                .map(|(parameter, value)| {
                    json!({ "ParameterName": parameter, "ParameterValue": value })
                })
                .collect();
            let case = format!("{case} with {pairs:?}");
            let (status, body) = call(
                &router,
                "UpdateParameterGroup",
                &json!({ "ParameterGroupName": name, "ParameterNameValues": pairs }),
            )
            .await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "InvalidParameterValueException"),
                "case {case:?}: expected the modelled invalid-value error, got {status} \
                 with {body}"
            );

            // A caller's OWN group of the same family takes the same update, so the
            // refusal is about the group being the service's and nothing else.
            let custom = format!("mutable-group-{index}");
            if update == &updates[0] {
                ok(
                    &router,
                    "CreateParameterGroup",
                    &json!({ "ParameterGroupName": custom, "Family": family }),
                    &case,
                )
                .await;
            }
            let accepted = ok(
                &router,
                "UpdateParameterGroup",
                &json!({ "ParameterGroupName": custom, "ParameterNameValues": pairs }),
                &case,
            )
            .await;
            assert_eq!(
                accepted["ParameterGroup"]["Name"],
                json!(custom),
                "case {case:?}: expected the caller's group to answer the update, got \
                 {accepted}"
            );
        }

        let after = parameters(&router, &name, None, &case).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused update must leave the baseline untouched"
        );
    }
}

/// `default.<name>` is not a magic prefix: a name no published family derives is the
/// modelled not-found fault, for the group listing and for its parameters alike.
#[tokio::test]
async fn a_default_name_for_no_published_family_names_nothing() {
    let router = router();
    let families = catalog_families(&router).await;

    // Rows: a family the catalog does not publish, the underscore spelling (which is the
    // FAMILY's form, not the group's), a truncated and an extended name, and the bare
    // prefix.
    let mut candidates: Vec<String> = vec![
        "default.memorydb-redis5".to_string(),
        "default.memorydb-valkey6".to_string(),
        "default.".to_string(),
        "default.memorydb".to_string(),
    ];
    for family in &families {
        candidates.push(format!("default.{family}"));
        candidates.push(format!("{}-extra", default_group_name(family)));
    }
    // Each candidate must really be outside the published set, so the rows cannot
    // accidentally name a group that exists.
    let published: BTreeSet<String> = families.iter().map(|f| default_group_name(f)).collect();

    for candidate in candidates {
        assert!(
            !published.contains(&candidate),
            "case {candidate:?}: the row must name no published default group"
        );
        for (operation, body) in [
            (
                "DescribeParameterGroups",
                json!({ "ParameterGroupName": candidate }),
            ),
            (
                "DescribeParameters",
                json!({ "ParameterGroupName": candidate }),
            ),
        ] {
            let case = format!("{operation} {candidate:?}");
            let (status, answered) = call(&router, operation, &body).await;
            assert_eq!(
                (status, error_code(&answered).as_str()),
                (404, "ParameterGroupNotFoundFault"),
                "case {case:?}: expected the modelled not-found fault, got {status} with \
                 {answered}"
            );
        }
    }
}

