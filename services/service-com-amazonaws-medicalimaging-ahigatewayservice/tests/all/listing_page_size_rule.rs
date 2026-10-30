// SPDX-License-Identifier: CC0-1.0

//! Every paginated listing this crate serves bounds its page by the SAME page-size rule.
//!
//! RULE: a page holds exactly the `maxResults` records the caller asked for while records
//! are left over (so the page a token comes with is a FULL page), never more than that,
//! and when the request names no `maxResults` the page holds up to the published maximum
//! — one rule, applied identically by every paginated listing, so a walk serves the
//! collection exactly once in its order whatever page size it is driven with.
//!
//! This suite drives that one rule through each of the four listings whose page size used
//! to be computed in the handler itself — `ListDatastores`, `ListDICOMImportJobs`,
//! `ListImageSetVersions` and `SearchImageSets` — so the single
//! `crate::state::list_page_size` they now share is exercised from every one of them and
//! answers each the way it answered before.
//!
//! The table's rows differ along page size against collection size: both published
//! `maxResults` bounds (1 and 50, which `MIN_LIST_PAGE_SIZE` / `MAX_LIST_PAGE_SIZE` name),
//! sizes either side of the collection, one exactly its size, and the request that names
//! no `maxResults` at all. Every case drives real HTTP through the production front door
//! (`crate::datastore_lifecycle`'s harness).

use serde_json::json;

use service_com_amazonaws_medicalimaging_ahigatewayservice::state::{
    MAX_LIST_PAGE_SIZE, MIN_LIST_PAGE_SIZE,
};

use crate::datastore_lifecycle::{Answer, Emulator};
use crate::image_set_copy::{seed_image_set, seed_image_set_in_hierarchy};
use crate::image_set_search::search;
use crate::import_job_listing::list_jobs;

/// A `RoleArn` the import-job submits below use (len 20..2048,
/// `^arn:aws(-[^:]+)?:iam::[0-9]{12}:role/.+$`).
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/ImportRole";

/// How many records each seeded collection holds: more than the small page sizes below,
/// so a walk driven with one of them crosses several page boundaries, and fewer than
/// [`MAX_LIST_PAGE_SIZE`], so the row that names no `maxResults` serves the whole
/// collection in one page.
const RECORDS: usize = 6;

/// How many pages a walk may take before it is called non-terminating.
const PAGE_LIMIT: usize = 64;

/// One of the paginated listings, already seeded with the collection it walks.
enum Listing {
    Datastores,
    ImportJobs {
        datastore_id: String,
    },
    ImageSetVersions {
        datastore_id: String,
        image_set_id: String,
    },
    ImageSetSearch {
        datastore_id: String,
    },
}

impl Listing {
    /// The operation this row drives, for a case label.
    fn label(&self) -> &'static str {
        match self {
            Listing::Datastores => "ListDatastores",
            Listing::ImportJobs { .. } => "ListDICOMImportJobs",
            Listing::ImageSetVersions { .. } => "ListImageSetVersions",
            Listing::ImageSetSearch { .. } => "SearchImageSets",
        }
    }

    /// The output member holding the page, and the member of each item that identifies
    /// the record, as this operation's output shape names them
    /// (`smithy_query(query="operation",operation="ListDatastores,ListDICOMImportJobs,ListImageSetVersions,SearchImageSets")`).
    fn page_members(&self) -> (&'static str, &'static str) {
        match self {
            Listing::Datastores => ("datastoreSummaries", "datastoreId"),
            Listing::ImportJobs { .. } => ("jobSummaries", "jobId"),
            Listing::ImageSetVersions { .. } => ("imageSetPropertiesList", "versionId"),
            Listing::ImageSetSearch { .. } => ("imageSetsMetadataSummaries", "imageSetId"),
        }
    }

    /// One page of this listing, requested with the query string `query`.
    async fn page(&self, emulator: &Emulator, query: &str) -> Answer {
        match self {
            Listing::Datastores => emulator.list_datastores(query).await,
            Listing::ImportJobs { datastore_id } => list_jobs(emulator, datastore_id, query).await,
            Listing::ImageSetVersions {
                datastore_id,
                image_set_id,
            } => {
                // `POST /datastore/{datastoreId}/imageSet/{imageSetId}/listImageSetVersions`
                // — `maxResults` and `nextToken` are `@httpQuery`.
                let mut uri =
                    format!("/datastore/{datastore_id}/imageSet/{image_set_id}/listImageSetVersions");
                if !query.is_empty() {
                    uri.push('?');
                    uri.push_str(query);
                }
                emulator.call("POST", &uri, None).await
            }
            // The empty criteria: filtered and sorted by nothing, which is what an SDK
            // puts on the wire when the caller names none.
            Listing::ImageSetSearch { datastore_id } => {
                search(emulator, datastore_id, None, query).await
            }
        }
    }

    /// The identifier of every record one answer's page holds, in the order served.
    fn served(&self, answer: &Answer) -> Vec<String> {
        let (list, id_member) = self.page_members();
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
}

/// Seed [`RECORDS`] data stores into a fresh emulator, so `ListDatastores` walks that
/// many.
///
/// Every listing below gets its OWN emulator, so each collection holds exactly the
/// records its own seeding put there — the three other seedings create data stores of
/// their own.
async fn seed_datastores() -> (Emulator, Listing) {
    let emulator = Emulator::new();
    for record in 0..RECORDS {
        let created = emulator
            .create_datastore(json!({
                "datastoreName": format!("page.size.rule.{record}"),
                "clientToken": format!("page-size-rule-{record}"),
            }))
            .await;
        assert_eq!(
            created.status, 200,
            "seeding data store {record}: expected HTTP 200, got {created:?}"
        );
    }
    (emulator, Listing::Datastores)
}

/// Seed one data store holding [`RECORDS`] import jobs.
async fn seed_import_jobs() -> (Emulator, Listing) {
    let emulator = Emulator::new();
    let datastore_id = emulator
        .active_datastore("page.size.rule.jobs", "page-size-rule-jobs")
        .await;
    for record in 0..RECORDS {
        let started = emulator
            .start_import_job(
                &datastore_id,
                json!({
                    "clientToken": format!("page-size-rule-job-{record}"),
                    "dataAccessRoleArn": ROLE_ARN,
                    "inputS3Uri": "s3://imaging-in/",
                    "outputS3Uri": "s3://imaging-out/",
                }),
            )
            .await;
        assert_eq!(
            started.status, 200,
            "seeding import job {record}: expected HTTP 200, got {started:?}"
        );
    }
    (emulator, Listing::ImportJobs { datastore_id })
}

/// Seed one image set holding [`RECORDS`] versions.
async fn seed_image_set_versions() -> (Emulator, Listing) {
    let emulator = Emulator::new();
    let datastore_id = emulator
        .active_datastore("page.size.rule.versions", "page-size-rule-versions")
        .await;
    let image_set_id = seed_image_set(&emulator, &datastore_id, RECORDS as u32);
    (
        emulator,
        Listing::ImageSetVersions {
            datastore_id,
            image_set_id,
        },
    )
}

/// Seed one data store holding [`RECORDS`] image sets, which is what an unfiltered
/// `SearchImageSets` walks.
async fn seed_searchable_image_sets() -> (Emulator, Listing) {
    let emulator = Emulator::new();
    let datastore_id = emulator
        .active_datastore("page.size.rule.search", "page-size-rule-search")
        .await;
    for record in 0..RECORDS {
        seed_image_set_in_hierarchy(&emulator, &datastore_id, 1, record % 2 == 0);
    }
    (emulator, Listing::ImageSetSearch { datastore_id })
}

/// The query string one row sends: the `maxResults` it names, or nothing when it names
/// none, with `extra` appended.
fn query_for(page_size: Option<usize>, extra: &str) -> String {
    let mut query = match page_size {
        Some(requested) => format!("maxResults={requested}"),
        None => String::new(),
    };
    if !extra.is_empty() {
        if !query.is_empty() {
            query.push('&');
        }
        query.push_str(extra);
    }
    query
}

/// Walk `listing` from its start with `page_size`, asserting the page-size rule on every
/// page, and answer every identifier served in order.
async fn walk(emulator: &Emulator, listing: &Listing, page_size: Option<usize>) -> Vec<String> {
    let effective = page_size.unwrap_or(MAX_LIST_PAGE_SIZE);
    let case = format!("{} maxResults={page_size:?}", listing.label());

    let mut served: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0_usize;
    loop {
        pages += 1;
        assert!(
            pages <= PAGE_LIMIT,
            "case {case}: the walk did not terminate after {pages} pages, served {served:?}"
        );
        let extra = token
            .as_deref()
            .map(|value| format!("nextToken={value}"))
            .unwrap_or_default();
        let answer = listing.page(emulator, &query_for(page_size, &extra)).await;
        assert_eq!(
            answer.status, 200,
            "case {case} page={pages}: expected HTTP 200, got {answer:?}"
        );

        let on_this_page = listing.served(&answer);
        assert!(
            on_this_page.len() <= effective,
            "case {case} page={pages}: expected at most {effective} records in a page, got {} in \
             {answer:?}",
            on_this_page.len()
        );
        token = answer.field(&["nextToken"]).as_str().map(ToString::to_string);
        if token.is_some() {
            // Records are left over, so this page is the full page the caller asked for:
            // the rule hands back a short page only where the collection runs out.
            assert_eq!(
                on_this_page.len(),
                effective,
                "case {case} page={pages}: expected a full page of {effective} records beside a \
                 nextToken, got {on_this_page:?}"
            );
        }
        served.extend(on_this_page);
        if token.is_none() {
            return served;
        }
    }
}

/// RULE: the one page-size rule every paginated listing shares — `maxResults` records per
/// page while records are left over, and the published maximum when the request names no
/// `maxResults` — serves each collection exactly once in its own order, from every
/// listing, whatever page size the walk is driven with.
#[tokio::test]
async fn every_paginated_listing_bounds_its_page_by_the_one_page_size_rule() {
    // The published bounds of `maxResults` (range 1..50, which `MIN_LIST_PAGE_SIZE` /
    // `MAX_LIST_PAGE_SIZE` name), sizes either side of the collection, one exactly its
    // size, and the request that names no `maxResults`.
    let page_sizes: Vec<Option<usize>> = vec![
        None,
        Some(MIN_LIST_PAGE_SIZE),
        Some(2),
        Some(RECORDS - 1),
        Some(RECORDS),
        Some(RECORDS + 1),
        Some(MAX_LIST_PAGE_SIZE),
    ];

    let seeded = vec![
        seed_datastores().await,
        seed_import_jobs().await,
        seed_image_set_versions().await,
        seed_searchable_image_sets().await,
    ];

    for (emulator, listing) in &seeded {
        // The whole collection in one page, which is what every row's walk must add up
        // to — taken at the published maximum, above the collection's size.
        let whole = listing
            .page(emulator, &query_for(Some(MAX_LIST_PAGE_SIZE), ""))
            .await;
        assert_eq!(
            whole.status, 200,
            "case {}: expected HTTP 200 from an unpaged listing, got {whole:?}",
            listing.label()
        );
        let expected = listing.served(&whole);
        assert_eq!(
            expected.len(),
            RECORDS,
            "case {}: expected the seeded collection to hold {RECORDS} records, got {expected:?}",
            listing.label()
        );
        assert!(
            whole.field(&["nextToken"]).is_null(),
            "case {}: expected no nextToken on a page that exhausts the collection, got {whole:?}",
            listing.label()
        );

        for page_size in &page_sizes {
            let walked = walk(emulator, listing, *page_size).await;
            assert_eq!(
                walked,
                expected,
                "case {} maxResults={page_size:?}: expected the walk to serve every record \
                 exactly once in the collection's order, got {walked:?}",
                listing.label()
            );
        }
    }
}
