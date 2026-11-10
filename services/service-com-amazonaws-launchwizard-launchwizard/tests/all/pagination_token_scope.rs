// SPDX-License-Identifier: CC0-1.0

//! A pagination token is usable only in the result set that issued it.
//!
//! The rule under test: a `nextToken` carries a cursor that names the item the previous
//! page ended on and no other item this service pages, so an operation whose result set
//! the request NARROWS builds that cursor out of the narrowing resources as well as the
//! item's own name. A token one scope issued therefore names nothing in another scope's
//! result set and is refused there with the fixed pagination refusal, instead of being
//! matched against a different list and resuming in the middle of it — which would answer
//! HTTP 200 while silently skipping every item before the match.
//!
//! The rows span every paginated operation of this service whose result set the request
//! narrows, and they differ along how many members narrow it and how the two scopes of a
//! replay differ:
//!
//! * `ListDeploymentPatternVersions` — narrowed by `workloadName` AND
//!   `deploymentPatternName`, over every `(workload, pattern)` pair the catalogue holds,
//!   so a replay across two patterns of ONE workload and a replay across two workloads
//!   are both rows (and the catalogue publishes the same version names under every
//!   pattern, so the version's own name tells no two of them apart);
//! * `ListWorkloadDeploymentPatterns` — narrowed by `workloadName`, over every workload;
//!   * `ListDeploymentEvents` — narrowed by `deploymentId`, over two deployments.
//!
//! Each row also pins the other half of the rule: the token IS accepted by the set that
//! issued it, resuming at the very next item, and after the cross-scope refusal a walk of
//! the other set still reports every item it holds.

use std::time::Duration;

use serde_json::{json, Map, Value};
use service_com_amazonaws_launchwizard_launchwizard::state::{
    scoped_cursor, INVALID_NEXT_TOKEN_MESSAGE,
};

use crate::deployment_lifecycle::create;
use crate::deployment_update::{error_message, every_pattern, one_pattern_per_workload};
use crate::harness::{catalogue, one_page, walk_pages, Harness};

/// The HTTP status a refused pagination token carries (`ValidationException`,
/// `errorKind: client`, `httpError: 400`).
const STATUS: u16 = 400;
/// See [`STATUS`].
const CODE: &str = "ValidationException";

/// `maxResults` is `@range(min: 1, max: 100)`, so this is the largest page the shape
/// allows — enough to hold any of the result sets below whole.
const WHOLE_SET: usize = 100;

/// Long enough for the clock-owed lifecycle steps a create schedules to come due, so a
/// created deployment's history holds more than one event before a row pages over it. The
/// emulator never sleeps; this is the test waiting for the clock.
const CREATE_SETTLE: Duration = Duration::from_millis(2_100);

/// One result set a paginated operation serves: the request members that NARROW it, plus
/// a label for the assertions.
struct Scope {
    label: String,
    body: Value,
}

/// One paginated operation whose result set the request narrows, with the scopes a token
/// is replayed across.
struct Paged {
    path: &'static str,
    items: &'static str,
    scopes: Vec<Scope>,
}

/// One page of a paginated operation: the items it reported and the token it handed back.
///
/// The reply is read through the shared single-page helper, so this suite and the others
/// that assert the token contract cannot disagree about how a page is read; what belongs
/// to this suite is the SCOPE the request narrows by.
async fn page(
    harness: &Harness,
    case: &str,
    paged: &Paged,
    scope: &Value,
    page_size: usize,
    token: Option<&str>,
) -> (Vec<Value>, Option<String>) {
    one_page(
        harness,
        case,
        paged.path,
        request(scope, page_size, token),
        paged.items,
    )
    .await
}

/// A scope's request body with the paging members this suite drives.
fn request(scope: &Value, page_size: usize, token: Option<&str>) -> Value {
    let mut request: Map<String, Value> = scope
        .as_object()
        .cloned()
        .expect("a paginated request body is an object");
    request.insert("maxResults".to_string(), json!(page_size));
    if let Some(token) = token {
        request.insert("nextToken".to_string(), json!(token));
    }
    Value::Object(request)
}

/// Every paginated operation of this service whose result set the request narrows, each
/// with the scopes the rows replay a token across.
async fn narrowed_operations(harness: &Harness) -> Vec<Paged> {
    // A version is named only within the pattern that publishes it, so the scopes are
    // every `(workload, pattern)` pair the catalogue holds.
    let versions = Paged {
        path: "/listDeploymentPatternVersions",
        items: "deploymentPatternVersions",
        scopes: every_pattern(harness)
            .await
            .into_iter()
            .map(|(workload, pattern)| Scope {
                label: format!("{workload}/{pattern}"),
                body: json!({"workloadName": workload, "deploymentPatternName": pattern}),
            })
            .collect(),
    };
    // A pattern is named only within its workload.
    let patterns = Paged {
        path: "/listWorkloadDeploymentPatterns",
        items: "workloadDeploymentPatterns",
        scopes: catalogue(harness)
            .await
            .into_iter()
            .map(|workload| Scope {
                label: workload.clone(),
                body: json!({"workloadName": workload}),
            })
            .collect(),
    };
    // An event is named by its append ordinal within ONE deployment's log — the sibling
    // path, held to the same rule here.
    let mut deployments = Vec::new();
    for (index, (workload, pattern)) in one_pattern_per_workload(harness)
        .await
        .into_iter()
        .take(2)
        .enumerate()
    {
        let case = format!("{workload}/{pattern}");
        let id = create(
            harness,
            &case,
            &workload,
            &pattern,
            &format!("token-scope-{index}"),
        )
        .await;
        deployments.push(Scope {
            label: format!("{case} deployment {index}"),
            body: json!({"deploymentId": id}),
        });
    }
    std::thread::sleep(CREATE_SETTLE);
    let events = Paged {
        path: "/listDeploymentEvents",
        items: "deploymentEvents",
        scopes: deployments,
    };
    vec![versions, patterns, events]
}

#[tokio::test]
async fn a_token_resumes_only_the_result_set_that_issued_it() {
    let harness = Harness::new();
    for paged in narrowed_operations(&harness).await {
        assert!(
            paged.scopes.len() >= 2,
            "case {:?}: a replay needs at least two scopes, got {}",
            paged.path,
            paged.scopes.len()
        );
        for (index, issuer) in paged.scopes.iter().enumerate() {
            let case = format!("{} {}", paged.path, issuer.label);
            // The whole set, so the rows below can say exactly which item a token must
            // resume after and which items a refused replay must not skip.
            let (whole, _) = page(&harness, &case, &paged, &issuer.body, WHOLE_SET, None).await;
            assert!(
                whole.len() >= 2,
                "case {case:?}: a token is only issued over a set of more than one item, got {}",
                whole.len()
            );

            let (first, token) = page(&harness, &case, &paged, &issuer.body, 1, None).await;
            assert_eq!(
                first,
                whole[..1].to_vec(),
                "case {case:?}: the first page must hold the first item of the set, got {first:?}"
            );
            let token = token.unwrap_or_else(|| {
                panic!("case {case:?}: a set longer than one page must hand back a token")
            });

            // The set that issued it accepts it, resuming at the very next item.
            let resumed_case = format!("{case} resumed in its own set");
            let (second, _) =
                page(&harness, &resumed_case, &paged, &issuer.body, 1, Some(&token)).await;
            assert_eq!(
                second,
                whole[1..2].to_vec(),
                "case {resumed_case:?}: expected the item after the one the page ended on \
                 {:?}, got {second:?}",
                whole[1]
            );

            for (other_index, other) in paged.scopes.iter().enumerate() {
                if other_index == index {
                    continue;
                }
                let case = format!(
                    "{}: a token issued for {} replayed on {}",
                    paged.path, issuer.label, other.label
                );
                let body = harness
                    .refused(
                        &case,
                        paged.path,
                        request(&other.body, 1, Some(&token)),
                        STATUS,
                        CODE,
                    )
                    .await;
                let message = error_message(&body);
                assert_eq!(
                    message, INVALID_NEXT_TOKEN_MESSAGE,
                    "case {case:?}: expected the fixed pagination refusal, got {message:?}"
                );
                assert!(
                    !message.contains(token.as_str()),
                    "case {case:?}: the refusal echoes the rejected token, got {message:?}"
                );
                assert!(
                    body.get(paged.items).is_none(),
                    "case {case:?}: a refused request must report no items, got {body}"
                );

                // Nothing of the other set was skipped: its own walk still reports every
                // item it holds, from the first.
                let (other_whole, _) =
                    page(&harness, &case, &paged, &other.body, WHOLE_SET, None).await;
                let walked =
                    walk_pages(&harness, &case, paged.path, &other.body, paged.items, 1).await;
                assert_eq!(
                    walked, other_whole,
                    "case {case:?}: the other set must still page every item it holds, got {} of \
                     {}",
                    walked.len(),
                    other_whole.len()
                );
            }
        }
    }
}

#[test]
fn a_scoped_cursor_tells_items_of_different_scopes_apart() {
    // Parts drawn from the shapes a cursor of this service is built out of, read off the
    // model rather than written here from memory:
    // smithy_query(query="shape",shape="WorkloadName,DeploymentPatternName,
    // DeploymentPatternVersionName,DeploymentId") — a `WorkloadName` is
    // ^[A-Za-z][a-zA-Z0-9-_]*$, a `DeploymentPatternName` ^[A-Za-z0-9][a-zA-Z0-9-]*$, a
    // `DeploymentPatternVersionName` len 5..30 with pattern
    // ^(([A-Za-z0-9][a-zA-Z0-9-]*)|(\d+\.\d+\.\d+))$ and a `DeploymentId`
    // ^[a-zA-Z0-9-]+$. The rows include names the seeded catalogue does not hold (an
    // underscore, a hyphen, a digit-leading pattern, a version name that is not a dotted
    // number) so the rule is not tested only on the entries it ships.
    let scopes = ["SAP", "A", "Work_load-9", "SAPx"];
    let holders = ["SapHanaSingle", "0pattern", "a-b", "SapHanaSingl"];
    let items = ["1.0.0", "10.20.30", "0.0.0", "Named-version1"];

    let mut cursors: Vec<(String, String)> = Vec::new();
    for scope in scopes {
        for holder in holders {
            for item in items {
                let case = format!("{scope}/{holder}@{item}");
                cursors.push((case, scoped_cursor([scope, holder, item])));
            }
        }
    }
    for (left_case, left) in &cursors {
        for (right_case, right) in &cursors {
            if left_case == right_case {
                continue;
            }
            assert_ne!(
                left, right,
                "case {left_case:?} vs {right_case:?}: two different items must not share the \
                 cursor {left:?}"
            );
        }
    }
    // One scope's cursor is a function of its parts only, so the same item always gets
    // the same cursor: a token stays usable across two calls of one result set.
    assert_eq!(
        scoped_cursor(["SAP", "SapHanaSingle", "1.0.0"]),
        scoped_cursor(["SAP", "SapHanaSingle", "1.0.0"]),
        "one item's cursor must be stable across two builds"
    );
}
