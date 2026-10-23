// SPDX-License-Identifier: CC0-1.0

//! `ListWorkloadEstimateUsage`, driven end-to-end over this service's awsJson1_0 wire
//! (`POST /`, `X-Amz-Target: AWSBCMPricingCalculator.<Operation>`) through the same
//! `MultiServiceRouter` the production front door uses.
//!
//! Each suite asserts a RULE over a table of inputs rather than one recorded request:
//! the rows differ along what the rule generalises over (the number of stored usage
//! lines, the page size, the filter name, its match option and its values, which
//! selection a page token was measured against, and whether the parent estimate exists).
//!
//! The wire harness is the one `bill_scenario_lifecycle` defines; a second copy could
//! drift from the front door that one exercises.

use serde_json::{json, Value};

use crate::bill_scenario_lifecycle::{error_code, is_resource_id, Emulator};
use crate::workload_estimate_lifecycle::create_estimate;

/// `WorkloadEstimateUsageMaxResults` is `@range 1..300` (smithy_query:
/// `shape="WorkloadEstimateUsageMaxResults"`). Declared HERE rather than read from the
/// implementation, so a changed bound fails these tests instead of moving with them.
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 300;

/// A well-formed `ResourceId` (`len 36..36`, lowercase-hex v4 UUID form) that no stored
/// resource carries, so a request naming it is refused for what it names rather than for
/// its shape.
const UNKNOWN_RESOURCE_ID: &str = "0f9e8d7c-6b5a-4938-2716-05f4e3d2c1b0";

/// The `historicalUsage` half of a seeded line carries its OWN service code, usage type,
/// operation and account id, each different from every line's own, so a `HISTORICAL_*`
/// filter that selects is provably reading this half rather than the line's.
const HISTORICAL_SERVICE_CODE: &str = "AWSDataTransfer";
const HISTORICAL_USAGE_TYPE: &str = "DataTransfer-Out-Bytes";
const HISTORICAL_OPERATION: &str = "TransferOut";
const HISTORICAL_ACCOUNT_ID: &str = "999999999999";
/// The one location any seeded line reports anywhere: on a `historicalUsage`, which is
/// the caller's own input, unlike a line's `location` (a price-list value this emulator
/// does not carry).
const HISTORICAL_LOCATION: &str = "us-east-1";

/// What a seeded line's `historicalUsage` is.
#[derive(Debug, Clone, Copy)]
enum Historical {
    /// The entry sends no `historicalUsage` at all.
    Absent,
    /// The entry sends one, without the optional `location`.
    WithoutLocation,
    /// The entry sends one carrying this `location`.
    At(&'static str),
}

/// One seeded usage line: every attribute a `ListUsageFilter` can select on, plus the
/// amount the line's `quantity` reports.
#[derive(Debug, Clone, Copy)]
struct Line {
    key: &'static str,
    service_code: &'static str,
    usage_type: &'static str,
    operation: &'static str,
    usage_account_id: &'static str,
    group: Option<&'static str>,
    historical: Historical,
    amount: f64,
}

/// The lines every suite seeds, in creation order.
///
/// They span the selectable attributes: several service codes sharing a prefix, usage
/// types sharing a prefix and differing in a substring, repeated and distinct
/// operations, three account ids, a group shared by two lines, a group no other line
/// carries, lines with no group at all, and the three `historicalUsage` shapes.
const LINES: [Line; 6] = [
    Line {
        key: "a",
        service_code: "AmazonEC2",
        usage_type: "BoxUsage:m5.xlarge",
        operation: "RunInstances",
        usage_account_id: "000000000000",
        group: Some("group-a"),
        historical: Historical::At(HISTORICAL_LOCATION),
        amount: 1.0,
    },
    Line {
        key: "b",
        service_code: "AmazonEC2",
        usage_type: "BoxUsage:m5.large",
        operation: "RunInstances",
        usage_account_id: "000000000000",
        group: Some("group-b"),
        historical: Historical::Absent,
        amount: 2.5,
    },
    Line {
        key: "c",
        service_code: "AmazonS3",
        usage_type: "TimedStorage-ByteHrs",
        operation: "PutObject",
        usage_account_id: "111111111111",
        group: None,
        historical: Historical::WithoutLocation,
        amount: 0.0,
    },
    Line {
        key: "d",
        service_code: "AWSLambda",
        usage_type: "Request",
        operation: "Invoke",
        usage_account_id: "111111111111",
        group: Some("group-a"),
        historical: Historical::Absent,
        amount: 1_000_000.5,
    },
    Line {
        key: "e",
        service_code: "AmazonRDS",
        usage_type: "InstanceUsage:db.t3.micro",
        operation: "CreateDBInstance",
        usage_account_id: "222222222222",
        group: None,
        historical: Historical::Absent,
        amount: 0.25,
    },
    Line {
        key: "f",
        service_code: "AmazonEC2",
        usage_type: "BoxUsage:m5.xlarge",
        operation: "RunInstances",
        usage_account_id: "222222222222",
        group: Some("grp9"),
        historical: Historical::Absent,
        amount: 7.5,
    },
];

/// One `BatchCreateWorkloadEstimateUsageEntry` for `line`.
///
/// `HistoricalUsageEntity` requires `billInterval` and `filterExpression` (smithy_query:
/// `shape="HistoricalUsageEntity"`), and every member of `Expression` is optional, so
/// the narrowest expression is the empty one.
fn entry(line: &Line) -> Value {
    let mut entry = json!({
        "key": line.key,
        "serviceCode": line.service_code,
        "usageType": line.usage_type,
        "operation": line.operation,
        "usageAccountId": line.usage_account_id,
        "amount": line.amount,
    });
    if let Some(group) = line.group {
        entry["group"] = json!(group);
    }
    if !matches!(line.historical, Historical::Absent) {
        let mut historical = json!({
            "serviceCode": HISTORICAL_SERVICE_CODE,
            "usageType": HISTORICAL_USAGE_TYPE,
            "operation": HISTORICAL_OPERATION,
            "usageAccountId": HISTORICAL_ACCOUNT_ID,
            "billInterval": { "start": 1_760_000_000, "end": 1_762_678_400 },
            "filterExpression": {},
        });
        if let Historical::At(location) = line.historical {
            historical["location"] = json!(location);
        }
        entry["historicalUsage"] = historical;
    }
    entry
}

/// Create an estimate holding the first `count` lines of [`LINES`], and return its id
/// with the item `BatchCreateWorkloadEstimateUsage` reported for each line, in that same
/// order.
///
/// The lines are seeded in batches of three, so the order the list reports is exercised
/// ACROSS calls rather than only within one.
async fn seed(emulator: &Emulator, count: usize) -> (String, Vec<Value>) {
    let estimate = create_estimate(emulator, "usage-listing").await;
    let mut created: Vec<Value> = Vec::new();
    for chunk in LINES[..count].chunks(3) {
        let entries: Vec<Value> = chunk.iter().map(entry).collect();
        let (status, body) = emulator
            .call(
                "BatchCreateWorkloadEstimateUsage",
                json!({ "workloadEstimateId": estimate, "usage": entries }),
            )
            .await;
        assert_eq!(
            status, 200,
            "seeding the batch starting at {:?}: expected 200, got {status} with {body}",
            chunk[0].key
        );
        let items = body["items"].as_array().cloned().unwrap_or_default();
        for line in chunk {
            let item = items
                .iter()
                .find(|item| item["key"] == json!(line.key))
                .unwrap_or_else(|| panic!("seeding {}: no item for that key in {body}", line.key))
                .clone();
            created.push(item);
        }
    }
    (estimate, created)
}

/// The `items` of a list response.
fn items(body: &Value) -> Vec<Value> {
    body["items"].as_array().cloned().unwrap_or_default()
}

/// The `id` of every reported item, in the order reported.
fn ids_of(body: &Value) -> Vec<String> {
    items(body)
        .iter()
        .map(|item| item["id"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// List one estimate's usage, with whatever else `request` carries.
async fn list(emulator: &Emulator, estimate: &str, extra: Value) -> (u16, Value) {
    let mut request = json!({ "workloadEstimateId": estimate });
    for (member, value) in extra.as_object().cloned().unwrap_or_default() {
        request[member] = value;
    }
    emulator.call("ListWorkloadEstimateUsage", request).await
}

/// RULE: the list reports one item per usage line stored under the estimate it names, in
/// the order those lines were created, and every member of an item is the one the
/// operation that stored the line reported for it — so a caller cannot tell the two
/// readers apart. The members that come from the price list this emulator does not carry
/// (`location`, `cost`, `currency`, `status`, and the quantity's `unit`) are absent
/// rather than invented, and the list reports only stored state: calling it twice answers
/// the same way.
///
/// The table spans the number of stored lines, from none upwards, and (through
/// [`LINES`]) the optional members a line carries.
#[tokio::test]
async fn the_list_reports_every_stored_usage_line_as_its_creator_did() {
    for count in 0..=LINES.len() {
        let case = format!("count={count}");
        let emulator = Emulator::new();
        let (estimate, created) = seed(&emulator, count).await;

        let (status, body) = list(&emulator, &estimate, json!({})).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200, got {status} with {body}"
        );
        let listed = items(&body);
        assert_eq!(
            listed.len(),
            count,
            "case {case}: expected {count} items, got {body}"
        );
        assert!(
            body["nextToken"].is_null(),
            "case {case}: a single page must carry no nextToken, got {body}"
        );

        for (index, (item, line)) in listed.iter().zip(&LINES[..count]).enumerate() {
            let where_ = format!("case {case} index={index} ({})", line.key);
            let id = item["id"].as_str().unwrap_or_default();
            assert!(
                is_resource_id(id),
                "{where_}: expected a ResourceId, got {id:?}"
            );
            for field in [
                "id",
                "serviceCode",
                "usageType",
                "operation",
                "usageAccountId",
                "group",
                "quantity",
                "location",
                "cost",
                "currency",
                "status",
                "historicalUsage",
            ] {
                assert_eq!(
                    item[field], created[index][field],
                    "{where_}: expected the listed {field} {:?} to be the creator's {:?}",
                    item[field], created[index][field]
                );
            }
            assert_eq!(
                item["quantity"]["amount"].as_f64(),
                Some(line.amount),
                "{where_}: expected quantity.amount {}, got {item}",
                line.amount
            );
            for absent in ["location", "cost", "currency", "status"] {
                assert!(
                    item[absent].is_null(),
                    "{where_}: {absent} comes from the price list and must stay absent, \
                     got {item}"
                );
            }
            assert!(
                item["quantity"]["unit"].is_null(),
                "{where_}: the quantity's unit comes from the price list and must stay \
                 absent, got {item}"
            );
            // `WorkloadEstimateUsageItem` declares no `key` member, so the batch entry's
            // key must not reach this reader.
            assert!(
                item["key"].is_null(),
                "{where_}: the batch entry's key is not a member of this item, got {item}"
            );
        }

        let (status, again) = list(&emulator, &estimate, json!({})).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200 from the second read, got {status} with {again}"
        );
        assert_eq!(
            items(&again),
            listed,
            "case {case}: a second read of the same state answered differently: {again}"
        );
    }
}

/// RULE: `maxResults` is the page size and `nextToken` is emitted only while items
/// remain, so walking the pages reports every stored line exactly once, in creation
/// order, whatever the page size.
///
/// The table spans the page size from the published minimum to the published maximum
/// (`WorkloadEstimateUsageMaxResults` is `@range 1..300`), including sizes that do and do
/// not divide the store's size, a size larger than it, and the absent page size.
#[tokio::test]
async fn walking_the_pages_reports_every_usage_line_exactly_once() {
    let page_sizes: [Option<i64>; 7] = [
        None,
        Some(MAX_RESULTS_MIN),
        Some(2),
        Some(4),
        Some(6),
        Some(7),
        Some(MAX_RESULTS_MAX),
    ];

    for size in page_sizes {
        let case = format!("maxResults={size:?}");
        let emulator = Emulator::new();
        let (estimate, created) = seed(&emulator, LINES.len()).await;
        let expected: Vec<String> = created
            .iter()
            .map(|item| item["id"].as_str().unwrap_or_default().to_string())
            .collect();

        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        // One more iteration than the smallest page size could need, so a token that
        // never clears fails the test instead of looping forever.
        for page in 0..=LINES.len() {
            let mut extra = json!({});
            if let Some(size) = size {
                extra["maxResults"] = json!(size);
            }
            if let Some(token) = token.as_deref() {
                extra["nextToken"] = json!(token);
            }
            let (status, body) = list(&emulator, &estimate, extra).await;
            assert_eq!(
                status, 200,
                "case {case} page={page}: expected 200, got {status} with {body}"
            );
            let page_items = items(&body);
            if let Some(size) = size {
                assert!(
                    i64::try_from(page_items.len()).unwrap_or(i64::MAX) <= size,
                    "case {case} page={page}: expected at most {size} items, got {}",
                    page_items.len()
                );
            }
            seen.extend(ids_of(&body));
            token = body["nextToken"].as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
        }

        assert_eq!(
            token, None,
            "case {case}: the walk did not reach a last page without a token"
        );
        assert_eq!(
            seen, expected,
            "case {case}: expected every line exactly once in creation order, got {seen:?}"
        );
    }
}

/// The other half of the page-size bound: a value AT a published bound is accepted, and
/// one past either bound is refused as a `ValidationException` (HTTP 400).
#[tokio::test]
async fn a_page_size_past_a_published_bound_is_refused() {
    let emulator = Emulator::new();
    let (estimate, _) = seed(&emulator, LINES.len()).await;

    for size in [MAX_RESULTS_MIN, 2, MAX_RESULTS_MAX] {
        let (status, body) = list(&emulator, &estimate, json!({ "maxResults": size })).await;
        assert_eq!(
            status, 200,
            "case maxResults={size}: a value at or inside the bound was refused with \
             {status} and {body}"
        );
    }

    for size in [MAX_RESULTS_MIN - 1, MAX_RESULTS_MAX + 1] {
        let (status, body) = list(&emulator, &estimate, json!({ "maxResults": size })).await;
        assert_eq!(
            status, 400,
            "case maxResults={size}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case maxResults={size}: expected ValidationException, got {body}"
        );
    }
}

/// One filter row: what it names, how it matches, and the line keys it must select.
struct FilterCase {
    case: &'static str,
    name: &'static str,
    match_option: Option<&'static str>,
    values: Vec<&'static str>,
    expected: Vec<&'static str>,
}

/// RULE: a filter selects on the usage attribute its name denotes — the `HISTORICAL_*`
/// names reading the line's `historicalUsage` — under the `matchOption` it carries
/// (absent means an exact match), with the values of one filter OR-ed and several filters
/// AND-ed; a line carrying nothing for the named attribute is not selected.
///
/// The table spans EVERY `ListUsageFilterName` value (smithy_query:
/// `shape="ListUsageFilterName"` — `USAGE_ACCOUNT_ID`, `SERVICE_CODE`, `USAGE_TYPE`,
/// `OPERATION`, `LOCATION`, `USAGE_GROUP` and the five `HISTORICAL_*` names), every
/// `MatchOption` (`shape="MatchOption"` — `EQUALS`, `STARTS_WITH`, `CONTAINS`) and its
/// absence, one and several values, a value nothing carries, and a value carried by the
/// OTHER half of a line (its own attribute where the filter names the historical one).
#[tokio::test]
async fn a_filter_selects_on_the_usage_attribute_it_names() {
    let cases = vec![
        FilterCase {
            case: "SERVICE_CODE, exact, no matchOption",
            name: "SERVICE_CODE",
            match_option: None,
            values: vec!["AmazonS3"],
            expected: vec!["c"],
        },
        FilterCase {
            case: "SERVICE_CODE, EQUALS",
            name: "SERVICE_CODE",
            match_option: Some("EQUALS"),
            values: vec!["AmazonEC2"],
            expected: vec!["a", "b", "f"],
        },
        FilterCase {
            case: "SERVICE_CODE, EQUALS, several values are OR-ed",
            name: "SERVICE_CODE",
            match_option: Some("EQUALS"),
            values: vec!["AmazonS3", "AWSLambda"],
            expected: vec!["c", "d"],
        },
        FilterCase {
            case: "SERVICE_CODE, STARTS_WITH a shared prefix",
            name: "SERVICE_CODE",
            match_option: Some("STARTS_WITH"),
            values: vec!["Amazon"],
            expected: vec!["a", "b", "c", "e", "f"],
        },
        FilterCase {
            case: "SERVICE_CODE, CONTAINS",
            name: "SERVICE_CODE",
            match_option: Some("CONTAINS"),
            values: vec!["Lambda"],
            expected: vec!["d"],
        },
        FilterCase {
            case: "SERVICE_CODE, EQUALS, case-sensitively",
            name: "SERVICE_CODE",
            match_option: Some("EQUALS"),
            values: vec!["amazonec2"],
            expected: vec![],
        },
        FilterCase {
            case: "SERVICE_CODE, a value no line carries",
            name: "SERVICE_CODE",
            match_option: Some("EQUALS"),
            values: vec!["AmazonDynamoDB"],
            expected: vec![],
        },
        FilterCase {
            case: "USAGE_TYPE, STARTS_WITH",
            name: "USAGE_TYPE",
            match_option: Some("STARTS_WITH"),
            values: vec!["BoxUsage:m5."],
            expected: vec!["a", "b", "f"],
        },
        FilterCase {
            case: "USAGE_TYPE, CONTAINS",
            name: "USAGE_TYPE",
            match_option: Some("CONTAINS"),
            values: vec!["xlarge"],
            expected: vec!["a", "f"],
        },
        FilterCase {
            case: "USAGE_TYPE, EQUALS",
            name: "USAGE_TYPE",
            match_option: Some("EQUALS"),
            values: vec!["InstanceUsage:db.t3.micro"],
            expected: vec!["e"],
        },
        FilterCase {
            case: "OPERATION, EQUALS, repeated across lines",
            name: "OPERATION",
            match_option: Some("EQUALS"),
            values: vec!["RunInstances"],
            expected: vec!["a", "b", "f"],
        },
        FilterCase {
            case: "OPERATION, CONTAINS",
            name: "OPERATION",
            match_option: Some("CONTAINS"),
            values: vec!["Object"],
            expected: vec!["c"],
        },
        FilterCase {
            case: "USAGE_ACCOUNT_ID, EQUALS",
            name: "USAGE_ACCOUNT_ID",
            match_option: Some("EQUALS"),
            values: vec!["000000000000"],
            expected: vec!["a", "b"],
        },
        FilterCase {
            case: "USAGE_ACCOUNT_ID, two values are OR-ed",
            name: "USAGE_ACCOUNT_ID",
            match_option: Some("EQUALS"),
            values: vec!["111111111111", "222222222222"],
            expected: vec!["c", "d", "e", "f"],
        },
        FilterCase {
            case: "USAGE_ACCOUNT_ID, the historical half's account id",
            name: "USAGE_ACCOUNT_ID",
            match_option: Some("EQUALS"),
            values: vec![HISTORICAL_ACCOUNT_ID],
            expected: vec![],
        },
        FilterCase {
            case: "USAGE_GROUP, EQUALS, shared by two lines",
            name: "USAGE_GROUP",
            match_option: Some("EQUALS"),
            values: vec!["group-a"],
            expected: vec!["a", "d"],
        },
        FilterCase {
            case: "USAGE_GROUP, STARTS_WITH, skipping the lines with no group",
            name: "USAGE_GROUP",
            match_option: Some("STARTS_WITH"),
            values: vec!["group-"],
            expected: vec!["a", "b", "d"],
        },
        FilterCase {
            case: "USAGE_GROUP, a group only one line carries",
            name: "USAGE_GROUP",
            match_option: Some("EQUALS"),
            values: vec!["grp9"],
            expected: vec!["f"],
        },
        FilterCase {
            // A line's `location` comes from the price list this emulator does not
            // carry, so no line carries one and this filter selects nothing.
            case: "LOCATION, a value the price list would supply",
            name: "LOCATION",
            match_option: Some("EQUALS"),
            values: vec![HISTORICAL_LOCATION],
            expected: vec![],
        },
        FilterCase {
            case: "HISTORICAL_SERVICE_CODE, EQUALS",
            name: "HISTORICAL_SERVICE_CODE",
            match_option: Some("EQUALS"),
            values: vec![HISTORICAL_SERVICE_CODE],
            expected: vec!["a", "c"],
        },
        FilterCase {
            case: "HISTORICAL_SERVICE_CODE, a line's own service code",
            name: "HISTORICAL_SERVICE_CODE",
            match_option: Some("EQUALS"),
            values: vec!["AmazonEC2"],
            expected: vec![],
        },
        FilterCase {
            case: "HISTORICAL_USAGE_TYPE, CONTAINS",
            name: "HISTORICAL_USAGE_TYPE",
            match_option: Some("CONTAINS"),
            values: vec!["Out-Bytes"],
            expected: vec!["a", "c"],
        },
        FilterCase {
            case: "HISTORICAL_OPERATION, EQUALS",
            name: "HISTORICAL_OPERATION",
            match_option: Some("EQUALS"),
            values: vec![HISTORICAL_OPERATION],
            expected: vec!["a", "c"],
        },
        FilterCase {
            case: "HISTORICAL_USAGE_ACCOUNT_ID, STARTS_WITH",
            name: "HISTORICAL_USAGE_ACCOUNT_ID",
            match_option: Some("STARTS_WITH"),
            values: vec!["9999"],
            expected: vec!["a", "c"],
        },
        FilterCase {
            // Only the line whose `historicalUsage` sent a `location` carries one.
            case: "HISTORICAL_LOCATION, EQUALS",
            name: "HISTORICAL_LOCATION",
            match_option: Some("EQUALS"),
            values: vec![HISTORICAL_LOCATION],
            expected: vec!["a"],
        },
        FilterCase {
            case: "HISTORICAL_LOCATION, a location nothing sent",
            name: "HISTORICAL_LOCATION",
            match_option: Some("EQUALS"),
            values: vec!["eu-west-1"],
            expected: vec![],
        },
    ];

    let emulator = Emulator::new();
    let (estimate, created) = seed(&emulator, LINES.len()).await;
    // The id each line key was assigned, so an expectation can be stated in keys.
    let id_of = |key: &str| -> String {
        let index = LINES
            .iter()
            .position(|line| line.key == key)
            .unwrap_or_else(|| panic!("{key} is not a seeded line"));
        created[index]["id"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    };

    for case in &cases {
        let mut filter = json!({ "name": case.name, "values": case.values });
        if let Some(match_option) = case.match_option {
            filter["matchOption"] = json!(match_option);
        }
        let (status, body) = list(&emulator, &estimate, json!({ "filters": [filter] })).await;
        assert_eq!(
            status, 200,
            "case {}: expected 200, got {status} with {body}",
            case.case
        );
        let expected: Vec<String> = case.expected.iter().map(|key| id_of(key)).collect();
        assert_eq!(
            ids_of(&body),
            expected,
            "case {}: expected the lines {:?}, got {body}",
            case.case,
            case.expected
        );
    }

    // Two filters are AND-ed: a line must satisfy both, and a pair that cannot both hold
    // selects nothing.
    for (case, filters, expected) in [
        (
            "SERVICE_CODE and USAGE_GROUP agreeing",
            json!([
                { "name": "SERVICE_CODE", "values": ["AmazonEC2"] },
                { "name": "USAGE_GROUP", "values": ["group-b"] },
            ]),
            vec!["b"],
        ),
        (
            "SERVICE_CODE and USAGE_GROUP disagreeing",
            json!([
                { "name": "SERVICE_CODE", "values": ["AmazonS3"] },
                { "name": "USAGE_GROUP", "values": ["group-a"] },
            ]),
            vec![],
        ),
        (
            "an own attribute AND a historical one",
            json!([
                { "name": "SERVICE_CODE", "values": ["AmazonEC2"] },
                { "name": "HISTORICAL_SERVICE_CODE", "values": [HISTORICAL_SERVICE_CODE] },
            ]),
            vec!["a"],
        ),
        (
            "three filters, each narrowing further",
            json!([
                { "name": "OPERATION", "values": ["RunInstances"] },
                { "name": "USAGE_TYPE", "values": ["BoxUsage:m5.xlarge"] },
                { "name": "USAGE_ACCOUNT_ID", "values": ["222222222222"] },
            ]),
            vec!["f"],
        ),
    ] {
        let (status, body) = list(&emulator, &estimate, json!({ "filters": filters })).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200, got {status} with {body}"
        );
        let expected_ids: Vec<String> = expected.iter().map(|key| id_of(key)).collect();
        assert_eq!(
            ids_of(&body),
            expected_ids,
            "case {case}: expected the lines {expected:?}, got {body}"
        );
    }
}

/// RULE: a `nextToken` is bound to the selection it was measured against, so an offset is
/// never applied to a differently selected list: a token from another request, or one
/// this service never issued, is refused as a `ValidationException` (HTTP 400) rather
/// than silently skipping or repeating items.
///
/// The table spans a token this service never issued, a hex token without this service's
/// prefix, a real token replayed under a filter set it was not measured against, a real
/// token replayed against ANOTHER estimate, and a token minted by another operation's
/// list.
#[tokio::test]
async fn a_token_measured_against_another_selection_is_refused() {
    let emulator = Emulator::new();
    let (estimate, created) = seed(&emulator, LINES.len()).await;

    let (status, page) = list(&emulator, &estimate, json!({ "maxResults": 1 })).await;
    assert_eq!(
        status, 200,
        "seeding a real token: expected 200, got {status} with {page}"
    );
    let real = page["nextToken"]
        .as_str()
        .expect("a first page of one of six leaves a nextToken")
        .to_string();

    // A second estimate, holding the same lines: a token measured against one estimate's
    // list is not an offset into another's.
    let (other, _) = seed(&emulator, LINES.len()).await;

    // A token from another paginated list of this service, measured against a different
    // operation.
    let (status, scenarios) = emulator
        .call("CreateBillScenario", json!({ "name": "scenario-one" }))
        .await;
    assert_eq!(
        status, 200,
        "seeding a bill scenario: expected 200, got {status} with {scenarios}"
    );
    let (status, scenarios) = emulator
        .call("CreateBillScenario", json!({ "name": "scenario-two" }))
        .await;
    assert_eq!(
        status, 200,
        "seeding a second bill scenario: expected 200, got {status} with {scenarios}"
    );
    let (status, scenario_page) = emulator
        .call("ListBillScenarios", json!({ "maxResults": 1 }))
        .await;
    assert_eq!(
        status, 200,
        "seeding a foreign token: expected 200, got {status} with {scenario_page}"
    );
    let foreign_operation_token = scenario_page["nextToken"]
        .as_str()
        .expect("a first page of one of two leaves a nextToken")
        .to_string();

    let cases: [(&str, &str, Value); 5] = [
        (
            "a token this service never issued",
            estimate.as_str(),
            json!({ "nextToken": "not-a-token" }),
        ),
        (
            "a hex token without this service's prefix",
            estimate.as_str(),
            json!({ "nextToken": "6f66667365743a30" }),
        ),
        (
            "a real token under a filter it was not measured against",
            estimate.as_str(),
            json!({
                "maxResults": 1,
                "nextToken": real.clone(),
                "filters": [{ "name": "SERVICE_CODE", "values": ["AmazonEC2"] }],
            }),
        ),
        (
            "a real token against another estimate's list",
            other.as_str(),
            json!({ "maxResults": 1, "nextToken": real.clone() }),
        ),
        (
            "a token minted by another operation's list",
            estimate.as_str(),
            json!({ "maxResults": 1, "nextToken": foreign_operation_token }),
        ),
    ];

    for (case, target, extra) in cases {
        let (status, body) = list(&emulator, target, extra.clone()).await;
        assert_eq!(
            status, 400,
            "case {case} ({extra}): expected 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case} ({extra}): expected ValidationException, got {body}"
        );
    }

    // The same real token still works for the request it WAS measured against.
    let (status, body) = list(
        &emulator,
        &estimate,
        json!({ "maxResults": 1, "nextToken": real }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the token's own request: expected 200, got {status} with {body}"
    );
    assert_eq!(
        ids_of(&body),
        vec![created[1]["id"].as_str().unwrap_or_default().to_string()],
        "the token's own request: expected the second page, got {body}"
    );
}

/// RULE: usage can only be listed for a workload estimate that exists, so a
/// `workloadEstimateId` no live estimate carries is `ResourceNotFoundException` (HTTP
/// 404), naming the id asked for.
///
/// The table spans an id nothing ever created, an id whose estimate was deleted (with the
/// usage it held), and an id belonging to this service's OTHER root resource — each with
/// and without the optional members, since the refusal does not depend on them.
#[tokio::test]
async fn an_estimate_that_does_not_exist_is_a_resource_not_found() {
    let emulator = Emulator::new();

    let (deleted, _) = seed(&emulator, LINES.len()).await;
    let (status, body) = emulator
        .call("DeleteWorkloadEstimate", json!({ "identifier": deleted }))
        .await;
    assert_eq!(
        status, 200,
        "deleting the estimate: expected 200, got {status} with {body}"
    );

    let (status, scenario) = emulator
        .call("CreateBillScenario", json!({ "name": "not-an-estimate" }))
        .await;
    assert_eq!(
        status, 200,
        "seeding a bill scenario: expected 200, got {status} with {scenario}"
    );
    let scenario_id = scenario["id"]
        .as_str()
        .expect("a created scenario carries an id")
        .to_string();

    let targets = [
        ("an id nothing created", UNKNOWN_RESOURCE_ID.to_string()),
        ("an id whose estimate was deleted", deleted),
        ("the id of a bill scenario", scenario_id),
    ];
    let extras = [
        json!({}),
        json!({ "maxResults": MAX_RESULTS_MIN }),
        json!({ "filters": [{ "name": "SERVICE_CODE", "values": ["AmazonEC2"] }] }),
    ];

    for (case, target) in &targets {
        for extra in &extras {
            let (status, body) = list(&emulator, target, extra.clone()).await;
            assert_eq!(
                status, 404,
                "case {case} ({extra}): expected 404, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case} ({extra}): expected ResourceNotFoundException, got {body}"
            );
            assert_eq!(
                body["resourceId"].as_str(),
                Some(target.as_str()),
                "case {case} ({extra}): expected the refusal to name {target}, got {body}"
            );
        }
    }
}
