// SPDX-License-Identifier: CC0-1.0

//! `ListCustomDetectionRuleAssociations`, end to end over the wire.
//!
//! Every request goes through [`smithy_server_runtime::MultiServiceRouter::dispatch`]
//! (the harness lives in the `detector_lifecycle` suite), so routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers are exercised
//! together.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! call:
//!
//! 1. The list reports the custom detection rule associations the account made
//!    through the Region serving the request — "Returns all custom detection rule
//!    associations for your account"
//!    (`smithy_query(query="doc",shape="ListCustomDetectionRuleAssociations")`) — in
//!    `RuleId` order whatever order they were created in, and every summary
//!    it reports resolves through `GetCustomDetectionRuleAssociation` to the same
//!    values.
//! 2. `RuleId` and `Mode` are EQUALITY filters on the members the summary reports
//!    ("You can filter by rule ID and mode"), each optional and each narrowing the
//!    set; a filter that selects nothing is an empty page, not an error — this
//!    operation models no `ResourceNotFoundException`
//!    (`smithy_query(operation="ListCustomDetectionRuleAssociations")`).
//! 3. It is PAGINATED (`smithy_query(operation="ListCustomDetectionRuleAssociations")`
//!    reports `inputToken`/`outputToken` `NextToken`, `items` `RuleAssociations`,
//!    `pageSize` `MaxResults`): walking the pages with any page size yields each
//!    association exactly once in the unpaged order, no page is longer than the page
//!    size asked for, and the last page carries no `NextToken`.
//! 4. It REPORTS: no number of lists or pages changes what a later caller sees.
//! 5. An association is REGIONAL: only the endpoint that made it reports it, and the
//!    `Arn` a summary carries names the Region serving the read.
//! 6. A `nextToken` the service never issued is refused with HTTP 400 under the
//!    service's `InvalidInputException` class, naming `nextToken` and without
//!    echoing the caller's value; a `maxResults` outside `range 1..100` and a `mode`
//!    outside the `AssociationMode` enum are refused by the modelled constraints
//!    with HTTP 400, and no refusal changes what the list reports.

use serde_json::{json, Value};

use crate::detector_lifecycle::{call, call_in, error_code, new_router, Router};

/// `ListCustomDetectionRuleAssociations` and `CreateCustomDetectionRuleAssociation`
/// are both bound at `/custom-detection-rule/association` (GET and POST)
/// (`smithy_query(operation="ListCustomDetectionRuleAssociations,CreateCustomDetectionRuleAssociation")`).
const ASSOCIATIONS_PATH: &str = "/custom-detection-rule/association";

/// `ListCustomDetectionRules` is `POST /custom-detection-rule/rule`, and
/// `GetCustomDetectionRuleAssociation` is
/// `GET /custom-detection-rule/rule/{RuleId}/association/{AssociationId}`
/// (`smithy_query(operation="ListCustomDetectionRules,GetCustomDetectionRuleAssociation")`).
const RULES_PATH: &str = "/custom-detection-rule/rule";

/// Every `AssociationMode` member (`smithy_query(shape="AssociationMode")`).
const MODES: [&str; 2] = ["DRY_RUN", "LIVE"];

/// The Region the harness signs for by default, and a second one, so the regional
/// scope of an association is asserted rather than assumed.
const HOME_REGION: &str = "us-east-1";
/// See [`HOME_REGION`].
const OTHER_REGION: &str = "eu-west-1";

/// `MaxResults` is `range 1..100`
/// (`smithy_query(operation="ListCustomDetectionRuleAssociations")`), so one page can
/// never carry more than 100 associations whatever a caller asks for.
const MAX_RESULTS_MAX: i64 = 100;

/// `AssociationId` is `len 1..64`, `pattern ^[a-zA-Z0-9_-]{1,64}$`
/// (`smithy_query(shape="AssociationSummary")`).
const ASSOCIATION_ID_MAX: usize = 64;

/// The error class every client rejection on this service carries in the body's
/// `__type` discriminator.
const INVALID_INPUT: &str = "InvalidInputException";

/// A well-formed `RuleId` (`^[a-z0-9]+(-[a-z0-9]+)*$`, `len 1..100`) the catalogue
/// does not publish, so no association can ever name it.
const ABSENT_RULE_ID: &str = "no-such-published-rule";

/// The ids GuardDuty publishes, read over the wire so the test cannot drift from the
/// catalogue.
async fn published_rule_ids(router: &Router) -> Vec<String> {
    let (status, body) = call(router, "POST", RULES_PATH, Some(json!({}))).await;
    assert_eq!(
        status, 200,
        "list-custom-detection-rules: expected 200, got {status} {body:?}"
    );
    let ids: Vec<String> = body["rules"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|rule| rule["ruleId"].as_str().map(str::to_string))
        .collect();
    assert!(
        ids.len() > 3,
        "list-custom-detection-rules: expected a catalogue of more than three rules, got {ids:?}"
    );
    ids
}

/// Enroll `region`'s account in `rule_id` in `mode`, returning the `AssociationId`.
async fn associate(router: &Router, region: &str, rule_id: &str, mode: &str) -> String {
    let (status, body) = call_in(
        router,
        region,
        "POST",
        ASSOCIATIONS_PATH,
        Some(json!({"ruleId": rule_id, "mode": mode})),
    )
    .await;
    assert_eq!(
        status, 200,
        "create-association {rule_id}/{mode} in {region}: expected 200, got {status} {body:?}"
    );
    body["ruleAssociation"]["associationId"]
        .as_str()
        .unwrap_or_else(|| panic!("create-association {rule_id}/{mode}: no associationId {body:?}"))
        .to_string()
}

/// One `ListCustomDetectionRuleAssociations` call through `region`'s endpoint, with
/// `query` appended to the URI (`""` for a request naming no parameter at all).
async fn list_in(router: &Router, region: &str, query: &str) -> (u16, Value) {
    call_in(
        router,
        region,
        "GET",
        &format!("{ASSOCIATIONS_PATH}{query}"),
        None,
    )
    .await
}

/// The `RuleAssociations` and `NextToken` of one page, failing the test if the list
/// did not succeed.
async fn page(
    router: &Router,
    region: &str,
    query: &str,
    case: &str,
) -> (Vec<Value>, Option<String>) {
    let (status, response) = list_in(router, region, query).await;
    assert_eq!(
        status, 200,
        "case {case:?}: list-custom-detection-rule-associations expected 200, got {status} {response:?}"
    );
    let found = response["ruleAssociations"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: ruleAssociations is @required, got {response:?}")
        });
    let token = response["nextToken"].as_str().map(str::to_string);
    (found, token)
}

/// Every association `region`'s endpoint reports for `query`, in one page large
/// enough to hold the whole set.
async fn all_in(router: &Router, region: &str, filter: &str, case: &str) -> Vec<Value> {
    let query = if filter.is_empty() {
        format!("?maxResults={MAX_RESULTS_MAX}")
    } else {
        format!("?maxResults={MAX_RESULTS_MAX}&{filter}")
    };
    let (found, token) = page(router, region, &query, case).await;
    assert!(
        token.is_none(),
        "case {case:?}: a page of {MAX_RESULTS_MAX} must exhaust the set, got a nextToken {token:?}"
    );
    found
}

/// The `(ruleId, mode)` pairs a page of summaries names, in page order.
fn pairs_of(summaries: &[Value]) -> Vec<(String, String)> {
    summaries
        .iter()
        .map(|summary| {
            (
                text(summary, "ruleId").to_string(),
                text(summary, "mode").to_string(),
            )
        })
        .collect()
}

/// The string member `member` of `summary`, failing the test when it is absent: every
/// member asserted through this one is `@required` on `AssociationSummary`.
fn text<'a>(summary: &'a Value, member: &str) -> &'a str {
    summary[member]
        .as_str()
        .unwrap_or_else(|| panic!("{member} is required on AssociationSummary, got {summary:?}"))
}

/// Walk every page of `filter` with `page_size` (`None` asks for the service's own
/// page size), returning the summaries in page order and each page's length.
async fn walk(
    router: &Router,
    region: &str,
    filter: &str,
    page_size: Option<i64>,
    case: &str,
) -> (Vec<Value>, Vec<usize>) {
    let mut found = Vec::new();
    let mut lengths = Vec::new();
    let mut token: Option<String> = None;
    // Bounded so a token the service never advances cannot loop forever.
    for _ in 0..512 {
        let mut parameters: Vec<String> = Vec::new();
        if !filter.is_empty() {
            parameters.push(filter.to_string());
        }
        if let Some(size) = page_size {
            parameters.push(format!("maxResults={size}"));
        }
        if let Some(token) = token.as_deref() {
            parameters.push(format!("nextToken={token}"));
        }
        let query = if parameters.is_empty() {
            String::new()
        } else {
            format!("?{}", parameters.join("&"))
        };
        let (this_page, next) = page(router, region, &query, case).await;
        if let Some(size) = page_size {
            assert!(
                this_page.len() as i64 <= size,
                "case {case:?}: a page must not exceed maxResults={size}, got {}",
                this_page.len()
            );
        }
        assert!(
            this_page.len() as i64 <= MAX_RESULTS_MAX,
            "case {case:?}: a page must carry at most {MAX_RESULTS_MAX} associations, got {}",
            this_page.len()
        );
        lengths.push(this_page.len());
        found.extend(this_page);
        match next {
            Some(next) => token = Some(next),
            None => return (found, lengths),
        }
    }
    panic!("case {case:?}: the list never ran out of pages");
}

/// The `(ruleId, mode)` pairs this suite enrolls the account in: the first eight
/// published rules, one association each because a rule carries at most one, in
/// alternating `AssociationMode`s so the set spans both modes over several rules.
///
/// Returned in the order the creates are MADE, which is deliberately the reverse of
/// the order the list must report, so an assertion on order cannot pass by accident.
fn plan(ids: &[String]) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = Vec::new();
    for (index, rule_id) in ids.iter().take(8).enumerate() {
        pairs.push((rule_id.clone(), MODES[index % MODES.len()].to_string()));
    }
    pairs.reverse();
    pairs
}

/// The order the list must report `pairs` in: `(RuleId, Mode)` ascending, the total
/// order a page token is an offset into.
fn reported_order(pairs: &[(String, String)]) -> Vec<(String, String)> {
    let mut ordered = pairs.to_vec();
    ordered.sort();
    ordered
}

/// Create every pair of [`plan`] through `region`'s endpoint, returning the pairs in
/// the order the list must report them.
async fn populate(router: &Router, region: &str, ids: &[String]) -> Vec<(String, String)> {
    let pairs = plan(ids);
    for (rule_id, mode) in &pairs {
        associate(router, region, rule_id, mode).await;
    }
    reported_order(&pairs)
}

// ---------------------------------------------------------------------------
// Rule 1: the list reports the Region's associations, and each one resolves.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_list_reports_every_association_the_region_holds_and_each_resolves() {
    let router = new_router();
    let ids = published_rule_ids(&router).await;

    // An account that has associated nothing has an empty list — `RuleAssociations`
    // is `@required`, so the member is there and carries no entry.
    let empty = all_in(&router, HOME_REGION, "", "before any association").await;
    assert!(
        empty.is_empty(),
        "an account with no association must report an empty list, got {empty:?}"
    );

    let expected = populate(&router, HOME_REGION, &ids).await;
    let summaries = all_in(&router, HOME_REGION, "", "every association").await;
    assert_eq!(
        pairs_of(&summaries),
        expected,
        "the list must report every association in (ruleId, mode) order, got {:?}",
        pairs_of(&summaries)
    );

    let mut seen_ids: Vec<String> = Vec::new();
    for summary in &summaries {
        let rule_id = text(summary, "ruleId");
        let mode = text(summary, "mode");
        let case = format!("{rule_id}/{mode}");
        let association_id = text(summary, "associationId");

        assert!(
            !association_id.is_empty()
                && association_id.len() <= ASSOCIATION_ID_MAX
                && association_id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
            "case {case:?}: associationId must satisfy ^[a-zA-Z0-9_-]{{1,64}}$, got {association_id:?}"
        );
        assert!(
            !seen_ids.contains(&association_id.to_string()),
            "case {case:?}: associationId {association_id:?} is not unique, already saw {seen_ids:?}"
        );
        seen_ids.push(association_id.to_string());
        assert!(
            ids.contains(&rule_id.to_string()),
            "case {case:?}: the list must only name published rules, got {rule_id:?}"
        );
        assert!(
            MODES.contains(&mode),
            "case {case:?}: mode must be an AssociationMode member, got {mode:?}"
        );
        // `ExpiresAt` is the shape's only optional member: a DRY_RUN association
        // stops evaluating a fixed interval after it is made and reports that
        // instant, and a LIVE one runs until it is deleted, so it reports none.
        let expires_at = summary.get("expiresAt").and_then(Value::as_f64);
        if mode == "DRY_RUN" {
            let created_at = summary["createdAt"].as_f64().unwrap_or_default();
            assert!(
                expires_at.is_some_and(|expires| expires > created_at),
                "case {case:?}: a DRY_RUN association must expire after it was created, got {summary:?}"
            );
        } else {
            assert!(
                expires_at.is_none(),
                "case {case:?}: a LIVE association must report no expiresAt, got {summary:?}"
            );
        }

        // The summary resolves through the detail read to the SAME association:
        // `AssociationSummary` is `AssociationDetail` without `AccountId`
        // (`smithy_query(shape="AssociationSummary,AssociationDetail")`), so every
        // member the summary carries must agree with the detail's.
        let (status, detail) = call(
            &router,
            "GET",
            &format!("{RULES_PATH}/{rule_id}/association/{association_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: get-association expected 200, got {status} {detail:?}"
        );
        let association = &detail["ruleAssociation"];
        for member in [
            "associationId",
            "arn",
            "ruleId",
            "mode",
            "createdAt",
            "updatedAt",
            "expiresAt",
        ] {
            assert_eq!(
                summary.get(member),
                association.get(member),
                "case {case:?}: the list and the read must agree on {member}, got {:?} and {:?}",
                summary.get(member),
                association.get(member)
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 2: ruleId and mode are equality filters, and select nothing as an empty page.
// ---------------------------------------------------------------------------

/// Which `(ruleId, mode)` pairs one filter row must select.
type Selects = Box<dyn Fn(&(String, String)) -> bool>;

/// One filter row: the query the list is called with, and the pairs it must report.
type FilterCase = (String, Selects);

#[tokio::test]
async fn rule_id_and_mode_filter_by_equality() {
    let router = new_router();
    let ids = published_rule_ids(&router).await;
    let expected = populate(&router, HOME_REGION, &ids).await;

    // Each row: the query filter, and the predicate the reported set must satisfy.
    // The rows span every form the two filters take — neither, each alone, both
    // together — over several rules and both modes.
    let mut rows: Vec<FilterCase> = vec![(
        String::new(),
        Box::new(|_: &(String, String)| true) as Selects,
    )];
    for rule_id in ids.iter().take(5) {
        let wanted = rule_id.clone();
        rows.push((
            format!("ruleId={rule_id}"),
            Box::new(move |pair: &(String, String)| pair.0 == wanted),
        ));
        for mode in MODES {
            let wanted = rule_id.clone();
            rows.push((
                format!("ruleId={rule_id}&mode={mode}"),
                Box::new(move |pair: &(String, String)| pair.0 == wanted && pair.1 == mode),
            ));
        }
    }
    for mode in MODES {
        rows.push((
            format!("mode={mode}"),
            Box::new(move |pair: &(String, String)| pair.1 == mode),
        ));
    }
    // A well-formed `RuleId` the catalogue does not publish, and one it publishes but
    // that nothing associated: both select nothing.
    rows.push((
        format!("ruleId={ABSENT_RULE_ID}"),
        Box::new(|_: &(String, String)| false),
    ));
    let unassociated = ids
        .iter()
        .find(|rule_id| !expected.iter().any(|(id, _)| id == *rule_id))
        .expect("the catalogue publishes a rule this suite does not associate")
        .clone();
    rows.push((
        format!("ruleId={unassociated}"),
        Box::new(|_: &(String, String)| false),
    ));

    for (filter, predicate) in rows {
        let case = format!("filter {filter:?}");
        let wanted: Vec<(String, String)> =
            expected.iter().filter(|pair| predicate(pair)).cloned().collect();
        let summaries = all_in(&router, HOME_REGION, &filter, &case).await;
        assert_eq!(
            pairs_of(&summaries),
            wanted,
            "case {case:?}: expected {wanted:?}, got {:?}",
            pairs_of(&summaries)
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: paging with any page size yields each association exactly once.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn paging_yields_each_association_exactly_once_in_order() {
    let router = new_router();
    let ids = published_rule_ids(&router).await;
    let expected = populate(&router, HOME_REGION, &ids).await;
    let count = expected.len();

    // Each row: a page size spanning one entry per page, a size that does not divide
    // the set, a size equal to it, one larger, the published maximum, and no size at
    // all; crossed with the unfiltered set and a filtered one.
    let sizes: Vec<Option<i64>> = vec![
        Some(1),
        Some(2),
        Some(3),
        Some(count as i64 - 1),
        Some(count as i64),
        Some(count as i64 + 1),
        Some(MAX_RESULTS_MAX),
        None,
    ];
    let filters = [
        String::new(),
        format!("mode={}", MODES[1]),
        format!("ruleId={}", ids[0]),
    ];

    for filter in &filters {
        let whole: Vec<(String, String)> = pairs_of(&all_in(
            &router,
            HOME_REGION,
            filter,
            &format!("unpaged {filter:?}"),
        )
        .await);
        for size in &sizes {
            let case = format!("filter {filter:?} paged by {size:?}");
            let (found, lengths) = walk(&router, HOME_REGION, filter, *size, &case).await;
            assert_eq!(
                pairs_of(&found),
                whole,
                "case {case:?}: paging must yield the unpaged order exactly once each, got {:?}",
                pairs_of(&found)
            );
            if let Some(size) = size {
                let full_pages = whole.len() / (*size as usize).max(1);
                for (index, length) in lengths.iter().enumerate().take(full_pages) {
                    assert_eq!(
                        *length, *size as usize,
                        "case {case:?}: page {index} before the last must be full, got {length}"
                    );
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 4: the list REPORTS; it changes nothing a later caller sees.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn listing_changes_nothing_a_later_caller_sees() {
    let router = new_router();
    let ids = published_rule_ids(&router).await;
    populate(&router, HOME_REGION, &ids).await;

    let first = all_in(&router, HOME_REGION, "", "the first list").await;

    // Lists, filtered lists and page walks, none of which may move anything.
    for round in 0..3 {
        let case = format!("round {round}");
        all_in(&router, HOME_REGION, "", &case).await;
        all_in(&router, HOME_REGION, &format!("mode={}", MODES[0]), &case).await;
        walk(&router, HOME_REGION, "", Some(1), &case).await;
    }

    let again = all_in(&router, HOME_REGION, "", "the list after reading").await;
    assert_eq!(
        again, first,
        "listing must not change what a later caller sees, got {again:?} after {first:?}"
    );

    // Including the timestamps: an `UpdatedAt` that moved on a read would make a
    // report a mutation.
    for summary in &first {
        assert_eq!(
            summary["createdAt"], summary["updatedAt"],
            "nothing has changed this association, so updatedAt must equal createdAt, got {summary:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 5: an association is regional, and the ARN names the Region read.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn only_the_region_that_made_an_association_reports_it() {
    let router = new_router();
    let ids = published_rule_ids(&router).await;

    // One Region takes the first two rules, the other the third: a rule carries at
    // most one association, so the two sets are disjoint in both rule and size.
    let home: Vec<(String, String)> = ids
        .iter()
        .take(2)
        .enumerate()
        .map(|(index, rule_id)| (rule_id.clone(), MODES[index % MODES.len()].to_string()))
        .collect();
    let other: Vec<(String, String)> = vec![(ids[2].clone(), MODES[0].to_string())];
    for (rule_id, mode) in home.iter().chain(other.iter()) {
        let region = if home.contains(&(rule_id.clone(), mode.clone())) {
            HOME_REGION
        } else {
            OTHER_REGION
        };
        associate(&router, region, rule_id, mode).await;
    }

    for (region, expected) in [(HOME_REGION, &home), (OTHER_REGION, &other)] {
        let case = format!("the list in {region}");
        let summaries = all_in(&router, region, "", &case).await;
        assert_eq!(
            pairs_of(&summaries),
            reported_order(expected),
            "case {case:?}: an endpoint reports only its own Region's associations, got {:?}",
            pairs_of(&summaries)
        );
        for summary in &summaries {
            let arn = text(summary, "arn");
            assert!(
                arn.starts_with("arn:") && arn.contains(&format!(":{region}:")),
                "case {case:?}: the arn must name the Region serving the read, got {arn:?}"
            );
            assert!(
                arn.ends_with(&format!(
                    "{}/association/{}",
                    text(summary, "ruleId"),
                    text(summary, "associationId")
                )),
                "case {case:?}: the arn must name the association's own ids, got {arn:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 6: refusals — classified, without echoing the caller's value.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_unissued_token_or_an_unusable_parameter_is_refused() {
    let router = new_router();
    let ids = published_rule_ids(&router).await;
    let expected = populate(&router, HOME_REGION, &ids).await;
    let count = expected.len() as i64;

    // Each row: the case, the query, and the member the refusal must name (empty
    // where the modelled constraint refuses it at the request boundary, which words
    // the refusal itself).
    let rows: Vec<(String, String, &str)> = vec![
        (
            "a nextToken that is not a token this service issues".to_string(),
            "?nextToken=not-a-token".to_string(),
            "nextToken",
        ),
        (
            "a nextToken pointing far past the end of the set".to_string(),
            "?nextToken=9999".to_string(),
            "nextToken",
        ),
        (
            "a nextToken pointing at the entry just past the last one".to_string(),
            format!("?nextToken={count}"),
            "nextToken",
        ),
        (
            "a nextToken pointing past the end of a FILTERED list".to_string(),
            format!("?mode={}&nextToken={count}", MODES[0]),
            "nextToken",
        ),
        (
            "a maxResults of 0: one below the published minimum of 1".to_string(),
            "?maxResults=0".to_string(),
            "",
        ),
        (
            "a maxResults of 101: one above the published maximum of 100".to_string(),
            format!("?maxResults={}", MAX_RESULTS_MAX + 1),
            "",
        ),
        (
            "a negative maxResults".to_string(),
            "?maxResults=-5".to_string(),
            "",
        ),
        (
            "a mode outside the AssociationMode enum".to_string(),
            "?mode=BOGUS".to_string(),
            "",
        ),
        (
            "a mode that is a member in the wrong case".to_string(),
            "?mode=live".to_string(),
            "",
        ),
        (
            "a ruleId that cannot be a RuleId at all".to_string(),
            "?ruleId=Not_A_Rule".to_string(),
            "",
        ),
    ];

    for (case, query, member) in rows {
        let (status, response) = list_in(&router, HOME_REGION, &query).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {response:?}"
        );
        let message = response["message"].as_str().unwrap_or_default();
        if !member.is_empty() {
            let class = error_code(&response);
            assert_eq!(
                class, INVALID_INPUT,
                "case {case:?}: expected the {INVALID_INPUT} class, got {class:?} {response:?}"
            );
            assert!(
                message.contains(member),
                "case {case:?}: the refusal must name {member}, got {message:?}"
            );
            for echoed in ["not-a-token", "9999", &count.to_string()] {
                assert!(
                    !message.contains(echoed),
                    "case {case:?}: the refusal must not echo {echoed:?}, got {message:?}"
                );
            }
        }
        // The refusal leaves the set exactly as it was.
        let after = all_in(&router, HOME_REGION, "", &case).await;
        assert_eq!(
            pairs_of(&after),
            expected,
            "case {case:?}: a refusal must not change what the list reports"
        );
    }

    // The token the service DOES issue is accepted on the same list, so the refusals
    // above are about tokens it never handed out and not about paging itself.
    let (first, token) = page(&router, HOME_REGION, "?maxResults=1", "the first page").await;
    let token = token.expect("the first page of a multi-association set carries a nextToken");
    let (second, _) = page(
        &router,
        HOME_REGION,
        &format!("?maxResults=1&nextToken={token}"),
        "the issued token",
    )
    .await;
    assert_eq!(
        pairs_of(&second),
        expected[1..2].to_vec(),
        "case \"the issued token\": expected the second association, got {:?} after {:?}",
        pairs_of(&second),
        pairs_of(&first)
    );
}
