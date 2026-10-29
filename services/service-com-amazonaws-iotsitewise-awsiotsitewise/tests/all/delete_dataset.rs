// SPDX-License-Identifier: CC0-1.0

//! `DeleteDataset` end to end, driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots with, so every assertion is about the
//! wire behaviour a real client sees.
//!
//! The rules these suites pin:
//!
//! * an accepted delete removes the dataset its `datasetId` names — scoped by the
//!   `workspaceName` it carries — together with the data segments that dataset
//!   held, and reports it `DELETING`, so every later read of it answers the
//!   modelled absence and nothing of it is left in the store;
//! * the data segments a deleted dataset held go with it, and the underlying time
//!   series samples go with a segment the dataset OWNED ("Deleting a session
//!   dataset also deletes the underlying time series data in the session") while a
//!   segment selected from ANOTHER dataset takes only the reference with it
//!   ("Deleting a curated dataset doesn't delete the underlying data in the source
//!   session datasets");
//! * a dataset whose data another dataset's segments reference cannot be deleted
//!   until that dataset is deleted or those segments are disassociated, and the
//!   refusal writes nothing;
//! * a `datasetId` that resolves to no dataset the request can reach is the
//!   modelled 404 and removes nothing;
//! * a retry carrying the `clientToken` of an accepted delete replays its answer
//!   and removes nothing further, while that token on a DIFFERENT request is the
//!   modelled conflict.
//!
//! The HTTP harness (`call`, `error_code`, `router_with_engine`) is the one the
//! asset suite defines and the dataset helpers (`create_dataset`, `ingest`,
//! `history_seconds`, `instant`, `time_series_id`, `wire_instant`) are the ones the
//! data-segment suites define; both are reused rather than copied.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{
    not_found_message, service_state, DataSegmentRecord, RESOURCE_DATASET, RESOURCE_WORKSPACE,
};

use crate::asset_lifecycle::{call, error_code, router_with_engine};
use crate::dataset_data_segments::{
    create_dataset, history_seconds, ingest, instant, time_series_id, wire_instant,
};

/// The workspace the datasets of these suites live in.
const WORKSPACE: &str = "kiln-floor";

/// A second workspace, so a row can prove a delete is scoped by the workspace it
/// names.
const OTHER_WORKSPACE: &str = "kiln-annex";

/// The alias of the data stream the deleted dataset's own segments cover.
const OWN_ALIAS: &str = "/plant/kiln/1/temperature";

/// The alias of a second stream, covered by the segments of a dataset no row
/// deletes.
const OTHER_ALIAS: &str = "/plant/kiln/2/temperature";

/// The sample seconds, relative to `crate::support::anchor`, each stream is
/// ingested with.
const SAMPLES: &[i64] = &[0, 5, 10, 15, 100, 105];

/// A `datasetId` no create ever minted: a well-formed `ID` outside the store.
const ABSENT_DATASET: &str = "ffffffff-ffff-ffff-ffff-ffffffffffff";

/// A `clientToken` inside the modelled bounds (`ClientToken` is length 36..64,
/// pattern `^\S{36,64}$`; smithy_query query="operation" operation="DeleteDataset").
const CLIENT_TOKEN: &str = "6f1b2d84-9c35-4e07-bb42-1a7d0c93e5f8";

/// A second token, so one suite can send two different ones.
const OTHER_CLIENT_TOKEN: &str = "0a9c8b7d-6e5f-4a3b-9c2d-1e0f9a8b7c6d";

/// One half-open span, in the table seconds and nanosecond offsets these rows are
/// written in.
type Span = ((i64, i32), (i64, i32));

/// The spans a dataset's own data segments cover: one whole-second span and one
/// whose end carries a nanosecond offset, so the data a delete takes is read from
/// the segment's own bounds rather than from whole seconds.
const OWN_SPANS: &[Span] = &[((0, 0), (10, 0)), ((100, 0), (105, 250_000_000))];

/// Seed `spans` as data segments of `dataset_id` covering `series`, each naming
/// `dataset_id` itself as the source its data came from — the dataset's OWN data,
/// which is what a session dataset's segments are.
///
/// Through the state's producer entry point, the way the data-segment suites seed
/// one: no implemented operation puts a session dataset's own data there.
fn seed_own_segments(
    engine: &Arc<engine::InMemoryEngine>,
    dataset_id: &str,
    series: &str,
    spans: &[Span],
) {
    let state = service_state(&**engine).expect("the IoT SiteWise store");
    let mut store = state.lock();
    for span in spans {
        let (start, end) = *span;
        let stored = store.put_dataset_data_segment(
            dataset_id,
            DataSegmentRecord {
                source_dataset_id: dataset_id.to_string(),
                time_series_id: series.to_string(),
                start_timestamp: instant(start.0, start.1),
                end_timestamp: instant(end.0, end.1),
            },
        );
        assert!(stored, "seeding {span:?} must be accepted");
    }
}

/// Whether the store still holds a data-segment log for `dataset_id`: the API
/// exposes no reader for a dataset it no longer holds, so this is the only way to
/// see that nothing was left behind.
fn holds_segments(engine: &Arc<engine::InMemoryEngine>, dataset_id: &str) -> bool {
    let state = service_state(&**engine).expect("the IoT SiteWise store");
    let store = state.lock();
    store.dataset_data_segments.contains_key(dataset_id)
}

/// The `timeSeriesId` of every segment `ListDatasetDataSegments` reports for
/// `dataset_id`, in the order it reports them.
async fn listed_series(
    router: &MultiServiceRouter,
    dataset_id: &str,
    workspace: &str,
) -> Vec<String> {
    let (status, body) = call(
        router,
        "GET",
        &format!("/datasets/{dataset_id}/data-segments?workspaceName={workspace}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "list {dataset_id}: got {status} {body}");
    body["dataSegments"]
        .as_array()
        .unwrap_or_else(|| panic!("no dataSegments in {body}"))
        .iter()
        .map(|segment| segment["timeSeriesId"].as_str().unwrap_or("").to_string())
        .collect()
}

/// `GET /datasets/{datasetId}`, as `(status, body)`.
async fn describe(router: &MultiServiceRouter, dataset_id: &str) -> (u16, Value) {
    call(router, "GET", &format!("/datasets/{dataset_id}"), None).await
}

/// `DELETE /datasets/{datasetId}` with the query members `workspace` and `token`
/// add, as `(status, body)`.
async fn delete_dataset(
    router: &MultiServiceRouter,
    dataset_id: &str,
    workspace: Option<&str>,
    token: Option<&str>,
) -> (u16, Value) {
    let mut query: Vec<String> = Vec::new();
    if let Some(workspace) = workspace {
        query.push(format!("workspaceName={workspace}"));
    }
    if let Some(token) = token {
        query.push(format!("clientToken={token}"));
    }
    let uri = if query.is_empty() {
        format!("/datasets/{dataset_id}")
    } else {
        format!("/datasets/{dataset_id}?{}", query.join("&"))
    };
    call(router, "DELETE", &uri, None).await
}

/// Assert that the accepted delete `body` reports the dataset `DELETING`, for the
/// named `case`.
fn assert_accepted(case: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 202,
        "case {case:?}: expected 202, got {status} {body}"
    );
    // "State is DELETING after a successfull call to this API."
    assert_eq!(
        body["datasetStatus"]["state"].as_str(),
        Some("DELETING"),
        "case {case:?}: expected state DELETING, got {body}"
    );
}

/// Assert that `body` is the modelled absence of `dataset_id`, for `case`.
fn assert_absent(case: &str, status: u16, body: &Value, dataset_id: &str) {
    assert_absent_resource(
        case,
        status,
        body,
        &not_found_message(RESOURCE_DATASET, dataset_id),
    );
}

/// Assert that `body` is the modelled absence stated by `expected`, for `case` —
/// the dataset the request named, or the workspace it scoped itself to when this
/// service holds no such workspace.
fn assert_absent_resource(case: &str, status: u16, body: &Value, expected: &str) {
    assert_eq!(
        status, 404,
        "case {case:?}: expected 404, got {status} {body}"
    );
    assert_eq!(
        error_code(body),
        "ResourceNotFoundException",
        "case {case:?}: unexpected error code in {body}"
    );
    assert_eq!(
        body["message"].as_str(),
        Some(expected),
        "case {case:?}: the refusal must name the resource that is absent, got {body}"
    );
}

/// The samples of a stream that SURVIVE the deletion of a dataset owning segments
/// at `spans`: every sample whose second falls in no span, each span being
/// half-open on its own nanosecond bounds.
fn surviving_samples(spans: &[Span]) -> Vec<i64> {
    SAMPLES
        .iter()
        .copied()
        .filter(|second| !spans.iter().any(|span| covers(span, *second)))
        .collect()
}

/// Whether the half-open span `span` covers the whole second `second` — the
/// precision every seeded sample carries.
fn covers(span: &Span, second: i64) -> bool {
    let ((start_second, start_nanos), (end_second, end_nanos)) = *span;
    let after_start = second > start_second || (second == start_second && start_nanos == 0);
    let before_end = second < end_second || (second == end_second && end_nanos > 0);
    after_start && before_end
}

/// One row of the delete table.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    /// The `DatasetTypeEnum` value the create names, or none at all. Source:
    /// smithy_query query="shape" shape="DatasetTypeEnum" — SESSION | CURATED |
    /// EXTERNAL.
    dataset_type: Option<&'static str>,
    /// The workspace the dataset is created in, if any.
    workspace: Option<&'static str>,
    /// How many of [`OWN_SPANS`] the dataset holds segments of its own data at.
    own_spans: usize,
    /// Whether the delete SCOPES itself to the dataset's own workspace.
    scope: bool,
    /// Whether the delete carries a `clientToken`.
    token: bool,
}

/// The rows span what the rule generalises over: every `DatasetTypeEnum` value
/// plus a dataset the store holds no type for, a dataset inside a workspace and
/// one outside every workspace, a dataset holding no data segment of its own and
/// ones holding a segment and two, and a delete that scopes itself to the
/// workspace or carries a token against one that does neither.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a session dataset owning two segments, scoped and with a token",
            dataset_type: Some("SESSION"),
            workspace: Some(WORKSPACE),
            own_spans: 2,
            scope: true,
            token: true,
        },
        Row {
            case: "a session dataset owning nothing, unscoped and tokenless",
            dataset_type: Some("SESSION"),
            workspace: Some(WORKSPACE),
            own_spans: 0,
            scope: false,
            token: false,
        },
        Row {
            case: "a curated dataset, scoped and tokenless",
            dataset_type: Some("CURATED"),
            workspace: Some(WORKSPACE),
            own_spans: 0,
            scope: true,
            token: false,
        },
        Row {
            case: "an external dataset outside every workspace, with a token",
            dataset_type: Some("EXTERNAL"),
            workspace: None,
            own_spans: 0,
            scope: false,
            token: true,
        },
        Row {
            case: "a dataset the store holds no type for, owning one segment",
            dataset_type: None,
            workspace: Some(WORKSPACE),
            own_spans: 1,
            scope: false,
            token: false,
        },
    ]
}

#[tokio::test]
async fn a_delete_removes_the_dataset_it_names_and_the_data_that_dataset_owned() {
    for row in rows() {
        let Row {
            case,
            dataset_type,
            workspace,
            own_spans,
            scope,
            token,
        } = row;
        let (router, engine) = router_with_engine();

        // A dataset of the same store the delete must not reach, owning segments of
        // its own over a second stream.
        let bystander =
            create_dataset(&router, "Kiln bystander", Some("SESSION"), Some(WORKSPACE)).await;
        ingest(&router, OWN_ALIAS, SAMPLES).await;
        ingest(&router, OTHER_ALIAS, SAMPLES).await;
        let own_series = time_series_id(&router, OWN_ALIAS).await;
        let other_series = time_series_id(&router, OTHER_ALIAS).await;
        seed_own_segments(&engine, &bystander, &other_series, OWN_SPANS);

        let dataset = create_dataset(&router, "Kiln target", dataset_type, workspace).await;
        let spans = &OWN_SPANS[..own_spans];
        seed_own_segments(&engine, &dataset, &own_series, spans);

        let (status, deleted) = delete_dataset(
            &router,
            &dataset,
            scope.then_some(workspace).flatten(),
            token.then_some(CLIENT_TOKEN),
        )
        .await;
        assert_accepted(case, status, &deleted);

        // The dataset is GONE: every later read of it answers the modelled absence.
        let (status, missing) = describe(&router, &dataset).await;
        assert_absent(case, status, &missing, &dataset);
        if let Some(workspace) = workspace {
            let (status, listed) = call(
                &router,
                "GET",
                &format!("/datasets/{dataset}/data-segments?workspaceName={workspace}"),
                None,
            )
            .await;
            assert_absent(case, status, &listed, &dataset);
        }
        assert!(
            !holds_segments(&engine, &dataset),
            "case {case:?}: the deleted dataset must leave no data segment behind, and it \
             was created holding {own_spans}"
        );

        // The data the dataset OWNED went with its segments; every sample outside
        // those spans stayed.
        assert_eq!(
            history_seconds(&router, OWN_ALIAS).await,
            surviving_samples(spans),
            "case {case:?}: expected the samples outside the deleted dataset's segments, \
             got {:?}",
            history_seconds(&router, OWN_ALIAS).await
        );

        // A delete names exactly one dataset: every other one, and its data, stands.
        let (status, described) = describe(&router, &bystander).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the bystander dataset must stand, got {status} {described}"
        );
        let bystander_segments = listed_series(&router, &bystander, WORKSPACE).await;
        assert_eq!(
            bystander_segments,
            vec![other_series.clone(); OWN_SPANS.len()],
            "case {case:?}: expected the bystander to keep its {} segments, got \
             {bystander_segments:?}",
            OWN_SPANS.len()
        );
        assert_eq!(
            history_seconds(&router, OTHER_ALIAS).await,
            SAMPLES.to_vec(),
            "case {case:?}: expected every sample of the other stream, got {:?}",
            history_seconds(&router, OTHER_ALIAS).await
        );
    }
}

/// How a row releases the reference that blocks the delete.
#[derive(Debug, Clone, Copy)]
enum Release {
    /// "First delete the curated dataset …"
    DeleteTheReferencingDataset,
    /// "… or disassociate the data segments."
    DisassociateTheSegments,
}

#[tokio::test]
async fn a_dataset_another_dataset_references_is_refused_until_that_reference_goes() {
    for release in [
        Release::DeleteTheReferencingDataset,
        Release::DisassociateTheSegments,
    ] {
        let case = format!("{release:?}");
        let case = case.as_str();
        let (router, engine) = router_with_engine();

        let session =
            create_dataset(&router, "Kiln session", Some("SESSION"), Some(WORKSPACE)).await;
        let curated =
            create_dataset(&router, "Kiln curated", Some("CURATED"), Some(WORKSPACE)).await;
        ingest(&router, OWN_ALIAS, SAMPLES).await;
        let series = time_series_id(&router, OWN_ALIAS).await;
        // The session dataset holds the first span as its OWN data, so what its
        // delete takes with it is observable once the reference is gone.
        seed_own_segments(&engine, &session, &series, &OWN_SPANS[..1]);

        // The producer side: the curated dataset selects a slice of the session
        // dataset's data, which is what REFERENCES that dataset.
        let entry = json!({
            "sourceDatasetId": session,
            "timeSeriesId": series,
            "startTimestamp": wire_instant(100, 0),
            "endTimestamp": wire_instant(105, 250_000_000),
        });
        let (status, associated) = call(
            &router,
            "POST",
            &format!("/datasets/{curated}/data-segments/associate"),
            Some(json!({
                "workspaceName": WORKSPACE,
                "associateDataSegmentEntries": [entry.clone()],
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the association got {status} {associated}"
        );
        assert_eq!(
            associated["failedAssociations"].as_array().map(Vec::len),
            Some(0),
            "case {case:?}: the association must be applied, got {associated}"
        );

        // The delete is refused while that reference stands.
        let (status, refused) =
            delete_dataset(&router, &session, Some(WORKSPACE), None).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "InvalidRequestException",
            "case {case:?}: unexpected error code in {refused}"
        );
        let message = refused["message"].as_str().unwrap_or("");
        assert!(
            message.contains(&session) && message.contains(&curated),
            "case {case:?}: the refusal must name the dataset and the one referencing it, \
             got {refused}"
        );

        // A refused delete removes NOTHING.
        let (status, described) = describe(&router, &session).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a refused delete must leave the dataset, got {status} {described}"
        );
        assert_eq!(
            history_seconds(&router, OWN_ALIAS).await,
            SAMPLES.to_vec(),
            "case {case:?}: a refused delete must leave every sample, got {:?}",
            history_seconds(&router, OWN_ALIAS).await
        );
        assert_eq!(
            listed_series(&router, &curated, WORKSPACE).await,
            vec![series.clone()],
            "case {case:?}: a refused delete must leave the referencing segment"
        );
        assert!(
            holds_segments(&engine, &session),
            "case {case:?}: a refused delete must leave the dataset's own segment"
        );

        match release {
            Release::DeleteTheReferencingDataset => {
                let (status, deleted) =
                    delete_dataset(&router, &curated, Some(WORKSPACE), None).await;
                assert_accepted(case, status, &deleted);
                // "Deleting a curated dataset doesn't delete the underlying data in
                // the source session datasets": only the reference went.
                let (status, source) = describe(&router, &session).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: the source dataset must stand, got {status} {source}"
                );
                assert_eq!(
                    history_seconds(&router, OWN_ALIAS).await,
                    SAMPLES.to_vec(),
                    "case {case:?}: deleting the referencing dataset must leave the source \
                     data, got {:?}",
                    history_seconds(&router, OWN_ALIAS).await
                );
            }
            Release::DisassociateTheSegments => {
                let (status, body) = call(
                    &router,
                    "POST",
                    &format!("/datasets/{curated}/data-segments/disassociate"),
                    Some(json!({
                        "workspaceName": WORKSPACE,
                        "disassociateDataSegmentEntries": [entry],
                    })),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: the disassociation got {status} {body}"
                );
                assert_eq!(
                    body["failedDisassociations"].as_array().map(Vec::len),
                    Some(0),
                    "case {case:?}: the disassociation must be applied, got {body}"
                );
                assert_eq!(
                    listed_series(&router, &curated, WORKSPACE).await,
                    Vec::<String>::new(),
                    "case {case:?}: the disassociated segment must be gone"
                );
            }
        }

        // With the reference released the delete is accepted, and it takes the
        // dataset's OWN data with it.
        let (status, deleted) = delete_dataset(&router, &session, Some(WORKSPACE), None).await;
        assert_accepted(case, status, &deleted);
        let (status, missing) = describe(&router, &session).await;
        assert_absent(case, status, &missing, &session);
        assert!(
            !holds_segments(&engine, &session),
            "case {case:?}: the deleted dataset must leave no data segment behind"
        );
        assert_eq!(
            history_seconds(&router, OWN_ALIAS).await,
            surviving_samples(&OWN_SPANS[..1]),
            "case {case:?}: the deleted dataset's own data must go with it, got {:?}",
            history_seconds(&router, OWN_ALIAS).await
        );
    }
}

#[tokio::test]
async fn an_id_the_request_does_not_resolve_is_refused_and_removes_nothing() {
    let (router, engine) = router_with_engine();
    crate::support::ensure_workspace(&router, OTHER_WORKSPACE).await;
    let dataset = create_dataset(&router, "Kiln target", Some("SESSION"), Some(WORKSPACE)).await;
    let external = create_dataset(&router, "Kiln external", Some("EXTERNAL"), None).await;
    ingest(&router, OWN_ALIAS, SAMPLES).await;
    let series = time_series_id(&router, OWN_ALIAS).await;
    seed_own_segments(&engine, &dataset, &series, OWN_SPANS);

    // A well-formed id no create minted: the stored id with one nibble changed.
    let mut neighbour: Vec<char> = dataset.chars().collect();
    let at = neighbour
        .iter()
        .position(|ch| *ch != '-' && *ch != '0')
        .expect("a non-zero character in the id");
    neighbour[at] = '0';
    let neighbour: String = neighbour.into_iter().collect();

    // (case, the id the request names, the workspace it scopes itself to, the
    // absence the refusal states) — a request scoping itself to a workspace this
    // service does not hold reports THAT workspace, because a delete resolves the
    // workspace its request names before the dataset it scopes to it.
    let rows: Vec<(&str, String, Option<&str>, String)> = vec![
        (
            "an id that names no dataset",
            ABSENT_DATASET.to_string(),
            None,
            not_found_message(RESOURCE_DATASET, ABSENT_DATASET),
        ),
        (
            "an id one nibble from a stored one",
            neighbour.clone(),
            None,
            not_found_message(RESOURCE_DATASET, &neighbour),
        ),
        (
            "a stored id scoped to another workspace",
            dataset.clone(),
            Some(OTHER_WORKSPACE),
            not_found_message(RESOURCE_DATASET, &dataset),
        ),
        (
            "a stored id scoped to a workspace nobody opened",
            dataset.clone(),
            Some("kiln-ghost"),
            not_found_message(RESOURCE_WORKSPACE, "kiln-ghost"),
        ),
        (
            "a dataset of no workspace scoped to one",
            external.clone(),
            Some(WORKSPACE),
            not_found_message(RESOURCE_DATASET, &external),
        ),
    ];

    for (case, target, workspace, expected) in rows {
        let (status, body) = delete_dataset(&router, &target, workspace, None).await;
        assert_absent_resource(case, status, &body, &expected);

        // The refused delete removed nothing: every dataset, every segment and every
        // sample is where it was.
        let (status, described) = describe(&router, &dataset).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a refused delete must leave the dataset, got {status} {described}"
        );
        let (status, described) = describe(&router, &external).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a refused delete must leave every other dataset, got \
             {status} {described}"
        );
        assert_eq!(
            listed_series(&router, &dataset, WORKSPACE).await,
            vec![series.clone(); OWN_SPANS.len()],
            "case {case:?}: a refused delete must leave the dataset's segments"
        );
        assert_eq!(
            history_seconds(&router, OWN_ALIAS).await,
            SAMPLES.to_vec(),
            "case {case:?}: a refused delete must leave every sample, got {:?}",
            history_seconds(&router, OWN_ALIAS).await
        );
    }

    // The dataset's OWN workspace resolves, so the scoping above refuses only a
    // workspace that does not hold it.
    let (status, deleted) = delete_dataset(&router, &dataset, Some(WORKSPACE), None).await;
    assert_accepted("the dataset's own workspace", status, &deleted);

    // An id this store no longer holds is the same modelled absence as one it never
    // held, so a tokenless second delete is refused rather than silently accepted.
    let (status, body) = delete_dataset(&router, &dataset, Some(WORKSPACE), None).await;
    assert_absent("a second delete of the same dataset", status, &body, &dataset);
}

#[tokio::test]
async fn a_retry_carrying_the_token_of_an_accepted_delete_replays_its_answer() {
    let (router, _engine) = router_with_engine();
    let dataset = create_dataset(&router, "Kiln target", Some("SESSION"), Some(WORKSPACE)).await;
    let other = create_dataset(&router, "Kiln other", Some("SESSION"), Some(WORKSPACE)).await;

    let (status, deleted) =
        delete_dataset(&router, &dataset, Some(WORKSPACE), Some(CLIENT_TOKEN)).await;
    assert_accepted("the first delete", status, &deleted);

    // The retry replays that answer — the dataset it removed, reported DELETING —
    // rather than the not-found refusal the removal itself would now produce.
    let (status, replayed) =
        delete_dataset(&router, &dataset, Some(WORKSPACE), Some(CLIENT_TOKEN)).await;
    assert_accepted("the retry", status, &replayed);
    assert_eq!(
        replayed, deleted,
        "the retry must answer exactly what the call it repeats answered, got {replayed}"
    );

    // And it removed nothing further: the other dataset stands.
    let (status, described) = describe(&router, &other).await;
    assert_eq!(
        status, 200,
        "the retry must remove nothing further, got {status} {described}"
    );

    // The SAME token on a DIFFERENT request belongs to another call, which is the
    // modelled conflict naming the dataset that call acted on.
    let (status, conflict) =
        delete_dataset(&router, &other, Some(WORKSPACE), Some(CLIENT_TOKEN)).await;
    assert_eq!(
        status, 409,
        "a spent token expected 409, got {status} {conflict}"
    );
    assert_eq!(
        error_code(&conflict),
        "ConflictingOperationException",
        "unexpected error code in {conflict}"
    );
    assert_eq!(
        conflict["resourceId"].as_str(),
        Some(dataset.as_str()),
        "the conflict must name the dataset the spending call acted on, got {conflict}"
    );

    // The refused request removed nothing, so another token still deletes it.
    let (status, described) = describe(&router, &other).await;
    assert_eq!(
        status, 200,
        "a refused delete must leave the dataset, got {status} {described}"
    );
    let (status, deleted) =
        delete_dataset(&router, &other, Some(WORKSPACE), Some(OTHER_CLIENT_TOKEN)).await;
    assert_accepted("a fresh token", status, &deleted);
    let (status, missing) = describe(&router, &other).await;
    assert_absent("after the second delete", status, &missing, &other);
}
