// SPDX-License-Identifier: CC0-1.0

//! How a `nextToken` of this service is SPELLED, end to end over HTTP.
//!
//! Every case drives the real request path (`MultiServiceRouter::dispatch`, the
//! same entry the served emulator uses), so the route, the status code and the
//! serialized body are all exercised, not just a handler body.
//!
//! RULE under test: a page token is this service's own opaque value, minted in
//! exactly ONE rendering per cursor, so a paginated operation resumes only the
//! value a caller was really handed, byte for byte — every other rendering of
//! the same cursor is a token nothing minted and earns the 400
//! `ValidationException` every paginated operation reports for a token it did
//! not mint, instead of being resumed as though the service had issued it.
//!
//! The rows differ along what the rule generalises over: WHICH paginated
//! operation minted the token (every one this service serves — account-level,
//! workspace-level, entity-level, ARN-addressed and the data plane, resuming at
//! a stored key and at a row's position), and HOW the token is re-rendered
//! (every digit of it re-cased, one half of it re-cased, and a single digit at
//! either end of it re-cased, so a check made over only the start or only the
//! end of the value would be caught). Each case is derived from a token the
//! service itself minted and is asserted to decode to the very same bytes, so a
//! row changes the RENDERING and nothing else: the cursor it carries is the
//! service's own.
//!
//! The companion cases pin the other side of the rule, so it cannot be satisfied
//! by refusing everything: the token as it was handed out still resumes, and a
//! walk made of the service's own tokens still partitions the listing exactly
//! once whatever the page size, with the re-rendered tokens refused at every
//! step of it.

use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_iottwinmaker_awsiottwinmaker::state::{
    SyncResource, MAX_PAGE_SIZE, SYNC_SOURCE_SITEWISE,
};

use crate::harness::{
    data_writer_functions, seed_sync_job, workspace_arn, Emulator, Resp, REGION,
};

/// The workspace every workspace-scoped page below is taken over.
const WORKSPACE: &str = "tokenSpellingWs";
/// A second workspace, so the account-level listing has a page to leave over.
const OTHER_WORKSPACE: &str = "tokenSpellingWsTwo";
/// The component type the seeded entities instantiate: a time-series property
/// for the history read, and two valued properties in a `TABULAR` group for the
/// latest-value and table reads.
const SENSOR: &str = "com.example.sensorSpelling";
/// The component the seeded entities carry [`SENSOR`] under.
const COMPONENT: &str = "Sensor";
/// A second sync source, so the sync-job listing has a page to leave over.
const SECOND_SOURCE: &str = "NEIGHBOUR";
/// Two instants one minute apart, as the ISO 8601 `Time` a write carries, so the
/// history read has a value to leave over.
const T0: &str = "2026-03-01T00:00:00Z";
/// See [`T0`].
const T1: &str = "2026-03-01T00:01:00Z";

/// The ONE sentence every paginated operation of this service reports for a
/// token it did not mint. Written out here rather than read from the crate, so
/// this asserts the wording instead of restating the implementation.
const REFUSAL: &str = "nextToken is not a token this listing minted";

/// The workspaces, component type, entities, scenes, tags, sync jobs, sync
/// resources, metadata transfer jobs and time-series values every page below is
/// taken over — at least two rows per sequence, so every one of them leaves a
/// page over and really mints a token.
async fn seed(emulator: &Emulator) {
    for workspace_id in [WORKSPACE, OTHER_WORKSPACE] {
        let created = emulator.create_workspace(workspace_id).await;
        assert_eq!(
            created.status, 200,
            "CreateWorkspace {workspace_id:?}: expected 200, got {created:?}"
        );
    }

    let created = emulator
        .create_component_type(
            WORKSPACE,
            SENSOR,
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
                // The write connector the seeded time series values are set THROUGH.
                "functions": data_writer_functions(),
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "CreateComponentType {SENSOR:?}: expected 200, got {created:?}"
    );

    // Two entities, each carrying two components, so the entity listing and the
    // component listing both have a page to leave over.
    for entity_id in ["unit-1", "unit-2"] {
        let created = emulator
            .create_entity(
                WORKSPACE,
                json!({
                    "entityId": entity_id,
                    "entityName": entity_id,
                    "components": {
                        COMPONENT: {"componentTypeId": SENSOR},
                        "Spare": {"componentTypeId": SENSOR},
                    },
                }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "CreateEntity {entity_id:?}: expected 200, got {created:?}"
        );
    }

    for scene_id in ["scene-1", "scene-2"] {
        let created = emulator
            .create_scene(
                WORKSPACE,
                json!({
                    "sceneId": scene_id,
                    "contentLocation": format!("s3://bucket/{scene_id}.json"),
                }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "CreateScene {scene_id:?}: expected 200, got {created:?}"
        );
    }

    let tagged = emulator
        .tag_resource(
            &workspace_arn(WORKSPACE),
            json!({"env": "test", "owner": "qa"}),
        )
        .await;
    assert_eq!(
        tagged.status, 200,
        "TagResource: expected 200, got {tagged:?}"
    );

    // Two sync jobs, so the sync-job listing has a page to leave over: the
    // create path serves one source, so both are written into the store the
    // handlers read, exactly as `CreateSyncJob` writes one.
    seed_sync_job(emulator, WORKSPACE, SYNC_SOURCE_SITEWISE);
    seed_sync_job(emulator, WORKSPACE, SECOND_SOURCE);
    seed_sync_resources(emulator);

    for job_id in ["mtj-alpha", "mtj-beta"] {
        let created = emulator
            .create_metadata_transfer_job(json!({
                "metadataTransferJobId": job_id,
                "sources": [{
                    "type": "s3",
                    "s3Configuration": {
                        "location": "arn:aws:s3:::metadata-bucket/entities.json",
                    },
                }],
                "destination": {"type": "iotsitewise"},
            }))
            .await;
        assert_eq!(
            created.status, 200,
            "CreateMetadataTransferJob {job_id:?}: expected 200, got {created:?}"
        );
    }

    let written = emulator
        .batch_put_property_values(
            WORKSPACE,
            json!([{
                "entityPropertyReference": {
                    "entityId": "unit-1",
                    "componentName": COMPONENT,
                    "propertyName": "Temp",
                },
                "propertyValues": [
                    {"time": T0, "value": {"doubleValue": 10.0}},
                    {"time": T1, "value": {"doubleValue": 20.0}},
                ],
            }]),
        )
        .await;
    assert_eq!(
        written.status, 200,
        "BatchPutPropertyValues seeding the series: expected 200, got {written:?}"
    );
}

/// Store two resources as the ones the seeded sync job holds, so the
/// sync-resource listing has a page to leave over.
///
/// Seeded because a sync resource is written by a synchronisation, which this
/// emulator does not perform, so no operation of it writes one.
fn seed_sync_resources(emulator: &Emulator) {
    let state = emulator.state();
    let mut state = state.lock();
    let state = state.region_mut(REGION);
    let job = state
        .workspaces
        .get_mut(WORKSPACE)
        .and_then(|workspace| workspace.sync_jobs.get_mut(SYNC_SOURCE_SITEWISE))
        .expect("the seeded sync job must exist first");
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
/// besides `nextToken`.
struct Paginated {
    /// The operation, for the assertion messages.
    operation: &'static str,
    path: String,
    body: Value,
}

/// Every paginated request this service serves, each otherwise valid and cut to
/// one row a page, so the only thing a case changes is the token's rendering.
fn paginated_requests() -> Vec<Paginated> {
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
            json!({"entityId": "unit-1", "componentName": COMPONENT}),
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
            json!({"resourceARN": workspace_arn(WORKSPACE)}),
        ),
        paginated(
            "ExecuteQuery",
            "/queries/execution".to_string(),
            json!({
                "workspaceId": WORKSPACE,
                "queryStatement": "SELECT e.entityId FROM EntityGraph MATCH (e)",
            }),
        ),
        paginated(
            "GetPropertyValue (latest values)",
            format!("/workspaces/{WORKSPACE}/entity-properties/value"),
            json!({
                "entityId": "unit-1",
                "componentName": COMPONENT,
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
                "componentName": COMPONENT,
                "selectedProperties": ["Temp"],
            }),
        ),
    ]
}

/// `token` with the hex digit at `at` upper-cased, where that digit is a letter
/// (the digits `0`-`9` have one case, so re-casing one of them renders the same
/// value).
fn with_digit_upper_cased(token: &str, at: usize) -> String {
    token
        .chars()
        .enumerate()
        .map(|(index, digit)| {
            if index == at {
                digit.to_ascii_uppercase()
            } else {
                digit
            }
        })
        .collect()
}

/// The position of a hex LETTER of `token`: the first one when `from_start`, the
/// last one otherwise. A token is ASCII hex, so a byte of it is a digit of it.
fn letter_position(token: &str, from_start: bool) -> usize {
    let found = if from_start {
        token.bytes().position(|digit| digit.is_ascii_alphabetic())
    } else {
        token.bytes().rposition(|digit| digit.is_ascii_alphabetic())
    };
    found.unwrap_or_else(|| {
        panic!("a minted token's envelope spells letters in hex, got {token:?}")
    })
}

/// Every rendering of `issued` that is NOT the one the service handed out, each
/// carrying the very same cursor bytes: the value re-cased whole, re-cased over
/// its first half, and re-cased at a single letter at either end of it.
fn respellings(issued: &str) -> Vec<(&'static str, String)> {
    let half = issued.len() / 2;
    let first_half_upper_cased = format!("{}{}", issued[..half].to_uppercase(), &issued[half..]);
    vec![
        ("everyDigitUpperCased", issued.to_uppercase()),
        ("theFirstHalfUpperCased", first_half_upper_cased),
        (
            "theFirstLetterUpperCased",
            with_digit_upper_cased(issued, letter_position(issued, true)),
        ),
        (
            "theLastLetterUpperCased",
            with_digit_upper_cased(issued, letter_position(issued, false)),
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

/// The `componentTypeId`s a `ListComponentTypes` page reported, in the order it
/// reported them.
fn component_type_ids(case: &str, resp: &Resp) -> Vec<String> {
    resp.member("componentTypeSummaries")
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: componentTypeSummaries must be a present array, got {resp:?}")
        })
        .iter()
        .map(|summary| summary["componentTypeId"].as_str().unwrap_or("").to_string())
        .collect()
}

/// RULE: a token rendered any way other than the way it was handed out is not a
/// token this listing minted — it is refused with the 400 `ValidationException`
/// every paginated operation reports for such a token, whichever paginated
/// operation minted it and however the value is re-rendered, while the value as
/// it was handed out still resumes.
#[tokio::test]
async fn a_page_token_rendered_another_way_is_not_one_this_listing_minted() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    for request in paginated_requests() {
        let first = emulator
            .call("POST", &request.path, Some(request.body.clone()))
            .await;
        assert_eq!(
            first.status, 200,
            "case {:?}: expected 200 from the first page, got {first:?}",
            request.operation
        );
        let issued = token_of(request.operation, &first);

        for (what, respelt) in respellings(&issued) {
            let case = format!("{}/{what}", request.operation);
            assert_ne!(
                respelt, issued,
                "case {case:?}: a row must render the token differently from the way it was \
                 handed out, got {respelt:?}"
            );
            assert_eq!(
                hex::decode(&respelt).ok(),
                hex::decode(&issued).ok(),
                "case {case:?}: a row must carry the very same cursor bytes, got {respelt:?}"
            );

            let mut body = request.body.clone();
            body["nextToken"] = json!(respelt);
            let refused = emulator.call("POST", &request.path, Some(body)).await;
            assert_eq!(
                (refused.status, refused.error_type.as_deref()),
                (400, Some("ValidationException")),
                "case {case:?}: expected a 400 ValidationException, got {refused:?}"
            );
            let reported = message(&refused);
            assert_eq!(
                reported, REFUSAL,
                "case {case:?}: expected the message {REFUSAL:?}, got {reported:?}"
            );
        }

        // The other side of the rule: the value as it was handed out resumes,
        // and the page it serves is not the one already served.
        let mut body = request.body.clone();
        body["nextToken"] = json!(issued);
        let resumed = emulator.call("POST", &request.path, Some(body)).await;
        assert_eq!(
            resumed.status, 200,
            "case {:?}: expected the token as handed out to resume, got {resumed:?}",
            request.operation
        );
        assert_ne!(
            resumed.body, first.body,
            "case {:?}: a resumed page must report what the first page left over, got {resumed:?}",
            request.operation
        );
    }
}

/// RULE, over a whole walk: a re-rendered token is refused at every step and
/// changes nothing, while the walk made of the service's own tokens still
/// partitions the listing exactly once — whatever the page size, so the rule
/// holds for a walk of one page and for one of several.
#[tokio::test]
async fn a_rerendered_token_is_refused_at_every_step_of_a_walk() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    let whole = emulator
        .list_component_types(WORKSPACE, json!({"maxResults": MAX_PAGE_SIZE}))
        .await;
    assert_eq!(
        whole.status, 200,
        "the whole listing: expected 200, got {whole:?}"
    );
    let expected = component_type_ids("whole", &whole);
    assert!(
        expected.len() > 2,
        "the seeded workspace must hold enough component types to page over, got {expected:?}"
    );

    let whole_page = u64::try_from(expected.len()).expect("the listing is countable");
    for max_results in [1_u64, 2, 3, whole_page] {
        let case = format!("maxResults={max_results}");
        let mut walked: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        loop {
            let mut body = json!({"maxResults": max_results});
            if let Some(token) = &next_token {
                body["nextToken"] = json!(token);
            }
            let page = emulator.list_component_types(WORKSPACE, body).await;
            assert_eq!(
                page.status, 200,
                "case {case:?}: expected 200 from a page of the walk, got {page:?}"
            );
            walked.extend(component_type_ids(&case, &page));

            next_token = page.member("nextToken").as_str().map(str::to_string);
            let Some(token) = next_token.clone() else {
                break;
            };

            // Every re-rendering of the token this page handed out is refused,
            // and refusing it leaves the walk exactly where it was.
            for (what, respelt) in respellings(&token) {
                let refused = emulator
                    .list_component_types(
                        WORKSPACE,
                        json!({"maxResults": max_results, "nextToken": respelt}),
                    )
                    .await;
                assert_eq!(
                    (refused.status, refused.error_type.as_deref()),
                    (400, Some("ValidationException")),
                    "case {case:?}/{what}: expected a 400 ValidationException, got {refused:?}"
                );
                assert_eq!(
                    message(&refused),
                    REFUSAL,
                    "case {case:?}/{what}: expected the message {REFUSAL:?}, got {refused:?}"
                );
            }
        }

        assert_eq!(
            walked, expected,
            "case {case:?}: the walk must report every component type exactly once, got {walked:?}"
        );
    }
}
