// SPDX-License-Identifier: CC0-1.0

//! `ListRegistries`: the schema registries this endpoint holds.
//!
//! RULE: the operation "returns a list of registries that you have created, with minimal
//! registry information", so it reports every registry the store holds — the same records
//! `CreateRegistry` wrote and `GetRegistry` reads back, member for member — one
//! `MaxResults`-sized page at a time with a `NextToken` that resumes the walk, leaving out
//! a registry in the `DELETING` status, and answering the EMPTY list when the store holds
//! none.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rows differ along what the rule generalises over: how many registries are stored,
//! the page size asked for (including each published bound and a size past the stored
//! count), which members each stored registry carries, the status it is in, and the Region
//! the listing is served in.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::{
    ComAmazonawsGlueAwsglueState, REGISTRY_STATUS_AVAILABLE, REGISTRY_STATUS_DELETING,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, call_in_region, emulator, emulator_on, ok, ACCOUNT_ID, REGION};

/// The page size a request that names no `MaxResults` is served.
///
/// Source: `smithy_query operation=ListRegistries` — `MaxResults` is documented "Maximum
/// number of results required per page. If the value is not supplied, this will be
/// defaulted to 25 per page".
const DEFAULT_PAGE_SIZE: usize = 25;

/// The published bounds of `MaxResults` (`MaxResultsNumber`, range 1..100), from the same
/// call.
const MIN_MAX_RESULTS: i32 = 1;
const MAX_MAX_RESULTS: i32 = 100;

/// The ARN of registry `name` as the endpoint of `region` mints it.
fn registry_arn(region: &str, name: &str) -> String {
    format!("arn:aws:glue:{region}:{ACCOUNT_ID}:registry/{name}")
}

/// Create one registry, with the description the row asks for.
async fn given_registry(router: &MultiServiceRouter, name: &str, description: Option<&str>) {
    let mut body = json!({ "RegistryName": name });
    if let Some(description) = description {
        body["Description"] = json!(description);
    }
    ok(router, "CreateRegistry", &body).await;
}

/// The `RegistryName` of every entry of one answer, in the order the service reported them.
fn listed_names(page: &Value) -> Vec<String> {
    page["Registries"]
        .as_array()
        .unwrap_or_else(|| panic!("Registries must be a list, got {page}"))
        .iter()
        .map(|entry| {
            entry["RegistryName"]
                .as_str()
                .unwrap_or_else(|| panic!("RegistryListItem.RegistryName is a string, got {entry}"))
                .to_string()
        })
        .collect()
}

/// Walk every page the service hands out for `base`, following its own `NextToken`, and
/// return the entries in the order they were reported.
async fn walk(
    router: &MultiServiceRouter,
    case: &str,
    base: &Value,
    page_size: Option<i32>,
    limit: usize,
) -> Vec<Value> {
    let mut listed: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut request = base.clone();
        if let Some(size) = page_size {
            request["MaxResults"] = json!(size);
        }
        if let Some(cursor) = &token {
            request["NextToken"] = json!(cursor);
        }
        let page = ok(router, "ListRegistries", &request).await;
        let entries = page["Registries"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: Registries must be a list, got {page}"))
            .clone();
        let expected_page = page_size
            .and_then(|size| usize::try_from(size).ok())
            .unwrap_or(DEFAULT_PAGE_SIZE);
        assert!(
            entries.len() <= expected_page,
            "case {case:?}: a page must hold at most {expected_page} entries, got {}",
            entries.len()
        );
        listed.extend(entries);
        token = page
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if token.is_none() {
            break;
        }
        assert!(
            listed.len() <= limit,
            "case {case:?}: the walk must terminate, got {} entries for {limit} registries",
            listed.len()
        );
    }
    listed
}

/// Every stored registry is listed, in name order, one page at a time — including the
/// store that holds none, which answers the empty list rather than a refusal.
#[tokio::test]
async fn every_stored_registry_is_listed_and_paged() {
    // (case, how many registries are stored, the page size asked for)
    let rows: &[(&str, usize, Option<i32>)] = &[
        ("no registries at all", 0, None),
        ("one registry, no page size", 1, None),
        (
            "the smallest page the model accepts",
            5,
            Some(MIN_MAX_RESULTS),
        ),
        ("a page smaller than the stored count", 7, Some(3)),
        ("a page larger than the stored count", 3, Some(50)),
        (
            "the largest page the model accepts",
            4,
            Some(MAX_MAX_RESULTS),
        ),
    ];

    for (case, stored, page_size) in rows {
        let router = emulator();
        // Created out of name order, so the listing's own order is what is asserted.
        let expected: Vec<String> = (0..*stored)
            .map(|index| format!("registry-{index:02}"))
            .collect();
        for name in expected.iter().rev() {
            given_registry(&router, name, None).await;
        }

        let listed = walk(&router, case, &json!({}), *page_size, *stored).await;
        let names: Vec<String> = listed
            .iter()
            .map(|entry| entry["RegistryName"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            names, expected,
            "case {case:?}: expected every stored registry in name order, got {names:?}"
        );

        // A registry the listing reports is the one `GetRegistry` resolves, member for
        // member: one record, two reads.
        for entry in &listed {
            let name = entry["RegistryName"].as_str().unwrap_or_default();
            let read = ok(
                &router,
                "GetRegistry",
                &json!({ "RegistryId": { "RegistryName": name } }),
            )
            .await;
            for member in [
                "RegistryName",
                "RegistryArn",
                "Description",
                "Status",
                "CreatedTime",
                "UpdatedTime",
            ] {
                assert_eq!(
                    entry.get(member),
                    read.get(member),
                    "case {case:?}: the listed {member} of {name} must be what GetRegistry \
                     reports, got {entry} and {read}"
                );
            }
        }
    }
}

/// A request that names no `MaxResults` is served the documented default page, and the
/// token it carries resumes the walk over the rest.
#[tokio::test]
async fn a_request_naming_no_page_size_is_served_the_documented_default_page() {
    let router = emulator();
    let stored = DEFAULT_PAGE_SIZE + 5;
    let expected: Vec<String> = (0..stored)
        .map(|index| format!("registry-{index:02}"))
        .collect();
    for name in &expected {
        given_registry(&router, name, None).await;
    }

    let first = ok(&router, "ListRegistries", &json!({})).await;
    let names = listed_names(&first);
    assert_eq!(
        names.len(),
        DEFAULT_PAGE_SIZE,
        "case no MaxResults: expected the documented default page of {DEFAULT_PAGE_SIZE}, got \
         {} entries",
        names.len()
    );
    assert_eq!(
        names,
        expected[..DEFAULT_PAGE_SIZE].to_vec(),
        "case no MaxResults: expected the first page in name order, got {names:?}"
    );
    let token = first
        .get("NextToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case no MaxResults: a truncated listing carries a token, got {first}"));

    let second = ok(&router, "ListRegistries", &json!({ "NextToken": token })).await;
    assert_eq!(
        listed_names(&second),
        expected[DEFAULT_PAGE_SIZE..].to_vec(),
        "case the continuation call: expected the rest of the registries, got {second}"
    );
    assert!(
        second.get("NextToken").is_none(),
        "case the last page: the last segment carries no token, got {second}"
    );
}

/// A listed registry carries the minimal details of the record the create stored: the
/// `Description` it sent (and none when it sent none), the status of a registry this store
/// holds, the ARN minted from the serving Region and the name, and two timestamps that
/// agree because nothing has updated it.
#[tokio::test]
async fn a_listed_registry_carries_the_members_the_create_stored() {
    let router = emulator();

    // The rows differ along which members the stored registry carries and over the
    // character classes and lengths the name's pattern admits.
    let rows: &[(&str, &str, Option<&str>)] = &[
        (
            "described",
            "aaa-described",
            Some("schemas whose compatibility is enforced"),
        ),
        ("no Description", "bbb-bare", None),
        // `DescriptionString` is length 0..2048, so "" is a description the model admits
        // and is not the same input as sending none.
        ("an empty Description", "ccc-empty", Some("")),
        (
            "every character class the name pattern admits",
            "ddd_registry-01$dev#north.east",
            Some("punctuation the pattern admits"),
        ),
        // The lower published bound on `SchemaRegistryNameString`'s length.
        ("a name of the shortest admitted length", "e", None),
    ];

    for (_, name, description) in rows {
        given_registry(&router, name, *description).await;
    }

    let listed = walk(&router, "the members", &json!({}), None, rows.len()).await;
    assert_eq!(
        listed.len(),
        rows.len(),
        "case the members: expected {} registries, got {listed:?}",
        rows.len()
    );

    // The rows were created in name order, which is the order the listing reports.
    for ((case, name, description), entry) in rows.iter().zip(listed.iter()) {
        assert_eq!(
            entry.get("RegistryName").and_then(Value::as_str),
            Some(*name),
            "case {case:?}: expected RegistryName {name}, got {entry}"
        );
        assert_eq!(
            entry.get("RegistryArn").and_then(Value::as_str),
            Some(registry_arn(REGION, name).as_str()),
            "case {case:?}: expected the registry's ARN, got {entry}"
        );
        assert_eq!(
            entry.get("Description").and_then(Value::as_str),
            *description,
            "case {case:?}: the stored Description must be reported, got {entry}"
        );
        assert_eq!(
            entry.get("Status").and_then(Value::as_str),
            Some(REGISTRY_STATUS_AVAILABLE),
            "case {case:?}: a registry this store holds is available, got {entry}"
        );
        let created = entry
            .get("CreatedTime")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: CreatedTime must be reported, got {entry}"));
        assert!(
            ::aws_smithy_types::DateTime::from_str(
                created,
                ::aws_smithy_types::date_time::Format::DateTime,
            )
            .is_ok(),
            "case {case:?}: expected a date-time CreatedTime, got {created:?}"
        );
        assert_eq!(
            entry.get("UpdatedTime").and_then(Value::as_str),
            Some(created),
            "case {case:?}: nothing has updated the registry, got {entry}"
        );
    }
}

/// A registry in the `DELETING` status is left out of the results, whatever else the store
/// holds — and the records around it are listed as before.
#[tokio::test]
async fn a_registry_a_delete_is_draining_is_not_listed() {
    // (case, the names put into DELETING, the names that must still be listed)
    let rows: &[(&str, &[&str], &[&str])] = &[
        (
            "nothing is draining",
            &[],
            &["aaa-first", "bbb-second", "ccc-third"],
        ),
        (
            "the first record is draining",
            &["aaa-first"],
            &["bbb-second", "ccc-third"],
        ),
        (
            "a record in the middle is draining",
            &["bbb-second"],
            &["aaa-first", "ccc-third"],
        ),
        (
            "every record is draining",
            &["aaa-first", "bbb-second", "ccc-third"],
            &[],
        ),
    ];

    for (case, draining, expected) in rows {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let router = emulator_on(engine.clone());
        for name in ["aaa-first", "bbb-second", "ccc-third"] {
            given_registry(&router, name, None).await;
        }
        // `DeleteRegistry` is still a scaffold, so the status a delete would set is set on
        // the store the listing reads rather than through that operation.
        {
            let state = engine.state::<ComAmazonawsGlueAwsglueState>();
            let mut store = state.lock();
            for name in *draining {
                store
                    .registry_mut(name)
                    .unwrap_or_else(|| panic!("case {case:?}: {name} was created"))
                    .status = REGISTRY_STATUS_DELETING.to_string();
            }
        }

        // Asserted one entry per page too, so the token resumes within the listed set
        // rather than over every record the store holds.
        for page_size in [None, Some(MIN_MAX_RESULTS)] {
            let listed = walk(&router, case, &json!({}), page_size, 3).await;
            let names: Vec<String> = listed
                .iter()
                .map(|entry| entry["RegistryName"].as_str().unwrap_or_default().to_string())
                .collect();
            assert_eq!(
                names,
                expected
                    .iter()
                    .map(|name| (*name).to_string())
                    .collect::<Vec<_>>(),
                "case {case:?} at MaxResults {page_size:?}: a draining registry must not be \
                 listed, got {names:?}"
            );
        }
    }
}

/// The listing reports what the store holds NOW, and reports it without writing: two
/// listings in a row agree and the store is left exactly as the creates made it.
#[tokio::test]
async fn the_listing_reports_what_the_store_holds_and_changes_nothing() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());
    for name in ["kept-one", "kept-two", "removed"] {
        given_registry(&router, name, None).await;
    }

    let before = listed_names(&ok(&router, "ListRegistries", &json!({})).await);
    assert_eq!(
        before,
        vec![
            "kept-one".to_string(),
            "kept-two".to_string(),
            "removed".to_string()
        ],
        "case before the record is dropped: expected every stored registry, got {before:?}"
    );

    {
        let state = engine.state::<ComAmazonawsGlueAwsglueState>();
        state.lock().registries.remove("removed");
    }

    let after = listed_names(&ok(&router, "ListRegistries", &json!({})).await);
    assert_eq!(
        after,
        vec!["kept-one".to_string(), "kept-two".to_string()],
        "case after the record is dropped: a registry the store no longer holds must not be \
         listed, got {after:?}"
    );

    let again = ok(&router, "ListRegistries", &json!({})).await;
    assert_eq!(
        listed_names(&again),
        after,
        "case listed twice: a listing must change nothing, got {again}"
    );

    let state = engine.state::<ComAmazonawsGlueAwsglueState>();
    let store = state.lock();
    let stored: Vec<String> = store.registries.keys().cloned().collect();
    assert_eq!(
        stored,
        vec!["kept-one".to_string(), "kept-two".to_string()],
        "case after two listings: the store holds exactly what the creates made, got {stored:?}"
    );
}

/// A registry lives in one per-account namespace, so the listing of another Region's
/// endpoint reports the same registries — each with an ARN minted for the Region the call
/// is served in, as every other schema-registry read mints it.
#[tokio::test]
async fn the_listing_mints_each_arn_for_the_serving_region() {
    let router = emulator();
    for name in ["aaa-first", "bbb-second"] {
        given_registry(&router, name, None).await;
    }

    for region in [REGION, "eu-west-1", "ap-southeast-2"] {
        let (status, page) = call_in_region(&router, region, "ListRegistries", &json!({})).await;
        assert_eq!(
            status, 200,
            "case region {region}: expected 200, got {status} with {page}"
        );
        let arns: Vec<String> = page["Registries"]
            .as_array()
            .unwrap_or_else(|| panic!("case region {region}: Registries is a list, got {page}"))
            .iter()
            .map(|entry| entry["RegistryArn"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            arns,
            vec![
                registry_arn(region, "aaa-first"),
                registry_arn(region, "bbb-second")
            ],
            "case region {region}: expected each ARN minted for the serving Region, got {arns:?}"
        );
    }
}

/// A `MaxResults` outside the published range is refused by the modelled constraint rather
/// than served as some other page size.
#[tokio::test]
async fn a_page_size_outside_the_published_range_is_refused() {
    let router = emulator();
    given_registry(&router, "bounded-registry", None).await;

    for (case, max_results) in [
        ("one below the minimum", MIN_MAX_RESULTS - 1),
        ("one above the maximum", MAX_MAX_RESULTS + 1),
    ] {
        let (status, body) = call(
            &router,
            "ListRegistries",
            &json!({ "MaxResults": max_results }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: MaxResults={max_results} must be refused, got {status} {body}"
        );
    }

    // Each published bound itself is served.
    for max_results in [MIN_MAX_RESULTS, MAX_MAX_RESULTS] {
        let page = ok(
            &router,
            "ListRegistries",
            &json!({ "MaxResults": max_results }),
        )
        .await;
        assert_eq!(
            listed_names(&page),
            vec!["bounded-registry".to_string()],
            "case MaxResults {max_results}: a published bound is served, got {page}"
        );
    }
}
