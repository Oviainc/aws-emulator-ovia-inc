// SPDX-License-Identifier: CC0-1.0

//! `ListCustomDetectionRules` and `GetCustomDetectionRule`, end to end over the wire.
//!
//! Every request goes through [`smithy_server_runtime::MultiServiceRouter::dispatch`]
//! (the harness lives in the `detector_lifecycle` suite), so routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers are exercised
//! together.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! call:
//!
//! 1. `ListCustomDetectionRules` reports the custom detection rules the service
//!    publishes — "Returns all available custom detection rules in GuardDuty"
//!    (`smithy_query(query="doc",shape="ListCustomDetectionRules")`) — in `RuleId`
//!    order, every reported member inside its modelled bounds, and every rule it
//!    names resolves through `GetCustomDetectionRule` to the same values plus the
//!    detection logic only the detail read carries.
//! 2. A `DetectionRuleFilter` selects on the value the list reports for the field it
//!    names: `EQUALS` the rules whose field is one of the filter's `Values`,
//!    `CONTAINS` the rules whose field has one of them as a substring, a filter that
//!    names no condition as `EQUALS`; a filter's values are OR-ed and a request's
//!    filters AND-ed.
//! 3. It is PAGINATED (`smithy_query(operation="ListCustomDetectionRules")` reports
//!    `inputToken`/`outputToken` `NextToken`, `items` `Rules`, `pageSize`
//!    `MaxResults`): walking the pages with any page size yields each rule exactly
//!    once in the unpaged order, the last page carries no `NextToken`, and no page is
//!    longer than the page size asked for.
//! 4. Both operations REPORT: no number of lists, pages or detail reads changes what
//!    a later caller sees.
//! 5. A rule is the SERVICE's, not an account's: every Region's endpoint publishes the
//!    same catalogue, and the ARN a read reports is the AWS-MANAGED one — empty Region
//!    segment, account segment `aws` — so it names neither the Region that served the
//!    read nor the caller's account.
//! 6. A `nextToken` the service never issued, a filter field outside
//!    `FilterFieldName`, and a `maxResults` outside `range 1..100` are refused with
//!    HTTP 400 without echoing the caller's value; a well-formed `ruleId` the
//!    catalogue does not publish is refused with the modelled 404
//!    `ResourceNotFoundException`, never a hollow success.

use serde_json::{json, Value};

use crate::client_rejections::page_token_refusal;
use crate::detector_lifecycle::{call, call_in, error_code, new_router, Router};

/// `ListCustomDetectionRules` is `POST /custom-detection-rule/rule` and
/// `GetCustomDetectionRule` is `GET /custom-detection-rule/rule/{RuleId}`
/// (`smithy_query(operation="ListCustomDetectionRules,GetCustomDetectionRule")`).
const RULES_PATH: &str = "/custom-detection-rule/rule";

/// Every `FilterFieldName` member — the rule fields a filter may name
/// (`smithy_query(shape="FilterFieldName")`).
const FILTER_FIELDS: [&str; 7] = [
    "name",
    "description",
    "dataSource",
    "severity",
    "tactic",
    "technique",
    "service",
];

/// Every `DetectionRuleSeverity` member (`smithy_query(shape="DetectionRuleSeverity")`).
const SEVERITIES: [&str; 4] = ["CRITICAL", "HIGH", "LOW", "MEDIUM"];

/// Every `DetectionRuleDataSource` member
/// (`smithy_query(shape="DetectionRuleDataSource")`).
const DATA_SOURCES: [&str; 2] = ["CloudTrailManagementEvent", "RuntimeMonitoring"];

/// `RuleLanguage`'s only member, and `RuleSchema`'s
/// (`smithy_query(shape="RuleLanguage,RuleSchema")`).
const LANGUAGES: [&str; 1] = ["SQL"];
/// See [`LANGUAGES`].
const SCHEMAS: [&str; 1] = ["CloudTrail"];

/// `MaxResults` is `range 1..100` (`smithy_query(operation="ListCustomDetectionRules")`),
/// so one page can never carry more than 100 rules whatever a caller asks for.
const MAX_RESULTS_MAX: i64 = 100;

/// The modelled length bounds of the `RuleSummary` members a rule reports, in the
/// order `(member, max)` (`smithy_query(shape="RuleSummary")`). `RuleId` is
/// `len 1..100`, `Name` 1..255, `Description` 1..1024, `Tactic` 1..255,
/// `Technique` 1..255 and `Service` 1..128.
const REQUIRED_TEXT_MEMBERS: [(&str, usize); 6] = [
    ("ruleId", 100),
    ("name", 255),
    ("description", 1024),
    ("tactic", 255),
    ("technique", 255),
    ("service", 128),
];

/// The error class every client rejection on this service carries in the body's
/// `__type` discriminator.
const INVALID_INPUT: &str = "InvalidInputException";

/// A well-formed `RuleId` (`^[a-z0-9]+(-[a-z0-9]+)*$`, `len 1..100`) the catalogue
/// does not publish.
const ABSENT_RULE_ID: &str = "no-such-published-rule";

/// One `ListCustomDetectionRules` call with `body` as its request body.
async fn list(router: &Router, body: Value) -> (u16, Value) {
    call(router, "POST", RULES_PATH, Some(body)).await
}

/// The `Rules` of one page, failing the test if the list did not succeed.
async fn page(router: &Router, body: Value, case: &str) -> (Vec<Value>, Option<String>) {
    let (status, response) = list(router, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: list-custom-detection-rules expected 200, got {status} {response:?}"
    );
    let rules = response["rules"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a rules array, got {response:?}"))
        .clone();
    let token = response["nextToken"].as_str().map(str::to_string);
    (rules, token)
}

/// The whole published catalogue, in one page.
async fn catalogue(router: &Router) -> Vec<Value> {
    let (rules, token) = page(
        router,
        json!({"maxResults": MAX_RESULTS_MAX}),
        "the whole catalogue",
    )
    .await;
    assert!(
        token.is_none(),
        "case \"the whole catalogue\": the catalogue must fit in one page of {MAX_RESULTS_MAX}, got a nextToken"
    );
    assert!(
        !rules.is_empty(),
        "case \"the whole catalogue\": GuardDuty publishes custom detection rules, so the list must not be empty"
    );
    rules
}

fn ids_of(rules: &[Value]) -> Vec<String> {
    rules
        .iter()
        .map(|rule| text(rule, "ruleId").to_string())
        .collect()
}

/// A string member of a reported rule, failing the test when it is absent.
fn text<'a>(rule: &'a Value, member: &str) -> &'a str {
    rule[member]
        .as_str()
        .unwrap_or_else(|| panic!("expected the member {member:?} on {rule:?}"))
}

/// Walk every page of a filtered list with `page_size` (`None` asks for the
/// service's own page size), returning the rules in page order and each page's
/// length.
async fn walk(
    router: &Router,
    filters: &Value,
    page_size: Option<i64>,
    case: &str,
) -> (Vec<Value>, Vec<usize>) {
    let mut rules = Vec::new();
    let mut lengths = Vec::new();
    let mut token: Option<String> = None;
    // Bounded so a token the service never advances cannot loop forever.
    for _ in 0..512 {
        let mut body = json!({"filters": filters.clone()});
        if let Some(size) = page_size {
            body["maxResults"] = json!(size);
        }
        if let Some(token) = token.as_deref() {
            body["nextToken"] = json!(token);
        }
        let (found, next) = page(router, body, case).await;
        if let Some(size) = page_size {
            assert!(
                found.len() as i64 <= size,
                "case {case:?}: a page must not exceed maxResults={size}, got {}",
                found.len()
            );
        }
        assert!(
            found.len() as i64 <= MAX_RESULTS_MAX,
            "case {case:?}: a page must carry at most {MAX_RESULTS_MAX} rules, got {}",
            found.len()
        );
        lengths.push(found.len());
        rules.extend(found);
        match next {
            Some(next) => token = Some(next),
            None => return (rules, lengths),
        }
    }
    panic!("case {case:?}: the list never ran out of pages");
}

/// Is `value` sendable as a `DetectionRuleFilterValue`? The member is
/// `len 1..255, pattern ^[a-zA-Z0-9 _.\-:/]+$`
/// (`smithy_query(operation="ListCustomDetectionRules")`), so a rule field carrying
/// anything else — a comma in a description, say — cannot be named by a filter value
/// at all, and the tables below skip it rather than send a request the model refuses.
fn sendable(value: &str) -> bool {
    !value.is_empty()
        && value.chars().count() <= 255
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '_' | '.' | '-' | ':' | '/'))
}

/// Does `rule_id` satisfy `RuleId`'s published form — `len 1..100`,
/// `pattern ^[a-z0-9]+(-[a-z0-9]+)*$` (`smithy_query(shape="RuleId")`)?
fn is_rule_id(rule_id: &str) -> bool {
    !rule_id.is_empty()
        && rule_id.len() <= 100
        && rule_id.split('-').all(|segment| {
            !segment.is_empty()
                && segment
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        })
}

// ---------------------------------------------------------------------------
// Rule 1: the list reports the published catalogue, and every rule resolves.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_list_reports_the_published_catalogue_and_each_rule_resolves() {
    let router = new_router();
    let rules = catalogue(&router).await;
    let ids = ids_of(&rules);

    // `RuleId` order, which is a total order over the catalogue: that is what makes a
    // page token mean the same thing on the call that handed it back.
    let mut sorted = ids.clone();
    sorted.sort();
    assert_eq!(
        ids, sorted,
        "case \"catalogue order\": expected the rules in ruleId order {sorted:?}, got {ids:?}"
    );
    let mut unique = ids.clone();
    unique.dedup();
    assert_eq!(
        unique, ids,
        "case \"catalogue ids\": a ruleId identifies one rule, got {ids:?}"
    );

    // The catalogue spans what a caller can select on, so a filter table has more
    // than one row to find: EVERY rule's `dataSource` and `severity` is a member of
    // that field's own enum, and the catalogue carries more than one distinct
    // `service` for a filter to narrow on.
    let off_domain: Vec<(&str, &str)> = rules
        .iter()
        .filter_map(|rule| {
            let data_source = text(rule, "dataSource");
            let severity = text(rule, "severity");
            (!DATA_SOURCES.contains(&data_source) || !SEVERITIES.contains(&severity))
                .then_some((data_source, severity))
        })
        .collect();
    assert!(
        off_domain.is_empty(),
        "case \"catalogue coverage\": every published rule names a dataSource of {DATA_SOURCES:?} and a severity of {SEVERITIES:?}, got {off_domain:?}"
    );
    let distinct_services: Vec<&str> = {
        let mut services: Vec<&str> = rules.iter().map(|rule| text(rule, "service")).collect();
        services.sort_unstable();
        services.dedup();
        services
    };
    assert!(
        distinct_services.len() > 1,
        "case \"catalogue coverage\": expected rules over more than one service, so a filter has something to narrow, got {distinct_services:?}"
    );

    for rule in &rules {
        let id = text(rule, "ruleId").to_string();
        let case = format!("the published rule {id:?}");

        for (member, max) in REQUIRED_TEXT_MEMBERS {
            let value = text(rule, member);
            assert!(
                !value.is_empty() && value.chars().count() <= max,
                "case {case:?}: {member} must be 1..={max} characters, got {} in {value:?}",
                value.chars().count()
            );
        }
        assert!(
            is_rule_id(&id),
            "case {case:?}: a ruleId must match ^[a-z0-9]+(-[a-z0-9]+)*$, got {id:?}"
        );
        let severity = text(rule, "severity");
        assert!(
            SEVERITIES.contains(&severity),
            "case {case:?}: severity must be a DetectionRuleSeverity member {SEVERITIES:?}, got {severity:?}"
        );
        let data_source = text(rule, "dataSource");
        assert!(
            DATA_SOURCES.contains(&data_source),
            "case {case:?}: dataSource must be a DetectionRuleDataSource member {DATA_SOURCES:?}, got {data_source:?}"
        );
        if let Some(language) = rule["language"].as_str() {
            assert!(
                LANGUAGES.contains(&language),
                "case {case:?}: language must be a RuleLanguage member {LANGUAGES:?}, got {language:?}"
            );
        }
        if let Some(schema) = rule["schema"].as_str() {
            assert!(
                SCHEMAS.contains(&schema),
                "case {case:?}: schema must be a RuleSchema member {SCHEMAS:?}, got {schema:?}"
            );
        }
        // `RuleSchema`'s only member is `CloudTrail`, so a rule over CloudTrail
        // management events reports it and a rule over any other data source has no
        // schema the model can name.
        assert_eq!(
            rule["schema"].as_str().is_some(),
            data_source == "CloudTrailManagementEvent",
            "case {case:?}: a schema is reported exactly for a CloudTrail-sourced rule, got {:?} for {data_source:?}",
            rule["schema"]
        );
        // `CreatedAt` is required, and a timestamp renders as epoch seconds on
        // restJson1.
        let created_at = rule["createdAt"].as_f64().unwrap_or_default();
        assert!(
            created_at > 0.0,
            "case {case:?}: createdAt must be an instant, got {:?}",
            rule["createdAt"]
        );

        // The ARN names this rule, in the form AWS publishes a managed catalogue
        // object under: no Region segment and the account segment `aws`.
        let arn = text(rule, "arn");
        let segments: Vec<&str> = arn.splitn(6, ':').collect();
        assert_eq!(
            segments.len(),
            6,
            "case {case:?}: an ARN has six segments, got {arn:?}"
        );
        assert_eq!(
            (segments[0], segments[2], segments[3], segments[4]),
            ("arn", "guardduty", "", "aws"),
            "case {case:?}: expected the AWS-managed guardduty ARN form, got {arn:?}"
        );
        assert!(
            segments[5].ends_with(&format!("/{id}")),
            "case {case:?}: the ARN must name the rule, got {arn:?}"
        );

        // The detail read resolves every listed rule to the same values, plus the
        // detection logic the summary does not carry.
        let (status, detail) = call(&router, "GET", &format!("{RULES_PATH}/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: get-custom-detection-rule expected 200, got {status} {detail:?}"
        );
        let got = &detail["rule"];
        for member in [
            "ruleId",
            "arn",
            "name",
            "description",
            "severity",
            "dataSource",
            "tactic",
            "technique",
            "service",
        ] {
            assert_eq!(
                got[member], rule[member],
                "case {case:?}: the detail read must report the listed {member}, got {:?} after {:?}",
                got[member], rule[member]
            );
        }
        for member in ["language", "schema", "createdAt", "updatedAt"] {
            assert_eq!(
                got[member], rule[member],
                "case {case:?}: the detail read must report the listed {member}, got {:?} after {:?}",
                got[member], rule[member]
            );
        }
        let expression = got["definition"]["expression"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a definition expression, got {got:?}"));
        assert!(
            !expression.is_empty() && expression.chars().count() <= 4096,
            "case {case:?}: RuleExpression is len 1..4096, got {} characters",
            expression.chars().count()
        );
        // "The presence of a platform member indicates that the rule is supported on
        // that platform", so a Runtime Monitoring rule names at least one platform
        // and a rule over another data source reports no Runtime Monitoring details
        // at all.
        let platforms = &got["runtimeMonitoringDetails"]["platforms"];
        if data_source == "RuntimeMonitoring" {
            let named = ["eks", "ec2", "ecsFargate"]
                .into_iter()
                .filter(|platform| !platforms[*platform].is_null())
                .count();
            assert!(
                named > 0,
                "case {case:?}: a RuntimeMonitoring rule must name the platforms it runs on, got {:?}",
                got["runtimeMonitoringDetails"]
            );
        } else {
            assert!(
                got["runtimeMonitoringDetails"].is_null(),
                "case {case:?}: a rule that is not a RuntimeMonitoring rule has no runtime details, got {:?}",
                got["runtimeMonitoringDetails"]
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 2: a filter selects on the value the list reports for its field.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn filters_select_the_rules_whose_field_matches() {
    let router = new_router();
    let rules = catalogue(&router).await;

    // Each row: the field, the value, and the condition the request carries (`None`
    // for a filter that names none). The rows are derived from the catalogue itself —
    // every field a filter may name, every distinct value the catalogue holds for it,
    // a value no rule carries, and a substring of a held value for `CONTAINS` — so
    // the table spans the class rather than one recorded field.
    let mut rows: Vec<(&str, String, Option<&str>)> = Vec::new();
    for field in FILTER_FIELDS {
        let mut values: Vec<String> = rules
            .iter()
            .map(|rule| text(rule, field).to_string())
            .filter(|value| sendable(value))
            .collect();
        values.sort();
        values.dedup();
        for value in values.iter().take(4) {
            rows.push((field, value.clone(), None));
            rows.push((field, value.clone(), Some("EQUALS")));
            rows.push((field, value.clone(), Some("CONTAINS")));
        }
        // A substring of a held value: the longest sendable word of the first value,
        // which `CONTAINS` must find and `EQUALS` must not (unless the word IS the
        // value).
        if let Some(word) = values
            .first()
            .and_then(|value| value.split_whitespace().max_by_key(|word| word.len()))
            .filter(|word| sendable(word))
            .map(str::to_string)
        {
            rows.push((field, word.clone(), Some("CONTAINS")));
            rows.push((field, word, Some("EQUALS")));
        }
        rows.push((field, "ValueNoRuleCarries".to_string(), None));
        rows.push((field, "ValueNoRuleCarries".to_string(), Some("CONTAINS")));
    }

    for (field, value, condition) in rows {
        let case = format!("filter {field}={value:?} with condition {condition:?}");
        let mut filter = json!({"name": field, "values": [value.clone()]});
        if let Some(condition) = condition {
            filter["condition"] = json!(condition);
        }
        let mut expected: Vec<String> = rules
            .iter()
            .filter(|rule| {
                let actual = text(rule, field);
                if condition == Some("CONTAINS") {
                    actual.contains(value.as_str())
                } else {
                    actual == value
                }
            })
            .map(|rule| text(rule, "ruleId").to_string())
            .collect();
        expected.sort();

        let (found, _) = walk(&router, &json!([filter]), None, &case).await;
        assert_eq!(
            ids_of(&found),
            expected,
            "case {case:?}: expected exactly the matching rules {expected:?}, got {:?}",
            ids_of(&found)
        );
    }
}

#[tokio::test]
async fn a_filters_values_are_or_ed_and_its_filters_and_ed() {
    let router = new_router();
    let rules = catalogue(&router).await;

    // Every severity pair, OR-ed inside one filter: a filter's `Values` are the
    // alternatives for one field.
    for (index, left) in SEVERITIES.into_iter().enumerate() {
        for right in SEVERITIES.into_iter().skip(index) {
            let case = format!("severity in {{{left}, {right}}}");
            let mut expected: Vec<String> = rules
                .iter()
                .filter(|rule| {
                    let severity = text(rule, "severity");
                    severity == left || severity == right
                })
                .map(|rule| text(rule, "ruleId").to_string())
                .collect();
            expected.sort();
            let filters = json!([{"name": "severity", "values": [left, right]}]);
            let (found, _) = walk(&router, &filters, None, &case).await;
            assert_eq!(
                ids_of(&found),
                expected,
                "case {case:?}: expected the union of both values {expected:?}, got {:?}",
                ids_of(&found)
            );
        }
    }

    // Every (severity, dataSource) pair, as two filters: a request's filters are
    // AND-ed, so the page is the intersection — including the pairs no rule
    // satisfies, which select nothing rather than falling back to one filter.
    for severity in SEVERITIES {
        for data_source in DATA_SOURCES {
            let case = format!("severity={severity} AND dataSource={data_source}");
            let mut expected: Vec<String> = rules
                .iter()
                .filter(|rule| {
                    text(rule, "severity") == severity && text(rule, "dataSource") == data_source
                })
                .map(|rule| text(rule, "ruleId").to_string())
                .collect();
            expected.sort();
            let filters = json!([
                {"name": "severity", "values": [severity]},
                {"name": "dataSource", "values": [data_source]},
            ]);
            let (found, _) = walk(&router, &filters, None, &case).await;
            assert_eq!(
                ids_of(&found),
                expected,
                "case {case:?}: expected the intersection {expected:?}, got {:?}",
                ids_of(&found)
            );
        }
    }

    // An empty filter list narrows nothing: it is the same page as no `Filters` at
    // all.
    let (unfiltered, _) = walk(&router, &json!([]), None, "an empty filter list").await;
    assert_eq!(
        ids_of(&unfiltered),
        ids_of(&rules),
        "case \"an empty filter list\": expected the whole catalogue, got {:?}",
        ids_of(&unfiltered)
    );
}

// ---------------------------------------------------------------------------
// Rule 3: paging with any page size yields each rule exactly once, in order.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn paging_yields_each_rule_exactly_once_in_order() {
    let router = new_router();
    let rules = catalogue(&router).await;
    let expected = ids_of(&rules);

    // The page sizes span `MaxResults`'s whole modelled range at both published
    // bounds and across the catalogue's own length, plus the service's page size when
    // the caller names none.
    let mut sizes: Vec<Option<i64>> = (1..=(expected.len() as i64 + 2)).map(Some).collect();
    sizes.push(Some(MAX_RESULTS_MAX));
    sizes.push(None);

    for size in sizes {
        let case = match size {
            Some(size) => format!("maxResults={size}"),
            None => "no maxResults".to_string(),
        };
        let (found, lengths) = walk(&router, &json!([]), size, &case).await;
        assert_eq!(
            ids_of(&found),
            expected,
            "case {case:?}: paging must yield every rule exactly once in order, got {:?}",
            ids_of(&found)
        );
        // Every page but the last is full: that is what makes the token walk
        // terminate in the number of calls a client budgets for.
        let per_page = size.unwrap_or(MAX_RESULTS_MAX).min(MAX_RESULTS_MAX) as usize;
        for (index, length) in lengths.iter().enumerate() {
            if index + 1 < lengths.len() {
                assert_eq!(
                    *length, per_page,
                    "case {case:?}: page {index} must be full at {per_page}, got {length}"
                );
            } else {
                assert!(
                    *length <= per_page,
                    "case {case:?}: the last page must carry at most {per_page} rules, got {length}"
                );
            }
        }
    }

    // Paging is over the FILTERED list: the token is an offset into the rules the
    // filters selected, so a walk at page size 1 under a filter yields exactly those.
    for data_source in DATA_SOURCES {
        let case = format!("paging the {data_source} rules one at a time");
        let mut matching: Vec<String> = rules
            .iter()
            .filter(|rule| text(rule, "dataSource") == data_source)
            .map(|rule| text(rule, "ruleId").to_string())
            .collect();
        matching.sort();
        let filters = json!([{"name": "dataSource", "values": [data_source]}]);
        let (found, _) = walk(&router, &filters, Some(1), &case).await;
        assert_eq!(
            ids_of(&found),
            matching,
            "case {case:?}: expected the filtered rules {matching:?}, got {:?}",
            ids_of(&found)
        );
    }

    // A page that completes the list carries no token at all.
    for size in [expected.len() as i64, expected.len() as i64 + 1, MAX_RESULTS_MAX] {
        let case = format!("a complete page at maxResults={size}");
        let (_, token) = page(&router, json!({"maxResults": size}), &case).await;
        assert!(
            token.is_none(),
            "case {case:?}: a complete page must carry no nextToken, got {token:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 4: both operations report; they change nothing a later call sees.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn reading_changes_nothing_a_later_call_sees() {
    let router = new_router();
    let first = catalogue(&router).await;

    // Several lists, paged and filtered, and a detail read of every rule in between.
    for size in [1, 2, 3] {
        walk(
            &router,
            &json!([]),
            Some(size),
            &format!("an interleaved page at maxResults={size}"),
        )
        .await;
    }
    for rule in &first {
        let id = text(rule, "ruleId");
        let (status, _) = call(&router, "GET", &format!("{RULES_PATH}/{id}"), None).await;
        assert_eq!(status, 200, "case {id:?}: the interleaved read expected 200");
    }
    walk(
        &router,
        &json!([{"name": "severity", "values": ["HIGH"]}]),
        None,
        "an interleaved filtered list",
    )
    .await;

    let again = catalogue(&router).await;
    assert_eq!(
        again, first,
        "case \"a repeated list\": reading must not change what a later list sees, got {again:?} after {first:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 5: the catalogue is the service's, and the ARN names the Region read.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_region_publishes_the_same_catalogue() {
    let router = new_router();
    let home = catalogue(&router).await;

    for region in ["us-east-1", "eu-west-1", "ap-southeast-2"] {
        let case = format!("the catalogue through {region}");
        let (status, response) = call_in(
            &router,
            region,
            "POST",
            RULES_PATH,
            Some(json!({"maxResults": MAX_RESULTS_MAX})),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {response:?}"
        );
        let rules = response["rules"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected a rules array, got {response:?}"))
            .clone();
        assert_eq!(
            ids_of(&rules),
            ids_of(&home),
            "case {case:?}: a rule is published by the service, so every Region names the same rules, got {:?}",
            ids_of(&rules)
        );
        for (rule, expected) in rules.iter().zip(home.iter()) {
            let arn = text(rule, "arn");
            assert_eq!(
                rule["arn"], expected["arn"],
                "case {case:?}: a published rule carries ONE ARN whatever Region reads it, got {arn:?}"
            );
            let segments: Vec<&str> = arn.splitn(6, ':').collect();
            assert_eq!(
                (segments.get(3).copied(), segments.get(4).copied()),
                (Some(""), Some("aws")),
                "case {case:?}: the ARN of an AWS-managed rule names no Region and no account, got {arn:?}"
            );
            assert_eq!(
                rule["createdAt"], expected["createdAt"],
                "case {case:?}: a rule carries one createdAt whatever Region reads it, got {:?} after {:?}",
                rule["createdAt"], expected["createdAt"]
            );
            // The detail read of the same rule, through the same endpoint, agrees.
            let id = text(rule, "ruleId");
            let (status, detail) =
                call_in(&router, region, "GET", &format!("{RULES_PATH}/{id}"), None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: get {id:?} expected 200, got {status} {detail:?}"
            );
            assert_eq!(
                detail["rule"]["arn"], rule["arn"],
                "case {case:?}: the detail read must report the listed ARN, got {:?}",
                detail["rule"]["arn"]
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 6: refusals — classified, without echoing the caller's value.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_unissued_token_or_an_unknown_filter_field_is_refused() {
    let router = new_router();
    let rules = catalogue(&router).await;
    let count = rules.len() as i64;

    // Each row: the case, the request body, and the member the bad value sat on
    // (empty where the refusal comes from the modelled constraint rather than from
    // the handler, which words the refusal itself).
    let rows: Vec<(String, Value, &str)> = vec![
        (
            "a nextToken that is not a token this service issues".to_string(),
            json!({"nextToken": "not-a-token"}),
            "nextToken",
        ),
        (
            "a nextToken pointing past the end of the catalogue".to_string(),
            json!({"nextToken": "9999"}),
            "nextToken",
        ),
        (
            "a nextToken pointing at the entry just past the last one".to_string(),
            json!({"nextToken": count.to_string()}),
            "nextToken",
        ),
        (
            "a nextToken pointing past the end of a FILTERED list".to_string(),
            json!({
                "nextToken": count.to_string(),
                "filters": [{"name": "severity", "values": ["HIGH"]}],
            }),
            "nextToken",
        ),
        (
            "a filter field outside the FilterFieldName enum".to_string(),
            json!({"filters": [{"name": "ruleId", "values": ["cloudtrail-logging-disabled"]}]}),
            "",
        ),
        (
            "a filter condition outside the DetectionRuleFilterCondition enum".to_string(),
            json!({"filters": [{"name": "severity", "values": ["HIGH"], "condition": "STARTS_WITH"}]}),
            "",
        ),
        (
            "a filter carrying no values: DetectionRuleFilterValues is len 1..50"
                .to_string(),
            json!({"filters": [{"name": "severity", "values": []}]}),
            "",
        ),
        (
            "a filter with no values member at all: Values is required".to_string(),
            json!({"filters": [{"name": "severity"}]}),
            "",
        ),
        (
            "a maxResults of 0: one below the published minimum of 1".to_string(),
            json!({"maxResults": 0}),
            "",
        ),
        (
            "a maxResults of 101: one above the published maximum of 100".to_string(),
            json!({"maxResults": MAX_RESULTS_MAX + 1}),
            "",
        ),
        (
            "a negative maxResults".to_string(),
            json!({"maxResults": -5}),
            "",
        ),
    ];

    for (case, body, member) in rows {
        let (status, response) = list(&router, body).await;
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
            // Every row that reaches the handler here carries a page token, and a
            // token this service did not issue is refused with the ONE sentence the
            // whole pagination surface answers, which names no member at all.
            assert_eq!(
                message,
                page_token_refusal(),
                "case {case:?}: expected the page-token refusal, got {message:?}"
            );
        }
        for echoed in ["not-a-token", "9999", "STARTS_WITH", "ruleId"] {
            assert!(
                !message.contains(echoed),
                "case {case:?}: the refusal must not echo {echoed:?}, got {message:?}"
            );
        }
        // The refusal leaves the catalogue exactly as it was.
        let after = catalogue(&router).await;
        assert_eq!(
            after, rules,
            "case {case:?}: a refusal must not change the catalogue"
        );
    }

    // The token the service DOES issue is accepted on the same list, so the refusals
    // above are about tokens it never handed out and not about paging itself.
    let (first, token) = page(&router, json!({"maxResults": 1}), "the first page").await;
    let token = token.expect("the first page of a multi-rule catalogue carries a nextToken");
    let (second, _) = page(
        &router,
        json!({"maxResults": 1, "nextToken": token}),
        "the issued token",
    )
    .await;
    assert_eq!(
        ids_of(&second),
        ids_of(&rules[1..2]),
        "case \"the issued token\": expected the second rule, got {:?} after {:?}",
        ids_of(&second),
        ids_of(&first)
    );
}

#[tokio::test]
async fn a_rule_the_catalogue_does_not_publish_is_refused_with_the_modelled_404() {
    let router = new_router();
    let rules = catalogue(&router).await;

    // Each row: a well-formed `RuleId` the catalogue does not publish. They span the
    // forms the pattern admits — a single segment, several segments, digits, and both
    // published length bounds.
    let absent = [
        ABSENT_RULE_ID.to_string(),
        "x".to_string(),
        "0".to_string(),
        "a1-b2-c3".to_string(),
        "r".repeat(100),
        // The published ids, upper-cased, are different strings: a rule id is an
        // identifier, so a near miss is not a rule.
        text(&rules[0], "ruleId").replace('-', "0"),
    ];

    for rule_id in absent {
        let case = format!("get the unpublished rule {rule_id:?}");
        let (status, response) = call(&router, "GET", &format!("{RULES_PATH}/{rule_id}"), None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} {response:?}"
        );
        let class = error_code(&response);
        assert_eq!(
            class, "ResourceNotFoundException",
            "case {case:?}: expected the modelled ResourceNotFoundException, got {class:?} {response:?}"
        );
        let message = response["message"].as_str().unwrap_or_default();
        // The rule id is the ONLY thing that says which rule was missing, and it came
        // from the request, so GuardDuty echoes it after its fixed prefix.
        assert_eq!(
            message,
            format!("Rule not found: {rule_id}"),
            "case {case:?}: expected the fixed not-found text carrying the rule id, got {message:?}"
        );
    }

    // A value that cannot be a `RuleId` at all is refused by the modelled constraint
    // on the member, before the catalogue is consulted: a 400, not a not-found.
    for malformed in ["Not_A_Rule", "has%20space", "-leading", "trailing-", "UPPER"] {
        let case = format!("get the malformed rule id {malformed:?}");
        let (status, response) =
            call(&router, "GET", &format!("{RULES_PATH}/{malformed}"), None).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the modelled constraint to refuse it with 400, got {status} {response:?}"
        );
    }
}
