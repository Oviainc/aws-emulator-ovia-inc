// SPDX-License-Identifier: CC0-1.0

//! What a page token is bound to across REGIONS, end to end over HTTP.
//!
//! Every case drives the real request path (`MultiServiceRouter::dispatch`, the
//! same entry the served emulator uses), so the route, the status code and the
//! serialized body are all exercised, not just a handler body.
//!
//! RULE under test: a `nextToken` resumes a walk only inside the one sequence it
//! was taken from, and this service is regional — each Region answers out of its
//! own store, where the same id names an independent resource — so a token is
//! bound to the Region that minted it: another Region's endpoint refuses it with
//! the 400 `ValidationException` every paginated operation reports for a token it
//! did not mint, instead of positioning that Region's own walk at the key and
//! serving a page with every earlier row of it missing.
//!
//! The rows differ along what the rule generalises over: WHICH paginated
//! operation minted the token (every one this service serves — account-level,
//! workspace-level, entity-level, ARN-addressed and the data plane, resuming at a
//! key and at an ordinal), and WHICH two Regions are involved (two of one
//! continent, two of different continents, two whose names share a prefix, and
//! two whose middle segments differ, so a store or a token keyed by a prefix of
//! the Region would be caught). Both Regions are seeded with the very same ids,
//! because "the same id names an independent resource in each Region" is what
//! makes a cross-Region resume able to skip rows silently.
//!
//! The companion case pins the other side of the rule, so it cannot be satisfied
//! by refusing everything: the token is still resumed by the Region that minted
//! it, and the other Region's own first page still reports its own first row.

use std::collections::BTreeMap;
use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_iottwinmaker_awsiottwinmaker::state::{
    sync_job_arn, workspace_arn, ResourceRef, SyncJob, SyncResource, DATA_WRITER_FUNCTION,
    SYNC_SOURCE_SITEWISE,
};

use crate::harness::{Emulator, Resp, ACCOUNT, SYNC_ROLE};

/// The Region PAIRS every case runs over: `(home, other)`, where `home` mints
/// the token and `other` is the endpoint that must refuse it.
const REGION_PAIRS: [(&str, &str); 4] = [
    ("us-east-1", "eu-west-1"),
    ("ap-southeast-2", "ca-central-1"),
    ("us-east-1", "us-east-2"),
    ("eu-central-1", "ap-northeast-3"),
];

/// The workspace every workspace-scoped page below is taken over. ONE id, seeded
/// in both Regions of a row.
const WORKSPACE: &str = "alpha";
/// A second workspace, so the account-level listing has a page to leave over.
const OTHER_WORKSPACE: &str = "beta";
/// The component type the seeded entities instantiate: a time-series property
/// for the history read, and two valued properties in a `TABULAR` group for the
/// latest-value and table reads.
const SENSOR: &str = "com.example.regionSensor";
/// A second sync source, so the sync-job listing has a page to leave over: the
/// create path serves one source, so a job under any other is seeded.
const SECOND_SOURCE: &str = "NEIGHBOUR";
/// Two instants one minute apart, as the ISO 8601 `Time` a write carries, so the
/// history read has a value to leave over.
const T0: &str = "2026-01-01T00:00:00Z";
/// See [`T0`].
const T1: &str = "2026-01-01T00:01:00Z";

/// The ONE sentence every paginated operation of this service reports for a
/// token it did not mint. Written out here rather than read from the crate, so
/// this asserts the wording instead of restating the implementation.
const REFUSAL: &str = "nextToken is not a token this listing minted";

/// The `functions` map a component type declares its WRITE CONNECTOR in, so the
/// data plane accepts a write to its properties; the Lambda it names sits in the
/// Region the component type is created through.
fn data_writer_functions(region: &str) -> Value {
    let writer = DATA_WRITER_FUNCTION;
    json!({
        writer: {
            "scope": "ENTITY",
            "implementedBy": {
                "lambda": {"arn": format!("arn:aws:lambda:{region}:{ACCOUNT}:function:writer")},
            },
        },
    })
}

/// One dispatched request signed for `region`, asserted to have been served.
async fn served(emulator: &Emulator, region: &str, what: &str, path: &str, body: Value) -> Resp {
    let resp = emulator
        .call_signed_for(Some(region), "POST", path, Some(body))
        .await;
    assert_eq!(
        resp.status, 200,
        "seeding {region}: expected 200 from {what}, got {resp:?}"
    );
    resp
}

/// Give `region`'s own store the workspaces, component type, entities, scenes,
/// tags, sync jobs, metadata transfer jobs and time-series values every page
/// below is taken over.
///
/// Called for BOTH Regions of a row with the same ids, so each Region holds an
/// independent twin of the same resources — the state in which a token resumed
/// at the wrong endpoint would land on a row that exists there and quietly skip
/// everything sorting before it.
async fn seed(emulator: &Emulator, region: &str) {
    for workspace_id in [WORKSPACE, OTHER_WORKSPACE] {
        served(
            emulator,
            region,
            "CreateWorkspace",
            &format!("/workspaces/{workspace_id}"),
            json!({}),
        )
        .await;
    }

    served(
        emulator,
        region,
        "CreateComponentType",
        &format!("/workspaces/{WORKSPACE}/component-types/{SENSOR}"),
        json!({
            "propertyDefinitions": {
                "Temp": {"dataType": {"type": "DOUBLE"}, "isTimeSeries": true},
                "Tag": {
                    "dataType": {"type": "STRING"},
                    "defaultValue": {"stringValue": "unset-tag"},
                },
                "Threshold": {
                    "dataType": {"type": "DOUBLE"},
                    "defaultValue": {"doubleValue": 1.5},
                },
            },
            "propertyGroups": {
                "Readings": {"groupType": "TABULAR", "propertyNames": ["Tag", "Threshold"]},
            },
            "functions": data_writer_functions(region),
        }),
    )
    .await;

    // Two entities, each carrying two components, so the entity listing and the
    // component listing both have a page to leave over.
    for entity_id in ["unit-1", "unit-2"] {
        served(
            emulator,
            region,
            "CreateEntity",
            &format!("/workspaces/{WORKSPACE}/entities"),
            json!({
                "entityId": entity_id,
                "entityName": entity_id,
                "components": {
                    "Sensor": {"componentTypeId": SENSOR},
                    "Spare": {"componentTypeId": SENSOR},
                },
            }),
        )
        .await;
    }

    for scene_id in ["scene-1", "scene-2"] {
        served(
            emulator,
            region,
            "CreateScene",
            &format!("/workspaces/{WORKSPACE}/scenes"),
            json!({
                "sceneId": scene_id,
                "contentLocation": format!("s3://bucket/{scene_id}.json"),
            }),
        )
        .await;
    }

    // Two tags on the workspace of THIS Region, addressed by the ARN that Region
    // minted, so the tag listing has a page to leave over.
    served(
        emulator,
        region,
        "TagResource",
        "/tags",
        json!({
            "resourceARN": workspace_arn(region, WORKSPACE),
            "tags": {"env": "test", "owner": "qa"},
        }),
    )
    .await;

    served(
        emulator,
        region,
        "CreateSyncJob",
        &format!("/workspaces/{WORKSPACE}/sync-jobs/{SYNC_SOURCE_SITEWISE}"),
        json!({"syncRole": SYNC_ROLE}),
    )
    .await;
    seed_second_sync_job(emulator, region);
    seed_sync_resources(emulator, region);

    for job_id in ["mtj-alpha", "mtj-beta"] {
        served(
            emulator,
            region,
            "CreateMetadataTransferJob",
            "/metadata-transfer-jobs",
            json!({
                "metadataTransferJobId": job_id,
                "sources": [{
                    "type": "s3",
                    "s3Configuration": {
                        "location": "arn:aws:s3:::metadata-bucket/entities.json",
                    },
                }],
                "destination": {"type": "iotsitewise"},
            }),
        )
        .await;
    }

    served(
        emulator,
        region,
        "BatchPutPropertyValues",
        &format!("/workspaces/{WORKSPACE}/entity-properties"),
        json!({"entries": [{
            "entityPropertyReference": {
                "entityId": "unit-1",
                "componentName": "Sensor",
                "propertyName": "Temp",
            },
            "propertyValues": [
                {"time": T0, "value": {"doubleValue": 10.0}},
                {"time": T1, "value": {"doubleValue": 20.0}},
            ],
        }]}),
    )
    .await;
}

/// Store a second sync job in `region`'s own store, written exactly as
/// `CreateSyncJob` writes one: the record under its source, and its ARN in that
/// Region's index. Seeded rather than created because the create path serves one
/// source per workspace.
fn seed_second_sync_job(emulator: &Emulator, region: &str) {
    let arn = sync_job_arn(region, WORKSPACE, SECOND_SOURCE);
    let state = emulator.state();
    let mut state = state.lock();
    let state = state.region_mut(region);
    let workspace = state
        .workspaces
        .get_mut(WORKSPACE)
        .unwrap_or_else(|| panic!("{region}: the workspace {WORKSPACE:?} must exist first"));
    workspace.sync_jobs.insert(
        SECOND_SOURCE.to_string(),
        SyncJob::accepted(
            SECOND_SOURCE.to_string(),
            arn.clone(),
            WORKSPACE.to_string(),
            SYNC_ROLE.to_string(),
            BTreeMap::new(),
            SystemTime::now(),
        ),
    );
    state.arn_index.insert(
        arn,
        ResourceRef::SyncJob {
            workspace_id: WORKSPACE.to_string(),
            sync_source: SECOND_SOURCE.to_string(),
        },
    );
}

/// Store two resources as the ones the created sync job of `region` holds, so
/// the sync-resource listing has a page to leave over.
///
/// Seeded because a sync resource is written by a synchronisation, which this
/// emulator does not perform, so no operation of it writes one.
fn seed_sync_resources(emulator: &Emulator, region: &str) {
    let state = emulator.state();
    let mut state = state.lock();
    let state = state.region_mut(region);
    let job = state
        .workspaces
        .get_mut(WORKSPACE)
        .and_then(|workspace| workspace.sync_jobs.get_mut(SYNC_SOURCE_SITEWISE))
        .unwrap_or_else(|| panic!("{region}: the created sync job must exist first"));
    for external_id in ["sitewiseAsset_alpha", "sitewiseAsset_beta"] {
        job.sync_resources.insert(
            external_id.to_string(),
            SyncResource {
                resource_type: "ENTITY".to_string(),
                external_id: external_id.to_string(),
                resource_id: Some("unit-1".to_string()),
                state: "IN_SYNC".to_string(),
                error: None,
                update_date_time: SystemTime::now(),
            },
        );
    }
}

/// One paginated request: the route it is served on, and the members it carries
/// besides `nextToken` — a page size small enough that the seeded state leaves a
/// page over, so every row below really mints a token.
struct Paginated {
    /// The operation, for the assertion messages.
    operation: &'static str,
    path: String,
    body: Value,
}

/// Every paginated request this service serves, each otherwise valid and
/// addressed to `region`'s own store, so the only thing wrong with it is the
/// token a case puts in it.
fn paginated_requests(region: &str) -> Vec<Paginated> {
    let paginated = |operation: &'static str, path: String, mut body: Value| {
        body["maxResults"] = json!(1);
        Paginated {
            operation,
            path,
            body,
        }
    };
    vec![
        paginated("ListWorkspaces", "/workspaces-list".to_string(), json!({})),
        paginated(
            "ListEntities",
            format!("/workspaces/{WORKSPACE}/entities-list"),
            json!({}),
        ),
        paginated(
            "ListComponentTypes",
            format!("/workspaces/{WORKSPACE}/component-types-list"),
            json!({}),
        ),
        paginated(
            "ListComponents",
            format!("/workspaces/{WORKSPACE}/entities/unit-1/components-list"),
            json!({}),
        ),
        paginated(
            "ListProperties",
            format!("/workspaces/{WORKSPACE}/properties-list"),
            json!({"entityId": "unit-1", "componentName": "Sensor"}),
        ),
        paginated(
            "ListScenes",
            format!("/workspaces/{WORKSPACE}/scenes-list"),
            json!({}),
        ),
        paginated(
            "ListSyncJobs",
            format!("/workspaces/{WORKSPACE}/sync-jobs-list"),
            json!({}),
        ),
        paginated(
            "ListSyncResources",
            format!("/workspaces/{WORKSPACE}/sync-jobs/{SYNC_SOURCE_SITEWISE}/resources-list"),
            json!({}),
        ),
        paginated(
            "ListMetadataTransferJobs",
            "/metadata-transfer-jobs-list".to_string(),
            json!({"sourceType": "s3", "destinationType": "iotsitewise"}),
        ),
        paginated(
            "ListTagsForResource",
            "/tags-list".to_string(),
            // The ARN of THIS Region's workspace: an ARN carries the Region that
            // minted it, so each endpoint is asked about its own resource.
            json!({"resourceARN": workspace_arn(region, WORKSPACE)}),
        ),
        paginated(
            "ExecuteQuery",
            "/queries/execution".to_string(),
            json!({
                "workspaceId": WORKSPACE,
                "queryStatement": "SELECT e FROM EntityGraph MATCH (e)",
            }),
        ),
        paginated(
            "GetPropertyValue (latest values)",
            format!("/workspaces/{WORKSPACE}/entity-properties/value"),
            json!({
                "entityId": "unit-1",
                "componentName": "Sensor",
                "selectedProperties": ["Tag", "Threshold"],
            }),
        ),
        paginated(
            "GetPropertyValue (tabular rows)",
            format!("/workspaces/{WORKSPACE}/entity-properties/value"),
            json!({
                "componentTypeId": SENSOR,
                "selectedProperties": ["Tag", "Threshold"],
                "tabularConditions": {"orderBy": [{"propertyName": "Tag"}]},
            }),
        ),
        paginated(
            "GetPropertyValueHistory",
            format!("/workspaces/{WORKSPACE}/entity-properties/history"),
            json!({
                "entityId": "unit-1",
                "componentName": "Sensor",
                "selectedProperties": ["Temp"],
            }),
        ),
    ]
}

/// The `message` an error body carries.
fn message(resp: &Resp) -> String {
    resp.member("message")
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The `nextToken` a response carries.
fn token_of(case: &str, resp: &Resp) -> String {
    resp.member("nextToken")
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case:?}: the seeded state leaves a page over, so a token must be minted, got {resp:?}")
        })
        .to_string()
}

/// A page token minted at one Region's endpoint is not one the other Region's
/// listing minted: it is refused there, while the Region that minted it still
/// resumes it and the other Region's own first page still reports its own rows.
#[tokio::test]
async fn a_page_token_is_bound_to_the_region_that_minted_it() {
    for (home, other) in REGION_PAIRS {
        let emulator = Emulator::new();
        seed(&emulator, home).await;
        seed(&emulator, other).await;

        let home_requests = paginated_requests(home);
        let other_requests = paginated_requests(other);
        for (home_request, other_request) in home_requests.iter().zip(other_requests.iter()) {
            let case = format!("{} {home} then {other}", home_request.operation);
            assert_eq!(
                home_request.operation, other_request.operation,
                "case {case:?}: the two Regions' request tables must line up"
            );

            let first = emulator
                .call_signed_for(
                    Some(home),
                    "POST",
                    &home_request.path,
                    Some(home_request.body.clone()),
                )
                .await;
            assert_eq!(
                first.status, 200,
                "case {case:?}: expected 200 from the first page, got {first:?}"
            );
            let token = token_of(&case, &first);

            // The other Region holds its own twin of the same resources, so its
            // own first page serves rows a silently resumed token would skip.
            let theirs = emulator
                .call_signed_for(
                    Some(other),
                    "POST",
                    &other_request.path,
                    Some(other_request.body.clone()),
                )
                .await;
            assert_eq!(
                theirs.status, 200,
                "case {case:?}: expected 200 from the other Region's own first page, got {theirs:?}"
            );
            let theirs_token = token_of(&case, &theirs);
            assert_ne!(
                theirs_token, token,
                "case {case:?}: two Regions' tokens over independent stores must differ, \
                 got {theirs_token:?}"
            );

            // The cross-Region resume: the other endpoint never minted this
            // token, so it refuses it instead of positioning its own walk at the
            // key.
            let mut resumed_elsewhere = other_request.body.clone();
            resumed_elsewhere["nextToken"] = json!(token);
            let refused = emulator
                .call_signed_for(
                    Some(other),
                    "POST",
                    &other_request.path,
                    Some(resumed_elsewhere),
                )
                .await;
            assert_eq!(
                (refused.status, refused.error_type.as_deref()),
                (400, Some("ValidationException")),
                "case {case:?}: expected a 400 ValidationException from the other Region, \
                 got {refused:?}"
            );
            let reported = message(&refused);
            assert_eq!(
                reported, REFUSAL,
                "case {case:?}: expected the message {REFUSAL:?}, got {reported:?}"
            );

            // The other side of the rule: the Region that minted the token still
            // resumes it, and the page it serves is not the one it already
            // served.
            let mut resumed_at_home = home_request.body.clone();
            resumed_at_home["nextToken"] = json!(token);
            let resumed = emulator
                .call_signed_for(Some(home), "POST", &home_request.path, Some(resumed_at_home))
                .await;
            assert_eq!(
                resumed.status, 200,
                "case {case:?}: expected the minting Region to resume its own token, \
                 got {resumed:?}"
            );
            assert_ne!(
                resumed.body, first.body,
                "case {case:?}: a resumed page must report what the first page left over, \
                 got {resumed:?}"
            );
        }
    }
}

/// The plainest case, spelled out: each Region holds its own `alpha` and `beta`,
/// so a token naming `beta` must not be accepted at the other Region's endpoint,
/// where it would skip that Region's `alpha` with an HTTP 200 and no indication
/// that the page was cut.
#[tokio::test]
async fn another_regions_token_cannot_skip_this_regions_earlier_rows() {
    for (home, other) in REGION_PAIRS {
        let case = format!("{home} then {other}");
        let emulator = Emulator::new();
        for region in [home, other] {
            for workspace_id in [WORKSPACE, OTHER_WORKSPACE] {
                served(
                    &emulator,
                    region,
                    "CreateWorkspace",
                    &format!("/workspaces/{workspace_id}"),
                    json!({}),
                )
                .await;
            }
        }

        let listed_ids = |resp: &Resp| -> Vec<String> {
            resp.member("workspaceSummaries")
                .as_array()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: workspaceSummaries must be a present array, got {resp:?}")
                })
                .iter()
                .map(|summary| summary["workspaceId"].as_str().unwrap_or("").to_string())
                .collect()
        };

        let first = served(
            &emulator,
            home,
            "ListWorkspaces",
            "/workspaces-list",
            json!({"maxResults": 1}),
        )
        .await;
        assert_eq!(
            listed_ids(&first),
            vec![WORKSPACE.to_string()],
            "case {case:?}: expected the first page to report the first id, got {first:?}"
        );
        let token = token_of(&case, &first);

        let refused = emulator
            .call_signed_for(
                Some(other),
                "POST",
                "/workspaces-list",
                Some(json!({"maxResults": 1, "nextToken": token.clone()})),
            )
            .await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: expected the other Region to refuse the token, got {refused:?}"
        );
        assert_eq!(
            message(&refused),
            REFUSAL,
            "case {case:?}: expected the message {REFUSAL:?}, got {refused:?}"
        );

        // Unresumed, the other Region reports its own first workspace — the row
        // the refused call would have skipped.
        let theirs = served(
            &emulator,
            other,
            "ListWorkspaces",
            "/workspaces-list",
            json!({"maxResults": 1}),
        )
        .await;
        assert_eq!(
            listed_ids(&theirs),
            vec![WORKSPACE.to_string()],
            "case {case:?}: expected the other Region to report its own first id, got {theirs:?}"
        );

        // And the minting Region resumes at the row its own first page left over.
        let resumed = served(
            &emulator,
            home,
            "ListWorkspaces",
            "/workspaces-list",
            json!({"maxResults": 1, "nextToken": token}),
        )
        .await;
        assert_eq!(
            listed_ids(&resumed),
            vec![OTHER_WORKSPACE.to_string()],
            "case {case:?}: expected the resumed page to report the row left over, got {resumed:?}"
        );
    }
}
