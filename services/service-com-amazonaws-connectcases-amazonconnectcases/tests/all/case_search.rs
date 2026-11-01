// SPDX-License-Identifier: CC0-1.0

//! End-to-end restJson1 tests for `SearchCases`, driven through the real
//! `MultiServiceRouter` so every assertion goes over the wire the SDK speaks.
//!
//! The rule under test: a search over a Cases domain is COMPUTED from the cases
//! that domain currently holds — it reports every stored case whose field values
//! and tags satisfy the request's `filter` and `searchTerm`, in the order the
//! request's `sorts` ask for, projecting exactly the requested fields, with
//! `totalCount` counting every match and `nextToken` carrying the rest — so a
//! case that was never created, or has since been deleted, is reported by no
//! search.
//!
//! Each suite below is a TABLE over that rule rather than one recorded request:
//! the rows differ along the comparison (`equalTo`, `contains`, the four ordered
//! ones), the combinator (`not`, `andAll`, `orAll`), the value's union variant
//! and hence the field's type, the sort direction, the page size and the count.

use std::collections::HashSet;

use serde_json::{json, Value};
use service_com_amazonaws_connectcases_amazonconnectcases::state::{
    DEFAULT_SEARCH_CASES_PAGE_SIZE, MAX_PAGE_SIZE,
};

// The request plumbing and the lifecycle helpers live in `tests/all/harness.rs`,
// shared with this crate's other suites.
use crate::harness::{
    domain, field_of_type, fields_of_every_type, get_case_fields, string_at, template, Emulator,
};

/// A user ARN written into the `User` field of the `with-a-user-arn` case; it is
/// text, so the quick search reaches it exactly as it reaches a `Text` field.
const AGENT_ARN: &str = "arn:aws:connect:us-east-1:000000000000:instance/i-1/agent/a-1";

/// The fixture every filter, sort and projection row below is evaluated against:
/// one domain, one field per modelled `FieldType`, one template that requires
/// nothing (so each case may write any subset of the schema), and four cases
/// whose values differ in every dimension the rule generalises over.
struct Fixture {
    emulator: Emulator,
    domain_id: String,
    text: String,
    number: String,
    boolean: String,
    datetime: String,
    user: String,
    /// `(label, caseId)` in creation order.
    cases: Vec<(&'static str, String)>,
}

impl Fixture {
    async fn new() -> Self {
        let emulator = Emulator::new();
        let domain_id = domain(&emulator, "search-domain").await;
        let fields = fields_of_every_type(&emulator, &domain_id).await;
        let text = field_of_type(&fields, "Text");
        let number = field_of_type(&fields, "Number");
        let boolean = field_of_type(&fields, "Boolean");
        let datetime = field_of_type(&fields, "DateTime");
        let user = field_of_type(&fields, "User");
        let template_id = template(&emulator, &domain_id, "search-template", &[]).await;

        // Four cases, created in this order. The first three carry a value for
        // every filtered field; the fourth writes the union's `emptyValue` for
        // the text field and nothing at all for the rest, which is the "no
        // value" row every comparison has to account for.
        let written: Vec<(&'static str, Value, Value)> = vec![
            (
                "printer-seattle",
                json!([
                    { "id": text, "value": { "stringValue": "Printer jam in Seattle" } },
                    { "id": number, "value": { "doubleValue": 10.0 } },
                    { "id": boolean, "value": { "booleanValue": true } },
                    { "id": datetime, "value": { "stringValue": "2026-01-01T00:00:00Z" } },
                    { "id": user, "value": { "userArnValue": AGENT_ARN } },
                ]),
                json!({ "team": "north", "tier": "gold" }),
            ),
            (
                "printer-boston",
                json!([
                    { "id": text, "value": { "stringValue": "printer JAM in Boston" } },
                    { "id": number, "value": { "doubleValue": 20.0 } },
                    { "id": boolean, "value": { "booleanValue": false } },
                    { "id": datetime, "value": { "stringValue": "2026-06-01T00:00:00Z" } },
                ]),
                json!({ "team": "south" }),
            ),
            (
                "network-outage",
                json!([
                    { "id": text, "value": { "stringValue": "Network outage" } },
                    { "id": number, "value": { "doubleValue": 30.0 } },
                    { "id": boolean, "value": { "booleanValue": true } },
                    { "id": datetime, "value": { "stringValue": "2026-12-01T00:00:00Z" } },
                ]),
                json!({}),
            ),
            (
                "no-values",
                json!([{ "id": text, "value": { "emptyValue": {} } }]),
                json!({}),
            ),
        ];

        let mut cases = Vec::new();
        for (label, fields, tags) in written {
            let case_id = create_case(
                &emulator,
                label,
                &domain_id,
                &template_id,
                fields,
                Some(tags),
            )
            .await;
            cases.push((label, case_id));
        }

        Self {
            emulator,
            domain_id,
            text,
            number,
            boolean,
            datetime,
            user,
            cases,
        }
    }

    /// Run one search and hand back its parsed response.
    async fn search(&self, case: &str, body: Value) -> Value {
        self.emulator
            .ok(
                case,
                "POST",
                &format!("/domains/{}/cases-search", self.domain_id),
                body,
            )
            .await
    }

    /// The labels of the cases a response reports, in the order it reports them.
    fn labels(&self, case: &str, response: &Value) -> Vec<String> {
        case_ids(case, response)
            .iter()
            .map(|case_id| self.label_of(case, case_id))
            .collect()
    }

    fn label_of(&self, case: &str, case_id: &str) -> String {
        self.cases
            .iter()
            .find(|(_, id)| id == case_id)
            .map(|(label, _)| (*label).to_string())
            .unwrap_or_else(|| panic!("case {case:?}: {case_id} is not one of {:?}", self.cases))
    }

    fn case_id(&self, label: &str) -> String {
        self.cases
            .iter()
            .find(|(candidate, _)| *candidate == label)
            .map(|(_, id)| id.clone())
            .unwrap_or_else(|| panic!("no case labelled {label:?} in {:?}", self.cases))
    }
}

/// Create one case and return its id.
async fn create_case(
    emulator: &Emulator,
    case: &str,
    domain_id: &str,
    template_id: &str,
    fields: Value,
    tags: Option<Value>,
) -> String {
    let mut body = json!({ "templateId": template_id, "fields": fields });
    if let Some(tags) = tags {
        body["tags"] = tags;
    }
    let created = emulator
        .ok(case, "POST", &format!("/domains/{domain_id}/cases"), body)
        .await;
    string_at(&created, "caseId", case)
}

/// The `caseId`s a `SearchCases` response reports, in order.
fn case_ids(case: &str, response: &Value) -> Vec<String> {
    response
        .get("cases")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: no cases list in {response:?}"))
        .iter()
        .map(|item| string_at(item, "caseId", case))
        .collect()
}

/// The `totalCount` a `SearchCases` response reports.
fn total_count(case: &str, response: &Value) -> i64 {
    response
        .get("totalCount")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("case {case:?}: no totalCount in {response:?}"))
}

#[tokio::test]
async fn a_filter_narrows_a_search_to_the_stored_cases_it_admits() {
    let fixture = Fixture::new().await;
    let (text, number) = (&fixture.text, &fixture.number);
    let (boolean, datetime) = (&fixture.boolean, &fixture.datetime);

    // One row per comparison the `FieldFilter` union declares
    // (`smithy_query(query="shape", shape="FieldFilter")`: equalTo, contains,
    // greaterThan, greaterThanOrEqualTo, lessThan, lessThanOrEqualTo) and per
    // combinator `CaseFilter` declares (field, tag, not, andAll, orAll), over
    // every union variant the stored values use. With no `sorts`, matches come
    // back in creation order.
    let rows: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "equalTo on a Text value",
            json!({ "field": { "equalTo": { "id": text, "value": { "stringValue": "Printer jam in Seattle" } } } }),
            vec!["printer-seattle"],
        ),
        (
            "equalTo is exact, not a prefix",
            json!({ "field": { "equalTo": { "id": text, "value": { "stringValue": "Printer jam" } } } }),
            vec![],
        ),
        (
            "equalTo on a Number value",
            json!({ "field": { "equalTo": { "id": number, "value": { "doubleValue": 20.0 } } } }),
            vec!["printer-boston"],
        ),
        (
            "equalTo on a Boolean value, true",
            json!({ "field": { "equalTo": { "id": boolean, "value": { "booleanValue": true } } } }),
            vec!["printer-seattle", "network-outage"],
        ),
        (
            "equalTo on a Boolean value, false",
            json!({ "field": { "equalTo": { "id": boolean, "value": { "booleanValue": false } } } }),
            vec!["printer-boston"],
        ),
        (
            "equalTo on a DateTime value",
            json!({ "field": { "equalTo": { "id": datetime, "value": { "stringValue": "2026-12-01T00:00:00Z" } } } }),
            vec!["network-outage"],
        ),
        (
            "equalTo on a User value",
            json!({ "field": { "equalTo": { "id": fixture.user, "value": { "userArnValue": AGENT_ARN } } } }),
            vec!["printer-seattle"],
        ),
        (
            "equalTo emptyValue finds the cases holding no value for the field",
            json!({ "field": { "equalTo": { "id": text, "value": { "emptyValue": {} } } } }),
            vec!["no-values"],
        ),
        (
            "equalTo emptyValue on a field that was never written",
            json!({ "field": { "equalTo": { "id": number, "value": { "emptyValue": {} } } } }),
            vec!["no-values"],
        ),
        (
            "contains ignores case",
            json!({ "field": { "contains": { "id": text, "value": { "stringValue": "printer jam" } } } }),
            vec!["printer-seattle", "printer-boston"],
        ),
        (
            "contains matches inside the value",
            json!({ "field": { "contains": { "id": text, "value": { "stringValue": "Boston" } } } }),
            vec!["printer-boston"],
        ),
        (
            "contains a phrase no value carries",
            json!({ "field": { "contains": { "id": text, "value": { "stringValue": "fax" } } } }),
            vec![],
        ),
        (
            "greaterThan on a Number value",
            json!({ "field": { "greaterThan": { "id": number, "value": { "doubleValue": 10.0 } } } }),
            vec!["printer-boston", "network-outage"],
        ),
        (
            "greaterThanOrEqualTo on a Number value",
            json!({ "field": { "greaterThanOrEqualTo": { "id": number, "value": { "doubleValue": 20.0 } } } }),
            vec!["printer-boston", "network-outage"],
        ),
        (
            "lessThan on a Number value",
            json!({ "field": { "lessThan": { "id": number, "value": { "doubleValue": 20.0 } } } }),
            vec!["printer-seattle"],
        ),
        (
            "lessThanOrEqualTo on a Number value",
            json!({ "field": { "lessThanOrEqualTo": { "id": number, "value": { "doubleValue": 20.0 } } } }),
            vec!["printer-seattle", "printer-boston"],
        ),
        (
            "greaterThan on a DateTime value orders chronologically",
            json!({ "field": { "greaterThan": { "id": datetime, "value": { "stringValue": "2026-01-01T00:00:00Z" } } } }),
            vec!["printer-boston", "network-outage"],
        ),
        (
            "lessThanOrEqualTo on a DateTime value orders chronologically",
            json!({ "field": { "lessThanOrEqualTo": { "id": datetime, "value": { "stringValue": "2026-06-01T00:00:00Z" } } } }),
            vec!["printer-seattle", "printer-boston"],
        ),
        (
            "an ordered comparison admits no case holding no value",
            json!({ "field": { "greaterThan": { "id": number, "value": { "doubleValue": -1.0 } } } }),
            vec!["printer-seattle", "printer-boston", "network-outage"],
        ),
        (
            "not inverts the clause",
            json!({ "not": { "field": { "equalTo": { "id": number, "value": { "doubleValue": 20.0 } } } } }),
            vec!["printer-seattle", "network-outage", "no-values"],
        ),
        (
            "andAll requires every clause",
            json!({ "andAll": [
                { "field": { "greaterThanOrEqualTo": { "id": number, "value": { "doubleValue": 20.0 } } } },
                { "field": { "equalTo": { "id": boolean, "value": { "booleanValue": false } } } },
            ] }),
            vec!["printer-boston"],
        ),
        (
            "orAll requires one clause",
            json!({ "orAll": [
                { "field": { "equalTo": { "id": number, "value": { "doubleValue": 10.0 } } } },
                { "field": { "equalTo": { "id": number, "value": { "doubleValue": 30.0 } } } },
            ] }),
            vec!["printer-seattle", "network-outage"],
        ),
        (
            "andAll over no clauses narrows nothing",
            json!({ "andAll": [] }),
            vec![
                "printer-seattle",
                "printer-boston",
                "network-outage",
                "no-values",
            ],
        ),
        (
            "orAll over no clauses admits nothing",
            json!({ "orAll": [] }),
            vec![],
        ),
        (
            "a combinator nests",
            json!({ "andAll": [
                { "not": { "field": { "equalTo": { "id": boolean, "value": { "booleanValue": true } } } } },
                { "field": { "greaterThan": { "id": number, "value": { "doubleValue": 10.0 } } } },
            ] }),
            vec!["printer-boston"],
        ),
        (
            "a tag filter naming a key and a value",
            json!({ "tag": { "equalTo": { "key": "team", "value": "north" } } }),
            vec!["printer-seattle"],
        ),
        (
            "a tag filter naming only a key",
            json!({ "tag": { "equalTo": { "key": "team" } } }),
            vec!["printer-seattle", "printer-boston"],
        ),
        (
            "a tag filter naming only a value",
            json!({ "tag": { "equalTo": { "value": "gold" } } }),
            vec!["printer-seattle"],
        ),
        (
            "a tag filter's key and value must match the same tag",
            json!({ "tag": { "equalTo": { "key": "team", "value": "gold" } } }),
            vec![],
        ),
    ];

    for (case, filter, expected) in rows {
        let response = fixture.search(case, json!({ "filter": filter })).await;
        let actual = fixture.labels(case, &response);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the cases {expected:?}, got {actual:?} from {response:?}"
        );
        assert_eq!(
            total_count(case, &response),
            expected.len() as i64,
            "case {case:?}: totalCount must count every match, got {response:?}"
        );
    }
}

#[tokio::test]
async fn a_search_term_narrows_to_the_cases_whose_text_carries_it() {
    let fixture = Fixture::new().await;

    // `searchTerm` is a quick search over the case's text, so the rows differ in
    // the word, its case, where in the value it falls, and which text-valued
    // field holds it.
    let rows: Vec<(&str, &str, Vec<&str>)> = vec![
        (
            "a word both printer cases carry, in neither's case",
            "jam",
            vec!["printer-seattle", "printer-boston"],
        ),
        ("a word only one case carries", "seattle", vec!["printer-seattle"]),
        (
            "a word carried by a User field's ARN",
            "agent/a-1",
            vec!["printer-seattle"],
        ),
        ("a phrase", "network outage", vec!["network-outage"]),
        ("a word no case carries", "thermostat", vec![]),
        (
            "an empty term narrows nothing",
            "",
            vec![
                "printer-seattle",
                "printer-boston",
                "network-outage",
                "no-values",
            ],
        ),
    ];

    for (case, term, expected) in rows {
        let response = fixture.search(case, json!({ "searchTerm": term })).await;
        let actual = fixture.labels(case, &response);
        assert_eq!(
            actual, expected,
            "case {case:?}: searching for {term:?} expected {expected:?}, got {actual:?} \
             from {response:?}"
        );
    }

    // A term and a filter narrow together.
    let case = "a term and a filter";
    let response = fixture
        .search(
            case,
            json!({
                "searchTerm": "printer",
                "filter": { "field": { "equalTo": { "id": fixture.boolean, "value": { "booleanValue": false } } } },
            }),
        )
        .await;
    let actual = fixture.labels(case, &response);
    assert_eq!(
        actual,
        vec!["printer-boston".to_string()],
        "case {case:?}: both criteria must hold, got {actual:?} from {response:?}"
    );
}

#[tokio::test]
async fn sorts_order_the_results_and_cases_holding_no_value_come_last() {
    let fixture = Fixture::new().await;
    let (text, number, boolean) = (&fixture.text, &fixture.number, &fixture.boolean);

    // One row per direction `Order` declares (`Asc | Desc`), over two value
    // variants, plus a two-sort row (the model's `sorts` cap) and the
    // no-sort default.
    let rows: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "no sorts answers in creation order",
            json!([]),
            vec![
                "printer-seattle",
                "printer-boston",
                "network-outage",
                "no-values",
            ],
        ),
        (
            "a Number field ascending",
            json!([{ "fieldId": number, "sortOrder": "Asc" }]),
            vec![
                "printer-seattle",
                "printer-boston",
                "network-outage",
                "no-values",
            ],
        ),
        (
            "a Number field descending still puts the valueless case last",
            json!([{ "fieldId": number, "sortOrder": "Desc" }]),
            vec![
                "network-outage",
                "printer-boston",
                "printer-seattle",
                "no-values",
            ],
        ),
        (
            "a Text field ascending",
            json!([{ "fieldId": text, "sortOrder": "Asc" }]),
            vec![
                "network-outage",
                "printer-seattle",
                "printer-boston",
                "no-values",
            ],
        ),
        (
            "a Text field descending",
            json!([{ "fieldId": text, "sortOrder": "Desc" }]),
            vec![
                "printer-boston",
                "printer-seattle",
                "network-outage",
                "no-values",
            ],
        ),
        (
            "the second sort breaks the first's ties",
            json!([
                { "fieldId": boolean, "sortOrder": "Asc" },
                { "fieldId": number, "sortOrder": "Desc" },
            ]),
            vec![
                "printer-boston",
                "network-outage",
                "printer-seattle",
                "no-values",
            ],
        ),
    ];

    for (case, sorts, expected) in rows {
        let response = fixture.search(case, json!({ "sorts": sorts })).await;
        let actual = fixture.labels(case, &response);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the order {expected:?}, got {actual:?} from {response:?}"
        );
    }
}

#[tokio::test]
async fn a_search_projects_exactly_the_requested_fields_as_get_case_reports_them() {
    let fixture = Fixture::new().await;
    let (text, number) = (fixture.text.clone(), fixture.number.clone());

    // The rows differ in how many fields are projected and in what order, which
    // is what `fields` controls: "The list of field identifiers to be returned
    // as part of the response".
    let rows: Vec<(&str, Vec<String>)> = vec![
        ("no fields projects none", vec![]),
        ("one field", vec![text.clone()]),
        ("two fields", vec![text.clone(), number.clone()]),
        (
            "two fields, the other way round",
            vec![number.clone(), text.clone()],
        ),
    ];

    for (case, requested) in rows {
        let projection: Vec<Value> = requested.iter().map(|id| json!({ "id": id })).collect();
        let response = fixture
            .search(case, json!({ "fields": projection }))
            .await;
        let items = response
            .get("cases")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: no cases list in {response:?}"))
            .clone();
        assert_eq!(
            items.len(),
            fixture.cases.len(),
            "case {case:?}: every stored case matches an unfiltered search, got {response:?}"
        );
        for item in &items {
            let case_id = string_at(item, "caseId", case);
            let label = fixture.label_of(case, &case_id);
            let row = format!("{case} [{label}]");
            let fields = item
                .get("fields")
                .and_then(Value::as_array)
                .unwrap_or_else(|| panic!("case {row:?}: no fields list in {item:?}"))
                .clone();
            let ids: Vec<String> = fields
                .iter()
                .map(|field| string_at(field, "id", &row))
                .collect();
            // A projection reports only the requested ids the case HOLDS a
            // value for, in the requested order: the `no-values` case was
            // written with the text field alone, so a projection of any other
            // field reports nothing for it.
            let expected: Vec<String> = if label == "no-values" {
                requested.iter().filter(|id| **id == text).cloned().collect()
            } else {
                requested.clone()
            };
            assert_eq!(
                ids, expected,
                "case {row:?}: expected exactly the requested ids the case holds, in order, \
                 got {ids:?}"
            );
            // The projected values are the SAME values `GetCase` reports for
            // those ids, which is the round trip through stored state.
            // `GetCase`'s own `fields` is `@length(1..)`, so the empty
            // projection has nothing to compare against there.
            if !requested.is_empty() {
                let through_get_case = get_case_fields(
                    &fixture.emulator,
                    &row,
                    &fixture.domain_id,
                    &case_id,
                    &requested,
                )
                .await;
                assert_eq!(
                    fields, through_get_case,
                    "case {row:?}: a search must project what GetCase reports, got {fields:?}"
                );
            }
        }
    }

    // Every case carries the id and templateId of the stored case, and its tags
    // only when it has any.
    let case = "identity and tags come from the stored case";
    let response = fixture.search(case, json!({})).await;
    for item in response
        .get("cases")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: no cases list in {response:?}"))
    {
        let case_id = string_at(item, "caseId", case);
        let label = fixture.label_of(case, &case_id);
        assert!(
            !string_at(item, "templateId", case).is_empty(),
            "case {case:?} [{label}]: the stored templateId must be reported, got {item:?}"
        );
        let expected_tags = match label.as_str() {
            "printer-seattle" => Some(json!({ "team": "north", "tier": "gold" })),
            "printer-boston" => Some(json!({ "team": "south" })),
            _ => None,
        };
        assert_eq!(
            item.get("tags").cloned(),
            expected_tags,
            "case {case:?} [{label}]: tags must be the stored tags, absent when there are none, \
             got {item:?}"
        );
    }
}

#[tokio::test]
async fn a_search_pages_through_every_match_exactly_once() {
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "paging-domain").await;
    let fields = fields_of_every_type(&emulator, &domain_id).await;
    let number = field_of_type(&fields, "Number");
    let template_id = template(&emulator, &domain_id, "paging-template", &[]).await;

    // One more than the documented default page size, so the default is
    // observable, and an odd total so the last page is partial for every page
    // size below.
    let total = DEFAULT_SEARCH_CASES_PAGE_SIZE as usize + 8;
    let mut created = Vec::new();
    for index in 0..total {
        let case = format!("paging case {index}");
        created.push(
            create_case(
                &emulator,
                &case,
                &domain_id,
                &template_id,
                json!([{ "id": number, "value": { "doubleValue": index as f64 } }]),
                None,
            )
            .await,
        );
    }

    // Rows: the documented default (no `maxResults`), a page size that divides
    // nothing evenly, the smallest the model admits, and the largest.
    let page_sizes: Vec<(&str, Option<i32>)> = vec![
        ("no maxResults takes the documented default", None),
        ("a page size in the middle of the range", Some(10)),
        ("the smallest page size the model admits", Some(1)),
        ("the largest page size the model admits", Some(MAX_PAGE_SIZE)),
    ];

    for (case, max_results) in page_sizes {
        let expected_page = max_results.unwrap_or(DEFAULT_SEARCH_CASES_PAGE_SIZE) as usize;
        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut body = json!({});
            if let Some(max_results) = max_results {
                body["maxResults"] = json!(max_results);
            }
            if let Some(token) = &next_token {
                body["nextToken"] = json!(token);
            }
            let response = emulator
                .ok(
                    case,
                    "POST",
                    &format!("/domains/{domain_id}/cases-search"),
                    body,
                )
                .await;
            pages += 1;
            assert!(
                pages <= total + 1,
                "case {case:?}: paging did not terminate after {pages} pages"
            );
            assert_eq!(
                total_count(case, &response),
                total as i64,
                "case {case:?}: totalCount counts every match on every page, got {response:?}"
            );
            let page = case_ids(case, &response);
            assert!(
                page.len() <= expected_page,
                "case {case:?}: a page holds at most {expected_page} entries, got {}",
                page.len()
            );
            seen.extend(page);
            next_token = response
                .get("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if next_token.is_none() {
                break;
            }
            assert_eq!(
                seen.len() % expected_page,
                0,
                "case {case:?}: a page that is not full is the last one, got {seen:?}"
            );
        }
        let distinct: HashSet<&String> = seen.iter().collect();
        assert_eq!(
            distinct.len(),
            total,
            "case {case:?}: paging must report every match exactly once, got {} entries \
             ({} distinct)",
            seen.len(),
            distinct.len()
        );
        assert_eq!(
            seen, created,
            "case {case:?}: paging must follow the search's own order"
        );
        let expected_pages = total.div_ceil(expected_page);
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} pages of {expected_page}, took {pages}"
        );
    }

    // A token this listing issued cannot be used to resume a DIFFERENT listing:
    // it names a position in an ordering the other listing does not have.
    let first = emulator
        .ok(
            "token reuse",
            "POST",
            &format!("/domains/{domain_id}/cases-search"),
            json!({ "maxResults": 2 }),
        )
        .await;
    let token = first
        .get("nextToken")
        .and_then(Value::as_str)
        .expect("the first page of a longer listing carries a nextToken")
        .to_string();
    let (status, code) = emulator
        .failure(
            "token reuse",
            "POST",
            &format!("/domains/{domain_id}/cases-search"),
            Some(json!({
                "maxResults": 2,
                "nextToken": token,
                "sorts": [{ "fieldId": number, "sortOrder": "Desc" }],
            })),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ValidationException"),
        "case \"token reuse\": a token from another ordering must be refused"
    );
}

#[tokio::test]
async fn a_search_reports_no_case_that_was_deleted() {
    let fixture = Fixture::new().await;
    let doomed = fixture.case_id("printer-boston");

    let case = "DeleteCase then SearchCases";
    let (status, _) = fixture
        .emulator
        .send(
            "DELETE",
            &format!("/domains/{}/cases/{}", fixture.domain_id, doomed),
            None,
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DeleteCase must accept a stored case"
    );

    // Neither an unfiltered search nor one whose filter the deleted case
    // satisfied reports it any more, and `totalCount` drops with it.
    let rows: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "every case",
            json!({}),
            vec!["printer-seattle", "network-outage", "no-values"],
        ),
        (
            "a filter the deleted case satisfied",
            json!({ "filter": { "field": { "contains": { "id": fixture.text, "value": { "stringValue": "printer" } } } } }),
            vec!["printer-seattle"],
        ),
        (
            "a term the deleted case carried",
            json!({ "searchTerm": "boston" }),
            vec![],
        ),
    ];
    for (row, body, expected) in rows {
        let response = fixture.search(row, body).await;
        let actual = fixture.labels(row, &response);
        assert_eq!(
            actual, expected,
            "case {row:?}: a deleted case is reported by no search, got {actual:?} \
             from {response:?}"
        );
        assert_eq!(
            total_count(row, &response),
            expected.len() as i64,
            "case {row:?}: totalCount must drop with the deleted case, got {response:?}"
        );
    }

    // The same case is gone from the reader the search agrees with.
    let (status, code) = fixture
        .emulator
        .failure(
            "GetCase after DeleteCase",
            "POST",
            &format!("/domains/{}/cases/{}", fixture.domain_id, doomed),
            Some(json!({ "fields": [{ "id": fixture.number }] })),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (404, "ResourceNotFoundException"),
        "case \"GetCase after DeleteCase\": the deleted case must be gone from every read"
    );
}

#[tokio::test]
async fn a_search_refuses_a_request_it_cannot_answer() {
    let fixture = Fixture::new().await;
    let known = fixture.text.clone();

    // Every reference a search carries is resolved against the domain's schema,
    // and every paging and ordering argument against the model's own range, so
    // a request that names something the store does not hold is refused rather
    // than answered with an empty page.
    let rows: Vec<(&str, Value, u16, &str)> = vec![
        (
            "fields names a field the domain does not define",
            json!({ "fields": [{ "id": "field-that-was-never-created" }] }),
            400,
            "ValidationException",
        ),
        (
            "sorts names a field the domain does not define",
            json!({ "sorts": [{ "fieldId": "field-that-was-never-created", "sortOrder": "Asc" }] }),
            400,
            "ValidationException",
        ),
        (
            "a filter names a field the domain does not define",
            json!({ "filter": { "field": { "equalTo": { "id": "field-that-was-never-created", "value": { "stringValue": "x" } } } } }),
            400,
            "ValidationException",
        ),
        (
            "a nested filter names a field the domain does not define",
            json!({ "filter": { "not": { "andAll": [
                { "field": { "equalTo": { "id": known, "value": { "stringValue": "x" } } } },
                { "field": { "equalTo": { "id": "field-that-was-never-created", "value": { "stringValue": "x" } } } },
            ] } } }),
            400,
            "ValidationException",
        ),
        (
            "sortOrder is not an Order",
            json!({ "sorts": [{ "fieldId": known, "sortOrder": "Ascending" }] }),
            400,
            "ValidationException",
        ),
        (
            "maxResults below the modelled range",
            json!({ "maxResults": 0 }),
            400,
            "ValidationException",
        ),
        (
            "maxResults above the modelled range",
            json!({ "maxResults": MAX_PAGE_SIZE + 1 }),
            400,
            "ValidationException",
        ),
        (
            "a nextToken this service never issued",
            json!({ "nextToken": "not-a-token-this-service-issued" }),
            400,
            "ValidationException",
        ),
    ];

    for (case, body, expected_status, expected_code) in rows {
        let (status, code) = fixture
            .emulator
            .failure(
                case,
                "POST",
                &format!("/domains/{}/cases-search", fixture.domain_id),
                Some(body),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (expected_status, expected_code),
            "case {case:?}: expected HTTP {expected_status} {expected_code}, got {status} {code}"
        );
    }

    // A search is scoped to a domain, so one that names no stored domain is a
    // 404 and not an empty page.
    let (status, code) = fixture
        .emulator
        .failure(
            "an unknown domain",
            "POST",
            "/domains/domain-that-was-never-created/cases-search",
            Some(json!({})),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (404, "ResourceNotFoundException"),
        "case \"an unknown domain\": a search is scoped to a stored domain"
    );
}
