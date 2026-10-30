// SPDX-License-Identifier: CC0-1.0

//! A continuation token's PAYLOAD is honoured only when it is spelled the way this
//! service writes one.
//!
//! RULE: a `nextToken` is this service's own opaque handle on a position in one
//! collection, so its decoded payload is read back only when every part of it is spelled
//! as the service writes it — each number a plain run of ASCII digits inside the range
//! that part can hold, and the identifier that breaks a sort-key tie one of the
//! identifiers the service MINTS (32 lowercase hex characters). A payload carrying
//! anything else is a position this service never handed out and is refused with
//! `ValidationException` (HTTP 400) and the service's fixed invalid-token sentence,
//! never honoured as a resume point: a resume at a key no record holds walks past the
//! record the real token named, which the caller would then never see.
//!
//! Every paginated listing whose token carries a tie-breaking identifier is covered:
//! `ListDatastores` and `ListDICOMImportJobs` (whose tokens carry an instant and an id)
//! and `SearchImageSets` (whose token carries the caller-chosen order's key and an id).
//! `ListImageSetVersions` pages by an ordinal alone, which has no identifier half.
//!
//! The suites are TABLES over the two axes the rule generalises over — the identifier
//! (empty, each width around the minted one, each character class outside the minted
//! alphabet) and the key (sign, padding, non-numeric text, a sub-second part at and past
//! the values one can hold, a missing separator) — and every row is built by editing the
//! payload of a token the service really ISSUED, with its collection marker read back off
//! that token rather than spelled again here. The controls are the unedited token, which
//! must still resume the walk exactly, and the full paged walk, which must still visit
//! every record once.
//!
//! Every case drives real HTTP through the production front door
//! (`crate::datastore_lifecycle`'s harness); nothing is asserted through the state struct.

use serde_json::json;

use service_com_amazonaws_medicalimaging_ahigatewayservice::state::INVALID_NEXT_TOKEN_MESSAGE;

use crate::datastore_lifecycle::{Answer, Emulator};
use crate::image_set_copy::seed_image_set_in_hierarchy;
use crate::image_set_search::search;
use crate::import_job_listing::list_jobs;

/// A `RoleArn` the import-job submits below use (len 20..2048,
/// `^arn:aws(-[^:]+)?:iam::[0-9]{12}:role/.+$`).
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/ImportRole";

/// How many records each covered collection holds: enough that a one-per-page walk mints
/// a token with records still ahead of it, so honouring a forged position could skip one.
const RECORDS: usize = 3;

/// The three parts of a token this service ISSUED: the collection marker, the sort key,
/// and the identifier that broke the key's tie.
///
/// Read back off a real token rather than spelled again here: a marker names the
/// collection (the listing, its scope, the order and the filter), so a literal copy of one
/// rots the moment any of those changes and leaves a case refused at the marker check
/// instead of at the payload it is about. A payload is `{marker}{key}:{identifier}`, and
/// neither an identifier this service mints nor any key it writes holds a `:`, so the last
/// two `:` are what separate the three parts.
pub(crate) fn split_issued_token(case: &str, token: &str) -> (String, String, String) {
    let bytes = ::hex::decode(token).unwrap_or_else(|error| {
        panic!("case {case:?}: expected hex from this service, got {token:?} ({error})")
    });
    let decoded = String::from_utf8(bytes).unwrap_or_else(|error| {
        panic!("case {case:?}: expected UTF-8 under the hex of {token:?}, got {error}")
    });
    let (head, identifier) = decoded.rsplit_once(':').unwrap_or_else(|| {
        panic!("case {case:?}: expected an identifier in the payload {decoded:?}")
    });
    let (marker, key) = head.rsplit_once(':').unwrap_or_else(|| {
        panic!("case {case:?}: expected a marked sort key in the payload {decoded:?}")
    });
    (
        format!("{marker}:"),
        key.to_string(),
        identifier.to_string(),
    )
}

/// A token carrying `payload` under the collection `marker`, spelled the way this service
/// spells one: hex over the marked payload.
pub(crate) fn token_carrying(marker: &str, payload: &str) -> String {
    ::hex::encode(format!("{marker}{payload}"))
}

/// Every forged payload one collection's table replays, built from the sort `key` and the
/// `identifier` of a token this service really issued.
///
/// The identifier rows span the width and the character class of the form this service
/// mints an identifier in — 32 lowercase hex characters, which is what every
/// `datastoreId`, `jobId` and `imageSetId` this emulator hands out is made of — including
/// a `[0-9a-z]{32}` text that a request may legally NAME a resource by but that no call
/// ever minted. The key rows span how a number can be written other than as this service
/// writes it, and the sub-second half of an instant at and past the values one second's
/// remainder can hold.
fn forged_payloads(case: &str, key: &str, identifier: &str) -> Vec<(String, String)> {
    let (secs, nanos) = key.split_once('.').unwrap_or_else(|| {
        panic!("case {case:?}: expected an instant-shaped sort key, got {key:?}")
    });
    // One minted-looking identifier, written out so a row does not depend on which
    // characters the issued identifier happens to hold.
    let minted = "0123456789abcdef0123456789abcdef";
    let mut rows: Vec<(String, String)> = Vec::new();

    // ---- the identifier axis: the key stays the one the service issued ----
    for (case, forged) in [
        // The id the issue's walk forged: inside `^[0-9a-z]{32}$`, the form a REQUEST may
        // name a resource by, but outside the hex form the service mints one in — and it
        // sorts above every minted id, so honouring it would skip the record the real
        // token named.
        ("an identifier outside the minted alphabet", "z".repeat(32)),
        (
            "an identifier with one character outside the minted alphabet",
            format!("{}z", &minted[..31]),
        ),
        ("an empty identifier", String::new()),
        (
            "an identifier one character short of the minted width",
            minted[..31].to_string(),
        ),
        (
            "an identifier one character past the minted width",
            format!("{minted}0"),
        ),
        ("an uppercase identifier", minted.to_uppercase()),
        (
            "an identifier carrying a hyphen",
            format!("{}-{}", &minted[..20], &minted[21..]),
        ),
        (
            "an identifier carrying a dot",
            format!("{}.{}", &minted[..20], &minted[21..]),
        ),
    ] {
        rows.push((case.to_string(), format!("{key}:{forged}")));
    }

    // ---- the key axis: the identifier stays the one the service issued ----
    for (case, forged) in [
        ("a signed number of seconds", format!("+{secs}.{nanos}")),
        ("seconds padded with a space", format!(" {secs}.{nanos}")),
        (
            "seconds that are not a number at all",
            format!("notanumber.{nanos}"),
        ),
        ("no seconds at all", format!(".{nanos}")),
        ("signed nanoseconds", format!("{secs}.+{nanos}")),
        ("no nanoseconds at all", format!("{secs}.")),
        (
            "a whole second of nanoseconds",
            format!("{secs}.1000000000"),
        ),
        (
            "more nanoseconds than the width they are written in holds",
            format!("{secs}.4294967296"),
        ),
        ("no fraction separator", secs.to_string()),
        ("an empty key", String::new()),
    ] {
        rows.push((case.to_string(), format!("{forged}:{identifier}")));
    }

    // A payload with no identifier half at all: the key alone, which names no position
    // either listing's walk can break a tie with.
    rows.push(("no identifier half at all".to_string(), key.to_string()));
    rows
}

/// One collection this suite pages, with everything its listing needs to be driven.
enum Listing {
    /// `ListDatastores` over the region the harness signs for.
    Datastores,
    /// `ListDICOMImportJobs` over one data store's import jobs.
    ImportJobs(String),
    /// `SearchImageSets` over one data store's image sets, under the default sort.
    ImageSets(String),
}

impl Listing {
    /// How a case names this collection.
    fn label(&self) -> &'static str {
        match self {
            Self::Datastores => "ListDatastores",
            Self::ImportJobs(_) => "ListDICOMImportJobs",
            Self::ImageSets(_) => "SearchImageSets",
        }
    }

    /// The output member holding this listing's page.
    fn list_member(&self) -> &'static str {
        match self {
            Self::Datastores => "datastoreSummaries",
            Self::ImportJobs(_) => "jobSummaries",
            Self::ImageSets(_) => "imageSetsMetadataSummaries",
        }
    }

    /// The member each summary names its record by.
    fn id_member(&self) -> &'static str {
        match self {
            Self::Datastores => "datastoreId",
            Self::ImportJobs(_) => "jobId",
            Self::ImageSets(_) => "imageSetId",
        }
    }

    /// One page of this listing, with `query` as the request's query string.
    async fn page(&self, emulator: &Emulator, query: &str) -> Answer {
        match self {
            Self::Datastores => emulator.list_datastores(query).await,
            Self::ImportJobs(datastore_id) => list_jobs(emulator, datastore_id, query).await,
            Self::ImageSets(datastore_id) => search(emulator, datastore_id, None, query).await,
        }
    }

    /// The identifiers one answer reports, in the order it served them.
    fn reported_ids(&self, answer: &Answer) -> Vec<String> {
        answer
            .field(&[self.list_member()])
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .map(|item| {
                        item[self.id_member()]
                            .as_str()
                            .unwrap_or_default()
                            .to_string()
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// An emulator holding every collection this suite pages, and the listings over them.
async fn seeded() -> (Emulator, Vec<Listing>) {
    let emulator = Emulator::new();

    // The data stores themselves are one collection; two of them also hold the import
    // jobs and the image sets the other two listings walk.
    let mut stores: Vec<String> = Vec::new();
    for index in 0..RECORDS {
        stores.push(
            emulator
                .active_datastore(
                    &format!("token.payload.{index}"),
                    &format!("token-payload-{index}"),
                )
                .await,
        );
    }

    let jobs_store = stores[0].clone();
    for index in 0..RECORDS {
        let started = emulator
            .start_import_job(
                &jobs_store,
                json!({
                    "clientToken": format!("token-payload-job-{index}"),
                    "dataAccessRoleArn": ROLE_ARN,
                    "inputS3Uri": "s3://imaging-in/",
                    "outputS3Uri": "s3://imaging-out/",
                }),
            )
            .await;
        assert_eq!(
            started.status, 200,
            "seeding import job {index}: expected HTTP 200, got {started:?}"
        );
    }

    let image_sets_store = stores[1].clone();
    for _ in 0..RECORDS {
        seed_image_set_in_hierarchy(&emulator, &image_sets_store, 1, true);
    }

    (
        emulator,
        vec![
            Listing::Datastores,
            Listing::ImportJobs(jobs_store),
            Listing::ImageSets(image_sets_store),
        ],
    )
}

/// Assert that `answer` is this service's refusal of a token it did not issue: HTTP 400,
/// `ValidationException`, the fixed sentence, and no page beside it.
fn assert_refused(case: &str, listing: &Listing, answer: &Answer) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected HTTP 400 for a payload this service never wrote, got {answer:?}"
    );
    assert_eq!(
        answer.error_code, "ValidationException",
        "case {case:?}: expected ValidationException, got {answer:?}"
    );
    assert_eq!(
        answer.string(&["message"]),
        INVALID_NEXT_TOKEN_MESSAGE,
        "case {case:?}: expected the service's fixed invalid-token sentence, got {answer:?}"
    );
    assert!(
        answer.field(&[listing.list_member()]).is_null(),
        "case {case:?}: expected no page beside the refusal, got {answer:?}"
    );
}

/// RULE: a continuation token's payload is honoured only when it is spelled as this
/// service writes one — every number a plain run of ASCII digits inside the range that
/// part holds, the tie-breaking identifier one of the 32-hex-character identifiers the
/// service mints — so a token whose payload carries anything else is refused with
/// `ValidationException` (HTTP 400) and the fixed invalid-token sentence rather than
/// resumed from, which is what stops a walk from being sent past a record it never served.
#[tokio::test]
async fn a_forged_token_payload_is_refused_rather_than_resumed_from() {
    let (emulator, listings) = seeded().await;

    for listing in &listings {
        let label = listing.label();

        // The collection in one page, which every walk below is compared against.
        let whole = listing.page(&emulator, "maxResults=50").await;
        assert_eq!(
            whole.status, 200,
            "case {label:?}: expected HTTP 200 from an unpaged listing, got {whole:?}"
        );
        let records = listing.reported_ids(&whole);
        assert!(
            records.len() >= 2,
            "case {label:?}: expected at least two records to page over, got {records:?}"
        );

        // A token this service really issued, at the boundary after the first record.
        let first_page = listing.page(&emulator, "maxResults=1").await;
        assert_eq!(
            first_page.status, 200,
            "case {label:?}: expected HTTP 200 from the first page, got {first_page:?}"
        );
        let issued = first_page
            .field(&["nextToken"])
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {label:?}: expected a nextToken on a one-per-page listing, got {first_page:?}")
            })
            .to_string();

        let (marker, key, identifier) = split_issued_token(label, &issued);
        assert_eq!(
            identifier, records[1],
            "case {label:?}: expected the issued token to name the next record, got {identifier:?}"
        );

        for (row, payload) in forged_payloads(label, &key, &identifier) {
            let case = format!("{label}: {row}");
            let forged = token_carrying(&marker, &payload);
            assert_ne!(
                forged, issued,
                "case {case:?}: the forged token must differ from the issued one"
            );

            let answer = listing
                .page(&emulator, &format!("maxResults=1&nextToken={forged}"))
                .await;
            assert_refused(&case, listing, &answer);
        }

        // The controls. The unedited token still resumes the walk at the record after the
        // boundary, so the refusals above are about the forged payloads and not about
        // paging...
        let resumed = listing
            .page(&emulator, &format!("maxResults=1&nextToken={issued}"))
            .await;
        assert_eq!(
            resumed.status, 200,
            "case {label:?}: expected the issued token to be honoured, got {resumed:?}"
        );
        assert_eq!(
            listing.reported_ids(&resumed),
            vec![records[1].clone()],
            "case {label:?}: expected the resume to serve the record after the boundary, got {resumed:?}"
        );

        // ...and the record the forged payloads tried to skip is still served by a walk
        // that pages with the tokens this service mints, exactly once.
        let mut walked: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0_usize;
        loop {
            let mut query = "maxResults=1".to_string();
            if let Some(token) = &next_token {
                query.push_str(&format!("&nextToken={token}"));
            }
            let page = listing.page(&emulator, &query).await;
            assert_eq!(
                page.status, 200,
                "case {label:?} page {pages}: expected HTTP 200, got {page:?}"
            );
            walked.extend(listing.reported_ids(&page));
            pages += 1;
            assert!(
                pages <= records.len(),
                "case {label:?}: paged further than the collection can hold, got {walked:?}"
            );
            match page.field(&["nextToken"]).as_str() {
                Some(token) => next_token = Some(token.to_string()),
                None => break,
            }
        }
        assert_eq!(
            walked, records,
            "case {label:?}: expected the paged walk to visit every record exactly once in \
             order, got {walked:?}"
        );
    }
}
