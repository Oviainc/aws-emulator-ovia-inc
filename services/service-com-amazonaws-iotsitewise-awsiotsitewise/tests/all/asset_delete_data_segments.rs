// SPDX-License-Identifier: CC0-1.0

//! `DeleteAsset` — and the property removal `UpdateAssetModel` propagates —
//! against streams a dataset still holds a data segment of, driven over HTTP
//! through the same `MultiServiceRouter` the emulator boots with.
//!
//! The rule these suites pin: a stored data stream may not be removed while any
//! dataset still holds a data segment covering it, so the call that would remove
//! it is REFUSED — as the terminal request error, naming the stream and a dataset
//! that holds a segment of it — and removes nothing until every such segment is
//! gone. An asset property's stream is addressable as the time series its
//! `(assetId, propertyId)` pair names, so deleting the asset, and dropping the
//! property from the model the asset was created from, both remove such streams
//! and both ask the same question.
//!
//! The table spans what the rule generalises over: the dataset holding the segment
//! (a session dataset's own data, a curated dataset's association of it, and both
//! at once), one and three segments, which property of the asset the segment
//! covers (one carrying samples and one carrying none, so the dependency is on the
//! stream rather than on the data behind it), how the request identifies the asset
//! (its id and its `externalId:` label), and the control rows a segment must NOT
//! stop: an asset nothing holds a segment of, a segment covering another asset's
//! stream, and one covering a stream no asset property owns.
//!
//! The expected refusal message is the hand-written oracle the sibling
//! time-series suite states the rule with: no suite here calls a handler's own
//! wording.
//!
//! The HTTP harness (`call`, `error_code`, `router_with_engine`, the asset
//! helpers), the dataset helpers (`create_dataset`, `ingest`, `instant`,
//! `time_series_id`, `wire_instant`, `at`) and the data-segment helpers
//! (`ingest_pair`, `segments_of`, `expected_message`) are the ones the asset,
//! data-segment and time-series-delete suites already define; all are reused
//! rather than copied.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{
    service_state, DataSegmentRecord,
};

use crate::asset_lifecycle::{
    call, create_asset, create_model, error_code, property_id, router_with_engine,
};
use crate::dataset_data_segments::{
    at, create_dataset, ingest, instant, time_series_id, wire_instant,
};
use crate::delete_time_series_data_segments::{expected_message, ingest_pair, segments_of};
use crate::describe_asset_property::{
    add_composite_model, create_typed_model, describe_model_composite,
};
use crate::hierarchies_etags_and_refusals::error_message;

/// The workspace this suite's datasets live in.
const WORKSPACE: &str = "kiln-yard";

/// The alias of a stream NO asset property owns: a control row holds a segment of
/// it, so a delete refused for an unrelated stream cannot pass.
const LOOSE_ALIAS: &str = "/plant/yard/1/temperature";

/// The external id the deleted asset carries, so a row can address it by its
/// `externalId:` label.
const ASSET_EXTERNAL_ID: &str = "kiln-yard-1";

/// The property whose stream carries samples.
const INGESTED_PROPERTY: &str = "Force";

/// The property whose stream carries NONE: it exists, so the pair names a stream a
/// segment can cover, but nothing was ever written to it.
const EMPTY_PROPERTY: &str = "Pressure";

/// The sample seconds, relative to the suite's anchor, every ingested stream holds.
const SAMPLES: &[i64] = &[0, 5, 10, 15];

/// How a row's request identifies the asset it deletes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Address {
    /// The asset's own id, in UUID form.
    Id,
    /// `externalId:<externalId>`, the other form every `CustomID` member admits.
    ExternalLabel,
}

/// Which dataset holds a seeded segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Dataset {
    /// The SESSION dataset, holding its OWN data (source == holder).
    Session,
    /// The CURATED dataset, holding an association selected FROM the session one
    /// (source != holder).
    Curated,
}

/// Which stream a seeded segment covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Covers {
    /// The deleted asset's property that carries samples.
    Ingested,
    /// The deleted asset's property that carries none.
    Empty,
    /// Another asset's property stream, which no row removes.
    OtherAsset,
    /// The stream no asset property owns, which no row removes.
    Loose,
}

impl Covers {
    /// Whether a segment covering this stream is a dependency of the asset the
    /// row deletes.
    fn belongs_to_the_deleted_asset(self) -> bool {
        matches!(self, Covers::Ingested | Covers::Empty)
    }
}

/// One data segment a row seeds: who holds it, whose data it covers, and its
/// half-open span in table seconds.
#[derive(Debug, Clone, Copy)]
struct Seed {
    holder: Dataset,
    covers: Covers,
    start: i64,
    end: i64,
}

/// One row of the table.
#[derive(Debug)]
struct Row {
    case: &'static str,
    address: Address,
    /// The segments held when the delete is called.
    seeds: &'static [Seed],
    /// Whether the delete is refused for them.
    refused: bool,
}

fn table() -> Vec<Row> {
    vec![
        Row {
            case: "the ingested property, the session dataset's own segment",
            address: Address::Id,
            seeds: &[Seed {
                holder: Dataset::Session,
                covers: Covers::Ingested,
                start: 0,
                end: 10,
            }],
            refused: true,
        },
        Row {
            case: "the ingested property, a curated dataset's association",
            address: Address::Id,
            seeds: &[Seed {
                holder: Dataset::Curated,
                covers: Covers::Ingested,
                start: 10,
                end: 20,
            }],
            refused: true,
        },
        Row {
            case: "the property carrying no samples, a curated dataset's association",
            address: Address::Id,
            seeds: &[Seed {
                holder: Dataset::Curated,
                covers: Covers::Empty,
                start: 0,
                end: 10,
            }],
            refused: true,
        },
        Row {
            case: "the ingested property, three of the session dataset's segments",
            address: Address::Id,
            seeds: &[
                Seed {
                    holder: Dataset::Session,
                    covers: Covers::Ingested,
                    start: 0,
                    end: 5,
                },
                Seed {
                    holder: Dataset::Session,
                    covers: Covers::Ingested,
                    start: 5,
                    end: 10,
                },
                Seed {
                    holder: Dataset::Session,
                    covers: Covers::Ingested,
                    start: 100,
                    end: 160,
                },
            ],
            refused: true,
        },
        Row {
            case: "both properties, a segment in each dataset",
            address: Address::Id,
            seeds: &[
                Seed {
                    holder: Dataset::Session,
                    covers: Covers::Ingested,
                    start: 0,
                    end: 10,
                },
                Seed {
                    holder: Dataset::Curated,
                    covers: Covers::Empty,
                    start: 10,
                    end: 20,
                },
            ],
            refused: true,
        },
        Row {
            case: "the externalId: label, the session dataset's own segment",
            address: Address::ExternalLabel,
            seeds: &[Seed {
                holder: Dataset::Session,
                covers: Covers::Ingested,
                start: 0,
                end: 10,
            }],
            refused: true,
        },
        Row {
            case: "no segment anywhere",
            address: Address::Id,
            seeds: &[],
            refused: false,
        },
        Row {
            case: "the externalId: label, no segment anywhere",
            address: Address::ExternalLabel,
            seeds: &[],
            refused: false,
        },
        Row {
            case: "a session segment of another asset's stream",
            address: Address::Id,
            seeds: &[Seed {
                holder: Dataset::Session,
                covers: Covers::OtherAsset,
                start: 0,
                end: 10,
            }],
            refused: false,
        },
        Row {
            case: "a curated association of a stream no property owns",
            address: Address::ExternalLabel,
            seeds: &[Seed {
                holder: Dataset::Curated,
                covers: Covers::Loose,
                start: 0,
                end: 10,
            }],
            refused: false,
        },
    ]
}

/// The datasets, assets and data streams every row starts from.
#[derive(Debug)]
struct Fixture {
    engine: Arc<engine::InMemoryEngine>,
    /// The SESSION dataset: the source a curated association selects from, and the
    /// holder of its own segments.
    session: String,
    /// The CURATED dataset: the holder of associations.
    curated: String,
    /// The asset model both assets were created from.
    model_id: String,
    /// The asset every row deletes.
    asset_id: String,
    /// Its property carrying samples.
    ingested_property: String,
    /// A second asset of the same model, which no row deletes.
    other_asset: String,
    other_property: String,
    /// The `TimeSeriesId` of each stream a row can hold a segment of.
    ingested_series: String,
    empty_series: String,
    other_series: String,
    loose_series: String,
}

impl Fixture {
    /// The `datasetId` `holder` names.
    fn dataset(&self, holder: Dataset) -> &str {
        match holder {
            Dataset::Session => &self.session,
            Dataset::Curated => &self.curated,
        }
    }

    /// The `TimeSeriesId` of the stream `covers` names.
    fn series(&self, covers: Covers) -> &str {
        match covers {
            Covers::Ingested => &self.ingested_series,
            Covers::Empty => &self.empty_series,
            Covers::OtherAsset => &self.other_series,
            Covers::Loose => &self.loose_series,
        }
    }

    /// The label a request of `address` identifies the deleted asset by.
    fn label(&self, address: Address) -> String {
        match address {
            Address::Id => self.asset_id.clone(),
            Address::ExternalLabel => format!("externalId:{ASSET_EXTERNAL_ID}"),
        }
    }
}

/// A router over a fresh engine carrying: a session and a curated dataset in
/// [`WORKSPACE`], a stream no asset property owns, an asset with two properties
/// (one ingested, one empty) that carries an external id, and a second asset of
/// the same model with an ingested property of its own.
async fn fixture() -> (MultiServiceRouter, Fixture) {
    let (router, engine) = router_with_engine();
    let session = create_dataset(&router, "Yard session", Some("SESSION"), Some(WORKSPACE)).await;
    let curated = create_dataset(&router, "Yard curated", Some("CURATED"), Some(WORKSPACE)).await;

    ingest(&router, LOOSE_ALIAS, SAMPLES).await;
    let loose_series = time_series_id(&router, LOOSE_ALIAS).await;

    let model_id = create_model(
        &router,
        "Kiln",
        json!([
            {"name": INGESTED_PROPERTY, "dataType": "DOUBLE", "type": {"measurement": {}}},
            {"name": EMPTY_PROPERTY, "dataType": "DOUBLE", "type": {"measurement": {}}},
        ]),
    )
    .await;

    let asset_id = create_asset_with_external_id(&router, "Kiln1", &model_id).await;
    let asset = describe_asset(&router, &asset_id).await;
    let ingested_property = property_id(&asset["assetProperties"], INGESTED_PROPERTY);
    let empty_property = property_id(&asset["assetProperties"], EMPTY_PROPERTY);
    ingest_pair(&router, &asset_id, &ingested_property, SAMPLES).await;

    let other_asset = create_asset(&router, "Kiln2", &model_id).await;
    let other = describe_asset(&router, &other_asset).await;
    let other_property = property_id(&other["assetProperties"], INGESTED_PROPERTY);
    ingest_pair(&router, &other_asset, &other_property, SAMPLES).await;

    let ingested_series = pair_series(&router, &asset_id, &ingested_property).await;
    let empty_series = pair_series(&router, &asset_id, &empty_property).await;
    let other_series = pair_series(&router, &other_asset, &other_property).await;

    (
        router,
        Fixture {
            engine,
            session,
            curated,
            model_id,
            asset_id,
            ingested_property,
            other_asset,
            other_property,
            ingested_series,
            empty_series,
            other_series,
            loose_series,
        },
    )
}

/// `POST /assets` for an asset carrying [`ASSET_EXTERNAL_ID`], answering its id:
/// the asset-suite's `create_asset` states no external id, and a row addresses
/// this asset by its `externalId:` label.
async fn create_asset_with_external_id(
    router: &MultiServiceRouter,
    name: &str,
    asset_model_id: &str,
) -> String {
    let (status, body) = call(
        router,
        "POST",
        "/assets",
        Some(json!({
            "assetName": name,
            "assetModelId": asset_model_id,
            "assetExternalId": ASSET_EXTERNAL_ID,
        })),
    )
    .await;
    assert_eq!(status, 202, "create asset {name}: got {status} {body}");
    body["assetId"]
        .as_str()
        .unwrap_or_else(|| panic!("create asset {name}: no assetId in {body}"))
        .to_string()
}

/// `GET /assets/{label}`, asserting it answered.
async fn describe_asset(router: &MultiServiceRouter, label: &str) -> Value {
    let (status, body) = call(router, "GET", &format!("/assets/{label}"), None).await;
    assert_eq!(status, 200, "DescribeAsset {label}: got {status} {body}");
    body
}

/// The `timeSeriesId` `DescribeTimeSeries` reports for the `(assetId, propertyId)`
/// pair — the identity a data segment names an asset property's stream by.
async fn pair_series(
    router: &MultiServiceRouter,
    asset_id: &str,
    property_id: &str,
) -> String {
    let query = format!("assetId={asset_id}&propertyId={property_id}");
    let (status, body) = call(
        router,
        "GET",
        &format!("/timeseries/describe?{query}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "describe {query}: got {status} {body}");
    body["timeSeriesId"]
        .as_str()
        .unwrap_or_else(|| panic!("describe {query}: no timeSeriesId in {body}"))
        .to_string()
}

/// `DELETE /assets/{label}`.
async fn delete_asset(router: &MultiServiceRouter, label: &str) -> (u16, Value) {
    call(router, "DELETE", &format!("/assets/{label}"), None).await
}

/// `POST /datasets/{curated}/data-segments/associate` with one entry, asserting the
/// association was applied.
async fn associate(
    router: &MultiServiceRouter,
    fixture: &Fixture,
    series: &str,
    start: i64,
    end: i64,
) {
    let (status, body) = call(
        router,
        "POST",
        &format!("/datasets/{}/data-segments/associate", fixture.curated),
        Some(json!({
            "workspaceName": WORKSPACE,
            "associateDataSegmentEntries": [{
                "sourceDatasetId": fixture.session,
                "timeSeriesId": series,
                "startTimestamp": wire_instant(start, 0),
                "endTimestamp": wire_instant(end, 0),
            }],
        })),
    )
    .await;
    assert_eq!(status, 200, "associate {series}: got {status} {body}");
    assert_eq!(
        body["failedAssociations"].as_array().map(Vec::len),
        Some(0),
        "associate {series}: the entry must be applied, got {body}"
    );
}

/// Seed one segment of `series` as the session dataset's OWN data, through the
/// state's producer entry point — a session dataset's own segments are its
/// ingested data, which a bulk import writes and no request of this suite does.
fn seed_own_segment(fixture: &Fixture, series: &str, start: i64, end: i64) {
    let state = service_state(&*fixture.engine).expect("the IoT SiteWise store");
    let mut store = state.lock();
    let stored = store.put_dataset_data_segment(
        &fixture.session,
        DataSegmentRecord {
            source_dataset_id: fixture.session.clone(),
            time_series_id: series.to_string(),
            start_timestamp: instant(start, 0),
            end_timestamp: instant(end, 0),
        },
    );
    assert!(
        stored,
        "seeding the segment [{start}, {end}) of {series} must be accepted"
    );
}

/// Put every segment `seeds` describes in the dataset it names.
async fn seed(router: &MultiServiceRouter, fixture: &Fixture, seeds: &[Seed]) {
    for seed in seeds {
        let covered = fixture.series(seed.covers).to_string();
        match seed.holder {
            Dataset::Session => seed_own_segment(fixture, &covered, seed.start, seed.end),
            Dataset::Curated => associate(router, fixture, &covered, seed.start, seed.end).await,
        }
    }
}

/// The sample seconds `GET /properties/history` reports for an asset property,
/// relative to the suite's anchor.
async fn pair_history(
    router: &MultiServiceRouter,
    asset_id: &str,
    property_id: &str,
) -> Vec<i64> {
    let (status, body) = call(
        router,
        "GET",
        &format!("/properties/history?assetId={asset_id}&propertyId={property_id}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "history of {property_id}: got {status} {body}");
    body["assetPropertyValueHistory"]
        .as_array()
        .unwrap_or_else(|| panic!("a history carries assetPropertyValueHistory, got {body}"))
        .iter()
        .map(|sample| sample["timestamp"]["timeInSeconds"].as_i64().unwrap_or(-1))
        .collect()
}

/// The sample seconds [`SAMPLES`] stands for on the wire.
fn absolute_samples() -> Vec<i64> {
    SAMPLES.iter().map(|second| at(*second)).collect()
}

#[tokio::test]
async fn deleting_an_asset_is_refused_while_a_dataset_holds_a_data_segment_of_its_streams() {
    for row in table() {
        let case = row.case;
        let (router, fixture) = fixture().await;
        seed(&router, &fixture, row.seeds).await;

        let held_before: Vec<usize> = [
            &fixture.ingested_series,
            &fixture.empty_series,
            &fixture.other_series,
            &fixture.loose_series,
        ]
        .iter()
        .map(|series| segments_of(&fixture.engine, series))
        .collect();

        let label = fixture.label(row.address);
        let (status, body) = delete_asset(&router, &label).await;

        if !row.refused {
            assert_eq!(
                status, 202,
                "case {case:?}: nothing holds a segment of this asset's streams, so the delete \
                 stands; got {status} {body}"
            );
            let (status, gone) = call(&router, "GET", &format!("/assets/{label}"), None).await;
            assert_eq!(
                (status, error_code(&gone)),
                (404, "ResourceNotFoundException"),
                "case {case:?}: the deleted asset must resolve to nothing, got {status} {gone}"
            );
            // A segment covering ANOTHER stream is untouched by this delete.
            for seed in row.seeds {
                assert_eq!(
                    segments_of(&fixture.engine, fixture.series(seed.covers)),
                    1,
                    "case {case:?}: a delete must leave another stream's segments alone"
                );
            }
            continue;
        }

        assert_eq!(
            status, 400,
            "case {case:?}: expected the delete to be refused, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case {case:?}: a still-depended-on resource is a terminal request error, got {body}"
        );
        // Any dataset still holding a segment of any stream of the asset is a
        // reason to refuse, so the message names one such pair.
        let candidates: Vec<String> = row
            .seeds
            .iter()
            .filter(|seed| seed.covers.belongs_to_the_deleted_asset())
            .map(|seed| {
                expected_message(fixture.series(seed.covers), fixture.dataset(seed.holder))
            })
            .collect();
        assert!(
            candidates
                .iter()
                .any(|expected| expected == error_message(&body)),
            "case {case:?}: expected one of {candidates:?}, got {:?}",
            error_message(&body)
        );

        // A refused delete removed nothing: the asset still describes itself with
        // both properties, its samples are all still there, and every segment is
        // still held.
        let described = describe_asset(&router, &label).await;
        assert_eq!(
            described["assetId"].as_str(),
            Some(fixture.asset_id.as_str()),
            "case {case:?}: the asset must still be the one addressed, got {described}"
        );
        assert_eq!(
            pair_history(&router, &fixture.asset_id, &fixture.ingested_property).await,
            absolute_samples(),
            "case {case:?}: a refused delete must leave every sample of the asset"
        );
        let held_after: Vec<usize> = [
            &fixture.ingested_series,
            &fixture.empty_series,
            &fixture.other_series,
            &fixture.loose_series,
        ]
        .iter()
        .map(|series| segments_of(&fixture.engine, series))
        .collect();
        assert_eq!(
            held_after, held_before,
            "case {case:?}: a refused delete must leave every data segment held"
        );
    }
}

#[tokio::test]
async fn the_asset_delete_goes_through_once_the_last_data_segment_is_gone() {
    let (router, fixture) = fixture().await;

    // One segment per property stream of the asset: the session dataset's own data
    // for the ingested one, a curated association for the other.
    seed_own_segment(&fixture, &fixture.ingested_series, 0, 10);
    associate(&router, &fixture, &fixture.empty_series, 10, 20).await;

    let (status, body) = delete_asset(&router, &fixture.asset_id).await;
    assert_eq!(status, 400, "two segments held: got {status} {body}");
    let candidates = [
        expected_message(&fixture.ingested_series, &fixture.session),
        expected_message(&fixture.empty_series, &fixture.curated),
    ];
    assert!(
        candidates
            .iter()
            .any(|expected| expected == error_message(&body)),
        "two segments held: expected one of {candidates:?}, got {:?}",
        error_message(&body)
    );

    // Dropping the association is not enough: the session dataset's own segment
    // still covers a stream of the asset, and the refusal names that pair.
    let (status, body) = call(
        &router,
        "POST",
        &format!("/datasets/{}/data-segments/disassociate", fixture.curated),
        Some(json!({
            "workspaceName": WORKSPACE,
            "disassociateDataSegmentEntries": [{
                "sourceDatasetId": fixture.session,
                "timeSeriesId": fixture.empty_series,
                "startTimestamp": wire_instant(10, 0),
                "endTimestamp": wire_instant(20, 0),
            }],
        })),
    )
    .await;
    assert_eq!(status, 200, "disassociate: got {status} {body}");
    assert_eq!(
        body["failedDisassociations"].as_array().map(Vec::len),
        Some(0),
        "disassociate: the entry must be applied, got {body}"
    );

    let (status, body) = delete_asset(&router, &fixture.asset_id).await;
    assert_eq!(status, 400, "one segment left: got {status} {body}");
    assert_eq!(
        error_message(&body),
        expected_message(&fixture.ingested_series, &fixture.session),
        "one segment left: the refusal must name the dataset holding it, got {body}"
    );

    // With the last segment deleted nothing depends on the asset's streams, and the
    // delete stands. The segment goes through the state's own remover rather than
    // `BatchDeleteDatasetDataSegments`: that entry's `timeSeriesId` is modelled as
    // `ID` (`smithy_query(query="shape", shape="DeleteDataSegmentEntry")` — length
    // 36..36, the UUID pattern), so no request of it can name a stream a
    // `(assetId, propertyId)` pair derives, which is 73 characters long.
    {
        let state = service_state(&*fixture.engine).expect("the IoT SiteWise store");
        let mut store = state.lock();
        let removed = store.delete_dataset_data_segment(
            &fixture.session,
            &fixture.ingested_series,
            &instant(0, 0),
            &instant(10, 0),
        );
        assert!(removed, "the last segment must be removable");
    }
    assert_eq!(
        segments_of(&fixture.engine, &fixture.ingested_series),
        0,
        "no segment of the asset's streams is left to hold the delete up"
    );

    let (status, body) = delete_asset(&router, &fixture.asset_id).await;
    assert_eq!(status, 202, "the last segment gone: got {status} {body}");
    let (status, gone) = call(
        &router,
        "GET",
        &format!("/assets/{}", fixture.asset_id),
        None,
    )
    .await;
    assert_eq!(
        (status, error_code(&gone)),
        (404, "ResourceNotFoundException"),
        "the deleted asset must resolve to nothing, got {status} {gone}"
    );

    // And no dataset is left listing a segment that names a stream the store no
    // longer holds.
    for dataset in [&fixture.session, &fixture.curated] {
        let (status, listed) = call(
            &router,
            "GET",
            &format!("/datasets/{dataset}/data-segments?workspaceName={WORKSPACE}"),
            None,
        )
        .await;
        assert_eq!(status, 200, "list {dataset}: got {status} {listed}");
        let dangling = listed["dataSegments"]
            .as_array()
            .unwrap_or_else(|| panic!("a listing carries dataSegments, got {listed}"))
            .iter()
            .filter(|summary| {
                let named = summary["timeSeriesId"].as_str().unwrap_or_default();
                named == fixture.ingested_series || named == fixture.empty_series
            })
            .count();
        assert_eq!(
            dangling, 0,
            "list {dataset}: no segment may name a stream of the deleted asset, got {listed}"
        );
    }
}

#[tokio::test]
async fn dropping_a_model_property_is_refused_while_a_dataset_holds_a_segment_of_its_stream() {
    // The same rule one step away: removing a property from the asset model
    // "deletes all previous data for that property", so it removes the stream of
    // every asset of that model.
    for (case, covers) in [
        ("the stream carrying samples", Covers::Ingested),
        ("the stream carrying none", Covers::Empty),
    ] {
        let (router, fixture) = fixture().await;
        let dropped_name = match covers {
            Covers::Ingested => INGESTED_PROPERTY,
            _ => EMPTY_PROPERTY,
        };
        associate(&router, &fixture, fixture.series(covers), 0, 10).await;

        // The model's new definition: every stored property but the one whose
        // stream a dataset still holds a segment of.
        let (status, model) = call(
            &router,
            "GET",
            &format!("/asset-models/{}", fixture.model_id),
            None,
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: DescribeAssetModel got {status} {model}");
        let kept: Vec<Value> = model["assetModelProperties"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no assetModelProperties in {model}"))
            .iter()
            .filter(|property| property["name"].as_str() != Some(dropped_name))
            .cloned()
            .collect();
        let update = json!({ "assetModelName": "Kiln", "assetModelProperties": kept });

        let (status, body) = call(
            &router,
            "PUT",
            &format!("/asset-models/{}", fixture.model_id),
            Some(update.clone()),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: dropping the property must be refused, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case {case:?}: a still-depended-on stream is a terminal request error, got {body}"
        );
        assert_eq!(
            error_message(&body),
            expected_message(fixture.series(covers), &fixture.curated),
            "case {case:?}: the refusal must name the stream and its holder, got {body}"
        );

        // The refused update wrote nothing: both assets still carry the property,
        // and its samples are still there.
        for (asset, property) in [
            (&fixture.asset_id, &fixture.ingested_property),
            (&fixture.other_asset, &fixture.other_property),
        ] {
            let described = describe_asset(&router, asset).await;
            let names: Vec<&str> = described["assetProperties"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: no assetProperties in {described}"))
                .iter()
                .filter_map(|property| property["name"].as_str())
                .collect();
            assert!(
                names.contains(&dropped_name),
                "case {case:?}: the asset {asset} must still carry {dropped_name}, got {names:?}"
            );
            assert_eq!(
                pair_history(&router, asset, property).await,
                absolute_samples(),
                "case {case:?}: a refused update must leave every sample of {asset}"
            );
        }

        // With the segment disassociated the stream has nothing depending on it,
        // and the same update stands: the property goes from every asset of the
        // model, and so does the stream the pair named.
        let (status, body) = call(
            &router,
            "POST",
            &format!("/datasets/{}/data-segments/disassociate", fixture.curated),
            Some(json!({
                "workspaceName": WORKSPACE,
                "disassociateDataSegmentEntries": [{
                    "sourceDatasetId": fixture.session,
                    "timeSeriesId": fixture.series(covers),
                    "startTimestamp": wire_instant(0, 0),
                    "endTimestamp": wire_instant(10, 0),
                }],
            })),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: disassociate got {status} {body}");

        let (status, body) = call(
            &router,
            "PUT",
            &format!("/asset-models/{}", fixture.model_id),
            Some(update),
        )
        .await;
        assert_eq!(
            status, 202,
            "case {case:?}: the segment gone, the update stands; got {status} {body}"
        );
        let described = describe_asset(&router, &fixture.asset_id).await;
        let names: Vec<&str> = described["assetProperties"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no assetProperties in {described}"))
            .iter()
            .filter_map(|property| property["name"].as_str())
            .collect();
        assert!(
            !names.contains(&dropped_name),
            "case {case:?}: {dropped_name} must be gone from the asset, got {names:?}"
        );
    }
}

/// The status `DescribeAssetProperty` answers for one property of an asset: 200
/// while the asset carries it, 404 once it does not.
async fn describe_property_status(
    router: &MultiServiceRouter,
    asset_id: &str,
    property_id: &str,
) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/assets/{asset_id}/properties/{property_id}"),
        None,
    )
    .await
}

#[tokio::test]
async fn dropping_a_component_model_property_a_reusing_asset_s_segment_covers_is_refused() {
    // The removal reaches one step further than the model it is requested on: a
    // composite model that REUSES a component model contributes that model's
    // definitions to the assets of the model holding it, so a property dropped
    // from the component model takes those assets' streams too — and a stream a
    // dataset still holds a segment of cannot go, whichever model's update would
    // remove it.
    let (router, fixture) = fixture().await;
    let component = create_typed_model(
        &router,
        "SpeedComponent",
        "COMPONENT_MODEL",
        json!([
            {"name": "Speed", "dataType": "DOUBLE", "unit": "m/s", "type": {"measurement": {}}},
            {"name": "Revolutions", "dataType": "INTEGER", "type": {"measurement": {}}},
        ]),
    )
    .await;
    let host_model = create_model(
        &router,
        "HostKiln",
        json!([
            {"name": INGESTED_PROPERTY, "dataType": "DOUBLE", "type": {"measurement": {}}},
        ]),
    )
    .await;
    let composite_id = add_composite_model(
        &router,
        &host_model,
        json!({
            "assetModelCompositeModelName": "Reused",
            "assetModelCompositeModelType": "CUSTOM",
            "composedAssetModelId": &component,
        }),
    )
    .await;
    let host_asset = create_asset(&router, "HostKiln1", &host_model).await;
    // A composite model's property is reported under the composite model that
    // contributes it, so its id is read there rather than off the asset.
    let contributed = property_id(
        &describe_model_composite(&router, &host_model, &composite_id).await
            ["assetModelCompositeModelProperties"],
        "Speed",
    );
    let kept_contributed = property_id(
        &describe_model_composite(&router, &host_model, &composite_id).await
            ["assetModelCompositeModelProperties"],
        "Revolutions",
    );
    ingest_pair(&router, &host_asset, &contributed, SAMPLES).await;
    let series = pair_series(&router, &host_asset, &contributed).await;
    associate(&router, &fixture, &series, 0, 10).await;

    // The component model's new definition keeps the other property — carried over
    // with the id the store minted for it, so it stays the same definition — and
    // drops the one the host asset's covered stream belongs to.
    let (status, stored) = call(
        &router,
        "GET",
        &format!("/asset-models/{component}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "DescribeAssetModel: got {status} {stored}");
    let kept: Vec<Value> = stored["assetModelProperties"]
        .as_array()
        .unwrap_or_else(|| panic!("no assetModelProperties in {stored}"))
        .iter()
        .filter(|property| property["name"].as_str() != Some("Speed"))
        .cloned()
        .collect();
    let update = json!({
        "assetModelName": "SpeedComponent",
        "assetModelProperties": kept,
    });
    let (status, body) = call(
        &router,
        "PUT",
        &format!("/asset-models/{component}"),
        Some(update.clone()),
    )
    .await;
    assert_eq!(
        status, 400,
        "dropping the reused property must be refused, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidRequestException",
        "a still-depended-on stream is a terminal request error, got {body}"
    );
    assert_eq!(
        error_message(&body),
        expected_message(&series, &fixture.curated),
        "the refusal must name the host asset's stream and its holder, got {body}"
    );
    let (status, described) = describe_property_status(&router, &host_asset, &contributed).await;
    assert_eq!(
        status, 200,
        "a refused update must leave the contributed property on the host asset, got {status} \
         {described}"
    );
    assert_eq!(
        pair_history(&router, &host_asset, &contributed).await,
        absolute_samples(),
        "a refused update must leave every sample of the host asset's stream"
    );

    // With the segment disassociated nothing depends on that stream, and the same
    // update stands: the contributed property goes from the host model's asset too,
    // which is what the propagation owes an asset carrying a reused definition.
    let (status, body) = call(
        &router,
        "POST",
        &format!("/datasets/{}/data-segments/disassociate", fixture.curated),
        Some(json!({
            "workspaceName": WORKSPACE,
            "disassociateDataSegmentEntries": [{
                "sourceDatasetId": fixture.session,
                "timeSeriesId": series,
                "startTimestamp": wire_instant(0, 0),
                "endTimestamp": wire_instant(10, 0),
            }],
        })),
    )
    .await;
    assert_eq!(status, 200, "disassociate: got {status} {body}");

    let (status, body) = call(
        &router,
        "PUT",
        &format!("/asset-models/{component}"),
        Some(update),
    )
    .await;
    assert_eq!(
        status, 202,
        "the segment gone, the update stands; got {status} {body}"
    );
    let (status, gone) = describe_property_status(&router, &host_asset, &contributed).await;
    assert_eq!(
        (status, error_code(&gone)),
        (404, "ResourceNotFoundException"),
        "the dropped definition must be gone from the host asset, got {status} {gone}"
    );
    let (status, kept) = describe_property_status(&router, &host_asset, &kept_contributed).await;
    assert_eq!(
        status, 200,
        "the definition the update kept must still reach the host asset, got {status} {kept}"
    );
}
