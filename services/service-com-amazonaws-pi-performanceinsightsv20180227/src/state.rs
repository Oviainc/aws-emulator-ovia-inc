// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-pi-performanceinsightsv20180227 service.
//!
//! Performance Insights persists exactly one kind of resource: the performance
//! analysis report a caller creates for a window on a monitored DB resource.
//! Everything the report lifecycle (`CreatePerformanceAnalysisReport`,
//! `GetPerformanceAnalysisReport`, `ListPerformanceAnalysisReports`,
//! `DeletePerformanceAnalysisReport`) answers is computed from the store below.
//!
//! Resource metrics are NOT stored: `GetResourceMetrics` derives its aligned
//! window, its effective period, its metric keys and the one data point it
//! reports per period from the request, so reading metrics never writes.
//!
//! The data sources themselves are not stored here either — no Performance
//! Insights operation registers one, they are the DB resources other services
//! own. Three questions are asked about a request, and they have different
//! answers:
//!
//! * whether the CALLER may perform the action at all is decided first, from
//!   the identity-based policies the IAM store holds for the identity the
//!   request's signing key belongs to ([`identity_based_denial`]), read through
//!   the shared [`::service_common::iam_authorization`] evaluator;
//! * whether a request addresses a data source this account may see at all is
//!   decided by [`is_data_source_identifier`], the authorization gate every
//!   operation in this crate applies before it answers; and
//! * whether the request's `ServiceType` is the service that OWNS the addressed
//!   data source is decided by [`service_type_refusal`], against the shared
//!   data-source directory the database control planes write when they mint a
//!   resource id ([`::service_common::db_data_sources`]).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use ::aws_smithy_types::DateTime;
use ::service_common::db_data_sources::DataSourceDirectory;
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_state;
use service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_state;

use crate::ops::types::{Insight, Tag};

/// `AnalysisStatus` while Performance Insights is still analysing the window.
pub const STATUS_RUNNING: &str = "RUNNING";

/// `AnalysisStatus` once the analysis of the window finished.
pub const STATUS_SUCCEEDED: &str = "SUCCEEDED";

/// Region used for a report's ARN when the request carries no SigV4 credential
/// scope to read one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// How long a freshly created report stays `RUNNING` before the clock owes it
/// `SUCCEEDED`.
///
/// AWS documents no analysis duration for `CreatePerformanceAnalysisReport`, so
/// this span is CHOSEN, inside the emulator's 250–750 ms band for a clock-owed
/// transition. It is deliberately not zero: the real service answers Create with
/// a report that is not finished yet, so a caller that reads the report straight
/// afterwards must still observe the non-terminal status.
pub const REPORT_ANALYSIS_SPAN: Duration = Duration::from_millis(500);

/// How many reports one monitored resource may hold.
///
/// An EMULATOR ceiling, not an AWS quota: this store is in memory and only
/// `DeletePerformanceAnalysisReport` ever drops an entry, so without a ceiling a
/// caller looping on Create could grow this process's memory without bound. It
/// is set far above any ordinary workflow, so a client exercising the report
/// lifecycle never meets it, and a request past it is refused with the
/// operation's modelled `InvalidArgumentException` rather than accepted and
/// silently dropped.
pub const MAX_REPORTS_PER_RESOURCE: usize = 1_000;

/// How many monitored resources may hold reports at once.
///
/// The same emulator ceiling, applied to the caller-chosen key space
/// `(ServiceType, Identifier)` so a caller cannot grow the store by naming a new
/// resource each time instead of a new report.
pub const MAX_MONITORED_RESOURCES: usize = 1_000;

/// One persisted performance analysis report.
#[derive(Debug, Clone)]
pub struct AnalysisReportRecord {
    /// `report-` + 17 lowercase hex characters (the modelled `AnalysisReportId`
    /// pattern), minted by [`ComAmazonawsPiPerformanceinsightsv20180227State::mint_report_id`].
    pub analysis_report_id: String,
    /// `RDS` or `DOCDB` — half of the resource key this report is filed under.
    pub service_type: String,
    /// The monitored resource (a DbiResourceId for RDS) this report analyses.
    pub identifier: String,
    /// The report's own ARN, the handle the tag operations address it by.
    pub resource_arn: String,
    /// When the report was created, as the service observed it.
    pub create_time: DateTime,
    /// The analysed window's start, as persisted by Create.
    pub start_time: DateTime,
    /// The analysed window's end, as persisted by Create (absent when the
    /// caller gave none).
    pub end_time: Option<DateTime>,
    /// `AnalysisStatus`: [`STATUS_RUNNING`] until the clock owes
    /// [`STATUS_SUCCEEDED`].
    pub status: String,
    /// The instant at which `status` becomes [`STATUS_SUCCEEDED`]. Stored, never
    /// slept on: a reporting operation materialises the transition the clock has
    /// already made due (see
    /// [`ComAmazonawsPiPerformanceinsightsv20180227State::materialize_due_statuses`]).
    pub status_due: Instant,
    /// The report's metadata tags: the ones Create persisted, in the order the
    /// caller sent them, as `TagResource` and `UntagResource` have since left
    /// them. This is the ONLY home for a report's tags — a listing's summary,
    /// `ListTagsForResource` and the two tag writers all read and write this one
    /// list, so no two of them can report a different tag set for one report.
    pub tags: Vec<Tag>,
    /// Creation sequence within this store, used as the stable tiebreak when two
    /// reports of one resource analyse windows with the same start time.
    pub sequence: u64,
    /// The insights the analysis produced. The emulator has no DB session data
    /// to analyse, so this stays empty rather than carrying invented findings.
    pub insights: Vec<Insight>,
}

/// How many tags one report holds.
///
/// The modelled `TagList` length maximum (0..200), which bounds both the tags a
/// single `TagResource` call carries and the tags `ListTagsForResource` reports —
/// so a stored set past it could not be reported at all. A write that would
/// exceed it is refused with the operation's modelled `InvalidArgumentException`
/// rather than accepted and silently clipped.
pub const MAX_TAGS_PER_REPORT: usize = 200;

impl AnalysisReportRecord {
    /// Add `tags` to this report, overwriting the value of a key it already
    /// carries and appending a key it does not.
    ///
    /// `Err` names the constraint when the resulting set would pass
    /// [`MAX_TAGS_PER_REPORT`]; the report is then left exactly as it was, so a
    /// refused write adds no tag at all.
    pub fn apply_tags(&mut self, tags: &[Tag]) -> Result<(), String> {
        let added = tags
            .iter()
            .filter(|tag| !self.tags.iter().any(|held| held.key == tag.key))
            .count();
        if self.tags.len() + added > MAX_TAGS_PER_REPORT {
            return Err(format!(
                "Tags would leave the resource with {} tags, more than the maximum of \
                 {MAX_TAGS_PER_REPORT} tags a resource holds.",
                self.tags.len() + added
            ));
        }
        for tag in tags {
            match self.tags.iter_mut().find(|held| held.key == tag.key) {
                Some(held) => held.value = tag.value.clone(),
                None => self.tags.push(tag.clone()),
            }
        }
        Ok(())
    }

    /// Remove every tag whose key is in `keys`. A key the report does not carry
    /// removes nothing, which is what makes `UntagResource` idempotent.
    pub fn remove_tags(&mut self, keys: &[String]) {
        self.tags.retain(|tag| !keys.contains(&tag.key));
    }
}

/// Per-service state for com-amazonaws-pi-performanceinsightsv20180227. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsPiPerformanceinsightsv20180227State {
    /// Reports per monitored resource, keyed by `(ServiceType, Identifier)` and
    /// held in creation order so a paged List visits each report exactly once.
    pub reports: HashMap<(String, String), Vec<AnalysisReportRecord>>,
    /// Monotonic report counter, mixed into the minted id so two reports of one
    /// resource can never share an id.
    pub report_seq: u64,
}

impl ComAmazonawsPiPerformanceinsightsv20180227State {
    /// Mint the next `AnalysisReportId` for a resource, with the creation
    /// sequence it was minted at.
    ///
    /// The modelled pattern is `^report-[0-9a-f]{17}$`, so the suffix is 17
    /// lowercase hex characters. UNIQUENESS comes from the low 32 bits of the
    /// monotonic counter in the last 8 of them, and from nothing else: two ids
    /// of one process can only repeat after 2^32 reports, which the store's own
    /// ceiling puts far out of reach. The leading 9 characters are a digest of
    /// the resource and the creation instant purely so the id looks opaque, the
    /// way a real `report-` id does, rather than counting visibly from one; they
    /// are not part of the uniqueness argument.
    pub fn mint_report_id(
        &mut self,
        service_type: &str,
        identifier: &str,
        create_time: DateTime,
    ) -> (String, u64) {
        use sha2::Digest as _;

        self.report_seq += 1;
        let seq = self.report_seq;

        let mut hasher = sha2::Sha256::new();
        hasher.update(service_type.as_bytes());
        hasher.update([0u8]);
        hasher.update(identifier.as_bytes());
        hasher.update([0u8]);
        hasher.update(seq.to_be_bytes());
        hasher.update(create_time.secs().to_be_bytes());
        hasher.update(create_time.subsec_nanos().to_be_bytes());
        let digest = hex::encode(hasher.finalize());

        (
            format!("report-{}{:08x}", &digest[..9], (seq & 0xffff_ffff) as u32),
            seq,
        )
    }

    /// Materialise every status transition the clock has already made due, for
    /// ONE monitored resource.
    ///
    /// This is the one write a reporting operation may make: the `RUNNING` ->
    /// `SUCCEEDED` move was owed at this instant whether or not anybody called.
    /// Scoped to the resource being read, so the cost of a read is the size of
    /// that resource's own report list and not of the whole store.
    pub fn materialize_due_statuses(&mut self, service_type: &str, identifier: &str) {
        let now = Instant::now();
        let Some(reports) = self
            .reports
            .get_mut(&(service_type.to_string(), identifier.to_string()))
        else {
            return;
        };
        for report in reports.iter_mut() {
            if report.status == STATUS_RUNNING && now >= report.status_due {
                report.status = STATUS_SUCCEEDED.to_string();
            }
        }
    }

    /// The reports of one monitored resource, in creation order.
    pub fn reports_of(&self, service_type: &str, identifier: &str) -> &[AnalysisReportRecord] {
        self.reports
            .get(&(service_type.to_string(), identifier.to_string()))
            .map_or(&[], Vec::as_slice)
    }

    /// The reports of one monitored resource in the order the service lists
    /// them.
    ///
    /// `ListPerformanceAnalysisReports` documents: "The reports are sorted based
    /// on the start time of each report." Two reports of one resource may
    /// analyse windows with the same start time, so the creation sequence is the
    /// stable tiebreak — which keeps a cursor's offset meaningful across pages.
    pub fn listed_reports_of(
        &self,
        service_type: &str,
        identifier: &str,
    ) -> Vec<&AnalysisReportRecord> {
        let mut reports: Vec<&AnalysisReportRecord> =
            self.reports_of(service_type, identifier).iter().collect();
        reports.sort_by_key(|report| (timestamp_key(report.start_time), report.sequence));
        reports
    }

    /// The report one `ResourceARN` names, wherever it is filed.
    ///
    /// The tag operations address a report by its ARN alone, and an ARN is minted
    /// once per report and never changes, so this resolves the same report the
    /// `(ServiceType, Identifier, AnalysisReportId)` triple does.
    pub fn report_by_arn(&self, resource_arn: &str) -> Option<&AnalysisReportRecord> {
        self.reports
            .values()
            .flatten()
            .find(|report| arn_addresses_report(&report.resource_arn, resource_arn))
    }

    /// The same report, for a tag write.
    pub fn report_by_arn_mut(&mut self, resource_arn: &str) -> Option<&mut AnalysisReportRecord> {
        self.reports
            .values_mut()
            .flatten()
            .find(|report| arn_addresses_report(&report.resource_arn, resource_arn))
    }

    /// One report of one monitored resource.
    pub fn report(
        &self,
        service_type: &str,
        identifier: &str,
        analysis_report_id: &str,
    ) -> Option<&AnalysisReportRecord> {
        self.reports_of(service_type, identifier)
            .iter()
            .find(|report| report.analysis_report_id == analysis_report_id)
    }

    /// Append a freshly created report to its resource's list.
    pub fn insert_report(&mut self, report: AnalysisReportRecord) {
        self.reports
            .entry((report.service_type.clone(), report.identifier.clone()))
            .or_default()
            .push(report);
    }

    /// Why one more report cannot be stored for this resource, or `None` when
    /// there is room for it.
    ///
    /// Checked BEFORE anything is written, so a refused Create leaves the store
    /// exactly as it was. A report costs one entry in `reports` — its tags live
    /// on the record itself, bounded by [`MAX_TAGS_PER_REPORT`] — so bounding the
    /// reports bounds the store.
    pub fn capacity_refusal(&self, service_type: &str, identifier: &str) -> Option<String> {
        let key = (service_type.to_string(), identifier.to_string());
        match self.reports.get(&key) {
            Some(reports) if reports.len() >= MAX_REPORTS_PER_RESOURCE => Some(format!(
                "The resource {identifier} already holds the maximum of \
                 {MAX_REPORTS_PER_RESOURCE} analysis reports this emulator stores; delete a \
                 report before creating another."
            )),
            Some(_) => None,
            None if self.reports.len() >= MAX_MONITORED_RESOURCES => Some(format!(
                "Analysis reports are already stored for the maximum of \
                 {MAX_MONITORED_RESOURCES} monitored resources this emulator holds; delete a \
                 resource's reports before creating reports for another."
            )),
            None => None,
        }
    }

    /// Remove one report, with the tags it carries. `false` when the resource
    /// holds no report with that id.
    pub fn remove_report(
        &mut self,
        service_type: &str,
        identifier: &str,
        analysis_report_id: &str,
    ) -> bool {
        let key = (service_type.to_string(), identifier.to_string());
        let Some(reports) = self.reports.get_mut(&key) else {
            return false;
        };
        let Some(position) = reports
            .iter()
            .position(|report| report.analysis_report_id == analysis_report_id)
        else {
            return false;
        };
        reports.remove(position);
        true
    }
}

/// The message every authorization refusal in this service carries.
///
/// ONE service-wide constant, with nothing from the request in it: the real
/// service answers the same sentence whether the data source is absent, belongs
/// to another account, or holds no report with the requested id, precisely so
/// the answer discloses which. A message rebuilt from a lookup miss ("… was not
/// found for the resource …") would leak exactly the existence information this
/// uniform refusal exists to withhold. The modelled requirement is the status
/// (400) and the error code (`NotAuthorizedException`); this text is the one the
/// corpus records for it, identically on five different operations, which is
/// what makes it a constant rather than one request's value.
pub const NOT_AUTHORIZED_MESSAGE: &str = "The specified resource is not authorized for this account";

/// The kind prefixes an immutable DB resource id is minted with: `db-` for a DB
/// instance (reported as `DbiResourceId`) and `cluster-` for a DB cluster
/// (reported as `DbClusterResourceId`).
const DATA_SOURCE_ID_PREFIXES: [&str; 2] = ["db-", "cluster-"];

/// Whether `identifier` addresses a data source Performance Insights serves for
/// this account — the check every operation that names a `(ServiceType,
/// Identifier)` pair makes before it answers anything.
///
/// Performance Insights gathers metrics from a data source addressed by the
/// IMMUTABLE resource id its owning service minted, which is what every
/// `Identifier` member of this model documents: "When you call
/// DescribeDBInstances, the identifier is returned as DbiResourceId. To use a DB
/// instance as a data source, specify its DbiResourceId value. For example,
/// specify db-ABCDEFGHIJKLMNOPQRSTU1VW2X." Those ids are a kind prefix plus a
/// token drawn from an uppercase-alphanumeric alphabet — the same ids the
/// emulator's own database services mint (RDS `mint_dbi_resource_id`: `db-`
/// plus 26 uppercase base-36 symbols; DocumentDB and Neptune
/// `CreateDBCluster`: `cluster-` plus a token of that alphabet). The modelled
/// `IdentifierString` pattern `^[a-zA-Z0-9-]+$` is far wider, so the boundary
/// validator passes anything that merely looks like a NAME — a
/// `DBInstanceIdentifier`, say — and this is the check that refuses it, with the
/// modelled `NotAuthorizedException` rather than an argument error: the value
/// is a well-formed `IdentifierString`, it just addresses no data source this
/// account may see.
///
/// It is a NECESSARY condition, not a sufficient one: this emulator monitors no
/// running DB engine and holds no cross-account inventory, so it cannot know
/// whether a well-formed resource id was ever minted for this account. What it
/// can decide — and does, uniformly for every operation — is that an identifier
/// of a form this service could never have handed out addresses nothing, and is
/// never served as if the account owned it.
///
/// It says nothing about WHICH service owns the identifier, because the form
/// cannot: RDS and DocumentDB mint ids of the same two forms. That question is
/// [`service_type_refusal`]'s, and it is answered from the record the minting
/// service left, never from the id's shape.
pub fn is_data_source_identifier(identifier: &str) -> bool {
    DATA_SOURCE_ID_PREFIXES.iter().any(|prefix| {
        identifier.strip_prefix(prefix).is_some_and(|token| {
            !token.is_empty()
                && token
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || byte.is_ascii_uppercase())
        })
    })
}

/// Whether `requested` addresses the report whose ARN is `minted`.
///
/// Every segment is compared EXACTLY — so another account's, another region's,
/// another data source's or another report's ARN addresses nothing here — except
/// the one carrying the `ServiceType`, which is compared without regard to case.
///
/// That one segment is the only place an ARN restates a value the request already
/// carries as a modelled ENUM, and it identifies nothing on its own: a report is
/// already named uniquely by the data source and the `AnalysisReportId` beside it,
/// both compared exactly, so no two reports can differ only in the case of this
/// segment and the tolerance can never resolve one report's ARN to another. It
/// exists because no response member of this model carries a report's ARN: a
/// client that tags a report BUILDS the ARN from the `ServiceType` and
/// `Identifier` it created the report with and the `AnalysisReportId` it got
/// back, and writes the enum value in whichever case it holds it.
pub fn arn_addresses_report(minted: &str, requested: &str) -> bool {
    match (split_report_arn(minted), split_report_arn(requested)) {
        (Some(minted), Some(requested)) => {
            minted.prefix == requested.prefix
                && minted.service_type.eq_ignore_ascii_case(requested.service_type)
                && minted.identifier == requested.identifier
                && minted.analysis_report_id == requested.analysis_report_id
        }
        // Not an ARN of the form this service mints for a report: only an exact
        // match could address one, and nothing of that form is stored.
        _ => minted == requested,
    }
}

/// The parts of a report ARN [`arn_addresses_report`] compares.
struct ReportArnParts<'a> {
    /// Everything up to the resource: `arn:<partition>:pi:<region>:<account>`.
    prefix: &'a str,
    /// The `ServiceType` segment of the resource.
    service_type: &'a str,
    /// The data source the report analyses.
    identifier: &'a str,
    /// The report's own id.
    analysis_report_id: &'a str,
}

/// Split an ARN of the form [`report_arn`] mints, or `None` when it is not one.
///
/// A report's resource part carries no `:`, so the LAST `:` separates the ARN's
/// prefix from it.
fn split_report_arn(arn: &str) -> Option<ReportArnParts<'_>> {
    let (prefix, resource) = arn.rsplit_once(':')?;
    let mut parts = resource.split('/');
    let (Some("perf-reports"), Some(service_type), Some(identifier), Some(analysis_report_id), None) = (
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
    ) else {
        return None;
    };
    Some(ReportArnParts {
        prefix,
        service_type,
        identifier,
        analysis_report_id,
    })
}

/// The current wall-clock instant as the modelled `ISOTimestamp` type.
pub fn now() -> DateTime {
    let since_epoch = ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .unwrap_or_default();
    DateTime::from_secs_f64(since_epoch.as_secs_f64())
}

/// The region this request was signed for, for the ARNs a handler mints.
/// [`DEFAULT_REGION`] when the request carries no SigV4 credential scope.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The ARN of one performance analysis report.
///
/// `arn:aws:pi:<region>:<account>:perf-reports/<ServiceType>/<Identifier>/<AnalysisReportId>`
/// — the shape AWS documents for a Performance Insights analysis report, whose
/// modelled `ResourceARN` pattern is `^arn:.*:pi:.*$`.
pub fn report_arn(
    region: &str,
    service_type: &str,
    identifier: &str,
    analysis_report_id: &str,
) -> String {
    ::emulator_server::emulator_arn(
        "pi",
        region,
        &format!("perf-reports/{service_type}/{identifier}/{analysis_report_id}"),
    )
}

/// The SCOPE a pagination cursor is bound to, as a digest of the request members
/// that fix the result set.
///
/// ONE helper for the whole service: hashing keeps a token far inside the
/// modelled `NextToken` length bound however long those members are, and the
/// `NUL` separator keeps the digest unambiguous — no member of a Performance
/// Insights request may contain that byte, so two different member lists cannot
/// hash the same bytes. Pass each member as text, in a fixed order.
pub fn page_scope(parts: &[&str]) -> String {
    use sha2::Digest as _;

    let mut hasher = sha2::Sha256::new();
    for part in parts {
        hasher.update(part.as_bytes());
        hasher.update([0u8]);
    }
    hex::encode(hasher.finalize())
}

/// Encode an opaque pagination cursor: the offset the next page resumes at,
/// bound to the `scope` (the request members that fix the result set) it was
/// handed out for.
///
/// Hex keeps the token inside the modelled `NextToken` pattern
/// `^[a-zA-Z0-9_=-]+$`, and makes it self-describing, so paging stores nothing:
/// listing never mutates what a later caller sees.
pub fn encode_page_token(scope: &str, offset: usize) -> String {
    hex::encode(format!("{offset}|{scope}"))
}

/// Decode a cursor produced by [`encode_page_token`]. `None` when the token is
/// not one this service handed out, or was handed out for a different result
/// set.
pub fn decode_page_token(token: &str, scope: &str) -> Option<usize> {
    let decoded = String::from_utf8(hex::decode(token).ok()?).ok()?;
    let (offset, token_scope) = decoded.split_once('|')?;
    if token_scope != scope {
        return None;
    }
    offset.parse().ok()
}

/// Order two `ISOTimestamp` values. `DateTime` carries seconds plus a
/// sub-second part, so both are compared.
pub fn timestamp_key(value: DateTime) -> (i64, u32) {
    (value.secs(), value.subsec_nanos())
}

/// The refusal a `(ServiceType, Identifier)` pair earns when the identifier
/// names a data source a DIFFERENT Amazon Web Services service owns, or `None`
/// when nothing this emulator has recorded contradicts the pair.
///
/// Performance Insights serves a data source under the `ServiceType` of the
/// service that OWNS it: a resource id belonging to one service's namespace,
/// queried under the other's, names no data source, and the pair is an invalid
/// ARGUMENT combination rather than an authorization outcome — the operations
/// model `InvalidArgumentException` (HTTP 400, one of the arguments provided is
/// invalid for this request) for exactly that.
///
/// The owning service cannot be read off the identifier: RDS and DocumentDB both
/// mint `db-<TOKEN>` for an instance and `cluster-<TOKEN>` for a cluster (RDS
/// `mint_dbi_resource_id`, DocumentDB `CreateDBInstance` / `CreateDBCluster`), so
/// the form is silent about ownership. What decides it is the record the minting
/// service left in the shared data-source directory
/// ([`::service_common::db_data_sources`]), which also carries the engine the
/// resource runs — so the refusal names the engine for which the requested
/// `ServiceType` is wrong, the way the real service's does, computed from the
/// stored resource and not from any one request.
///
/// A data source the directory holds no record of is UNDECIDED, not refused: this
/// emulator can only know what it minted itself, and refusing every id it did not
/// mint would turn a missing record into a wrong answer. Such a pair is then
/// decided by [`is_data_source_identifier`] alone, as before.
pub fn service_type_refusal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    service_type: &str,
    identifier: &str,
) -> Option<String> {
    let directory = data_source_directory(engine)?;
    let db_engine = {
        let guard = directory.lock();
        guard
            .registered_elsewhere(identifier, service_type)
            .map(|record| record.engine.clone())?
    };
    Some(wrong_service_type_message(&db_engine))
}

/// The message a `ServiceType` that does not own the addressed data source is
/// refused with.
///
/// The modelled requirement is the status (400) and the error code
/// (`InvalidArgumentException`); the sentence states the reason the refusal has,
/// built from the engine the stored resource runs, so a client that passed the
/// wrong `ServiceType` can see which engine it addressed.
pub fn wrong_service_type_message(db_engine: &str) -> String {
    format!("The specified ServiceType is invalid for engine {db_engine}.")
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsPiPerformanceinsightsv20180227State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsPiPerformanceinsightsv20180227State>)
}

// ---------------------------------------------------------------------------
// Metric names
// ---------------------------------------------------------------------------

/// The aggregate functions a Performance Insights metric name carries, from
/// `GetResourceMetrics$MetricQueries`' own documentation: "You must append the
/// aggregate function to the metric. For example, to find the average for the
/// metric db.load you must use db.load.avg. Valid values for aggregate
/// functions include .avg, .min, .max, and .sum."
pub const AGGREGATE_FUNCTION_SUFFIXES: [&str; 4] = [".avg", ".min", ".max", ".sum"];

/// The ONE aggregate function `DescribeDimensionKeys$AdditionalMetrics`
/// documents: "The only supported statistic function is .avg."
pub const ADDITIONAL_METRIC_SUFFIX: &str = ".avg";

/// The sentence Performance Insights answers a metric name whose aggregate
/// function it does not know with.
///
/// ONE service-wide constant carrying nothing from the request: the same
/// sentence answers a metric with no aggregate function at all and one whose
/// suffix is not an aggregate function. The requirement is the status (400) and
/// the error code (`InvalidArgumentException`); this is the text the corpus
/// records for the condition.
pub const UNKNOWN_STATISTIC_MESSAGE: &str = "The specified statistic is not a known statistic";

/// Whether `metric` names a metric AND an aggregate function, which is what
/// every metric member of this model asks a caller for.
///
/// The check is on the SUFFIX only: Performance Insights publishes a metric
/// catalogue per DB engine (`ListAvailableResourceMetrics` is what enumerates
/// it) which this emulator does not hold, so the metric itself is not resolved
/// here — the aggregate function is, because the model states its value set.
/// A name that is nothing but the suffix (`".avg"`) names no metric to
/// aggregate, so it does not satisfy this either.
pub fn names_an_aggregate_function(metric: &str) -> bool {
    AGGREGATE_FUNCTION_SUFFIXES
        .iter()
        .any(|suffix| metric.len() > suffix.len() && metric.ends_with(suffix))
}

/// The same question for an `AdditionalMetrics` entry, whose documentation
/// narrows the set to `.avg` alone.
pub fn names_the_additional_metric_function(metric: &str) -> bool {
    metric.len() > ADDITIONAL_METRIC_SUFFIX.len() && metric.ends_with(ADDITIONAL_METRIC_SUFFIX)
}

// ---------------------------------------------------------------------------
// The aligned window every time-series operation reports
// ---------------------------------------------------------------------------

/// The granularities `PeriodInSeconds` accepts, from that member's own
/// documentation: 1 (one second), 60 (one minute), 300 (five minutes), 3600 (one
/// hour) and 86400 (twenty-four hours).
pub const VALID_PERIODS_IN_SECONDS: &[i64] = &[1, 60, 300, 3600, 86400];

/// How far from the epoch a window's timestamps may lie, in either direction.
///
/// NOT a guess at what AWS accepts: it is the magnitude that keeps every step of
/// the window arithmetic total. With both ends inside `±i64::MAX / 4`, the span
/// (`end - start`), the end's round-up (`end + 1`, then up to one period more)
/// and the aligned window (`aligned_end - aligned_start`) all stay inside i64,
/// so a caller-chosen `ISOTimestamp` cannot overflow them. A value past it is
/// answered with the modelled 400 instead.
pub const MAX_TIMESTAMP_MAGNITUDE: i64 = i64::MAX / 4;

/// The data-point budget the service aims at when the caller specifies no
/// period. `PeriodInSeconds`' documentation: "If you don't specify
/// PeriodInSeconds, then Performance Insights will choose a value for you, with
/// a goal of returning roughly 100-200 data points in the response."
const PERIOD_CHOICE_MAX_POINTS: i64 = 200;

/// How many data points one response may carry, counting every series in it.
///
/// An EMULATOR ceiling, not an AWS quota: `GetResourceMetrics` materialises one
/// data point per period of the aligned window FOR EVERY metric key it answers,
/// and both the window and the period come from the request, so without a
/// budget a single request would decide how much memory this process allocates
/// and how large a body it serialises (a two-year window at the one-second
/// granularity is 6.3 × 10^7 periods, times up to the fifteen metric queries
/// `MetricQueryList` admits). The budget is taken over the WHOLE response
/// rather than per series, because the per-key series are what multiply, and it
/// is asked by the operation that ALLOCATES per period — an operation reporting
/// no series over a window is not refused for the window's width.
///
/// It is set far above any ordinary query — a full day at the finest documented
/// granularity is 86 400 points, and the service's own goal when it picks the
/// period is 100-200 — so a client charting a series never meets it. A request
/// past it is refused with the operation's modelled `InvalidArgumentException`,
/// never answered with a truncated grid a client could not tell from a complete
/// one, and the refusal names the two request members that bring it back inside
/// the budget (`MaxResults`, which pages the keys, and `PeriodInSeconds`).
pub const MAX_DATA_POINTS_PER_RESPONSE: i64 = 100_000;

/// The window a time-series request reports back: the effective period and the
/// granular boundaries the requested window was widened to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AlignedWindow {
    /// The effective `PeriodInSeconds`: the caller's, or the one the service
    /// chose from the span.
    pub period: i64,
    /// `AlignedStartTime` as epoch seconds, on a period boundary at or before
    /// the requested `StartTime`.
    pub start: i64,
    /// `AlignedEndTime` as epoch seconds, on a period boundary at or after the
    /// requested `EndTime`.
    pub end: i64,
}

impl AlignedWindow {
    /// How many whole periods the window holds — the number of data points a
    /// series over it carries.
    pub fn period_count(&self) -> i64 {
        (self.end - self.start) / self.period
    }

    /// The timestamp of period `index`, stamped as `alignment` asks: the
    /// period's START for `START_TIME`, its END otherwise.
    ///
    /// `PeriodAlignment`'s documentation: "The returned timestamp which is the
    /// start or end time of the time periods. The default value is END_TIME."
    pub fn period_timestamp(&self, index: i64, alignment: Option<&str>) -> i64 {
        let offset = if alignment == Some(PERIOD_ALIGNMENT_START_TIME) {
            index
        } else {
            index + 1
        };
        self.start + offset * self.period
    }
}

/// The `PeriodAlignment` value that stamps a point with its period's start; any
/// other value (including the absent default) stamps it with the period's end.
pub const PERIOD_ALIGNMENT_START_TIME: &str = "START_TIME";

/// Resolve the window a request's `StartTime`, `EndTime` and `PeriodInSeconds`
/// describe, or say why no window can be computed for them.
///
/// `Err` is the sentence for the operation's modelled `InvalidArgumentException`.
/// The ordering rule (`StartTime` before `EndTime`) is NOT checked here: it is
/// the service-wide refusal every handler answers first (see
/// [`crate::service::TIME_RANGE_ORDERING_MESSAGE`]), so a window that is merely
/// inverted is never reported as an arithmetic problem.
pub fn aligned_window(
    start_time: DateTime,
    end_time: DateTime,
    period_in_seconds: Option<i32>,
) -> Result<AlignedWindow, String> {
    // Both timestamps are caller-chosen and a `timestamp` shape carries no
    // `@range`, so the model cannot bound them and this must, before any
    // arithmetic reads them.
    for (member, value) in [("StartTime", start_time), ("EndTime", end_time)] {
        let seconds = value.secs();
        if !(-MAX_TIMESTAMP_MAGNITUDE..=MAX_TIMESTAMP_MAGNITUDE).contains(&seconds) {
            return Err(format!(
                "{member} must be within {MAX_TIMESTAMP_MAGNITUDE} seconds of the epoch \
                 for its window to be computable; got {seconds}."
            ));
        }
    }

    let span_seconds = (end_time.secs() - start_time.secs()).max(1);
    let period = match period_in_seconds {
        None => choose_period(span_seconds),
        Some(requested) => {
            let requested = i64::from(requested);
            if !VALID_PERIODS_IN_SECONDS.contains(&requested) {
                return Err(format!(
                    "PeriodInSeconds must be one of {}; got {requested}.",
                    VALID_PERIODS_IN_SECONDS
                        .iter()
                        .map(i64::to_string)
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            requested
        }
    };

    // `AlignedStartTime` is documented to be less than or equal to the
    // requested `StartTime`, and `AlignedEndTime` greater than or equal to the
    // requested `EndTime`, each on a boundary of the effective period.
    let start = align_floor(start_time.secs(), period);
    let end_seconds = end_time.secs() + i64::from(end_time.subsec_nanos() > 0);
    let end = align_ceil(end_seconds, period);

    // An operation that MATERIALISES something per period bounds the count
    // itself ([`data_point_budget_refusal`]); the window arithmetic here is
    // total for every value this far, so a window is not refused for its width
    // by an operation that reports no series over it.
    Ok(AlignedWindow { period, start, end })
}

/// Why a response carrying `series` series over `window` cannot be
/// materialised, or `None` when it fits [`MAX_DATA_POINTS_PER_RESPONSE`].
///
/// Asked BEFORE any point is built, so a request past the budget allocates
/// nothing: the grid is the one thing in this service whose size a caller
/// chooses, and `MaxResults` bounds the number of metric KEYS rather than the
/// points under each of them.
///
/// The budget is taken against AT LEAST ONE series, whatever `series` says,
/// because that is what makes it total: a page carrying no key at all still
/// computes the window, and `periods × 0` would be inside any budget however
/// wide the window a caller asked for. So this answers `Some` for every window
/// whose own period count is past the budget, and a caller that proceeds on
/// `None` knows `window.period_count()` fits it.
pub fn data_point_budget_refusal(window: AlignedWindow, series: usize) -> Option<String> {
    let series = series.max(1);
    let periods = window.period_count();
    let points = periods.saturating_mul(i64::try_from(series).unwrap_or(i64::MAX));
    (points > MAX_DATA_POINTS_PER_RESPONSE).then(|| {
        format!(
            "this request asks for {series} series of {periods} data points each, {points} in \
             all; this emulator materialises at most {MAX_DATA_POINTS_PER_RESPONSE} data points \
             per response, so page the metric queries with MaxResults, ask for a shorter window, \
             or ask for a coarser PeriodInSeconds."
        )
    })
}

/// Align `seconds` DOWN to the nearest multiple of `period` — the granular
/// boundary at or before the requested start.
fn align_floor(seconds: i64, period: i64) -> i64 {
    seconds - seconds.rem_euclid(period)
}

/// Align `seconds` UP to the nearest multiple of `period` — the granular
/// boundary at or after the requested end.
fn align_ceil(seconds: i64, period: i64) -> i64 {
    let remainder = seconds.rem_euclid(period);
    if remainder == 0 {
        seconds
    } else {
        seconds + (period - remainder)
    }
}

/// The period the service picks for a window of `span_seconds` when the caller
/// specified none: the finest documented granularity that keeps the response
/// inside [`PERIOD_CHOICE_MAX_POINTS`] data points, and the coarsest one for a
/// window too long for any of them.
fn choose_period(span_seconds: i64) -> i64 {
    let coarsest = *VALID_PERIODS_IN_SECONDS
        .last()
        .expect("the documented period set is not empty");
    VALID_PERIODS_IN_SECONDS
        .iter()
        .copied()
        .find(|period| {
            let points = span_seconds / period + i64::from(span_seconds % period != 0);
            points <= PERIOD_CHOICE_MAX_POINTS
        })
        .unwrap_or(coarsest)
}

// ---------------------------------------------------------------------------
// Identity-based authorization
// ---------------------------------------------------------------------------

/// The IAM action namespace of this service: the model's `arnNamespace`, which
/// is also the prefix every `pi:` action and every `pi` ARN carries.
pub const SERVICE_ACTION_PREFIX: &str = "pi";

/// The resource ONE request addresses, as the authorization decision needs it.
#[derive(Debug, Clone)]
pub struct RequestedResource {
    /// The `pi` ARN a policy statement's `Resource` is matched against.
    /// Performance Insights documents two resource types — `metrics` and
    /// `perf-reports` — and an operation addresses whichever of them it names.
    pub arn: String,
    /// What a refusal names the resource as: the `Identifier` the request
    /// carries, which is how the recorded `pi` denial names it.
    pub named: String,
}

/// The `AnalysisReportId` segment that stands for every report of a data
/// source, for an operation that addresses the whole collection: IAM's own
/// wildcard, so a policy scoped to one report does not authorize a listing of
/// them all.
pub const ANY_REPORT: &str = "*";

/// The `metrics` resource a time-series request addresses:
/// `arn:aws:pi:<region>:<account>:metrics/<ServiceType>/<Identifier>`.
pub fn metrics_resource(
    parts: &::http::request::Parts,
    service_type: &str,
    identifier: &str,
) -> RequestedResource {
    RequestedResource {
        arn: ::emulator_server::emulator_arn(
            SERVICE_ACTION_PREFIX,
            &request_region(parts),
            &format!("metrics/{service_type}/{identifier}"),
        ),
        named: identifier.to_string(),
    }
}

/// The `perf-reports` resource a report request addresses. `analysis_report_id`
/// is [`ANY_REPORT`] for an operation that addresses the whole collection
/// rather than one report.
pub fn report_resource(
    parts: &::http::request::Parts,
    service_type: &str,
    identifier: &str,
    analysis_report_id: &str,
) -> RequestedResource {
    RequestedResource {
        arn: report_arn(
            &request_region(parts),
            service_type,
            identifier,
            analysis_report_id,
        ),
        named: identifier.to_string(),
    }
}

/// The resource a TAG request addresses: the `ResourceARN` it carries.
///
/// A tag operation names its resource by ARN rather than by the
/// `(ServiceType, Identifier, AnalysisReportId)` triple, so the ARN a policy
/// statement's `Resource` is matched against is that same ARN — and it is also
/// what a refusal names the resource as, because it is how the request named it.
pub fn tagged_resource(resource_arn: &str) -> RequestedResource {
    RequestedResource {
        arn: resource_arn.to_string(),
        named: resource_arn.to_string(),
    }
}

/// Why the caller may not perform `action` on `resource`, or `None` when its
/// identity-based policies allow it.
///
/// AWS evaluates the caller's identity-based policies in FRONT of the service,
/// so this is the first thing an operation asks and the answer does not depend
/// on anything the request names existing: the refusal is built from the
/// request's own principal, action and resource.
///
/// What is evaluated is the identity the REQUEST's signing key belongs to:
///
/// * an access key `iam:CreateAccessKey` minted for an IAM user — that user's
///   inline policies;
/// * a session `sts:AssumeRole` minted from a role — that role's inline
///   policies and the customer-managed policies attached to it;
/// * anything else (the emulator's own long-term credentials, a federated or
///   session token naming no role) is the ambient account identity, which no
///   identity-based policy constrains, so it is served.
///
/// The DECISION itself is not made here: the documents are collected here and
/// evaluated by [`::service_common::iam_authorization`], the tree's one reading
/// of an IAM policy document, so this service cannot answer a document
/// differently from any other service asked about it. What is local is which
/// documents are in force on this caller and how the refusal reads.
///
/// A policy this account does not hold the document of — an AWS-managed policy,
/// whose catalogue this emulator deliberately does not carry — leaves the
/// decision UNKNOWN rather than a denial: refusing a caller because the
/// emulator cannot read a document AWS owns would invent a denial exactly as
/// inventing the grant would invent one. So a caller holding such an attachment
/// is served when nothing else about the request is stated, while an explicit
/// `Deny` the shared evaluator DID read still refuses.
///
/// A denial is recorded as an audit event — the principal, the action, the
/// resource and the decision, none of them a secret — because an authorization
/// decision is the kind of outcome a caller has to be able to attribute to the
/// request that caused it.
pub fn identity_based_denial<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    action: &str,
    resource: &RequestedResource,
) -> Option<String> {
    use ::service_common::iam_authorization as iam_authz;

    let caller = caller_identity(engine, ctx)?;
    let full_action = format!("{SERVICE_ACTION_PREFIX}:{action}");
    // Identity-based statements name no `Principal`, so the principal list is
    // the caller's own ARN and is consulted by nothing here; it is what the
    // shared request carries for a resource-based policy.
    let principals = [caller.arn.clone()];
    let request = iam_authz::Request::new(&full_action, &resource.arn, &principals);
    let (outcome, message) = match iam_authz::identity_decision(&caller.documents, &request) {
        iam_authz::Decision::Allowed => return None,
        // Nothing the emulator can read grants it, but a document AWS owns may:
        // served rather than refused on a document this emulator cannot hold.
        iam_authz::Decision::NotStated if caller.unreadable_attachment => return None,
        iam_authz::Decision::NotStated => (
            "implicit-deny",
            format!(
                "User: {} is not authorized to perform: {full_action} on \
                 resource: {} because no identity-based policy allows the \
                 {full_action} action",
                caller.arn, resource.named
            ),
        ),
        iam_authz::Decision::Denied => (
            "explicit-deny",
            format!(
                "User: {} is not authorized to perform: {full_action} on \
                 resource: {} with an explicit deny in an identity-based policy",
                caller.arn, resource.named
            ),
        ),
    };
    ::tracing::info!(
        audit = true,
        operation = %action,
        outcome = %outcome,
        principal = %caller.arn,
        resource = %resource.arn,
        "request refused: no identity-based policy allows it"
    );
    Some(message)
}

/// An IAM identity a request's signing key belongs to, with the identity-based
/// policy documents attached to it.
struct CallerIdentity {
    /// The principal ARN a refusal names.
    arn: String,
    /// The documents in force on this identity, for the shared evaluator.
    documents: Vec<String>,
    /// Whether the identity carries an attachment whose document AWS owns, so a
    /// grant this emulator cannot read may exist.
    unreadable_attachment: bool,
}

/// The IAM identity this request's signing key belongs to, or `None` for the
/// ambient account identity (which no identity-based policy constrains).
///
/// The session identity a key speaks for is resolved by STS's own
/// [`sts_state::resolve_caller`], so this service reads a credential exactly as
/// the service that minted it does — including materialising the expiries the
/// clock has already made due.
fn caller_identity<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) -> Option<CallerIdentity> {
    let access_key_id = ::engine::extract_akid(&ctx.parts().headers);
    // Without an IAM store there is no identity this account minted, so there
    // is nothing to evaluate and nothing to refuse.
    let iam = iam_state::service_state(engine)?;
    // Taken and RELEASED before the IAM store is locked, so no handler holds
    // both at once.
    let session = sts_state::service_state(engine).map(|sts| {
        sts_state::resolve_caller(
            &mut sts.lock(),
            &access_key_id,
            ::std::time::SystemTime::now(),
            ctx.credential_store(),
        )
    });
    let iam = iam.lock();

    let session_arn = match session {
        // A credential past its promised lifetime is evaluated as an identity
        // with NO policy: the role may well allow the action, but this
        // credential no longer speaks for the role.
        Some(Err(expired)) => {
            return Some(CallerIdentity {
                arn: format!("an expired session credential ({})", expired.access_key_id),
                documents: Vec::new(),
                unreadable_attachment: false,
            })
        }
        Some(Ok(identity)) => Some(identity.arn),
        // No STS store on this engine: the key names no session.
        None => None,
    };

    // An assumed-role session is decided by its ROLE's policies. Any other
    // session identity STS resolves (the ambient one, a federated user) names
    // no role, so it falls through to the access-key binding below.
    let assumed = session_arn.and_then(|arn| {
        let role_name = assumed_role_name(&arn)?.to_string();
        Some((arn, role_name))
    });
    if let Some((session_arn, role_name)) = assumed {
        let role_key = iam_state::name_key(&role_name);
        let mut documents: Vec<String> = iam
            .role_inline_policies
            .get(&role_key)
            .map(|named| {
                named
                    .values()
                    .map(|policy| policy.policy_document.clone())
                    .collect()
            })
            .unwrap_or_default();
        let mut unreadable_attachment = false;
        for arn in iam
            .role_attached_policies
            .get(&role_key)
            .into_iter()
            .flatten()
        {
            match managed_policy_document(&iam, arn) {
                Some(document) => documents.push(document),
                // Only an AWS-MANAGED policy is a grant this emulator cannot
                // read: an attachment naming a customer-managed policy the
                // account does not hold grants nothing.
                None if is_aws_managed_policy(arn) => unreadable_attachment = true,
                None => {}
            }
        }
        // The principal a refusal names is the SESSION's own ARN, the identity
        // `sts:GetCallerIdentity` answers for this credential.
        return Some(CallerIdentity {
            arn: session_arn,
            documents,
            unreadable_attachment,
        });
    }

    let record = iam.access_keys.get(&access_key_id)?;
    // A key whose user record is gone names an identity that no longer holds
    // any policy, so it is evaluated as an identity with none rather than
    // falling through to the ambient account identity: a dangling binding must
    // not widen what a key may do.
    let arn = match iam.users.get(&record.user_key) {
        Some(user) => user.arn.clone(),
        None => user_arn_of_key(&record.user_key),
    };
    // `AttachUserPolicy` is not implemented in this emulator, so a user's
    // identity-based policies are exactly its inline ones.
    let documents = iam
        .user_inline_policies
        .get(&record.user_key)
        .map(|named| {
            named
                .values()
                .map(|policy| policy.policy_document.clone())
                .collect()
        })
        .unwrap_or_default();
    Some(CallerIdentity {
        arn,
        documents,
        unreadable_attachment: false,
    })
}

/// Whether `arn` names an AWS-MANAGED policy: one whose account segment is
/// IAM's `aws`, so it belongs to AWS rather than to this account and the IAM
/// store holds no document for it. Read through IAM's own ARN parser, so this
/// service and IAM agree on what an AWS-managed policy ARN is.
fn is_aws_managed_policy(arn: &str) -> bool {
    iam_state::parse_policy_arn(arn)
        .is_some_and(|policy| policy.account == iam_state::AWS_MANAGED_POLICY_ACCOUNT)
}

/// The ARN of the IAM user an access-key binding names, for a binding whose
/// user record the account no longer holds.
///
/// Built the way IAM builds a user's ARN at the root path
/// (`arn:aws:iam::<account>:user/<name>`), from the only thing the binding
/// keeps: the case-folded user name. It names the principal in a refusal and
/// authorizes nothing.
fn user_arn_of_key(user_key: &str) -> String {
    ::emulator_server::emulator_arn("iam", "", &format!("user/{user_key}"))
}

/// The role name an STS assumed-role session ARN names.
///
/// `sts:AssumeRole` reports the session as
/// `arn:aws:sts::<account>:assumed-role/<RoleName>/<SessionName>`
/// ([`sts_state::sts_arn`]), so the role is the first segment after
/// `assumed-role/`. `None` for any other ARN form — a federated user, say,
/// which no role's policies decide.
fn assumed_role_name(arn: &str) -> Option<&str> {
    let resource = arn.splitn(6, ':').nth(5)?;
    let name = resource.strip_prefix("assumed-role/")?.split('/').next()?;
    (!name.is_empty()).then_some(name)
}

/// The document of the managed policy `arn` names, or `None` when this account
/// does not hold one (an AWS-managed policy, which belongs to AWS).
fn managed_policy_document(
    iam: &iam_state::ComAmazonawsIamAwsidentitymanagementv20100508State,
    arn: &str,
) -> Option<String> {
    let held = iam.policy_by_arn(arn)?;
    let default_version = held.policy.default_version_id.as_deref()?;
    held.versions
        .get(default_version)?
        .document
        .clone()
}

/// The SHARED data-source directory in the shared engine: call it with a
/// handler's `&*self.engine`.
///
/// A data source is minted by a database control plane (Amazon RDS, Amazon
/// DocumentDB) and monitored here, so which service owns a resource id is ONE
/// record under one shared type —
/// [`::service_common::db_data_sources::DataSourceDirectory`] — and not a second
/// copy inside [`ComAmazonawsPiPerformanceinsightsv20180227State`]: the engine
/// keys state by TYPE, so a copy of its own here would be invisible to the
/// services that mint the ids.
///
/// LOCK ORDER: this directory is taken and RELEASED before [`service_state`] is
/// locked (see [`service_type_refusal`], its only reader here), so a handler
/// never holds both at once and the writers' order — their own store first, this
/// one second — cannot invert against it.
pub fn data_source_directory<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<DataSourceDirectory>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<DataSourceDirectory>)
}
