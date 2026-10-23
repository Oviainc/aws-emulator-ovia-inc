// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListTables` over the awsJson1_1 wire: `POST /` with
//! `X-Amz-Target: AWSBillingAndCostManagementDataExports.ListTables`.
//!
//! The rule under test: `ListTables` reports the service's data-export table catalog —
//! every table it holds, exactly once, in one stable order — one page at a time, where a
//! page holds at most `MaxResults` tables and the token a page carries resumes
//! immediately after it. Asserted over a TABLE of page sizes (unpaged, each modelled
//! bound, every size from one table to one past the catalog) and over EVERY table the
//! catalog reports, because the rule is about the catalog and its paging rather than
//! about any one table or page size. The catalog is service-owned: listing it changes
//! nothing, so two listings of it agree.
//!
//! `GetTable`'s cover of the same catalog's entries lives in `table_catalog.rs`; the
//! router, the request signing and the error-code helper come from the
//! `export_lifecycle` suite of this same test target rather than being built again here.

use serde_json::{json, Map, Value};

use super::export_lifecycle::{error_code, Client};

/// `Table.TableName`: `length 0..1024` (smithy_query shape `TableName`).
const TABLE_NAME_MAX_LEN: usize = 1024;

/// `GenericString`, the target of every description and table-property string:
/// `length 0..1024` (smithy_query shape `GenericString`).
const GENERIC_STRING_MAX_LEN: usize = 1024;

/// `MaxResults`: `range 1..300` (smithy_query operation `ListTables`, member
/// `MaxResults`).
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 300;

/// A table of the data-export catalog that every `QueryStatement` in this crate's tests
/// selects from, so the catalog must advertise it. One entry, not the whole catalog: the
/// assertions below are about the catalog as a whole.
const KNOWN_TABLE: &str = "COST_AND_USAGE_REPORT";

/// A ceiling on the pages one walk may take, so a listing that handed back a resume
/// token without making progress fails the test instead of looping forever.
const MAX_PAGES: usize = 64;

/// A reported table property's `DefaultValue`, which a property that publishes one
/// carries.
fn default_value_of(property: &Value) -> Option<&str> {
    property.get("DefaultValue").and_then(Value::as_str)
}

/// The only table property whose value the caller WRITES rather than picks from a
/// published list and that the dictionary publishes NO list at all for — the billing view
/// ARN (AWS Data Exports table dictionary) — so it is the only one that may report an
/// empty `ValidValues`. Every other property publishes the values it accepts, the
/// recommendations filter publishing the wildcard `*`, and `catalog_dictionary.rs` pins
/// each of those lists.
const FREE_FORM_PROPERTIES: &[&str] = &["BILLING_VIEW_ARN"];

/// The WILDCARD valid value: a property that publishes it accepts any value the caller
/// writes, so its `DefaultValue` need not appear in the list (AWS Data Exports table
/// dictionary).
const WILDCARD_VALID_VALUE: &str = "*";

/// Call `ListTables` with the paging members a case sets, requiring HTTP 200.
async fn list_tables(client: &Client, max_results: Option<i64>, token: Option<&str>) -> Value {
    let mut request = Map::new();
    if let Some(max) = max_results {
        request.insert("MaxResults".to_string(), json!(max));
    }
    if let Some(token) = token {
        request.insert("NextToken".to_string(), json!(token));
    }
    client.ok("ListTables", &Value::Object(request)).await
}

/// The `Tables` of one response, as reported.
fn tables_of(case: &str, response: &Value) -> Vec<Value> {
    response
        .get("Tables")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: ListTables must report Tables, got {response}"))
        .clone()
}

/// A reported table's `TableName`.
fn name_of(case: &str, table: &Value) -> String {
    table
        .get("TableName")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: a listed table must carry TableName, got {table}"))
        .to_string()
}

/// Walk the whole catalog with `max_results`, returning every table reported, in the
/// order the listing reported them, and how many pages it took.
async fn walk(client: &Client, max_results: Option<i64>) -> (Vec<Value>, usize) {
    let case = format!("MaxResults={max_results:?}");
    let mut seen: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        assert!(
            pages < MAX_PAGES,
            "case {case:?}: the listing did not terminate within {MAX_PAGES} pages"
        );
        let response = list_tables(client, max_results, token.as_deref()).await;
        let page = tables_of(&case, &response);
        pages += 1;
        if let Some(max) = max_results {
            assert!(
                page.len() as i64 <= max,
                "case {case:?}: a page carried {} tables, more than MaxResults",
                page.len()
            );
        }
        seen.extend(page);
        match response.get("NextToken").and_then(Value::as_str) {
            Some(next) => {
                assert!(
                    !next.is_empty(),
                    "case {case:?}: a resume token must not be empty, got {response}"
                );
                token = Some(next.to_string());
            }
            None => break,
        }
    }
    (seen, pages)
}

/// Every page size walks the whole catalog exactly once, in one stable order, and only
/// the final page omits the resume token.
#[tokio::test]
async fn list_tables_pages_over_the_whole_catalog() {
    let client = Client::new();

    // The catalog as the unpaged call reports it: the reference every paged walk is
    // compared against, so the assertions do not restate the catalog's contents.
    let (unpaged, pages) = walk(&client, None).await;
    let expected: Vec<String> = unpaged
        .iter()
        .map(|table| name_of("MaxResults=None", table))
        .collect();
    assert_eq!(
        pages, 1,
        "case \"MaxResults=None\": an unpaged listing must answer in one page, took {pages}"
    );
    assert!(
        !expected.is_empty(),
        "case \"MaxResults=None\": the catalog must advertise at least one table"
    );
    let mut sorted = expected.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(
        expected, sorted,
        "case \"MaxResults=None\": the catalog must be listed in one stable order with no repeats, got {expected:?}"
    );
    assert!(
        expected.iter().any(|name| name == KNOWN_TABLE),
        "case \"MaxResults=None\": the catalog must advertise {KNOWN_TABLE}, got {expected:?}"
    );

    let total = expected.len() as i64;
    // The rows: the modelled bounds of MaxResults, every page size from one table up to
    // one past the catalog, and the unpaged call.
    let mut sizes: Vec<Option<i64>> = vec![None, Some(MAX_RESULTS_MIN), Some(MAX_RESULTS_MAX)];
    sizes.extend((1..=total + 1).map(Some));
    for max_results in sizes {
        let case = format!("MaxResults={max_results:?}");
        let (seen, pages) = walk(&client, max_results).await;
        let names: Vec<String> = seen.iter().map(|table| name_of(&case, table)).collect();
        assert_eq!(
            names, expected,
            "case {case:?}: paging must report every table exactly once in catalog order"
        );
        let expected_pages = match max_results {
            Some(max) => ((total + max - 1) / max).max(1) as usize,
            None => 1,
        };
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} page(s) over {total} table(s), took {pages}"
        );
        // Each reported table is the same whichever page it arrived on.
        assert_eq!(
            seen, unpaged,
            "case {case:?}: a paged table must match the one the unpaged listing reported"
        );
    }
}

/// Every table the catalog reports satisfies the modelled `Table` contract, and its
/// property descriptions are self-consistent: a default the property itself accepts.
#[tokio::test]
async fn every_listed_table_satisfies_the_modelled_table_contract() {
    let client = Client::new();
    let (tables, _) = walk(&client, None).await;

    for table in &tables {
        let name = name_of("catalog", table);
        assert!(
            !name.is_empty() && name.len() <= TABLE_NAME_MAX_LEN,
            "case {name:?}: TableName must be 1..={TABLE_NAME_MAX_LEN} characters, got {} characters",
            name.len()
        );
        let description = table
            .get("Description")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {name:?}: a listed table must carry Description, got {table}"));
        assert!(
            !description.is_empty() && description.len() <= GENERIC_STRING_MAX_LEN,
            "case {name:?}: Description must be 1..={GENERIC_STRING_MAX_LEN} characters, got {} characters",
            description.len()
        );

        // TableProperties is a list of descriptions; a table need not have any, but a
        // description that IS reported names a property, the values it accepts, and —
        // where the catalog publishes one — a default among them ("Each table property
        // has a default value that it assumes if not specified" — smithy_query shape
        // `GetTableRequest`, member `TableProperties`). A property the catalog publishes
        // NO default for assumes nothing and reports none: it is in force only for a
        // request that names it, which `catalog_schema.rs` covers. A FREE-FORM property —
        // one whose value the caller writes rather than picks, such as a billing view
        // ARN — publishes no valid values, so only its default is constrained.
        let properties = table
            .get("TableProperties")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {name:?}: TableProperties must be a list, got {table}"));
        let mut property_names: Vec<&str> = Vec::new();
        for property in properties {
            let property_name = property
                .get("Name")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {name:?}: a table property must carry Name, got {property}"));
            property_names.push(property_name);
            let valid_values: Vec<&str> = property
                .get("ValidValues")
                .and_then(Value::as_array)
                .unwrap_or_else(|| panic!("case {name:?}/{property_name}: ValidValues must be a list, got {property}"))
                .iter()
                .map(|value| {
                    value.as_str().unwrap_or_else(|| {
                        panic!("case {name:?}/{property_name}: a valid value must be a string, got {value}")
                    })
                })
                .collect();
            assert!(
                !valid_values.is_empty() || FREE_FORM_PROPERTIES.contains(&property_name),
                "case {name:?}/{property_name}: only a free-form property {FREE_FORM_PROPERTIES:?} may report an empty ValidValues, got {property}"
            );
            if let Some(default_value) = default_value_of(property) {
                assert!(
                    valid_values.is_empty()
                        || valid_values.contains(&WILDCARD_VALID_VALUE)
                        || valid_values.contains(&default_value),
                    "case {name:?}/{property_name}: DefaultValue {default_value:?} must be one of {valid_values:?}"
                );
            }
            let property_description = property
                .get("Description")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {name:?}/{property_name}: Description is absent from {property}"));
            assert!(
                !property_description.is_empty()
                    && property_description.len() <= GENERIC_STRING_MAX_LEN,
                "case {name:?}/{property_name}: Description must be 1..={GENERIC_STRING_MAX_LEN} characters, got {} characters",
                property_description.len()
            );
        }
        let mut unique = property_names.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(
            unique.len(),
            property_names.len(),
            "case {name:?}: a table property must be described once, got {property_names:?}"
        );
    }
}

/// Every table `ListTables` advertises is a table `GetTable` describes, with the same
/// name, description and table properties: the two readers answer out of one catalog.
#[tokio::test]
async fn every_listed_table_is_one_get_table_describes() {
    let client = Client::new();
    let (tables, _) = walk(&client, None).await;

    for table in &tables {
        let name = name_of("catalog", table);
        let described = client
            .ok("GetTable", &json!({ "TableName": name }))
            .await;
        assert_eq!(
            described.get("TableName").and_then(Value::as_str),
            Some(name.as_str()),
            "case {name:?}: GetTable must describe the table the listing advertised, got {described}"
        );
        assert_eq!(
            described.get("Description"),
            table.get("Description"),
            "case {name:?}: the two readers must report one description, got {described}"
        );
        // The listing reports each property's description; GetTable reports the
        // properties in FORCE as a map, so the names the two share are the properties
        // that assume a published default — a property the catalog publishes no default
        // for is in force only for a request that names it.
        let listed_names: Vec<&str> = table
            .get("TableProperties")
            .and_then(Value::as_array)
            .expect("the listing reports TableProperties")
            .iter()
            .filter(|property| default_value_of(property).is_some())
            .map(|property| {
                property
                    .get("Name")
                    .and_then(Value::as_str)
                    .expect("a listed property carries Name")
            })
            .collect();
        let mut described_names: Vec<&str> = described
            .get("TableProperties")
            .and_then(Value::as_object)
            .map(|properties| properties.keys().map(String::as_str).collect())
            .unwrap_or_default();
        described_names.sort_unstable();
        let mut expected_names = listed_names.clone();
        expected_names.sort_unstable();
        assert_eq!(
            described_names, expected_names,
            "case {name:?}: GetTable must report the properties the listing advertised"
        );
    }
}

/// Listing the catalog is a read: it changes nothing a later caller sees, and it is
/// unaffected by the exports a caller creates.
#[tokio::test]
async fn listing_the_catalog_changes_nothing() {
    let client = Client::new();
    let first = list_tables(&client, None, None).await;

    // A paged walk, a page token, and an export created in between: none of them may
    // change what the catalog answers.
    let _ = walk(&client, Some(1)).await;
    client
        .ok("CreateExport", &minimal_export_request("catalog_probe"))
        .await;

    let second = list_tables(&client, None, None).await;
    assert_eq!(
        first, second,
        "a listing of the catalog must report the same tables on every call"
    );
}

/// A `MaxResults` outside the modelled range is refused, and each published bound is
/// accepted.
#[tokio::test]
async fn max_results_outside_the_modelled_range_is_refused() {
    let client = Client::new();
    for accepted in [MAX_RESULTS_MIN, MAX_RESULTS_MAX] {
        let (status, body) = client
            .call("ListTables", &json!({ "MaxResults": accepted }))
            .await;
        assert_eq!(
            status, 200,
            "case MaxResults={accepted}: a value inside range 1..300 must be accepted, got {status} with {body}"
        );
    }
    for refused in [MAX_RESULTS_MIN - 1, MAX_RESULTS_MAX + 1] {
        let (status, body) = client
            .call("ListTables", &json!({ "MaxResults": refused }))
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case MaxResults={refused}: a value outside range 1..300 must be refused, got {body}"
        );
    }
}

/// A `NextToken` this listing did not issue is refused with the modelled
/// `ValidationException`, and a token is bound to the listing that cut it: neither
/// listing resumes at a position in a sequence it never walked.
#[tokio::test]
async fn an_unissued_next_token_is_refused() {
    let client = Client::new();
    let issued = list_tables(&client, Some(1), None).await["NextToken"]
        .as_str()
        .expect("a first page over a catalog of several tables must carry a NextToken")
        .to_string();

    let mut corrupted = issued.clone();
    corrupted.replace_range(0..1, if issued.starts_with('a') { "b" } else { "a" });
    // Forged tokens: not hex, too short to carry the integrity tag, hex of a plausible
    // table name with no tag, and an issued token with its tag corrupted.
    for forged in [
        "not-a-token".to_string(),
        "00".to_string(),
        hex::encode(KNOWN_TABLE),
        corrupted,
        issued[..issued.len() - 2].to_string(),
    ] {
        let (status, body) = client
            .call("ListTables", &json!({ "NextToken": forged }))
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {forged:?}: an unissued NextToken must be refused, got {body}"
        );
    }

    // A token cut from the catalog names a position in the catalog, so the export
    // listing must refuse it...
    let (status, body) = client
        .call("ListExports", &json!({ "NextToken": issued.clone() }))
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ValidationException"),
        "case \"tables token at ListExports\": a foreign token must be refused, got {body}"
    );

    // ...and the other way round: a token cut from the export listing is not a position
    // in the catalog.
    client
        .ok("CreateExport", &minimal_export_request("token_probe_one"))
        .await;
    client
        .ok("CreateExport", &minimal_export_request("token_probe_two"))
        .await;
    let exports_token = client
        .ok("ListExports", &json!({ "MaxResults": 1 }))
        .await["NextToken"]
        .as_str()
        .expect("a first page over two exports must carry a NextToken")
        .to_string();
    let (status, body) = client
        .call("ListTables", &json!({ "NextToken": exports_token }))
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ValidationException"),
        "case \"exports token at ListTables\": a foreign token must be refused, got {body}"
    );

    // The token the catalog listing DID issue is still honoured.
    let resumed = list_tables(&client, Some(1), Some(&issued)).await;
    assert_eq!(
        tables_of("resumed", &resumed).len(),
        1,
        "case \"issued token\": the token this listing issued must resume it, got {resumed}"
    );
}

/// The smallest `CreateExport` request the model accepts, for the exports this suite
/// needs only as something for the OTHER listing to page over.
///
/// `pub(crate)` so another suite that needs several exports and does not care about their
/// definitions builds them from this one request rather than from a second copy of it.
pub(crate) fn minimal_export_request(name: &str) -> Value {
    json!({
        "Export": {
            "Name": name,
            "DataQuery": {
                "QueryStatement": format!("SELECT line_item_usage_account_id FROM {KNOWN_TABLE}")
            },
            "DestinationConfigurations": {
                "S3Destination": {
                    "S3Bucket": format!("{name}-bucket"),
                    "S3Prefix": "exports/daily",
                    "S3Region": "us-east-1",
                    "S3OutputConfigurations": {
                        "OutputType": "CUSTOM",
                        "Format": "TEXT_OR_CSV",
                        "Compression": "GZIP",
                        "Overwrite": "OVERWRITE_REPORT"
                    }
                }
            },
            "RefreshCadence": { "Frequency": "SYNCHRONOUS" }
        },
        "ResourceTags": []
    })
}
