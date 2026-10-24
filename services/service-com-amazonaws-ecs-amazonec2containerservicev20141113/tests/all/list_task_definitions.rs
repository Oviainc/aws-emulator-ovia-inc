// SPDX-License-Identifier: CC0-1.0

//! `ListTaskDefinitions`, driven end to end over the shared wire harness in
//! [`crate::ecs_wire`].
//!
//! The rule under test: `ListTaskDefinitions` answers with the ARNs of the
//! task-definition revisions STORED for the account whose status is the one the request
//! filters on (`ACTIVE` when it names none) and whose family is the one `familyPrefix`
//! names in full (every family when it names none), ordered lexicographically by family
//! name and then by ascending revision (reversed when `sort` is `DESC`), cut into pages
//! of `maxResults` entries whose `nextToken` resumes the walk where the page stopped.
//!
//! The docs the rows are read off:
//!
//! * `ListTaskDefinitions$familyPrefix`: "The full family name to filter the
//!   ListTaskDefinitions results with. Specifying a familyPrefix limits the listed task
//!   definitions to task definition revisions that belong to that family."
//! * `ListTaskDefinitions$status`: "By default, only ACTIVE task definitions are listed.
//!   By setting this parameter to INACTIVE, you can view task definitions that are
//!   INACTIVE".
//! * `ListTaskDefinitions$sort`: "By default, (ASC) task definitions are listed
//!   lexicographically by family name and in ascending numerical order by revision so
//!   that the newest task definitions in a family are listed last. Setting this parameter
//!   to DESC reverses the sort order on family name and revision."
//! * `ListTaskDefinitions$maxResults`: "This value can be between 1 and 100. If this
//!   parameter isn't used, then ListTaskDefinitions returns up to 100 results and a
//!   nextToken value if applicable."
//! * `ListTaskDefinitions$nextToken` (output): "This value is null when there are no more
//!   results to return."
//!
//! Every case goes through the same front door a client uses, so nothing reaches into the
//! state struct and a response that is not computed from persisted state cannot pass.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    list, ok, refusal, refused, register_definition, service, string, ACCOUNT, SIGNED_REGION,
};

/// Every value `TaskDefinitionStatus` takes, from the model's enum (smithy_query
/// query="shape" shape="TaskDefinitionStatus": ACTIVE, DELETE_IN_PROGRESS, INACTIVE).
const TASK_DEFINITION_STATUSES: [&str; 3] = ["ACTIVE", "DELETE_IN_PROGRESS", "INACTIVE"];

/// Every value `SortOrder` takes (smithy_query query="shape" shape="SortOrder": ASC,
/// DESC).
const SORT_ORDERS: [&str; 2] = ["ASC", "DESC"];

/// How many pages a paging walk may take before the test calls it a loop. Generous: the
/// smallest page size under test is one entry.
const MAX_PAGES: usize = 64;

/// The ARN one revision of `family` carries
/// (`arn:aws:ecs:<region>:<account>:task-definition/<family>:<revision>`).
fn definition_arn(family: &str, revision: u32) -> String {
    format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:task-definition/{family}:{revision}")
}

/// The ARNs of `revisions`, in the order given.
fn arns(revisions: &[(&str, u32)]) -> Vec<String> {
    revisions
        .iter()
        .map(|(family, revision)| definition_arn(family, *revision))
        .collect()
}

/// ONE `ListTaskDefinitions` page: the ARNs it carries and the `nextToken` it hands back.
async fn page(
    router: &MultiServiceRouter,
    request: Value,
    case: &str,
) -> (Vec<String>, Option<String>) {
    let body = ok(router, "ListTaskDefinitions", request, case).await;
    let arns = list(&body, "taskDefinitionArns", case)
        .iter()
        .map(|value| {
            value
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a string ARN, got {value}"))
                .to_string()
        })
        .collect();
    let next_token = body["nextToken"].as_str().map(str::to_string);
    (arns, next_token)
}

/// Every ARN `request` reaches, in page order, walking `nextToken` to the end.
async fn all_pages(router: &MultiServiceRouter, request: Value, case: &str) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let mut this_page = request.clone();
        if let Some(token) = &token {
            this_page["nextToken"] = json!(token);
        }
        let (arns, next) = page(router, this_page, case).await;
        seen.extend(arns);
        match next {
            None => return seen,
            Some(next) => token = Some(next),
        }
    }
    panic!("case {case:?}: ListTaskDefinitions never ran out of pages within {MAX_PAGES} pages");
}

/// Register `count` revisions of `family`, asserting each one takes the next revision
/// number — the family counter the listing's order is read off.
async fn register_revisions(router: &MultiServiceRouter, family: &str, count: u32, case: &str) {
    for revision in 1..=count {
        let arn = register_definition(router, family, case).await;
        assert_eq!(
            arn,
            definition_arn(family, revision),
            "case {case:?}: registration {revision} of {family:?} must take revision {revision}"
        );
    }
}

/// Deregister ONE revision, which is what moves it out of `ACTIVE`.
async fn deregister(router: &MultiServiceRouter, family: &str, revision: u32, case: &str) {
    let body = ok(
        router,
        "DeregisterTaskDefinition",
        json!({ "taskDefinition": format!("{family}:{revision}") }),
        case,
    )
    .await;
    assert_eq!(
        string(&body["taskDefinition"], "status", case),
        "INACTIVE",
        "case {case:?}: a deregistered revision is INACTIVE"
    );
}

/// The three families every case below reads: one with several revisions, one whose name
/// EXTENDS that family's name (so a leading fragment that is not a full family name is
/// visible), and one that sorts after both.
async fn stored_revisions(router: &MultiServiceRouter) {
    let case = "setup";
    register_revisions(router, "alpha", 3, case).await;
    register_revisions(router, "alphaextra", 1, case).await;
    register_revisions(router, "beta", 2, case).await;
    // One revision out of ACTIVE, so a status filter has something to tell apart.
    deregister(router, "alpha", 2, case).await;
}

#[tokio::test]
async fn list_task_definitions_reports_the_stored_revisions_narrowed_by_every_filter() {
    let router = service();
    stored_revisions(&router).await;

    // ASC is the documented default order: family name lexicographically, then revision
    // ascending. `alpha:2` was deregistered, so it is not in the ACTIVE listing.
    let active_asc = arns(&[("alpha", 1), ("alpha", 3), ("alphaextra", 1), ("beta", 1), ("beta", 2)]);
    let mut active_desc = active_asc.clone();
    active_desc.reverse();

    // The rows differ along what the rule generalises over: the status filtered on (each
    // value the model's enum carries), the family named (in full, as a leading fragment
    // of two families, and as a family nothing was registered under) and the sort order.
    let cases: Vec<(&str, Value, Vec<String>)> = vec![
        (
            "no filter at all lists every ACTIVE revision in ASC order",
            json!({}),
            active_asc.clone(),
        ),
        (
            "status ACTIVE is the default made explicit",
            json!({ "status": TASK_DEFINITION_STATUSES[0] }),
            active_asc.clone(),
        ),
        (
            "status DELETE_IN_PROGRESS matches no stored revision",
            json!({ "status": TASK_DEFINITION_STATUSES[1] }),
            Vec::new(),
        ),
        (
            "status INACTIVE lists only the deregistered revision",
            json!({ "status": TASK_DEFINITION_STATUSES[2] }),
            arns(&[("alpha", 2)]),
        ),
        (
            "sort ASC is the default made explicit",
            json!({ "sort": SORT_ORDERS[0] }),
            active_asc.clone(),
        ),
        (
            "sort DESC reverses family name and revision",
            json!({ "sort": SORT_ORDERS[1] }),
            active_desc.clone(),
        ),
        (
            "familyPrefix names one family in full",
            json!({ "familyPrefix": "alpha" }),
            arns(&[("alpha", 1), ("alpha", 3)]),
        ),
        (
            "familyPrefix names the family whose name extends another's",
            json!({ "familyPrefix": "alphaextra" }),
            arns(&[("alphaextra", 1)]),
        ),
        (
            "a leading fragment is not a full family name",
            json!({ "familyPrefix": "alph" }),
            Vec::new(),
        ),
        (
            "familyPrefix names a family nothing was registered under",
            json!({ "familyPrefix": "nosuchfamily" }),
            Vec::new(),
        ),
        (
            "familyPrefix and status intersect",
            json!({ "familyPrefix": "alpha", "status": "INACTIVE" }),
            arns(&[("alpha", 2)]),
        ),
        (
            "familyPrefix and status no revision satisfies together",
            json!({ "familyPrefix": "beta", "status": "INACTIVE" }),
            Vec::new(),
        ),
        (
            "familyPrefix and sort DESC",
            json!({ "familyPrefix": "alpha", "sort": "DESC" }),
            arns(&[("alpha", 3), ("alpha", 1)]),
        ),
    ];

    for (case, request, expected) in cases {
        let listed = all_pages(&router, request.clone(), case).await;
        assert_eq!(
            listed, expected,
            "case {case:?}: expected {expected:?}, got {listed:?}"
        );
        // A reporting operation: reading changes nothing a later caller sees.
        let again = all_pages(&router, request, case).await;
        assert_eq!(
            again, expected,
            "case {case:?}: a second identical listing must report {expected:?}, got {again:?}"
        );
    }

    // ROUND TRIP: every ARN the listing reports resolves, through
    // `DescribeTaskDefinition`, to the revision it names.
    for arn in &active_asc {
        let case = arn.as_str();
        let described = ok(
            &router,
            "DescribeTaskDefinition",
            json!({ "taskDefinition": arn }),
            case,
        )
        .await;
        assert_eq!(
            &string(&described["taskDefinition"], "taskDefinitionArn", case),
            arn,
            "case {case:?}: DescribeTaskDefinition resolved a listed ARN to another revision"
        );
        assert_eq!(
            string(&described["taskDefinition"], "status", case),
            "ACTIVE",
            "case {case:?}: an ACTIVE listing may only report ACTIVE revisions"
        );
    }
}

#[tokio::test]
async fn list_task_definitions_pages_in_a_stable_order_at_max_results() {
    let router = service();
    let case = "setup";
    // Two families whose revisions interleave in the family-then-revision order, and
    // enough of them that the DEFAULT page size (100) still leaves one page.
    register_revisions(&router, "paged", 7, case).await;
    register_revisions(&router, "pagedtoo", 4, case).await;

    let mut expected: Vec<String> = (1..=7).map(|revision| definition_arn("paged", revision)).collect();
    expected.extend((1..=4).map(|revision| definition_arn("pagedtoo", revision)));

    for sort in SORT_ORDERS {
        let mut ordered = expected.clone();
        if sort == "DESC" {
            ordered.reverse();
        }
        // The rows differ along the page size: each published bound (1 and 100), sizes
        // that divide the set and sizes that do not, and the whole set in one page.
        for size in [1_i64, 2, 3, 4, 11, 12, 100] {
            let case = format!("sort {sort} at maxResults {size}");
            let case = case.as_str();
            let mut seen: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0_usize;
            loop {
                let mut request = json!({ "sort": sort, "maxResults": size });
                if let Some(token) = &token {
                    request["nextToken"] = json!(token);
                }
                let (arns, next) = page(&router, request, case).await;
                pages += 1;
                assert!(
                    arns.len() as i64 <= size,
                    "case {case:?}: a page may not exceed maxResults, got {} entries",
                    arns.len()
                );
                assert_eq!(
                    next.is_some(),
                    seen.len() + arns.len() < ordered.len(),
                    "case {case:?}: a nextToken must be present exactly while results remain, \
                     got {next:?} after {} of {} entries",
                    seen.len() + arns.len(),
                    ordered.len()
                );
                seen.extend(arns);
                match next {
                    None => break,
                    Some(next) => token = Some(next),
                }
                assert!(
                    pages < MAX_PAGES,
                    "case {case:?}: ListTaskDefinitions never ran out of pages"
                );
            }
            // The pages concatenate, in order, to the whole set exactly once: nothing is
            // dropped, nothing repeated, and the order does not depend on the page size.
            assert_eq!(
                seen, ordered,
                "case {case:?}: expected the pages to walk {ordered:?}, got {seen:?}"
            );
            let full_pages = ordered
                .len()
                .div_ceil(usize::try_from(size).expect("a positive size"));
            assert_eq!(
                pages,
                full_pages.max(1),
                "case {case:?}: expected {full_pages} page(s) for {} entries",
                ordered.len()
            );
        }
    }

    // "If this parameter isn't used, then ListTaskDefinitions returns up to 100 results
    // and a nextToken value if applicable": 11 revisions fit in one default-sized page.
    let case = "no maxResults";
    let (only, next) = page(&router, json!({}), case).await;
    assert_eq!(
        only, expected,
        "case {case:?}: a default-sized page holds the whole set, got {only:?}"
    );
    assert!(
        next.is_none(),
        "case {case:?}: expected no nextToken on the last page, got {next:?}"
    );

    // A revision registered AFTER a token was minted does not disturb the page the token
    // resumes: the token names the revision the next page starts at, not an index.
    let case = "a revision registered mid-walk";
    let (first, token) = page(&router, json!({ "maxResults": 2 }), case).await;
    assert_eq!(
        first,
        arns(&[("paged", 1), ("paged", 2)]),
        "case {case:?}: the first page is the first two revisions in order"
    );
    let token = token.unwrap_or_else(|| panic!("case {case:?}: expected a nextToken"));
    register_revisions(&router, "aaaa", 1, case).await;
    let (second, _) = page(
        &router,
        json!({ "maxResults": 2, "nextToken": token }),
        case,
    )
    .await;
    assert_eq!(
        second,
        arns(&[("paged", 3), ("paged", 4)]),
        "case {case:?}: the resumed page continues where the first stopped"
    );
}

#[tokio::test]
async fn list_task_definitions_refuses_an_out_of_range_page_size_and_an_unminted_token() {
    let router = service();
    let case = "setup";
    register_revisions(&router, "refusing", 1, case).await;

    // "This value can be between 1 and 100": each published bound is accepted and the
    // value just outside it is refused.
    for size in [1, 100] {
        let case = format!("maxResults {size} is in range");
        ok(
            &router,
            "ListTaskDefinitions",
            json!({ "maxResults": size }),
            case.as_str(),
        )
        .await;
    }
    for size in [0, 101, -1] {
        let case = format!("maxResults {size} is out of range");
        refused(
            &router,
            "ListTaskDefinitions",
            json!({ "maxResults": size }),
            "InvalidParameterException",
            case.as_str(),
        )
        .await;
    }

    // A `nextToken` this operation never minted names no page: an identifier that isn't
    // valid, which is what `ClientException` reports — with ONE fixed sentence naming the
    // member, whatever the unusable token was. The rows span the ways a token can fail to
    // name a stored revision: empty, not a reference at all, a base64-looking opaque
    // blob, a family with no revision, a revision number outside the minted range, and a
    // well-formed reference to a family nothing was registered under.
    for token in [
        "",
        "not a token",
        "c2ltdWxhdGVkLXRva2Vu==",
        "refusing",
        "refusing:0",
        "refusing:7",
        "nosuchfamily:1",
    ] {
        let case = format!("nextToken {token:?} was never minted");
        let case = case.as_str();
        let (code, message) = refusal(
            &router,
            "ListTaskDefinitions",
            json!({ "nextToken": token }),
            case,
        )
        .await;
        assert_eq!(
            code, "ClientException",
            "case {case:?}: expected a ClientException, got {code:?}"
        );
        assert_eq!(
            message, "Invalid nextToken.",
            "case {case:?}: expected the one sentence ECS answers for an unusable token, \
             got {message:?}"
        );
    }

    // A token this operation DID mint still resumes after the revision it names left the
    // listing: the revision is stored whatever its status.
    let case = "a token whose revision was deregistered mid-walk";
    register_revisions(&router, "resuming", 2, case).await;
    let (first, token) = page(
        &router,
        json!({ "familyPrefix": "resuming", "maxResults": 1 }),
        case,
    )
    .await;
    assert_eq!(
        first,
        vec![definition_arn("resuming", 1)],
        "case {case:?}: the first page is the family's first revision"
    );
    let token = token.unwrap_or_else(|| panic!("case {case:?}: expected a nextToken"));
    deregister(&router, "resuming", 2, case).await;
    let (second, next) = page(
        &router,
        json!({ "familyPrefix": "resuming", "maxResults": 1, "nextToken": token }),
        case,
    )
    .await;
    assert!(
        second.is_empty() && next.is_none(),
        "case {case:?}: the deregistered revision left the ACTIVE listing, got {second:?} \
         and {next:?}"
    );
}
