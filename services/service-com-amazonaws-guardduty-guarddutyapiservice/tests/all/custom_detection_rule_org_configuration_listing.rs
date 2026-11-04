// SPDX-License-Identifier: CC0-1.0

//! `ListCustomDetectionRuleOrgConfigurations`, end to end over the wire.
//!
//! Every request goes through [`smithy_server_runtime::MultiServiceRouter::dispatch`]
//! (the harness lives in the `detector_lifecycle` suite), so routing, restJson1
//! (de)serialization of the `httpQuery` inputs, modelled-constraint validation and the
//! handler are exercised together.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! call:
//!
//! 1. The listing reports exactly the organization configurations
//!    `CreateCustomDetectionRuleOrgConfiguration` recorded through the Region serving
//!    the read — "Returns all organization-level configurations for custom detection
//!    rules" (`smithy_query(query="doc",shape="ListCustomDetectionRuleOrgConfigurations")`)
//!    — one summary per `(RuleId, Mode)` pair, in `(RuleId, Mode)` order whatever order
//!    the creates arrived in, and every member of each summary agrees with what
//!    `GetCustomDetectionRuleOrgConfiguration` reports for that pair.
//! 2. "You can filter the results by status": the `status` query parameter selects on
//!    the `Status` the summary REPORTS, which is derived from the clock — so the same
//!    configuration is selected by `PROCESSING` while the roll-out is still owed and by
//!    `ACTIVE` once the clock owed it the move (`state::ORG_CONFIGURATION_TRANSITION_MILLIS`),
//!    and a `DetectionRuleConfigurationStatus` member nothing is in selects nothing.
//! 3. It is PAGINATED (`smithy_query(operation="ListCustomDetectionRuleOrgConfigurations")`
//!    reports `inputToken`/`outputToken` `NextToken`, `items` `Configurations`,
//!    `pageSize` `MaxResults`): walking the pages with any page size yields each
//!    configuration exactly once in the unpaged order, no page is longer than the page
//!    size asked for, and the page that completes the list carries no `NextToken`.
//! 4. It REPORTS: no number of lists or pages changes what a later caller sees.
//! 5. A configuration belongs to the Region whose endpoint created it: another Region's
//!    endpoint lists its own and never that one.
//! 6. A `nextToken` the service never issued is refused with HTTP 400 under this
//!    service's rejection class, naming `nextToken` and echoing nothing; a `maxResults`
//!    outside `range 1..100` and a `status` outside
//!    `DetectionRuleConfigurationStatus` are refused with HTTP 400 by the modelled
//!    constraint, and no refusal changes the list.
//!
//! Every org-configuration request below is made by the organization's delegated
//! administrator, which the operation is available only to
//! (`crate::detector_lifecycle::as_delegated_admin`); the refusal of a caller without
//! that standing is the subject of the `org_configuration_authorization` suite.

use serde_json::{json, Value};

use service_com_amazonaws_guardduty_guarddutyapiservice::state::ORG_CONFIGURATION_TRANSITION_MILLIS;

use crate::client_rejections::page_token_refusal;
use crate::detector_lifecycle::{as_delegated_admin, call, call_in, error_code, new_router, Router};

/// `ListCustomDetectionRuleOrgConfigurations` is
/// `GET /custom-detection-rule/org-configuration` and the create that feeds it is
/// `POST` on the same path
/// (`smithy_query(operation="ListCustomDetectionRuleOrgConfigurations,CreateCustomDetectionRuleOrgConfiguration")`).
const ORG_CONFIGURATION_PATH: &str = "/custom-detection-rule/org-configuration";

/// `ListCustomDetectionRules` is `POST /custom-detection-rule/rule`
/// (`smithy_query(operation="ListCustomDetectionRules")`) — the catalogue a
/// configuration can name a rule out of.
const RULES_PATH: &str = "/custom-detection-rule/rule";

/// Every `DetectionRuleConfigurationStatus` member — the values the `status` filter
/// may name (`smithy_query(shape="DetectionRuleConfigurationStatus")`).
const STATUSES: [&str; 3] = ["ACTIVE", "PROCESSING", "FAILED"];

/// Every `AssociationMode` member (`smithy_query(shape="AssociationMode")`).
const MODES: [&str; 2] = ["LIVE", "DRY_RUN"];

/// `MaxResults` targets `DetectionRuleMaxResults`, `range 1..100`
/// (`smithy_query(operation="ListCustomDetectionRuleOrgConfigurations")`), so a page
/// can never carry more than 100 summaries whatever a caller asks for.
const MAX_RESULTS_MAX: i64 = 100;

/// The error class every client rejection this service's handlers raise carries in the
/// body's `__type` discriminator.
const INVALID_INPUT: &str = "InvalidInputException";

/// Long enough that the roll-out the create left owed is due: the span is
/// `ORG_CONFIGURATION_TRANSITION_MILLIS` and the wait clears it with room to spare.
fn past_the_transition() -> std::time::Duration {
    std::time::Duration::from_millis(ORG_CONFIGURATION_TRANSITION_MILLIS as u64 + 250)
}

/// The listing path carrying `params` as its query string.
fn listing_uri(params: &[(&str, String)]) -> String {
    if params.is_empty() {
        return ORG_CONFIGURATION_PATH.to_string();
    }
    let query = params
        .iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<String>>()
        .join("&");
    format!("{ORG_CONFIGURATION_PATH}?{query}")
}

/// One listing call through `region`'s endpoint.
async fn list_in(router: &Router, region: &str, params: &[(&str, String)]) -> (u16, Value) {
    as_delegated_admin(router, region).await;
    call_in(router, region, "GET", &listing_uri(params), None).await
}

/// One listing call through the default Region's endpoint.
async fn list(router: &Router, params: &[(&str, String)]) -> (u16, Value) {
    list_in(router, DEFAULT_REGION, params).await
}

/// The Region the harness signs for unless a case names another.
const DEFAULT_REGION: &str = "us-east-1";

/// The `configurations` of one page through `region`, with its `nextToken`, failing the
/// test if the listing did not succeed.
async fn page_in(
    router: &Router,
    region: &str,
    params: &[(&str, String)],
    case: &str,
) -> (Vec<Value>, Option<String>) {
    let (status, response) = list_in(router, region, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: list-custom-detection-rule-org-configurations expected 200, got {status} {response:?}"
    );
    let configurations = response["configurations"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a configurations array, got {response:?}")
        })
        .clone();
    let token = response["nextToken"].as_str().map(str::to_string);
    (configurations, token)
}

/// [`page_in`] through the default Region.
async fn page(router: &Router, params: &[(&str, String)], case: &str) -> (Vec<Value>, Option<String>) {
    page_in(router, DEFAULT_REGION, params, case).await
}

/// Every configuration `region`'s endpoint holds, in one page.
async fn all_in(router: &Router, region: &str, case: &str) -> Vec<Value> {
    let (configurations, token) =
        page_in(router, region, &[("maxResults", MAX_RESULTS_MAX.to_string())], case).await;
    assert!(
        token.is_none(),
        "case {case:?}: at most one configuration per published rule per mode fits in a page of {MAX_RESULTS_MAX}, got a nextToken"
    );
    configurations
}

/// The `(ruleId, mode)` pairs a page names, in the order it named them.
fn pairs_of(configurations: &[Value]) -> Vec<(String, String)> {
    configurations
        .iter()
        .map(|configuration| (text(configuration, "ruleId"), text(configuration, "mode")))
        .collect()
}

/// One required text member of a summary.
fn text(configuration: &Value, member: &str) -> String {
    configuration[member]
        .as_str()
        .unwrap_or_else(|| panic!("expected a {member} on {configuration:?}"))
        .to_string()
}

/// The ids of the rules GuardDuty publishes, read over the wire so the test cannot
/// drift from the catalogue.
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
        ids.len() > 2,
        "list-custom-detection-rules: expected a catalogue of more than two rules, got {ids:?}"
    );
    ids
}

/// Record one organization configuration through `region`'s endpoint, failing the test
/// if the create did not succeed.
async fn create_in(router: &Router, region: &str, body: Value) {
    as_delegated_admin(router, region).await;
    let (status, created) = call_in(
        router,
        region,
        "POST",
        ORG_CONFIGURATION_PATH,
        Some(body.clone()),
    )
    .await;
    assert_eq!(
        status, 200,
        "create-org-configuration {body:?} in {region}: expected 200, got {status} {created:?}"
    );
}

/// `GetCustomDetectionRuleOrgConfiguration` is
/// `GET /custom-detection-rule/org-configuration/{RuleId}?mode=`
/// (`smithy_query(operation="GetCustomDetectionRuleOrgConfiguration")`).
async fn get_in(router: &Router, region: &str, rule_id: &str, mode: &str) -> (u16, Value) {
    as_delegated_admin(router, region).await;
    call_in(
        router,
        region,
        "GET",
        &format!("{ORG_CONFIGURATION_PATH}/{rule_id}?mode={mode}"),
        None,
    )
    .await
}

/// One row of the configuration set the suites below build: which published rule (by
/// catalogue index), in which mode, and the account scope the create carries.
struct Row {
    rule: usize,
    mode: &'static str,
    scope: fn() -> Value,
}

/// No account scope at all: neither member named.
fn no_scope() -> Value {
    json!({})
}

/// An `includeAccountIds` scope.
fn include_scope() -> Value {
    json!({"includeAccountIds": ["111122223333", "222233334444"]})
}

/// An `excludeAccountIds` scope — the other side of the mutually exclusive pair.
fn exclude_scope() -> Value {
    json!({"excludeAccountIds": ["333344445555"]})
}

/// The configuration set every suite below builds, deliberately NOT in the order the
/// listing must report: the rows differ along the rule, the mode (both members of
/// `AssociationMode`, including both modes of one and the same rule) and the account
/// scope (none, include, exclude).
const ROWS: [Row; 6] = [
    Row {
        rule: 3,
        mode: "LIVE",
        scope: include_scope,
    },
    Row {
        rule: 0,
        mode: "DRY_RUN",
        scope: no_scope,
    },
    Row {
        rule: 3,
        mode: "DRY_RUN",
        scope: exclude_scope,
    },
    Row {
        rule: 1,
        mode: "LIVE",
        scope: no_scope,
    },
    Row {
        rule: 2,
        mode: "DRY_RUN",
        scope: include_scope,
    },
    Row {
        rule: 0,
        mode: "LIVE",
        scope: exclude_scope,
    },
];

/// Create every [`ROWS`] entry through `region`'s endpoint and return the
/// `(ruleId, mode)` pairs the listing must report, in the order it must report them.
async fn seed(router: &Router, region: &str, published: &[String]) -> Vec<(String, String)> {
    let mut expected: Vec<(String, String)> = Vec::new();
    for row in &ROWS {
        let rule_id = published[row.rule].clone();
        let mut body = (row.scope)();
        body["ruleId"] = json!(rule_id);
        body["mode"] = json!(row.mode);
        create_in(router, region, body).await;
        expected.push((rule_id, row.mode.to_string()));
    }
    // `(RuleId, Mode)` order — the order the store holds them in, and the order a page
    // token is an offset into. Not the order the creates above arrived in.
    expected.sort();
    expected
}

// ---------------------------------------------------------------------------
// Rule 1: the listing reports what the creates recorded, in (RuleId, Mode) order.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_listing_reports_every_recorded_configuration_in_rule_and_mode_order() {
    let router = new_router();
    let published = published_rule_ids(&router).await;

    // Before any create the list is EMPTY, not absent and not canned: `Configurations`
    // is `@required`, so the answer is an empty array.
    let (status, response) = list(&router, &[]).await;
    assert_eq!(
        status, 200,
        "an empty account expected 200, got {status} {response:?}"
    );
    assert_eq!(
        response["configurations"],
        json!([]),
        "an account with no organization configurations must list an empty array, got {response:?}"
    );
    assert!(
        response["nextToken"].is_null(),
        "an empty list must carry no nextToken, got {response:?}"
    );

    let expected = seed(&router, "us-east-1", &published).await;
    // Past the roll-out the creates left owed, so every status below is settled and a
    // listing and a detail read cannot straddle the transition.
    tokio::time::sleep(past_the_transition()).await;

    let configurations = all_in(&router, "us-east-1", "the whole set").await;
    assert_eq!(
        pairs_of(&configurations),
        expected,
        "the listing must report every recorded pair exactly once in (ruleId, mode) order, got {:?}",
        pairs_of(&configurations)
    );

    for configuration in &configurations {
        let (rule_id, mode) = (text(configuration, "ruleId"), text(configuration, "mode"));
        let case = format!("{rule_id} in mode {mode}");

        assert!(
            published.contains(&rule_id),
            "case {case:?}: a summary may only name a published rule, got {rule_id:?}"
        );
        assert!(
            MODES.contains(&mode.as_str()),
            "case {case:?}: mode must be an AssociationMode member, got {mode:?}"
        );
        let status = text(configuration, "status");
        assert!(
            STATUSES.contains(&status.as_str()),
            "case {case:?}: status must be a DetectionRuleConfigurationStatus member, got {status:?}"
        );
        assert!(
            configuration["createdAt"].is_number(),
            "case {case:?}: createdAt is required, got {configuration:?}"
        );
        assert!(
            configuration["updatedAt"].is_number(),
            "case {case:?}: updatedAt is required, got {configuration:?}"
        );
        // Nothing has changed the configuration since the create, and GuardDuty
        // documents no lifetime for one, so neither an invented reason nor an invented
        // expiry appears.
        assert!(
            configuration["statusReason"].is_null(),
            "case {case:?}: a configuration proceeding normally has no statusReason, got {configuration:?}"
        );
        assert!(
            configuration["expiresAt"].is_null(),
            "case {case:?}: no lifetime is documented, so expiresAt must be absent, got {configuration:?}"
        );

        // The summary is a PROJECTION of the same stored record the detail read
        // reports: every member the two shapes share must agree.
        let (got, read) = get_in(&router, "us-east-1", &rule_id, &mode).await;
        assert_eq!(
            got, 200,
            "case {case:?}: the detail read of a listed pair expected 200, got {got} {read:?}"
        );
        let detail = read["configuration"].clone();
        for member in ["ruleId", "mode", "status", "createdAt", "updatedAt"] {
            assert_eq!(
                configuration[member], detail[member],
                "case {case:?}: the summary's {member} must match the detail read's, got {:?} and {:?}",
                configuration[member], detail[member]
            );
        }
    }

    // A pair nothing configured is named by no summary, in either mode.
    let unconfigured = published
        .last()
        .expect("the catalogue carries rules")
        .clone();
    for mode in MODES {
        assert!(
            !pairs_of(&configurations).contains(&(unconfigured.clone(), mode.to_string())),
            "an unconfigured pair ({unconfigured}, {mode}) must not be listed, got {:?}",
            pairs_of(&configurations)
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: the status filter selects on the status the summary reports.
// ---------------------------------------------------------------------------

/// The rows differ along the mode and the account scope, and each is checked on BOTH
/// sides of the transition the create left owed — so the filter is asserted against a
/// status that is derived rather than stored.
///
/// Exactly ONE dispatch happens inside the roll-out span (the `PROCESSING` listing
/// straight after the create); the whole status table is asserted after the span, where
/// the status is settled for good, so no assertion races the clock.
#[tokio::test]
async fn the_status_filter_selects_on_the_status_the_summary_reports() {
    for row in &ROWS[..3] {
        let router = new_router();
        let published = published_rule_ids(&router).await;
        let rule_id = published[row.rule].clone();
        let mut body = (row.scope)();
        body["ruleId"] = json!(rule_id);
        body["mode"] = json!(row.mode);
        create_in(&router, "us-east-1", body).await;

        let pair = (rule_id.clone(), row.mode.to_string());

        // The roll-out span is never zero, so ONE filtered listing issued straight
        // after the create still observes the transient status. Only this single
        // dispatch happens inside the span, so the assertion does not depend on how
        // fast the rest of the suite runs.
        let case = format!("{rule_id} in mode {} straight after the create", row.mode);
        let (found, _) = page(
            &router,
            &[
                ("maxResults", MAX_RESULTS_MAX.to_string()),
                ("status", "PROCESSING".to_string()),
            ],
            &case,
        )
        .await;
        assert_eq!(
            pairs_of(&found),
            vec![pair.clone()],
            "case {case:?}: the roll-out is still owed, so the PROCESSING filter must select the configuration, got {:?}",
            pairs_of(&found)
        );

        // Past the span the status is settled for good, so the whole
        // `DetectionRuleConfigurationStatus` table can be asserted without racing the
        // clock.
        tokio::time::sleep(past_the_transition()).await;
        let unfiltered =
            all_in(&router, "us-east-1", &format!("{rule_id} after the transition")).await;
        assert_eq!(
            pairs_of(&unfiltered),
            vec![pair.clone()],
            "case {rule_id}: the unfiltered list must name the one configured pair, got {:?}",
            pairs_of(&unfiltered)
        );

        let mut selected_total = 0usize;
        for status in STATUSES {
            let case = format!(
                "{rule_id} in mode {} after the transition, status={status}",
                row.mode
            );
            let (found, token) = page(
                &router,
                &[
                    ("maxResults", MAX_RESULTS_MAX.to_string()),
                    ("status", status.to_string()),
                ],
                &case,
            )
            .await;
            assert!(
                token.is_none(),
                "case {case:?}: a complete filtered page must carry no nextToken, got {token:?}"
            );
            selected_total += found.len();
            if status == "ACTIVE" {
                assert_eq!(
                    pairs_of(&found),
                    vec![pair.clone()],
                    "case {case:?}: the configuration now reports ACTIVE, so this filter must select it, got {:?}",
                    pairs_of(&found)
                );
            } else {
                // PROCESSING is no longer owed, and nothing in this service enters
                // FAILED, so each selects nothing — an empty page, not a refusal.
                assert!(
                    found.is_empty(),
                    "case {case:?}: the configuration reports ACTIVE, so this filter must select nothing, got {:?}",
                    pairs_of(&found)
                );
            }
        }
        // The filters PARTITION the list: every configuration is selected by exactly
        // one `DetectionRuleConfigurationStatus` member.
        assert_eq!(
            selected_total,
            unfiltered.len(),
            "case {rule_id}: the three status filters must partition the list of {} configurations, got {selected_total}",
            unfiltered.len()
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: paging yields each configuration exactly once, in the unpaged order.
// ---------------------------------------------------------------------------

/// Walk every page at `page_size` (the service's own when `None`), optionally narrowed
/// to `status`, and return the configurations in the order the pages named them with
/// each page's length.
async fn walk(
    router: &Router,
    status: Option<&str>,
    page_size: Option<i64>,
    case: &str,
) -> (Vec<Value>, Vec<usize>) {
    let mut found = Vec::new();
    let mut lengths = Vec::new();
    let mut token: Option<String> = None;
    // Bounded so a token the service never advances cannot loop forever.
    for _ in 0..256 {
        let mut params: Vec<(&str, String)> = Vec::new();
        if let Some(status) = status {
            params.push(("status", status.to_string()));
        }
        if let Some(size) = page_size {
            params.push(("maxResults", size.to_string()));
        }
        if let Some(token) = token.as_deref() {
            params.push(("nextToken", token.to_string()));
        }
        let (configurations, next) = page(router, &params, case).await;
        if let Some(size) = page_size {
            assert!(
                configurations.len() as i64 <= size,
                "case {case:?}: a page must not exceed maxResults={size}, got {}",
                configurations.len()
            );
        }
        assert!(
            configurations.len() as i64 <= MAX_RESULTS_MAX,
            "case {case:?}: a page must carry at most {MAX_RESULTS_MAX} summaries, got {}",
            configurations.len()
        );
        lengths.push(configurations.len());
        found.extend(configurations);
        match next {
            Some(next) => token = Some(next),
            None => return (found, lengths),
        }
    }
    panic!("case {case:?}: paging did not terminate");
}

#[tokio::test]
async fn paging_the_org_configurations_yields_each_exactly_once() {
    let router = new_router();
    let published = published_rule_ids(&router).await;
    let expected = seed(&router, "us-east-1", &published).await;
    tokio::time::sleep(past_the_transition()).await;

    // Page sizes spanning one-at-a-time, every size up to just past the set's own
    // length, and the largest size the member admits, plus the service's own page size
    // when the caller names none.
    let mut sizes: Vec<Option<i64>> = (1..=(expected.len() as i64 + 2)).map(Some).collect();
    sizes.push(Some(MAX_RESULTS_MAX));
    sizes.push(None);

    // Unfiltered, and narrowed to the status every configuration now reports: the
    // token must stay meaningful under the filter it was issued with.
    for status in [None, Some("ACTIVE")] {
        for size in &sizes {
            let case = match (status, size) {
                (None, Some(size)) => format!("maxResults={size}"),
                (None, None) => "no maxResults".to_string(),
                (Some(status), Some(size)) => format!("status={status}, maxResults={size}"),
                (Some(status), None) => format!("status={status}, no maxResults"),
            };
            let (found, lengths) = walk(&router, status, *size, &case).await;
            assert_eq!(
                pairs_of(&found),
                expected,
                "case {case:?}: paging must yield every configuration exactly once in order, got {:?}",
                pairs_of(&found)
            );
            if let Some(size) = size {
                // Every page but the last is FULL, so a caller cannot mistake a short
                // page for the end of the list.
                for (index, length) in lengths.iter().enumerate().take(lengths.len() - 1) {
                    assert_eq!(
                        *length, *size as usize,
                        "case {case:?}: page {index} must be full at {size}, got {length}"
                    );
                }
            }
        }
    }

    // A page that completes the list carries no token at all.
    for size in [
        expected.len() as i64,
        expected.len() as i64 + 1,
        MAX_RESULTS_MAX,
    ] {
        let case = format!("a complete page at maxResults={size}");
        let (_, token) = page(&router, &[("maxResults", size.to_string())], &case).await;
        assert!(
            token.is_none(),
            "case {case:?}: a complete page must carry no nextToken, got {token:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 4: the listing reports; it changes nothing a later call sees.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn listing_org_configurations_changes_nothing_a_later_caller_sees() {
    let router = new_router();
    let published = published_rule_ids(&router).await;
    seed(&router, "us-east-1", &published).await;
    tokio::time::sleep(past_the_transition()).await;

    let first = all_in(&router, "us-east-1", "the first listing").await;
    let details: Vec<Value> = {
        let mut details = Vec::new();
        for (rule_id, mode) in pairs_of(&first) {
            let (status, read) = get_in(&router, "us-east-1", &rule_id, &mode).await;
            assert_eq!(status, 200, "the detail read of {rule_id}/{mode} expected 200, got {read:?}");
            details.push(read["configuration"].clone());
        }
        details
    };

    // Many listings, paged and filtered, interleaved with detail reads.
    for size in [1, 2, 3] {
        walk(&router, None, Some(size), &format!("an interleaved page at maxResults={size}")).await;
        walk(
            &router,
            Some("ACTIVE"),
            Some(size),
            &format!("an interleaved ACTIVE page at maxResults={size}"),
        )
        .await;
    }
    for status in STATUSES {
        let _ = list(&router, &[("status", status.to_string())]).await;
    }

    let again = all_in(&router, "us-east-1", "the listing after many reads").await;
    assert_eq!(
        again, first,
        "a listing must not change what a later listing reports"
    );
    for (index, (rule_id, mode)) in pairs_of(&first).iter().enumerate() {
        let (status, read) = get_in(&router, "us-east-1", rule_id, mode).await;
        assert_eq!(status, 200, "the re-read of {rule_id}/{mode} expected 200, got {read:?}");
        assert_eq!(
            read["configuration"], details[index],
            "a listing must not change what a later detail read of {rule_id}/{mode} reports"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 5: a configuration belongs to the Region whose endpoint created it.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_org_configuration_is_listed_only_by_the_region_that_created_it() {
    let router = new_router();
    let published = published_rule_ids(&router).await;

    // Different pairs per Region, so a Region reporting another's set is visible in the
    // pairs and not only in the counts.
    let home = ("us-east-1", published[0].clone(), "LIVE");
    let away = ("eu-west-1", published[1].clone(), "DRY_RUN");
    for (region, rule_id, mode) in [&home, &away] {
        create_in(
            &router,
            region,
            json!({"ruleId": rule_id, "mode": mode}),
        )
        .await;
    }
    tokio::time::sleep(past_the_transition()).await;

    for (region, rule_id, mode) in [&home, &away] {
        let listed = all_in(&router, region, &format!("the listing through {region}")).await;
        assert_eq!(
            pairs_of(&listed),
            vec![(rule_id.clone(), (*mode).to_string())],
            "case {region}: an endpoint lists only the configurations it created, got {:?}",
            pairs_of(&listed)
        );
    }

    // A third Region created none, so it lists none — not the other Regions' sets.
    let listed = all_in(&router, "ap-southeast-2", "a Region that created none").await;
    assert!(
        listed.is_empty(),
        "a Region that created no configuration must list none, got {:?}",
        pairs_of(&listed)
    );
}

// ---------------------------------------------------------------------------
// Rule 6: an unusable request is refused, and a refusal changes nothing.
// ---------------------------------------------------------------------------

/// One refusal row: the case, the query parameters that make the request unusable,
/// and the member the bad value sat on — empty where the refusal comes from the
/// modelled constraint rather than from the handler, which words the refusal itself.
struct Refusal {
    case: &'static str,
    params: Vec<(&'static str, String)>,
    member: &'static str,
}

#[tokio::test]
async fn an_unusable_listing_request_is_refused_without_echoing_the_value() {
    let router = new_router();
    let published = published_rule_ids(&router).await;
    let expected = seed(&router, "us-east-1", &published).await;
    tokio::time::sleep(past_the_transition()).await;
    let before = all_in(&router, "us-east-1", "before any refusal").await;
    let count = expected.len() as i64;

    let rows: Vec<Refusal> = vec![
        Refusal {
            case: "a nextToken that is not a token this service issues",
            params: vec![("nextToken", "not-a-token".to_string())],
            member: "nextToken",
        },
        Refusal {
            case: "a nextToken pointing far past the end of the set",
            params: vec![("nextToken", "9999".to_string())],
            member: "nextToken",
        },
        Refusal {
            case: "a nextToken pointing at the entry just past the last one",
            params: vec![("nextToken", count.to_string())],
            member: "nextToken",
        },
        Refusal {
            case: "a nextToken past the end of a FILTERED set",
            params: vec![
                ("status", "ACTIVE".to_string()),
                ("nextToken", count.to_string()),
            ],
            member: "nextToken",
        },
        Refusal {
            case: "a nextToken past the end of a set the filter emptied",
            params: vec![
                ("status", "FAILED".to_string()),
                ("nextToken", "0".to_string()),
            ],
            member: "nextToken",
        },
        Refusal {
            case: "a maxResults of 0: one below the published minimum of 1",
            params: vec![("maxResults", "0".to_string())],
            member: "",
        },
        Refusal {
            case: "a maxResults of 101: one above the published maximum of 100",
            params: vec![("maxResults", (MAX_RESULTS_MAX + 1).to_string())],
            member: "",
        },
        Refusal {
            case: "a negative maxResults",
            params: vec![("maxResults", "-5".to_string())],
            member: "",
        },
        Refusal {
            case: "a status outside the DetectionRuleConfigurationStatus enum",
            params: vec![("status", "ENABLED".to_string())],
            member: "",
        },
    ];

    for row in rows {
        let case = row.case;
        let (status, response) = list(&router, &row.params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {response:?}"
        );
        let message = response["message"].as_str().unwrap_or_default();
        if !row.member.is_empty() {
            let class = error_code(&response);
            assert_eq!(
                class, INVALID_INPUT,
                "case {case:?}: expected the {INVALID_INPUT} class, got {class:?} {response:?}"
            );
            // Every row that reaches the handler here carries a page token, and a
            // token this service did not issue is refused with the ONE sentence the
            // whole pagination surface answers, which names no member at all.
            assert_eq!(
                message,
                page_token_refusal(),
                "case {case:?}: expected the page-token refusal, got {message:?}"
            );
        }
        for echoed in ["not-a-token", "9999", "ENABLED", "-5"] {
            assert!(
                !message.contains(echoed),
                "case {case:?}: the refusal must not echo {echoed:?}, got {message:?}"
            );
        }
        let after = all_in(&router, "us-east-1", &format!("after {case:?}")).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refusal must not change the listing"
        );
    }

    // The token the service DOES issue is accepted on the same listing, so the refusals
    // above are about tokens it never handed out and not about paging itself.
    let (first, token) = page(&router, &[("maxResults", "1".to_string())], "the first page").await;
    let token = token.expect("the first page of a multi-configuration set carries a nextToken");
    let (second, _) = page(
        &router,
        &[("maxResults", "1".to_string()), ("nextToken", token)],
        "the issued token",
    )
    .await;
    assert_eq!(
        pairs_of(&first),
        expected[..1].to_vec(),
        "the first page must be the first configuration in order, got {:?}",
        pairs_of(&first)
    );
    assert_eq!(
        pairs_of(&second),
        expected[1..2].to_vec(),
        "the issued token must resume at the second configuration, got {:?}",
        pairs_of(&second)
    );
}
