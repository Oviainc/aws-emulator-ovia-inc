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
//! The data sources themselves are not stored here either — no Performance
//! Insights operation registers one, they are the DB resources other services
//! own. Two questions are asked about them, and they have different answers:
//!
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
