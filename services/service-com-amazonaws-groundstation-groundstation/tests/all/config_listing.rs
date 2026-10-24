// SPDX-License-Identifier: CC0-1.0

//! `ListConfigs` (`GET /config`), end to end over HTTP through the same front door as
//! the rest of this target's suites.
//!
//! One rule, asserted over tables rather than over one recorded call: a listing REPORTS
//! the store it reads, so `ListConfigs` answers exactly the configs this account holds
//! at the moment of the call — one `ConfigListItem` per stored config, carrying that
//! config's own `configId`, `configType`, `configArn` and `name` — a page at a time,
//! and changes nothing a later caller sees.
//!
//! * every `ConfigTypeData` member is a row of the "exactly the stored configs" table,
//!   so the listing is asserted for the whole config store rather than for the one type
//!   a recording used;
//! * a create, an update and a delete each show up in the next listing, and nothing
//!   else moves;
//! * paging is asserted for a table of page sizes — dividing the sequence evenly,
//!   unevenly, exactly, and larger than the store — each of which must yield every
//!   config exactly once in the listing's own order;
//! * `maxResults` is asserted on each published bound of `PaginationMaxResults` and one
//!   value past each;
//! * a `nextToken` this listing did not issue is refused rather than resumed.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_groundstation_groundstation::state::BAD_REQUEST_ERROR_CODE;
use smithy_server_runtime::MultiServiceRouter;

use crate::scheduling_loop::{
    antenna_uplink_config_data, config_data_table, create_config, create_mission_profile, harness,
    list_pages, send,
};

/// What a `ConfigListItem` must say about one stored config, keyed in the tables below by
/// its `configArn` (the one member that names the config uniquely on its own).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Listed {
    config_id: String,
    config_type: String,
    name: String,
}

/// A JSON member of `item`, as a string (empty when absent, so a missing member fails the
/// comparison it is part of rather than panicking here).
fn member(item: &Value, name: &str) -> String {
    item.get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Every item `ListConfigs` returns, across as many pages as it takes, in the order the
/// listing gave them. `page_size` is the caller's `maxResults`, or `None` to name none.
async fn list_all(router: &MultiServiceRouter, page_size: Option<i64>) -> Vec<Value> {
    list_pages(router, "/config", "configList", page_size).await
}

/// Assert the listing reports exactly `expected`, and that every item it reports reads
/// back through `GetConfig` as the same config.
async fn assert_listing_is(
    router: &MultiServiceRouter,
    expected: &BTreeMap<String, Listed>,
    case: &str,
) {
    let items = list_all(router, None).await;
    let mut got: BTreeMap<String, Listed> = BTreeMap::new();
    for item in &items {
        let arn = member(item, "configArn");
        let listed = Listed {
            config_id: member(item, "configId"),
            config_type: member(item, "configType"),
            name: member(item, "name"),
        };
        assert!(
            got.insert(arn.clone(), listed).is_none(),
            "case {case}: {arn} appears more than once in the listing, got {items:?}"
        );
    }
    assert_eq!(
        &got, expected,
        "case {case}: the listing must report exactly the stored configs, got {got:?}"
    );

    for (arn, listed) in &got {
        let read = send(
            router,
            "GET",
            &format!("/config/{}/{}", listed.config_type, listed.config_id),
            None,
        )
        .await;
        assert_eq!(
            read.status, 200,
            "case {case}: a listed config must read back 200, got {} {}",
            read.status, read.body
        );
        assert_eq!(
            read.string("configArn"),
            *arn,
            "case {case}: GetConfig answered another configArn, got {}",
            read.body
        );
        assert_eq!(
            read.string("name"),
            listed.name,
            "case {case}: the listing and GetConfig disagree on the name, got {}",
            read.body
        );
    }
}

#[tokio::test]
async fn list_configs_reports_exactly_the_stored_configs_and_follows_their_lifecycle() {
    let router = harness();

    // An account that has created nothing holds no config, so the listing is present,
    // empty, and closed.
    let empty = send(&router, "GET", "/config", None).await;
    assert_eq!(
        empty.status, 200,
        "case empty-store: expected ListConfigs 200, got {} {}",
        empty.status, empty.body
    );
    assert_eq!(
        empty.field("configList"),
        &json!([]),
        "case empty-store: a store holding no config must list none, got {}",
        empty.body
    );
    assert!(
        empty.field("nextToken").is_null(),
        "case empty-store: a listing with nothing more to come must carry no nextToken, got {}",
        empty.body
    );

    // `uplinkEchoConfig` references an antenna-uplink config, so that one exists first.
    let echo_source_name = "echo source uplink";
    let (echo_source_id, _, echo_source_arn) = create_config(
        &router,
        echo_source_name,
        json!({"antennaUplinkConfig": antenna_uplink_config_data()}),
        None,
    )
    .await;

    let mut expected: BTreeMap<String, Listed> = BTreeMap::new();
    expected.insert(
        echo_source_arn.clone(),
        Listed {
            config_id: echo_source_id,
            config_type: "antenna-uplink".to_string(),
            name: echo_source_name.to_string(),
        },
    );

    // The table spans every `ConfigTypeData` member (source:
    // `crate::scheduling_loop::config_data_table`, built from
    // `smithy_query(query="shape", shape="ConfigTypeData")`), so the listing is asserted
    // over the whole config store and not over one type.
    for (config_type, config_data) in config_data_table(&echo_source_arn) {
        let name = format!("listed {config_type}");
        let (config_id, created_type, config_arn) = create_config(
            &router,
            &name,
            config_data.clone(),
            Some(json!({"case": config_type})),
        )
        .await;
        assert_eq!(
            created_type, config_type,
            "case {config_type}: CreateConfig stored another configType, got {created_type:?}"
        );
        assert!(
            expected
                .insert(
                    config_arn,
                    Listed {
                        config_id,
                        config_type: config_type.to_string(),
                        name,
                    },
                )
                .is_none(),
            "case {config_type}: CreateConfig reused a configArn"
        );
    }
    assert_listing_is(&router, &expected, "every-config-type").await;

    // A listing is a REPORT: reading it twice reports the same store.
    let again = list_all(&router, None).await;
    assert_eq!(
        again,
        list_all(&router, None).await,
        "case repeated-read: a listing must not change what the next caller sees"
    );

    // An update is visible in the next listing, under the same ARN.
    let (tracking_arn, tracking) = expected
        .iter()
        .find(|(_, listed)| listed.config_type == "tracking")
        .map(|(arn, listed)| (arn.clone(), listed.clone()))
        .expect("the table created a tracking config");
    let renamed = "renamed tracking";
    let updated = send(
        &router,
        "PUT",
        &format!("/config/tracking/{}", tracking.config_id),
        Some(json!({
            "name": renamed,
            "configData": {"trackingConfig": {"autotrack": "REMOVED"}},
        })),
    )
    .await;
    assert_eq!(
        updated.status, 200,
        "case renamed: expected UpdateConfig 200, got {} {}",
        updated.status, updated.body
    );
    expected.insert(
        tracking_arn,
        Listed {
            name: renamed.to_string(),
            ..tracking
        },
    );
    assert_listing_is(&router, &expected, "after-an-update").await;

    // And a delete removes exactly its own config from the listing.
    let (endpoint_arn, endpoint) = expected
        .iter()
        .find(|(_, listed)| listed.config_type == "dataflow-endpoint")
        .map(|(arn, listed)| (arn.clone(), listed.clone()))
        .expect("the table created a dataflow-endpoint config");
    let deleted = send(
        &router,
        "DELETE",
        &format!("/config/dataflow-endpoint/{}", endpoint.config_id),
        None,
    )
    .await;
    assert_eq!(
        deleted.status, 200,
        "case deleted: expected DeleteConfig 200, got {} {}",
        deleted.status, deleted.body
    );
    expected.remove(&endpoint_arn);
    assert_listing_is(&router, &expected, "after-a-delete").await;
}

#[tokio::test]
async fn list_configs_pages_through_every_config_once_whatever_the_page_size() {
    let router = harness();

    // Six configs, so a page size can divide the sequence evenly, unevenly, exactly, or
    // not bound it at all.
    for index in 0..6 {
        create_config(
            &router,
            &format!("paging config {index}"),
            json!({"trackingConfig": {"autotrack": "PREFERRED"}}),
            None,
        )
        .await;
    }

    let whole: Vec<String> = list_all(&router, None)
        .await
        .iter()
        .map(|item| member(item, "configArn"))
        .collect();
    assert_eq!(
        whole.len(),
        6,
        "case whole-listing: expected every created config, got {whole:?}"
    );

    for size in [1_i64, 2, 4, 5, 6, 7, 100] {
        let paged: Vec<String> = list_all(&router, Some(size))
            .await
            .iter()
            .map(|item| member(item, "configArn"))
            .collect();
        assert_eq!(
            paged, whole,
            "case maxResults={size}: paging must yield every config exactly once, in the \
             listing's order, got {paged:?}"
        );
    }

    // Each published bound of `PaginationMaxResults` and one value past it (source:
    // `smithy_query(query="operation", operation="ListConfigs")` — range 1..100). A value
    // the model refuses never reaches the handler: this service's validator answers it.
    for (case, size, accepted) in [
        ("the lower bound", 1_i64, true),
        ("the upper bound", 100, true),
        ("one below the lower bound", 0, false),
        ("one above the upper bound", 101, false),
    ] {
        let reply = send(&router, "GET", &format!("/config?maxResults={size}"), None).await;
        if accepted {
            assert_eq!(
                reply.status, 200,
                "case maxResults={size} ({case}): expected 200, got {} {}",
                reply.status, reply.body
            );
        } else {
            assert_eq!(
                reply.status, 400,
                "case maxResults={size} ({case}): expected a refusal, got {} {}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error_code(),
                BAD_REQUEST_ERROR_CODE,
                "case maxResults={size} ({case}): expected {BAD_REQUEST_ERROR_CODE}, got {}",
                reply.body
            );
        }
    }
}

#[tokio::test]
async fn list_configs_refuses_a_next_token_it_did_not_issue() {
    let router = harness();

    // Two mission profiles, so `ListMissionProfiles` has a token to hand out; each one
    // brings its own tracking config, so the config store has pages too.
    create_mission_profile(&router, "token profile a", 60, None, None).await;
    create_mission_profile(&router, "token profile b", 60, None, None).await;

    let configs_token = send(&router, "GET", "/config?maxResults=1", None)
        .await
        .field("nextToken")
        .as_str()
        .map(str::to_string)
        .expect("a capped page hands back a token");
    let profiles_token = send(&router, "GET", "/missionprofile?maxResults=1", None)
        .await
        .field("nextToken")
        .as_str()
        .map(str::to_string)
        .expect("a capped page hands back a token");

    // A token addresses an offset into the sequence that issued it, so this listing
    // accepts only its own, and refuses anything else rather than resuming at an offset
    // into an unrelated sequence.
    let resumed = send(
        &router,
        "GET",
        &format!("/config?maxResults=1&nextToken={configs_token}"),
        None,
    )
    .await;
    assert_eq!(
        resumed.status, 200,
        "case own-token: a token this listing issued must resume it, got {} {}",
        resumed.status, resumed.body
    );

    for (case, token) in [
        ("a token from another listing", profiles_token.as_str()),
        ("a token this service never issued", "not-a-token"),
        ("a token naming no offset", "configs.not-a-number"),
    ] {
        let reply = send(
            &router,
            "GET",
            &format!("/config?maxResults=1&nextToken={token}"),
            None,
        )
        .await;
        assert_eq!(
            reply.status, 431,
            "case {case} ({token}): expected 431, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "InvalidParameterException",
            "case {case} ({token}): wrong error shape, got {}",
            reply.body
        );
    }
}
