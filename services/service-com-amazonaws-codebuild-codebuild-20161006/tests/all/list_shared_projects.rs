// SPDX-License-Identifier: CC0-1.0

//! `ListSharedProjects`, over the wire the router serves: `POST /` with
//! `X-Amz-Target: CodeBuild_20161006.ListSharedProjects` on awsJson1_1.
//!
//! RULE: the listing reports the ARNs of exactly those build projects of the account
//! that carry a resource-based sharing policy — the record a project is shared with
//! other accounts or users by — ordered by the criterion and direction the request
//! asks for, cut into pages of at most `maxResults` entries that `nextToken` resumes;
//! a project's `projectVisibility` is a different grant and never makes a project
//! shared; a policy held for another resource kind lists no project; and the read
//! leaves the store exactly as it found it.
//!
//! The sharing record is seeded straight into the service's state here, through the
//! same seam `report_group_reads` beside this one seeds a report group with: seeding it
//! is what proves this listing is computed from the store rather than from the request,
//! and the operation that stores one over the wire (`PutResourcePolicy`) is covered by
//! `resource_policy` beside this suite, including the shared listing it feeds.
//!
//! The harness (`call`, `create_project`, `emulator_on`, `error_type`, `REGION`) is the
//! one `project_build_lifecycle` already drives this service with.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_codebuild_codebuild_20161006::state::{
    self, ComAmazonawsCodebuildCodebuild20161006State as CodeBuildState, StoredResourcePolicy,
};
use smithy_server_runtime::MultiServiceRouter;

use super::project_build_lifecycle::{call, create_project, emulator_on, error_type, REGION};

/// A resource-based policy document granting another account read access to `arn` —
/// one of the grants the CodeBuild user guide's "Sharing a Project" describes. Its
/// CONTENT is not what this listing reads (the attachment is), so one document serves
/// every row.
fn sharing_policy(arn: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"AWS": "arn:aws:iam::222222222222:root"},
            "Action": ["codebuild:BatchGetProjects", "codebuild:ListBuildsForProject"],
            "Resource": arn,
        }],
    })
    .to_string()
}

/// Attach a sharing policy to `arn` in the store the handlers read.
fn share(engine: &engine::InMemoryEngine, arn: &str) {
    engine.state::<CodeBuildState>().lock().resource_policies.insert(
        arn.to_string(),
        StoredResourcePolicy {
            policy: sharing_policy(arn),
            last_modified_millis: state::now_millis(),
        },
    );
}

/// Call `ListSharedProjects` with `request`, asserting it succeeded, and hand back the
/// body.
async fn list(router: &MultiServiceRouter, request: Value) -> Value {
    let (status, body) = call(router, "ListSharedProjects", request.clone()).await;
    assert_eq!(
        status, 200,
        "ListSharedProjects {request}: expected 200, got {status} with {body}"
    );
    body
}

/// The `projects` member of a listing, as a vector of ARNs — asserting it was stated
/// as a list at all.
fn listed(body: &Value, case: &str) -> Vec<String> {
    let entries = body["projects"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: projects must be stated as a list, got {body}"));
    entries
        .iter()
        .map(|entry| {
            entry
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: every entry is an ARN, got {body}"))
                .to_string()
        })
        .collect()
}

/// Create `names` as projects of the account and hand back the ARN each was stored
/// with, read from the service's own answer.
async fn create_projects(router: &MultiServiceRouter, names: &[&str]) -> Vec<String> {
    let mut arns = Vec::new();
    for name in names {
        let project = create_project(router, name, json!({})).await;
        arns.push(
            project["arn"]
                .as_str()
                .unwrap_or_else(|| panic!("CreateProject {name:?} must report an ARN, got {project}"))
                .to_string(),
        );
    }
    arns
}

/// RULE: exactly the projects carrying a sharing policy are listed — whatever their
/// visibility, and whatever else the store holds.
#[tokio::test]
async fn only_the_projects_carrying_a_sharing_policy_are_listed() {
    // The rows differ along how many of the account's projects are shared (none, one,
    // a middle subset, all) and along the visibility of the shared and unshared ones,
    // which the rule says is irrelevant. `shared` and `public` index into `names`.
    struct Case {
        case: &'static str,
        names: &'static [&'static str],
        shared: &'static [usize],
        public: &'static [usize],
        /// Published and then taken private again — a project whose visibility moved
        /// and ended where it started.
        republished: &'static [usize],
    }
    let cases = [
        Case {
            case: "an account that shares nothing",
            names: &["alpha", "bravo"],
            shared: &[],
            public: &[],
            republished: &[],
        },
        Case {
            case: "an account whose only project is shared",
            names: &["alpha"],
            shared: &[0],
            public: &[],
            republished: &[],
        },
        Case {
            case: "one of three shared",
            names: &["alpha", "bravo", "charlie"],
            shared: &[1],
            public: &[],
            republished: &[],
        },
        Case {
            case: "all three shared",
            names: &["alpha", "bravo", "charlie"],
            shared: &[0, 1, 2],
            public: &[],
            republished: &[],
        },
        Case {
            case: "a public project that is NOT shared",
            names: &["alpha", "bravo"],
            shared: &[],
            public: &[0, 1],
            republished: &[],
        },
        Case {
            case: "a public project that IS shared, beside a private shared one",
            names: &["alpha", "bravo", "charlie"],
            shared: &[0, 1],
            public: &[0],
            republished: &[],
        },
        Case {
            case: "a shared project published and taken private again",
            names: &["alpha", "bravo"],
            shared: &[1],
            public: &[],
            republished: &[1],
        },
        Case {
            case: "an unshared project published and taken private again",
            names: &["alpha", "bravo"],
            shared: &[],
            public: &[],
            republished: &[0],
        },
    ];

    for Case {
        case,
        names,
        shared,
        public,
        republished,
    } in cases
    {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let router = emulator_on(Arc::clone(&engine));
        let arns = create_projects(&router, names).await;

        for (index, visibilities) in public
            .iter()
            .map(|index| (index, &["PUBLIC_READ"][..]))
            .chain(
                republished
                    .iter()
                    .map(|index| (index, &["PUBLIC_READ", "PRIVATE"][..])),
            )
        {
            for visibility in visibilities {
                let (status, body) = call(
                    &router,
                    "UpdateProjectVisibility",
                    json!({"projectArn": arns[*index], "projectVisibility": visibility}),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: setting {} to {visibility} must succeed, \
                     got {status} with {body}",
                    arns[*index]
                );
            }
        }
        for index in shared {
            share(&engine, &arns[*index]);
        }
        // A policy for another resource kind CodeBuild shares: it names no project, so
        // it must list none.
        share(&engine, &state::report_group_arn(REGION, "shared-group"));

        let mut expected: Vec<String> = shared.iter().map(|index| arns[*index].clone()).collect();
        expected.sort();
        let body = list(&router, json!({})).await;
        assert_eq!(
            listed(&body, case),
            expected,
            "case {case:?}: expected the shared projects {expected:?}, got {body}"
        );
        assert!(
            body["nextToken"].is_null(),
            "case {case:?}: one page holds every entry, so no nextToken may be stated, got {body}"
        );

        // The read is reporting only: a second call answers the same thing and the
        // store is untouched.
        let again = list(&router, json!({})).await;
        assert_eq!(
            again, body,
            "case {case:?}: a second read must answer the same listing, got {again}"
        );
        let store = engine.state::<CodeBuildState>();
        let held = store.lock();
        assert_eq!(
            (held.projects.len(), held.resource_policies.len()),
            (names.len(), shared.len() + 1),
            "case {case:?}: a read must neither add to nor drop from the store"
        );
    }
}

/// RULE: the listing is ordered by the requested criterion in the requested direction —
/// by ARN or by when each shared project was last changed, ascending or descending —
/// and an absent member takes the ascending-by-ARN default.
#[tokio::test]
async fn the_listing_is_ordered_by_the_requested_criterion_and_direction() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));
    // Four projects of this account, every one of them shared.
    let names = ["ord-a", "ord-b", "ord-c", "ord-d"];
    let arns = create_projects(&router, &names).await;
    for arn in &arns {
        share(&engine, arn);
    }
    // The instant each project was last changed at, seeded as a fixed value rather than
    // taken from the clock, so the modified order DISAGREES with the ARN order and each
    // row below can only pass for its own criterion. `ord-c` and `ord-d` carry the same
    // instant, which is what the ARN tie-break is for.
    let stamps = [
        ("ord-a", 1_700_000_300_000_i64),
        ("ord-b", 1_700_000_100_000),
        ("ord-c", 1_700_000_200_000),
        ("ord-d", 1_700_000_200_000),
    ];
    {
        let store = engine.state::<CodeBuildState>();
        let mut held = store.lock();
        for (name, millis) in stamps {
            let stored = held
                .projects
                .get_mut(name)
                .unwrap_or_else(|| panic!("the store holds {name:?} after CreateProject"));
            stored.last_modified_millis = millis;
            stored.project.last_modified = Some(state::datetime(millis));
        }
    }

    let by_arn: Vec<String> = {
        let mut sorted = arns.clone();
        sorted.sort();
        sorted
    };
    // The seeded instants, ties broken by ARN exactly as a total order needs.
    let by_modified_time: Vec<String> = {
        let mut stamped: Vec<(i64, String)> = stamps
            .iter()
            .map(|(name, millis)| (*millis, state::project_arn(REGION, name)))
            .collect();
        stamped.sort();
        stamped.into_iter().map(|(_, arn)| arn).collect()
    };
    assert_ne!(
        by_arn, by_modified_time,
        "the two criteria must disagree for this table to mean anything: {by_arn:?}"
    );

    let descending = |order: &[String]| -> Vec<String> {
        let mut reversed = order.to_vec();
        reversed.reverse();
        reversed
    };

    // The rows span both values of `SharedResourceSortByType` (ARN | MODIFIED_TIME) and
    // both of `SortOrderType` (ASCENDING | DESCENDING), plus each member absent.
    let rows: Vec<(&str, Value, Vec<String>)> = vec![
        ("neither member", json!({}), by_arn.clone()),
        (
            "ARN, ascending",
            json!({"sortBy": "ARN", "sortOrder": "ASCENDING"}),
            by_arn.clone(),
        ),
        (
            "ARN, descending",
            json!({"sortBy": "ARN", "sortOrder": "DESCENDING"}),
            descending(&by_arn),
        ),
        ("ARN, no order", json!({"sortBy": "ARN"}), by_arn.clone()),
        (
            "MODIFIED_TIME, ascending",
            json!({"sortBy": "MODIFIED_TIME", "sortOrder": "ASCENDING"}),
            by_modified_time.clone(),
        ),
        (
            "MODIFIED_TIME, descending",
            json!({"sortBy": "MODIFIED_TIME", "sortOrder": "DESCENDING"}),
            descending(&by_modified_time),
        ),
        (
            "MODIFIED_TIME, no order",
            json!({"sortBy": "MODIFIED_TIME"}),
            by_modified_time.clone(),
        ),
        (
            "only an order",
            json!({"sortOrder": "DESCENDING"}),
            descending(&by_arn),
        ),
    ];

    for (case, request, expected) in rows {
        let body = list(&router, request.clone()).await;
        assert_eq!(
            listed(&body, case),
            expected,
            "case {case:?} ({request}): expected {expected:?}, got {body}"
        );
    }
}

/// RULE: the listing is cut into pages of at most `maxResults` entries, `nextToken`
/// resumes the next page, the last page states none, and walking the pages yields the
/// whole ordering exactly once.
#[tokio::test]
async fn the_listing_pages_by_max_results_and_resumes_at_the_next_token() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));
    let names: Vec<String> = (0..5).map(|index| format!("paged-{index}")).collect();
    let borrowed: Vec<&str> = names.iter().map(String::as_str).collect();
    let arns = create_projects(&router, &borrowed).await;
    for arn in &arns {
        share(&engine, arn);
    }
    let mut expected = arns.clone();
    expected.sort();

    // The sizes span one entry per page, a size that divides the count, one that does
    // not, the exact count, and `PageSize`'s published maximum of 100 (from
    // `smithy_query(query="operation",operation="ListSharedProjects")`: @range(min: 1,
    // max: 100)), where one page holds everything.
    for page_size in [1_usize, 2, 3, 5, 100] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0_usize;
        loop {
            let mut request = json!({"maxResults": page_size});
            if let (Value::Object(target), Some(token)) = (&mut request, token.as_ref()) {
                target.insert("nextToken".to_string(), json!(token));
            }
            let case = format!("maxResults {page_size}, page {pages}");
            let body = list(&router, request).await;
            let page = listed(&body, &case);
            assert!(
                !page.is_empty() && page.len() <= page_size,
                "case {case:?}: a page holds 1..={page_size} entries, got {body}"
            );
            walked.extend(page);
            pages += 1;
            token = body["nextToken"].as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
            assert!(
                pages <= expected.len(),
                "case {case:?}: paging must terminate, walked {walked:?}"
            );
        }
        assert_eq!(
            walked, expected,
            "case {page_size}: walking the pages must yield every shared project once, \
             in order, got {walked:?}"
        );
        assert_eq!(
            pages,
            expected.len().div_ceil(page_size),
            "case {page_size}: unexpected page count for {} entries",
            expected.len()
        );
    }

    // The token is the entry the page opens at, so the one the FIRST page reported
    // resumes at the second entry.
    let first = list(&router, json!({"maxResults": 1})).await;
    let resumed = list(
        &router,
        json!({"maxResults": 1, "nextToken": first["nextToken"]}),
    )
    .await;
    assert_eq!(
        listed(&resumed, "resumed"),
        vec![expected[1].clone()],
        "the reported token must resume at the next entry, got {resumed}"
    );

    // A token naming nothing in the listing is a bad input, not an empty page.
    for bogus in [
        "not-a-token".to_string(),
        state::project_arn(REGION, "never-created"),
        state::project_arn("eu-west-1", "paged-0"),
    ] {
        let (status, body) = call(
            &router,
            "ListSharedProjects",
            json!({"nextToken": bogus.clone()}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {bogus:?}: an unresolvable nextToken is refused, got {status} with {body}"
        );
        assert_eq!(
            error_type(&body),
            "InvalidInputException",
            "case {bogus:?}: CodeBuild states a bad input as InvalidInputException, got {body}"
        );
    }
}
