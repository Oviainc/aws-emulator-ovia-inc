// SPDX-License-Identifier: CC0-1.0

//! A paginated listing's continuation token is a position in the collection the
//! request's own FILTER selected.
//!
//! RULE: a listing pages the records left after the request's filter is applied, so the
//! filter — not only the collection's scope and the order it is walked in — is part of
//! the collection a token is a position in: a token minted while paging the records one
//! filter selects is refused with `ValidationException` (HTTP 400) and this service's
//! fixed invalid-token sentence when it is replayed under any other filter, or under none,
//! rather than honoured as a resume point in a walk it is not a position in (which would
//! serve a page that skips records the caller never saw).
//!
//! The three paginated listings this crate serves that take a filter are covered here:
//! `ListDatastores` (`datastoreStatus`), `ListDICOMImportJobs` (`jobStatus`) and
//! `SearchImageSets` (`searchCriteria.filters`). `ListImageSetVersions` takes no filter,
//! so its token names only the image set whose versions it walks.
//!
//! Each suite is a TABLE over the filter axis: the rows are every filter a request can
//! page under crossed with every filter it can be replayed under — for the two status
//! filters, the whole catalog the model declares plus "no filter"; for the search, filters
//! over different attributes, both operators, two of them ANDed, and that same conjunction
//! written in the other order (which selects the same image sets, so it is the one
//! cross-filter replay that must still be honoured). The diagonal is the control: a token
//! replayed under the filter that minted it resumes the walk exactly where the previous
//! page stopped. Every case drives real HTTP through the production front door
//! (`crate::datastore_lifecycle`'s harness); nothing is asserted through the state struct.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use service_com_amazonaws_medicalimaging_ahigatewayservice::ops::types::{
    DATASTORE_STATUS_VALUES, JOB_STATUS_VALUES,
};
use service_com_amazonaws_medicalimaging_ahigatewayservice::state::{
    service_state, DatastoreStatus, JobStatus, DATASTORE_ACTIVATION_DELAY,
    INVALID_NEXT_TOKEN_MESSAGE,
};

use crate::datastore_lifecycle::{Answer, Emulator, SETTLE_MULTIPLE};
use crate::image_set_copy::seed_image_set_in_hierarchy;
use crate::image_set_search::search;
use crate::import_job_listing::list_jobs;

/// How far out the clock's next move on a seeded record is pushed, so the status a row
/// was seeded in holds for the whole of a case instead of racing the scheduler.
const HELD: Duration = Duration::from_secs(3600);

/// How many records each status holds, so every filter's walk has a page boundary inside
/// it and therefore mints a token.
const PER_STATUS: usize = 2;

/// A `RoleArn` every import-job submit below uses (len 20..2048,
/// `^arn:aws(-[^:]+)?:iam::[0-9]{12}:role/.+$`).
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/ImportRole";

/// The identifier member of every item one answer's `list` holds, in the order served.
fn listed_ids(answer: &Answer, list: &str, id_member: &str) -> Vec<String> {
    answer
        .field(&[list])
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| item[id_member].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The filter axis of a status-filtered listing: no filter at all, plus every value the
/// model declares for that filter.
fn filter_axis(catalog: &'static [&'static str]) -> Vec<Option<&'static str>> {
    std::iter::once(None)
        .chain(catalog.iter().copied().map(Some))
        .collect()
}

/// Assert that `answer` is this service's refusal of a token that is not a position in
/// the walk the request asked for: HTTP 400, `ValidationException`, the fixed sentence,
/// and NO page beside it.
fn assert_refused(case: &str, answer: &Answer, list: &str) {
    assert_eq!(
        answer.status, 400,
        "case {case}: expected HTTP 400 for a token minted under another filter, got {answer:?}"
    );
    assert_eq!(
        answer.error_code, "ValidationException",
        "case {case}: expected ValidationException, got {answer:?}"
    );
    assert_eq!(
        answer.string(&["message"]),
        INVALID_NEXT_TOKEN_MESSAGE,
        "case {case}: expected the service's fixed invalid-token sentence, got {answer:?}"
    );
    assert!(
        answer.field(&[list]).is_null(),
        "case {case}: expected no page beside the refusal, got {answer:?}"
    );
}

// ---------------------------------------------------------------------------
// ListDatastores — the `datastoreStatus` filter
// ---------------------------------------------------------------------------

/// Create [`PER_STATUS`] data stores for every status the model declares and hold each
/// one in the status its row names, answering the ids in creation order.
///
/// The rounds are created status by status, so each status's stores are SPREAD through
/// the unfiltered order: a token honoured in a collection it was not minted in therefore
/// lands mid-way through another walk, where it would skip records, rather than at its
/// start or past its end.
async fn seeded_datastores(emulator: &Emulator) -> Vec<(String, &'static str)> {
    let mut seeded: Vec<(String, &'static str)> = Vec::new();
    for round in 0..PER_STATUS {
        for status in DATASTORE_STATUS_VALUES {
            let created = emulator
                .create_datastore(json!({
                    "datastoreName": format!("filter.tokens.{status}.{round}"),
                    "clientToken": format!("filter-tokens-{status}-{round}"),
                }))
                .await;
            assert_eq!(
                created.status, 200,
                "seeding a {status} store (round {round}): expected HTTP 200, got {created:?}"
            );
            seeded.push((created.string(&["datastoreId"]), status));
        }
    }
    // One wait for the whole batch, so every store has left `CREATING` on its own before
    // any row is held in the status it names.
    tokio::time::sleep(SETTLE_MULTIPLE * DATASTORE_ACTIVATION_DELAY).await;

    let state = service_state(emulator.engine()).expect("the medical-imaging state store");
    let mut store = state.lock();
    let held_until = SystemTime::now() + HELD;
    for (datastore_id, status) in &seeded {
        let datastore = store
            .datastores
            .get_mut(datastore_id)
            .expect("a seeded data store");
        datastore.datastore_status =
            DatastoreStatus::from_wire(status).expect("a status the model declares");
        datastore.status_due_at = Some(held_until);
    }
    drop(store);
    seeded
}

/// The `datastoreStatus` query one filter row sends, empty when the row filters nothing.
fn datastore_filter_query(filter: Option<&str>) -> String {
    filter
        .map(|status| format!("&datastoreStatus={status}"))
        .unwrap_or_default()
}

/// Every data store one filter's walk reports in a single page, in the order it walks
/// them.
async fn datastores_under(emulator: &Emulator, filter: Option<&str>) -> Vec<String> {
    let answer = emulator
        .list_datastores(&format!(
            "maxResults=50{}",
            datastore_filter_query(filter)
        ))
        .await;
    assert_eq!(
        answer.status, 200,
        "case filter={filter:?}: expected HTTP 200 from an unpaged listing, got {answer:?}"
    );
    listed_ids(&answer, "datastoreSummaries", "datastoreId")
}

/// The token one filter's walk mints after serving its first record.
async fn datastore_token(emulator: &Emulator, filter: Option<&str>) -> String {
    let answer = emulator
        .list_datastores(&format!("maxResults=1{}", datastore_filter_query(filter)))
        .await;
    assert_eq!(
        answer.status, 200,
        "case filter={filter:?}: expected HTTP 200 from the first page, got {answer:?}"
    );
    answer
        .field(&["nextToken"])
        .as_str()
        .unwrap_or_else(|| panic!("case filter={filter:?}: expected a nextToken, got {answer:?}"))
        .to_string()
}

#[tokio::test]
async fn a_datastore_listing_token_is_a_position_only_under_the_filter_that_minted_it() {
    let emulator = Emulator::new();
    let seeded = seeded_datastores(&emulator).await;
    assert_eq!(
        seeded.len(),
        PER_STATUS * DATASTORE_STATUS_VALUES.len(),
        "expected one seeded store per status per round, got {seeded:?}"
    );

    let axis = filter_axis(DATASTORE_STATUS_VALUES);
    for minted_under in &axis {
        let walk = datastores_under(&emulator, *minted_under).await;
        assert!(
            walk.len() >= 2,
            "case minted_under={minted_under:?}: expected this filter's walk to hold at least \
             two stores, got {walk:?}"
        );
        let token = datastore_token(&emulator, *minted_under).await;

        for replayed_under in &axis {
            let case = format!("minted_under={minted_under:?} replayed_under={replayed_under:?}");
            let answer = emulator
                .list_datastores(&format!(
                    "maxResults=1&nextToken={token}{}",
                    datastore_filter_query(*replayed_under)
                ))
                .await;
            if replayed_under == minted_under {
                assert_eq!(
                    answer.status, 200,
                    "case {case}: expected the token to be honoured under the filter that \
                     minted it, got {answer:?}"
                );
                assert_eq!(
                    listed_ids(&answer, "datastoreSummaries", "datastoreId"),
                    vec![walk[1].clone()],
                    "case {case}: expected the resume to serve the record after the boundary, \
                     got {answer:?}"
                );
            } else {
                assert_refused(&case, &answer, "datastoreSummaries");
            }
        }
    }
}

#[tokio::test]
async fn paging_a_filtered_datastore_listing_visits_its_records_exactly_once() {
    let emulator = Emulator::new();
    seeded_datastores(&emulator).await;

    // Page sizes either side of each filter's walk, including both published `maxResults`
    // bounds (1 and 50).
    for page_size in [1_usize, 2, 3, 50] {
        for filter in filter_axis(DATASTORE_STATUS_VALUES) {
            let expected = datastores_under(&emulator, filter).await;
            let case = format!("filter={filter:?} maxResults={page_size}");

            let mut walked: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            for page in 0..=expected.len() + 1 {
                let mut query = format!("maxResults={page_size}{}", datastore_filter_query(filter));
                if let Some(value) = &token {
                    query.push_str(&format!("&nextToken={value}"));
                }
                let answer = emulator.list_datastores(&query).await;
                assert_eq!(
                    answer.status, 200,
                    "case {case} page={page}: expected HTTP 200, got {answer:?}"
                );
                let served = listed_ids(&answer, "datastoreSummaries", "datastoreId");
                assert!(
                    served.len() <= page_size,
                    "case {case} page={page}: expected at most {page_size} summaries, got \
                     {served:?}"
                );
                walked.extend(served);
                match answer.field(&["nextToken"]).as_str() {
                    Some(next) => token = Some(next.to_string()),
                    None => {
                        token = None;
                        break;
                    }
                }
            }
            assert!(
                token.is_none(),
                "case {case}: expected the walk to terminate, got {walked:?}"
            );
            assert_eq!(
                walked, expected,
                "case {case}: expected the walk to visit this filter's stores exactly once in \
                 order, got {walked:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// ListDICOMImportJobs — the `jobStatus` filter
// ---------------------------------------------------------------------------

/// Submit [`PER_STATUS`] import jobs for every status the model declares into one data
/// store and hold each one in the status its row names, answering the data store's id.
async fn seeded_import_jobs(emulator: &Emulator) -> String {
    let datastore_id = emulator
        .active_datastore("filter.tokens.jobs", "filter-tokens-jobs")
        .await;

    let mut seeded: Vec<(String, &'static str)> = Vec::new();
    for round in 0..PER_STATUS {
        for status in JOB_STATUS_VALUES {
            let started = emulator
                .start_import_job(
                    &datastore_id,
                    json!({
                        "clientToken": format!("filter-tokens-job-{status}-{round}"),
                        "dataAccessRoleArn": ROLE_ARN,
                        "inputS3Uri": "s3://imaging-in/",
                        "outputS3Uri": "s3://imaging-out/",
                    }),
                )
                .await;
            assert_eq!(
                started.status, 200,
                "seeding a {status} job (round {round}): expected HTTP 200, got {started:?}"
            );
            seeded.push((started.string(&["jobId"]), status));
        }
    }

    let state = service_state(emulator.engine()).expect("the medical-imaging state store");
    let mut store = state.lock();
    let held_until = SystemTime::now() + HELD;
    let datastore = store
        .datastores
        .get_mut(&datastore_id)
        .expect("the seeded data store");
    for (job_id, status) in &seeded {
        let job = datastore
            .import_jobs
            .get_mut(job_id)
            .expect("a seeded import job");
        job.job_status = JobStatus::from_wire(status).expect("a status the model declares");
        // Pushed out so the clock owes a job still on its way nothing for the whole case.
        job.in_progress_due_at = held_until;
        job.completed_due_at = held_until;
        // A job that has ended reports when it did, exactly as one the clock finished.
        job.ended_at = (job.job_status == JobStatus::Completed).then_some(job.submitted_at);
    }
    drop(store);
    datastore_id
}

/// The `jobStatus` query one filter row sends, empty when the row filters nothing.
fn job_filter_query(filter: Option<&str>) -> String {
    filter
        .map(|status| format!("&jobStatus={status}"))
        .unwrap_or_default()
}

/// Every import job one filter's walk reports in a single page, in the order it walks
/// them.
async fn jobs_under(emulator: &Emulator, datastore_id: &str, filter: Option<&str>) -> Vec<String> {
    let answer = list_jobs(
        emulator,
        datastore_id,
        &format!("maxResults=50{}", job_filter_query(filter)),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case filter={filter:?}: expected HTTP 200 from an unpaged listing, got {answer:?}"
    );
    listed_ids(&answer, "jobSummaries", "jobId")
}

#[tokio::test]
async fn an_import_job_listing_token_is_a_position_only_under_the_filter_that_minted_it() {
    let emulator = Emulator::new();
    let datastore_id = seeded_import_jobs(&emulator).await;

    let axis = filter_axis(JOB_STATUS_VALUES);
    for minted_under in &axis {
        let walk = jobs_under(&emulator, &datastore_id, *minted_under).await;
        assert!(
            walk.len() >= 2,
            "case minted_under={minted_under:?}: expected this filter's walk to hold at least \
             two jobs, got {walk:?}"
        );
        let first_page = list_jobs(
            &emulator,
            &datastore_id,
            &format!("maxResults=1{}", job_filter_query(*minted_under)),
        )
        .await;
        let token = first_page
            .field(&["nextToken"])
            .as_str()
            .unwrap_or_else(|| {
                panic!("case minted_under={minted_under:?}: expected a nextToken, got {first_page:?}")
            })
            .to_string();

        for replayed_under in &axis {
            let case = format!("minted_under={minted_under:?} replayed_under={replayed_under:?}");
            let answer = list_jobs(
                &emulator,
                &datastore_id,
                &format!(
                    "maxResults=1&nextToken={token}{}",
                    job_filter_query(*replayed_under)
                ),
            )
            .await;
            if replayed_under == minted_under {
                assert_eq!(
                    answer.status, 200,
                    "case {case}: expected the token to be honoured under the filter that \
                     minted it, got {answer:?}"
                );
                assert_eq!(
                    listed_ids(&answer, "jobSummaries", "jobId"),
                    vec![walk[1].clone()],
                    "case {case}: expected the resume to serve the record after the boundary, \
                     got {answer:?}"
                );
            } else {
                // `jobSummaries` is `@required`, so a refusal carries no list at all.
                assert_refused(&case, &answer, "jobSummaries");
            }
        }
    }
}

// ---------------------------------------------------------------------------
// SearchImageSets — the `searchCriteria.filters` conjunction
// ---------------------------------------------------------------------------

/// The instant the seeded `createdAt` values are measured from — whole seconds, so a
/// `BETWEEN` bound below is exactly the number the record holds.
const BASE_EPOCH_SECS: u64 = 1_767_225_600;

/// The number a `createdAt` bound carries for a seeded offset.
fn wire_instant(offset_secs: u64) -> f64 {
    (BASE_EPOCH_SECS + offset_secs) as f64
}

/// One image set to seed: the attributes the filters below select on.
#[derive(Debug, Clone, Copy)]
struct SearchSeed {
    patient_id: &'static str,
    is_primary: bool,
    created_offset_secs: u64,
}

/// The seeded image sets. No two filter sets below select the same subset of them — bar
/// the one pair that states the SAME conjunction in the other order — so every refusal a
/// row asserts is about a token minted in a different walk.
fn search_seeds() -> Vec<SearchSeed> {
    vec![
        SearchSeed {
            patient_id: "PID-1",
            is_primary: true,
            created_offset_secs: 0,
        },
        SearchSeed {
            patient_id: "PID-1",
            is_primary: true,
            created_offset_secs: 10,
        },
        SearchSeed {
            patient_id: "PID-2",
            is_primary: true,
            created_offset_secs: 20,
        },
        SearchSeed {
            patient_id: "PID-1",
            is_primary: false,
            created_offset_secs: 30,
        },
        SearchSeed {
            patient_id: "PID-2",
            is_primary: false,
            created_offset_secs: 40,
        },
        SearchSeed {
            patient_id: "PID-2",
            is_primary: false,
            created_offset_secs: 50,
        },
    ]
}

/// Seed one data store holding [`search_seeds`]'s image sets, and answer its id.
///
/// The image sets themselves are built by the crate's own seeder
/// (`crate::image_set_copy::seed_image_set_in_hierarchy`, which is how every image-set
/// suite mints one); this only writes the attributes the filters read.
async fn seeded_image_sets(emulator: &Emulator) -> String {
    let datastore_id = emulator
        .active_datastore("filter.tokens.search", "filter-tokens-search")
        .await;
    for seed in search_seeds() {
        let image_set_id =
            seed_image_set_in_hierarchy(emulator, &datastore_id, 1, seed.is_primary);
        let state = service_state(emulator.engine()).expect("the medical-imaging state store");
        let mut store = state.lock();
        let image_set = store
            .datastores
            .get_mut(&datastore_id)
            .expect("the seeded data store")
            .image_sets
            .get_mut(&image_set_id)
            .expect("a seeded image set");
        image_set.created_at =
            SystemTime::UNIX_EPOCH + Duration::from_secs(BASE_EPOCH_SECS + seed.created_offset_secs);
        image_set.dicom_tags.dicom_patient_id = Some(seed.patient_id.to_string());
    }
    datastore_id
}

/// One row of the search filter axis: the criteria it pages under, and which other rows
/// select the same image sets it does (its own label included).
struct SearchFilterCase {
    label: &'static str,
    criteria: Option<Value>,
    /// The labels whose criteria state the SAME conjunction, so a token minted under this
    /// row is a position in their walks too.
    equivalent: &'static [&'static str],
}

/// The search filter axis: no filter, one filter over each of two attributes, both
/// operators, two filters ANDed, and that same conjunction written in the other order.
///
/// The attributes and operators are the ones the model declares
/// (`smithy_query(query="shape",shape="SearchByAttributeValue")` and
/// `smithy_query(query="shape",shape="Operator")`). ANDed filters select the image sets
/// every one of them holds of, so the order they are written in cannot change the walk:
/// the `.reversed` row is the one cross-row replay that must still be honoured.
fn search_filter_cases() -> Vec<SearchFilterCase> {
    vec![
        SearchFilterCase {
            label: "no filters",
            criteria: None,
            equivalent: &["no filters"],
        },
        SearchFilterCase {
            label: "patient PID-1",
            criteria: Some(json!({
                "filters": [{ "operator": "EQUAL", "values": [{ "DICOMPatientId": "PID-1" }] }]
            })),
            equivalent: &["patient PID-1"],
        },
        SearchFilterCase {
            label: "patient PID-2",
            criteria: Some(json!({
                "filters": [{ "operator": "EQUAL", "values": [{ "DICOMPatientId": "PID-2" }] }]
            })),
            equivalent: &["patient PID-2"],
        },
        SearchFilterCase {
            label: "primary only",
            criteria: Some(json!({
                "filters": [{ "operator": "EQUAL", "values": [{ "isPrimary": true }] }]
            })),
            equivalent: &["primary only"],
        },
        SearchFilterCase {
            label: "createdAt range",
            criteria: Some(json!({
                "filters": [{
                    "operator": "BETWEEN",
                    "values": [
                        { "createdAt": wire_instant(5) },
                        { "createdAt": wire_instant(45) },
                    ],
                }]
            })),
            equivalent: &["createdAt range"],
        },
        SearchFilterCase {
            label: "patient PID-1 and primary",
            criteria: Some(json!({
                "filters": [
                    { "operator": "EQUAL", "values": [{ "DICOMPatientId": "PID-1" }] },
                    { "operator": "EQUAL", "values": [{ "isPrimary": true }] },
                ]
            })),
            equivalent: &["patient PID-1 and primary", "primary and patient PID-1"],
        },
        SearchFilterCase {
            label: "primary and patient PID-1",
            criteria: Some(json!({
                "filters": [
                    { "operator": "EQUAL", "values": [{ "isPrimary": true }] },
                    { "operator": "EQUAL", "values": [{ "DICOMPatientId": "PID-1" }] },
                ]
            })),
            equivalent: &["patient PID-1 and primary", "primary and patient PID-1"],
        },
    ]
}

#[tokio::test]
async fn a_search_token_is_a_position_only_under_the_filters_that_minted_it() {
    let emulator = Emulator::new();
    let datastore_id = seeded_image_sets(&emulator).await;
    let rows = search_filter_cases();

    for minted_under in &rows {
        let whole = search(
            &emulator,
            &datastore_id,
            minted_under.criteria.clone(),
            "maxResults=50",
        )
        .await;
        assert_eq!(
            whole.status, 200,
            "case minted_under={:?}: expected HTTP 200 from an unpaged search, got {whole:?}",
            minted_under.label
        );
        let walk = listed_ids(&whole, "imageSetsMetadataSummaries", "imageSetId");
        assert!(
            walk.len() >= 2,
            "case minted_under={:?}: expected this filter's result to hold at least two image \
             sets, got {walk:?}",
            minted_under.label
        );

        let first_page = search(
            &emulator,
            &datastore_id,
            minted_under.criteria.clone(),
            "maxResults=1",
        )
        .await;
        let token = first_page
            .field(&["nextToken"])
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "case minted_under={:?}: expected a nextToken, got {first_page:?}",
                    minted_under.label
                )
            })
            .to_string();

        for replayed_under in &rows {
            let case = format!(
                "minted_under={:?} replayed_under={:?}",
                minted_under.label, replayed_under.label
            );
            let answer = search(
                &emulator,
                &datastore_id,
                replayed_under.criteria.clone(),
                &format!("maxResults=1&nextToken={token}"),
            )
            .await;
            if minted_under.equivalent.contains(&replayed_under.label) {
                assert_eq!(
                    answer.status, 200,
                    "case {case}: expected the token to be honoured under criteria stating the \
                     same conjunction, got {answer:?}"
                );
                assert_eq!(
                    listed_ids(&answer, "imageSetsMetadataSummaries", "imageSetId"),
                    vec![walk[1].clone()],
                    "case {case}: expected the resume to serve the image set after the boundary, \
                     got {answer:?}"
                );
            } else {
                // `imageSetsMetadataSummaries` is `@required`, so a refusal carries no
                // result list at all.
                assert_refused(&case, &answer, "imageSetsMetadataSummaries");
            }
        }
    }
}
