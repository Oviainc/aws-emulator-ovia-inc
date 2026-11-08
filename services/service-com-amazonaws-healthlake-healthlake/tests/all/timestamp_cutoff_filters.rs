// SPDX-License-Identifier: CC0-1.0

//! How every listing of this service reads the pair of timestamp cutoff members
//! its request carries.
//!
//! The rule under test: a listing applies a timestamp cutoff member exactly as
//! that member's own documentation words it — a member that selects records
//! "before" / "after" the instant it names is a STRICT bound, so a record at
//! exactly that instant is reported by neither, while a member that selects
//! records "at or before" / "at or after" it is an INCLUSIVE bound, so a record
//! at exactly that instant is reported by both.
//!
//! The reading each listing below is held to is its OWN modelled member
//! documentation, read with
//! `smithy_query(query="shape", shape="<the request or filter shape>")`:
//!
//! * `ListFHIRExportJobs` — "Limits the response to FHIR export jobs submitted
//!   before a user- specified date." / "... submitted after a user-specified
//!   date." — STRICT;
//! * `ListFHIRImportJobs` — "Limits the response to FHIR import jobs submitted
//!   before a user- specified date." / "... submitted after a user-specified
//!   date." — STRICT;
//! * `ListFHIRDatastores` (`DatastoreFilter`) — "All data stores created before
//!   the specified date are included in the results." / "... created after the
//!   specified date ..." — STRICT;
//! * `ListDataTransformationJobs` — "Filters the results to include only jobs
//!   submitted at or before this timestamp." / "... at or after this
//!   timestamp." — INCLUSIVE.
//!
//! Every call goes over the real awsJson1_0 wire through
//! [`MultiServiceRouter::dispatch`](smithy_server_runtime::MultiServiceRouter)
//! by way of the harness in `fhir_datastore_lifecycle`, so routing,
//! deserialization and the modelled constraints are all exercised, not
//! bypassed.

use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use service_com_amazonaws_healthlake_healthlake::ops::types::{
    DataTransformationS3Configuration, InputDataConfig, OutputDataConfig, S3Configuration,
    TransformationInputDataConfig, TransformationOutputDataConfig,
};
use service_com_amazonaws_healthlake_healthlake::state::{
    ComAmazonawsHealthlakeHealthlakeState, ExportJob, ImportJob, JobStatus, TransformationJob,
    TransformationJobStatus,
};

use crate::fhir_datastore_lifecycle::HealthLake;

/// The epoch second every placed instant below is measured from. A fixed
/// instant, so a cutoff can be exactly the instant a record carries rather than
/// something near it.
const BASE_SECS: i64 = 1_700_000_000;

/// The instants the records of each listing are placed at, as `(seconds after
/// BASE_SECS, nanoseconds)`. Two records share a whole second and differ only
/// below it, so a cutoff is exercised against SUB-SECOND precision as well as
/// against whole seconds.
const PLACED: [(i64, u32); 4] = [(10, 0), (20, 0), (20, 250_000_000), (30, 0)];

/// The `DataAccessRoleArn` every placed job carries, satisfying the modelled
/// IAM-role patterns (`^arn:aws(-[^:]+)?:iam::[0-9]{12}:role/.+$`).
const DATA_ACCESS_ROLE_ARN: &str = "arn:aws:iam::111122223333:role/HealthLakeCutoffs";

/// A `KmsKeyId` the modelled `EncryptionKeyID` pattern admits.
const KMS_KEY_ID: &str = "1234abcd-12ab-34cd-56ef-1234567890ab";

/// One of the placed instants, as the modelled timestamp a record holds.
fn placed_at(index: usize) -> DateTime {
    let (secs, nanos) = PLACED[index];
    DateTime::from_secs_and_nanos(BASE_SECS + secs, nanos)
}

/// A cutoff value as the wire carries a `timestamp` member under awsJson1_0:
/// epoch seconds, fractional where the instant is not a whole second.
fn cutoff(secs: i64, nanos: u32) -> Value {
    json!((BASE_SECS + secs) as f64 + f64::from(nanos) / 1e9)
}

/// The records one listing was placed with: their identifiers in [`PLACED`]
/// order, beside the scope its requests must name.
struct Placed {
    ids: Vec<String>,
    /// The `DatastoreId` the placed records belong to, empty for a listing that
    /// is not scoped to one.
    scope: String,
}

/// The listings whose requests carry a pair of timestamp cutoff members.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Listing {
    ExportJobs,
    ImportJobs,
    Datastores,
    TransformationJobs,
}

impl Listing {
    const ALL: [Listing; 4] = [
        Listing::ExportJobs,
        Listing::ImportJobs,
        Listing::Datastores,
        Listing::TransformationJobs,
    ];

    fn operation(self) -> &'static str {
        match self {
            Listing::ExportJobs => "ListFHIRExportJobs",
            Listing::ImportJobs => "ListFHIRImportJobs",
            Listing::Datastores => "ListFHIRDatastores",
            Listing::TransformationJobs => "ListDataTransformationJobs",
        }
    }

    /// The member that bounds the listing on the LATE side, and the one that
    /// bounds it on the EARLY side.
    fn members(self) -> (&'static str, &'static str) {
        match self {
            Listing::Datastores => ("CreatedBefore", "CreatedAfter"),
            _ => ("SubmittedBefore", "SubmittedAfter"),
        }
    }

    /// Whether this listing's OWN member documentation spells the inclusive
    /// reading ("at or before" / "at or after") rather than the strict one
    /// ("before" / "after"); this module's header quotes each.
    fn inclusive(self) -> bool {
        self == Listing::TransformationJobs
    }

    /// Place one record of this listing's resource at each instant in
    /// [`PLACED`].
    async fn place(self, service: &HealthLake) -> Placed {
        match self {
            Listing::ExportJobs => {
                let scope = datastore(service, "export cutoffs").await;
                let ids = record_ids("export");
                let store = service
                    .engine
                    .state::<ComAmazonawsHealthlakeHealthlakeState>();
                let mut store = store.lock();
                for (index, id) in ids.iter().enumerate() {
                    store.export_jobs.insert(
                        id.clone(),
                        ExportJob {
                            id: id.clone(),
                            datastore_id: scope.clone(),
                            name: None,
                            status: JobStatus::Completed,
                            pending: None,
                            submitted_at: placed_at(index),
                            ended_at: None,
                            output_data_config: OutputDataConfig::S3Configuration(
                                S3Configuration {
                                    s3_uri: format!("s3://healthlake-export-bucket/{id}"),
                                    kms_key_id: KMS_KEY_ID.to_string(),
                                },
                            ),
                            data_access_role_arn: Some(DATA_ACCESS_ROLE_ARN.to_string()),
                            message: None,
                            client_token: None,
                            request_fingerprint: String::new(),
                        },
                    );
                }
                Placed { ids, scope }
            }
            Listing::ImportJobs => {
                let scope = datastore(service, "import cutoffs").await;
                let ids = record_ids("import");
                let store = service
                    .engine
                    .state::<ComAmazonawsHealthlakeHealthlakeState>();
                let mut store = store.lock();
                for (index, id) in ids.iter().enumerate() {
                    store.import_jobs.insert(
                        id.clone(),
                        ImportJob {
                            id: id.clone(),
                            datastore_id: scope.clone(),
                            name: None,
                            status: JobStatus::Completed,
                            pending: None,
                            submitted_at: placed_at(index),
                            ended_at: None,
                            input_data_config: InputDataConfig::S3Uri(format!(
                                "s3://healthlake-import-bucket/input/{id}"
                            )),
                            output_data_config: Some(OutputDataConfig::S3Configuration(
                                S3Configuration {
                                    s3_uri: format!("s3://healthlake-import-bucket/output/{id}"),
                                    kms_key_id: KMS_KEY_ID.to_string(),
                                },
                            )),
                            data_access_role_arn: Some(DATA_ACCESS_ROLE_ARN.to_string()),
                            message: None,
                            progress_report: None,
                            validation_level: None,
                            // A record this suite seeds was not started through
                            // the API, so it carries no `ClientToken` and there
                            // is no request to fingerprint.
                            client_token: None,
                            request_fingerprint: String::new(),
                        },
                    );
                }
                Placed { ids, scope }
            }
            Listing::TransformationJobs => {
                let ids = record_ids("transform");
                let store = service
                    .engine
                    .state::<ComAmazonawsHealthlakeHealthlakeState>();
                let mut store = store.lock();
                for (index, id) in ids.iter().enumerate() {
                    store.transformation_jobs.insert(
                        id.clone(),
                        TransformationJob {
                            id: id.clone(),
                            name: None,
                            status: TransformationJobStatus::Completed,
                            pending: None,
                            submitted_at: placed_at(index),
                            ended_at: None,
                            input_data_config: TransformationInputDataConfig {
                                s3_uri: "s3://healthlake-transform-input/ccda".to_string(),
                                source_format: None,
                            },
                            output_data_config: TransformationOutputDataConfig {
                                s3_configuration: DataTransformationS3Configuration {
                                    s3_uri: "s3://healthlake-transform-output/fhir".to_string(),
                                    kms_key_id: KMS_KEY_ID.to_string(),
                                },
                            },
                            data_access_role_arn: DATA_ACCESS_ROLE_ARN.to_string(),
                            profile_id: None,
                            profile_name: None,
                            profile_version: None,
                            drift_detection_enabled: None,
                            provenance_enabled: None,
                            message: None,
                            progress_report: None,
                            client_token: None,
                        },
                    );
                }
                Placed {
                    ids,
                    scope: String::new(),
                }
            }
            Listing::Datastores => {
                // A data store is MINTED through its own operation — nothing
                // here writes a record the create path would not — and only its
                // creation instant, the member the cutoffs read, is then
                // placed.
                let mut ids = Vec::new();
                for index in 0..PLACED.len() {
                    ids.push(datastore(service, &format!("cutoff store {index}")).await);
                }
                let store = service
                    .engine
                    .state::<ComAmazonawsHealthlakeHealthlakeState>();
                let mut store = store.lock();
                for (index, id) in ids.iter().enumerate() {
                    let placed = store
                        .datastores
                        .get_mut(id)
                        .unwrap_or_else(|| panic!("the created data store {id} is held"));
                    placed.created_at = placed_at(index);
                }
                Placed {
                    ids,
                    scope: String::new(),
                }
            }
        }
    }

    /// One request of this listing, carrying each cutoff that is set and scoped
    /// to `scope` where the listing requires a scope.
    fn request(self, scope: &str, before: Option<Value>, after: Option<Value>) -> Value {
        let (before_member, after_member) = self.members();
        let mut cutoffs = json!({});
        if let Some(before) = before {
            cutoffs[before_member] = before;
        }
        if let Some(after) = after {
            cutoffs[after_member] = after;
        }
        match self {
            Listing::Datastores => {
                let mut request = json!({});
                if cutoffs.as_object().is_some_and(|set| !set.is_empty()) {
                    request["Filter"] = cutoffs;
                }
                request
            }
            Listing::TransformationJobs => cutoffs,
            Listing::ExportJobs | Listing::ImportJobs => {
                let mut request = cutoffs;
                request["DatastoreId"] = json!(scope);
                request
            }
        }
    }

    /// The identifiers one response of this listing reported, sorted.
    fn reported(self, payload: &Value) -> Vec<String> {
        let (list_member, id_member) = match self {
            Listing::ExportJobs => ("ExportJobPropertiesList", "JobId"),
            Listing::ImportJobs => ("ImportJobPropertiesList", "JobId"),
            Listing::Datastores => ("DatastorePropertiesList", "DatastoreId"),
            Listing::TransformationJobs => ("Items", "JobId"),
        };
        let mut reported: Vec<String> = payload[list_member]
            .as_array()
            .unwrap_or_else(|| panic!("{list_member} must be a list, got {payload}"))
            .iter()
            .map(|entry| {
                entry[id_member]
                    .as_str()
                    .unwrap_or_else(|| panic!("every entry carries a {id_member}, got {entry}"))
                    .to_string()
            })
            .collect();
        reported.sort();
        reported
    }
}

/// The identifiers of the placed records of one job listing, one per entry of
/// [`PLACED`].
fn record_ids(prefix: &str) -> Vec<String> {
    (0..PLACED.len())
        .map(|index| format!("{prefix}-cutoff-{index}"))
        .collect()
}

/// Create a data store through its own operation and return its `DatastoreId`.
async fn datastore(service: &HealthLake, name: &str) -> String {
    let (id, _arn) = service
        .create(&json!({ "DatastoreTypeVersion": "R4", "DatastoreName": name }))
        .await;
    id
}

/// The identifiers at `indices` of `ids`, sorted as a response is compared.
fn expected(ids: &[String], indices: &[usize]) -> Vec<String> {
    let mut expected: Vec<String> = indices.iter().map(|index| ids[*index].clone()).collect();
    expected.sort();
    expected
}

/// One row of the cutoff table: the cutoffs a request carries, and which placed
/// records each reading of the pair selects.
struct Row {
    case: &'static str,
    before: Option<Value>,
    after: Option<Value>,
    /// The record indices a STRICT reading of the pair reports.
    strict: &'static [usize],
    /// The record indices an INCLUSIVE reading reports.
    inclusive: &'static [usize],
}

/// The rows the table spans. The records sit at [`PLACED`], so the rows differ
/// along which member is set, whether a cutoff falls ON a record instant,
/// between two of them or outside every one, whether it is a whole second or a
/// fraction of one, and whether the two members together bound a window, a
/// single instant or nothing at all.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "neither member set",
            before: None,
            after: None,
            strict: &[0, 1, 2, 3],
            inclusive: &[0, 1, 2, 3],
        },
        Row {
            case: "the late bound exactly on a record instant",
            before: Some(cutoff(20, 0)),
            after: None,
            strict: &[0],
            inclusive: &[0, 1],
        },
        Row {
            case: "the late bound exactly on a sub-second record instant",
            before: Some(cutoff(20, 250_000_000)),
            after: None,
            strict: &[0, 1],
            inclusive: &[0, 1, 2],
        },
        Row {
            case: "the late bound past a sub-second record instant",
            before: Some(cutoff(20, 500_000_000)),
            after: None,
            strict: &[0, 1, 2],
            inclusive: &[0, 1, 2],
        },
        Row {
            case: "the late bound on the earliest record instant",
            before: Some(cutoff(10, 0)),
            after: None,
            strict: &[],
            inclusive: &[0],
        },
        Row {
            case: "the late bound before every record",
            before: Some(cutoff(5, 0)),
            after: None,
            strict: &[],
            inclusive: &[],
        },
        Row {
            case: "the early bound exactly on a record instant",
            before: None,
            after: Some(cutoff(20, 0)),
            strict: &[2, 3],
            inclusive: &[1, 2, 3],
        },
        Row {
            case: "the early bound exactly on a sub-second record instant",
            before: None,
            after: Some(cutoff(20, 250_000_000)),
            strict: &[3],
            inclusive: &[2, 3],
        },
        Row {
            case: "the early bound short of a record instant",
            before: None,
            after: Some(cutoff(19, 750_000_000)),
            strict: &[1, 2, 3],
            inclusive: &[1, 2, 3],
        },
        Row {
            case: "the early bound on the latest record instant",
            before: None,
            after: Some(cutoff(30, 0)),
            strict: &[],
            inclusive: &[3],
        },
        Row {
            case: "the early bound past every record",
            before: None,
            after: Some(cutoff(35, 0)),
            strict: &[],
            inclusive: &[],
        },
        Row {
            case: "a window whose bounds are the extreme record instants",
            before: Some(cutoff(30, 0)),
            after: Some(cutoff(10, 0)),
            strict: &[1, 2],
            inclusive: &[0, 1, 2, 3],
        },
        Row {
            case: "both bounds on one record instant",
            before: Some(cutoff(20, 0)),
            after: Some(cutoff(20, 0)),
            strict: &[],
            inclusive: &[1],
        },
        Row {
            case: "a window around one record instant",
            before: Some(cutoff(20, 125_000_000)),
            after: Some(cutoff(19, 875_000_000)),
            strict: &[1],
            inclusive: &[1],
        },
        Row {
            case: "a window whose bounds are the wrong way round",
            before: Some(cutoff(10, 0)),
            after: Some(cutoff(30, 0)),
            strict: &[],
            inclusive: &[],
        },
    ]
}

#[tokio::test]
async fn every_listing_reads_its_cutoffs_as_its_own_documentation_words_them() {
    for listing in Listing::ALL {
        let service = HealthLake::new();
        let placed = listing.place(&service).await;

        for row in rows() {
            let request = listing.request(&placed.scope, row.before.clone(), row.after.clone());
            let payload = service.ok(listing.operation(), &request).await;
            let reported = listing.reported(&payload);
            let wanted = expected(
                &placed.ids,
                if listing.inclusive() {
                    row.inclusive
                } else {
                    row.strict
                },
            );
            assert_eq!(
                reported,
                wanted,
                "case {:?} of {}: the listing must report {wanted:?}, got {reported:?}",
                row.case,
                listing.operation()
            );
        }
    }
}

#[tokio::test]
async fn two_adjacent_cutoff_calls_agree_about_a_boundary_instant() {
    // The invariant: for ANY instant, the page bounded late by it and the page
    // bounded early by it cover every record exactly once under a strict
    // reading — a record at the instant itself is in neither page — while an
    // inclusive reading puts a record at that instant in BOTH pages.
    for listing in Listing::ALL {
        let service = HealthLake::new();
        let placed = listing.place(&service).await;

        for (index, instant) in PLACED.iter().enumerate() {
            let at = cutoff(instant.0, instant.1);
            let before = listing.reported(
                &service
                    .ok(
                        listing.operation(),
                        &listing.request(&placed.scope, Some(at.clone()), None),
                    )
                    .await,
            );
            let after = listing.reported(
                &service
                    .ok(
                        listing.operation(),
                        &listing.request(&placed.scope, None, Some(at.clone())),
                    )
                    .await,
            );
            // Every record sharing this instant — the records the two pages
            // have to agree about: both report them under an inclusive
            // reading, neither does under a strict one.
            let sharing: Vec<usize> = PLACED
                .iter()
                .enumerate()
                .filter(|(_, other)| *other == &PLACED[index])
                .map(|(other, _)| other)
                .collect();
            let at_the_instant = expected(&placed.ids, &sharing);
            let case = format!("{} at {at}", listing.operation());

            for record in &at_the_instant {
                let reported = (before.contains(record), after.contains(record));
                assert_eq!(
                    reported,
                    (listing.inclusive(), listing.inclusive()),
                    "case {case:?}: the record {record:?} at the cutoff instant must be \
                     reported by both pages only under the inclusive reading, got \
                     (before, after) = {reported:?}"
                );
            }

            let overlap: Vec<String> = before
                .iter()
                .filter(|id| after.contains(id))
                .cloned()
                .collect();
            let wanted_overlap = if listing.inclusive() {
                at_the_instant.clone()
            } else {
                Vec::new()
            };
            assert_eq!(
                overlap, wanted_overlap,
                "case {case:?}: the two pages may overlap only on the records at the cutoff \
                 instant, and only under the inclusive reading, must be {wanted_overlap:?}, \
                 got {overlap:?}"
            );

            let mut covered: Vec<String> = before.iter().chain(after.iter()).cloned().collect();
            covered.sort();
            covered.dedup();
            let mut wanted = placed.ids.clone();
            if !listing.inclusive() {
                wanted.retain(|id| !at_the_instant.contains(id));
            }
            wanted.sort();
            assert_eq!(
                covered, wanted,
                "case {case:?}: the two pages together must cover {wanted:?}, got {covered:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_placed_record_holds_the_instant_it_was_placed_at() {
    // Every expectation above is built from the placed instants, so a record
    // must really hold the instant it was placed at — including the sub-second
    // part, which the stored timestamp keeps and a cutoff comparison reads.
    let service = HealthLake::new();
    let placed = Listing::ExportJobs.place(&service).await;
    let store = service
        .engine
        .state::<ComAmazonawsHealthlakeHealthlakeState>();
    let store = store.lock();
    for (index, id) in placed.ids.iter().enumerate() {
        let job = store
            .export_jobs
            .get(id)
            .unwrap_or_else(|| panic!("the placed job {id} is held"));
        let at = placed_at(index);
        assert_eq!(
            (job.submitted_at.secs(), job.submitted_at.subsec_nanos()),
            (at.secs(), at.subsec_nanos()),
            "case {id:?}: the placed record must hold {at:?}, got {:?}",
            job.submitted_at
        );
    }
}
