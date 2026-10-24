// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the IoT SiteWise dataset slice (`CreateDataset` ->
//! `DescribeDataset` -> `UpdateDataset`), driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots with, so every assertion is about the
//! wire behaviour a real client sees.
//!
//! The rules these suites pin:
//!
//! * `UpdateDataset` REPLACES the caller-owned members (`datasetName`,
//!   `datasetDescription`, `datasetConfig`, `metadata`, `datasetSource`) of the
//!   stored dataset its `datasetId` names, leaves that dataset's identity and
//!   provenance (`datasetId`, `datasetArn`, `datasetType`, `workspaceName`,
//!   `datasetCreationDate`) alone, takes it to its next `datasetVersion` and
//!   reports it `UPDATING` until the transition the call owes falls due — so every
//!   later read answers with exactly what the last accepted write wrote;
//! * a `datasetId` that resolves to no stored dataset, or one the request scopes to
//!   a workspace or a version the store does not hold for it, is the modelled 404
//!   rather than a fabricated answer, and writes nothing;
//! * a created dataset is persisted and reads back exactly as it was created, with
//!   the ARN form `CreateDataset` documents;
//! * `workspaceName` is required for a session or curated dataset and omitted for
//!   an external one.
//!
//! The HTTP harness (`call`, `router`, `error_code`, `is_modelled_id`) is the one
//! the asset suite already has; this suite reuses it rather than growing a second.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::ACTIVATION_DELAY;

use super::asset_lifecycle::{call, error_code, is_modelled_id, router, REGION};

/// Where every dataset request goes, as `OPERATIONS.md` reports the route:
/// `POST /datasets`, `GET|PUT /datasets/{datasetId}`.
const DATASETS: &str = "/datasets";

/// A `datasetSource` naming a Kendra knowledge base.
///
/// `sourceType` / `sourceFormat` values come from the enums themselves
/// (smithy_query query="shape" shape="DatasetSourceType" — KENDRA | SITEWISE;
/// shape="DatasetSourceFormat" — KNOWLEDGE_BASE | TIMESERIES), and
/// `KendraSourceDetail` requires both ARNs.
fn kendra_source(knowledge_base: &str) -> Value {
    json!({
        "sourceType": "KENDRA",
        "sourceFormat": "KNOWLEDGE_BASE",
        "sourceDetail": {
            "kendra": {
                "knowledgeBaseArn":
                    format!("arn:aws:kendra:{REGION}:123456789012:index/{knowledge_base}"),
                "roleArn": format!("arn:aws:iam::123456789012:role/{knowledge_base}"),
            }
        },
    })
}

/// A `datasetSource` naming this service's own time-series data, with no
/// `sourceDetail` at all (the member is optional).
fn sitewise_source() -> Value {
    json!({ "sourceType": "SITEWISE", "sourceFormat": "TIMESERIES" })
}

/// A `datasetConfig` carrying a session window, which `SessionConfig` states as
/// two required nanosecond-precision timestamps.
fn session_config(start: i64, end: i64) -> Value {
    json!({
        "session": {
            "sessionStartTimestamp": { "timeInSeconds": start, "offsetInNanos": 0 },
            "sessionEndTimestamp": { "timeInSeconds": end, "offsetInNanos": 500 },
        }
    })
}

/// A `metadata` map built from `pairs`, for a row whose keys or values are
/// computed rather than literal. The modelled bounds are `MetadataKey` length
/// 1..128 and `MetadataValue` length 0..2048 (smithy_query query="shape"
/// shape="MetadataKey,MetadataValue").
fn metadata_of(pairs: &[(String, String)]) -> Value {
    Value::Object(
        pairs
            .iter()
            .map(|(key, value)| (key.clone(), Value::String(value.clone())))
            .collect(),
    )
}

/// The members a create or an update carries for a dataset: what the caller owns.
#[derive(Debug, Clone)]
struct DatasetBody {
    name: String,
    description: Option<String>,
    config: Option<Value>,
    metadata: Option<Value>,
    source: Value,
}

impl DatasetBody {
    /// The JSON body these members send, with every absent member OMITTED rather
    /// than sent as null.
    fn to_json(&self) -> Value {
        let mut body = json!({ "datasetName": self.name, "datasetSource": self.source });
        let object = body.as_object_mut().expect("an object body");
        if let Some(description) = self.description.as_deref() {
            object.insert("datasetDescription".to_string(), json!(description));
        }
        if let Some(config) = self.config.clone() {
            object.insert("datasetConfig".to_string(), config);
        }
        if let Some(metadata) = self.metadata.clone() {
            object.insert("metadata".to_string(), metadata);
        }
        body
    }

    /// Assert that `described` — a `DescribeDataset` body — reports exactly these
    /// members, for the named `case`.
    fn assert_described(&self, case: &str, described: &Value) {
        assert_eq!(
            described["datasetName"].as_str().unwrap_or(""),
            self.name,
            "case {case:?}: expected datasetName {:?}, got {described}",
            self.name
        );
        // The model makes `datasetDescription` required on the response while every
        // member that writes it is optional, so a dataset given no description
        // reports the empty string.
        assert_eq!(
            described["datasetDescription"].as_str().unwrap_or(""),
            self.description.clone().unwrap_or_default(),
            "case {case:?}: expected datasetDescription {:?}, got {described}",
            self.description
        );
        assert_eq!(
            described.get("datasetConfig"),
            self.config.as_ref(),
            "case {case:?}: expected datasetConfig {:?}, got {described}",
            self.config
        );
        // A dataset carrying no metadata reports none; an empty map sent on the wire
        // is the same thing.
        let expected_metadata = self
            .metadata
            .clone()
            .filter(|metadata| metadata.as_object().is_some_and(|map| !map.is_empty()));
        assert_eq!(
            described.get("metadata").cloned(),
            expected_metadata,
            "case {case:?}: expected metadata {:?}, got {described}",
            self.metadata
        );
        assert_eq!(
            described["datasetSource"], self.source,
            "case {case:?}: expected datasetSource {}, got {described}",
            self.source
        );
    }
}

/// One row of the create table: the dataset type and workspace the create names,
/// plus the caller-owned members it carries.
#[derive(Debug, Clone)]
struct CreateCase {
    label: &'static str,
    /// The `DatasetTypeEnum` value the create sends, or none at all. Source:
    /// smithy_query query="shape" shape="DatasetTypeEnum" — SESSION | CURATED |
    /// EXTERNAL.
    dataset_type: Option<&'static str>,
    workspace: Option<&'static str>,
    body: DatasetBody,
}

/// The create rows: every `DatasetTypeEnum` value plus a create that names no
/// type, crossed with each `DatasetSourceType`/`DatasetSourceFormat` pair, a
/// present / absent `datasetConfig`, a present / absent description and metadata
/// of several sizes.
fn create_cases() -> Vec<CreateCase> {
    vec![
        CreateCase {
            label: "external-kendra-no-workspace",
            dataset_type: Some("EXTERNAL"),
            workspace: None,
            body: DatasetBody {
                name: "External Kendra".to_string(),
                description: Some("a dataset that connects to Kendra".to_string()),
                config: None,
                metadata: Some(json!({ "owner": "search" })),
                source: kendra_source("support-articles"),
            },
        },
        CreateCase {
            label: "session-in-a-workspace",
            dataset_type: Some("SESSION"),
            workspace: Some("plant-one"),
            body: DatasetBody {
                name: "Session #1".to_string(),
                description: None,
                config: Some(session_config(1_760_000_000, 1_760_003_600)),
                metadata: None,
                source: sitewise_source(),
            },
        },
        CreateCase {
            label: "curated-in-a-workspace",
            dataset_type: Some("CURATED"),
            workspace: Some("plant_two-9"),
            body: DatasetBody {
                name: "Curated.set-1".to_string(),
                description: Some("d".repeat(2048)),
                config: None,
                metadata: Some(json!({ "a": "", "b": "x".repeat(2048), "c": "3" })),
                source: sitewise_source(),
            },
        },
        CreateCase {
            label: "no-type-no-workspace",
            dataset_type: None,
            workspace: None,
            body: DatasetBody {
                name: "n".repeat(256),
                description: Some("x".to_string()),
                config: None,
                metadata: Some(json!({})),
                source: sitewise_source(),
            },
        },
    ]
}

/// The body one create row sends.
fn create_body(case: &CreateCase) -> Value {
    let mut body = case.body.to_json();
    let object = body.as_object_mut().expect("an object body");
    if let Some(dataset_type) = case.dataset_type {
        object.insert("datasetType".to_string(), json!(dataset_type));
    }
    if let Some(workspace) = case.workspace {
        object.insert("workspaceName".to_string(), json!(workspace));
    }
    body
}

/// Create the dataset `case` describes and return its id, asserting the create
/// itself answered as `CreateDataset` documents.
async fn create_dataset(router: &MultiServiceRouter, case: &CreateCase) -> String {
    let (status, created) = call(router, "POST", DATASETS, Some(create_body(case))).await;
    assert_eq!(
        status, 202,
        "case {:?}: expected 202, got {status} {created}",
        case.label
    );
    let id = created["datasetId"].as_str().unwrap_or("").to_string();
    assert!(
        is_modelled_id(&id),
        "case {:?}: datasetId must be a modelled UUID, got {created}",
        case.label
    );
    assert_eq!(
        created["datasetArn"].as_str().unwrap_or(""),
        format!(
            "arn:aws:iotsitewise:{REGION}:{}:dataset/{id}",
            engine::EMULATOR_ACCOUNT_ID
        ),
        "case {:?}: unexpected datasetArn in {created}",
        case.label
    );
    // "State is CREATING after a successfull call to this API."
    assert_eq!(
        created["datasetStatus"]["state"].as_str().unwrap_or(""),
        "CREATING",
        "case {:?}: expected state CREATING, got {created}",
        case.label
    );
    id
}

/// `DescribeDataset`'s body for `id`, which must answer 200.
async fn describe(router: &MultiServiceRouter, id: &str) -> Value {
    let (status, body) = call(router, "GET", &format!("{DATASETS}/{id}"), None).await;
    assert_eq!(status, 200, "DescribeDataset {id}: got {status} {body}");
    body
}

/// A modelled `Timestamp` member as epoch seconds.
fn epoch_seconds(body: &Value, member: &str) -> f64 {
    body[member]
        .as_f64()
        .unwrap_or_else(|| panic!("{member} is not an epoch-seconds number in {body}"))
}

#[tokio::test]
async fn a_created_dataset_reads_back_through_describe() {
    let router = router();
    let mut seen_ids: Vec<String> = Vec::new();

    for case in create_cases() {
        let id = create_dataset(&router, &case).await;
        assert!(
            !seen_ids.contains(&id),
            "case {:?}: datasetId {id} was already minted for another dataset",
            case.label
        );
        seen_ids.push(id.clone());

        let described = describe(&router, &id).await;
        case.body.assert_described(case.label, &described);
        assert_eq!(
            described["datasetId"].as_str().unwrap_or(""),
            id,
            "case {:?}: DescribeDataset reported another id in {described}",
            case.label
        );
        assert_eq!(
            described.get("datasetType").and_then(Value::as_str),
            case.dataset_type,
            "case {:?}: unexpected datasetType in {described}",
            case.label
        );
        assert_eq!(
            described.get("workspaceName").and_then(Value::as_str),
            case.workspace,
            "case {:?}: unexpected workspaceName in {described}",
            case.label
        );
        // The create is the dataset's first accepted write, so it is at version 1.
        assert_eq!(
            described["datasetVersion"].as_str().unwrap_or(""),
            "1",
            "case {:?}: a created dataset is at version 1, got {described}",
            case.label
        );
        assert_eq!(
            epoch_seconds(&described, "datasetCreationDate"),
            epoch_seconds(&described, "datasetLastUpdateDate"),
            "case {:?}: an untouched dataset's lastUpdateDate is its creationDate, got {described}",
            case.label
        );
        // Nothing has enriched this dataset, so no enrichment status is reported.
        assert_eq!(
            described.get("enrichmentStatus"),
            None,
            "case {:?}: a new dataset has no enrichmentStatus, got {described}",
            case.label
        );

        // Reading does not change what the next caller sees.
        let again = describe(&router, &id).await;
        assert_eq!(
            again, described,
            "case {:?}: DescribeDataset answered differently on the second read: {again}",
            case.label
        );
    }

    assert_eq!(
        seen_ids.len(),
        create_cases().len(),
        "every row must have created a dataset, got {seen_ids:?}"
    );
}

/// One row of the update table: the members the update carries.
#[derive(Debug, Clone)]
struct UpdateCase {
    label: &'static str,
    body: DatasetBody,
}

/// The update rows span what the rule generalises over: the form and length of the
/// replacement name (the modelled `RestrictedName` bounds come from
/// smithy_query query="operation" operation="UpdateDataset": length 1..256,
/// pattern `^[a-zA-Z0-9 _\-#$*!@.]+$`), whether the request carries a description,
/// a config and metadata at all, how many metadata entries it carries, and which
/// `datasetSource` it points at.
fn update_cases() -> Vec<UpdateCase> {
    vec![
        UpdateCase {
            label: "every member replaced",
            body: DatasetBody {
                name: "Renamed dataset".to_string(),
                description: Some("the renamed dataset".to_string()),
                config: Some(session_config(1_770_000_000, 1_770_000_900)),
                metadata: Some(json!({ "owner": "ops", "tier": "gold" })),
                source: sitewise_source(),
            },
        },
        UpdateCase {
            label: "optional members omitted, so cleared",
            body: DatasetBody {
                name: "Bare dataset".to_string(),
                description: None,
                config: None,
                metadata: None,
                source: kendra_source("runbooks"),
            },
        },
        UpdateCase {
            label: "name at the modelled minimum length (1)",
            body: DatasetBody {
                name: "x".to_string(),
                description: Some("a".to_string()),
                config: None,
                metadata: Some(json!({ "k": "v" })),
                source: sitewise_source(),
            },
        },
        UpdateCase {
            label: "name at the modelled maximum length (256)",
            body: DatasetBody {
                name: "n".repeat(256),
                description: Some("d".repeat(2048)),
                config: None,
                metadata: Some(metadata_of(&[("k".repeat(128), "v".repeat(2048))])),
                source: sitewise_source(),
            },
        },
        UpdateCase {
            label: "name using every punctuation the pattern allows",
            body: DatasetBody {
                name: "Set #1 $x *y !z @w .v-u_t".to_string(),
                description: Some("punctuation".to_string()),
                config: Some(session_config(1_780_000_000, 1_780_000_001)),
                metadata: Some(json!({ "a": "1", "b": "2", "c": "3", "d": "4" })),
                source: kendra_source("manuals"),
            },
        },
        UpdateCase {
            label: "source switched to Kendra with no sourceDetail",
            body: DatasetBody {
                name: "No detail".to_string(),
                description: Some("a Kendra source with no detail".to_string()),
                config: None,
                metadata: None,
                source: json!({ "sourceType": "KENDRA", "sourceFormat": "KNOWLEDGE_BASE" }),
            },
        },
    ]
}

#[tokio::test]
async fn an_update_replaces_the_caller_owned_members_of_the_dataset_it_names() {
    let router = router();

    for case in update_cases() {
        // Each row starts from a dataset carrying every optional member, so an
        // omitted member in the update is observably cleared.
        let created = CreateCase {
            label: case.label,
            dataset_type: Some("SESSION"),
            workspace: Some("plant-one"),
            body: DatasetBody {
                name: "Before the update".to_string(),
                description: Some("the initial description".to_string()),
                config: Some(session_config(1_750_000_000, 1_750_000_060)),
                metadata: Some(json!({ "initial": "yes" })),
                source: sitewise_source(),
            },
        };
        let id = create_dataset(&router, &created).await;
        let before = describe(&router, &id).await;
        let label = case.label;

        let (status, updated) = call(
            &router,
            "PUT",
            &format!("{DATASETS}/{id}"),
            Some(case.body.to_json()),
        )
        .await;
        assert_eq!(
            status, 202,
            "case {label:?}: expected 202, got {status} {updated}"
        );
        assert_eq!(
            updated["datasetId"].as_str().unwrap_or(""),
            id,
            "case {label:?}: expected datasetId {id}, got {updated}"
        );
        assert_eq!(
            updated["datasetArn"], before["datasetArn"],
            "case {label:?}: the update must report the dataset's own ARN, got {updated}"
        );
        // "State is UPDATING after a successfull call to this API."
        assert_eq!(
            updated["datasetStatus"]["state"].as_str().unwrap_or(""),
            "UPDATING",
            "case {label:?}: expected state UPDATING, got {updated}"
        );

        // --- the write is what every later read answers with -------------------
        let after = describe(&router, &id).await;
        case.body.assert_described(label, &after);

        // --- what the caller does not own is untouched -------------------------
        for member in ["datasetId", "datasetArn", "datasetType", "workspaceName"] {
            assert_eq!(
                after.get(member),
                before.get(member),
                "case {label:?}: {member} must not change across an update, got {after}"
            );
        }
        assert_eq!(
            epoch_seconds(&after, "datasetCreationDate"),
            epoch_seconds(&before, "datasetCreationDate"),
            "case {label:?}: datasetCreationDate must not change across an update, got {after}"
        );
        assert!(
            epoch_seconds(&after, "datasetLastUpdateDate")
                >= epoch_seconds(&before, "datasetLastUpdateDate"),
            "case {label:?}: datasetLastUpdateDate must not move backwards, got {after}"
        );
        // The accepted write took the dataset to its next version.
        assert_eq!(
            after["datasetVersion"].as_str().unwrap_or(""),
            "2",
            "case {label:?}: an updated dataset is at version 2, got {after}"
        );

        // A second update is a second accepted write: the version counts them, and
        // the newest write is still what a read answers with.
        let (status, again) = call(
            &router,
            "PUT",
            &format!("{DATASETS}/{id}"),
            Some(
                DatasetBody {
                    name: "After the second update".to_string(),
                    ..case.body.clone()
                }
                .to_json(),
            ),
        )
        .await;
        assert_eq!(
            status, 202,
            "case {label:?}: the second update expected 202, got {status} {again}"
        );
        let settled = describe(&router, &id).await;
        assert_eq!(
            settled["datasetVersion"].as_str().unwrap_or(""),
            "3",
            "case {label:?}: a twice-updated dataset is at version 3, got {settled}"
        );
        assert_eq!(
            settled["datasetName"].as_str().unwrap_or(""),
            "After the second update",
            "case {label:?}: the newest write must be what a read answers, got {settled}"
        );
    }
}

#[tokio::test]
async fn updating_a_dataset_the_store_does_not_hold_is_refused() {
    let router = router();
    let case = CreateCase {
        label: "the-only-dataset",
        dataset_type: Some("CURATED"),
        workspace: Some("plant-one"),
        body: DatasetBody {
            name: "The only dataset".to_string(),
            description: Some("untouched".to_string()),
            config: None,
            metadata: Some(json!({ "keep": "me" })),
            source: sitewise_source(),
        },
    };
    let id = create_dataset(&router, &case).await;

    // Well-formed ids no create ever minted: one all-f, one that differs from the
    // stored id in a single nibble.
    let mut neighbour: Vec<char> = id.chars().collect();
    let at = neighbour
        .iter()
        .position(|ch| *ch != '-' && *ch != '0')
        .expect("a non-zero character in the id");
    neighbour[at] = '0';
    let neighbour: String = neighbour.into_iter().collect();

    // (case, the id the request names, the workspaceName its body carries) — an id
    // that names nothing, and the stored id scoped to a workspace that does not hold
    // it. `UpdateDataset` carries `workspaceName` in the BODY (it is neither an
    // `@httpLabel` nor an `@httpQuery` member; smithy_query query="operation"
    // operation="UpdateDataset"), unlike `DescribeDataset`, which takes it as a query
    // parameter.
    let rows: Vec<(&str, String, Option<&str>)> = vec![
        (
            "all-f id",
            "ffffffff-ffff-ffff-ffff-ffffffffffff".to_string(),
            None,
        ),
        ("neighbouring id", neighbour.clone(), None),
        ("another workspace", id.clone(), Some("plant-three")),
    ];

    for (label, target, workspace) in rows {
        let mut body = DatasetBody {
            name: "Should not be written".to_string(),
            description: None,
            config: None,
            metadata: None,
            source: sitewise_source(),
        }
        .to_json();
        if let Some(workspace) = workspace {
            body["workspaceName"] = json!(workspace);
        }
        let (status, body) = call(&router, "PUT", &format!("{DATASETS}/{target}"), Some(body)).await;
        assert_eq!(
            status, 404,
            "case {label:?}: expected 404, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {label:?}: unexpected error code in {body}"
        );

        // A refused update writes NOTHING, and the ids it named stay unknown.
        let described = describe(&router, &id).await;
        case.body.assert_described(label, &described);
        assert_eq!(
            described["datasetVersion"].as_str().unwrap_or(""),
            "1",
            "case {label:?}: a refused update must not take the dataset to a new version, \
             got {described}"
        );
    }

    for label in ["ffffffff-ffff-ffff-ffff-ffffffffffff", neighbour.as_str()] {
        assert!(
            is_modelled_id(label),
            "case {label:?}: the probe id must itself be modelled"
        );
        let (status, body) = call(&router, "GET", &format!("{DATASETS}/{label}"), None).await;
        assert_eq!(
            status, 404,
            "case {label:?}: DescribeDataset expected 404, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {label:?}: unexpected error code in {body}"
        );
    }

    // The dataset's OWN workspace resolves, so the scoping above refuses only a
    // workspace that does not hold the dataset.
    let mut body = DatasetBody {
        name: "Renamed in its own workspace".to_string(),
        description: Some("untouched".to_string()),
        config: None,
        metadata: Some(json!({ "keep": "me" })),
        source: sitewise_source(),
    }
    .to_json();
    body["workspaceName"] = json!("plant-one");
    let (status, accepted) = call(&router, "PUT", &format!("{DATASETS}/{id}"), Some(body)).await;
    assert_eq!(
        status, 202,
        "an update naming the dataset's own workspace: got {status} {accepted}"
    );
    let described = describe(&router, &id).await;
    assert_eq!(
        described["datasetName"].as_str().unwrap_or(""),
        "Renamed in its own workspace",
        "the accepted update must be what a later read answers, got {described}"
    );
}

#[tokio::test]
async fn a_read_is_scoped_by_the_workspace_and_the_version_it_names() {
    let router = router();
    let id = create_dataset(
        &router,
        &CreateCase {
            label: "versioned",
            dataset_type: Some("SESSION"),
            workspace: Some("plant-one"),
            body: DatasetBody {
                name: "Versioned".to_string(),
                description: Some("version 1".to_string()),
                config: None,
                metadata: None,
                source: sitewise_source(),
            },
        },
    )
    .await;

    // (case, query, accepted) — the dataset's own workspace and version resolve;
    // anything else names something this store does not hold.
    let rows: Vec<(&str, &str, bool)> = vec![
        ("its own workspace", "?workspaceName=plant-one", true),
        ("its own version", "?datasetVersion=1", true),
        (
            "both of its own",
            "?workspaceName=plant-one&datasetVersion=1",
            true,
        ),
        ("another workspace", "?workspaceName=plant-two", false),
        ("a later version", "?datasetVersion=2", false),
        ("version zero", "?datasetVersion=0", false),
    ];

    for (label, query, accepted) in rows {
        let (status, body) = call(&router, "GET", &format!("{DATASETS}/{id}{query}"), None).await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {label:?}: expected 200, got {status} {body}"
            );
            assert_eq!(
                body["datasetId"].as_str().unwrap_or(""),
                id,
                "case {label:?}: expected datasetId {id}, got {body}"
            );
        } else {
            assert_eq!(
                status, 404,
                "case {label:?}: expected 404, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {label:?}: unexpected error code in {body}"
            );
        }
    }

    // After one update the dataset is at version 2, so that is the version a read
    // resolves and version 1 is no longer held.
    let (status, updated) = call(
        &router,
        "PUT",
        &format!("{DATASETS}/{id}"),
        Some(
            DatasetBody {
                name: "Versioned".to_string(),
                description: Some("version 2".to_string()),
                config: None,
                metadata: None,
                source: sitewise_source(),
            }
            .to_json(),
        ),
    )
    .await;
    assert_eq!(status, 202, "the update: got {status} {updated}");

    for (label, query, accepted) in [("version 2", "?datasetVersion=2", true), ("version 1", "?datasetVersion=1", false)] {
        let (status, body) = call(&router, "GET", &format!("{DATASETS}/{id}{query}"), None).await;
        let expected = if accepted { 200 } else { 404 };
        assert_eq!(
            status, expected,
            "case {label:?}: expected {expected}, got {status} {body}"
        );
    }
}

#[tokio::test]
async fn the_workspace_rule_bounds_what_a_create_accepts() {
    let router = router();

    // (case, datasetType, workspaceName, accepted). The rule is the model's own, on
    // `workspaceName`: "Required for session and curated datasets. Omit this field
    // for datasets that connect to an external datasource." A create that names no
    // type says nothing about which side of the rule it falls on, so neither form is
    // refused for it.
    let rows: Vec<(&str, Option<&str>, Option<&str>, bool)> = vec![
        ("SESSION with a workspace", Some("SESSION"), Some("ws-1"), true),
        ("SESSION without one", Some("SESSION"), None, false),
        ("CURATED with a workspace", Some("CURATED"), Some("ws-1"), true),
        ("CURATED without one", Some("CURATED"), None, false),
        ("EXTERNAL without a workspace", Some("EXTERNAL"), None, true),
        ("EXTERNAL with one", Some("EXTERNAL"), Some("ws-1"), false),
        ("no type, with a workspace", None, Some("ws-1"), true),
        ("no type, without one", None, None, true),
    ];

    for (label, dataset_type, workspace, accepted) in rows {
        let case = CreateCase {
            label: "workspace-rule",
            dataset_type,
            workspace,
            body: DatasetBody {
                name: format!("Dataset for {label}").replace(',', ""),
                description: None,
                config: None,
                metadata: None,
                source: sitewise_source(),
            },
        };
        let (status, body) = call(&router, "POST", DATASETS, Some(create_body(&case))).await;
        if accepted {
            assert_eq!(
                status, 202,
                "case {label:?}: expected 202, got {status} {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {label:?}: expected 400, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidRequestException",
                "case {label:?}: unexpected error code in {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_caller_supplied_dataset_id_is_taken_by_at_most_one_dataset() {
    let router = router();
    let chosen = "3f2b1c0d-4e5f-4a6b-8c9d-0e1f2a3b4c5d";
    let body = |name: &str| {
        json!({
            "datasetId": chosen,
            "datasetName": name,
            "datasetSource": sitewise_source(),
        })
    };

    let (status, created) = call(&router, "POST", DATASETS, Some(body("First"))).await;
    assert_eq!(status, 202, "the first create: got {status} {created}");
    assert_eq!(
        created["datasetId"].as_str().unwrap_or(""),
        chosen,
        "a caller-supplied datasetId must be the dataset's id, got {created}"
    );

    let (status, conflict) = call(&router, "POST", DATASETS, Some(body("Second"))).await;
    assert_eq!(
        status, 409,
        "a duplicate datasetId expected 409, got {status} {conflict}"
    );
    assert_eq!(
        error_code(&conflict),
        "ResourceAlreadyExistsException",
        "unexpected error code in {conflict}"
    );
    assert_eq!(
        conflict["resourceId"].as_str().unwrap_or(""),
        chosen,
        "the conflict must name the dataset holding the id, got {conflict}"
    );

    // The dataset already holding the id is untouched by the refused create.
    let described = describe(&router, chosen).await;
    assert_eq!(
        described["datasetName"].as_str().unwrap_or(""),
        "First",
        "a refused create must not rename the dataset holding the id, got {described}"
    );
}

#[tokio::test]
async fn the_owed_transitions_settle_to_active_and_touch_no_other_dataset() {
    let router = router();
    let case = CreateCase {
        label: "settling",
        dataset_type: Some("SESSION"),
        workspace: Some("plant-one"),
        body: DatasetBody {
            name: "Settling".to_string(),
            description: Some("before".to_string()),
            config: None,
            metadata: None,
            source: sitewise_source(),
        },
    };
    let id = create_dataset(&router, &case).await;
    let sibling = CreateCase {
        label: "sibling",
        ..case.clone()
    };
    let sibling_id = create_dataset(&router, &sibling).await;

    // The create's own owed transition settles first, so the dataset is ACTIVE
    // before the update and the UPDATING below cannot be the leftover CREATING.
    let settle = ACTIVATION_DELAY + Duration::from_millis(200);
    tokio::time::sleep(settle).await;
    let described = describe(&router, &id).await;
    assert_eq!(
        described["datasetStatus"]["state"].as_str().unwrap_or(""),
        "ACTIVE",
        "a created dataset settles to ACTIVE, got {described}"
    );

    let (status, updated) = call(
        &router,
        "PUT",
        &format!("{DATASETS}/{id}"),
        Some(
            DatasetBody {
                name: "Settled".to_string(),
                description: Some("after".to_string()),
                config: None,
                metadata: None,
                source: sitewise_source(),
            }
            .to_json(),
        ),
    )
    .await;
    assert_eq!(status, 202, "UpdateDataset: got {status} {updated}");

    // The transition the update owes is not instant: a read taken straight after it
    // still observes UPDATING.
    let during = describe(&router, &id).await;
    assert_eq!(
        during["datasetStatus"]["state"].as_str().unwrap_or(""),
        "UPDATING",
        "a dataset reports UPDATING until the owed transition falls due, got {during}"
    );

    tokio::time::sleep(settle).await;
    let settled = describe(&router, &id).await;
    assert_eq!(
        settled["datasetStatus"]["state"].as_str().unwrap_or(""),
        "ACTIVE",
        "the owed UPDATING -> ACTIVE transition must fall due, got {settled}"
    );
    assert_eq!(
        settled["datasetName"].as_str().unwrap_or(""),
        "Settled",
        "settling must not undo the update, got {settled}"
    );

    // An update names exactly one dataset: every other one reads back as it was.
    let untouched = describe(&router, &sibling_id).await;
    sibling.body.assert_described("sibling", &untouched);
    assert_eq!(
        untouched["datasetVersion"].as_str().unwrap_or(""),
        "1",
        "updating one dataset must not version another, got {untouched}"
    );
}
