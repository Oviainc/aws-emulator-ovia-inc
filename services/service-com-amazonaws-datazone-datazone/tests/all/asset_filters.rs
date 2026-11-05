// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for DataZone asset filters
//! (`CreateAssetFilter`/`GetAssetFilter`).
//!
//! Every case drives the service the way a client does — a real `restJson1` request
//! through the router — and uses only the response body as evidence, so a handler that
//! answered from a hardcoded value instead of from the store would fail here.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one request:
//!
//! 1. `CreateAssetFilter` stores a filter under a persisted asset of a live domain and
//!    reports the VIEW its configuration defines: a column filter's
//!    `effectiveColumnNames` are the included columns the asset's schema has, a row
//!    filter's `effectiveRowFilter` is its expression as one predicate, and the other
//!    member stays absent.
//! 2. The filter is `VALID` exactly when every column it names is available in the
//!    asset's schema, and `INVALID` with an `errorMessage` naming the unavailable
//!    columns otherwise; an asset whose stored forms declare no columns constrains
//!    nothing, so a filter over it is valid.
//! 3. `GetAssetFilter` reports the stored record back, member for member, and changes
//!    nothing a later caller sees. The derived members follow the ASSET: a filter naming
//!    a column a later asset revision declares reports `VALID` from then on.
//! 4. The preconditions are refused with the operations' own modelled errors: a missing
//!    domain, asset or filter with `ResourceNotFoundException`, and a filter bigger than
//!    this emulator evaluates — nested past `MAX_ROW_FILTER_DEPTH`, holding more than
//!    `MAX_ROW_FILTER_NODES` expressions, or naming more than
//!    `MAX_INCLUDED_COLUMN_NAMES` columns — with `ValidationException`, storing nothing.
//! 5. A repeated `clientToken` returns the filter the first call created instead of a
//!    second one.

use crate::harness::Emulator;
use serde_json::{json, Value};

/// The columns the fixture asset's relational-table form declares.
///
/// An asset's column schema lives in the content of a metadata form — DataZone's
/// relational table form carries a `columns` array whose entries name a `columnName` —
/// so this is the schema an asset filter over the fixture asset is judged against.
const SCHEMA_COLUMNS: [&str; 6] = [
    "order_id",
    "region",
    "amount",
    "ship_date",
    "customer_email",
    "status_code",
];

/// `FilterId`'s modelled pattern, `^[a-zA-Z0-9_-]{1,36}$` (smithy_query
/// shape="FilterId").
fn is_filter_id(id: &str) -> bool {
    (1..=36).contains(&id.len())
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// A member of a body as a list of strings, or `None` when the member is absent.
fn strings(body: &Value, member: &str) -> Option<Vec<String>> {
    Some(
        body.get(member)?
            .as_array()?
            .iter()
            .map(|entry| entry.as_str().unwrap_or_default().to_string())
            .collect(),
    )
}

/// One asset: the domain it lives in, its id, and the filters URI of it.
struct Fixture {
    domain: String,
    /// An asset whose form declares [`SCHEMA_COLUMNS`].
    with_schema: String,
    /// An asset whose forms declare no `columns` array at all, so its schema is not
    /// known to the emulator.
    without_schema: String,
}

impl Fixture {
    /// The `CreateAssetFilter`/`ListAssetFilters` URI of one asset.
    fn filters_uri(&self, asset: &str) -> String {
        format!("/v2/domains/{}/assets/{asset}/filters", self.domain)
    }

    /// The `GetAssetFilter` URI of one filter.
    fn filter_uri(&self, asset: &str, filter: &str) -> String {
        format!(
            "/v2/domains/{}/assets/{asset}/filters/{filter}",
            self.domain
        )
    }
}

/// A settled domain, a project, and the two assets every case below filters.
async fn fixture(dz: &Emulator, domain_name: &str) -> Fixture {
    let domain = dz.available_domain(domain_name).await;
    let project = dz.project(&domain, "filter owner").await;
    let assets_uri = format!("/v2/domains/{domain}/assets");

    let columns: Vec<Value> = SCHEMA_COLUMNS
        .iter()
        .map(|name| json!({ "columnName": name, "dataType": "string" }))
        .collect();
    let schema = serde_json::to_string(&json!({ "columns": columns }))
        .expect("the form content serializes");
    let with_schema = dz
        .call(
            "POST",
            &assets_uri,
            Some(json!({
                "name": "orders",
                "typeIdentifier": "amazon.datazone.RelationalTable",
                "owningProjectIdentifier": project,
                "formsInput": [{
                    "formName": "RelationalTableForm",
                    "typeIdentifier": "amazon.datazone.RelationalTableFormType",
                    "content": schema,
                }],
            })),
        )
        .await;
    assert_eq!(
        with_schema.status, 201,
        "creating the asset with a schema: expected 201, got {} {:?}",
        with_schema.status, with_schema.body
    );

    let without_schema = dz
        .call(
            "POST",
            &assets_uri,
            Some(json!({
                "name": "unschemad",
                "typeIdentifier": "amazon.datazone.RelationalTable",
                "owningProjectIdentifier": project,
                "formsInput": [{
                    "formName": "OwnershipForm",
                    "content": "{\"owner\":\"data-eng\"}",
                }],
            })),
        )
        .await;
    assert_eq!(
        without_schema.status, 201,
        "creating the asset without a schema: expected 201, got {} {:?}",
        without_schema.status, without_schema.body
    );

    Fixture {
        domain,
        with_schema: with_schema.s("id"),
        without_schema: without_schema.s("id"),
    }
}

/// One `CreateAssetFilter` request and the view it must report back.
struct FilterCase {
    label: &'static str,
    /// Whether the filter is created on the asset that declares a schema.
    on_schema_asset: bool,
    /// The `configuration` union the request carries.
    configuration: Value,
    expect_status: &'static str,
    /// `effectiveColumnNames`, or `None` when the member must be absent.
    expect_columns: Option<Vec<&'static str>>,
    /// `effectiveRowFilter`, or `None` when the member must be absent.
    expect_row_filter: Option<&'static str>,
    /// The columns the `errorMessage` must name; empty for a valid filter, which
    /// carries no `errorMessage` at all.
    expect_missing: Vec<&'static str>,
}

/// The rows differ along what the rule generalises over: which member of the
/// configuration union is used, how many columns are named, whether the asset declares
/// a schema at all, whether a named column is in it, how deep the row filter nests, and
/// which comparison the expression makes.
fn filter_cases() -> Vec<FilterCase> {
    vec![
        FilterCase {
            label: "column filter over columns the schema has",
            on_schema_asset: true,
            configuration: json!({
                "columnConfiguration": { "includedColumnNames": ["order_id", "region"] },
            }),
            expect_status: "VALID",
            expect_columns: Some(vec!["order_id", "region"]),
            expect_row_filter: None,
            expect_missing: vec![],
        },
        FilterCase {
            label: "column filter naming every column of the schema, out of order",
            on_schema_asset: true,
            configuration: json!({
                "columnConfiguration": { "includedColumnNames": [
                    "status_code", "customer_email", "ship_date", "amount", "region", "order_id",
                ] },
            }),
            expect_status: "VALID",
            // Reported in the order the REQUEST named them, not the schema's.
            expect_columns: Some(vec![
                "status_code",
                "customer_email",
                "ship_date",
                "amount",
                "region",
                "order_id",
            ]),
            expect_row_filter: None,
            expect_missing: vec![],
        },
        FilterCase {
            label: "column filter repeating a column",
            on_schema_asset: true,
            configuration: json!({
                "columnConfiguration": { "includedColumnNames": ["amount", "amount", "region"] },
            }),
            expect_status: "VALID",
            expect_columns: Some(vec!["amount", "region"]),
            expect_row_filter: None,
            expect_missing: vec![],
        },
        FilterCase {
            label: "column filter naming one column the schema does not have",
            on_schema_asset: true,
            configuration: json!({
                "columnConfiguration": { "includedColumnNames": ["order_id", "ssn"] },
            }),
            expect_status: "INVALID",
            expect_columns: Some(vec!["order_id"]),
            expect_row_filter: None,
            expect_missing: vec!["ssn"],
        },
        FilterCase {
            label: "column filter naming only columns the schema does not have",
            on_schema_asset: true,
            configuration: json!({
                "columnConfiguration": { "includedColumnNames": ["ssn", "home_address"] },
            }),
            expect_status: "INVALID",
            expect_columns: Some(vec![]),
            expect_row_filter: None,
            expect_missing: vec!["ssn", "home_address"],
        },
        FilterCase {
            label: "column filter with no included column at all",
            on_schema_asset: true,
            configuration: json!({ "columnConfiguration": { "includedColumnNames": [] } }),
            expect_status: "VALID",
            expect_columns: Some(vec![]),
            expect_row_filter: None,
            expect_missing: vec![],
        },
        FilterCase {
            label: "column filter on an asset whose forms declare no columns",
            on_schema_asset: false,
            configuration: json!({
                "columnConfiguration": { "includedColumnNames": ["anything", "at_all"] },
            }),
            // An asset that declares no schema contradicts nothing.
            expect_status: "VALID",
            expect_columns: Some(vec!["anything", "at_all"]),
            expect_row_filter: None,
            expect_missing: vec![],
        },
        FilterCase {
            label: "row filter with one comparison over a column the schema has",
            on_schema_asset: true,
            configuration: json!({
                "rowConfiguration": {
                    "rowFilter": { "expression": { "equalTo": {
                        "columnName": "region", "value": "us-west-2",
                    } } },
                    "sensitive": true,
                },
            }),
            expect_status: "VALID",
            expect_columns: None,
            expect_row_filter: Some("region = 'us-west-2'"),
            expect_missing: vec![],
        },
        FilterCase {
            label: "row filter with two comparisons joined by and",
            on_schema_asset: true,
            configuration: json!({
                "rowConfiguration": {
                    "rowFilter": { "and": [
                        { "expression": { "greaterThan": {
                            "columnName": "amount", "value": "100",
                        } } },
                        { "expression": { "isNotNull": { "columnName": "ship_date" } } },
                    ] },
                    "sensitive": false,
                },
            }),
            expect_status: "VALID",
            expect_columns: None,
            expect_row_filter: Some("(amount > '100' AND ship_date IS NOT NULL)"),
            expect_missing: vec![],
        },
        FilterCase {
            label: "row filter nesting an or inside an and",
            on_schema_asset: true,
            configuration: json!({
                "rowConfiguration": {
                    "rowFilter": { "and": [
                        { "or": [
                            { "expression": { "like": {
                                "columnName": "customer_email", "value": "%@example.com",
                            } } },
                            { "expression": { "in": {
                                "columnName": "status_code", "values": ["OPEN", "SHIPPED"],
                            } } },
                        ] },
                        { "expression": { "notEqualTo": {
                            "columnName": "region", "value": "eu-west-1",
                        } } },
                    ] },
                },
            }),
            expect_status: "VALID",
            expect_columns: None,
            expect_row_filter: Some(
                "((customer_email LIKE '%@example.com' OR status_code IN ('OPEN', 'SHIPPED')) \
                 AND region <> 'eu-west-1')",
            ),
            expect_missing: vec![],
        },
        FilterCase {
            label: "row filter with a single-member group",
            on_schema_asset: true,
            configuration: json!({
                "rowConfiguration": {
                    "rowFilter": { "or": [
                        { "expression": { "isNull": { "columnName": "ship_date" } } },
                    ] },
                },
            }),
            expect_status: "VALID",
            expect_columns: None,
            expect_row_filter: Some("ship_date IS NULL"),
            expect_missing: vec![],
        },
        FilterCase {
            label: "row filter whose value carries a quote",
            on_schema_asset: true,
            configuration: json!({
                "rowConfiguration": {
                    "rowFilter": { "expression": { "equalTo": {
                        "columnName": "customer_email", "value": "o'hara@example.com",
                    } } },
                },
            }),
            expect_status: "VALID",
            expect_columns: None,
            expect_row_filter: Some("customer_email = 'o''hara@example.com'"),
            expect_missing: vec![],
        },
        FilterCase {
            label: "row filter over a column the schema does not have",
            on_schema_asset: true,
            configuration: json!({
                "rowConfiguration": {
                    "rowFilter": { "and": [
                        { "expression": { "equalTo": { "columnName": "region", "value": "us-east-1" } } },
                        { "expression": { "lessThan": { "columnName": "ssn", "value": "9" } } },
                    ] },
                },
            }),
            expect_status: "INVALID",
            expect_columns: None,
            // The predicate is still reported: it is what the filter says, whether or
            // not the asset has every column it names.
            expect_row_filter: Some("(region = 'us-east-1' AND ssn < '9')"),
            expect_missing: vec!["ssn"],
        },
        FilterCase {
            label: "row filter on an asset whose forms declare no columns",
            on_schema_asset: false,
            configuration: json!({
                "rowConfiguration": {
                    "rowFilter": { "expression": { "notLike": {
                        "columnName": "whatever", "value": "x%",
                    } } },
                },
            }),
            expect_status: "VALID",
            expect_columns: None,
            expect_row_filter: Some("whatever NOT LIKE 'x%'"),
            expect_missing: vec![],
        },
    ]
}

#[tokio::test]
async fn a_created_filter_reports_the_view_it_defines_and_is_read_back_by_get() {
    let dz = Emulator::new();
    let fx = fixture(&dz, "asset-filter-domain").await;

    for (index, case) in filter_cases().into_iter().enumerate() {
        let asset = if case.on_schema_asset {
            fx.with_schema.clone()
        } else {
            fx.without_schema.clone()
        };
        let name = format!("view {index}");
        let created = dz
            .call(
                "POST",
                &fx.filters_uri(&asset),
                Some(json!({
                    "name": name,
                    "description": "a controlled view",
                    "configuration": case.configuration,
                })),
            )
            .await;
        assert_eq!(
            created.status, 201,
            "case {:?}: expected 201, got {} {:?}",
            case.label, created.status, created.body
        );
        let id = created.s("id");
        assert!(
            is_filter_id(&id),
            "case {:?}: expected an id matching ^[a-zA-Z0-9_-]{{1,36}}$, got {id:?}",
            case.label
        );
        assert_eq!(
            created.s("assetId"),
            asset,
            "case {:?}: expected the filter on asset {asset}, got {:?}",
            case.label,
            created.body
        );
        assert_eq!(
            created.s("domainId"),
            fx.domain,
            "case {:?}: expected the filter in domain {}, got {:?}",
            case.label,
            fx.domain,
            created.body
        );
        assert_eq!(
            created.s("name"),
            name,
            "case {:?}: expected the requested name, got {:?}",
            case.label,
            created.body
        );
        assert_eq!(
            created.s("description"),
            "a controlled view",
            "case {:?}: expected the requested description, got {:?}",
            case.label,
            created.body
        );
        assert!(
            created.body.get("createdAt").is_some(),
            "case {:?}: expected a createdAt, got {:?}",
            case.label,
            created.body
        );
        assert_eq!(
            created.s("status"),
            case.expect_status,
            "case {:?}: expected status {}, got {:?}",
            case.label,
            case.expect_status,
            created.body
        );
        assert_eq!(
            strings(&created.body, "effectiveColumnNames"),
            case.expect_columns
                .as_ref()
                .map(|names| names.iter().map(|name| (*name).to_string()).collect()),
            "case {:?}: unexpected effectiveColumnNames in {:?}",
            case.label,
            created.body
        );
        assert_eq!(
            created.body.get("effectiveRowFilter").and_then(Value::as_str),
            case.expect_row_filter,
            "case {:?}: unexpected effectiveRowFilter in {:?}",
            case.label,
            created.body
        );
        let error_message = created.body.get("errorMessage").and_then(Value::as_str);
        if case.expect_missing.is_empty() {
            assert!(
                error_message.is_none(),
                "case {:?}: a valid filter carries no errorMessage, got {error_message:?}",
                case.label
            );
        } else {
            let message = error_message.unwrap_or_default();
            for missing in &case.expect_missing {
                assert!(
                    message.contains(missing),
                    "case {:?}: expected the errorMessage to name {missing:?}, got {message:?}",
                    case.label
                );
            }
            for present in SCHEMA_COLUMNS {
                assert!(
                    !message.contains(present),
                    "case {:?}: the errorMessage names the available column {present:?}: {message:?}",
                    case.label
                );
            }
        }

        // The round trip: the stored record, read back through the implemented reader.
        let read = dz.call("GET", &fx.filter_uri(&asset, &id), None).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: expected GetAssetFilter to answer 200, got {} {:?}",
            case.label, read.status, read.body
        );
        assert_eq!(
            read.body, created.body,
            "case {:?}: GetAssetFilter must report what CreateAssetFilter stored",
            case.label
        );
        // A read changes nothing a later caller sees.
        let again = dz.call("GET", &fx.filter_uri(&asset, &id), None).await;
        assert_eq!(
            again.body, read.body,
            "case {:?}: a second read answered differently",
            case.label
        );
    }
}

/// Every member of the modelled `RowFilterExpression` union, and the predicate it
/// renders as.
///
/// SOURCE: `smithy_query(query="shape", shape="RowFilterExpression")` — equalTo,
/// notEqualTo, greaterThan, lessThan, greaterThanOrEqualTo, lessThanOrEqualTo, isNull,
/// isNotNull, in, notIn, like, notLike. Each is exercised on a column the fixture
/// asset's schema declares, so the verdict is `VALID` for all of them.
fn expression_cases() -> Vec<(&'static str, Value, &'static str)> {
    vec![
        (
            "equalTo",
            json!({ "equalTo": { "columnName": "region", "value": "us-west-2" } }),
            "region = 'us-west-2'",
        ),
        (
            "notEqualTo",
            json!({ "notEqualTo": { "columnName": "region", "value": "us-west-2" } }),
            "region <> 'us-west-2'",
        ),
        (
            "greaterThan",
            json!({ "greaterThan": { "columnName": "amount", "value": "10" } }),
            "amount > '10'",
        ),
        (
            "lessThan",
            json!({ "lessThan": { "columnName": "amount", "value": "10" } }),
            "amount < '10'",
        ),
        (
            "greaterThanOrEqualTo",
            json!({ "greaterThanOrEqualTo": { "columnName": "amount", "value": "10" } }),
            "amount >= '10'",
        ),
        (
            "lessThanOrEqualTo",
            json!({ "lessThanOrEqualTo": { "columnName": "amount", "value": "10" } }),
            "amount <= '10'",
        ),
        (
            "isNull",
            json!({ "isNull": { "columnName": "ship_date" } }),
            "ship_date IS NULL",
        ),
        (
            "isNotNull",
            json!({ "isNotNull": { "columnName": "ship_date" } }),
            "ship_date IS NOT NULL",
        ),
        (
            "in",
            json!({ "in": { "columnName": "status_code", "values": ["OPEN"] } }),
            "status_code IN ('OPEN')",
        ),
        (
            "notIn",
            json!({ "notIn": { "columnName": "status_code", "values": ["OPEN", "CLOSED"] } }),
            "status_code NOT IN ('OPEN', 'CLOSED')",
        ),
        (
            "like",
            json!({ "like": { "columnName": "customer_email", "value": "a%" } }),
            "customer_email LIKE 'a%'",
        ),
        (
            "notLike",
            json!({ "notLike": { "columnName": "customer_email", "value": "a%" } }),
            "customer_email NOT LIKE 'a%'",
        ),
    ]
}

#[tokio::test]
async fn every_row_filter_expression_renders_as_one_predicate() {
    let dz = Emulator::new();
    let fx = fixture(&dz, "row-filter-domain").await;

    for (index, (label, expression, predicate)) in expression_cases().into_iter().enumerate() {
        let created = dz
            .call(
                "POST",
                &fx.filters_uri(&fx.with_schema),
                Some(json!({
                    "name": format!("rows {index}"),
                    "configuration": { "rowConfiguration": {
                        "rowFilter": { "expression": expression },
                    } },
                })),
            )
            .await;
        assert_eq!(
            created.status, 201,
            "case {label:?}: expected 201, got {} {:?}",
            created.status, created.body
        );
        assert_eq!(
            created.s("effectiveRowFilter"),
            predicate,
            "case {label:?}: unexpected effectiveRowFilter in {:?}",
            created.body
        );
        assert_eq!(
            created.s("status"),
            "VALID",
            "case {label:?}: every column named is in the schema, so the filter is valid: {:?}",
            created.body
        );
        assert!(
            created.body.get("effectiveColumnNames").is_none(),
            "case {label:?}: a row filter names no columns, got {:?}",
            created.body.get("effectiveColumnNames")
        );
    }
}

#[tokio::test]
async fn a_filter_needs_a_live_domain_an_asset_and_its_own_id() {
    let dz = Emulator::new();
    let fx = fixture(&dz, "filter-precondition-domain").await;
    let configuration = json!({
        "columnConfiguration": { "includedColumnNames": ["order_id"] },
    });

    // A domain nothing created: a permission is held ON a domain, so the caller holds
    // none there and authorization decides the create before any lookup.
    let no_domain = dz
        .call(
            "POST",
            "/v2/domains/dzd_00000000000000000000/assets/aaaaaaaaaa/filters",
            Some(json!({ "name": "view", "configuration": configuration })),
        )
        .await;
    assert_eq!(
        (
            no_domain.status,
            no_domain.error_code.as_str(),
            no_domain.s("message")
        ),
        (
            403,
            "AccessDeniedException",
            "User is not permitted to perform operation: CreateAssetFilter".to_string()
        ),
        "a filter in a domain nothing created: got {} {:?}",
        no_domain.status,
        no_domain.body
    );

    // An asset nothing created, in a domain that exists.
    let no_asset = dz
        .call(
            "POST",
            &fx.filters_uri("deadbeef"),
            Some(json!({ "name": "view", "configuration": configuration.clone() })),
        )
        .await;
    assert_eq!(
        (no_asset.status, no_asset.error_code.as_str()),
        (404, "ResourceNotFoundException"),
        "a filter on an asset nothing created: got {} {:?}",
        no_asset.status,
        no_asset.body
    );

    // A filter id nothing created, on an asset that exists.
    let no_filter = dz
        .call("GET", &fx.filter_uri(&fx.with_schema, "deadbeef"), None)
        .await;
    assert_eq!(
        (no_filter.status, no_filter.error_code.as_str()),
        (404, "ResourceNotFoundException"),
        "reading a filter nothing created: got {} {:?}",
        no_filter.status,
        no_filter.body
    );

    // A filter of one asset is not a filter of another.
    let created = dz
        .call(
            "POST",
            &fx.filters_uri(&fx.with_schema),
            Some(json!({ "name": "scoped view", "configuration": configuration })),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "creating the scoped filter: expected 201, got {} {:?}",
        created.status, created.body
    );
    let elsewhere = dz
        .call(
            "GET",
            &fx.filter_uri(&fx.without_schema, &created.s("id")),
            None,
        )
        .await;
    assert_eq!(
        (elsewhere.status, elsewhere.error_code.as_str()),
        (404, "ResourceNotFoundException"),
        "a filter read through another asset: got {} {:?}",
        elsewhere.status,
        elsewhere.body
    );

    // A row filter nested deeper than this emulator evaluates is refused, not stored.
    // The bound itself is the service's own `MAX_ROW_FILTER_DEPTH`, so the case cannot
    // drift from the handler's rule; one level past it is what must be refused.
    let mut deep = json!({ "expression": { "isNull": { "columnName": "region" } } });
    for _ in 0..service_com_amazonaws_datazone_datazone::state::MAX_ROW_FILTER_DEPTH {
        deep = json!({ "and": [deep] });
    }
    let too_deep = dz
        .call(
            "POST",
            &fx.filters_uri(&fx.with_schema),
            Some(json!({
                "name": "deep view",
                "configuration": { "rowConfiguration": { "rowFilter": deep } },
            })),
        )
        .await;
    assert_eq!(
        (too_deep.status, too_deep.error_code.as_str()),
        (400, "ValidationException"),
        "a row filter nested past the bound: got {} {:?}",
        too_deep.status,
        too_deep.body
    );

    // A row filter with MORE EXPRESSIONS than this emulator evaluates is refused too:
    // nesting is only one of the two dimensions a request can grow in, and a single
    // shallow `and` group can carry any number of members. The bound is the service's
    // own `MAX_ROW_FILTER_NODES`, so the case cannot drift from the handler's rule.
    let members: Vec<Value> = (0..=service_com_amazonaws_datazone_datazone::state::MAX_ROW_FILTER_NODES)
        .map(|index| json!({ "expression": { "isNull": { "columnName": format!("c{index}") } } }))
        .collect();
    let too_wide = dz
        .call(
            "POST",
            &fx.filters_uri(&fx.with_schema),
            Some(json!({
                "name": "wide view",
                "configuration": { "rowConfiguration": { "rowFilter": { "and": members } } },
            })),
        )
        .await;
    assert_eq!(
        (too_wide.status, too_wide.error_code.as_str()),
        (400, "ValidationException"),
        "a row filter holding more expressions than the bound: got {} {:?}",
        too_wide.status,
        too_wide.body
    );

    // And a column filter naming more columns than this emulator evaluates.
    let names: Vec<Value> = (0
        ..=service_com_amazonaws_datazone_datazone::state::MAX_INCLUDED_COLUMN_NAMES)
        .map(|index| json!(format!("c{index}")))
        .collect();
    let too_many = dz
        .call(
            "POST",
            &fx.filters_uri(&fx.with_schema),
            Some(json!({
                "name": "many columns",
                "configuration": { "columnConfiguration": { "includedColumnNames": names } },
            })),
        )
        .await;
    assert_eq!(
        (too_many.status, too_many.error_code.as_str()),
        (400, "ValidationException"),
        "a column filter naming more columns than the bound: got {} {:?}",
        too_many.status,
        too_many.body
    );

    // Each refusal left nothing behind: the asset still takes a filter, and the
    // refusals above were not stored as one.
    let accepted = dz
        .call(
            "POST",
            &fx.filters_uri(&fx.with_schema),
            Some(json!({
                "name": "after refusals",
                "configuration": { "columnConfiguration": { "includedColumnNames": ["region"] } },
            })),
        )
        .await;
    assert_eq!(
        accepted.status, 201,
        "a filter inside every bound: expected 201, got {} {:?}",
        accepted.status, accepted.body
    );
}

#[tokio::test]
async fn a_filter_follows_the_schema_of_the_asset_it_views() {
    let dz = Emulator::new();
    let fx = fixture(&dz, "filter-revision-domain").await;

    // A filter naming a column the asset does not have yet.
    let created = dz
        .call(
            "POST",
            &fx.filters_uri(&fx.with_schema),
            Some(json!({
                "name": "pending view",
                "configuration": { "columnConfiguration": {
                    "includedColumnNames": ["region", "settled_at"],
                } },
            })),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "creating the filter: expected 201, got {} {:?}",
        created.status, created.body
    );
    let filter = created.s("id");
    assert_eq!(
        created.s("status"),
        "INVALID",
        "a filter naming a column the asset lacks is invalid: {:?}",
        created.body
    );
    assert_eq!(
        strings(&created.body, "effectiveColumnNames"),
        Some(vec!["region".to_string()]),
        "only the column the asset has is effective: {:?}",
        created.body
    );

    // A new revision of the asset declares that column, so the SAME filter is now
    // satisfied by the asset.
    let columns: Vec<Value> = SCHEMA_COLUMNS
        .iter()
        .chain(["settled_at"].iter())
        .map(|name| json!({ "columnName": name, "dataType": "string" }))
        .collect();
    let schema = serde_json::to_string(&json!({ "columns": columns }))
        .expect("the form content serializes");
    let revision = dz
        .call(
            "POST",
            &format!(
                "/v2/domains/{}/assets/{}/revisions",
                fx.domain, fx.with_schema
            ),
            Some(json!({
                "name": "orders",
                "formsInput": [{
                    "formName": "RelationalTableForm",
                    "typeIdentifier": "amazon.datazone.RelationalTableFormType",
                    "content": schema,
                }],
            })),
        )
        .await;
    assert_eq!(
        revision.status, 200,
        "recording the asset revision: expected 200, got {} {:?}",
        revision.status, revision.body
    );

    let read = dz
        .call("GET", &fx.filter_uri(&fx.with_schema, &filter), None)
        .await;
    assert_eq!(
        read.s("status"),
        "VALID",
        "a filter whose columns the asset now declares is valid: {:?}",
        read.body
    );
    assert_eq!(
        strings(&read.body, "effectiveColumnNames"),
        Some(vec!["region".to_string(), "settled_at".to_string()]),
        "every included column is effective once the asset declares it: {:?}",
        read.body
    );
    assert!(
        read.body.get("errorMessage").is_none(),
        "a valid filter carries no errorMessage, got {:?}",
        read.body.get("errorMessage")
    );
    assert_eq!(
        read.s("id"),
        filter,
        "the filter was re-evaluated, not replaced: {:?}",
        read.body
    );
}

#[tokio::test]
async fn a_repeated_client_token_returns_the_first_filter() {
    let dz = Emulator::new();
    let fx = fixture(&dz, "filter-idempotency-domain").await;
    let body = json!({
        "name": "token view",
        "clientToken": "a-caller-chosen-token",
        "configuration": { "columnConfiguration": { "includedColumnNames": ["amount"] } },
    });

    let first = dz
        .call("POST", &fx.filters_uri(&fx.with_schema), Some(body.clone()))
        .await;
    assert_eq!(
        first.status, 201,
        "the first create: expected 201, got {} {:?}",
        first.status, first.body
    );
    let repeat = dz
        .call("POST", &fx.filters_uri(&fx.with_schema), Some(body))
        .await;
    assert_eq!(
        repeat.body, first.body,
        "a repeated clientToken must answer with the filter the first call created"
    );

    // The same token on ANOTHER asset is another filter: the token is scoped to the
    // asset the filter hangs off.
    let elsewhere = dz
        .call(
            "POST",
            &fx.filters_uri(&fx.without_schema),
            Some(json!({
                "name": "token view",
                "clientToken": "a-caller-chosen-token",
                "configuration": { "columnConfiguration": { "includedColumnNames": ["amount"] } },
            })),
        )
        .await;
    assert_eq!(
        elsewhere.status, 201,
        "the same token on another asset: expected 201, got {} {:?}",
        elsewhere.status, elsewhere.body
    );
    assert_ne!(
        elsewhere.s("id"),
        first.s("id"),
        "the same token on another asset must create that asset's own filter"
    );
}
