// SPDX-License-Identifier: CC0-1.0

//! `ListNetworkMigrationDefinitions`, end to end over HTTP.
//!
//! Every assertion goes through the router's `dispatch`, so it exercises the same
//! restJson1 deserialize → handle → serialize path a client drives, and reads the
//! service's state only through its own operations: what `CreateNetworkMigrationDefinition`
//! returned must be what a later listing reports.
//!
//! The harness (`Mgn`, `error_code`, the region and account constants) is the one
//! `application_lifecycle.rs` defines; `call` takes the operation's modelled URI path, so
//! these operations are reached under `/network-migration/<Operation>`.

use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_mgn_applicationmigrationservice::ops::types::{
    SourceConfiguration, SourceS3Configuration, TargetNetwork, TargetS3Configuration,
};
use service_com_amazonaws_mgn_applicationmigrationservice::state::{
    NetworkMigrationDefinition, NetworkMigrationDefinitionConfiguration,
};

use crate::application_lifecycle::{error_code, Mgn, EMULATOR_ACCOUNT, OTHER_ACCOUNT, REGION};

/// The modelled `http.uri` of `CreateNetworkMigrationDefinition`
/// (`smithy_query(query="operation",operation="CreateNetworkMigrationDefinition")`),
/// without the leading slash the harness adds.
const CREATE: &str = "network-migration/CreateNetworkMigrationDefinition";

/// The modelled `http.uri` of `ListNetworkMigrationDefinitions`, likewise.
const LIST: &str = "network-migration/ListNetworkMigrationDefinitions";

/// The modelled `http.uri` of `GetNetworkMigrationDefinition`, likewise.
const GET: &str = "network-migration/GetNetworkMigrationDefinition";

/// Every member the modelled `NetworkMigrationDefinitionSummary` declares. Source:
/// `smithy_query(query="closure",operation="ListNetworkMigrationDefinitions")` →
/// `NetworkMigrationDefinitionSummary`.
const SUMMARY_MEMBERS: [&str; 6] = [
    "networkMigrationDefinitionID",
    "name",
    "sourceEnvironment",
    "arn",
    "tags",
    "scopeTags",
];

/// The model's `SourceEnvironment` values, in the order the model lists them. Source:
/// `smithy_query(query="closure",operation="ListNetworkMigrationDefinitions")` →
/// `SourceEnvironment.values`.
const SOURCE_ENVIRONMENTS: [&str; 8] = [
    "NSX",
    "VSPHERE",
    "FORTIGATE_FIREWALL",
    "PALO_ALTO_FIREWALL",
    "CISCO_ACI",
    "LOGICAL_MODEL",
    "MODELIZE_IT",
    "AWS_DISCOVERY_COLLECTOR",
];

/// The most ids the modelled `networkMigrationDefinitionIDs` filter accepts
/// (`NetworkMigrationDefintionsIDsFilter`, len 0..20).
const MAX_FILTER_IDS: usize = 20;

/// A `targetS3Configuration` as the model requires it: a bucket name and a 12-digit
/// owner account.
fn target_s3(bucket: &str) -> Value {
    json!({"s3Bucket": bucket, "s3BucketOwner": EMULATOR_ACCOUNT})
}

/// A `sourceConfigurations` entry for `environment`.
fn source_configuration(environment: &str) -> Value {
    json!({
        "sourceEnvironment": environment,
        "sourceS3Configuration": {
            "s3Bucket": "source-exports",
            "s3BucketOwner": EMULATOR_ACCOUNT,
            "s3Key": format!("exports/{environment}.json"),
        },
    })
}

fn definition_id(definition: &Value) -> String {
    definition["networkMigrationDefinitionID"]
        .as_str()
        .unwrap_or_else(|| panic!("no networkMigrationDefinitionID in {definition:?}"))
        .to_string()
}

/// The class a listed definition spans: the source-configuration count (0, 1 and the
/// modelled maximum of 2, over `SourceEnvironment` values the issue did not cite), the
/// tag and scope-tag counts (none, one, several), and the description (absent, present).
///
/// The list bounds and the enum values are the model's
/// (`smithy_query(query="closure",operation="CreateNetworkMigrationDefinition")`:
/// `SourceConfigurationList` len 0..2, `TagsMap` len 0..50, `ScopeTagsMap` len 0..40).
fn creation_cases() -> Vec<(&'static str, Value)> {
    vec![
        (
            "no source configuration, no tags",
            json!({
                "name": "plan-without-sources",
                "targetS3Configuration": target_s3("migration-artifacts"),
                "targetNetwork": {"topology": "ISOLATED_VPC"},
            }),
        ),
        (
            "one source configuration and one tag of each kind",
            json!({
                "name": "plan-vsphere",
                "description": "one source",
                "sourceConfigurations": [source_configuration(SOURCE_ENVIRONMENTS[1])],
                "targetS3Configuration": target_s3("migration-artifacts"),
                "targetNetwork": {"topology": "HUB_AND_SPOKE"},
                "tags": {"env": "dev"},
                "scopeTags": {"BusinessUnit": "retail"},
            }),
        ),
        (
            "two source configurations: the summary reports the first",
            json!({
                "name": "plan-cisco-then-modelize",
                "sourceConfigurations": [
                    source_configuration(SOURCE_ENVIRONMENTS[4]),
                    source_configuration(SOURCE_ENVIRONMENTS[6]),
                ],
                "targetS3Configuration": target_s3("migration-artifacts"),
                "targetNetwork": {"topology": "ISOLATED_VPC"},
                "tags": {"env": "prod", "owner": "network-team", "tier": "1"},
                "scopeTags": {"BusinessUnit": "wholesale", "Project": "lift"},
            }),
        ),
        (
            "a last source environment the catalog ends on",
            json!({
                "name": "plan-discovery-collector",
                "description": "",
                "sourceConfigurations": [source_configuration(SOURCE_ENVIRONMENTS[7])],
                "targetS3Configuration": target_s3("migration-artifacts.2"),
                "targetNetwork": {"topology": "ISOLATED_VPC"},
                "scopeTags": {"BusinessUnit": "retail"},
            }),
        ),
        (
            "a first source environment the catalog starts on",
            json!({
                "name": "plan-nsx",
                "sourceConfigurations": [source_configuration(SOURCE_ENVIRONMENTS[0])],
                "targetS3Configuration": target_s3("migration-artifacts"),
                "targetNetwork": {"topology": "ISOLATED_VPC"},
                "tags": {"env": "staging"},
            }),
        ),
    ]
}

/// The summary a definition's create response implies: the members of the stored record
/// the summary shape declares, with `sourceEnvironment` taken from the record's first
/// source configuration.
fn expected_summary(created: &Value) -> Value {
    let mut summary = json!({
        "networkMigrationDefinitionID": created["networkMigrationDefinitionID"].clone(),
        "name": created["name"].clone(),
        "arn": created["arn"].clone(),
    });
    if let Some(first) = created["sourceConfigurations"]
        .as_array()
        .and_then(|configurations| configurations.first())
    {
        summary["sourceEnvironment"] = first["sourceEnvironment"].clone();
    }
    for member in ["tags", "scopeTags"] {
        if !created[member].is_null() {
            summary[member] = created[member].clone();
        }
    }
    summary
}

/// Every item a listing reports, following `nextToken` to the end, asserting 200 and a
/// page no larger than `max_results` asked for.
async fn list_all(mgn: &Mgn, case: &str, filters: Option<Value>, max_results: Option<u32>) -> Vec<Value> {
    let mut collected = Vec::new();
    let mut next_token: Option<String> = None;
    // A page per definition at worst, plus a margin: a listing that keeps handing back a
    // token is a defect, not a reason to loop forever.
    for page_index in 0..64 {
        let mut request = json!({});
        if let Some(filters) = filters.clone() {
            request["filters"] = filters;
        }
        if let Some(max_results) = max_results {
            request["maxResults"] = json!(max_results);
        }
        if let Some(token) = next_token.clone() {
            request["nextToken"] = json!(token);
        }
        let (status, body) = mgn.call(LIST, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListNetworkMigrationDefinitions expected 200, got {status} with {body:?}"
        );
        let items = body["items"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no items array in {body:?}"))
            .clone();
        if let Some(limit) = max_results {
            assert!(
                items.len() <= limit as usize,
                "case {case:?}: page {page_index} returned {} items, more than maxResults {limit}",
                items.len()
            );
        }
        collected.extend(items.clone());
        match body["nextToken"].as_str() {
            None => return collected,
            Some(token) => {
                assert!(
                    !items.is_empty(),
                    "case {case:?}: page {page_index} handed back a token with no items: {body:?}"
                );
                next_token = Some(token.to_string());
            }
        }
    }
    panic!("case {case:?}: the listing never stopped handing back a nextToken");
}

/// A listing reports the account's stored definitions in creation order, as the modelled
/// summary of each stored record, whatever the page size — and a page never carries a
/// member the summary shape does not declare.
#[tokio::test]
async fn definitions_are_listed_in_creation_order_at_every_page_size() {
    let mgn = Mgn::new();

    // An account with no definitions lists nothing, and hands back no cursor.
    let (status, empty) = mgn.call(LIST, &json!({})).await;
    assert_eq!(
        status, 200,
        "an empty listing expected 200, got {status} with {empty:?}"
    );
    assert_eq!(
        empty["items"].as_array().map(Vec::len),
        Some(0),
        "an account with no definitions must list no items, got {empty:?}"
    );
    assert!(
        empty["nextToken"].is_null(),
        "a listing with nothing left must carry no nextToken, got {empty:?}"
    );

    let mut expected = Vec::new();
    for (case, request) in creation_cases() {
        let (status, created) = mgn.call(CREATE, &request).await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreateNetworkMigrationDefinition expected 201, got {status} with {created:?}"
        );
        expected.push((case, expected_summary(&created)));
    }

    // Absent, each bound of the modelled `MaxResultsType` range (1..1000), a size that
    // splits the selection unevenly, and a size past the count.
    for max_results in [None, Some(1), Some(2), Some(3), Some(5), Some(9), Some(1000)] {
        let case = format!("maxResults {max_results:?}");
        let items = list_all(&mgn, &case, None, max_results).await;
        assert_eq!(
            items.len(),
            expected.len(),
            "case {case:?}: expected {} definitions, got {items:?}",
            expected.len()
        );
        for (index, (created_case, want)) in expected.iter().enumerate() {
            let got = &items[index];
            assert_eq!(
                got, want,
                "case {case:?} / {created_case:?}: expected the stored record's summary {want:?}, got {got:?}"
            );
            let unknown: Vec<&String> = got
                .as_object()
                .unwrap_or_else(|| panic!("case {case:?}: item {index} is not an object: {got:?}"))
                .keys()
                .filter(|member| !SUMMARY_MEMBERS.contains(&member.as_str()))
                .collect();
            assert!(
                unknown.is_empty(),
                "case {case:?} / {created_case:?}: the summary carries members the model does not declare: {unknown:?}"
            );
        }
    }
}

/// The `networkMigrationDefinitionIDs` filter selects exactly the named definitions; an
/// absent filter and a present-but-empty id list select everything, and an id the
/// account holds no record of selects nothing.
#[tokio::test]
async fn the_id_filter_selects_exactly_the_named_definitions() {
    let mgn = Mgn::new();
    let mut ids = Vec::new();
    for (case, request) in creation_cases() {
        let (status, created) = mgn.call(CREATE, &request).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201, got {status} with {created:?}"
        );
        ids.push(definition_id(&created));
    }
    let unknown = "nmd-0A1b2C3d4E5f6G7h8";

    // (the filter, the ids it must select) — spanning no filter, an empty list, one id,
    // several, every id, an unknown id, a mix, and the filter's modelled maximum of 20.
    let mut padded: Vec<String> = ids.clone();
    while padded.len() < MAX_FILTER_IDS {
        padded.push(format!("nmd-{:017}", padded.len()));
    }
    let cases: Vec<(String, Option<Value>, Vec<String>)> = vec![
        ("no filters member".to_string(), None, ids.clone()),
        (
            "an empty id list".to_string(),
            Some(json!({"networkMigrationDefinitionIDs": []})),
            ids.clone(),
        ),
        (
            "one id".to_string(),
            Some(json!({"networkMigrationDefinitionIDs": [ids[2]]})),
            vec![ids[2].clone()],
        ),
        (
            "two ids, named out of creation order".to_string(),
            Some(json!({"networkMigrationDefinitionIDs": [ids[3], ids[1]]})),
            vec![ids[1].clone(), ids[3].clone()],
        ),
        (
            "every id".to_string(),
            Some(json!({"networkMigrationDefinitionIDs": ids.clone()})),
            ids.clone(),
        ),
        (
            "one id the account holds no record of".to_string(),
            Some(json!({"networkMigrationDefinitionIDs": [unknown]})),
            Vec::new(),
        ),
        (
            "a known id beside an unknown one".to_string(),
            Some(json!({"networkMigrationDefinitionIDs": [unknown, ids[0]]})),
            vec![ids[0].clone()],
        ),
        (
            format!("the filter's modelled maximum of {MAX_FILTER_IDS} ids"),
            Some(json!({"networkMigrationDefinitionIDs": padded})),
            ids.clone(),
        ),
    ];

    for (case, filters, want) in cases {
        // Page size 1 as well as the default, so filtering and paging compose.
        for max_results in [None, Some(1)] {
            let case = format!("{case} at maxResults {max_results:?}");
            let items = list_all(&mgn, &case, filters.clone(), max_results).await;
            let got: Vec<String> = items.iter().map(definition_id).collect();
            assert_eq!(
                got, want,
                "case {case:?}: expected definitions {want:?}, got {got:?}"
            );
        }
    }
}

/// A definition of another account is not this account's to see: it is absent from the
/// listing, and naming its id in the filter selects nothing.
#[tokio::test]
async fn another_accounts_definitions_are_not_listed() {
    let mgn = Mgn::new();
    let foreign_ids = ["nmd-aaaaaaaaaaaaaaaaa", "nmd-bbbbbbbbbbbbbbbbb"];
    {
        let store = mgn.store();
        let mut guard = store.lock();
        for (index, id) in foreign_ids.iter().enumerate() {
            let stored = guard.insert_network_migration_definition(NetworkMigrationDefinition {
                arn: format!(
                    "arn:aws:mgn:{REGION}:{OTHER_ACCOUNT}:network-migration-definition/{id}"
                ),
                network_migration_definition_id: (*id).to_string(),
                account_id: OTHER_ACCOUNT.to_string(),
                created_at: SystemTime::now(),
                updated_at: SystemTime::now(),
                configuration: NetworkMigrationDefinitionConfiguration {
                    name: format!("other-account-{index}"),
                    source_configurations: vec![SourceConfiguration {
                        source_environment: SOURCE_ENVIRONMENTS[3].to_string(),
                        source_s3_configuration: SourceS3Configuration {
                            s3_bucket: "source-exports".to_string(),
                            s3_bucket_owner: OTHER_ACCOUNT.to_string(),
                            s3_key: "exports/palo-alto.json".to_string(),
                        },
                    }],
                    target_s3_configuration: TargetS3Configuration {
                        s3_bucket: "migration-artifacts".to_string(),
                        s3_bucket_owner: OTHER_ACCOUNT.to_string(),
                    },
                    target_network: TargetNetwork {
                        topology: "ISOLATED_VPC".to_string(),
                        ..TargetNetwork::default()
                    },
                    ..NetworkMigrationDefinitionConfiguration::default()
                },
            });
            assert!(stored, "seeding {id:?} must store it");
        }
    }

    let (status, created) = mgn
        .call(
            CREATE,
            &json!({
                "name": "own-definition",
                "targetS3Configuration": target_s3("migration-artifacts"),
                "targetNetwork": {"topology": "ISOLATED_VPC"},
            }),
        )
        .await;
    assert_eq!(status, 201, "expected 201, got {status} with {created:?}");
    let own = definition_id(&created);

    let items = list_all(&mgn, "another account's definitions", None, None).await;
    let got: Vec<String> = items.iter().map(definition_id).collect();
    assert_eq!(
        got,
        vec![own.clone()],
        "a listing must report only this account's definitions, got {got:?}"
    );

    for id in foreign_ids {
        let case = format!("filtering on another account's {id}");
        let items = list_all(
            &mgn,
            &case,
            Some(json!({"networkMigrationDefinitionIDs": [id]})),
            None,
        )
        .await;
        assert!(
            items.is_empty(),
            "case {case:?}: another account's definition must not be selectable, got {items:?}"
        );
    }
}

/// A `nextToken` this operation did not issue, or one whose anchor definition the
/// selection no longer holds, cannot be resumed: it is refused with a 400
/// `ValidationException` rather than served as some other page. An empty token is the
/// first page, as the modelled length 0..2048 allows.
#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let mgn = Mgn::new();
    for (case, request) in creation_cases() {
        let (status, created) = mgn.call(CREATE, &request).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201, got {status} with {created:?}"
        );
    }

    let unresumable = [
        "not-a-token",
        "0",
        "nmd-0A1b2C3d4E5f6G7h8",
        "mgn-applications-app-0A1b2C3d4E5f6G7h8",
        // This operation's own marker, but anchored on a definition the selection does
        // not hold.
        "mgn-network-migration-definitions-nmd-0A1b2C3d4E5f6G7h8",
    ];
    for token in unresumable {
        let (status, body) = mgn.call(LIST, &json!({"nextToken": token})).await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "ValidationException"),
            "case {token:?}: an unresumable nextToken must be refused with 400 ValidationException, got {status} with {body:?}"
        );
    }

    // An empty token names no anchor, so it is the first page rather than a refusal.
    let (status, first) = mgn.call(LIST, &json!({"nextToken": "", "maxResults": 2})).await;
    assert_eq!(
        status, 200,
        "an empty nextToken expected 200, got {status} with {first:?}"
    );
    assert_eq!(
        first["items"].as_array().map(Vec::len),
        Some(2),
        "an empty nextToken must answer the first page, got {first:?}"
    );

    // A token the operation issued resumes where the page stopped, and a definition
    // appears on exactly one page.
    let resumed = list_all(&mgn, "following the issued tokens", None, Some(2)).await;
    let mut ids: Vec<String> = resumed.iter().map(definition_id).collect();
    let count = ids.len();
    ids.sort();
    ids.dedup();
    assert_eq!(
        ids.len(),
        count,
        "following tokens must report each definition exactly once, got {ids:?}"
    );
    assert_eq!(
        count,
        creation_cases().len(),
        "following tokens must report every definition, got {count}"
    );
}

/// Listing is a read: it changes nothing a later caller sees. Two identical listings
/// answer the same items, and a get after a listing answers the record the create
/// returned.
#[tokio::test]
async fn listing_does_not_change_what_a_later_caller_sees() {
    let mgn = Mgn::new();
    let mut created_records = Vec::new();
    for (case, request) in creation_cases() {
        let (status, created) = mgn.call(CREATE, &request).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201, got {status} with {created:?}"
        );
        created_records.push((case, created));
    }

    let first = list_all(&mgn, "first listing", None, Some(2)).await;
    let again = list_all(&mgn, "second listing", None, Some(3)).await;
    assert_eq!(
        again, first,
        "two listings of an unchanged store must answer the same items, got {again:?}"
    );

    for (case, created) in &created_records {
        let id = definition_id(created);
        let (status, fetched) = mgn
            .call(GET, &json!({"networkMigrationDefinitionID": id}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetNetworkMigrationDefinition expected 200, got {status} with {fetched:?}"
        );
        assert_eq!(
            &fetched, created,
            "case {case:?}: a listing must not change the stored record, got {fetched:?}"
        );
    }
}
