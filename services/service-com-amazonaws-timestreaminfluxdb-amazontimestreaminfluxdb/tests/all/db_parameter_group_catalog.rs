// SPDX-License-Identifier: CC0-1.0

//! The service-managed DB parameter group catalog, end to end over the wire.
//!
//! The suite drives real `awsJson1_0` requests through the same [`MultiServiceRouter`] the
//! production router dispatches with, reusing the harness in
//! [`crate::db_instance_lifecycle`] (`router`, `call`, `call_in`, `ok`) so there is one
//! wire harness for this crate.
//!
//! The rule: a set of DB parameter groups exists in every account and Region without
//! anyone creating them, so every reader reports them — `ListDbParameterGroups` lists them
//! ahead of the customer-created groups, `GetDbParameterGroup` resolves each one by its
//! identifier, and their names are taken for `CreateDbParameterGroup`. Each such group is
//! not an account-scoped resource: its `id` IS its `name` and it carries no ARN.
//!
//! The assertions are a TABLE over the whole catalog, not a replay of one recorded call:
//! the rows span both engine groups and the per-instance-size defaults of both v3 engine
//! families, every size the model declares (including the two largest, which no issue
//! cited), the Region the request is signed into, the page size at both published bounds
//! of `MaxResults`, and the presence of customer-created groups beside the catalog.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_timestreaminfluxdb_amazontimestreaminfluxdb as influxdb;
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instance_lifecycle::{call, call_in, ok, router};

/// The `db.influx.` prefix every DB instance type carries
/// (`smithy_query(query="shape", shape="DbInstanceType")`).
const DB_INSTANCE_TYPE_PREFIX: &str = "db.influx.";

/// The size token each per-instance-size service-managed default group's name carries, in
/// the order the modelled `DbInstanceType` enum declares its values — ascending size.
///
/// From `smithy_query(query="shape", shape="DbInstanceType")`: db.influx.medium,
/// db.influx.large, db.influx.xlarge, db.influx.2xlarge, db.influx.4xlarge,
/// db.influx.8xlarge, db.influx.12xlarge, db.influx.16xlarge, db.influx.24xlarge — each
/// with the `db.influx.` prefix dropped and its first character upper-cased.
const SIZE_TOKENS: &[&str] = &[
    "Medium",
    "Large",
    "Xlarge",
    "2xlarge",
    "4xlarge",
    "8xlarge",
    "12xlarge",
    "16xlarge",
    "24xlarge",
];

/// The v3 engine families of the modelled `Parameters` union
/// (`smithy_query(query="shape", shape="Parameters")`), as the token a group name carries
/// and the union member a group of that family configures.
const V3_FAMILIES: &[(&str, &str)] = &[
    ("Core", "InfluxDBv3Core"),
    ("Enterprise", "InfluxDBv3Enterprise"),
];

/// One expected catalog entry: the group's name (which is also its id) and its
/// description.
struct CatalogEntry {
    name: String,
    description: String,
    /// The `Parameters` union member a group of this engine family configures.
    parameters_member: &'static str,
}

/// The catalog this emulator must report, in order: the engine groups a create names to
/// start a v3 engine, then one per-instance-size default for each v3 engine family.
fn expected_catalog() -> Vec<CatalogEntry> {
    let mut expected = vec![
        CatalogEntry {
            name: "InfluxDBV3Core".to_string(),
            description: "Service Parameter group to create InfluxDB v3 core engine".to_string(),
            parameters_member: "InfluxDBv3Core",
        },
        CatalogEntry {
            name: "InfluxDBV3Enterprise".to_string(),
            description: "Service Parameter group to create InfluxDB v3 enterprise engine"
                .to_string(),
            parameters_member: "InfluxDBv3Enterprise",
        },
        CatalogEntry {
            name: "InfluxDBV3Enterprise1Node".to_string(),
            description:
                "Service Parameter group to create InfluxDB v3 enterprise engine with 1 node"
                    .to_string(),
            parameters_member: "InfluxDBv3Enterprise",
        },
    ];
    for (family, parameters_member) in V3_FAMILIES {
        for size in SIZE_TOKENS {
            let name = format!("InfluxDBV3{family}{size}Default");
            expected.push(CatalogEntry {
                description: format!("Service-managed parameter group: {name}"),
                name,
                parameters_member,
            });
        }
    }
    expected
}

/// The `items` of a list answer.
fn items_of(case: &str, body: &Value) -> Vec<Value> {
    body["items"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected an `items` list, got {body}"))
        .clone()
}

/// One whole page of `ListDbParameterGroups` in `region`, at the modelled maximum page
/// size.
async fn list_in(router: &MultiServiceRouter, region: &str, case: &str) -> Vec<Value> {
    let body = ok(
        case,
        call_in(
            router,
            region,
            "ListDbParameterGroups",
            json!({ "maxResults": 100 }),
        )
        .await,
    );
    items_of(case, &body)
}

/// The size token a DB instance type contributes to a default group's name: the type
/// without its `db.influx.` prefix, first character upper-cased.
fn size_token(db_instance_type: &str) -> String {
    let size = db_instance_type
        .strip_prefix(DB_INSTANCE_TYPE_PREFIX)
        .unwrap_or(db_instance_type);
    let mut characters = size.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().chain(characters).collect(),
        None => String::new(),
    }
}

#[tokio::test]
async fn the_size_tokens_cover_every_modelled_db_instance_type() {
    // The hand-written [`SIZE_TOKENS`] above is the modelled `DbInstanceType` enum in
    // declaration order; this pins it against the generated value list, so a model that
    // adds or drops an instance type fails here instead of silently shrinking the catalog.
    let modelled: BTreeSet<String> = influxdb::ops::types::DB_INSTANCE_TYPE_VALUES
        .iter()
        .map(|db_instance_type| size_token(db_instance_type))
        .collect();
    let written: BTreeSet<String> = SIZE_TOKENS.iter().map(|size| (*size).to_string()).collect();
    assert_eq!(
        written, modelled,
        "case \"size tokens\": expected the modelled DbInstanceType sizes {modelled:?}, \
         got {written:?}"
    );
}

#[tokio::test]
async fn a_fresh_account_lists_the_service_managed_catalog_in_order() {
    let router = router();
    let items = list_in(&router, "us-west-2", "a fresh account").await;
    let expected = expected_catalog();

    assert_eq!(
        items.len(),
        expected.len(),
        "case \"a fresh account\": expected {} service-managed groups, got {}: {items:?}",
        expected.len(),
        items.len()
    );

    for (position, entry) in expected.iter().enumerate() {
        let case = format!("catalog entry {position} ({})", entry.name);
        let summary = &items[position];
        // A service-managed group is not an account-scoped resource: its id IS its name
        // and it carries no ARN.
        assert_eq!(
            summary["id"].as_str(),
            Some(entry.name.as_str()),
            "case {case:?}: expected id {:?}, got {:?}",
            entry.name,
            summary["id"]
        );
        assert_eq!(
            summary["name"].as_str(),
            Some(entry.name.as_str()),
            "case {case:?}: expected name {:?}, got {:?}",
            entry.name,
            summary["name"]
        );
        assert_eq!(
            summary["arn"].as_str(),
            Some(""),
            "case {case:?}: expected an empty arn, got {:?}",
            summary["arn"]
        );
        assert_eq!(
            summary["description"].as_str(),
            Some(entry.description.as_str()),
            "case {case:?}: expected description {:?}, got {:?}",
            entry.description,
            summary["description"]
        );
    }
}

#[tokio::test]
async fn every_catalog_group_resolves_through_the_reader_in_every_region() {
    let router = router();
    // The catalog is the service's own, so it is identical in every Region, including one
    // no request has otherwise touched.
    for region in ["us-west-2", "eu-central-1", "ap-southeast-2"] {
        let listed = list_in(&router, region, region).await;
        for entry in expected_catalog() {
            let case = format!("{} in {region}", entry.name);
            let summary = listed
                .iter()
                .find(|item| item["id"].as_str() == Some(entry.name.as_str()))
                .unwrap_or_else(|| panic!("case {case:?}: {} is missing from {listed:?}", entry.name));

            let fetched = ok(
                &case,
                call_in(
                    &router,
                    region,
                    "GetDbParameterGroup",
                    json!({ "identifier": &entry.name }),
                )
                .await,
            );
            // The reader answers the same record the summary projects.
            for member in ["arn", "description", "id", "name"] {
                assert_eq!(
                    &fetched[member], &summary[member],
                    "case {case:?}: the reader disagrees with the summary on {member:?}: \
                     expected {:?}, got {:?}",
                    summary[member], fetched[member]
                );
            }
            // A group configures exactly one engine parameter family: the one its name
            // names.
            assert!(
                fetched["parameters"]
                    .get(entry.parameters_member)
                    .is_some(),
                "case {case:?}: expected the {} parameter family, got {:?}",
                entry.parameters_member,
                fetched["parameters"]
            );

            // A read changes nothing a later caller sees.
            let again = ok(
                &case,
                call_in(
                    &router,
                    region,
                    "GetDbParameterGroup",
                    json!({ "identifier": &entry.name }),
                )
                .await,
            );
            assert_eq!(
                again, fetched,
                "case {case:?}: a second read must report the same record, got {again}"
            );
        }
    }
}

#[tokio::test]
async fn paging_over_the_catalog_yields_it_in_order_exactly_once() {
    let router = router();
    // `MaxResults` is modelled range 1..100, so the rows span both published bounds and
    // sizes between them, plus an omitted page size.
    let page_sizes: [Option<i64>; 5] = [Some(1), Some(4), Some(7), Some(100), None];
    let expected: Vec<String> = expected_catalog()
        .into_iter()
        .map(|entry| entry.name)
        .collect();

    for max_results in page_sizes {
        let case = format!("the catalog at maxResults {max_results:?}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "case {case:?}: paging did not terminate after {pages} pages, saw {seen:?}"
            );
            let mut request = json!({});
            if let Some(limit) = max_results {
                request["maxResults"] = json!(limit);
            }
            if let Some(token) = &token {
                request["nextToken"] = json!(token);
            }
            let page = ok(&case, call(&router, "ListDbParameterGroups", request).await);
            let items = items_of(&case, &page);
            if let Some(limit) = max_results {
                assert!(
                    items.len() as i64 <= limit,
                    "case {case:?}: a page of {} exceeds maxResults {limit}",
                    items.len()
                );
            }
            for item in &items {
                seen.push(
                    item["id"]
                        .as_str()
                        .unwrap_or_else(|| panic!("case {case:?}: expected an id, got {item}"))
                        .to_string(),
                );
            }
            match page.get("nextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen, expected,
            "case {case:?}: paging must yield the whole catalog in catalog order, got {seen:?}"
        );
    }
}

#[tokio::test]
async fn the_catalog_is_reported_ahead_of_the_customer_created_groups() {
    let router = router();
    let catalog: Vec<String> = expected_catalog()
        .into_iter()
        .map(|entry| entry.name)
        .collect();

    // A table of customer creates beside the catalog: a short name, a hyphenated one and
    // one at the modelled maximum length 64.
    let mut created: Vec<String> = Vec::new();
    let names = [
        "Abc".to_string(),
        "pg-beside-catalog".to_string(),
        format!("A{}", "b".repeat(63)),
    ];
    for name in names {
        let case = format!("created {name:?}");
        let group = ok(
            &case,
            call(&router, "CreateDbParameterGroup", json!({ "name": &name })).await,
        );
        created.push(
            group["id"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected an id, got {group}"))
                .to_string(),
        );

        let listed = list_in(&router, "us-west-2", &case).await;
        let listed_ids: Vec<String> = listed
            .iter()
            .map(|item| item["id"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            &listed_ids[..catalog.len()],
            &catalog[..],
            "case {case:?}: the catalog must still be reported first, got {listed_ids:?}"
        );
        let mut rest = listed_ids[catalog.len()..].to_vec();
        rest.sort();
        let mut expected_rest = created.clone();
        expected_rest.sort();
        assert_eq!(
            rest, expected_rest,
            "case {case:?}: expected the created groups after the catalog, got {rest:?}"
        );
    }
}

#[tokio::test]
async fn a_create_under_a_service_managed_name_conflicts() {
    let router = router();
    // A parameter group name is unique per customer and per region, and a service-managed
    // name is taken in every region: one engine group, one per-size default of each
    // family, and one of the sizes the recorded catalog did not cite.
    for name in [
        "InfluxDBV3Core",
        "InfluxDBV3Enterprise1Node",
        "InfluxDBV3CoreMediumDefault",
        "InfluxDBV3Enterprise24xlargeDefault",
    ] {
        for region in ["us-west-2", "eu-central-1"] {
            let case = format!("{name:?} in {region}");
            let (status, body) = call_in(
                &router,
                region,
                "CreateDbParameterGroup",
                json!({ "name": name }),
            )
            .await;
            assert_eq!(
                status, 409,
                "case {case:?}: expected HTTP 409, got {status} with {body}"
            );
            assert_eq!(
                body["resourceId"].as_str(),
                Some(name),
                "case {case:?}: the conflict must name the existing group, got {body}"
            );
        }
    }
}
