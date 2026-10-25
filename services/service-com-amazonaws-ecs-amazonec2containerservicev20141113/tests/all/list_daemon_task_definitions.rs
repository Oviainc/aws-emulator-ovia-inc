// SPDX-License-Identifier: CC0-1.0

//! `ListDaemonTaskDefinitions`, driven end to end over the shared wire harness in
//! [`crate::ecs_wire`].
//!
//! The rule under test: the listing reports a summary of every daemon task-definition
//! revision STORED for the account whose status the request filters on (`ACTIVE` when it
//! names none, every status under `ALL`), whose family the request names exactly
//! (`family`) or as a leading fragment (`familyPrefix`), reduced to each family's highest
//! revision under `LAST_REGISTERED`, ordered by family name and then ascending revision
//! (reversed by `DESC`), and cut into pages of `maxResults` entries whose `nextToken`
//! resumes the walk where the page stopped.
//!
//! The docs the rows are read off:
//!
//! * `ListDaemonTaskDefinitions$family`: "The exact name of the daemon task definition
//!   family to filter results with."
//! * `ListDaemonTaskDefinitions$familyPrefix`: "Specifying a familyPrefix limits the
//!   listed daemon task definitions to daemon task definition families that start with
//!   the familyPrefix string."
//! * `ListDaemonTaskDefinitions$status`: "By default, only ACTIVE daemon task definitions
//!   are listed. If you set this parameter to DELETE_IN_PROGRESS, only daemon task
//!   definitions that are in the process of being deleted are listed. If you set this
//!   parameter to ALL, all daemon task definitions are listed regardless of status."
//! * `ListDaemonTaskDefinitions$revision`: "Specify LAST_REGISTERED to return only the
//!   last registered revision for each daemon task definition family."
//! * `ListDaemonTaskDefinitions$sort`: "By default (ASC), daemon task definitions are
//!   listed in ascending order by family name and revision number."
//! * `ListDaemonTaskDefinitions$maxResults`: "This value can be between 1 and 100. If
//!   this parameter isn't used, then ListDaemonTaskDefinitions returns up to 100 results
//!   and a nextToken value if applicable."
//! * `DaemonTaskDefinitionSummary`: "A summary of a daemon task definition" — its ARN,
//!   the instant and the principal that registered it, its status, and the instant a
//!   delete was requested for it.
//!
//! Every case goes through the same front door a client uses, so nothing reaches into the
//! state struct and a response that is not computed from persisted state cannot pass.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{list, ok, refusal, refused, service, string, ACCOUNT, SIGNED_REGION};

/// The filter catalogs this suite exercises, as this crate generated them from the model
/// (`smithy_query(query="shape", shape="DaemonTaskDefinitionStatusFilter,\
/// DaemonTaskDefinitionRevisionFilter,SortOrder")`), so every value under test is the
/// model's and not a copy of it.
use service_com_amazonaws_ecs_amazonec2containerservicev20141113::ops::types::{
    DAEMON_TASK_DEFINITION_REVISION_FILTER_VALUES, DAEMON_TASK_DEFINITION_STATUS_FILTER_VALUES,
    SORT_ORDER_VALUES,
};

/// The image every container here starts from; irrelevant to every rule, so one value.
const IMAGE: &str = "public.ecr.aws/aws-observability/aws-otel-collector:latest";

/// How many pages a paging walk may take before the test calls it a loop. Generous: the
/// smallest page size under test is one entry.
const MAX_PAGES: usize = 64;

/// One value of `DaemonTaskDefinitionStatusFilter`, looked up in the model's own catalog
/// so a value this suite names but the model does not carry fails here rather than
/// silently filtering nothing.
fn status_filter(value: &str) -> &'static str {
    in_catalog(
        value,
        DAEMON_TASK_DEFINITION_STATUS_FILTER_VALUES,
        "DaemonTaskDefinitionStatusFilter",
    )
}

/// One value of `DaemonTaskDefinitionRevisionFilter`, from the model's catalog.
fn revision_filter(value: &str) -> &'static str {
    in_catalog(
        value,
        DAEMON_TASK_DEFINITION_REVISION_FILTER_VALUES,
        "DaemonTaskDefinitionRevisionFilter",
    )
}

/// One value of `SortOrder`, from the model's catalog.
fn sort_order(value: &str) -> &'static str {
    in_catalog(value, SORT_ORDER_VALUES, "SortOrder")
}

/// `value` as the model's `catalog` carries it.
fn in_catalog(value: &str, catalog: &'static [&'static str], shape: &str) -> &'static str {
    catalog
        .iter()
        .copied()
        .find(|candidate| *candidate == value)
        .unwrap_or_else(|| panic!("{value:?} is not a {shape} value; the model lists {catalog:?}"))
}

/// The ARN a daemon revision of `family` carries, as
/// `arn:aws:ecs:<region>:<account>:daemon-task-definition/<family>:<revision>`.
fn daemon_arn(family: &str, revision: u32) -> String {
    format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:daemon-task-definition/{family}:{revision}")
}

/// The ARNs of `revisions`, in the order given.
fn arns(revisions: &[(&str, u32)]) -> Vec<String> {
    revisions
        .iter()
        .map(|(family, revision)| daemon_arn(family, *revision))
        .collect()
}

/// The principal every request to this emulator authenticates as: the one IAM user its
/// `sts:GetCallerIdentity` publishes, which is what "the principal that registered the
/// daemon task definition" names.
fn caller_identity() -> String {
    format!("arn:aws:iam::{ACCOUNT}:user/emulator")
}

/// Register `count` revisions of `family`, asserting each one takes the next revision
/// number — the family counter the listing's order is read off. Each revision carries a
/// container named after its number, so the revisions differ by CONTENT too.
async fn register_revisions(router: &MultiServiceRouter, family: &str, count: u32, case: &str) {
    for revision in 1..=count {
        let body = ok(
            router,
            "RegisterDaemonTaskDefinition",
            json!({
                "family": family,
                "containerDefinitions": [
                    { "name": format!("agent{revision}"), "image": IMAGE, "memory": 128 },
                ],
            }),
            case,
        )
        .await;
        assert_eq!(
            string(&body, "daemonTaskDefinitionArn", case),
            daemon_arn(family, revision),
            "case {case:?}: registration {revision} of {family:?} must take revision {revision}"
        );
    }
}

/// ONE `ListDaemonTaskDefinitions` page: the summaries it carries and the `nextToken` it
/// hands back.
async fn page(
    router: &MultiServiceRouter,
    request: Value,
    case: &str,
) -> (Vec<Value>, Option<String>) {
    let body = ok(router, "ListDaemonTaskDefinitions", request, case).await;
    assert!(
        body.get("daemonTaskDefinitions").is_some(),
        "case {case:?}: the summary list is always materialised, got {body}"
    );
    let summaries = list(&body, "daemonTaskDefinitions", case);
    let next_token = body["nextToken"].as_str().map(str::to_string);
    (summaries, next_token)
}

/// The ARNs a page of summaries names, in page order.
fn listed_arns(summaries: &[Value], case: &str) -> Vec<String> {
    summaries
        .iter()
        .map(|summary| string(summary, "arn", case))
        .collect()
}

/// Every summary `request` reaches, in page order, walking `nextToken` to the end.
async fn all_pages(router: &MultiServiceRouter, request: Value, case: &str) -> Vec<Value> {
    let mut seen: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let mut this_page = request.clone();
        if let Some(token) = &token {
            this_page["nextToken"] = json!(token);
        }
        let (summaries, next) = page(router, this_page, case).await;
        seen.extend(summaries);
        match next {
            None => return seen,
            Some(next) => token = Some(next),
        }
    }
    panic!("case {case:?}: the listing never ran out of pages within {MAX_PAGES} pages");
}

/// Every ARN `request` reaches, in page order.
async fn all_listed_arns(router: &MultiServiceRouter, request: Value, case: &str) -> Vec<String> {
    let summaries = all_pages(router, request, case).await;
    listed_arns(&summaries, case)
}

/// The three families every filter case below reads: one with several revisions, one
/// whose name EXTENDS that family's name (so a leading fragment that is not a whole
/// family name is visible), and one that sorts after both.
async fn stored_revisions(router: &MultiServiceRouter) {
    let case = "setup";
    register_revisions(router, "alpha", 3, case).await;
    register_revisions(router, "alphaextra", 1, case).await;
    register_revisions(router, "beta", 2, case).await;
}

#[tokio::test]
async fn the_listing_reports_the_stored_revisions_narrowed_by_every_filter() {
    let router = service();
    stored_revisions(&router).await;

    let every = arns(&[
        ("alpha", 1),
        ("alpha", 2),
        ("alpha", 3),
        ("alphaextra", 1),
        ("beta", 1),
        ("beta", 2),
    ]);
    let mut every_desc = every.clone();
    every_desc.reverse();
    let last_registered = arns(&[("alpha", 3), ("alphaextra", 1), ("beta", 2)]);
    let mut last_registered_desc = last_registered.clone();
    last_registered_desc.reverse();

    // The rows differ along what the rule generalises over: the status filtered on (every
    // value the model's enum carries), how the family is named (exactly, as a leading
    // fragment of two families, as a fragment of none), the revision filter, the sort
    // order, and the combinations of those.
    let cases: Vec<(&str, Value, Vec<String>)> = vec![
        (
            "no filter at all lists every stored revision in ASC order",
            json!({}),
            every.clone(),
        ),
        (
            "status ACTIVE is the default made explicit",
            json!({ "status": status_filter("ACTIVE") }),
            every.clone(),
        ),
        (
            "status ALL lists every revision regardless of status",
            json!({ "status": status_filter("ALL") }),
            every.clone(),
        ),
        (
            "status DELETE_IN_PROGRESS matches no revision while no delete was requested",
            json!({ "status": status_filter("DELETE_IN_PROGRESS") }),
            Vec::new(),
        ),
        (
            "sort ASC is the default made explicit",
            json!({ "sort": sort_order("ASC") }),
            every.clone(),
        ),
        (
            "sort DESC reverses family name and revision",
            json!({ "sort": sort_order("DESC") }),
            every_desc.clone(),
        ),
        (
            "family names one family exactly",
            json!({ "family": "alpha" }),
            arns(&[("alpha", 1), ("alpha", 2), ("alpha", 3)]),
        ),
        (
            "family names the family whose name extends another's",
            json!({ "family": "alphaextra" }),
            arns(&[("alphaextra", 1)]),
        ),
        (
            "a leading fragment is not an exact family name",
            json!({ "family": "alph" }),
            Vec::new(),
        ),
        (
            "family names a family nothing was registered under",
            json!({ "family": "nosuchfamily" }),
            Vec::new(),
        ),
        (
            "familyPrefix is a leading fragment, so it spans two families",
            json!({ "familyPrefix": "alph" }),
            arns(&[
                ("alpha", 1),
                ("alpha", 2),
                ("alpha", 3),
                ("alphaextra", 1),
            ]),
        ),
        (
            "familyPrefix that is a whole family name still spans what extends it",
            json!({ "familyPrefix": "alpha" }),
            arns(&[
                ("alpha", 1),
                ("alpha", 2),
                ("alpha", 3),
                ("alphaextra", 1),
            ]),
        ),
        (
            "familyPrefix no family starts with",
            json!({ "familyPrefix": "nosuchfamily" }),
            Vec::new(),
        ),
        (
            "familyPrefix and sort DESC",
            json!({ "familyPrefix": "beta", "sort": sort_order("DESC") }),
            arns(&[("beta", 2), ("beta", 1)]),
        ),
        (
            "revision LAST_REGISTERED keeps each family's highest revision",
            json!({ "revision": revision_filter("LAST_REGISTERED") }),
            last_registered.clone(),
        ),
        (
            "revision LAST_REGISTERED under sort DESC",
            json!({
                "revision": revision_filter("LAST_REGISTERED"),
                "sort": sort_order("DESC"),
            }),
            last_registered_desc.clone(),
        ),
        (
            "revision LAST_REGISTERED narrowed to one family",
            json!({
                "family": "alpha",
                "revision": revision_filter("LAST_REGISTERED"),
            }),
            arns(&[("alpha", 3)]),
        ),
        (
            "revision LAST_REGISTERED narrowed by a prefix spanning two families",
            json!({
                "familyPrefix": "alph",
                "revision": revision_filter("LAST_REGISTERED"),
            }),
            arns(&[("alpha", 3), ("alphaextra", 1)]),
        ),
        (
            "revision LAST_REGISTERED and a status nothing carries",
            json!({
                "revision": revision_filter("LAST_REGISTERED"),
                "status": status_filter("DELETE_IN_PROGRESS"),
            }),
            Vec::new(),
        ),
        (
            "family and familyPrefix that agree narrow to the exact family",
            json!({ "family": "alpha", "familyPrefix": "alph" }),
            arns(&[("alpha", 1), ("alpha", 2), ("alpha", 3)]),
        ),
        (
            "family and familyPrefix no family satisfies together",
            json!({ "family": "beta", "familyPrefix": "alph" }),
            Vec::new(),
        ),
    ];

    for (case, request, expected) in cases {
        let listed = all_listed_arns(&router, request.clone(), case).await;
        assert_eq!(
            listed, expected,
            "case {case:?}: expected {expected:?}, got {listed:?}"
        );
        // A reporting operation: reading changes nothing a later caller sees.
        let again = all_listed_arns(&router, request, case).await;
        assert_eq!(
            again, expected,
            "case {case:?}: a second identical listing must report {expected:?}, got {again:?}"
        );
    }
}

#[tokio::test]
async fn every_summary_reports_what_the_registration_stored() {
    let router = service();
    stored_revisions(&router).await;

    let case = "every stored revision";
    let summaries = all_pages(&router, json!({}), case).await;
    assert_eq!(
        summaries.len(),
        6,
        "case {case:?}: expected a summary per stored revision, got {summaries:?}"
    );

    for summary in &summaries {
        let arn = string(summary, "arn", case);
        let case = arn.as_str();
        assert_eq!(
            string(summary, "status", case),
            status_filter("ACTIVE"),
            "case {case:?}: a registered revision is ACTIVE, got {summary}"
        );
        assert_eq!(
            string(summary, "registeredBy", case),
            caller_identity(),
            "case {case:?}: expected the registering principal, got {summary}"
        );
        let registered_at = summary
            .get("registeredAt")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {case:?}: expected a registeredAt, got {summary}"));
        assert!(
            registered_at > 0.0,
            "case {case:?}: expected a registeredAt instant, got {registered_at}"
        );
        assert!(
            summary.get("deleteRequestedAt").is_none(),
            "case {case:?}: no delete was requested, so the summary carries no \
             deleteRequestedAt, got {summary}"
        );

        // ROUND TRIP: the summary reports the SAME stored revision
        // `DescribeDaemonTaskDefinition` describes, member for member.
        let described = ok(
            &router,
            "DescribeDaemonTaskDefinition",
            json!({ "daemonTaskDefinition": &arn }),
            case,
        )
        .await;
        let view = &described["daemonTaskDefinition"];
        assert_eq!(
            string(view, "daemonTaskDefinitionArn", case),
            arn,
            "case {case:?}: a listed ARN must describe the revision it names, got {view}"
        );
        assert_eq!(
            string(view, "status", case),
            string(summary, "status", case),
            "case {case:?}: the listing and the describe report one status, got {view}"
        );
        assert_eq!(
            string(view, "registeredBy", case),
            string(summary, "registeredBy", case),
            "case {case:?}: the listing and the describe report one principal, got {view}"
        );
        assert_eq!(
            view.get("registeredAt").and_then(Value::as_f64),
            Some(registered_at),
            "case {case:?}: the listing and the describe report one registeredAt, got {view}"
        );
        assert_eq!(
            view.get("deleteRequestedAt"),
            summary.get("deleteRequestedAt"),
            "case {case:?}: the listing and the describe agree on deleteRequestedAt, got {view}"
        );
    }
}

#[tokio::test]
async fn the_listing_pages_in_a_stable_order_at_every_page_size() {
    let router = service();
    let case = "setup";
    // Two families whose revisions interleave in the family-then-revision order, and
    // enough of them that the DEFAULT page size (100) still leaves one page.
    register_revisions(&router, "paged", 7, case).await;
    register_revisions(&router, "pagedtoo", 4, case).await;

    let mut expected: Vec<String> = (1..=7).map(|revision| daemon_arn("paged", revision)).collect();
    expected.extend((1..=4).map(|revision| daemon_arn("pagedtoo", revision)));

    for sort in SORT_ORDER_VALUES.iter().copied() {
        let mut ordered = expected.clone();
        if sort == sort_order("DESC") {
            ordered.reverse();
        }
        // The rows differ along the page size: the lower published bound (1), sizes that
        // divide the set and sizes that do not, and the upper bound (100), which holds
        // the whole set in one page.
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
                let (summaries, next) = page(&router, request, case).await;
                pages += 1;
                assert!(
                    summaries.len() as i64 <= size,
                    "case {case:?}: a page may not exceed maxResults, got {} entries",
                    summaries.len()
                );
                let page_arns = listed_arns(&summaries, case);
                assert_eq!(
                    next.is_some(),
                    seen.len() + page_arns.len() < ordered.len(),
                    "case {case:?}: a nextToken must be present exactly while results remain, \
                     got {next:?} after {} of {} entries",
                    seen.len() + page_arns.len(),
                    ordered.len()
                );
                seen.extend(page_arns);
                match next {
                    None => break,
                    Some(next) => token = Some(next),
                }
                assert!(
                    pages < MAX_PAGES,
                    "case {case:?}: the listing never ran out of pages"
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

    // "If this parameter isn't used, then ListDaemonTaskDefinitions returns up to 100
    // results and a nextToken value if applicable": 11 revisions fit in one page.
    let case = "no maxResults";
    let (only, next) = page(&router, json!({}), case).await;
    assert_eq!(
        listed_arns(&only, case),
        expected,
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
        listed_arns(&first, case),
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
        listed_arns(&second, case),
        arns(&[("paged", 3), ("paged", 4)]),
        "case {case:?}: the resumed page continues where the first stopped"
    );
}

#[tokio::test]
async fn the_listing_refuses_an_out_of_range_page_size_and_an_unminted_token() {
    let router = service();
    let case = "setup";
    register_revisions(&router, "refusing", 1, case).await;

    // "This value can be between 1 and 100": each published bound is accepted and the
    // value just outside it is refused.
    for size in [1, 100] {
        let case = format!("maxResults {size} is in range");
        ok(
            &router,
            "ListDaemonTaskDefinitions",
            json!({ "maxResults": size }),
            case.as_str(),
        )
        .await;
    }
    for size in [0, 101, -1] {
        let case = format!("maxResults {size} is out of range");
        refused(
            &router,
            "ListDaemonTaskDefinitions",
            json!({ "maxResults": size }),
            "InvalidParameterException",
            case.as_str(),
        )
        .await;
    }

    // A `nextToken` this operation never minted names no page: an identifier that isn't
    // valid, which is what `ClientException` reports — with ONE fixed sentence, whatever
    // the unusable token was. The rows span the ways a token can fail to name a stored
    // revision: empty, not a reference at all, an opaque base64-looking blob, a family
    // with no revision, a revision number outside the minted range, and a well-formed
    // reference to a family nothing was registered under.
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
            "ListDaemonTaskDefinitions",
            json!({ "nextToken": token }),
            case,
        )
        .await;
        assert_eq!(
            code, "ClientException",
            "case {case:?}: expected a ClientException, got {code:?} with {message:?}"
        );
        assert_eq!(
            message, "Invalid nextToken.",
            "case {case:?}: expected the one sentence ECS answers for an unusable token, \
             got {message:?}"
        );
    }
}
