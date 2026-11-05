// SPDX-License-Identifier: CC0-1.0

//! Resource data syncs end to end: `CreateResourceDataSync` persists a sync
//! configuration, `ListResourceDataSync` reports what is held,
//! `UpdateResourceDataSync` replaces the source of one and `DeleteResourceDataSync`
//! removes it.
//!
//! Every case goes through the front door a client uses — the shared harness in
//! `all/parameter_store.rs` ([`router`], [`call`], [`call_in_region`]) dispatches
//! `POST /` with `X-Amz-Target: AmazonSSM.<Operation>` through the multi-service
//! router. Nothing reaches into the state struct, so a response that is not computed
//! from persisted state cannot pass: every read follows a write.
//!
//! The suites are tables. The rules under test are
//!
//! * a sync is addressed by its `SyncName` and its `SyncType`, where a request naming no
//!   `SyncType` addresses the documented default `SyncToDestination` — so a create, a
//!   listing, an update and a delete all read an omitted type the same way, and a name
//!   addressed with another type addresses no sync;
//! * a listing reports, for each sync it holds, exactly what the write persisted, and
//!   nothing about a sync RUN (no `LastStatus`, no sync times), because nothing
//!   synchronizes anything here;
//! * a configuration outside the rules the docs state over `SyncType`, `S3Destination`
//!   and `SyncSource` together is refused and persists nothing.
//!
//! Each row is one input along an axis those rules generalise over — the sync type, which
//! member carries the configuration, the shape and size of the source, the name at its
//! published bounds, the page size, the Region — rather than the single configuration a
//! recorded unit happened to send.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::parameter_store::{call, call_in_region, error_code, router, ACCOUNT, REGION};

/// How many sync configurations the store holds at once
/// (`crate::state::resource_data_syncs::MAX_RESOURCE_DATA_SYNCS`): the emulator's own
/// ceiling on the one map a create grows, not a quota AWS publishes.
const MAX_SYNCS: usize =
    service_com_amazonaws_ssm_amazonssm::state::resource_data_syncs::MAX_RESOURCE_DATA_SYNCS;

/// The published floor of `ResourceDataSyncName`
/// (`smithy_query(query="shape", shape="ResourceDataSyncName")`: `length 1..64`).
const SYNC_NAME_MIN: usize = 1;

/// The published ceiling of the same shape.
const SYNC_NAME_MAX: usize = 64;

/// The published ceiling of a listing's `MaxResults`
/// (`smithy_query(query="shape", shape="MaxResults")`: `range 1..50`).
const MAX_RESULTS_MAX: i64 = 50;

/// The published floor of the same member.
const MAX_RESULTS_MIN: i64 = 1;

/// The sync type that "synchronizes data to an S3 bucket for Inventory", and the type a
/// request that names none addresses (`CreateResourceDataSync`'s `SyncType` doc: "The
/// default value is SyncToDestination").
const TO_DESTINATION: &str = "SyncToDestination";

/// The sync type that synchronizes "data from a single account and multiple Regions, or
/// multiple Amazon Web Services accounts and Amazon Web Services Regions" (same doc).
const FROM_SOURCE: &str = "SyncFromSource";

/// The only value `ResourceDataSyncS3Format` accepts
/// (`smithy_query(query="shape", shape="ResourceDataSyncS3Format")`).
const SYNC_FORMAT: &str = "JsonSerDe";

/// The sentence `ResourceDataSyncNotFoundException` carries for a sync a request
/// addresses that the Region holds none of: fixed text whose variable parts are the name
/// and the type the request addressed the sync by.
fn not_found_message(sync_name: &str, sync_type: &str) -> String {
    format!("ResourceDataSync with name {sync_name} synctype {sync_type} does not exists")
}

/// An S3 destination for a sync: "Amazon S3 configuration details for the sync", with the
/// three members `ResourceDataSyncS3Destination` requires and, optionally, the three it
/// does not.
fn s3_destination(bucket: &str, extras: bool) -> Value {
    let mut destination = json!({
        "BucketName": bucket,
        "SyncFormat": SYNC_FORMAT,
        "Region": REGION,
    });
    if extras {
        destination["Prefix"] = json!("inventory/");
        // `ResourceDataSyncAWSKMSKeyARN` is `len 1..512, pattern ^arn:`.
        destination["AWSKMSKeyARN"] =
            json!(format!("arn:aws:kms:{REGION}:{ACCOUNT}:key/0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0"));
        destination["DestinationDataSharing"] = json!({ "DestinationDataSharingType": "Organization" });
    }
    destination
}

/// A sync source that aggregates one account's Regions
/// (`ResourceDataSyncSource$SourceType`: "SingleAccountMultiRegions").
fn single_account_source(regions: &[&str], include_future_regions: bool) -> Value {
    json!({
        "SourceType": "SingleAccountMultiRegions",
        "SourceRegions": regions,
        "IncludeFutureRegions": include_future_regions,
    })
}

/// A sync source that aggregates an organization's data
/// (`ResourceDataSyncSource$SourceType`: "AwsOrganizations"), of one of the two
/// `OrganizationSourceType` values the docs name. `units` are
/// `ResourceDataSyncOrganizationalUnitId` values (`^ou-[0-9a-z]{4,32}-[a-z0-9]{8,32}$`).
fn organizations_source(organization_source_type: &str, units: &[&str], regions: &[&str]) -> Value {
    let mut organizations = json!({ "OrganizationSourceType": organization_source_type });
    if !units.is_empty() {
        organizations["OrganizationalUnits"] = json!(units
            .iter()
            .map(|id| json!({ "OrganizationalUnitId": id }))
            .collect::<Vec<Value>>());
    }
    json!({
        "SourceType": "AwsOrganizations",
        "AwsOrganizationsSource": organizations,
        "SourceRegions": regions,
        "EnableAllOpsDataSources": true,
    })
}

/// `CreateResourceDataSync` at `region`'s endpoint, returning `(status, body)`.
async fn create_in_region(
    router: &MultiServiceRouter,
    region: &str,
    body: &Value,
) -> (u16, Value) {
    call_in_region(router, region, "CreateResourceDataSync", body).await
}

/// `CreateResourceDataSync` at [`REGION`]'s endpoint, asserting it succeeded.
async fn create_ok(router: &MultiServiceRouter, case: &str, body: &Value) {
    let (status, response) = create_in_region(router, REGION, body).await;
    assert_eq!(
        status, 200,
        "case {case}: CreateResourceDataSync must succeed, got {status} {response}"
    );
}

/// `ListResourceDataSync` at `region`'s endpoint with `body`, returning `(status, body)`.
async fn list_in_region(router: &MultiServiceRouter, region: &str, body: &Value) -> (u16, Value) {
    call_in_region(router, region, "ListResourceDataSync", body).await
}

/// The `ResourceDataSyncItems` a listing of `sync_type` reports at `region`, asserting the
/// listing succeeded. `sync_type` is `None` for a listing that names no type at all.
async fn items_in_region(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    sync_type: Option<&str>,
) -> Vec<Value> {
    let body = match sync_type {
        Some(sync_type) => json!({ "SyncType": sync_type }),
        None => json!({}),
    };
    let (status, response) = list_in_region(router, region, &body).await;
    assert_eq!(
        status, 200,
        "case {case}: ListResourceDataSync {body} must succeed, got {status} {response}"
    );
    response["ResourceDataSyncItems"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case}: a listing reports ResourceDataSyncItems, got {response}")
        })
        .clone()
}

/// The one item a listing of `sync_type` reports for `sync_name`, or `None` when the
/// listing reports no sync of that name.
async fn item_named(
    router: &MultiServiceRouter,
    case: &str,
    sync_type: Option<&str>,
    sync_name: &str,
) -> Option<Value> {
    items_in_region(router, case, REGION, sync_type)
        .await
        .into_iter()
        .find(|item| item["SyncName"].as_str() == Some(sync_name))
}

/// The `SyncName`s a listing of `sync_type` reports at [`REGION`], in the order reported.
async fn names(router: &MultiServiceRouter, case: &str, sync_type: Option<&str>) -> Vec<String> {
    items_in_region(router, case, REGION, sync_type)
        .await
        .into_iter()
        .map(|item| item["SyncName"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Assert `response` is the refusal a configuration outside the documented rules carries:
/// HTTP 400, `ResourceDataSyncInvalidConfigurationException`, and a message saying what is
/// wrong.
fn assert_invalid_configuration(case: &str, status: u16, response: &Value) {
    assert_eq!(
        (status, error_code(response).as_str()),
        (400, "ResourceDataSyncInvalidConfigurationException"),
        "case {case}: expected 400 ResourceDataSyncInvalidConfigurationException, got \
         {status} {response}"
    );
    assert!(
        response["Message"]
            .as_str()
            .is_some_and(|message| !message.is_empty()),
        "case {case}: the refusal says what is wrong with the configuration, got {response}"
    );
}

/// Assert `response` is the refusal an unheld sync carries: HTTP 400,
/// `ResourceDataSyncNotFoundException`, the sentence this service words for it, and the
/// name and type the request addressed the sync by.
fn assert_not_found(case: &str, status: u16, response: &Value, sync_name: &str, sync_type: &str) {
    assert_eq!(
        (status, error_code(response).as_str()),
        (400, "ResourceDataSyncNotFoundException"),
        "case {case}: expected 400 ResourceDataSyncNotFoundException, got {status} {response}"
    );
    let expected = not_found_message(sync_name, sync_type);
    assert_eq!(
        response["Message"].as_str(),
        Some(expected.as_str()),
        "case {case}: expected the message {expected:?}, got {response}"
    );
    assert_eq!(
        (
            response["SyncName"].as_str(),
            response["SyncType"].as_str()
        ),
        (Some(sync_name), Some(sync_type)),
        "case {case}: the refusal names the sync it looked for, got {response}"
    );
}

/// One row of the create table: a sync configuration, and the type it is held under.
#[derive(Debug)]
struct CreateCase {
    /// What the row is about, named in every assertion.
    case: &'static str,
    /// The sync's name.
    sync_name: String,
    /// The `SyncType` the request carries, or `None` when it names none.
    sync_type: Option<&'static str>,
    /// The type the sync is held under: the one the request named, or the documented
    /// default for a request that named none.
    held_type: &'static str,
    /// The `S3Destination` the request carries, or `None`.
    s3_destination: Option<Value>,
    /// The `SyncSource` the request carries, or `None`.
    sync_source: Option<Value>,
}

impl CreateCase {
    /// The create body for this row.
    fn body(&self) -> Value {
        let mut body = json!({ "SyncName": self.sync_name });
        if let Some(sync_type) = self.sync_type {
            body["SyncType"] = json!(sync_type);
        }
        if let Some(destination) = &self.s3_destination {
            body["S3Destination"] = destination.clone();
        }
        if let Some(source) = &self.sync_source {
            body["SyncSource"] = source.clone();
        }
        body
    }
}

/// Every row of the create table: both sync types, a destination with and without the
/// members it does not require, sources of each documented source type and organization
/// source type, over one and over three Regions, and a name at each published bound.
fn create_cases() -> Vec<CreateCase> {
    vec![
        CreateCase {
            case: "no sync type at all, so the default destination sync",
            sync_name: "inventory-default".to_string(),
            sync_type: None,
            held_type: TO_DESTINATION,
            s3_destination: Some(s3_destination("inventory-bucket", false)),
            sync_source: None,
        },
        CreateCase {
            case: "a destination sync with every optional destination member",
            sync_name: "inventory-encrypted".to_string(),
            sync_type: Some(TO_DESTINATION),
            held_type: TO_DESTINATION,
            s3_destination: Some(s3_destination("inventory-bucket", true)),
            sync_source: None,
        },
        CreateCase {
            case: "a source sync over one Region of one account",
            sync_name: "explorer-one-region".to_string(),
            sync_type: Some(FROM_SOURCE),
            held_type: FROM_SOURCE,
            s3_destination: None,
            sync_source: Some(single_account_source(&["us-east-1"], false)),
        },
        CreateCase {
            case: "a source sync over three Regions, future Regions included",
            sync_name: "explorer-three-regions".to_string(),
            sync_type: Some(FROM_SOURCE),
            held_type: FROM_SOURCE,
            s3_destination: None,
            sync_source: Some(single_account_source(
                &["us-east-1", "us-west-2", "eu-central-1"],
                true,
            )),
        },
        CreateCase {
            case: "a source sync over an entire organization",
            sync_name: "explorer-entire-organization".to_string(),
            sync_type: Some(FROM_SOURCE),
            held_type: FROM_SOURCE,
            s3_destination: None,
            sync_source: Some(organizations_source(
                "EntireOrganization",
                &[],
                &["us-east-1"],
            )),
        },
        CreateCase {
            case: "a source sync over two organizational units",
            sync_name: "explorer-two-units".to_string(),
            sync_type: Some(FROM_SOURCE),
            held_type: FROM_SOURCE,
            s3_destination: None,
            sync_source: Some(organizations_source(
                "OrganizationalUnits",
                &["ou-abc1-11111111", "ou-abc1-22222222"],
                &["us-east-1", "us-west-2"],
            )),
        },
        CreateCase {
            case: "a name at its published floor",
            sync_name: "s".repeat(SYNC_NAME_MIN),
            sync_type: Some(TO_DESTINATION),
            held_type: TO_DESTINATION,
            s3_destination: Some(s3_destination("floor-bucket", false)),
            sync_source: None,
        },
        CreateCase {
            case: "a name at its published ceiling",
            sync_name: "n".repeat(SYNC_NAME_MAX),
            sync_type: Some(FROM_SOURCE),
            held_type: FROM_SOURCE,
            s3_destination: None,
            sync_source: Some(single_account_source(&["us-west-2"], false)),
        },
    ]
}

/// A create persists the configuration it carries, and a listing of the type the sync is
/// held under reports exactly that configuration back — including for a create that named
/// no type at all, which is held under the documented default.
#[tokio::test]
async fn a_create_persists_what_a_listing_reports() {
    let router = router();
    for case in create_cases() {
        create_ok(&router, case.case, &case.body()).await;

        let item = item_named(&router, case.case, Some(case.held_type), &case.sync_name)
            .await
            .unwrap_or_else(|| {
                panic!(
                    "case {}: the sync {} must be reported under {}",
                    case.case, case.sync_name, case.held_type
                )
            });
        assert_eq!(
            item["SyncType"].as_str(),
            Some(case.held_type),
            "case {}: expected SyncType {}, got {item}",
            case.case,
            case.held_type
        );
        assert_eq!(
            item.get("S3Destination").cloned(),
            case.s3_destination.clone(),
            "case {}: the destination is what the create carried, got {item}",
            case.case
        );
        match &case.sync_source {
            None => assert!(
                item.get("SyncSource").is_none(),
                "case {}: a sync created with no source reports none, got {item}",
                case.case
            ),
            Some(source) => {
                let reported = &item["SyncSource"];
                for member in ["SourceType", "SourceRegions", "AwsOrganizationsSource"] {
                    assert_eq!(
                        reported.get(member).cloned(),
                        source.get(member).cloned(),
                        "case {}: {member} is what the create carried, got {item}",
                        case.case
                    );
                }
                for member in ["IncludeFutureRegions", "EnableAllOpsDataSources"] {
                    let expected = source[member].as_bool().unwrap_or(false);
                    let actual = reported[member].as_bool().unwrap_or(false);
                    assert_eq!(
                        actual, expected,
                        "case {}: expected {member} {expected}, got {item}",
                        case.case
                    );
                }
                // The four `State` values the model names are each a reason the source
                // cannot be read; a source this store holds is in none of them.
                assert!(
                    reported.get("State").is_none(),
                    "case {}: a readable source reports no State, got {item}",
                    case.case
                );
            }
        }

        // The configuration was created, not run: nothing here synchronizes Inventory or
        // OpsData anywhere, so nothing about a sync run is reported.
        for member in [
            "LastStatus",
            "LastSyncTime",
            "LastSuccessfulSyncTime",
            "LastSyncStatusMessage",
        ] {
            assert!(
                item.get(member).is_none(),
                "case {}: nothing has synced, so {member} is not reported, got {item}",
                case.case
            );
        }
        let created = item["SyncCreatedTime"].as_f64().unwrap_or_else(|| {
            panic!(
                "case {}: a sync reports when it was created, got {item}",
                case.case
            )
        });
        let modified = item["SyncLastModifiedTime"].as_f64().unwrap_or_else(|| {
            panic!(
                "case {}: a sync reports when it was last changed, got {item}",
                case.case
            )
        });
        assert_eq!(
            created, modified,
            "case {}: a sync nothing has changed was last changed when it was created, \
             got {item}",
            case.case
        );
    }
}

/// A sync is addressed by its name AND its type: a listing reports the syncs of the type
/// it names, and of the documented default when it names none.
#[tokio::test]
async fn a_listing_reports_the_syncs_of_the_type_it_addresses() {
    let router = router();
    create_ok(
        &router,
        "the destination sync",
        &json!({
            "SyncName": "to-destination",
            "SyncType": TO_DESTINATION,
            "S3Destination": s3_destination("inventory-bucket", false),
        }),
    )
    .await;
    create_ok(
        &router,
        "the source sync",
        &json!({
            "SyncName": "from-source",
            "SyncType": FROM_SOURCE,
            "SyncSource": single_account_source(&["us-east-1"], false),
        }),
    )
    .await;

    let cases: Vec<(&str, Option<&str>, Vec<&str>)> = vec![
        (
            "the destination type, named",
            Some(TO_DESTINATION),
            vec!["to-destination"],
        ),
        (
            "no type at all, so the default one",
            None,
            vec!["to-destination"],
        ),
        ("the source type", Some(FROM_SOURCE), vec!["from-source"]),
    ];
    for (case, sync_type, expected) in cases {
        let reported = names(&router, case, sync_type).await;
        assert_eq!(
            reported, expected,
            "case {case}: expected {expected:?}, got {reported:?}"
        );
    }

    // A type outside the two the docs name is no type a sync can carry.
    let (status, response) = list_in_region(
        &router,
        REGION,
        &json!({ "SyncType": "SyncToSomewhereElse" }),
    )
    .await;
    assert_invalid_configuration("a sync type nothing can carry", status, &response);
}

/// A second create of a name the Region already holds a sync under is refused with
/// `ResourceDataSyncAlreadyExistsException`, naming the sync, and leaves what is held
/// exactly as it was — whichever type the second create names.
#[tokio::test]
async fn a_create_of_a_held_name_is_refused_and_changes_nothing() {
    let router = router();
    let first = json!({
        "SyncName": "held",
        "S3Destination": s3_destination("inventory-bucket", false),
    });
    create_ok(&router, "the first create", &first).await;
    let before = item_named(&router, "the first create", Some(TO_DESTINATION), "held")
        .await
        .expect("the first create persisted the sync");

    let cases: Vec<(&str, Value)> = vec![
        ("the same configuration again", first.clone()),
        (
            "the same name, another destination",
            json!({
                "SyncName": "held",
                "SyncType": TO_DESTINATION,
                "S3Destination": s3_destination("another-bucket", true),
            }),
        ),
        (
            "the same name, the other sync type",
            json!({
                "SyncName": "held",
                "SyncType": FROM_SOURCE,
                "SyncSource": single_account_source(&["us-east-1"], false),
            }),
        ),
    ];
    for (case, body) in cases {
        let (status, response) = create_in_region(&router, REGION, &body).await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "ResourceDataSyncAlreadyExistsException"),
            "case {case}: expected 400 ResourceDataSyncAlreadyExistsException, got \
             {status} {response}"
        );
        assert_eq!(
            response["SyncName"].as_str(),
            Some("held"),
            "case {case}: the refusal names the sync it collided with, got {response}"
        );

        let after = item_named(&router, case, Some(TO_DESTINATION), "held")
            .await
            .unwrap_or_else(|| panic!("case {case}: the held sync is still reported"));
        assert_eq!(
            after, before,
            "case {case}: a refused create leaves the held sync as it was, got {after}"
        );
        // The refused create admitted no sync of the other type either.
        let source_syncs = names(&router, case, Some(FROM_SOURCE)).await;
        assert!(
            source_syncs.is_empty(),
            "case {case}: a refused create persists nothing, got {source_syncs:?}"
        );
    }
}

/// A configuration outside the rules the docs state over `SyncType`, `S3Destination` and
/// `SyncSource` together is refused with
/// `ResourceDataSyncInvalidConfigurationException` and persists nothing.
#[tokio::test]
async fn a_configuration_outside_the_documented_rules_is_refused() {
    let router = router();
    let cases: Vec<(&str, Value)> = vec![
        (
            "a destination sync with no destination",
            json!({ "SyncName": "no-destination", "SyncType": TO_DESTINATION }),
        ),
        (
            "no sync type and no destination, so the default type with none",
            json!({ "SyncName": "no-destination-defaulted" }),
        ),
        (
            "a source sync with no source",
            json!({ "SyncName": "no-source", "SyncType": FROM_SOURCE }),
        ),
        (
            "a sync type outside the two the docs name",
            json!({
                "SyncName": "unknown-type",
                "SyncType": "SyncToDestinations",
                "S3Destination": s3_destination("inventory-bucket", false),
            }),
        ),
        (
            "a source type outside the two the docs name",
            json!({
                "SyncName": "unknown-source-type",
                "SyncType": FROM_SOURCE,
                "SyncSource": {
                    "SourceType": "MultiAccountMultiRegions",
                    "SourceRegions": ["us-east-1"],
                },
            }),
        ),
        (
            "an Organizations source with no AwsOrganizationsSource",
            json!({
                "SyncName": "organizations-without-source",
                "SyncType": FROM_SOURCE,
                "SyncSource": {
                    "SourceType": "AwsOrganizations",
                    "SourceRegions": ["us-east-1"],
                },
            }),
        ),
        (
            "an organization source type outside the two the docs name",
            json!({
                "SyncName": "unknown-organization-source-type",
                "SyncType": FROM_SOURCE,
                "SyncSource": organizations_source("EntireAccount", &[], &["us-east-1"]),
            }),
        ),
        (
            "an organizational-unit source naming no unit",
            json!({
                "SyncName": "units-without-units",
                "SyncType": FROM_SOURCE,
                "SyncSource": organizations_source("OrganizationalUnits", &[], &["us-east-1"]),
            }),
        ),
    ];
    for (case, body) in cases {
        let (status, response) = create_in_region(&router, REGION, &body).await;
        assert_invalid_configuration(case, status, &response);

        for sync_type in [TO_DESTINATION, FROM_SOURCE] {
            let reported = names(&router, case, Some(sync_type)).await;
            assert!(
                reported.is_empty(),
                "case {case}: a refused create persists nothing, got {reported:?} under \
                 {sync_type}"
            );
        }
    }
}

/// A delete removes the one sync it addresses — by name and by type, the type defaulting
/// the same way everywhere — and a delete that addresses no sync is refused and removes
/// nothing.
#[tokio::test]
async fn a_delete_removes_only_the_sync_it_addresses() {
    let router = router();
    let destination = json!({
        "SyncName": "inventory",
        "S3Destination": s3_destination("inventory-bucket", false),
    });
    let source = json!({
        "SyncName": "explorer",
        "SyncType": FROM_SOURCE,
        "SyncSource": single_account_source(&["us-east-1"], false),
    });

    // Every refusal: the name and type addressed, and the sync the request named is
    // still held afterwards.
    let refusals: Vec<(&str, Value, &str, &str)> = vec![
        (
            "a name nothing holds",
            json!({ "SyncName": "never-created" }),
            "never-created",
            TO_DESTINATION,
        ),
        (
            "a name nothing holds, of the source type",
            json!({ "SyncName": "never-created", "SyncType": FROM_SOURCE }),
            "never-created",
            FROM_SOURCE,
        ),
        (
            "a held destination sync addressed as a source sync",
            json!({ "SyncName": "inventory", "SyncType": FROM_SOURCE }),
            "inventory",
            FROM_SOURCE,
        ),
        (
            "a held source sync addressed with no type at all",
            json!({ "SyncName": "explorer" }),
            "explorer",
            TO_DESTINATION,
        ),
    ];
    for (case, body, sync_name, sync_type) in refusals {
        create_ok(&router, case, &destination).await;
        create_ok(&router, case, &source).await;

        let (status, response) = call(&router, "DeleteResourceDataSync", &body).await;
        assert_not_found(case, status, &response, sync_name, sync_type);

        let held = names(&router, case, Some(TO_DESTINATION)).await;
        let sources = names(&router, case, Some(FROM_SOURCE)).await;
        assert_eq!(
            (held.as_slice(), sources.as_slice()),
            (["inventory".to_string()].as_slice(), ["explorer".to_string()].as_slice()),
            "case {case}: a refused delete removes nothing, got {held:?} and {sources:?}"
        );

        // Clear the store for the next row: each delete addresses its own sync.
        for (name, sync_type) in [("inventory", TO_DESTINATION), ("explorer", FROM_SOURCE)] {
            let (status, response) = call(
                &router,
                "DeleteResourceDataSync",
                &json!({ "SyncName": name, "SyncType": sync_type }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case}: the held sync {name} deletes, got {status} {response}"
            );
        }
    }

    // A delete that addresses a held sync removes that one and leaves the other.
    create_ok(&router, "the pair", &destination).await;
    create_ok(&router, "the pair", &source).await;
    let (status, response) = call(
        &router,
        "DeleteResourceDataSync",
        // No `SyncType` at all: the documented default addresses the destination sync.
        &json!({ "SyncName": "inventory" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case the pair: the destination sync deletes, got {status} {response}"
    );
    assert!(
        names(&router, "the pair", Some(TO_DESTINATION))
            .await
            .is_empty(),
        "case the pair: the deleted sync is no longer reported"
    );
    assert_eq!(
        names(&router, "the pair", Some(FROM_SOURCE)).await,
        vec!["explorer".to_string()],
        "case the pair: the sync of the other type is untouched"
    );

    // Deleting it twice: the second delete addresses nothing.
    let (status, response) = call(
        &router,
        "DeleteResourceDataSync",
        &json!({ "SyncName": "inventory" }),
    )
    .await;
    assert_not_found(
        "the same delete twice",
        status,
        &response,
        "inventory",
        TO_DESTINATION,
    );
}

/// An update replaces the source of the sync it addresses and stamps when it ran, while
/// the creation instant and the sync's name and type stay put.
#[tokio::test]
async fn an_update_replaces_the_source_and_stamps_when_it_ran() {
    let router = router();
    let create = json!({
        "SyncName": "explorer",
        "SyncType": FROM_SOURCE,
        "SyncSource": single_account_source(&["us-east-1"], false),
    });
    create_ok(&router, "the updated sync", &create).await;
    let before = item_named(&router, "the updated sync", Some(FROM_SOURCE), "explorer")
        .await
        .expect("the create persisted the sync");

    let replacements: Vec<(&str, Value)> = vec![
        (
            "another Region",
            single_account_source(&["eu-central-1"], false),
        ),
        (
            "three Regions and future Regions",
            single_account_source(&["us-east-1", "us-west-2", "ap-south-1"], true),
        ),
        (
            "back to one Region",
            single_account_source(&["us-east-1"], false),
        ),
    ];
    for (case, source) in replacements {
        let (status, response) = call(
            &router,
            "UpdateResourceDataSync",
            &json!({ "SyncName": "explorer", "SyncType": FROM_SOURCE, "SyncSource": source }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: UpdateResourceDataSync must succeed, got {status} {response}"
        );

        let after = item_named(&router, case, Some(FROM_SOURCE), "explorer")
            .await
            .unwrap_or_else(|| panic!("case {case}: the updated sync is still reported"));
        assert_eq!(
            after["SyncSource"]["SourceRegions"], source["SourceRegions"],
            "case {case}: the listing reports the Regions the update carried, got {after}"
        );
        assert_eq!(
            after["SyncSource"]["IncludeFutureRegions"]
                .as_bool()
                .unwrap_or(false),
            source["IncludeFutureRegions"].as_bool().unwrap_or(false),
            "case {case}: the listing reports the future-Region choice the update carried, \
             got {after}"
        );
        assert_eq!(
            (after["SyncName"].clone(), after["SyncType"].clone()),
            (before["SyncName"].clone(), before["SyncType"].clone()),
            "case {case}: an update changes neither the name nor the type, got {after}"
        );
        assert_eq!(
            after["SyncCreatedTime"], before["SyncCreatedTime"],
            "case {case}: SyncCreatedTime is unchanged by an update, got {after}"
        );
        let created = after["SyncCreatedTime"].as_f64().unwrap_or_default();
        let modified = after["SyncLastModifiedTime"].as_f64().unwrap_or_default();
        assert!(
            modified >= created,
            "case {case}: SyncLastModifiedTime {modified} is at or after SyncCreatedTime \
             {created}, got {after}"
        );
    }

    // `SyncLastModifiedTime` is when the last write ran, not when the sync was created:
    // after a wait longer than the timestamp's own resolution it moves, and
    // `SyncCreatedTime` does not.
    tokio::time::sleep(Duration::from_millis(1_100)).await;
    let (status, response) = call(
        &router,
        "UpdateResourceDataSync",
        &json!({
            "SyncName": "explorer",
            "SyncType": FROM_SOURCE,
            "SyncSource": single_account_source(&["us-west-2"], false),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case the stamped sync: the update must succeed, got {status} {response}"
    );
    let after = item_named(&router, "the stamped sync", Some(FROM_SOURCE), "explorer")
        .await
        .expect("the updated sync is still reported");
    assert_eq!(
        after["SyncCreatedTime"], before["SyncCreatedTime"],
        "case the stamped sync: SyncCreatedTime is unchanged by an update, got {after}"
    );
    let created = after["SyncCreatedTime"].as_f64().unwrap_or_default();
    let modified = after["SyncLastModifiedTime"].as_f64().unwrap_or_default();
    assert!(
        modified > created,
        "case the stamped sync: SyncLastModifiedTime {modified} moved past \
         SyncCreatedTime {created}, got {after}"
    );
}

/// An update the operation cannot serve — a type other than the one it supports, a sync
/// nothing holds, a source outside the documented rules, or a change of the account
/// options the docs forbid changing — is refused and leaves the sync as it was.
#[tokio::test]
async fn an_update_outside_what_it_supports_is_refused_and_changes_nothing() {
    let router = router();
    create_ok(
        &router,
        "the destination sync",
        &json!({
            "SyncName": "inventory",
            "S3Destination": s3_destination("inventory-bucket", false),
        }),
    )
    .await;
    create_ok(
        &router,
        "the source sync",
        &json!({
            "SyncName": "explorer",
            "SyncType": FROM_SOURCE,
            "SyncSource": single_account_source(&["us-east-1"], false),
        }),
    )
    .await;
    let destination_before = item_named(&router, "setup", Some(TO_DESTINATION), "inventory")
        .await
        .expect("the destination sync is held");
    let source_before = item_named(&router, "setup", Some(FROM_SOURCE), "explorer")
        .await
        .expect("the source sync is held");

    // Refusals that are not about a sync being absent: the operation cannot serve them.
    let invalid: Vec<(&str, Value)> = vec![
        (
            "the sync type the operation does not support",
            json!({
                "SyncName": "inventory",
                "SyncType": TO_DESTINATION,
                "SyncSource": single_account_source(&["us-east-1"], false),
            }),
        ),
        (
            "a source type outside the two the docs name",
            json!({
                "SyncName": "explorer",
                "SyncType": FROM_SOURCE,
                "SyncSource": {
                    "SourceType": "MultiAccountMultiRegions",
                    "SourceRegions": ["us-east-1"],
                },
            }),
        ),
        (
            "the account options changed to an organization",
            json!({
                "SyncName": "explorer",
                "SyncType": FROM_SOURCE,
                "SyncSource": organizations_source("EntireOrganization", &[], &["us-east-1"]),
            }),
        ),
        (
            "an organizational-unit source naming no unit",
            json!({
                "SyncName": "explorer",
                "SyncType": FROM_SOURCE,
                "SyncSource": organizations_source("OrganizationalUnits", &[], &["us-east-1"]),
            }),
        ),
    ];
    for (case, body) in invalid {
        let (status, response) = call(&router, "UpdateResourceDataSync", &body).await;
        assert_invalid_configuration(case, status, &response);
        assert_unchanged(&router, case, &destination_before, &source_before).await;
    }

    // Refusals that ARE about a sync being absent.
    let absent: Vec<(&str, Value, &str, &str)> = vec![
        (
            "a name nothing holds",
            json!({
                "SyncName": "never-created",
                "SyncType": FROM_SOURCE,
                "SyncSource": single_account_source(&["us-east-1"], false),
            }),
            "never-created",
            FROM_SOURCE,
        ),
        (
            "a held destination sync addressed as a source sync",
            json!({
                "SyncName": "inventory",
                "SyncType": FROM_SOURCE,
                "SyncSource": single_account_source(&["us-east-1"], false),
            }),
            "inventory",
            FROM_SOURCE,
        ),
    ];
    for (case, body, sync_name, sync_type) in absent {
        let (status, response) = call(&router, "UpdateResourceDataSync", &body).await;
        assert_not_found(case, status, &response, sync_name, sync_type);
        assert_unchanged(&router, case, &destination_before, &source_before).await;
    }
}

/// Both held syncs are exactly as they were: what a refused update must leave behind.
async fn assert_unchanged(
    router: &MultiServiceRouter,
    case: &str,
    destination_before: &Value,
    source_before: &Value,
) {
    let destination = item_named(router, case, Some(TO_DESTINATION), "inventory")
        .await
        .unwrap_or_else(|| panic!("case {case}: the destination sync is still reported"));
    assert_eq!(
        &destination, destination_before,
        "case {case}: a refused update leaves the destination sync as it was, got \
         {destination}"
    );
    let source = item_named(router, case, Some(FROM_SOURCE), "explorer")
        .await
        .unwrap_or_else(|| panic!("case {case}: the source sync is still reported"));
    assert_eq!(
        &source, source_before,
        "case {case}: a refused update leaves the source sync as it was, got {source}"
    );
}

/// A delete naming a sync type no sync can carry asks for a kind of sync that cannot
/// exist, so it is refused with `ResourceDataSyncInvalidConfigurationException` — the way
/// a create and a listing of that type are — rather than reported as an absent sync, and
/// it removes nothing.
#[tokio::test]
async fn a_delete_of_a_type_no_sync_carries_is_refused() {
    let router = router();
    create_ok(
        &router,
        "the held sync",
        &json!({
            "SyncName": "inventory",
            "S3Destination": s3_destination("inventory-bucket", false),
        }),
    )
    .await;

    for case in ["SyncToDestinations", "syncfromsource", "SyncFromSources"] {
        let (status, response) = call(
            &router,
            "DeleteResourceDataSync",
            &json!({ "SyncName": "inventory", "SyncType": case }),
        )
        .await;
        assert_invalid_configuration(case, status, &response);
        assert_eq!(
            names(&router, case, Some(TO_DESTINATION)).await,
            vec!["inventory".to_string()],
            "case {case}: a refused delete removes nothing"
        );
    }
}

/// Within ONE source type an update changes the data sources it names — the Regions, the
/// future-Region choice, and which units of the same organization it reads — which is
/// what the operation exists for; only the source type itself, the sync's account
/// options, cannot change.
#[tokio::test]
async fn an_update_changes_the_sources_within_one_source_type() {
    let router = router();
    create_ok(
        &router,
        "the organization sync",
        &json!({
            "SyncName": "explorer",
            "SyncType": FROM_SOURCE,
            "SyncSource": organizations_source(
                "OrganizationalUnits",
                &["ou-abc1-11111111"],
                &["us-east-1"],
            ),
        }),
    )
    .await;

    let replacements: Vec<(&str, Value)> = vec![
        (
            "another organizational unit",
            organizations_source("OrganizationalUnits", &["ou-abc1-22222222"], &["us-east-1"]),
        ),
        (
            "two organizational units over two Regions",
            organizations_source(
                "OrganizationalUnits",
                &["ou-abc1-11111111", "ou-abc1-33333333"],
                &["us-east-1", "eu-central-1"],
            ),
        ),
        (
            "the entire organization",
            organizations_source("EntireOrganization", &[], &["us-east-1"]),
        ),
    ];
    for (case, source) in replacements {
        let (status, response) = call(
            &router,
            "UpdateResourceDataSync",
            &json!({ "SyncName": "explorer", "SyncType": FROM_SOURCE, "SyncSource": source }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: the update must succeed, got {status} {response}"
        );
        let after = item_named(&router, case, Some(FROM_SOURCE), "explorer")
            .await
            .unwrap_or_else(|| panic!("case {case}: the updated sync is still reported"));
        assert_eq!(
            after["SyncSource"]["AwsOrganizationsSource"], source["AwsOrganizationsSource"],
            "case {case}: the listing reports the organization source the update carried, \
             got {after}"
        );
        assert_eq!(
            after["SyncSource"]["SourceRegions"], source["SourceRegions"],
            "case {case}: the listing reports the Regions the update carried, got {after}"
        );
    }
}

/// The name a create carries is the sync's key, held VERBATIM: `ResourceDataSyncName`
/// carries a length bound and no pattern, so names differing only in case, in punctuation
/// or in script are different syncs, each reported back exactly as it was created — and a
/// listing walks them in ascending name order whatever order they were created in.
#[tokio::test]
async fn a_name_is_held_verbatim_and_a_listing_walks_them_in_order() {
    let router = router();
    // One name per form the modelled shape admits (`length 1..64`, no pattern):
    // `smithy_query(query="shape", shape="ResourceDataSyncName")`.
    let held: Vec<&str> = vec![
        "Inventory",
        "inventory",
        "inventory.2",
        "inventory-2",
        "inventory_2",
        "inventory 2",
        "Inventário",
        "0",
    ];
    // Created in reverse, so the order a listing reports cannot be the order of creation.
    for name in held.iter().rev() {
        create_ok(
            &router,
            name,
            &json!({
                "SyncName": name,
                "S3Destination": s3_destination("inventory-bucket", false),
            }),
        )
        .await;
    }

    let reported = names(&router, "every name form", Some(TO_DESTINATION)).await;
    let mut expected: Vec<String> = held.iter().map(|name| (*name).to_string()).collect();
    expected.sort();
    assert_eq!(
        reported, expected,
        "case every name form: a listing walks the syncs in ascending name order, got \
         {reported:?}"
    );

    // Each one is addressed, and deleted, by the name it was created under — not by a
    // normalised form of it.
    for name in &held {
        let item = item_named(&router, name, Some(TO_DESTINATION), name)
            .await
            .unwrap_or_else(|| panic!("case {name}: the sync is reported under its own name"));
        assert_eq!(
            item["SyncName"].as_str(),
            Some(*name),
            "case {name}: the name is reported verbatim, got {item}"
        );
        let (status, response) = call(
            &router,
            "DeleteResourceDataSync",
            &json!({ "SyncName": name }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name}: the sync deletes under its own name, got {status} {response}"
        );
    }
    let left = names(&router, "after deleting every name", Some(TO_DESTINATION)).await;
    assert!(
        left.is_empty(),
        "case after deleting every name: the Region holds no sync, got {left:?}"
    );
}

/// A listing pages by sync name: each page holds at most `MaxResults` syncs, the token it
/// hands out is where the next page starts, and the pages together are every sync the
/// Region holds, each exactly once, in ascending name order — whatever page size the
/// caller asks for.
#[tokio::test]
async fn a_listing_pages_by_name_and_resumes_where_it_stopped() {
    let router = router();
    let held: Vec<String> = (1..=7).map(|nth| format!("sync-{nth:02}")).collect();
    for name in &held {
        create_ok(
            &router,
            name,
            &json!({
                "SyncName": name,
                "S3Destination": s3_destination("inventory-bucket", false),
            }),
        )
        .await;
    }

    for page_size in [1_i64, 2, 3, held.len() as i64, MAX_RESULTS_MAX] {
        let case = format!("pages of {page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut body = json!({ "MaxResults": page_size });
            if let Some(token) = &token {
                body["NextToken"] = json!(token);
            }
            let (status, response) = list_in_region(&router, REGION, &body).await;
            assert_eq!(
                status, 200,
                "case {case}: the listing must succeed, got {status} {response}"
            );
            let page: Vec<String> = response["ResourceDataSyncItems"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case}: a page reports its items, got {response}"))
                .iter()
                .map(|item| item["SyncName"].as_str().unwrap_or_default().to_string())
                .collect();
            assert!(
                page.len() as i64 <= page_size,
                "case {case}: a page holds at most {page_size} syncs, got {page:?}"
            );
            walked.extend(page.clone());
            pages += 1;
            assert!(
                pages <= held.len() + 1,
                "case {case}: the walk terminates, got {walked:?}"
            );
            match response["NextToken"].as_str() {
                None => break,
                Some(next) => {
                    assert_eq!(
                        page.len() as i64,
                        page_size,
                        "case {case}: a page that hands out a token was full, got {page:?}"
                    );
                    token = Some(next.to_string());
                }
            }
        }
        assert_eq!(
            walked, held,
            "case {case}: the pages together are every sync held, once each, in name \
             order, got {walked:?}"
        );
    }
}

/// A `NextToken` no sync name could take was not issued by a listing, so it is refused
/// with `InvalidNextToken`; a `MaxResults` outside its published range is refused by the
/// modelled constraint before the listing runs.
#[tokio::test]
async fn a_token_or_page_size_outside_the_contract_is_refused() {
    let router = router();
    create_ok(
        &router,
        "the one sync",
        &json!({
            "SyncName": "inventory",
            "S3Destination": s3_destination("inventory-bucket", false),
        }),
    )
    .await;

    let (status, response) = list_in_region(
        &router,
        REGION,
        &json!({ "NextToken": "t".repeat(SYNC_NAME_MAX + 1) }),
    )
    .await;
    assert_eq!(
        (status, error_code(&response).as_str()),
        (400, "InvalidNextToken"),
        "case a token longer than any sync name: expected 400 InvalidNextToken, got \
         {status} {response}"
    );

    // A token that IS a possible sync name resumes the walk at that position, even when
    // no sync of that name is held.
    let (status, response) =
        list_in_region(&router, REGION, &json!({ "NextToken": "inventory" })).await;
    assert_eq!(
        status, 200,
        "case a token that is a possible name: the listing resumes, got {status} \
         {response}"
    );
    assert_eq!(
        response["ResourceDataSyncItems"][0]["SyncName"].as_str(),
        Some("inventory"),
        "case a token that is a possible name: the page starts there, got {response}"
    );

    for (case, max_results) in [
        ("one below the published floor", MAX_RESULTS_MIN - 1),
        ("one past the published ceiling", MAX_RESULTS_MAX + 1),
    ] {
        let (status, response) =
            list_in_region(&router, REGION, &json!({ "MaxResults": max_results })).await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "ValidationException"),
            "case a MaxResults {case}: expected 400 ValidationException, got {status} \
             {response}"
        );
    }
}

/// Systems Manager is regional: a sync is reported, updated and deleted through the
/// endpoint of the Region it was created at, and names no sync at any other.
#[tokio::test]
async fn a_sync_is_held_at_the_region_that_created_it() {
    let router = router();
    let body = json!({
        "SyncName": "inventory",
        "S3Destination": s3_destination("inventory-bucket", false),
    });
    let (status, response) = create_in_region(&router, "eu-central-1", &body).await;
    assert_eq!(
        status, 200,
        "case the other Region: the create must succeed, got {status} {response}"
    );

    let elsewhere = items_in_region(
        &router,
        "the creating Region",
        "eu-central-1",
        Some(TO_DESTINATION),
    )
    .await;
    assert_eq!(
        elsewhere.len(),
        1,
        "case the creating Region: the sync is reported there, got {elsewhere:?}"
    );
    let here = items_in_region(&router, "another Region", REGION, Some(TO_DESTINATION)).await;
    assert!(
        here.is_empty(),
        "case another Region: the sync is reported at no other endpoint, got {here:?}"
    );

    // The same name is a different sync at a different Region's endpoint, so a create
    // there is not a collision...
    let (status, response) = create_in_region(&router, REGION, &body).await;
    assert_eq!(
        status, 200,
        "case the same name at another Region: the create must succeed, got {status} \
         {response}"
    );
    // ...and a delete at one endpoint leaves the other Region's sync alone.
    let (status, response) = call_in_region(
        &router,
        REGION,
        "DeleteResourceDataSync",
        &json!({ "SyncName": "inventory" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case the delete at one endpoint: it must succeed, got {status} {response}"
    );
    let elsewhere = items_in_region(
        &router,
        "after the delete elsewhere",
        "eu-central-1",
        Some(TO_DESTINATION),
    )
    .await;
    assert_eq!(
        elsewhere.len(),
        1,
        "case after the delete elsewhere: the other Region's sync is untouched, got \
         {elsewhere:?}"
    );
}

/// The sync store is bounded: once it holds as many configurations as the emulator keeps,
/// a further create is refused with `ResourceDataSyncCountExceededException` and writes
/// nothing, while the syncs already held still list and still delete.
#[tokio::test]
async fn a_create_past_the_stores_ceiling_is_refused() {
    let router = router();
    for nth in 0..MAX_SYNCS {
        // Spread the syncs over two Regions, so the ceiling is shown to be on the store
        // as a whole rather than on one Region's share of it.
        let region = if nth % 2 == 0 { REGION } else { "eu-central-1" };
        let body = json!({
            "SyncName": format!("sync-{nth:03}"),
            "S3Destination": s3_destination("inventory-bucket", false),
        });
        let (status, response) = create_in_region(&router, region, &body).await;
        assert_eq!(
            status, 200,
            "case filling the store: create {nth} must succeed, got {status} {response}"
        );
    }

    let (status, response) = create_in_region(
        &router,
        REGION,
        &json!({
            "SyncName": "one-too-many",
            "S3Destination": s3_destination("inventory-bucket", false),
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&response).as_str()),
        (400, "ResourceDataSyncCountExceededException"),
        "case one past the ceiling: expected 400 ResourceDataSyncCountExceededException, \
         got {status} {response}"
    );
    assert!(
        response["Message"]
            .as_str()
            .is_some_and(|message| !message.is_empty()),
        "case one past the ceiling: the refusal says what the ceiling is, got {response}"
    );

    // The refusal wrote nothing: the store still holds what it held, and still serves it.
    let held = items_in_region(
        &router,
        "one past the ceiling",
        REGION,
        Some(TO_DESTINATION),
    )
    .await;
    assert_eq!(
        held.len(),
        MAX_SYNCS / 2,
        "case one past the ceiling: the Region's own syncs are what it held, got \
         {} items",
        held.len()
    );
    assert!(
        !held
            .iter()
            .any(|item| item["SyncName"].as_str() == Some("one-too-many")),
        "case one past the ceiling: the refused create persisted nothing, got {held:?}"
    );
    let (status, response) = call(
        &router,
        "DeleteResourceDataSync",
        &json!({ "SyncName": "sync-000" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case one past the ceiling: a held sync still deletes, got {status} {response}"
    );
}
