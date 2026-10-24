// SPDX-License-Identifier: CC0-1.0

//! A data store is a REGIONAL resource: it exists in the region the creating request
//! was addressed to, and a request addressed to any other region reaches nothing.
//!
//! Every case drives real HTTP through the production front door
//! (`MultiServiceRouter::dispatch`), signing each request for a chosen region through
//! the harness's `Emulator::call_in` — the SigV4 credential scope is where the service
//! reads the region from. The suites are TABLES: the axis the rule generalises over is
//! the region a request is addressed to (and, for the listing, how many data stores a
//! region holds), so the cited `us-east-1`/`eu-west-1` pair is two rows among several.
//!
//! An image set is seeded through the crate's public `state` module (as the copy suite
//! does and explains) because no implemented operation mints one yet; everything each
//! case asserts goes over the wire.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use service_com_amazonaws_medicalimaging_ahigatewayservice::state::{
    DATASTORE_ACTIVATION_DELAY, DATASTORE_NOT_FOUND_MESSAGE, MAX_DATASTORES,
};

use crate::datastore_lifecycle::{
    unheld_id, Answer, Emulator, ACCOUNT, SETTLE_MULTIPLE,
};
use crate::image_set_copy::seed_image_set;
use crate::resource_tagging::label;

/// The regions these cases address. Real AWS region codes, every one of them inside the
/// `[a-z0-9-]+` alphabet the service's own ARN pattern admits for a region
/// (`crate::state::ARN_PATTERN`, from
/// `smithy_query(query="shape",shape="DatastoreArn")`). The issue cited `us-east-1` and
/// `eu-west-1`; the rest are rows it did not name.
const REGIONS: [&str; 5] = [
    "us-east-1",
    "eu-west-1",
    "ap-southeast-2",
    "eu-central-1",
    "sa-east-1",
];

/// The tag map every data store these cases create carries, so a cross-region write can
/// be shown to have changed nothing.
const SEEDED_TAGS: [(&str, &str); 1] = [("team", "radiology")];

/// `POST /datastore` in `region`, WITHOUT waiting for the activation the clock owes the
/// new store: a caller creates many and settles once.
async fn create_datastore_in(
    emulator: &Emulator,
    region: &str,
    datastore_name: &str,
    client_token: &str,
) -> Answer {
    let tags: Value = SEEDED_TAGS
        .iter()
        .map(|(key, value)| ((*key).to_string(), Value::from(*value)))
        .collect::<serde_json::Map<String, Value>>()
        .into();
    emulator
        .call_in(
            region,
            "POST",
            "/datastore",
            Some(json!({
                "datastoreName": datastore_name,
                "clientToken": client_token,
                "tags": tags,
            })),
        )
        .await
}

/// [`create_datastore_in`], asserting the create succeeded and answering the minted id.
async fn created_datastore_id(
    emulator: &Emulator,
    region: &str,
    datastore_name: &str,
    client_token: &str,
) -> String {
    let created = create_datastore_in(emulator, region, datastore_name, client_token).await;
    assert_eq!(
        created.status, 200,
        "case {region:?}: expected HTTP 200 from CreateDatastore, got {created:?}"
    );
    created.string(&["datastoreId"])
}

/// `GET /datastore/{datastoreId}` in `region`.
async fn get_datastore_in(emulator: &Emulator, region: &str, datastore_id: &str) -> Answer {
    emulator
        .call_in(region, "GET", &format!("/datastore/{datastore_id}"), None)
        .await
}

/// Every `datastoreSummary` `ListDatastores` reports in `region`, paged to the end with
/// `page_size` so the walk itself is exercised per region rather than only its first
/// page.
async fn listed_datastores_in(emulator: &Emulator, region: &str, page_size: usize) -> Vec<Value> {
    let mut summaries: Vec<Value> = Vec::new();
    let mut next_token: Option<String> = None;
    // A page always carries at least one record or ends the walk, so the number of
    // requests is bounded by the number of records; the cap only keeps a paging bug from
    // hanging the suite.
    for _ in 0..=(MAX_DATASTORES + 1) {
        let mut uri = format!("/datastore?maxResults={page_size}");
        if let Some(token) = &next_token {
            uri.push_str(&format!("&nextToken={token}"));
        }
        let answer = emulator.call_in(region, "GET", &uri, None).await;
        assert_eq!(
            answer.status, 200,
            "case {region:?}: expected HTTP 200 from ListDatastores, got {answer:?}"
        );
        if let Some(page) = answer.field(&["datastoreSummaries"]).as_array() {
            summaries.extend(page.iter().cloned());
        }
        match answer.field(&["nextToken"]).as_str() {
            Some(token) => next_token = Some(token.to_string()),
            None => return summaries,
        }
    }
    panic!("case {region:?}: ListDatastores paged further than the collection can hold");
}

/// The `datastoreId` of each summary, as a set.
fn summary_ids(summaries: &[Value]) -> BTreeSet<String> {
    summaries
        .iter()
        .map(|summary| {
            summary["datastoreId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The ARN a data store created in `region` must report: the region is part of the
/// resource's identity, not decoration.
fn expected_datastore_arn(region: &str, datastore_id: &str) -> String {
    format!("arn:aws:medical-imaging:{region}:{ACCOUNT}:datastore/{datastore_id}")
}

// ---------------------------------------------------------------------------
// ListDatastores
// ---------------------------------------------------------------------------

/// One row of the listing table: a region, and how many data stores are created in it.
#[derive(Debug)]
struct RegionCase {
    region: &'static str,
    stores: usize,
}

/// RULE: `ListDatastores` enumerates exactly the data stores created in the region the
/// request was addressed to — none of another region's — and reports each one's ARN
/// naming that region.
///
/// Rows span the axes the rule generalises over: which region a request is addressed to,
/// and how many stores that region holds (none, one, several), with the listing paged a
/// record at a time so a page boundary cannot hide a store or admit a foreign one.
#[tokio::test]
async fn list_datastores_reports_only_the_data_stores_of_the_region_it_was_addressed_to() {
    let emulator = Emulator::new();
    let cases = [
        RegionCase {
            region: REGIONS[0],
            stores: 2,
        },
        RegionCase {
            region: REGIONS[1],
            stores: 1,
        },
        RegionCase {
            region: REGIONS[2],
            stores: 3,
        },
        RegionCase {
            // A region nothing is created in: its listing must be empty rather than
            // showing everyone else's stores.
            region: REGIONS[3],
            stores: 0,
        },
        RegionCase {
            region: REGIONS[4],
            stores: 1,
        },
    ];

    let mut created: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
    for case in &cases {
        for index in 0..case.stores {
            let datastore_id = created_datastore_id(
                &emulator,
                case.region,
                &format!("store.{}.{index}", case.region),
                &format!("token-{}-{index}", case.region),
            )
            .await;
            created
                .entry(case.region)
                .or_default()
                .insert(datastore_id.clone());
        }
    }
    // One settle for every store created above: the clock owes each the move to ACTIVE.
    tokio::time::sleep(SETTLE_MULTIPLE * DATASTORE_ACTIVATION_DELAY).await;

    for case in &cases {
        let expected = created.get(case.region).cloned().unwrap_or_default();
        let summaries = listed_datastores_in(&emulator, case.region, 1).await;
        let listed = summary_ids(&summaries);
        assert_eq!(
            listed, expected,
            "case {case:?}: expected exactly this region's data stores {expected:?}, got {listed:?}"
        );
        for summary in &summaries {
            let datastore_id = summary["datastoreId"].as_str().unwrap_or_default();
            let arn = summary["datastoreArn"].as_str().unwrap_or_default();
            assert_eq!(
                arn,
                expected_datastore_arn(case.region, datastore_id),
                "case {case:?}: expected the summary ARN to name this region, got {arn:?}"
            );
            let status = summary["datastoreStatus"].as_str().unwrap_or_default();
            assert_eq!(
                status, "ACTIVE",
                "case {case:?}: expected ACTIVE after the activation span, got {summary:?}"
            );
        }
    }

    // And the same store read by id: reachable from its own region, with the ARN of that
    // region, and nowhere else.
    for case in &cases {
        for datastore_id in created.get(case.region).into_iter().flatten() {
            let read = get_datastore_in(&emulator, case.region, datastore_id).await;
            assert_eq!(
                read.status, 200,
                "case {case:?} reading {datastore_id}: expected HTTP 200 in its own region, got {read:?}"
            );
            assert_eq!(
                read.string(&["datastoreProperties", "datastoreArn"]),
                expected_datastore_arn(case.region, datastore_id),
                "case {case:?} reading {datastore_id}: expected the ARN to name this region, got {read:?}"
            );
            for other in REGIONS.iter().filter(|other| **other != case.region) {
                let away = get_datastore_in(&emulator, other, datastore_id).await;
                assert_eq!(
                    away.status, 404,
                    "case {case:?} reading {datastore_id} from {other:?}: expected HTTP 404, got {away:?}"
                );
                assert_eq!(
                    away.error_code, "ResourceNotFoundException",
                    "case {case:?} reading {datastore_id} from {other:?}: expected ResourceNotFoundException, got {away:?}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Every id-keyed operation
// ---------------------------------------------------------------------------

/// One row of the id-keyed table: a request that names a data store, as the route and
/// body an SDK client sends, beside the text this service refuses an absent data store
/// with on that route — one constant stated by resource KIND, on the id-keyed routes and
/// on the three `@httpLabel`-keyed tagging routes alike.
#[derive(Debug)]
struct OperationCase {
    case: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
    missing_message: String,
}

/// A well-formed `StartDICOMImportJob` body.
fn import_job_body(client_token: &str) -> Value {
    json!({
        "clientToken": client_token,
        "dataAccessRoleArn": "arn:aws:iam::000000000000:role/ImportRole",
        "inputS3Uri": "s3://imaging-in/",
        "outputS3Uri": "s3://imaging-out/",
    })
}

/// Every operation that reaches a resource by its data store's id, one row each, in an
/// order that leaves the mutating ones last — so a row the control loop runs in the home
/// region cannot make a later row's resource missing.
fn id_keyed_operations(
    datastore_id: &str,
    image_set_id: &str,
    job_id: &str,
    datastore_arn: &str,
) -> Vec<OperationCase> {
    let tag_label = label(datastore_arn);
    // Every route — id-keyed or ARN-keyed — states the resource KIND that is missing.
    let missing_datastore = DATASTORE_NOT_FOUND_MESSAGE.to_string();
    vec![
        OperationCase {
            case: "GetDatastore",
            method: "GET",
            uri: format!("/datastore/{datastore_id}"),
            body: None,
            missing_message: missing_datastore.clone(),
        },
        OperationCase {
            case: "ListDICOMImportJobs",
            method: "GET",
            uri: format!("/listDICOMImportJobs/datastore/{datastore_id}"),
            body: None,
            missing_message: missing_datastore.clone(),
        },
        OperationCase {
            case: "GetDICOMImportJob",
            method: "GET",
            uri: format!("/getDICOMImportJob/datastore/{datastore_id}/job/{job_id}"),
            body: None,
            missing_message: missing_datastore.clone(),
        },
        OperationCase {
            case: "ListImageSetVersions",
            method: "POST",
            uri: format!("/datastore/{datastore_id}/imageSet/{image_set_id}/listImageSetVersions"),
            body: None,
            missing_message: missing_datastore.clone(),
        },
        OperationCase {
            case: "SearchImageSets",
            method: "POST",
            uri: format!("/datastore/{datastore_id}/searchImageSets"),
            // The body IS the `searchCriteria` payload; `{}` is the empty criteria,
            // which filters and sorts by nothing.
            body: Some(json!({})),
            missing_message: missing_datastore.clone(),
        },
        OperationCase {
            case: "GetImageSet",
            method: "POST",
            uri: format!("/datastore/{datastore_id}/imageSet/{image_set_id}/getImageSet"),
            body: None,
            missing_message: missing_datastore.clone(),
        },
        OperationCase {
            case: "GetImageFrame",
            method: "POST",
            uri: format!("/datastore/{datastore_id}/imageSet/{image_set_id}/getImageFrame"),
            body: Some(json!({ "imageFrameId": unheld_id() })),
            missing_message: missing_datastore.clone(),
        },
        OperationCase {
            case: "ListTagsForResource",
            method: "GET",
            uri: format!("/tags/{tag_label}"),
            body: None,
            missing_message: missing_datastore.clone(),
        },
        OperationCase {
            case: "TagResource",
            method: "POST",
            uri: format!("/tags/{tag_label}"),
            body: Some(json!({ "tags": { "crossregion": "written" } })),
            missing_message: missing_datastore.clone(),
        },
        OperationCase {
            case: "UntagResource",
            method: "DELETE",
            uri: format!("/tags/{tag_label}?tagKeys={}", label(SEEDED_TAGS[0].0)),
            body: None,
            missing_message: missing_datastore.clone(),
        },
        OperationCase {
            case: "StartDICOMImportJob",
            method: "POST",
            uri: format!("/startDICOMImportJob/datastore/{datastore_id}"),
            body: Some(import_job_body("cross-region-job-token")),
            missing_message: missing_datastore.clone(),
        },
        OperationCase {
            case: "CopyImageSet",
            method: "POST",
            uri: format!("/datastore/{datastore_id}/imageSet/{image_set_id}/copyImageSet"),
            body: Some(json!({ "sourceImageSet": { "latestVersionId": "1" } })),
            missing_message: missing_datastore.clone(),
        },
        OperationCase {
            case: "DeleteImageSet",
            method: "POST",
            uri: format!("/datastore/{datastore_id}/imageSet/{image_set_id}/deleteImageSet"),
            body: None,
            missing_message: missing_datastore.clone(),
        },
        OperationCase {
            case: "DeleteDatastore",
            method: "DELETE",
            uri: format!("/datastore/{datastore_id}"),
            body: None,
            missing_message: missing_datastore,
        },
    ]
}

/// RULE: every operation that names a data store resolves it in the region the request
/// was addressed to, so one held in another region is answered exactly as an id nothing
/// ever minted — `ResourceNotFoundException` (HTTP 404) — and the call changes nothing.
///
/// Rows span both axes the rule generalises over: every id-keyed operation this service
/// implements (reads, writes, listings and the three ARN-keyed tagging routes), against
/// every region other than the one holding the store. The control loop re-runs each row
/// in the home region, where the answer must NOT report a missing data store — proof the
/// 404 comes from the region and not from a malformed request.
#[tokio::test]
async fn an_operation_addressed_to_another_region_finds_no_data_store_and_changes_nothing() {
    let home = REGIONS[0];
    let emulator = Emulator::new();

    let datastore_id =
        created_datastore_id(&emulator, home, "cross.region", "cross-region-token").await;
    tokio::time::sleep(SETTLE_MULTIPLE * DATASTORE_ACTIVATION_DELAY).await;
    let image_set_id = seed_image_set(&emulator, &datastore_id, 1);
    let started = emulator
        .call_in(
            home,
            "POST",
            &format!("/startDICOMImportJob/datastore/{datastore_id}"),
            Some(import_job_body("home-region-job-token")),
        )
        .await;
    assert_eq!(
        started.status, 200,
        "seeding an import job in {home:?}: expected HTTP 200, got {started:?}"
    );
    let job_id = started.string(&["jobId"]);
    let datastore_arn = expected_datastore_arn(home, &datastore_id);

    let rows = id_keyed_operations(&datastore_id, &image_set_id, &job_id, &datastore_arn);
    for region in REGIONS.iter().filter(|region| **region != home) {
        for row in &rows {
            let answer = emulator
                .call_in(region, row.method, &row.uri, row.body.clone())
                .await;
            assert_eq!(
                answer.status, 404,
                "case {} from {region:?}: expected HTTP 404, got {answer:?}",
                row.case
            );
            assert_eq!(
                answer.error_code, "ResourceNotFoundException",
                "case {} from {region:?}: expected ResourceNotFoundException, got {answer:?}",
                row.case
            );
            assert_eq!(
                answer.string(&["message"]),
                row.missing_message,
                "case {} from {region:?}: expected the data store to be the missing resource, got {answer:?}",
                row.case
            );
        }
    }

    // Not one of those calls landed: the store is still there, still ACTIVE, and still
    // carries exactly the tags it was created with.
    let read = get_datastore_in(&emulator, home, &datastore_id).await;
    assert_eq!(
        read.status, 200,
        "after the cross-region calls: expected the home-region store to still be readable, got {read:?}"
    );
    assert_eq!(
        read.string(&["datastoreProperties", "datastoreStatus"]),
        "ACTIVE",
        "after the cross-region calls: expected ACTIVE, got {read:?}"
    );
    let tags = emulator
        .call_in(home, "GET", &format!("/tags/{}", label(&datastore_arn)), None)
        .await;
    let expected_tags: Value = SEEDED_TAGS
        .iter()
        .map(|(key, value)| ((*key).to_string(), Value::from(*value)))
        .collect::<serde_json::Map<String, Value>>()
        .into();
    assert_eq!(
        tags.field(&["tags"]),
        &expected_tags,
        "after the cross-region calls: expected the tag map untouched, got {tags:?}"
    );

    // The control: the same rows in the home region reach the data store. What each one
    // then answers is its own operation's business (a refusal about an image set or a
    // conflict is fine) — it just may not report the DATA STORE as missing.
    for row in &rows {
        let answer = emulator
            .call_in(home, row.method, &row.uri, row.body.clone())
            .await;
        let message = answer.string(&["message"]);
        assert_ne!(
            message, row.missing_message,
            "case {} from {home:?}: expected the data store to be reachable in its own region, got {answer:?}",
            row.case
        );
        // An answer carrying no message at all is a success, not a silent refusal:
        // without this, a row that answered an empty body would pass the line above
        // trivially.
        if message.is_empty() {
            assert!(
                answer.status < 300,
                "case {} from {home:?}: expected a success when the answer carries no message, got {answer:?}",
                row.case
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Idempotency and the quota
// ---------------------------------------------------------------------------

/// RULE: a `clientToken` names the resource it minted in the region it was used in, so
/// the same token replayed against another region mints that region's own data store
/// rather than reporting one the caller cannot reach.
///
/// Rows are the regions the one token is replayed against.
#[tokio::test]
async fn an_idempotency_token_mints_one_data_store_per_region() {
    const SHARED_TOKEN: &str = "one-token-every-region";
    let emulator = Emulator::new();

    let mut minted: BTreeMap<&str, String> = BTreeMap::new();
    for region in REGIONS {
        let first = create_datastore_in(&emulator, region, "token.scope", SHARED_TOKEN).await;
        assert_eq!(
            first.status, 200,
            "case {region:?}: expected HTTP 200 from the first create, got {first:?}"
        );
        let repeat = create_datastore_in(&emulator, region, "token.scope", SHARED_TOKEN).await;
        assert_eq!(
            repeat.string(&["datastoreId"]),
            first.string(&["datastoreId"]),
            "case {region:?}: expected the repeat call to report the SAME data store, got {repeat:?}"
        );
        minted.insert(region, first.string(&["datastoreId"]));
    }

    let distinct: BTreeSet<&String> = minted.values().collect();
    assert_eq!(
        distinct.len(),
        minted.len(),
        "expected one data store per region for the shared token, got {minted:?}"
    );

    tokio::time::sleep(SETTLE_MULTIPLE * DATASTORE_ACTIVATION_DELAY).await;
    for (region, datastore_id) in &minted {
        let summaries = listed_datastores_in(&emulator, region, 50).await;
        let listed = summary_ids(&summaries);
        let expected: BTreeSet<String> = [datastore_id.clone()].into_iter().collect();
        assert_eq!(
            listed, expected,
            "case {region:?}: expected only the store this region minted, got {listed:?}"
        );
    }
}

/// RULE: the data store capacity bound is charged to one region, so filling one region
/// to the cap does not refuse a create in another.
///
/// Rows: the region already at the cap, and a second region still empty.
#[tokio::test]
async fn the_data_store_capacity_bound_is_charged_per_region() {
    let full = REGIONS[0];
    let spare = REGIONS[1];
    let emulator = Emulator::new();

    for index in 0..MAX_DATASTORES {
        let created = create_datastore_in(
            &emulator,
            full,
            &format!("cap.store-{index}"),
            &format!("cap-token-{index}"),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {full:?} store {index} of {MAX_DATASTORES}: expected HTTP 200 at or below the cap, got {created:?}"
        );
    }

    let over = create_datastore_in(&emulator, full, "cap.store-over", "cap-token-over").await;
    assert_eq!(
        over.status, 402,
        "case {full:?} one store past the cap: expected HTTP 402, got {over:?}"
    );
    assert_eq!(
        over.error_code, "ServiceQuotaExceededException",
        "case {full:?} one store past the cap: expected ServiceQuotaExceededException, got {over:?}"
    );

    let elsewhere = create_datastore_in(&emulator, spare, "cap.elsewhere", "cap-token-spare").await;
    assert_eq!(
        elsewhere.status, 200,
        "case {spare:?} with {full:?} at the cap: expected HTTP 200, got {elsewhere:?}"
    );
    let datastore_id = elsewhere.string(&["datastoreId"]);
    tokio::time::sleep(SETTLE_MULTIPLE * DATASTORE_ACTIVATION_DELAY).await;
    let summaries = listed_datastores_in(&emulator, spare, 50).await;
    let listed = summary_ids(&summaries);
    let expected: BTreeSet<String> = [datastore_id].into_iter().collect();
    assert_eq!(
        listed, expected,
        "case {spare:?}: expected only its own store, got {listed:?}"
    );
}
