// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for RESUMING a page token of a listing whose held sequence GREW while
//! the client was between two pages of one walk.
//!
//! The rule: a page token this service issued names a position that the only change its
//! held sequence undergoes — a record being appended to the creation order the listing is
//! built from — does not move, so following the token to exhaustion reports every record
//! the store held for the whole walk EXACTLY ONCE, in the order the listing reports, and a
//! record created after the walk passed the head of the sequence is reported by the NEXT
//! walk rather than injected into this one. For a listing that reports its sequence
//! newest-first the position therefore counts from the OLD end of it, and for one that
//! reports it in creation order from the front.
//!
//! Each test asserts that rule over a TABLE whose rows differ in what the rule generalises
//! over — which listing is paged and which end its token is anchored at, how many records
//! are held when the walk starts, how big a page is, and when and how many records are
//! created during the walk — never over one cited request.
//!
//! The HTTP harness (`emulator_with_engine`, `call`, `Answer`, `recovery_point`) and the
//! export-listing helpers (`seed_job`, `passable_role`) are the ones the sibling suites
//! already use: every suite drives the same emulator the same way.

use std::time::SystemTime;

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_backupsearch_cryobackupsearchservice::state;

use crate::export_listing::{passable_role, seed_job};
use crate::search_jobs::{call, emulator_with_engine, recovery_point, Answer};

/// The region every call in this suite is signed for — the one `seed_job` mints its ARNs
/// in, so a seeded record and a called one belong to the same listing.
const REGION: &str = "us-west-2";

/// Which of the two NEWEST-FIRST job listings of this service a row pages. Both report
/// the reverse of a creation order the store only appends to, which is the sequence shape
/// the rule is about.
#[derive(Clone, Copy, Debug)]
enum Listing {
    SearchJobs,
    ExportJobs,
}

impl Listing {
    /// The resource path the listing is served at.
    fn path(self) -> &'static str {
        match self {
            Listing::SearchJobs => "/search-jobs",
            Listing::ExportJobs => "/export-search-jobs",
        }
    }

    /// The output member carrying the page.
    fn page_member(self) -> &'static str {
        match self {
            Listing::SearchJobs => "SearchJobs",
            Listing::ExportJobs => "ExportJobs",
        }
    }

    /// The summary member naming the record, which is what a walk collects.
    fn identifier_member(self) -> &'static str {
        match self {
            Listing::SearchJobs => "SearchJobIdentifier",
            Listing::ExportJobs => "ExportJobIdentifier",
        }
    }
}

/// When, during one walk, records are appended to the listing's held sequence.
#[derive(Clone, Copy, Debug)]
enum Creation {
    /// Nothing is created: the control, a walk of a sequence that stands still.
    Never,
    /// `count` records created in one go, just before the walk's `after_pages`-th resumed
    /// page.
    Once { after_pages: usize, count: usize },
    /// One record created before EVERY resumed page, so the sequence grows under the
    /// walk repeatedly rather than once.
    BeforeEveryPage,
}

/// One row of the table: a listing, how many records it holds when the walk starts, the
/// page size the walk uses, and when records are created during it.
#[derive(Clone, Copy, Debug)]
struct Row {
    listing: Listing,
    held: usize,
    max_results: usize,
    creation: Creation,
}

/// The rows of the table: both newest-first listings, three record counts, two page sizes
/// and four creation schedules.
fn rows() -> Vec<Row> {
    let mut rows = Vec::new();
    for listing in [Listing::SearchJobs, Listing::ExportJobs] {
        for held in [2_usize, 3, 5] {
            for max_results in [1_usize, 2] {
                for creation in [
                    Creation::Never,
                    Creation::Once {
                        after_pages: 1,
                        count: 1,
                    },
                    Creation::Once {
                        after_pages: 1,
                        count: 2,
                    },
                    Creation::Once {
                        after_pages: 2,
                        count: 1,
                    },
                    Creation::BeforeEveryPage,
                ] {
                    rows.push(Row {
                        listing,
                        held,
                        max_results,
                        creation,
                    });
                }
            }
        }
    }
    rows
}

/// One emulator, the listing a row pages, and every record that listing holds in the order
/// it was created — which is the reverse of the order the listing reports.
struct Fixture {
    emulator: MultiServiceRouter,
    listing: Listing,
    /// The settled search job the export rows export, minted once; empty for a row that
    /// pages the search-job listing.
    exportable: String,
    /// Every record of the paged listing, in creation order.
    created: Vec<String>,
}

impl Fixture {
    /// An emulator holding nothing of the paged listing yet.
    fn new(listing: Listing) -> Self {
        let (emulator, engine) = emulator_with_engine();
        let exportable = match listing {
            // An export can only be started from a search job whose results are final,
            // so the row seeds one settled job and exports it repeatedly.
            Listing::ExportJobs => seed_job(&engine, state::COMPLETED, Vec::new()),
            Listing::SearchJobs => String::new(),
        };
        Self {
            emulator,
            listing,
            exportable,
            created: Vec::new(),
        }
    }

    /// Append one record to the paged listing's held sequence, as a client does: a
    /// `StartSearchJob` or a `StartSearchResultExportJob`.
    async fn create_record(&mut self, case: &str) -> String {
        let index = self.created.len();
        let started = match self.listing {
            Listing::SearchJobs => {
                call(
                    &self.emulator,
                    REGION,
                    "PUT",
                    "/search-jobs",
                    Some(json!({
                        "Name": format!("job-{index}"),
                        "SearchScope": {"BackupResourceTypes": ["S3"]}
                    })),
                )
                .await
            }
            Listing::ExportJobs => {
                call(
                    &self.emulator,
                    REGION,
                    "PUT",
                    "/export-search-jobs",
                    Some(json!({
                        "SearchJobIdentifier": self.exportable,
                        "ExportSpecification": {"s3ExportSpecification": {
                            "DestinationBucket": format!("exports-{index}")
                        }},
                        "RoleArn": passable_role("export")
                    })),
                )
                .await
            }
        };
        assert_eq!(
            started.status, 200,
            "case {case:?}: starting a record must succeed to set up the row, got {} ({:?})",
            started.status, started.body
        );
        let identifier = started.string(self.listing.identifier_member());
        assert!(
            !identifier.is_empty(),
            "case {case:?}: a started record must be named, got {:?}",
            started.body
        );
        self.created.push(identifier.clone());
        identifier
    }

    /// One page of the listing, `token` resuming it when given.
    async fn page(&self, max_results: usize, token: Option<&str>) -> Answer {
        let uri = match token {
            Some(token) => format!(
                "{}?MaxResults={max_results}&NextToken={token}",
                self.listing.path()
            ),
            None => format!("{}?MaxResults={max_results}", self.listing.path()),
        };
        call(&self.emulator, REGION, "GET", &uri, None).await
    }

    /// Every record the listing holds, NEWEST-CREATED FIRST: the order it reports them in.
    fn newest_first(&self) -> Vec<String> {
        self.created.iter().rev().cloned().collect()
    }
}

/// Assert the invariants that hold of EVERY page of a listing, and answer the identifiers
/// it carried and the token it handed out.
fn page_invariants(answer: &Answer, row: Row, case: &str) -> (Vec<String>, Option<String>) {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: a page of a listing must be answered, got {} ({:?})",
        answer.status, answer.body
    );
    let entries: Vec<String> = answer
        .get(row.listing.page_member())
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|summary| {
            summary[row.listing.identifier_member()]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    assert!(
        entries.len() <= row.max_results,
        "case {case:?}: a page must hold at most MaxResults entries, got {}",
        entries.len()
    );
    let token = answer.get("NextToken").as_str().map(ToString::to_string);
    assert_eq!(
        token.is_some(),
        entries.len() == row.max_results,
        "case {case:?}: a page of {} entries must {}carry a NextToken, got {:?}",
        entries.len(),
        if entries.len() == row.max_results {
            ""
        } else {
            "not "
        },
        answer.get("NextToken")
    );
    (entries, token)
}

/// Follow one walk of the fixture's listing to exhaustion at `row`'s page size, creating
/// records at the point `creation` names, and answer what the walk reported together with
/// what was created during it.
async fn walk(
    fixture: &mut Fixture,
    row: Row,
    creation: Creation,
    case: &str,
) -> (Vec<String>, Vec<String>) {
    let mut reported: Vec<String> = Vec::new();
    let mut created: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0_usize;
    loop {
        // Appended between two pages of the walk, exactly as a concurrent caller does.
        if pages > 0 {
            match creation {
                Creation::Once { after_pages, count } if pages == after_pages => {
                    for _ in 0..count {
                        created.push(fixture.create_record(case).await);
                    }
                }
                Creation::BeforeEveryPage => created.push(fixture.create_record(case).await),
                Creation::Never | Creation::Once { .. } => {}
            }
        }

        let answer = fixture.page(row.max_results, token.as_deref()).await;
        assert_ne!(
            answer.error_code.as_deref(),
            Some("ValidationException"),
            "case {case:?}: a token this service issued must not be refused because the sequence grew, got {:?}",
            answer.body
        );
        let (entries, next) = page_invariants(&answer, row, case);
        for identifier in entries {
            assert!(
                !reported.contains(&identifier),
                "case {case:?}: one walk must report a record at most once, saw {identifier:?} twice (reported so far {reported:?})"
            );
            reported.push(identifier);
        }

        pages += 1;
        let bound = fixture.created.len() + 2;
        assert!(
            pages <= bound,
            "case {case:?}: paging must terminate, took {pages} pages over {} records",
            fixture.created.len()
        );
        match next {
            Some(next) => token = Some(next),
            None => break,
        }
    }
    (reported, created)
}

#[tokio::test]
async fn a_walk_whose_sequence_grows_reports_every_record_held_throughout_it_exactly_once() {
    for row in rows() {
        let case = format!(
            "{:?}, {} held, MaxResults={}, created {:?}",
            row.listing, row.held, row.max_results, row.creation
        );
        let mut fixture = Fixture::new(row.listing);
        for _ in 0..row.held {
            fixture.create_record(&case).await;
        }
        let held_newest_first = fixture.newest_first();

        let (reported, created) = walk(&mut fixture, row, row.creation, &case).await;

        assert_eq!(
            reported, held_newest_first,
            "case {case:?}: the walk must report every record held throughout it exactly once, newest-created first, got {reported:?}"
        );
        for identifier in &created {
            assert!(
                !reported.contains(identifier),
                "case {case:?}: a record created after the walk passed the head of the sequence belongs to the next walk, got {identifier:?} in {reported:?}"
            );
        }

        // The control: the records created mid-walk really are held, and a FRESH walk
        // reports every one of them, at the head of the sequence, exactly once.
        let (fresh, _) = walk(&mut fixture, row, Creation::Never, &case).await;
        assert_eq!(
            fresh,
            fixture.newest_first(),
            "case {case:?}: a fresh walk must report every held record exactly once, newest-created first, got {fresh:?}"
        );
        for identifier in &created {
            assert!(
                fresh.contains(identifier),
                "case {case:?}: a record created during the previous walk must be reported by the next one, missing {identifier:?} from {fresh:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_walk_of_a_sequence_reported_in_creation_order_is_unmoved_by_records_appended_to_the_store(
) {
    // The rule's other arm: `ListSearchJobBackups` reports a job's covered recovery points
    // in the order the record holds them — an order appended to at the END — so its token
    // counts from the front, and a walk of it keeps reporting each covered backup exactly
    // once in that order while the store grows under it.
    for backup_count in [1_usize, 3, 7] {
        for max_results in [1_usize, 2, 3] {
            let case = format!("{backup_count} backups, MaxResults={max_results}");
            let (emulator, engine) = emulator_with_engine();
            let seeded = (0..backup_count)
                .map(|index| {
                    recovery_point(
                        "S3",
                        &format!(
                            "arn:aws:backup:{REGION}:{}:recovery-point:rp-{index}",
                            engine::emulator_account_id()
                        ),
                        &format!("arn:aws:s3:::bucket-{index}"),
                        SystemTime::now(),
                    )
                })
                .collect::<Vec<_>>();
            let identifier = seed_job(&engine, state::COMPLETED, seeded.clone());

            let mut seen: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0_usize;
            loop {
                // The store grows between two pages: another search job is started, which
                // is what appends to this service's creation orders.
                if pages > 0 {
                    let started = call(
                        &emulator,
                        REGION,
                        "PUT",
                        "/search-jobs",
                        Some(json!({"SearchScope": {"BackupResourceTypes": ["S3"]}})),
                    )
                    .await;
                    assert_eq!(
                        started.status, 200,
                        "case {case:?}: StartSearchJob must succeed mid-walk, got {} ({:?})",
                        started.status, started.body
                    );
                }

                let uri = match &token {
                    Some(token) => format!(
                        "/search-jobs/{identifier}/backups?maxResults={max_results}&nextToken={token}"
                    ),
                    None => format!("/search-jobs/{identifier}/backups?maxResults={max_results}"),
                };
                let listed = call(&emulator, REGION, "GET", &uri, None).await;
                assert_eq!(
                    listed.status, 200,
                    "case {case:?}: a page of the backups listing must be answered, got {} ({:?})",
                    listed.status, listed.body
                );
                let page = listed.get("Results").as_array().cloned().unwrap_or_default();
                assert!(
                    page.len() <= max_results,
                    "case {case:?}: a page must hold at most MaxResults backups, got {}",
                    page.len()
                );
                for entry in &page {
                    let arn = entry["BackupResourceArn"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string();
                    assert!(
                        !seen.contains(&arn),
                        "case {case:?}: one walk must report a backup at most once, saw {arn:?} twice"
                    );
                    seen.push(arn);
                }
                pages += 1;
                assert!(
                    pages <= backup_count + 2,
                    "case {case:?}: paging must terminate, took {pages} pages"
                );
                token = listed.get("NextToken").as_str().map(ToString::to_string);
                if token.is_none() {
                    break;
                }
            }

            let expected: Vec<String> = seeded
                .iter()
                .map(|entry| entry.backup_resource_arn.clone().unwrap_or_default())
                .collect();
            assert_eq!(
                seen, expected,
                "case {case:?}: the walk must report every covered backup exactly once in the order the record holds them, got {seen:?}"
            );
        }
    }
}
