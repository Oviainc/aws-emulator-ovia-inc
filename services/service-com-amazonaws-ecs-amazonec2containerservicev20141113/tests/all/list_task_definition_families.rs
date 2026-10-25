// SPDX-License-Identifier: CC0-1.0

//! `ListTaskDefinitionFamilies`, driven end to end over the shared wire harness in
//! [`crate::ecs_wire`].
//!
//! The rule under test: `ListTaskDefinitionFamilies` answers with the NAMES of the
//! families the account's STORED task-definition revisions belong to — whatever those
//! revisions' status — keeping those whose name begins with `familyPrefix` (every family
//! when it names none) and those on the side of the `status` filter (families with an
//! `ACTIVE` revision for `ACTIVE`, families with none for `INACTIVE`, every family for
//! `ALL` or for no filter at all), ordered lexicographically and cut into pages of
//! `maxResults` names whose `nextToken` resumes the walk where the page stopped.
//!
//! The docs the rows are read off:
//!
//! * `ListTaskDefinitionFamilies`: "Returns a list of task definition families that are
//!   registered to your account. This list includes task definition families that no
//!   longer have any ACTIVE task definition revisions."
//! * `ListTaskDefinitionFamilies$familyPrefix`: "If you specify a familyPrefix, only task
//!   definition family names that begin with the familyPrefix string are returned."
//! * `ListTaskDefinitionFamilies$status`: "By default, both ACTIVE and INACTIVE task
//!   definition families are listed. If this parameter is set to ACTIVE, only task
//!   definition families that have an ACTIVE task definition revision are returned. If
//!   this parameter is set to INACTIVE, only task definition families that do not have any
//!   ACTIVE task definition revisions are returned."
//! * `ListTaskDefinitionFamilies$maxResults`: "This value can be between 1 and 100. If
//!   this parameter isn't used, then ListTaskDefinitionFamilies returns up to 100 results
//!   and a nextToken value if applicable."
//! * `ListTaskDefinitionFamilies$nextToken` (output): "This value is null when there are
//!   no more results to return."
//!
//! Every case goes through the same front door a client uses, so nothing reaches into the
//! state struct and a response that is not computed from persisted state cannot pass.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{list, ok, refusal, refused, register_definition, service, string};

/// Every value `TaskDefinitionFamilyStatus` takes, from the model's enum (smithy_query
/// query="shape" shape="TaskDefinitionFamilyStatus": ACTIVE, ALL, INACTIVE).
const FAMILY_STATUSES: [&str; 3] = ["ACTIVE", "ALL", "INACTIVE"];

/// How many pages a paging walk may take before the test calls it a loop. Generous: the
/// smallest page size under test is one name.
const MAX_PAGES: usize = 64;

/// ONE `ListTaskDefinitionFamilies` page: the family names it carries and the `nextToken`
/// it hands back.
async fn page(
    router: &MultiServiceRouter,
    request: Value,
    case: &str,
) -> (Vec<String>, Option<String>) {
    let body = ok(router, "ListTaskDefinitionFamilies", request, case).await;
    let families = list(&body, "families", case)
        .iter()
        .map(|value| {
            value
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a string family, got {value}"))
                .to_string()
        })
        .collect();
    let next_token = body["nextToken"].as_str().map(str::to_string);
    (families, next_token)
}

/// Every family name `request` reaches, in page order, walking `nextToken` to the end.
async fn all_pages(router: &MultiServiceRouter, request: Value, case: &str) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let mut this_page = request.clone();
        if let Some(token) = &token {
            this_page["nextToken"] = json!(token);
        }
        let (families, next) = page(router, this_page, case).await;
        seen.extend(families);
        match next {
            None => return seen,
            Some(next) => token = Some(next),
        }
    }
    panic!(
        "case {case:?}: ListTaskDefinitionFamilies never ran out of pages within {MAX_PAGES} pages"
    );
}

/// Register `count` revisions of `family` through the operation that creates them.
async fn register_revisions(router: &MultiServiceRouter, family: &str, count: u32, case: &str) {
    for _ in 0..count {
        register_definition(router, family, case).await;
    }
}

/// Deregister ONE revision, which is what takes it out of `ACTIVE` — and, when it was the
/// family's last `ACTIVE` revision, takes the FAMILY out of the `ACTIVE` listing.
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

/// The four families every case in the first test reads: one whose every revision has
/// been deregistered (so it has no `ACTIVE` revision left), one whose name EXTENDS that
/// family's name, one that keeps an `ACTIVE` revision beside a deregistered one, and one
/// that sorts after all three.
async fn stored_families(router: &MultiServiceRouter) {
    let case = "setup";
    register_revisions(router, "alpha", 3, case).await;
    register_revisions(router, "alphaextra", 1, case).await;
    register_revisions(router, "beta", 2, case).await;
    register_revisions(router, "gamma", 1, case).await;
    // `alpha` keeps no ACTIVE revision; `beta` keeps one.
    for revision in 1..=3 {
        deregister(router, "alpha", revision, case).await;
    }
    deregister(router, "beta", 1, case).await;
}

#[tokio::test]
async fn list_task_definition_families_reports_stored_families_narrowed_by_every_filter() {
    let router = service();
    stored_families(&router).await;

    let every_family = vec![
        "alpha".to_string(),
        "alphaextra".to_string(),
        "beta".to_string(),
        "gamma".to_string(),
    ];
    let with_active = vec![
        "alphaextra".to_string(),
        "beta".to_string(),
        "gamma".to_string(),
    ];
    let without_active = vec!["alpha".to_string()];

    // The rows differ along what the rule generalises over: the status filtered on (each
    // value the model's enum carries, plus the absent default), the prefix named (a whole
    // family name, a fragment shared by two families, the empty string, a fragment of one
    // name only, and a prefix nothing was registered under) and the two intersected.
    let cases: Vec<(&str, Value, Vec<String>)> = vec![
        (
            "no filter at all lists every stored family",
            json!({}),
            every_family.clone(),
        ),
        (
            "status ALL is the default made explicit",
            json!({ "status": FAMILY_STATUSES[1] }),
            every_family.clone(),
        ),
        (
            "status ACTIVE keeps the families with an ACTIVE revision",
            json!({ "status": FAMILY_STATUSES[0] }),
            with_active.clone(),
        ),
        (
            "status INACTIVE keeps the families with no ACTIVE revision",
            json!({ "status": FAMILY_STATUSES[2] }),
            without_active.clone(),
        ),
        (
            "familyPrefix is a leading fragment two families share",
            json!({ "familyPrefix": "alpha" }),
            vec!["alpha".to_string(), "alphaextra".to_string()],
        ),
        (
            "familyPrefix is one family's whole name",
            json!({ "familyPrefix": "alphaextra" }),
            vec!["alphaextra".to_string()],
        ),
        (
            "familyPrefix is a one-letter fragment of one name",
            json!({ "familyPrefix": "g" }),
            vec!["gamma".to_string()],
        ),
        (
            "the empty familyPrefix every name begins with",
            json!({ "familyPrefix": "" }),
            every_family.clone(),
        ),
        (
            "familyPrefix nothing was registered under",
            json!({ "familyPrefix": "nosuchfamily" }),
            Vec::new(),
        ),
        (
            "a prefix that is not a leading fragment of the name it resembles",
            json!({ "familyPrefix": "lpha" }),
            Vec::new(),
        ),
        (
            "familyPrefix and status ACTIVE intersect",
            json!({ "familyPrefix": "alpha", "status": "ACTIVE" }),
            vec!["alphaextra".to_string()],
        ),
        (
            "familyPrefix and status INACTIVE intersect",
            json!({ "familyPrefix": "alpha", "status": "INACTIVE" }),
            without_active.clone(),
        ),
        (
            "familyPrefix and status no family satisfies together",
            json!({ "familyPrefix": "beta", "status": "INACTIVE" }),
            Vec::new(),
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

    // ROUND TRIP: every family the ACTIVE listing reports resolves, through
    // `DescribeTaskDefinition` on the family name alone, to that family's latest ACTIVE
    // revision — and every family the INACTIVE listing reports resolves through
    // `ListTaskDefinitions` to revisions that are all INACTIVE.
    for family in &with_active {
        let case = family.as_str();
        let described = ok(
            &router,
            "DescribeTaskDefinition",
            json!({ "taskDefinition": family }),
            case,
        )
        .await;
        assert_eq!(
            &string(&described["taskDefinition"], "family", case),
            family,
            "case {case:?}: DescribeTaskDefinition resolved a listed family to another one"
        );
        assert_eq!(
            string(&described["taskDefinition"], "status", case),
            "ACTIVE",
            "case {case:?}: an ACTIVE family must have an ACTIVE revision"
        );
    }
    for family in &without_active {
        let case = family.as_str();
        let active = ok(
            &router,
            "ListTaskDefinitions",
            json!({ "familyPrefix": family, "status": "ACTIVE" }),
            case,
        )
        .await;
        assert!(
            list(&active, "taskDefinitionArns", case).is_empty(),
            "case {case:?}: an INACTIVE family may hold no ACTIVE revision, got {active}"
        );
        let inactive = ok(
            &router,
            "ListTaskDefinitions",
            json!({ "familyPrefix": family, "status": "INACTIVE" }),
            case,
        )
        .await;
        assert_eq!(
            list(&inactive, "taskDefinitionArns", case).len(),
            3,
            "case {case:?}: the family's three deregistered revisions stay stored, got \
             {inactive}"
        );
    }
}

#[tokio::test]
async fn list_task_definition_families_pages_in_a_stable_order_at_max_results() {
    let router = service();
    let case = "setup";
    // Families registered OUT of lexicographic order, with differing revision counts, so
    // the listing's order cannot come from insertion order or from revision count.
    let registered = ["pf", "pa", "pe", "pb", "pd", "pc"];
    for (index, family) in registered.iter().enumerate() {
        let revisions = u32::try_from(index % 3).expect("a small count") + 1;
        register_revisions(&router, family, revisions, case).await;
    }
    let mut ordered: Vec<String> = registered.iter().map(|name| name.to_string()).collect();
    ordered.sort();

    // The rows differ along the page size: the published lower bound (1), sizes that
    // divide the set and sizes that do not, and the published upper bound (100), which
    // holds the whole set in one page.
    for size in [1_i64, 2, 3, 4, 5, 6, 7, 100] {
        let case = format!("maxResults {size}");
        let case = case.as_str();
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0_usize;
        loop {
            let mut request = json!({ "maxResults": size });
            if let Some(token) = &token {
                request["nextToken"] = json!(token);
            }
            let (families, next) = page(&router, request, case).await;
            pages += 1;
            assert!(
                families.len() as i64 <= size,
                "case {case:?}: a page may not exceed maxResults, got {} entries",
                families.len()
            );
            assert_eq!(
                next.is_some(),
                seen.len() + families.len() < ordered.len(),
                "case {case:?}: a nextToken must be present exactly while results remain, \
                 got {next:?} after {} of {} names",
                seen.len() + families.len(),
                ordered.len()
            );
            seen.extend(families);
            match next {
                None => break,
                Some(next) => token = Some(next),
            }
            assert!(
                pages < MAX_PAGES,
                "case {case:?}: ListTaskDefinitionFamilies never ran out of pages"
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
            "case {case:?}: expected {full_pages} page(s) for {} names",
            ordered.len()
        );
    }

    // "If this parameter isn't used, then ListTaskDefinitionFamilies returns up to 100
    // results and a nextToken value if applicable": six families fit in one default page.
    let case = "no maxResults";
    let (only, next) = page(&router, json!({}), case).await;
    assert_eq!(
        only, ordered,
        "case {case:?}: a default-sized page holds the whole set, got {only:?}"
    );
    assert!(
        next.is_none(),
        "case {case:?}: expected no nextToken on the last page, got {next:?}"
    );

    // A family registered AFTER a token was minted does not disturb the page the token
    // resumes: the token names the family the next page starts at, not an index.
    let case = "a family registered mid-walk";
    let (first, token) = page(&router, json!({ "maxResults": 2 }), case).await;
    assert_eq!(
        first,
        vec!["pa".to_string(), "pb".to_string()],
        "case {case:?}: the first page is the first two families in order"
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
        vec!["pc".to_string(), "pd".to_string()],
        "case {case:?}: the resumed page continues where the first stopped"
    );
}

#[tokio::test]
async fn list_task_definition_families_refuses_an_out_of_range_page_size_and_an_unminted_token() {
    let router = service();
    let case = "setup";
    register_revisions(&router, "refusing", 1, case).await;

    // "This value can be between 1 and 100": each published bound is accepted and the
    // value just outside it is refused.
    for size in [1, 100] {
        let case = format!("maxResults {size} is in range");
        ok(
            &router,
            "ListTaskDefinitionFamilies",
            json!({ "maxResults": size }),
            case.as_str(),
        )
        .await;
    }
    for size in [0, 101, -1] {
        let case = format!("maxResults {size} is out of range");
        refused(
            &router,
            "ListTaskDefinitionFamilies",
            json!({ "maxResults": size }),
            "InvalidParameterException",
            case.as_str(),
        )
        .await;
    }

    // A `nextToken` this operation never minted names no page: an identifier that isn't
    // valid, which is what `ClientException` reports. The rows span the ways a token can
    // fail to name a stored family: empty, not a name at all, a base64-looking opaque
    // blob, a leading fragment of a stored family's name, a name that extends it, and a
    // revision reference rather than a family name.
    for token in [
        "",
        "not a token",
        "c2ltdWxhdGVkLXRva2Vu==",
        "refusin",
        "refusingmore",
        "refusing:1",
    ] {
        let case = format!("nextToken {token:?} was never minted");
        let case = case.as_str();
        let (code, message) = refusal(
            &router,
            "ListTaskDefinitionFamilies",
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

    // A token this operation DID mint still resumes after the family it names left the
    // listing: the family is stored whatever its revisions' status, and the walk carries
    // on with the first family that sorts after it.
    let case = "a token whose family left the ACTIVE listing mid-walk";
    register_revisions(&router, "resuminga", 1, case).await;
    register_revisions(&router, "resumingb", 1, case).await;
    register_revisions(&router, "resumingc", 1, case).await;
    let (first, token) = page(
        &router,
        json!({ "familyPrefix": "resuming", "status": "ACTIVE", "maxResults": 1 }),
        case,
    )
    .await;
    assert_eq!(
        first,
        vec!["resuminga".to_string()],
        "case {case:?}: the first page is the first family in order"
    );
    let token = token.unwrap_or_else(|| panic!("case {case:?}: expected a nextToken"));
    assert_eq!(
        token, "resumingb",
        "case {case:?}: the token names the family the next page starts at"
    );
    deregister(&router, "resumingb", 1, case).await;
    let resumed = all_pages(
        &router,
        json!({ "familyPrefix": "resuming", "status": "ACTIVE", "maxResults": 1,
                "nextToken": token }),
        case,
    )
    .await;
    assert_eq!(
        resumed,
        vec!["resumingc".to_string()],
        "case {case:?}: the resumed walk skips the family that left the listing and keeps \
         the one after it, got {resumed:?}"
    );
}
