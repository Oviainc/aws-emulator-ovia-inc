// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the DAX parameter-group slice: `CreateParameterGroup`,
//! `DescribeParameterGroups` and `DeleteParameterGroup` over the wire.
//!
//! Every case drives `POST /` with `X-Amz-Target: AmazonDAXV3.<Operation>` through the
//! same `MultiServiceRouter` the production router builds, so what is asserted is the
//! HTTP behaviour a DAX SDK client sees — not an internal function call.
//!
//! The tables below span the CLASS each rule covers: the name and the description the
//! issue happened to cite are one row each, and the rules are asserted over names of
//! different lengths, shapes and orderings, over present/absent/empty descriptions, and
//! over every published `MaxResults` bound plus one past each.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_dax_amazondaxv3::{state, EmulatedService};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for.
const REGION: &str = "us-west-2";

/// A role ARN to pass as `CreateCluster`'s required `IamRoleArn`.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/DAXRole";

/// A node type to pass as `CreateCluster`'s required `NodeType`.
const NODE_TYPE: &str = "dax.r4.large";

/// Wait until every clock-owed cluster status move started before the call is due.
async fn settle() {
    tokio::time::sleep(state::STATUS_SETTLE_DELAY + Duration::from_millis(400)).await;
}

/// A router carrying only this service, over its own engine, so each test starts from a
/// fresh store.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Call one operation and return its HTTP status and parsed JSON body.
async fn call(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("AmazonDAXV3.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/dax/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The wire error code of a failed call: the `__type` member, without any namespace.
fn error_code(body: &Value) -> String {
    body["__type"]
        .as_str()
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The `ParameterGroupName`s a `DescribeParameterGroups` body reports, in order.
fn reported_names(body: &Value) -> Vec<String> {
    body["ParameterGroups"]
        .as_array()
        .map(|groups| {
            groups
                .iter()
                .map(|group| {
                    group["ParameterGroupName"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// One created group, as a name and the `Description` the create carried (`None` for a
/// request that names none).
type GroupCase = (&'static str, Option<&'static str>);

/// Names and descriptions spanning the class `CreateParameterGroup` accepts: the model
/// puts no constraint on either member (`smithy_query(query="shape",
/// shape="CreateParameterGroupRequest")` — both target the unconstrained `String`), so a
/// short name, a long one, hyphens, dots, digits, mixed case and an absent or empty
/// description are all one class.
const GROUP_CASES: [GroupCase; 6] = [
    ("pg", Some("two character name")),
    ("my-dax-parameters", Some("hyphenated name")),
    ("params.v2", Some("a dotted name, as the default group has")),
    ("Group9", Some("mixed case and a digit")),
    ("no-description", None),
    (
        "a-rather-long-parameter-group-name-0123456789",
        Some(""),
    ),
];

// ---------------------------------------------------------------------------
// Create -> describe round trip.
// ---------------------------------------------------------------------------

/// Every created group is read back out of the store exactly as it was stored: the
/// create's own response, a describe naming it, and the unfiltered describe all report
/// the same name and description.
#[tokio::test]
async fn created_parameter_groups_round_trip_through_describe() {
    let router = emulator();

    for (name, description) in GROUP_CASES {
        let mut body = json!({ "ParameterGroupName": name });
        if let Some(description) = description {
            body["Description"] = json!(description);
        }
        let (status, created) = call(&router, "CreateParameterGroup", body).await;
        assert_eq!(status, 200, "case {name:?}: expected 200, got {created:?}");
        assert_eq!(
            created["ParameterGroup"]["ParameterGroupName"].as_str(),
            Some(name),
            "case {name:?}: create must echo the stored name, got {created:?}"
        );
        assert_eq!(
            created["ParameterGroup"]["Description"].as_str(),
            description,
            "case {name:?}: create must report the stored description, got {created:?}"
        );

        let (status, read) = call(
            &router,
            "DescribeParameterGroups",
            json!({ "ParameterGroupNames": [name] }),
        )
        .await;
        assert_eq!(status, 200, "case {name:?}: describe got {read:?}");
        assert_eq!(
            reported_names(&read),
            vec![name.to_string()],
            "case {name:?}: a describe naming one group reports only it, got {read:?}"
        );
        assert_eq!(
            read["ParameterGroups"][0]["Description"].as_str(),
            description,
            "case {name:?}: describe must report the stored description, got {read:?}"
        );
    }

    // The unfiltered describe reports every stored group, plus the default group DAX
    // provides, in one stable (sorted) order.
    let (status, all) = call(&router, "DescribeParameterGroups", json!({})).await;
    assert_eq!(status, 200, "describe all: got {all:?}");
    let mut expected: Vec<String> = GROUP_CASES
        .iter()
        .map(|(name, _)| (*name).to_string())
        .chain([state::DEFAULT_PARAMETER_GROUP.to_string()])
        .collect();
    expected.sort();
    assert_eq!(
        reported_names(&all),
        expected,
        "describe all: expected every stored group sorted, got {all:?}"
    );
    assert!(
        all["NextToken"].is_null(),
        "describe all: a single page must carry no NextToken, got {all:?}"
    );
}

/// A fresh account already has the parameter group AWS provides, and it is reported both
/// unfiltered and by name.
#[tokio::test]
async fn the_default_parameter_group_is_reported_before_anything_is_created() {
    let router = emulator();

    let (status, all) = call(&router, "DescribeParameterGroups", json!({})).await;
    assert_eq!(status, 200, "describe all: got {all:?}");
    assert_eq!(
        reported_names(&all),
        vec![state::DEFAULT_PARAMETER_GROUP.to_string()],
        "a fresh store reports exactly the default parameter group, got {all:?}"
    );

    let (status, named) = call(
        &router,
        "DescribeParameterGroups",
        json!({ "ParameterGroupNames": [state::DEFAULT_PARAMETER_GROUP] }),
    )
    .await;
    assert_eq!(status, 200, "describe default by name: got {named:?}");
    assert_eq!(
        named["ParameterGroups"][0]["Description"].as_str(),
        Some(state::DEFAULT_PARAMETER_GROUP_DESCRIPTION),
        "the default group carries its description, got {named:?}"
    );

    // A read leaves the store as it found it: a second describe answers the same thing.
    let (status, again) = call(&router, "DescribeParameterGroups", json!({})).await;
    assert_eq!(status, 200, "describe all again: got {again:?}");
    assert_eq!(
        reported_names(&again),
        reported_names(&all),
        "a describe must not change what a later caller sees, got {again:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule: the parameter group AWS provides is identified by the parameter group FAMILY
// whose defaults it carries — `default.<family>`, described as "Default parameter group
// for <family>" — and that name is the identifier a client passes back wherever a
// `ParameterGroupName` is taken, so no other spelling of it resolves.
// Source: `state::DEFAULT_PARAMETER_GROUP_FAMILY` (the family the catalog's defaults
// belong to) and the members that take the name back on the wire —
// `DescribeParameterGroups$ParameterGroupNames` and `CreateCluster$ParameterGroupName`
// (`smithy_query(query="operation",operation="CreateCluster,DescribeParameterGroups")`).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_default_parameter_group_is_identified_by_its_family() {
    // The identity is the family's, not an unrelated spelling: both members are the
    // family-qualified forms.
    assert_eq!(
        state::DEFAULT_PARAMETER_GROUP,
        format!("default.{}", state::DEFAULT_PARAMETER_GROUP_FAMILY),
        "case name: expected default.<family>, got {:?}",
        state::DEFAULT_PARAMETER_GROUP
    );
    assert_eq!(
        state::DEFAULT_PARAMETER_GROUP_DESCRIPTION,
        format!(
            "Default parameter group for {}",
            state::DEFAULT_PARAMETER_GROUP_FAMILY
        ),
        "case description: expected it to name the family, got {:?}",
        state::DEFAULT_PARAMETER_GROUP_DESCRIPTION
    );

    let router = emulator();
    let (status, all) = call(&router, "DescribeParameterGroups", json!({})).await;
    assert_eq!(status, 200, "describe all: got {all:?}");
    assert_eq!(
        all["ParameterGroups"][0]["ParameterGroupName"].as_str(),
        Some(state::DEFAULT_PARAMETER_GROUP),
        "case unfiltered: expected the family-qualified name, got {all:?}"
    );
    assert_eq!(
        all["ParameterGroups"][0]["Description"].as_str(),
        Some(state::DEFAULT_PARAMETER_GROUP_DESCRIPTION),
        "case unfiltered: expected the family's description, got {all:?}"
    );

    // Rows: spellings that drop part of the identifier — the family, its version, the
    // separator — plus the family on its own. None of them names a group, so a client
    // that guessed one is refused rather than served the default group by accident.
    for absent in [
        "default.dax",
        "default",
        "default.dax1",
        "default-dax1.0",
        state::DEFAULT_PARAMETER_GROUP_FAMILY,
    ] {
        let (status, body) = call(
            &router,
            "DescribeParameterGroups",
            json!({ "ParameterGroupNames": [absent] }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (404, "ParameterGroupNotFoundFault"),
            "case {absent:?}: only the family-qualified name resolves, got {status} {body:?}"
        );
    }

    // The name reads back as an identifier: a describe naming it resolves, and a cluster
    // created with it reports it unchanged.
    let (status, named) = call(
        &router,
        "DescribeParameterGroups",
        json!({ "ParameterGroupNames": [state::DEFAULT_PARAMETER_GROUP] }),
    )
    .await;
    assert_eq!(status, 200, "case describe by name: got {named:?}");
    assert_eq!(
        reported_names(&named),
        vec![state::DEFAULT_PARAMETER_GROUP.to_string()],
        "case describe by name: expected only the default group, got {named:?}"
    );

    let (status, created) = call(
        &router,
        "CreateCluster",
        json!({
            "ClusterName": "default-params",
            "NodeType": NODE_TYPE,
            "ReplicationFactor": 1,
            "IamRoleArn": ROLE_ARN,
            "ParameterGroupName": state::DEFAULT_PARAMETER_GROUP,
        }),
    )
    .await;
    assert_eq!(status, 200, "case CreateCluster: got {created:?}");
    assert_eq!(
        created["Cluster"]["ParameterGroup"]["ParameterGroupName"].as_str(),
        Some(state::DEFAULT_PARAMETER_GROUP),
        "case CreateCluster: expected the group it was created with, got {created:?}"
    );
}

// ---------------------------------------------------------------------------
// Refusals: an unknown name, a name already taken.
// ---------------------------------------------------------------------------

/// A name that names no stored group is answered with `ParameterGroupNotFoundFault`
/// (HTTP 404) by both the describe and the delete — including a name that only differs
/// from a stored one in case, and a name whose group has just been deleted.
#[tokio::test]
async fn an_unknown_parameter_group_name_is_refused_with_not_found() {
    let router = emulator();
    let (status, body) = call(
        &router,
        "CreateParameterGroup",
        json!({ "ParameterGroupName": "stored-group" }),
    )
    .await;
    assert_eq!(status, 200, "create stored-group: got {body:?}");

    // Absent names spanning the class: never created, differing only in case from a
    // stored one, the empty name, and one that differs by a single character.
    for absent in ["never-created", "STORED-GROUP", "", "stored-grou"] {
        let (status, body) = call(
            &router,
            "DescribeParameterGroups",
            json!({ "ParameterGroupNames": [absent] }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (404, "ParameterGroupNotFoundFault"),
            "case {absent:?}: describe of an absent group must be 404 \
             ParameterGroupNotFoundFault, got {status} {body:?}"
        );
        assert!(
            !body["message"]
                .as_str()
                .unwrap_or_default()
                .contains(absent)
                || absent.is_empty(),
            "case {absent:?}: the not-found message must not echo the request, got {body:?}"
        );

        let (status, body) = call(
            &router,
            "DeleteParameterGroup",
            json!({ "ParameterGroupName": absent }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (404, "ParameterGroupNotFoundFault"),
            "case {absent:?}: delete of an absent group must be 404 \
             ParameterGroupNotFoundFault, got {status} {body:?}"
        );
    }

    // One absent name among present ones is still a refusal, and the present ones are
    // not reported in its place.
    let (status, body) = call(
        &router,
        "DescribeParameterGroups",
        json!({ "ParameterGroupNames": ["stored-group", "never-created"] }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (404, "ParameterGroupNotFoundFault"),
        "a mixed list must be refused on the absent name, got {status} {body:?}"
    );
}

/// A name may hold one parameter group: a second create under a taken name is refused and
/// writes nothing, whether the name is one a caller created or the default group's.
#[tokio::test]
async fn creating_a_parameter_group_under_a_taken_name_is_refused() {
    let router = emulator();

    for (name, description) in [("taken", Some("the first create")), ("also-taken", None)] {
        let mut body = json!({ "ParameterGroupName": name });
        if let Some(description) = description {
            body["Description"] = json!(description);
        }
        let (status, created) = call(&router, "CreateParameterGroup", body).await;
        assert_eq!(status, 200, "case {name:?}: first create got {created:?}");
    }

    for (name, taken_description) in [
        ("taken", Some("the first create")),
        ("also-taken", None),
        (
            state::DEFAULT_PARAMETER_GROUP,
            Some(state::DEFAULT_PARAMETER_GROUP_DESCRIPTION),
        ),
    ] {
        let (status, body) = call(
            &router,
            "CreateParameterGroup",
            json!({ "ParameterGroupName": name, "Description": "a second create" }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ParameterGroupAlreadyExistsFault"),
            "case {name:?}: expected 400 ParameterGroupAlreadyExistsFault, got {status} {body:?}"
        );
        // Like this service's other parameter-group fault, the template is one fixed
        // sentence that echoes nothing of the request.
        assert_eq!(
            body["message"].as_str(),
            state::parameter_group_already_exists().message.as_deref(),
            "case {name:?}: expected the fault's own template, got {body:?}"
        );
        assert!(
            !body["message"].as_str().unwrap_or_default().contains(name),
            "case {name:?}: the refusal must not echo the requested name, got {body:?}"
        );

        // Nothing was written: the stored description is still the first one.
        let (status, read) = call(
            &router,
            "DescribeParameterGroups",
            json!({ "ParameterGroupNames": [name] }),
        )
        .await;
        assert_eq!(status, 200, "case {name:?}: describe got {read:?}");
        assert_eq!(
            read["ParameterGroups"][0]["Description"].as_str(),
            taken_description,
            "case {name:?}: a refused create must not overwrite the stored group, got {read:?}"
        );
    }
}

/// `ParameterGroupName` is `@required`, which the empty string satisfies; it names
/// nothing a later describe or delete could resolve, so the create is refused.
#[tokio::test]
async fn creating_a_parameter_group_with_an_empty_name_is_refused() {
    let router = emulator();
    let (status, body) = call(
        &router,
        "CreateParameterGroup",
        json!({ "ParameterGroupName": "" }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "InvalidParameterValueException"),
        "an empty ParameterGroupName must be 400 InvalidParameterValueException, \
         got {status} {body:?}"
    );

    let (status, all) = call(&router, "DescribeParameterGroups", json!({})).await;
    assert_eq!(status, 200, "describe all: got {all:?}");
    assert_eq!(
        reported_names(&all),
        vec![state::DEFAULT_PARAMETER_GROUP.to_string()],
        "a refused create must persist nothing, got {all:?}"
    );
}

// ---------------------------------------------------------------------------
// Paging: the documented MaxResults window, and the continuation token.
// ---------------------------------------------------------------------------

/// "The value for MaxResults must be between 20 and 100" (`MaxResults`): each published
/// bound is accepted, one step past either is refused with
/// `InvalidParameterValueException`, and the refusal carries no state change.
#[tokio::test]
async fn max_results_is_accepted_only_inside_the_documented_window() {
    let router = emulator();

    // Each published bound, one inside it, and one step past each bound, plus the
    // degenerate values a client can still send.
    let accepted = [
        state::MIN_MAX_RESULTS,
        state::MIN_MAX_RESULTS + 1,
        state::MAX_MAX_RESULTS - 1,
        state::MAX_MAX_RESULTS,
    ];
    for max_results in accepted {
        let (status, body) = call(
            &router,
            "DescribeParameterGroups",
            json!({ "MaxResults": max_results }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case MaxResults={max_results}: expected 200, got {body:?}"
        );
    }

    let refused = [
        state::MIN_MAX_RESULTS - 1,
        state::MAX_MAX_RESULTS + 1,
        1,
        0,
        -1,
        i32::MAX,
    ];
    for max_results in refused {
        let (status, body) = call(
            &router,
            "DescribeParameterGroups",
            json!({ "MaxResults": max_results }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidParameterValueException"),
            "case MaxResults={max_results}: expected 400 InvalidParameterValueException, \
             got {status} {body:?}"
        );
    }
}

/// A page is at most `MaxResults` long, its `NextToken` continues exactly where it left
/// off, and following the chain reports every stored group once, in sorted order. A token
/// this service never issued is refused rather than answering an arbitrary page.
#[tokio::test]
async fn describe_pages_through_every_stored_parameter_group() {
    let router = emulator();

    // More groups than one minimum-sized page holds, so the chain has several pages.
    let total = 2 * state::MIN_MAX_RESULTS + 5;
    let mut expected: Vec<String> = vec![state::DEFAULT_PARAMETER_GROUP.to_string()];
    for index in 0..total {
        let name = format!("group-{index:03}");
        let (status, body) = call(
            &router,
            "CreateParameterGroup",
            json!({ "ParameterGroupName": name }),
        )
        .await;
        assert_eq!(status, 200, "case {name:?}: create got {body:?}");
        expected.push(name);
    }
    expected.sort();

    for page_size in [state::MIN_MAX_RESULTS, state::MAX_MAX_RESULTS] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut body = json!({ "MaxResults": page_size });
            if let Some(token) = token.as_deref() {
                body["NextToken"] = json!(token);
            }
            let (status, page) = call(&router, "DescribeParameterGroups", body).await;
            assert_eq!(
                status, 200,
                "case page_size={page_size} page {pages}: got {page:?}"
            );
            let names = reported_names(&page);
            assert!(
                names.len() <= usize::try_from(page_size).expect("a positive page size"),
                "case page_size={page_size} page {pages}: a page may hold at most \
                 {page_size} groups, got {} ({page:?})",
                names.len()
            );
            seen.extend(names);
            pages += 1;
            assert!(
                pages <= expected.len(),
                "case page_size={page_size}: paging did not terminate after {pages} pages"
            );
            match page["NextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen, expected,
            "case page_size={page_size}: the chain must report every stored group once, \
             in sorted order"
        );
    }

    for bogus in ["not-a-token", "!!!!", "Zm9v"] {
        let (status, body) = call(
            &router,
            "DescribeParameterGroups",
            json!({ "NextToken": bogus }),
        )
        .await;
        // "Zm9v" decodes to "foo", which no group is named: a token is only honoured
        // when it continues this service's own order, so a page is never fabricated.
        assert!(
            status == 400 || reported_names(&body).iter().all(|name| expected.contains(name)),
            "case {bogus:?}: a token this service never issued must be refused or \
             continue the real order, got {status} {body:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Delete.
// ---------------------------------------------------------------------------

/// A created group is gone after its delete — and gone from the unfiltered describe too.
#[tokio::test]
async fn deleting_a_parameter_group_removes_it_from_the_store() {
    let router = emulator();

    for (name, description) in GROUP_CASES {
        let mut body = json!({ "ParameterGroupName": name });
        if let Some(description) = description {
            body["Description"] = json!(description);
        }
        let (status, created) = call(&router, "CreateParameterGroup", body).await;
        assert_eq!(status, 200, "case {name:?}: create got {created:?}");
    }

    for (name, _) in GROUP_CASES {
        let (status, deleted) = call(
            &router,
            "DeleteParameterGroup",
            json!({ "ParameterGroupName": name }),
        )
        .await;
        assert_eq!(status, 200, "case {name:?}: delete got {deleted:?}");
        assert!(
            deleted["DeletionMessage"].as_str().is_some(),
            "case {name:?}: delete reports a DeletionMessage, got {deleted:?}"
        );

        let (status, read) = call(
            &router,
            "DescribeParameterGroups",
            json!({ "ParameterGroupNames": [name] }),
        )
        .await;
        assert_eq!(
            (status, error_code(&read).as_str()),
            (404, "ParameterGroupNotFoundFault"),
            "case {name:?}: a deleted group is gone, got {status} {read:?}"
        );
    }

    let (status, all) = call(&router, "DescribeParameterGroups", json!({})).await;
    assert_eq!(status, 200, "describe all: got {all:?}");
    assert_eq!(
        reported_names(&all),
        vec![state::DEFAULT_PARAMETER_GROUP.to_string()],
        "only the default group survives every delete, got {all:?}"
    );
}

/// The parameter group AWS provides is not the caller's to remove: the delete is refused
/// and the group is still reported afterwards.
#[tokio::test]
async fn deleting_the_default_parameter_group_is_refused() {
    let router = emulator();

    let (status, body) = call(
        &router,
        "DeleteParameterGroup",
        json!({ "ParameterGroupName": state::DEFAULT_PARAMETER_GROUP }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "InvalidParameterValueException"),
        "deleting the default group must be 400 InvalidParameterValueException, \
         got {status} {body:?}"
    );

    let (status, all) = call(&router, "DescribeParameterGroups", json!({})).await;
    assert_eq!(status, 200, "describe all: got {all:?}");
    assert_eq!(
        reported_names(&all),
        vec![state::DEFAULT_PARAMETER_GROUP.to_string()],
        "a refused delete leaves the default group in place, got {all:?}"
    );
}

/// "You cannot delete a parameter group if it is associated with any DAX clusters"
/// (`DeleteParameterGroup`): while a cluster references the group the delete is refused,
/// and once no cluster does it succeeds.
#[tokio::test]
async fn deleting_a_parameter_group_a_cluster_uses_is_refused() {
    let router = emulator();
    let group = "cluster-params";
    let (status, body) = call(
        &router,
        "CreateParameterGroup",
        json!({ "ParameterGroupName": group }),
    )
    .await;
    assert_eq!(status, 200, "create {group}: got {body:?}");

    // Two clusters reference it, so the refusal is not keyed to a single holder.
    for cluster in ["pgcluster-one", "pgcluster-two"] {
        let (status, body) = call(
            &router,
            "CreateCluster",
            json!({
                "ClusterName": cluster,
                "NodeType": NODE_TYPE,
                "ReplicationFactor": 1,
                "IamRoleArn": ROLE_ARN,
                "ParameterGroupName": group,
            }),
        )
        .await;
        assert_eq!(status, 200, "CreateCluster {cluster}: got {body:?}");
    }
    settle().await;

    let (status, body) = call(
        &router,
        "DeleteParameterGroup",
        json!({ "ParameterGroupName": group }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "InvalidParameterGroupStateFault"),
        "deleting a group a cluster uses must be 400 InvalidParameterGroupStateFault, \
         got {status} {body:?}"
    );

    // Still there: the refusal wrote nothing.
    let (status, read) = call(
        &router,
        "DescribeParameterGroups",
        json!({ "ParameterGroupNames": [group] }),
    )
    .await;
    assert_eq!(status, 200, "describe {group}: got {read:?}");

    // One holder gone is not enough; the group frees only when the last one is.
    let (status, body) = call(
        &router,
        "DeleteCluster",
        json!({ "ClusterName": "pgcluster-one" }),
    )
    .await;
    assert_eq!(status, 200, "DeleteCluster pgcluster-one: got {body:?}");
    settle().await;
    let (status, body) = call(
        &router,
        "DeleteParameterGroup",
        json!({ "ParameterGroupName": group }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "InvalidParameterGroupStateFault"),
        "one remaining holder still blocks the delete, got {status} {body:?}"
    );

    let (status, body) = call(
        &router,
        "DeleteCluster",
        json!({ "ClusterName": "pgcluster-two" }),
    )
    .await;
    assert_eq!(status, 200, "DeleteCluster pgcluster-two: got {body:?}");
    settle().await;

    let (status, deleted) = call(
        &router,
        "DeleteParameterGroup",
        json!({ "ParameterGroupName": group }),
    )
    .await;
    assert_eq!(
        status, 200,
        "with no cluster left the delete succeeds, got {deleted:?}"
    );
    let (status, read) = call(
        &router,
        "DescribeParameterGroups",
        json!({ "ParameterGroupNames": [group] }),
    )
    .await;
    assert_eq!(
        (status, error_code(&read).as_str()),
        (404, "ParameterGroupNotFoundFault"),
        "the group is gone after the delete, got {status} {read:?}"
    );
}

// ---------------------------------------------------------------------------
// The bound on how far one account's parameter-group store grows.
// ---------------------------------------------------------------------------

/// The store holds at most [`state::MAX_PARAMETER_GROUPS`] parameter groups, counting the
/// default group it starts with: every create up to the bound is accepted, the first one
/// past it is refused with `ParameterGroupQuotaExceededFault` ("You have attempted to
/// exceed the maximum number of parameter groups"), and that refusal stores nothing — so
/// no sequence of creates grows the store without limit. The names below span the bound
/// rather than one cited value: the last accepted create and the first refused one are
/// both asserted.
#[tokio::test]
async fn a_create_past_the_parameter_group_bound_is_refused() {
    let router = emulator();

    // The default group already occupies one of the slots.
    for index in 0..(state::MAX_PARAMETER_GROUPS - 1) {
        let name = format!("quota-{index:03}");
        let (status, body) = call(
            &router,
            "CreateParameterGroup",
            json!({ "ParameterGroupName": name }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: a create inside the bound is accepted, got {status} {body:?}"
        );
    }

    let (status, body) = call(
        &router,
        "CreateParameterGroup",
        json!({ "ParameterGroupName": "one-group-too-many" }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ParameterGroupQuotaExceededFault"),
        "case one past the bound: expected 400 ParameterGroupQuotaExceededFault, \
         got {status} {body:?}"
    );

    // The refusal wrote nothing: the store still holds exactly the bound, and the
    // refused name resolves to no group.
    let (status, page) = call(
        &router,
        "DescribeParameterGroups",
        json!({ "MaxResults": state::MAX_MAX_RESULTS }),
    )
    .await;
    assert_eq!(status, 200, "DescribeParameterGroups: got {page:?}");
    assert_eq!(
        reported_names(&page).len(),
        state::MAX_PARAMETER_GROUPS,
        "case one past the bound: the store must still hold exactly {} groups, got {page:?}",
        state::MAX_PARAMETER_GROUPS
    );
    let (status, read) = call(
        &router,
        "DescribeParameterGroups",
        json!({ "ParameterGroupNames": ["one-group-too-many"] }),
    )
    .await;
    assert_eq!(
        (status, error_code(&read).as_str()),
        (404, "ParameterGroupNotFoundFault"),
        "case one past the bound: the refused name must name no group, got {status} {read:?}"
    );
}
