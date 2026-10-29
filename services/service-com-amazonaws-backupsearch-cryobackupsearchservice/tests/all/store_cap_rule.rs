// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the ONE cap rule every capped store of this service applies, over
//! each operation that grows one: `StartSearchJob` and `StartSearchResultExportJob`.
//!
//! The rule under test: a store already holding its maximum number of records refuses the
//! call that would add another with the modelled `ServiceQuotaExceededException` (HTTP
//! 402), whose members name the account the quota is counted against, the kind of record
//! it caps, the service enforcing it and the quota itself — while a store below its
//! maximum admits the call and persists the record it minted. The two operations differ
//! only in the PARAMETERS of that refusal: the cap, the resource type, the quota code and
//! the word naming the kind of job.
//!
//! So the table below spans the rule rather than one request: its rows differ in which
//! capped store the call grows, how full that store is (one short of its maximum, exactly
//! at it, and further past it than one call could carry it) and the region the call is
//! signed for. A second table drives the shared rule directly over the same fill
//! dimension, at each boundary count and one past it, and varies the service code the
//! refusal echoes — the one member the rule takes from the request rather than from the
//! record type.
//!
//! The HTTP harness (`emulator_with_engine`, `call`, `Answer`) is the search-job suite's
//! and the exportable search job is seeded with the export-listing suite's `seed_job`, so
//! this suite drives the same emulator the same way the others do.

use std::collections::HashMap;
use std::time::SystemTime;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_backupsearch_cryobackupsearchservice::ops::types;
use service_com_amazonaws_backupsearch_cryobackupsearchservice::state;

use crate::export_listing::seed_job;
use crate::search_jobs::{call, emulator_with_engine, Answer};

/// The signing name every call in this suite is signed with, which is the service code a
/// quota refusal of this service reports.
const SIGNING_NAME: &str = "backup-search";

/// The region the seeded records' ARNs name, matching the region `seed_job` mints for.
const SEED_REGION: &str = "us-west-2";

/// One capped store of this service, with the parameters the shared cap rule reads for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Capped {
    SearchJob,
    ExportJob,
}

impl Capped {
    /// How many records of this kind the store holds at most.
    fn cap(self) -> usize {
        match self {
            Self::SearchJob => state::MAX_SEARCH_JOBS,
            Self::ExportJob => state::MAX_EXPORT_JOBS,
        }
    }

    /// The `resourceType` a refusal about this kind of record names.
    fn resource_type(self) -> &'static str {
        match self {
            Self::SearchJob => state::SEARCH_JOB_RESOURCE,
            Self::ExportJob => state::EXPORT_JOB_RESOURCE,
        }
    }

    /// The `quotaCode` a refusal about this kind of record names, by the one convention
    /// both kinds follow: the kind of job, then the account the quota is counted per.
    fn quota_code(self) -> String {
        let kind = match self {
            Self::SearchJob => "Search",
            Self::ExportJob => "Export",
        };
        format!("{kind}JobsPerAccount")
    }

    /// The cap sentence the refusal carries: the maximum this store holds, and the word
    /// naming the kind of job it holds.
    fn cap_sentence(self) -> String {
        let word = match self {
            Self::SearchJob => "search",
            Self::ExportJob => "export",
        };
        format!(
            "this account already holds the maximum of {} {word} jobs",
            self.cap()
        )
    }

    /// The operation that grows this store, for a case name.
    fn operation(self) -> &'static str {
        match self {
            Self::SearchJob => "StartSearchJob",
            Self::ExportJob => "StartSearchResultExportJob",
        }
    }

    /// The collection URI that operation is bound to.
    fn collection_uri(self) -> &'static str {
        match self {
            Self::SearchJob => "/search-jobs",
            Self::ExportJob => "/export-search-jobs",
        }
    }

    /// The output member carrying the identifier the call minted, which is also the member
    /// this service's own reader reports it back under.
    fn identifier_member(self) -> &'static str {
        match self {
            Self::SearchJob => "SearchJobIdentifier",
            Self::ExportJob => "ExportJobIdentifier",
        }
    }

    /// The URI this service's reader reports the record `identifier` names at.
    fn read_uri(self, identifier: &str) -> String {
        format!("{}/{identifier}", self.collection_uri())
    }
}

/// How many records of `kind` the store currently holds.
fn held(engine: &engine::InMemoryEngine, kind: Capped) -> usize {
    let store = engine.state::<state::ComAmazonawsBackupsearchCryobackupsearchserviceState>();
    let store = store.lock();
    match kind {
        Capped::SearchJob => store.search_jobs.len(),
        Capped::ExportJob => store.export_jobs.len(),
    }
}

/// Seed `count` more export jobs of `search_job` straight into the store, as
/// `StartSearchResultExportJob` would hold them: driving thousands of requests would only
/// be a slower way to reach the same state. The search-job side is seeded with the
/// export-listing suite's `seed_job`.
fn seed_export_jobs(engine: &engine::InMemoryEngine, count: usize, search_job: &str) {
    let store = engine.state::<state::ComAmazonawsBackupsearchCryobackupsearchserviceState>();
    let mut store = store.lock();
    for _ in 0..count {
        let identifier = store
            .mint_export_identifier()
            .expect("the store mints a fresh export-job identifier");
        store.insert_export_job(state::ExportJob {
            identifier: identifier.clone(),
            arn: state::export_job_arn(SIGNING_NAME, SEED_REGION, &identifier),
            search_job_identifier: search_job.to_string(),
            search_job_arn: state::search_job_arn(SIGNING_NAME, SEED_REGION, search_job),
            status: state::COMPLETED.to_string(),
            creation_time: state::timestamp(SystemTime::now()),
            completion_time: None,
            status_message: None,
            export_specification: types::ExportSpecification::s3ExportSpecification(
                types::S3ExportSpecification {
                    destination_bucket: "capped".to_string(),
                    destination_prefix: None,
                },
            ),
            role_arn: None,
            tags: HashMap::new(),
            client_token: None,
            request_fingerprint: identifier,
            next_status_due: None,
        });
    }
}

/// Grow the store of `kind` by `count` records, which is what puts a row at the fill level
/// it is about. `search_job` is the search job an export row's records belong to.
fn grow(engine: &engine::InMemoryEngine, kind: Capped, count: usize, search_job: &str) {
    match kind {
        Capped::SearchJob => {
            for _ in 0..count {
                seed_job(engine, state::COMPLETED, Vec::new());
            }
        }
        Capped::ExportJob => seed_export_jobs(engine, count, search_job),
    }
}

/// The request body the operation that grows the store of `kind` takes. The members that
/// are not the cap's business are the ones the other suites submit.
fn start_body(kind: Capped, search_job: &str) -> Value {
    match kind {
        Capped::SearchJob => json!({
            "Name": "capped",
            "SearchScope": {"BackupResourceTypes": ["S3"]},
        }),
        Capped::ExportJob => json!({
            "SearchJobIdentifier": search_job,
            "ExportSpecification": {
                "s3ExportSpecification": {"DestinationBucket": "capped-exports"}
            },
        }),
    }
}

/// Call the operation that grows the store of `kind`.
async fn start(
    emulator: &MultiServiceRouter,
    kind: Capped,
    region: &str,
    search_job: &str,
) -> Answer {
    call(
        emulator,
        region,
        "PUT",
        kind.collection_uri(),
        Some(start_body(kind, search_job)),
    )
    .await
}

/// Assert `answer` is the refusal the cap rule answers a full store's growth with, down to
/// each of the five members the modelled error declares.
fn assert_quota_refusal(case: &str, kind: Capped, answer: &Answer) {
    assert_eq!(
        answer.status, 402,
        "case {case:?}: a full store must refuse with the modelled quota status, got {} ({:?})",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code.as_deref(),
        Some("ServiceQuotaExceededException"),
        "case {case:?}: the refusal must be the modelled ServiceQuotaExceededException, got {:?}",
        answer.error_code
    );
    assert_eq!(
        answer.string("message"),
        kind.cap_sentence(),
        "case {case:?}: the refusal must carry the cap sentence, got {:?}",
        answer.get("message")
    );
    assert_eq!(
        answer.string("resourceId"),
        engine::emulator_account_id(),
        "case {case:?}: the quota is counted against this account, got {:?}",
        answer.get("resourceId")
    );
    assert_eq!(
        answer.string("resourceType"),
        kind.resource_type(),
        "case {case:?}: the refusal must name the kind of record the cap bounds, got {:?}",
        answer.get("resourceType")
    );
    assert_eq!(
        answer.string("serviceCode"),
        SIGNING_NAME,
        "case {case:?}: the refusal must name the service enforcing the quota, got {:?}",
        answer.get("serviceCode")
    );
    assert_eq!(
        answer.string("quotaCode"),
        kind.quota_code(),
        "case {case:?}: the refusal must name the quota that was exceeded, got {:?}",
        answer.get("quotaCode")
    );
}

/// One row of the cap table: which store the call grows, how full it is beforehand, the
/// region the call is signed for, and whether the rule admits it.
struct CapCase {
    case: String,
    kind: Capped,
    region: &'static str,
    held: usize,
    admitted: bool,
}

/// The rows of the cap table, built from whatever maximum each store enforces rather than
/// from a retyped number, so a row sits at that store's own boundary.
fn cap_cases() -> Vec<CapCase> {
    let mut cases = Vec::new();
    for kind in [Capped::SearchJob, Capped::ExportJob] {
        let cap = kind.cap();
        for (held, admitted, region) in [
            (cap - 1, true, "us-west-2"),
            (cap, false, "us-west-2"),
            // Nothing evicts a record, so a store can be found further past its maximum
            // than one call could carry it — and in a region other than the one its
            // records were created in, because the quota is the account's.
            (cap + 5, false, "eu-central-1"),
        ] {
            cases.push(CapCase {
                case: format!("{} with {held} of {cap} records held", kind.operation()),
                kind,
                region,
                held,
                admitted,
            });
        }
    }
    cases
}

#[tokio::test]
async fn every_capped_store_applies_one_cap_rule_and_answers_one_refusal() {
    let mut refusals: Vec<(Capped, String, String)> = Vec::new();

    for case in cap_cases() {
        let (emulator, engine) = emulator_with_engine();
        // An export is of ONE search job's results, so an export row needs a search job an
        // export may be started from, whatever its own store holds.
        let search_job = match case.kind {
            Capped::SearchJob => String::new(),
            Capped::ExportJob => seed_job(&engine, state::COMPLETED, Vec::new()),
        };
        grow(&engine, case.kind, case.held, &search_job);
        assert_eq!(
            held(&engine, case.kind),
            case.held,
            "case {:?}: the row must start from {} held records",
            case.case,
            case.held
        );

        let answer = start(&emulator, case.kind, case.region, &search_job).await;

        if case.admitted {
            assert_eq!(
                answer.status, 200,
                "case {:?}: a store below its maximum must admit the call, got {} ({:?})",
                case.case, answer.status, answer.body
            );
            // Admitted means minted: the record reads back through this service's own
            // reader, so the row's success is a persisted record and not an empty answer.
            let identifier = answer.string(case.kind.identifier_member());
            let read_back = call(
                &emulator,
                case.region,
                "GET",
                &case.kind.read_uri(&identifier),
                None,
            )
            .await;
            assert_eq!(
                read_back.status, 200,
                "case {:?}: the admitted record must read back, got {} ({:?})",
                case.case, read_back.status, read_back.body
            );
            assert_eq!(
                read_back.string(case.kind.identifier_member()),
                identifier,
                "case {:?}: the reader must report the record that was minted, got {:?}",
                case.case,
                read_back.body
            );
            // And the call after it, with the store now at its maximum, is refused by the
            // same rule.
            let past = start(&emulator, case.kind, case.region, &search_job).await;
            assert_quota_refusal(&case.case, case.kind, &past);
            refusals.push((
                case.kind,
                past.string("resourceId"),
                past.string("serviceCode"),
            ));
        } else {
            assert_quota_refusal(&case.case, case.kind, &answer);
            // A refused call persists nothing, so a store past its maximum cannot be
            // grown further by being called.
            assert_eq!(
                held(&engine, case.kind),
                case.held,
                "case {:?}: a refused call must leave the store as it was",
                case.case
            );
            refusals.push((
                case.kind,
                answer.string("resourceId"),
                answer.string("serviceCode"),
            ));
        }
    }

    // The members that are the RULE's rather than one store's agree across every row of
    // both stores, which is what one shared rule means and what two copies of it could not
    // guarantee.
    let first = refusals
        .first()
        .expect("the cap table has rows")
        .clone();
    for (kind, resource_id, service_code) in &refusals {
        assert_eq!(
            (resource_id.as_str(), service_code.as_str()),
            (first.1.as_str(), first.2.as_str()),
            "case {kind:?}: every capped store's refusal names the same account and service, got \
             {resource_id:?} / {service_code:?}"
        );
    }
}

#[test]
fn the_cap_rule_admits_below_the_maximum_and_refuses_from_it_on() {
    // The shared rule's own verdict, under the state module, over the fill dimension the
    // HTTP rows walk: each boundary count of each capped store, and the service code the
    // refusal takes from the request rather than from the record type.
    for kind in [Capped::SearchJob, Capped::ExportJob] {
        let (_emulator, engine) = emulator_with_engine();
        let search_job = match kind {
            Capped::SearchJob => String::new(),
            Capped::ExportJob => seed_job(&engine, state::COMPLETED, Vec::new()),
        };
        let cap = kind.cap();
        let mut grown = 0_usize;

        for probe in [0, 1, cap - 1, cap, cap + 1, cap + 5] {
            grow(&engine, kind, probe - grown, &search_job);
            grown = probe;
            let case = format!("{:?} holding {probe} of {cap} records", kind);

            for service_code in [SIGNING_NAME, "backup-search-under-another-name"] {
                let store =
                    engine.state::<state::ComAmazonawsBackupsearchCryobackupsearchserviceState>();
                let verdict = {
                    let store = store.lock();
                    match kind {
                        Capped::SearchJob => store.check_search_job_capacity(service_code),
                        Capped::ExportJob => store.check_export_job_capacity(service_code),
                    }
                };

                if probe < cap {
                    assert!(
                        verdict.is_ok(),
                        "case {case:?}: a store below its maximum must admit another record, got \
                         {verdict:?}"
                    );
                    continue;
                }
                let refusal = verdict.expect_err(&format!(
                    "case {case:?}: a store at its maximum must refuse another record"
                ));
                assert_eq!(
                    refusal.message,
                    kind.cap_sentence(),
                    "case {case:?}: the refusal must carry the cap sentence, got {:?}",
                    refusal.message
                );
                assert_eq!(
                    refusal.resource_id,
                    engine::emulator_account_id(),
                    "case {case:?}: the quota is counted against this account, got {:?}",
                    refusal.resource_id
                );
                assert_eq!(
                    refusal.resource_type,
                    kind.resource_type(),
                    "case {case:?}: the refusal must name the kind of record the cap bounds, got \
                     {:?}",
                    refusal.resource_type
                );
                assert_eq!(
                    refusal.service_code, service_code,
                    "case {case:?}: the refusal must name the service the request reached, got {:?}",
                    refusal.service_code
                );
                assert_eq!(
                    refusal.quota_code,
                    kind.quota_code(),
                    "case {case:?}: the refusal must name the quota that was exceeded, got {:?}",
                    refusal.quota_code
                );
            }
        }
    }
}
