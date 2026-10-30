// SPDX-License-Identifier: CC0-1.0

//! `DescribeBrokerInstanceOptions`, driven over the wire.
//!
//! RULE: the operation reports Amazon MQ's published instance-option catalog — one entry
//! per (engine type, host instance type, storage type) combination the service offers,
//! each carrying the deployment modes and engine versions that combination supports and
//! the Availability Zones of the Region the caller signed for — narrowed by every filter
//! member the request set and paged by `maxResults` with a round-trippable `nextToken`.
//! Because the catalog describes the SERVICE and not the account, the answer does not
//! depend on anything a caller created and a call changes nothing a later caller sees.
//!
//! The tables span what the rule generalises over rather than one recorded request: each
//! filter member crossed with every value the model enumerates for it, with the values
//! the catalog itself publishes for the free-form one and with a value it does not
//! publish; the published bounds of `maxResults` and one past each; and several Regions.
//! The expected answer of a filtered call is always derived from the UNFILTERED response
//! read over the same wire, so no entry is written out here.
//!
//! The value catalogs the tables span are the model's own enumerations —
//! `smithy_query(query="shape", shape="EngineType"|"BrokerStorageType"|"DeploymentMode")`,
//! which the crate re-exports as `ENGINE_TYPE_VALUES`, `BROKER_STORAGE_TYPE_VALUES` and
//! `DEPLOYMENT_MODE_VALUES`.

use serde_json::{json, Value};

use service_com_amazonaws_mq_mq::ops::types::{
    BROKER_STORAGE_TYPE_VALUES, DEPLOYMENT_MODE_VALUES, ENGINE_TYPE_VALUES,
};
use service_com_amazonaws_mq_mq::state::{
    invalid_page_token_message, availability_zones_in_region, DEFAULT_MAX_RESULTS,
    DEPLOYMENT_MODE_SINGLE_INSTANCE, ENGINE_TYPE_RABBITMQ, MAX_MAX_RESULTS, MIN_MAX_RESULTS,
    STORAGE_TYPE_EFS,
};

use crate::broker_lifecycle::{create_body, Mq};

/// The operation's modelled path (`smithy_query(query="operation",
/// operation="DescribeBrokerInstanceOptions")`: `GET /v1/broker-instance-options`).
const PATH: &str = "/v1/broker-instance-options";

/// One `DescribeBrokerInstanceOptions` call, with `query` as its query string.
async fn describe(mq: &Mq, query: &str) -> (u16, Value) {
    let path = if query.is_empty() {
        PATH.to_string()
    } else {
        format!("{PATH}?{query}")
    };
    mq.call("GET", &path, None).await
}

/// The same call, also reporting the error code a restJson1 client branches on.
async fn describe_with_code(mq: &Mq, query: &str) -> (u16, String, Value) {
    let path = if query.is_empty() {
        PATH.to_string()
    } else {
        format!("{PATH}?{query}")
    };
    mq.call_with_code("GET", &path, None).await
}

/// The entries of a successful answer.
fn entries(case: &str, body: &Value) -> Vec<Value> {
    let Some(entries) = body.get("brokerInstanceOptions").and_then(Value::as_array) else {
        panic!("case {case:?}: expected a brokerInstanceOptions array, got {body:?}");
    };
    entries.clone()
}

/// One entry's string member.
fn member(case: &str, entry: &Value, name: &str) -> String {
    let Some(value) = entry.get(name).and_then(Value::as_str) else {
        panic!("case {case:?}: expected a string {name}, got {entry:?}");
    };
    value.to_string()
}

/// One entry's list-of-string member.
fn members(case: &str, entry: &Value, name: &str) -> Vec<String> {
    let Some(values) = entry.get(name).and_then(Value::as_array) else {
        panic!("case {case:?}: expected a {name} array, got {entry:?}");
    };
    values
        .iter()
        .map(|value| {
            let Some(value) = value.as_str() else {
                panic!("case {case:?}: expected {name} to hold strings, got {value:?}");
            };
            value.to_string()
        })
        .collect()
}

/// The whole catalog of `region`, read over the wire in one page.
async fn whole_catalog(case: &str, mq: &Mq) -> Vec<Value> {
    let (status, body) = describe(mq, &format!("maxResults={MAX_MAX_RESULTS}")).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 for the unfiltered catalog, got {status} {body:?}"
    );
    assert!(
        body.get("nextToken").is_none(),
        "case {case:?}: expected the whole catalog to fit one page of {MAX_MAX_RESULTS}, \
         got a nextToken in {body:?}"
    );
    entries(case, &body)
}

#[tokio::test]
async fn the_catalog_is_well_formed_and_covers_every_modelled_engine_type() {
    let region = "us-east-1";
    let mq = Mq::new(region);
    let case = "unfiltered";
    let catalog = whole_catalog(case, &mq).await;

    assert!(
        !catalog.is_empty(),
        "case {case:?}: expected the published catalog to hold entries, got {catalog:?}"
    );

    let expected_zones = availability_zones_in_region(region);
    let mut keys: Vec<(String, String, String)> = Vec::new();
    for entry in &catalog {
        let engine_type = member(case, entry, "engineType");
        let host_instance_type = member(case, entry, "hostInstanceType");
        let storage_type = member(case, entry, "storageType");
        let case = &format!("{engine_type}/{host_instance_type}/{storage_type}");

        assert!(
            ENGINE_TYPE_VALUES.contains(&engine_type.as_str()),
            "case {case:?}: engineType must be one of {ENGINE_TYPE_VALUES:?}, got \
             {engine_type:?}"
        );
        assert!(
            BROKER_STORAGE_TYPE_VALUES.contains(&storage_type.as_str()),
            "case {case:?}: storageType must be one of {BROKER_STORAGE_TYPE_VALUES:?}, got \
             {storage_type:?}"
        );
        // `BrokerStorageType`'s own documentation: "EFS is not supported for RabbitMQ
        // engine type".
        assert!(
            engine_type != ENGINE_TYPE_RABBITMQ || storage_type != STORAGE_TYPE_EFS,
            "case {case:?}: {ENGINE_TYPE_RABBITMQ} is not offered on {STORAGE_TYPE_EFS}, got \
             {entry:?}"
        );

        let modes = members(case, entry, "supportedDeploymentModes");
        assert!(
            modes.contains(&DEPLOYMENT_MODE_SINGLE_INSTANCE.to_string()),
            "case {case:?}: every option supports {DEPLOYMENT_MODE_SINGLE_INSTANCE}, got \
             {modes:?}"
        );
        for mode in &modes {
            assert!(
                DEPLOYMENT_MODE_VALUES.contains(&mode.as_str()),
                "case {case:?}: supportedDeploymentModes must hold modes from \
                 {DEPLOYMENT_MODE_VALUES:?}, got {mode:?}"
            );
        }

        let versions = members(case, entry, "supportedEngineVersions");
        assert!(
            !versions.is_empty(),
            "case {case:?}: expected at least one supported engine version, got {versions:?}"
        );

        let Some(zones) = entry.get("availabilityZones").and_then(Value::as_array) else {
            panic!("case {case:?}: expected an availabilityZones array, got {entry:?}");
        };
        let zone_names: Vec<String> = zones
            .iter()
            .map(|zone| member(case, zone, "name"))
            .collect();
        assert_eq!(
            zone_names, expected_zones,
            "case {case:?}: the zones must be the signed Region's, expected \
             {expected_zones:?}, got {zone_names:?}"
        );

        keys.push((engine_type, host_instance_type, storage_type));
    }

    let mut distinct = keys.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        keys.len(),
        "case {case:?}: every (engineType, hostInstanceType, storageType) appears once, \
         got {keys:?}"
    );

    for engine_type in ENGINE_TYPE_VALUES {
        assert!(
            keys.iter().any(|(engine, _, _)| engine == engine_type),
            "case {engine_type:?}: the catalog must offer options for every modelled engine \
             type, got {keys:?}"
        );
    }
}

#[tokio::test]
async fn each_filter_selects_exactly_the_matching_catalog_entries() {
    let mq = Mq::new("us-east-1");
    let catalog = whole_catalog("unfiltered", &mq).await;

    // The free-form filter's values are the ones the catalog itself publishes, plus a
    // host instance type it does not publish (an `mq.m5` size outside the offered set),
    // which must select nothing rather than everything.
    let mut host_instance_types: Vec<String> = catalog
        .iter()
        .map(|entry| member("unfiltered", entry, "hostInstanceType"))
        .collect();
    host_instance_types.sort();
    host_instance_types.dedup();
    host_instance_types.push("mq.m5.24xlarge".to_string());

    let engine_filters: Vec<Option<String>> = std::iter::once(None)
        .chain(ENGINE_TYPE_VALUES.iter().map(|v| Some((*v).to_string())))
        .collect();
    let storage_filters: Vec<Option<String>> = std::iter::once(None)
        .chain(BROKER_STORAGE_TYPE_VALUES.iter().map(|v| Some((*v).to_string())))
        .collect();
    let host_filters: Vec<Option<String>> = std::iter::once(None)
        .chain(host_instance_types.into_iter().map(Some))
        .collect();

    for engine_filter in &engine_filters {
        for storage_filter in &storage_filters {
            for host_filter in &host_filters {
                let mut query = vec![format!("maxResults={MAX_MAX_RESULTS}")];
                if let Some(engine) = engine_filter {
                    query.push(format!("engineType={engine}"));
                }
                if let Some(storage) = storage_filter {
                    query.push(format!("storageType={storage}"));
                }
                if let Some(host) = host_filter {
                    query.push(format!("hostInstanceType={host}"));
                }
                let query = query.join("&");
                let case = &query;

                let expected: Vec<Value> = catalog
                    .iter()
                    .filter(|entry| {
                        [
                            ("engineType", engine_filter),
                            ("storageType", storage_filter),
                            ("hostInstanceType", host_filter),
                        ]
                        .iter()
                        .all(|(name, filter)| match filter {
                            None => true,
                            Some(filter) => &member(case, entry, name) == filter,
                        })
                    })
                    .cloned()
                    .collect();

                let (status, body) = describe(&mq, &query).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected 200, got {status} {body:?}"
                );
                let got = entries(case, &body);
                assert_eq!(
                    got, expected,
                    "case {case:?}: expected the {} matching entries, got {}",
                    expected.len(),
                    got.len()
                );
            }
        }
    }
}

#[tokio::test]
async fn pagination_walks_every_entry_exactly_once() {
    let mq = Mq::new("eu-west-1");
    let catalog = whole_catalog("unfiltered", &mq).await;

    // The published bounds of `maxResults` (1 and 100) and sizes between them, so the
    // walk is tested where a page is one entry, several entries and the whole catalog.
    let sizes = [MIN_MAX_RESULTS, 2, 5, MAX_MAX_RESULTS];
    for size in sizes {
        let case = &format!("maxResults={size}");
        let mut seen: Vec<Value> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match &token {
                None => format!("maxResults={size}"),
                Some(token) => format!("maxResults={size}&nextToken={token}"),
            };
            let (status, body) = describe(&mq, &query).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 on page {pages}, got {status} {body:?}"
            );
            assert_eq!(
                body.get("maxResults").and_then(Value::as_i64),
                Some(i64::from(size)),
                "case {case:?}: the answer reports the page size it used, got {body:?}"
            );
            let page = entries(case, &body);
            assert!(
                page.len() <= usize::try_from(size).unwrap_or_default(),
                "case {case:?}: a page holds at most {size} entries, got {}",
                page.len()
            );
            seen.extend(page);
            pages += 1;
            assert!(
                pages <= catalog.len() + 1,
                "case {case:?}: the walk must terminate, got {pages} pages for {} entries",
                catalog.len()
            );
            match body.get("nextToken").and_then(Value::as_str) {
                None => break,
                Some(next) => token = Some(next.to_string()),
            }
        }
        assert_eq!(
            seen, catalog,
            "case {case:?}: paging must report every entry exactly once, in the same order"
        );
    }

    // The default page size, from the output member's own documentation ("20 by
    // default").
    let case = "no maxResults";
    let (status, body) = describe(&mq, "").await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200, got {status} {body:?}"
    );
    assert_eq!(
        body.get("maxResults").and_then(Value::as_i64),
        Some(i64::try_from(DEFAULT_MAX_RESULTS).unwrap_or_default()),
        "case {case:?}: the answer reports the documented default page size, got {body:?}"
    );
    let page = entries(case, &body);
    assert_eq!(
        page,
        catalog[..catalog.len().min(DEFAULT_MAX_RESULTS)].to_vec(),
        "case {case:?}: the first page is the catalog's first {DEFAULT_MAX_RESULTS} entries"
    );
}

#[tokio::test]
async fn values_outside_the_published_bounds_and_sets_are_refused() {
    let mq = Mq::new("us-east-1");

    // Each row: the query, the member the refusal must name, and the sentence it must
    // carry. The enum values are built FROM the model's sets rather than copied from a
    // request, and the bounds are the published ones and one past each.
    let engine_outside = format!("{}_NOT_AN_ENGINE", ENGINE_TYPE_VALUES[0]);
    let storage_outside = format!("{}_NOT_A_STORAGE", BROKER_STORAGE_TYPE_VALUES[0]);
    let bogus_token = "not-a-token-this-service-issued";
    let rows: Vec<(String, &str, Option<String>)> = vec![
        (
            format!("maxResults={}", MIN_MAX_RESULTS - 1),
            "maxResults",
            None,
        ),
        (
            format!("maxResults={}", MAX_MAX_RESULTS + 1),
            "maxResults",
            None,
        ),
        (
            format!("engineType={engine_outside}"),
            "engineType",
            Some(format!(
                "Broker engine type [{engine_outside}] is invalid. Valid values are: [{}]",
                ENGINE_TYPE_VALUES.join(", ")
            )),
        ),
        (
            format!("storageType={storage_outside}"),
            "storageType",
            Some(format!(
                "Broker storage type [{storage_outside}] is invalid. Valid values are: [{}]",
                BROKER_STORAGE_TYPE_VALUES.join(", ")
            )),
        ),
        (
            format!("nextToken={bogus_token}"),
            "nextToken",
            Some(invalid_page_token_message(bogus_token)),
        ),
    ];

    for (query, attribute, message) in &rows {
        let case = query;
        let (status, code, body) = describe_with_code(&mq, query).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body:?}"
        );
        assert_eq!(
            code, "BadRequestException",
            "case {case:?}: expected the modelled error code, got {code:?} {body:?}"
        );
        assert_eq!(
            body.get("errorAttribute").and_then(Value::as_str),
            Some(*attribute),
            "case {case:?}: the refusal must name the member at fault, got {body:?}"
        );
        if let Some(message) = message {
            assert_eq!(
                body.get("message").and_then(Value::as_str),
                Some(message.as_str()),
                "case {case:?}: unexpected sentence in {body:?}"
            );
        }
    }

    // The published bounds themselves are accepted.
    for size in [MIN_MAX_RESULTS, MAX_MAX_RESULTS] {
        let query = format!("maxResults={size}");
        let (status, body) = describe(&mq, &query).await;
        assert_eq!(
            status, 200,
            "case {query:?}: a value on the published bound is accepted, got {status} {body:?}"
        );
    }
}

#[tokio::test]
async fn the_catalog_follows_the_region_and_not_the_account() {
    // Every Region offers the same options; only the zone names differ, because a zone
    // name is its Region's code plus a letter.
    let mut without_zones: Option<Vec<Value>> = None;
    for region in ["us-east-1", "eu-west-1", "ap-southeast-2"] {
        let mq = Mq::new(region);
        let catalog = whole_catalog(region, &mq).await;
        let expected_zones: Vec<Value> = availability_zones_in_region(region)
            .into_iter()
            .map(|name| json!({"name": name}))
            .collect();
        for entry in &catalog {
            assert_eq!(
                entry.get("availabilityZones"),
                Some(&Value::Array(expected_zones.clone())),
                "case {region:?}: the zones must be named for the signed Region, got {entry:?}"
            );
        }
        let stripped: Vec<Value> = catalog
            .iter()
            .map(|entry| {
                let mut entry = entry.clone();
                entry
                    .as_object_mut()
                    .expect("an entry is a JSON object")
                    .remove("availabilityZones");
                entry
            })
            .collect();
        match &without_zones {
            None => without_zones = Some(stripped),
            Some(first) => assert_eq!(
                &stripped, first,
                "case {region:?}: the offered options do not differ by Region"
            ),
        }
    }

    // The catalog describes the service, so what the account holds cannot change it,
    // and reading it changes nothing a later caller sees.
    let mq = Mq::new("us-east-1");
    let before = whole_catalog("before a broker exists", &mq).await;
    let (status, created) = mq.create_broker(create_body("catalog-witness")).await;
    assert_eq!(
        status, 200,
        "case \"create a broker\": expected 200, got {status} {created:?}"
    );
    let after = whole_catalog("after a broker exists", &mq).await;
    assert_eq!(
        after, before,
        "case \"after a broker exists\": the catalog must not depend on the account's brokers"
    );
    let again = whole_catalog("read twice", &mq).await;
    assert_eq!(
        again, after,
        "case \"read twice\": a read must not change what a later caller sees"
    );
}
