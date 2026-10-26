// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the two `CreateDataset` rules the dataset lifecycle suite
//! beside this one leaves unpinned, driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots with, so every assertion is about the
//! wire behaviour a real client sees.
//!
//! The rules these suites pin:
//!
//! * the store is bounded: with the dataset map at its ceiling a create is the
//!   modelled quota refusal (`LimitExceededException`, HTTP 410), it stores
//!   nothing, and the very same create is accepted again as soon as there is
//!   room — whatever form of dataset it asks for;
//! * the `tags` a create carries are persisted on the dataset it mints and read
//!   back through `ListTagsForResource` under the ARN the create answered, for
//!   every map size `TagMap` admits, while a dataset created with no `tags` reads
//!   back an empty set rather than a fabricated one; a map larger than `TagMap`
//!   admits is refused as client input and mints nothing.
//!
//! The HTTP harness (`call`, `router_with_engine`, `error_code`,
//! `is_modelled_id`) is the one the asset suite already has and the tag reader is
//! the tagging suite's; this suite reuses them rather than growing a second copy.

use std::collections::{BTreeMap, HashMap};
use std::time::SystemTime;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::ops::types::DatasetSource;
use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{
    dataset_arn, service_state, DatasetRecord, DATASET_INITIAL_VERSION, MAX_DATASETS,
    MAX_TAGS_PER_RESOURCE, STATE_CREATING,
};

use super::asset_lifecycle::{call, error_code, is_modelled_id, router_with_engine, REGION};
use super::resource_tags::list_tags;

/// Where every dataset request goes, as `OPERATIONS.md` reports the routes:
/// `POST /datasets`, `GET|PUT /datasets/{datasetId}`.
const DATASETS: &str = "/datasets";

/// One form of dataset a create can ask for: the `datasetType` it names, if any,
/// and the workspace that type then requires or forbids.
///
/// The types are the whole `DatasetTypeEnum` value list (smithy_query
/// query="shape" shape="DatasetTypeEnum" — SESSION | CURATED | EXTERNAL), plus a
/// create that names no type at all, which the member's optionality admits.
#[derive(Debug, Clone, Copy)]
struct Form {
    label: &'static str,
    dataset_type: Option<&'static str>,
    workspace: Option<&'static str>,
}

/// Every form of create these suites run their rule over. `workspaceName` is
/// present exactly where the model says it must be: "Required for session and
/// curated datasets. Omit this field for datasets that connect to an external
/// datasource."
fn forms() -> Vec<Form> {
    vec![
        Form {
            label: "a session dataset of a workspace",
            dataset_type: Some("SESSION"),
            workspace: Some("bounds-workspace"),
        },
        Form {
            label: "a curated dataset of a workspace",
            dataset_type: Some("CURATED"),
            workspace: Some("bounds-workspace"),
        },
        Form {
            label: "an external dataset, in no workspace",
            dataset_type: Some("EXTERNAL"),
            workspace: None,
        },
        Form {
            label: "a dataset naming no type",
            dataset_type: None,
            workspace: None,
        },
    ]
}

/// The body a create sends for `form`, naming `name` and carrying `tags` when the
/// row has any. Every absent member is OMITTED rather than sent as null.
///
/// `datasetSource` is required on the input, and `sourceType` / `sourceFormat` are
/// required on `DatasetSource` itself; the pair used here is this service's own
/// time-series data (smithy_query query="shape"
/// shape="DatasetSourceType,DatasetSourceFormat" — KENDRA | SITEWISE and
/// KNOWLEDGE_BASE | TIMESERIES).
fn create_body(form: &Form, name: &str, tags: Option<Value>) -> Value {
    let mut body = json!({
        "datasetName": name,
        "datasetSource": { "sourceType": "SITEWISE", "sourceFormat": "TIMESERIES" },
    });
    let object = body.as_object_mut().expect("an object body");
    if let Some(dataset_type) = form.dataset_type {
        object.insert("datasetType".to_string(), json!(dataset_type));
    }
    if let Some(workspace) = form.workspace {
        object.insert("workspaceName".to_string(), json!(workspace));
    }
    if let Some(tags) = tags {
        object.insert("tags".to_string(), tags);
    }
    body
}

/// A seeded dataset record, as the ceiling suite fills the map with: everything a
/// create would have written for the id `at` names, already settled.
fn seeded_record(at: usize) -> DatasetRecord {
    let id = format!("{at:08x}-1c2d-4e5f-8a9b-0c1d2e3f4a5b");
    let now = SystemTime::now();
    DatasetRecord {
        arn: dataset_arn(REGION, &id),
        id,
        name: format!("seeded-{at}"),
        description: None,
        dataset_type: None,
        config: None,
        metadata: HashMap::new(),
        source: DatasetSource {
            source_type: "SITEWISE".to_string(),
            source_format: "TIMESERIES".to_string(),
            source_detail: None,
        },
        workspace_name: None,
        creation_date: now,
        last_update_date: now,
        active_at: now,
        settling_state: STATE_CREATING.to_string(),
        version: DATASET_INITIAL_VERSION,
        tags: HashMap::new(),
    }
}

/// How many datasets the shared store holds.
fn held(engine: &engine::InMemoryEngine) -> usize {
    service_state(engine)
        .expect("the IoT SiteWise store")
        .lock()
        .datasets
        .len()
}

/// `DescribeDataset` for `id`, which must answer 200 with the ARN form
/// `CreateDataset` documents: "arn:${Partition}:iotsitewise:${Region}:${Account}:
/// dataset/${DatasetId}", spelled out here rather than rebuilt with the helper the
/// handler mints it with.
async fn describe(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    let (status, body) = call(router, "GET", &format!("{DATASETS}/{id}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeDataset {id} expected 200, got {status} {body}"
    );
    assert_eq!(
        body["datasetArn"].as_str().unwrap_or(""),
        format!(
            "arn:aws:iotsitewise:{REGION}:{}:dataset/{id}",
            engine::EMULATOR_ACCOUNT_ID
        ),
        "case {case:?}: unexpected datasetArn in {body}"
    );
    body
}

#[tokio::test]
async fn a_create_past_the_dataset_ceiling_is_the_modelled_quota_refusal() {
    for form in forms() {
        let case = form.label;
        let (router, engine) = router_with_engine();

        // The map is filled to its ceiling through the shared store rather than
        // with `MAX_DATASETS` HTTP creates: the bound is what is under test, not
        // the thousand requests it would take to reach it over the wire.
        {
            let state = service_state(&*engine).expect("the IoT SiteWise store");
            let mut store = state.lock();
            for at in 0..MAX_DATASETS {
                let record = seeded_record(at);
                store.datasets.insert(record.id.clone(), record);
            }
        }
        assert_eq!(
            held(&engine),
            MAX_DATASETS,
            "case {case:?}: the seeded ids must be distinct, the store holds {}",
            held(&engine)
        );

        let (status, refused) = call(
            &router,
            "POST",
            DATASETS,
            Some(create_body(&form, "At the ceiling", None)),
        )
        .await;
        assert_eq!(
            (status, error_code(&refused)),
            (410, "LimitExceededException"),
            "case {case:?}: a create at the ceiling must be the quota refusal, \
             got {status} {refused}"
        );
        assert!(
            refused["message"]
                .as_str()
                .unwrap_or("")
                .contains(&MAX_DATASETS.to_string()),
            "case {case:?}: the refusal must state the ceiling it was about, got {refused}"
        );
        assert_eq!(
            held(&engine),
            MAX_DATASETS,
            "case {case:?}: a refused create must store nothing, the store holds {}",
            held(&engine)
        );

        // With room again the same create is accepted, so the refusal is the
        // bound and not a dead end, and what it mints is really persisted.
        let freed = seeded_record(0).id;
        service_state(&*engine)
            .expect("the IoT SiteWise store")
            .lock()
            .datasets
            .remove(&freed);
        let (status, created) = call(
            &router,
            "POST",
            DATASETS,
            Some(create_body(&form, "With room again", None)),
        )
        .await;
        assert_eq!(
            status, 202,
            "case {case:?}: with room again the same create must be accepted, \
             got {status} {created}"
        );
        let id = created["datasetId"].as_str().unwrap_or("").to_string();
        assert!(
            is_modelled_id(&id),
            "case {case:?}: datasetId must be a modelled UUID, got {created}"
        );
        let described = describe(&router, case, &id).await;
        assert_eq!(
            described["datasetName"].as_str().unwrap_or(""),
            "With room again",
            "case {case:?}: the accepted create must be the dataset that reads back, \
             got {described}"
        );
        assert_eq!(
            held(&engine),
            MAX_DATASETS,
            "case {case:?}: the accepted create must take the freed slot, the store holds {}",
            held(&engine)
        );
    }
}

/// `count` tags as a request sends them, with one empty value among them because
/// `TagValue` is modelled `@length(min: 0)` while `TagKey` is `@length(min: 1)`.
fn tags_json(count: usize) -> Value {
    let mut map = Map::new();
    for at in 0..count {
        let value = if at == 0 {
            String::new()
        } else {
            format!("value-{at}")
        };
        map.insert(format!("tag-key-{at:03}"), Value::String(value));
    }
    Value::Object(map)
}

/// The same tags as the map `ListTagsForResource` must then report.
fn expected_tags(count: usize) -> BTreeMap<String, String> {
    tags_json(count)
        .as_object()
        .expect("an object")
        .iter()
        .map(|(key, value)| {
            (
                key.clone(),
                value.as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// The tag set `ListTagsForResource` reports for `arn`, asserting the 200 a
/// stored resource answers with.
async fn stored_tags(
    router: &MultiServiceRouter,
    case: &str,
    arn: &str,
) -> BTreeMap<String, String> {
    let (status, body) = list_tags(router, arn).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListTagsForResource expected 200, got {status} {body}"
    );
    // A stored resource reports a tags map whether or not it carries any tag, so an
    // absent member is itself a failure rather than an empty set.
    let reported = body.get("tags").and_then(Value::as_object);
    assert!(
        reported.is_some(),
        "case {case:?}: a stored resource must report a tags map, empty or not, got {body}"
    );
    reported
        .expect("asserted just above")
        .iter()
        .map(|(key, value)| {
            (
                key.clone(),
                value.as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

#[tokio::test]
async fn the_tags_a_create_carries_are_persisted_on_the_dataset_it_mints() {
    let (router, engine) = router_with_engine();

    // The map sizes `TagMap` admits — absent, its `@length` floor, a middling
    // count and its ceiling (smithy_query query="shape" shape="TagMap" —
    // @length(min: 1, max: 50)) — crossed with every form of dataset.
    let sizes: Vec<Option<usize>> = vec![None, Some(1), Some(7), Some(MAX_TAGS_PER_RESOURCE)];

    for form in forms() {
        for size in &sizes {
            let count = size.unwrap_or(0);
            let case = format!("{} with {size:?} tags", form.label);
            let before = held(&engine);
            let (status, created) = call(
                &router,
                "POST",
                DATASETS,
                // `datasetName` targets `RestrictedName`, whose pattern
                // `^[a-zA-Z0-9 _\-#$*!@.]+$` admits none of the punctuation a
                // debug-formatted row label carries.
                Some(create_body(
                    &form,
                    &format!("Tagged with {count} tags"),
                    size.map(tags_json),
                )),
            )
            .await;
            assert_eq!(
                status, 202,
                "case {case:?}: expected 202, got {status} {created}"
            );
            let id = created["datasetId"].as_str().unwrap_or("").to_string();
            assert!(
                is_modelled_id(&id),
                "case {case:?}: datasetId must be a modelled UUID, got {created}"
            );
            assert_eq!(
                held(&engine),
                before + 1,
                "case {case:?}: the create must store exactly one dataset, the store holds {}",
                held(&engine)
            );

            // The ARN the create answered is the one the tag set is reached
            // through, and the dataset reads back under it.
            let arn = created["datasetArn"].as_str().unwrap_or("").to_string();
            describe(&router, &case, &id).await;
            let listed = stored_tags(&router, &case, &arn).await;
            assert_eq!(
                listed,
                expected_tags(count),
                "case {case:?}: ListTagsForResource reported another tag set for {arn}"
            );
        }
    }

    // One entry past `TagMap`'s ceiling: refused as client input, and nothing is
    // minted. Only the STATUS is asserted, as the tagging suite's own
    // past-the-ceiling row does: a 51-entry map is refused by the boundary
    // validator before this handler runs, and the error code that walk renders is
    // the subject of its own issue.
    let form = forms()[0];
    let before = held(&engine);
    let (status, refused) = call(
        &router,
        "POST",
        DATASETS,
        Some(create_body(
            &form,
            "Past the tag ceiling",
            Some(tags_json(MAX_TAGS_PER_RESOURCE + 1)),
        )),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {:?}: {} tags in one create must be refused, got {status} {refused}",
        "one map past the tag ceiling",
        MAX_TAGS_PER_RESOURCE + 1
    );
    assert_eq!(
        held(&engine),
        before,
        "case {:?}: a refused create must store nothing, the store holds {}",
        "one map past the tag ceiling",
        held(&engine)
    );
}
