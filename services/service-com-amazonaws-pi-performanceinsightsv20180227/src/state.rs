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
//! window, its effective period and its metric keys from the request, and
//! reports no data points at all, so reading metrics never writes.
//!
//! The data sources themselves are not stored either — no Performance Insights
//! operation registers one, they are the DB resources other services own — so
//! whether a request addresses a data source this account may see is decided by
//! [`is_data_source_identifier`], the authorization gate every operation in this
//! crate applies before it answers.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};

use ::aws_smithy_types::DateTime;

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
    /// The metadata Create persisted, in the order the caller sent it.
    pub tags: Vec<Tag>,
    /// Creation sequence within this store, used as the stable tiebreak when two
    /// reports of one resource analyse windows with the same start time.
    pub sequence: u64,
    /// The insights the analysis produced. The emulator has no DB session data
    /// to analyse, so this stays empty rather than carrying invented findings.
    pub insights: Vec<Insight>,
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
    /// Tags by resource ARN, written by Create so the Tag/Untag/ListTagsForResource
    /// slice has one home for them.
    pub tags_by_arn: HashMap<String, BTreeMap<String, String>>,
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
    /// exactly as it was. One report costs one `reports` entry and one
    /// `tags_by_arn` entry, and `remove_report` drops both, so bounding the
    /// reports bounds both maps.
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

    /// Remove one report, and the tags filed under its ARN. `false` when the
    /// resource holds no report with that id.
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
        let removed = reports.remove(position);
        self.tags_by_arn.remove(&removed.resource_arn);
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
