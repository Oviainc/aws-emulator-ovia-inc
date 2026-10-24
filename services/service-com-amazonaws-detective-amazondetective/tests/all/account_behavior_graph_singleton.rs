// SPDX-License-Identifier: CC0-1.0

//! `CreateGraph` is the account's ONE behavior graph per Region.
//!
//! The rule under test, from `CreateGraph`'s own documentation: "An account can only be
//! the administrator account for one behavior graph within a Region. If the same account
//! calls CreateGraph with the same administrator account, it always returns the same
//! behavior graph ARN. It does not create a new behavior graph." So every call by this
//! emulator's one account in a Region answers with that Region's graph — however often
//! it is called, whatever body it carries, and however many callers race — and the store
//! holds exactly one graph per Region. Deleting it is what lets the next call mint
//! another.
//!
//! Every case drives the real wire path: the production `MultiServiceRouter`, restJson1
//! serialization, the modelled-constraint walk and the handlers.

use std::collections::BTreeSet;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{call, call_with, create_graph_ok, emulator, REGION};

/// Every graph ARN `ListGraphs` reports, in the order it served them. The store these
/// cases build never exceeds one page (the default is the modelled `MemberResultsLimit`
/// maximum of 200), and the assertion below would fail loudly if it did.
async fn listed_graph_arns(router: &MultiServiceRouter) -> Vec<String> {
    let (status, page) = call(router, "/graphs/list", &json!({})).await;
    assert_eq!(status, 200, "ListGraphs: expected 200, got {status} {page:?}");
    assert!(
        page.get("NextToken").is_none(),
        "ListGraphs: these cases must fit one page, got {page:?}"
    );
    page.get("GraphList")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("ListGraphs carried no GraphList, got {page:?}"))
        .iter()
        .map(|graph| {
            graph
                .get("Arn")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("a listed graph carried no Arn, got {graph:?}"))
                .to_string()
        })
        .collect()
}

/// The tag map `ListTagsForResource` reports for `graph_arn`.
async fn tags_of(router: &MultiServiceRouter, region: &str, graph_arn: &str) -> Map<String, Value> {
    let (status, body) = call_with(
        router,
        "GET",
        region,
        &format!("/tags/{graph_arn}"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        status, 200,
        "ListTagsForResource({graph_arn}): expected 200, got {status} {body:?}"
    );
    body.get("Tags")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

/// One row of the repeat-call table: the Region the calls are signed for, the bodies of
/// the successive `CreateGraph` calls, and the tags the graph must end up carrying.
struct RepeatCase {
    name: &'static str,
    region: &'static str,
    requests: Vec<Value>,
    expected_tags: Vec<(&'static str, &'static str)>,
}

/// The table. The rule generalises over the Region, how MANY times `CreateGraph` is
/// called, and what each call carries, so the rows differ along exactly those: two
/// calls, five calls, and repeats whose `Tags` differ from the first call's — including
/// a first call with no tags followed by a tagged repeat, which cannot assign tags
/// because `Tags` is "the tags to assign to the NEW behavior graph" and the repeat
/// creates none.
fn repeat_cases() -> Vec<RepeatCase> {
    vec![
        RepeatCase {
            name: "twice-untagged",
            region: REGION,
            requests: vec![json!({}), json!({})],
            expected_tags: vec![],
        },
        RepeatCase {
            name: "tagged-then-untagged",
            region: "us-east-1",
            requests: vec![json!({ "Tags": { "Env": "test" } }), json!({})],
            expected_tags: vec![("Env", "test")],
        },
        RepeatCase {
            name: "untagged-then-tagged",
            region: "eu-west-1",
            requests: vec![json!({}), json!({ "Tags": { "Env": "prod" } })],
            expected_tags: vec![],
        },
        RepeatCase {
            name: "five-times-with-changing-tags",
            region: "ap-southeast-2",
            requests: vec![
                json!({ "Tags": { "Env": "test", "Team": "sec" } }),
                json!({}),
                json!({ "Tags": { "Env": "staging" } }),
                json!({ "Tags": { "Other": "value" } }),
                json!({}),
            ],
            expected_tags: vec![("Env", "test"), ("Team", "sec")],
        },
    ]
}

#[tokio::test]
async fn repeated_create_graph_from_one_account_answers_with_its_one_graph() {
    for case in repeat_cases() {
        // One emulator per row, so the store holds only the graphs this row created.
        let router = emulator();
        let first = create_graph_ok(&router, case.region, &case.requests[0]).await;

        for (n, body) in case.requests.iter().enumerate().skip(1) {
            let again = create_graph_ok(&router, case.region, body).await;
            assert_eq!(
                again, first,
                "case {:?} call {n}: expected the account's one graph ARN {first:?}, got {again:?}",
                case.name
            );
        }

        let listed = listed_graph_arns(&router).await;
        assert_eq!(
            listed,
            vec![first.clone()],
            "case {:?}: expected exactly the one stored graph, got {listed:?}",
            case.name
        );

        // No new graph was created, so no repeat assigned tags: the graph still carries
        // what the create that minted it stored.
        let tags = tags_of(&router, case.region, &first).await;
        let expected: Map<String, Value> = case
            .expected_tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), json!(value)))
            .collect();
        assert_eq!(
            tags, expected,
            "case {:?}: expected the tags the create stored {expected:?}, got {tags:?}",
            case.name
        );
    }
}

#[tokio::test]
async fn the_singleton_is_per_region_so_each_region_keeps_its_own_graph() {
    let router = emulator();
    // The rows differ along the Region, which is what the rule is keyed by.
    let regions = ["us-west-2", "us-east-1", "eu-central-1", "sa-east-1"];
    let mut minted: Vec<String> = Vec::new();

    for region in regions {
        let arn = create_graph_ok(&router, region, &json!({})).await;
        let again = create_graph_ok(&router, region, &json!({})).await;
        assert_eq!(
            again, arn,
            "case {region:?}: expected the Region's one graph ARN {arn:?}, got {again:?}"
        );
        assert!(
            arn.contains(&format!(":detective:{region}:")),
            "case {region:?}: expected an ARN minted in that Region, got {arn:?}"
        );
        assert!(
            !minted.contains(&arn),
            "case {region:?}: expected a graph of its own, got one already minted: {arn:?}"
        );
        minted.push(arn);
    }

    let mut expected = minted.clone();
    expected.sort();
    let listed = listed_graph_arns(&router).await;
    assert_eq!(
        listed, expected,
        "ListGraphs: expected one graph per Region in ARN order, got {listed:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_create_graph_callers_converge_on_one_arn() {
    // The rows differ along how many callers race; the rule is the same for each.
    for callers in [2usize, 8, 16] {
        let router = emulator();
        let mut handles = Vec::with_capacity(callers);
        for _ in 0..callers {
            let router = router.clone();
            handles.push(tokio::spawn(async move {
                create_graph_ok(&router, REGION, &json!({})).await
            }));
        }

        let mut answered: BTreeSet<String> = BTreeSet::new();
        for handle in handles {
            answered.insert(
                handle
                    .await
                    .unwrap_or_else(|err| panic!("case {callers} callers: a caller panicked: {err}")),
            );
        }
        assert_eq!(
            answered.len(),
            1,
            "case {callers} concurrent callers: expected every call to agree on one ARN, got {answered:?}"
        );

        let listed = listed_graph_arns(&router).await;
        assert_eq!(
            listed,
            answered.iter().cloned().collect::<Vec<String>>(),
            "case {callers} concurrent callers: expected exactly one stored graph, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn the_accounts_graph_is_minted_again_only_after_the_one_it_has_is_deleted() {
    let router = emulator();
    let first = create_graph_ok(&router, REGION, &json!({})).await;
    assert_eq!(
        create_graph_ok(&router, REGION, &json!({})).await,
        first,
        "before the delete: expected the account's one graph ARN {first:?}"
    );

    let (status, body) = call(&router, "/graph/removal", &json!({ "GraphArn": first })).await;
    assert_eq!(
        status, 200,
        "DeleteGraph: expected 200, got {status} {body:?}"
    );

    // The account no longer administers a graph in this Region, so the next call mints
    // one — and repeats converge on THAT one.
    let second = create_graph_ok(&router, REGION, &json!({})).await;
    assert_ne!(
        second, first,
        "after the delete: expected a freshly minted graph, got the deleted ARN {second:?}"
    );
    assert_eq!(
        create_graph_ok(&router, REGION, &json!({})).await,
        second,
        "after the delete: expected the new graph's ARN {second:?} on a repeat call"
    );

    let listed = listed_graph_arns(&router).await;
    assert_eq!(
        listed,
        vec![second.clone()],
        "ListGraphs: expected only the graph that replaced the deleted one, got {listed:?}"
    );
}
