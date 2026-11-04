// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-entityresolution-awsveniceservice service.
//!
//! The control-plane collections are persisted here, and every member of each
//! record is the MODELLED type of the member it is returned on, so a reader clones
//! what the writer stored and the two cannot drift: `CreateSchemaMapping` followed by
//! `GetSchemaMapping` returns the created record field-for-field, and so does
//! `CreateIdNamespace` followed by `GetIdNamespace`. Beside them sits the data-plane
//! record this service's real-time matching pair shares: the match index
//! `GenerateMatchId` writes and `GetMatchId` reads.
//!
//! The provider-service collection — [`ProviderService`] — has no create API at all: a
//! provider
//! service is AWS-MANAGED catalog data, "the ProviderServices that are available in
//! this Amazon Web Services Region"
//! (`smithy_query(query="operation",operation="ListProviderServices")`), so the store
//! is SEEDED with the catalog when it is first touched (see [`provider_service_catalog`])
//! and no handler writes to it. `ListProviderServices` pages over exactly those
//! records and `GetProviderService` reads one of them, so the two cannot disagree.
//! Every part of an entry is catalog data, the Region its ARN names included
//! ([`ProviderService::region`]), so neither read composes anything of the request
//! into what it serves.
//!
//! `hasWorkflows` is deliberately NOT a stored field. It is a property of the
//! relationship between the two maps — "is this schema mapping referenced by some
//! matching workflow's `inputSourceConfig`" — so storing it would let a workflow
//! create or delete leave a stale flag behind. It is derived on read by
//! [`ComAmazonawsEntityresolutionAwsveniceserviceState::schema_has_workflows`].
//!
//! Beside the control-plane resources this module owns the MATCHING JOBS a workflow
//! has been run with ([`MatchingJob`]), and the clock-driven status lifecycle every
//! reader of one materialises ([`MatchingJob::materialize_due`]).
//!
//! The RESOURCE-BASED POLICIES ([`ResourcePolicy`]) are keyed by the ARN of the
//! resource they are attached to, because one policy API covers all four resource
//! types a `VeniceGlobalArn` names. The policy DOCUMENT is stored as the writer left
//! it — `PutPolicy` writes one verbatim, and `AddPolicyStatement` /
//! `DeletePolicyStatement` change its `Statement` list and leave the rest of it alone
//! ([`policy_document_parts`], [`render_policy_document`]) — so a reader serves the
//! document the last write produced and the two cannot drift.

/// Whether IAM lets the principal a request speaks for perform the operation it asks for:
/// the decision every operation whose model declares `AccessDeniedException` takes before
/// it reads or writes anything.
pub mod authorization;

use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

use ::aws_smithy_types::Document;
use sha2::{Digest, Sha256};

use crate::ops::types::{
    IdMappingIncrementalRunConfig, IdMappingTechniques, IdMappingWorkflowInputSource,
    IdMappingWorkflowOutputSource, IdNamespaceIdMappingWorkflowProperties, IdNamespaceInputSource,
    IncrementalRunConfig, InputSource, JobOutputSource, OutputSource, ProviderComponentSchema,
    ProviderEndpointConfiguration, ProviderIdNameSpaceConfiguration,
    ProviderIntermediateDataAccessConfiguration, ProviderMarketplaceConfiguration,
    ProviderSchemaAttribute, ResolutionTechniques, Rule, SchemaInputAttribute,
};

/// One schema mapping as this service persists it.
///
/// `description` is `Option<String>` because the member is optional on the create
/// request and on `GetSchemaMapping`'s output; `CreateSchemaMapping`'s output models
/// it @required, which is served as the empty string when the create carried none.
#[derive(Debug, Clone)]
pub struct SchemaMapping {
    /// The name the schema mapping is keyed by, as the caller supplied it.
    pub schema_name: String,
    /// The ARN minted when the schema mapping was created.
    pub schema_arn: String,
    pub description: Option<String>,
    pub mapped_input_fields: Vec<SchemaInputAttribute>,
    pub created_at: ::aws_smithy_types::DateTime,
    pub updated_at: ::aws_smithy_types::DateTime,
    pub tags: Option<HashMap<String, String>>,
}

/// One matching workflow as this service persists it.
#[derive(Debug, Clone)]
pub struct MatchingWorkflow {
    /// The name the workflow is keyed by, as the caller supplied it.
    pub workflow_name: String,
    /// The ARN minted when the workflow was created.
    pub workflow_arn: String,
    pub description: Option<String>,
    pub input_source_config: Vec<InputSource>,
    pub output_source_config: Vec<OutputSource>,
    pub resolution_techniques: ResolutionTechniques,
    pub incremental_run_config: Option<IncrementalRunConfig>,
    pub role_arn: String,
    pub created_at: ::aws_smithy_types::DateTime,
    pub updated_at: ::aws_smithy_types::DateTime,
    pub tags: Option<HashMap<String, String>>,
}

/// One customer record a rule-based matching workflow has already processed, as the
/// workflow's match index holds it.
///
/// `GenerateMatchId` is what puts a record here ("For existing records, it retrieves
/// their Match IDs and associated rules. For records without matches, it generates new
/// Match IDs"), and `GetMatchId` is the reader of the same index ("Returns the
/// corresponding Match ID of a customer record if the record has been processed in a
/// rule-based matching workflow"). Both the `matchId` and the `matchRule` are STORED,
/// never recomputed per call: that is what makes a second call for the same record
/// answer the same `matchId`.
#[derive(Debug, Clone)]
pub struct IndexedRecord {
    /// `Record$inputSourceARN`, as the caller submitted it.
    pub input_source_arn: String,
    /// `Record$uniqueId` — the record's identity inside the workflow, and the key this
    /// record is held under.
    pub unique_id: String,
    /// `Record$recordAttributeMap`, as the caller submitted it: the raw values, so a
    /// later comparison can choose whether to normalize them.
    pub record_attribute_map: HashMap<String, String>,
    /// The `matchId` of the group this record belongs to.
    pub match_id: String,
    /// The `ruleName` of the rule that joined this record to its group, and the empty
    /// string when no rule did — a record that matched nothing opens its own group, so
    /// there is no rule to name. `MatchGroup$matchRule` is modelled @required, so that
    /// case is served as the empty string, the same convention this service already
    /// uses for an @required output member the request carried nothing for.
    pub match_rule: String,
}

/// One ID namespace as this service persists it.
///
/// Every optional member is `Option<..>` on both the create output and
/// `GetIdNamespace`'s output, so what the create stored is exactly what a reader
/// serves: a namespace created without a `roleArn` reads back without one, rather
/// than with an invented value.
#[derive(Debug, Clone)]
pub struct IdNamespace {
    /// The name the ID namespace is keyed by, as the caller supplied it.
    pub id_namespace_name: String,
    /// The ARN minted when the ID namespace was created.
    pub id_namespace_arn: String,
    pub description: Option<String>,
    pub input_source_config: Option<Vec<IdNamespaceInputSource>>,
    pub id_mapping_workflow_properties: Option<Vec<IdNamespaceIdMappingWorkflowProperties>>,
    /// `IdNamespaceType`, one of `SOURCE` | `TARGET`
    /// (`smithy_query(query="shape",shape="IdNamespaceType")`). The modelled enum is
    /// refused at the boundary, so a stored value is always one of the two.
    pub r#type: String,
    pub role_arn: Option<String>,
    pub created_at: ::aws_smithy_types::DateTime,
    pub updated_at: ::aws_smithy_types::DateTime,
    pub tags: Option<HashMap<String, String>>,
}

/// The `JobStatus` values, as `smithy_query(query="shape",shape="JobStatus")` gives
/// them: `RUNNING | SUCCEEDED | FAILED | QUEUED`.
///
/// A job's status is one of these strings and nothing else; the two terminal ones are
/// listed again in [`TERMINAL_JOB_STATUSES`], which is the single definition of
/// "nothing further is owed from here".
pub mod job_status {
    pub const QUEUED: &str = "QUEUED";
    pub const RUNNING: &str = "RUNNING";
    pub const SUCCEEDED: &str = "SUCCEEDED";
    pub const FAILED: &str = "FAILED";
}

/// The `JobStatus` values no clock-owed transition leaves: a job in one of these keeps
/// its status, its `endTime` and its `errorDetails` forever.
pub const TERMINAL_JOB_STATUSES: &[&str] = &[job_status::SUCCEEDED, job_status::FAILED];

/// True when `status` is one no clock-owed transition leaves.
pub fn is_terminal_job_status(status: &str) -> bool {
    TERMINAL_JOB_STATUSES.contains(&status)
}

/// How long a matching job spends in each non-terminal status before the clock owes it
/// the next one.
///
/// A CHOSEN span — AWS documents no duration for a matching job, whose real runtime
/// depends on the input size — inside the 250–750 ms window this emulator fixes for a
/// clock-owed transition, so a caller that reads the job straight after
/// `StartMatchingJob` still observes the `QUEUED` it was started in, and the span is
/// never zero.
pub const JOB_STATUS_TRANSITION: Duration = Duration::from_millis(500);

/// Why a matching job cannot reach `SUCCEEDED` in this emulator, reported on the
/// modelled `ErrorDetails$errorMessage` of the `FAILED` job.
///
/// A matching job reads the workflow's input records out of its `inputSourceConfig`
/// (an S3 or Glue location), resolves entities across them with Entity Resolution's
/// matching engine, and writes matched records to `outputSourceConfig`. This emulator
/// has no such engine and no access to those locations, so the job is accepted,
/// tracked and readable, and the execution it cannot perform ends in the service's own
/// modelled failure status (`FAILED`, carrying this reason) rather than in a
/// `SUCCEEDED` with record counts nothing computed.
pub const JOB_NOT_EXECUTED_MESSAGE: &str =
    "Entity Resolution matching is not available in this emulator: no matching engine is \
     attached, so the job could not read the workflow's input sources or write matched \
     records to its output source.";

/// How many jobs the store retains per matching workflow.
///
/// A RETENTION policy rather than a refusal: a workflow can be run any number of
/// times, so `StartMatchingJob` drops that workflow's OLDEST jobs that are no longer
/// in progress once it would hold more than this, exactly as a real service ages out
/// old job history, and the store cannot grow without limit however many starts a
/// caller issues. A job still in progress is never dropped, and an aged-out job reads
/// as the modelled "no such job" exactly as a job id that never existed does.
pub const MAX_JOBS_PER_WORKFLOW: usize = 50;

/// One run of a matching workflow as this service persists it.
///
/// Every timestamp is epoch MILLISECONDS, the same precision [`now`] truncates to, so
/// a stored instant both compares against the clock and round-trips through the
/// modelled `Timestamp` members unchanged.
#[derive(Debug, Clone)]
pub struct MatchingJob {
    /// The `jobId` minted when the job was started: 32 lowercase hex characters, as
    /// the modelled `JobId` (`^[a-f0-9]{32}$`) requires. See [`new_job_id`].
    pub job_id: String,
    /// The workflow this job runs, i.e. the `workflowName` the start call named.
    pub workflow_name: String,
    /// One of [`job_status`].
    pub status: String,
    /// `startTime`: the instant the job was started, in epoch milliseconds.
    pub start_time: i64,
    /// `endTime`: set only once the status is terminal.
    pub end_time: Option<i64>,
    /// `errorDetails.errorMessage`: set only on a `FAILED` job.
    pub error_message: Option<String>,
    /// `outputSourceConfig`, as it was derived from the workflow when the job started
    /// (see [`job_output_source_config`]). It is kept ON THE JOB, not re-derived on
    /// read, because a job reports where THAT RUN would write — a later
    /// `UpdateMatchingWorkflow` must not rewrite the history of a job already started.
    pub output_source_config: Option<Vec<JobOutputSource>>,
    /// The instant the next status transition becomes due, while one is owed.
    pub next_transition_at: Option<i64>,
}

impl MatchingJob {
    /// Apply every status transition the clock has ALREADY made due at `now` (epoch
    /// milliseconds).
    ///
    /// The lifecycle is clock-driven, so each reader materialises it before answering
    /// and nothing sleeps: `QUEUED` -> `RUNNING` -> `FAILED`, the last carrying
    /// [`JOB_NOT_EXECUTED_MESSAGE`]. A transition that is not due yet is left alone,
    /// and a terminal job is never touched again — so a read only ever writes what was
    /// already owed at the same instant had nobody called.
    pub fn materialize_due(&mut self, now: i64) {
        while let Some(due) = self.next_transition_at {
            if is_terminal_job_status(&self.status) {
                // Nothing is owed from a terminal status: drop any stale due instant.
                self.next_transition_at = None;
                return;
            }
            if due > now {
                return;
            }
            let span = i64::try_from(JOB_STATUS_TRANSITION.as_millis()).unwrap_or(i64::MAX);
            match self.status.as_str() {
                job_status::QUEUED => {
                    self.status = job_status::RUNNING.to_string();
                    self.next_transition_at = Some(due.saturating_add(span));
                }
                _ => {
                    // `RUNNING`, the only other non-terminal `JobStatus`.
                    self.status = job_status::FAILED.to_string();
                    self.error_message = Some(JOB_NOT_EXECUTED_MESSAGE.to_string());
                    self.end_time = Some(due);
                    self.next_transition_at = None;
                }
            }
        }
    }

    /// True while this job holds its workflow's single run slot: any status that is not
    /// terminal.
    ///
    /// This is what `StartMatchingJob` refuses a concurrent start on (the
    /// "Workflow is currently running" conflict `ConflictException`'s own doc names),
    /// and what the [`MAX_JOBS_PER_WORKFLOW`] retention never drops.
    pub fn is_in_progress(&self) -> bool {
        !is_terminal_job_status(&self.status)
    }
}

/// The `outputSourceConfig` a job of `workflow` reports, derived from the workflow's
/// own `outputSourceConfig`.
///
/// `JobOutputSourceConfig` is `length 1..1`
/// (`smithy_query(query="shape",shape="JobOutputSourceConfig")`), so at most the
/// workflow's FIRST output source becomes the job's. A `JobOutputSource` is
/// `roleArn` + `outputS3Path` (both @required) + optional `KMSArn`: the role is the
/// workflow's `roleArn` — the role Entity Resolution assumes for the run — and the
/// path and KMS key are that output source's.
///
/// `OutputSource$outputS3Path` targets `OptionalS3Path`, whose pattern admits the
/// EMPTY string and whose default IS the empty string, while the job member's
/// `S3Path` is `length 1..1024` and must match `^s3://…`
/// (`smithy_query(query="shape",shape="OutputSource")`). A workflow configured with
/// no output path therefore yields no job output config at all, rather than an
/// invented path or an empty one the job member cannot hold.
pub fn job_output_source_config(workflow: &MatchingWorkflow) -> Option<Vec<JobOutputSource>> {
    let first = workflow.output_source_config.first()?;
    let output_s3_path = first
        .output_s3_path
        .clone()
        .filter(|path| !path.is_empty())?;
    Some(vec![JobOutputSource {
        role_arn: workflow.role_arn.clone(),
        output_s3_path,
        kms_arn: first.kms_arn.clone(),
    }])
}

/// One provider service as this service persists it: an entry of the AWS-managed
/// catalog of "the ProviderServices that are available in this Amazon Web Services
/// Region".
///
/// There is no API that creates, updates or deletes one, so this record is written
/// exactly once — when the store is seeded by [`provider_service_catalog`] — and only
/// ever read after that, by `ListProviderServices` and `GetProviderService`.
///
/// Every member is the MODELLED type of the member it is returned on, EXCEPT the ARN,
/// which is not stored as a string: `ProviderServiceArn` carries no account and names
/// a region
/// (`^arn:…:entityresolution:<region>::providerservice/<providerName>/<providerServiceName>$`),
/// so it is a pure function of the entry — provider token, service token and the
/// entry's OWN [`Self::region`] — see [`ProviderService::arn`].
#[derive(Debug, Clone)]
pub struct ProviderService {
    pub provider_name: String,
    pub provider_service_name: String,
    pub provider_service_display_name: String,
    /// A `ServiceType` value: `ASSIGNMENT` or `ID_MAPPING`.
    pub provider_service_type: String,
    /// The Region segment of this entry's `providerServiceArn`: a property of the
    /// catalog entry, not of the caller. The catalog is AWS-managed data published
    /// with each provider service, and one page of it carries entries whose ARNs name
    /// DIFFERENT Regions, so the segment cannot be the region a request was signed
    /// for. Matches the modelled ARN's `[a-z]{2}-[a-z]{1,10}-[0-9]` segment.
    pub region: String,
    pub anonymized_output: bool,
    pub provider_endpoint_configuration: ProviderEndpointConfiguration,
    pub provider_component_schema: ProviderComponentSchema,
    pub provider_configuration_definition: Option<Document>,
    pub provider_entity_output_definition: Document,
    pub provider_job_configuration: Option<Document>,
    pub provider_id_name_space_configuration: Option<ProviderIdNameSpaceConfiguration>,
    pub provider_intermediate_data_access_configuration:
        Option<ProviderIntermediateDataAccessConfiguration>,
}

impl ProviderService {
    /// The key this entry is stored and ordered under: `<providerName>/<providerServiceName>`.
    ///
    /// The pair is the catalog's identity — `GetProviderService` takes exactly those
    /// two — and the composite is also a legal `nextToken` (the modelled pattern
    /// `^[a-zA-Z_0-9-=+/]*$` admits `/`, and both halves are `EntityName`s).
    pub fn catalog_key(&self) -> String {
        format!("{}/{}", self.provider_name, self.provider_service_name)
    }

    /// This entry's `providerServiceArn`.
    ///
    /// The modelled `ProviderServiceArn` pattern
    /// (`smithy_query(query="shape",shape="ProviderServiceSummary")`) fixes the whole
    /// shape of it: an EMPTY account segment, then
    /// `providerservice/<providerName>/<providerServiceName>`. Every segment —
    /// including the Region — comes from the entry, so the ARN is the same whatever
    /// region a request is signed for, and the ARN `ListProviderServices` reports is
    /// exactly the one `GetProviderService` resolves.
    pub fn arn(&self) -> String {
        provider_service_arn(
            &self.region,
            &self.provider_name,
            &self.provider_service_name,
        )
    }
}

/// The `providerServiceArn` of the catalog entry `provider_name`/`provider_service_name` in
/// `region`.
///
/// The one speller of that ARN, so the entry's own [`ProviderService::arn`] and the resource
/// a provider-service read is AUTHORIZED against cannot disagree. The modelled
/// `ProviderServiceArn` pattern fixes its whole shape — an EMPTY account segment, because the
/// catalog is AWS-managed data rather than this account's, then
/// `providerservice/<providerName>/<providerServiceName>`.
pub fn provider_service_arn(
    region: &str,
    provider_name: &str,
    provider_service_name: &str,
) -> String {
    format!("arn:aws:entityresolution:{region}::providerservice/{provider_name}/{provider_service_name}")
}

/// One ID mapping workflow as this service persists it.
///
/// `role_arn` is a plain `String` rather than an `Option<String>` because
/// `IdMappingRoleArn` is modelled with `default: ""`
/// (`smithy_query(query="operation",operation="CreateIdMappingWorkflow")`), so a
/// request that carries no role is the empty string and not a distinct "absent"
/// state. `output_source_config` and `incremental_run_config` have NO modelled
/// default, so absent stays absent for them.
#[derive(Debug, Clone)]
pub struct IdMappingWorkflow {
    /// The name the workflow is keyed by, as the caller supplied it.
    pub workflow_name: String,
    /// The ARN minted when the workflow was created.
    pub workflow_arn: String,
    pub description: Option<String>,
    pub input_source_config: Vec<IdMappingWorkflowInputSource>,
    pub output_source_config: Option<Vec<IdMappingWorkflowOutputSource>>,
    pub id_mapping_techniques: IdMappingTechniques,
    pub incremental_run_config: Option<IdMappingIncrementalRunConfig>,
    pub role_arn: String,
    pub created_at: ::aws_smithy_types::DateTime,
    pub updated_at: ::aws_smithy_types::DateTime,
    pub tags: Option<HashMap<String, String>>,
}

/// The policy-language `Version` a `PolicyDocument` this service composes carries.
///
/// A resource-based policy is an IAM policy document — `AddPolicyStatement` composes
/// one from each statement's `effect`, `action`, `principal` and `condition`
/// (`smithy_query(query="operation",operation="AddPolicyStatement")`) — and
/// `2012-10-17` is the current version of that language, so what this service composes
/// is a document an IAM policy parser accepts. A document a `PutPolicy` caller wrote
/// keeps ITS own members: this is only what a composed one starts from.
pub const POLICY_VERSION: &str = "2012-10-17";

/// The two forms `StatementPrincipal` admits, and which `Principal` key each is
/// written under.
///
/// `AddPolicyStatement$principal` is documented as "The Amazon Web Services service or
/// Amazon Web Services account that can access the resource defined as ARN", i.e. ONE
/// member carrying both kinds, and its modelled pattern is exactly that alternation:
/// `^((\d{12})|([a-z0-9\.]+))$` (`length 12..64`,
/// `smithy_query(query="shape",shape="StatementPrincipalList")`) — a 12-digit account
/// id, or a lowercase dotted service name. An IAM policy document writes an account
/// principal under `Principal.AWS` and a service principal under `Principal.Service`,
/// so the branch of that alternation a value matches is what decides its key: a value
/// of exactly twelve ASCII digits is the account form, and every other value the
/// pattern admits is the service-name form.
fn is_service_principal(principal: &str) -> bool {
    let is_account_id =
        principal.len() == 12 && principal.bytes().all(|byte| byte.is_ascii_digit());
    !is_account_id
}

/// The longest `PolicyDocument` this service will store, and so the largest a policy
/// can grow to: the modelled maximum of the member it is served on (`PolicyDocument`,
/// `length 1..40960`, `smithy_query(query="shape",shape="PolicyDocument")`).
///
/// `PutPolicy` takes a whole document, which the boundary validator already holds to
/// that maximum; `AddPolicyStatement` GROWS one across calls out of values with no
/// modelled maximum of their own (an `action` or `principal` list is `length 1..`, a
/// `condition` is up to 40960 characters), so the add is where the bound has to be
/// checked: a policy that could not be served as a `PolicyDocument` is refused rather
/// than stored (see [`messages::policy_document_too_large`]). Together with
/// [`ComAmazonawsEntityresolutionAwsveniceserviceState::prune_orphaned_policies`] that
/// bounds the whole collection — at most one document's worth per live resource, and
/// every resource collection has its own quota.
pub const MAX_POLICY_DOCUMENT_LENGTH: usize = 40960;

/// The `Statement` member of a policy document: the list of statements
/// `AddPolicyStatement` appends to and `DeletePolicyStatement` removes from.
const POLICY_STATEMENT_MEMBER: &str = "Statement";

/// The `Version` member of a policy document, which carries [`POLICY_VERSION`].
const POLICY_VERSION_MEMBER: &str = "Version";

/// The `Sid` member of one statement, which is where that statement's `statementId`
/// lives.
const POLICY_SID_MEMBER: &str = "Sid";

/// A stored `PolicyDocument` taken apart into the object it is and its `Statement`
/// list — `None` when `document` is not a document this service can change: one that
/// is not a JSON object, or whose `Statement` member is neither absent nor an array.
///
/// Every OTHER member of the document (`Version`, `Id`, anything a `PutPolicy` caller
/// wrote) stays in the returned object, so adding or removing a statement preserves
/// the rest of the document instead of replacing it with a composed one.
pub fn policy_document_parts(
    document: &str,
) -> Option<(
    ::serde_json::Map<String, ::serde_json::Value>,
    Vec<::serde_json::Value>,
)> {
    let ::serde_json::Value::Object(mut object) = ::serde_json::from_str(document).ok()? else {
        return None;
    };
    let statements = match object.remove(POLICY_STATEMENT_MEMBER) {
        None => Vec::new(),
        Some(::serde_json::Value::Array(statements)) => statements,
        Some(_) => return None,
    };
    Some((object, statements))
}

/// The document `object` renders once `statements` is written back as its `Statement`
/// member: the counterpart of [`policy_document_parts`], and `None` only if the result
/// cannot be serialized at all.
pub fn render_policy_document(
    mut object: ::serde_json::Map<String, ::serde_json::Value>,
    statements: Vec<::serde_json::Value>,
) -> Option<String> {
    object.insert(
        POLICY_STATEMENT_MEMBER.to_string(),
        ::serde_json::Value::Array(statements),
    );
    ::serde_json::to_string(&::serde_json::Value::Object(object)).ok()
}

/// The document a resource carrying no policy yet starts from when a statement is
/// added to it: the policy language's `Version`, and nothing else.
pub fn new_policy_document() -> ::serde_json::Map<String, ::serde_json::Value> {
    let mut object = ::serde_json::Map::new();
    object.insert(
        POLICY_VERSION_MEMBER.to_string(),
        ::serde_json::Value::String(POLICY_VERSION.to_string()),
    );
    object
}

/// The `Sid` of one statement of a policy document, when it carries one.
///
/// `Sid` is where a statement's `statementId` lives, so this is what decides whether a
/// policy already holds the statement an add names, and which statement a delete
/// removes.
pub fn statement_sid(statement: &::serde_json::Value) -> Option<&str> {
    statement.get(POLICY_SID_MEMBER)?.as_str()
}

/// The partition of an ARN — the second colon-separated field of
/// `arn:<partition>:<service>:…` — defaulting to the commercial partition when the
/// value carries none.
///
/// A principal expanded into an ARN belongs to the SAME partition as the resource
/// whose policy names it: `VeniceGlobalArn` admits `aws`, `aws-us-gov` and `aws-cn`
/// (`smithy_query(query="operation",operation="AddPolicyStatement")`), so the
/// partition is read from the resource rather than hard-coded.
fn arn_partition(arn: &str) -> &str {
    let mut fields = arn.split(':');
    match (fields.next(), fields.next()) {
        (Some("arn"), Some(partition)) if !partition.is_empty() => partition,
        _ => "aws",
    }
}

/// One member of an IAM policy statement that is composed from a modelled LIST input.
///
/// A policy document admits either a single value or an array of them for these
/// members, and a document AWS writes carries the SCALAR when there is exactly one
/// value, so a statement composed here renders the same shape: a one-element list is
/// the value itself, and two or more stay an array in the order the request gave them.
/// That is what lets a client's policy parser or a document comparison see the same
/// structure it would get from the real service.
fn statement_member(values: &[String]) -> ::serde_json::Value {
    match values {
        [only] => ::serde_json::Value::String(only.clone()),
        several => ::serde_json::json!(several),
    }
}

/// One statement of a policy document, composed from the members `AddPolicyStatement`
/// carries plus the ARN of the resource the policy is attached to.
///
/// The members composed from the request are NORMALIZED into what an IAM policy
/// document holds, which is what the write stores and every later read serves:
///
/// * `Principal` — the modelled `StatementPrincipal` carries BOTH principal kinds in
///   one member, while an IAM document writes them under two keys, so they are split
///   here (see [`is_service_principal`]) and a kind no principal belongs to is left off
///   the block. An ACCOUNT principal is an identity, and the identity an account id
///   names in a policy document is that account's IAM root user, so a bare account id
///   is expanded to its root-user ARN; a service principal is already the name a
///   document holds and goes through as it stands.
/// * `Principal.AWS`, `Principal.Service` and `Action` — rendered through
///   [`statement_member`], so a single value is a scalar and several stay an array.
///
/// `Resource` is the policy's own resource, so a statement added through this API can
/// never name another one, and `Condition` is absent when the request carried none,
/// because an IAM statement without conditions carries no `Condition` block at all.
pub fn policy_statement(
    statement_id: &str,
    effect: &str,
    action: &[String],
    principal: &[String],
    condition: Option<&::serde_json::Value>,
    resource_arn: &str,
) -> ::serde_json::Value {
    let partition = arn_partition(resource_arn);
    // Split on the RAW values, which is what the modelled alternation classifies, and
    // expand the account branch afterwards.
    let (service, accounts): (Vec<&String>, Vec<&String>) = principal
        .iter()
        .partition(|principal| is_service_principal(principal));
    let service: Vec<String> = service.into_iter().cloned().collect();
    let aws: Vec<String> = accounts
        .into_iter()
        .map(|account| format!("arn:{partition}:iam::{account}:root"))
        .collect();
    let mut principal_block = ::serde_json::Map::new();
    if !aws.is_empty() {
        principal_block.insert("AWS".to_string(), statement_member(&aws));
    }
    if !service.is_empty() {
        principal_block.insert("Service".to_string(), statement_member(&service));
    }

    let mut statement = ::serde_json::Map::new();
    statement.insert(
        POLICY_SID_MEMBER.to_string(),
        ::serde_json::json!(statement_id),
    );
    statement.insert("Effect".to_string(), ::serde_json::json!(effect));
    statement.insert(
        "Principal".to_string(),
        ::serde_json::Value::Object(principal_block),
    );
    statement.insert("Action".to_string(), statement_member(action));
    statement.insert("Resource".to_string(), ::serde_json::json!(resource_arn));
    if let Some(condition) = condition {
        statement.insert("Condition".to_string(), condition.clone());
    }
    ::serde_json::Value::Object(statement)
}

/// One resource-based policy as this service persists it.
///
/// `PutPolicy`, `AddPolicyStatement` and `DeletePolicyStatement` all answer the same
/// two members beside the resource ARN — the policy document and the revision token —
/// and `GetPolicy` reads exactly those back, so one record serves the whole policy
/// surface and a reader cannot disagree with the writer.
#[derive(Debug, Clone)]
pub struct ResourcePolicy {
    /// `policy`: the resource-based policy document as the WRITE that produced this
    /// revision left it. A `PolicyDocument` is an opaque string on the wire
    /// (`smithy_query(query="shape",shape="PolicyDocument")`), so a `PutPolicy`
    /// caller's document is stored byte for byte and nothing reformats it on the way
    /// out; a document `AddPolicyStatement` composed is stored exactly as that write
    /// rendered it (see [`policy_statement`]), so every reader serves that one
    /// rendering.
    pub policy: String,
    /// `token`: "a unique identifier for the current revision of the policy"
    /// (`smithy_query(query="doc",shape="PutPolicyOutput",member="token")`), minted
    /// fresh by the write that produced this revision (see [`new_policy_token`]) and
    /// then stable for as long as that revision stands — so a reader reports the same
    /// token the write answered, and a write carrying an earlier revision's token is
    /// recognisable as stale.
    pub token: String,
    /// The `createdAt` of the resource this policy was attached to.
    ///
    /// A resource-based policy belongs to a RESOURCE, so deleting the resource takes
    /// its policy with it. The policy map is keyed by the resource's ARN, and an ARN is
    /// a function of the resource's name, so a resource that is deleted and then
    /// created again under the same name would otherwise inherit the deleted one's
    /// policy. Stamping the entry with the incarnation it belongs to is what keeps a
    /// re-created resource policy-less: an entry whose stamp is not the current
    /// resource's `createdAt` names a resource that no longer exists, so no reader
    /// serves it (see
    /// [`ComAmazonawsEntityresolutionAwsveniceserviceState::resource_policy`]).
    pub resource_created_at: ::aws_smithy_types::DateTime,
}

/// Per-service state for com-amazonaws-entityresolution-awsveniceservice. A handler reaches it through [`service_state`].
///
/// Every control-plane map is a `BTreeMap`, not a `HashMap`, because the list
/// operations are `@paginated` and a page boundary is only meaningful over a stable
/// order: the `nextToken` a page returns is the key of the first record it did NOT
/// return, so the next call resumes at that key. The match index is ordered for a
/// related reason: rule evaluation walks it, so an ordered index makes "the first
/// indexed record this rule matches" a well-defined record rather than whichever one
/// a hash happened to put first.
#[derive(Debug)]
pub struct ComAmazonawsEntityresolutionAwsveniceserviceState {
    /// Every schema mapping that exists, keyed by `schemaName`.
    pub schema_mappings: BTreeMap<String, SchemaMapping>,
    /// Every matching workflow that exists, keyed by `workflowName`.
    pub matching_workflows: BTreeMap<String, MatchingWorkflow>,
    /// Every ID namespace that exists, keyed by `idNamespaceName`.
    pub id_namespaces: BTreeMap<String, IdNamespace>,
    /// Every matching job that exists, keyed by its `jobId`, which is unique across
    /// workflows — the job's own `workflow_name` says which workflow it belongs to, so
    /// one map serves both the per-job read and the per-workflow listing.
    pub matching_jobs: BTreeMap<String, MatchingJob>,
    /// The Region's provider-service catalog, keyed by
    /// [`ProviderService::catalog_key`]. Seeded, never written by a handler.
    pub provider_services: BTreeMap<String, ProviderService>,
    /// Every ID mapping workflow that exists, keyed by `workflowName`.
    ///
    /// A SEPARATE namespace from [`Self::matching_workflows`]: the two resources have
    /// their own ARN types (`matchingworkflow/…` vs `idmappingworkflow/…`), their own
    /// create/get/list/delete operations and their own URI prefixes, so the same name
    /// can name one of each.
    pub id_mapping_workflows: BTreeMap<String, IdMappingWorkflow>,
    /// The records each matching workflow has processed: `workflowName` ->
    /// `uniqueId` -> the record. A workflow with no processed record has no entry.
    pub match_index: BTreeMap<String, BTreeMap<String, IndexedRecord>>,
    /// The resource-based policy attached to each resource that has one, keyed by the
    /// resource's OWN identity — the `<resourceType>/<name>` segment
    /// ([`policy_key`]), which is what makes one map serve all four resource types
    /// (`schemamapping`, `matchingworkflow`, `idmappingworkflow`, `idnamespace`) while
    /// keying each entry exactly as the collection holding that resource keys it.
    ///
    /// The key deliberately carries NO request-supplied segment. The `VeniceGlobalArn`
    /// a policy operation takes as its `@httpLabel` also carries a partition, account
    /// and region, and the region a request is served in comes from the caller's own
    /// SigV4 credential scope ([`request_region`]) rather than from the stored record —
    /// so keying by the ARN would let one live resource accumulate one entry per
    /// distinct region string a client signs with, none of them ever prunable. Keyed by
    /// the resource segment, an entry is the resource's single policy whatever region
    /// the write was signed for, exactly as the resource itself is one record whatever
    /// region created it.
    ///
    /// A resource with no policy has NO entry here, which is what distinguishes "the
    /// resource exists but carries no policy" from the policy a write attached to it.
    ///
    /// BOUNDED by the live resources, not by how many requests have run: a policy is
    /// not a resource of its own and dies with the resource it is attached to, so a
    /// write drops the entries whose resource is gone before it adds its own (see
    /// [`ComAmazonawsEntityresolutionAwsveniceserviceState::prune_orphaned_policies`]).
    /// That leaves at most one entry per live resource — and every resource collection
    /// here carries its own quota, which is what
    /// [`MAX_RESOURCE_POLICIES`] sums; a write refuses past that bound even if the
    /// pruning predicate were to regress.
    pub policies: BTreeMap<String, ResourcePolicy>,
}

/// `Default` is what the engine builds the store with on first touch, so it is where
/// the provider-service catalog is seeded: the customer-owned collections start empty,
/// and the AWS-managed one starts complete.
impl Default for ComAmazonawsEntityresolutionAwsveniceserviceState {
    fn default() -> Self {
        Self {
            schema_mappings: BTreeMap::new(),
            matching_workflows: BTreeMap::new(),
            id_namespaces: BTreeMap::new(),
            matching_jobs: BTreeMap::new(),
            provider_services: provider_service_catalog(),
            id_mapping_workflows: BTreeMap::new(),
            // The data plane starts empty too: a workflow has processed no record
            // until `GenerateMatchId` indexes one.
            match_index: BTreeMap::new(),
            policies: BTreeMap::new(),
        }
    }
}

/// One row of the provider-service catalog table: everything that differs between
/// entries. The rest of a [`ProviderService`] is DERIVED from these fields by
/// [`provider_service_catalog`], so no two members of one entry can disagree.
struct CatalogRow {
    /// `providerName` — "typically the company name", an `EntityName`
    /// (`^[a-zA-Z_0-9-]*$`), so it carries no space or dot however the company spells
    /// its name; the spelled-out form belongs in `providerServiceDisplayName`.
    provider_name: &'static str,
    /// `providerServiceName` — "the name of the product that the provider service
    /// provides", also an `EntityName`.
    provider_service_name: &'static str,
    display_name: &'static str,
    /// A modelled `ServiceType`: `ASSIGNMENT` (the provider assigns its own id to each
    /// record of a matching workflow) or `ID_MAPPING` (it maps ids between two
    /// namespaces). Those two are the whole enum
    /// (`smithy_query(query="shape",shape="ServiceType")`).
    service_type: &'static str,
    /// Whether the provider's output is anonymized, i.e. cannot be related back to the
    /// identifying input (`GetProviderService$anonymizedOutput`).
    anonymized_output: bool,
    /// The Region segment of this entry's ARN — see [`ProviderService::region`]. A
    /// property of the published entry, so entries of one catalog differ in it.
    region: &'static str,
    /// Whether this provider needs the caller to hand it an S3 bucket for intermediate
    /// data. `ProviderIntermediateDataAccessConfiguration` is documented as what "is
    /// required by SOME providers", so it is per-entry, and it is the same fact that
    /// decides whether the entry takes an intermediate-source configuration.
    intermediate_data_access: bool,
    /// The provider's input schema: `(SchemaAttributeType, hashing)` per
    /// `providerSchemaAttributes` member. The type doubles as the attribute's
    /// `fieldName`, because a provider service's published schema names its fields by
    /// the attribute type it accepts there (`providerComponentSchema.schemas` carries
    /// exactly those type tokens).
    attributes: &'static [(&'static str, bool)],
    /// The properties of the `providerConfiguration` document this provider service
    /// takes, each with whether it is required, in the order the configuration
    /// definition's `required` list names them. A provider service is reached through
    /// the provider's own API, so these are the credentials and options that API needs
    /// (`ProviderProperties$providerConfiguration`,
    /// `smithy_query(query="shape",shape="ProviderProperties")`).
    configuration_properties: &'static [(&'static str, bool)],
    /// The attribute the provider answers per input record: the provider's own
    /// identifier for the entity, which is what `providerEntityOutputDefinition`
    /// declares under its `entity` object.
    output_attribute: &'static str,
}

/// The `providerConfiguration` properties a provider service reached through an
/// OAuth-style client credential pair takes: the pair, both required.
const CLIENT_CREDENTIAL_PROPERTIES: &[(&str, bool)] = &[("client_id", true), ("client_secret", true)];

/// The provider-service catalog this emulator serves.
///
/// These are the provider services AWS Entity Resolution publishes, as a client sees
/// them on the wire: a LOWERCASE provider token (`liveramp`, `transunion`) and, per
/// provider, one entry per CAPABILITY (`assignment`, `assignment-pii`, `idmapping`),
/// each with the display name AWS publishes for it and the Region its own listing
/// lives in — which is why one page carries entries from more than one Region.
///
/// Which providers Entity Resolution integrates, and which `SchemaAttributeType`s
/// each of them accepts, is also spelled out per provider in the doc of
/// `ProviderSchemaAttribute$type`
/// (`smithy_query(query="shape",shape="ProviderSchemaAttribute")`): LiveRamp,
/// TransUnion and Unified ID 2.0. Each entry's attributes are taken from that
/// provider's own list there, and the table as a whole spans both `ServiceType`s, both
/// `anonymizedOutput` values, both Region values, a provider with several services and
/// providers with one.
const CATALOG: &[CatalogRow] = &[
    CatalogRow {
        provider_name: "liveramp",
        provider_service_name: "assignment",
        display_name: "Assignment Email",
        service_type: "ASSIGNMENT",
        region: "us-east-2",
        // LiveRamp answers a RampID, a pseudonymous identifier that cannot be related
        // back to the email that was sent in.
        anonymized_output: true,
        intermediate_data_access: true,
        // The email-only assignment service: one hashed `EMAIL_ADDRESS`, which
        // LiveRamp's documented list admits.
        attributes: &[("EMAIL_ADDRESS", true)],
        // LiveRamp's API takes a client credential pair and the resolution flow to run.
        configuration_properties: &[
            ("client_id", true),
            ("client_secret", true),
            ("resolution_flow", false),
        ],
        output_attribute: "RAMPID",
    },
    CatalogRow {
        provider_name: "liveramp",
        provider_service_name: "assignment-pii",
        display_name: "Assignment PII",
        service_type: "ASSIGNMENT",
        region: "us-east-2",
        anonymized_output: true,
        intermediate_data_access: true,
        // The PII assignment service: name, address, phone and email, each of them a
        // type LiveRamp's documented list admits, hashed where the provider takes a
        // hash.
        attributes: &[
            ("NAME_FIRST", false),
            ("NAME_LAST", false),
            ("ADDRESS", false),
            ("ADDRESS_CITY", false),
            ("ADDRESS_STATE", false),
            ("ADDRESS_POSTALCODE", false),
            ("PHONE_NUMBER", true),
            ("EMAIL_ADDRESS", true),
        ],
        configuration_properties: &[
            ("client_id", true),
            ("client_secret", true),
            ("resolution_flow", false),
        ],
        output_attribute: "RAMPID",
    },
    CatalogRow {
        provider_name: "liveramp",
        provider_service_name: "idmapping",
        display_name: "ID Mapping",
        service_type: "ID_MAPPING",
        region: "us-east-2",
        anonymized_output: true,
        intermediate_data_access: true,
        // An id-mapping service takes ids, not PII: `UNIQUE_ID` in and the provider's
        // own `PROVIDER_ID`, both of which LiveRamp's documented list admits.
        attributes: &[("UNIQUE_ID", false), ("PROVIDER_ID", false)],
        configuration_properties: &[
            ("client_id", true),
            ("client_secret", true),
            ("resolution_flow", false),
        ],
        output_attribute: "RAMPID",
    },
    CatalogRow {
        provider_name: "transunion",
        provider_service_name: "assignment",
        display_name: "TruAudience Identity Resolution & Enrichment",
        service_type: "ASSIGNMENT",
        region: "us-east-1",
        // An enrichment service answers attributes of the resolved entity, which do
        // relate back to the identifying input.
        anonymized_output: false,
        intermediate_data_access: false,
        // TransUnion's documented types, which — unlike LiveRamp's — include `DATE`,
        // `IPV4` and `MAID`.
        attributes: &[
            ("NAME_FIRST", false),
            ("NAME_LAST", false),
            ("ADDRESS", false),
            ("ADDRESS_CITY", false),
            ("ADDRESS_STATE", false),
            ("ADDRESS_POSTALCODE", false),
            ("PHONE_NUMBER", true),
            ("EMAIL_ADDRESS", true),
            ("DATE", false),
            ("IPV4", false),
            ("MAID", false),
        ],
        configuration_properties: CLIENT_CREDENTIAL_PROPERTIES,
        output_attribute: "TRUAUDIENCE_ID",
    },
    CatalogRow {
        provider_name: "unifiedid",
        provider_service_name: "idmapping",
        display_name: "Unified ID 2.0",
        service_type: "ID_MAPPING",
        region: "us-east-1",
        // UID2 answers with tokens rather than with anything that identifies the
        // input record, which is exactly what `anonymizedOutput` reports.
        anonymized_output: true,
        intermediate_data_access: false,
        // Unified ID 2.0's documented list is the whole of its schema: three types.
        attributes: &[
            ("UNIQUE_ID", false),
            ("EMAIL_ADDRESS", true),
            ("PHONE_NUMBER", true),
        ],
        configuration_properties: CLIENT_CREDENTIAL_PROPERTIES,
        output_attribute: "UID2",
    },
];

/// The catalog as the store holds it: [`CATALOG`] expanded into modelled records,
/// keyed and ordered by [`ProviderService::catalog_key`].
///
/// Every derived member is a function of the row, so the record is identical on every
/// boot: nothing here reads a clock, a request or a random source.
pub fn provider_service_catalog() -> BTreeMap<String, ProviderService> {
    CATALOG
        .iter()
        .map(|row| {
            let provider_service = ProviderService {
                provider_name: row.provider_name.to_string(),
                provider_service_name: row.provider_service_name.to_string(),
                provider_service_display_name: row.display_name.to_string(),
                provider_service_type: row.service_type.to_string(),
                region: row.region.to_string(),
                anonymized_output: row.anonymized_output,
                provider_endpoint_configuration:
                    ProviderEndpointConfiguration::marketplaceConfiguration(
                        marketplace_configuration(row),
                    ),
                provider_component_schema: component_schema(row),
                provider_configuration_definition: configuration_definition(row),
                provider_entity_output_definition: entity_output_definition(row),
                provider_job_configuration: job_configuration(row),
                provider_id_name_space_configuration: id_name_space_configuration(row),
                provider_intermediate_data_access_configuration:
                    intermediate_data_access_configuration(row),
            };
            (provider_service.catalog_key(), provider_service)
        })
        .collect()
}

/// The entry's Data Exchange identifiers (`ProviderMarketplaceConfiguration`).
///
/// AWS mints these four in Data Exchange, and this emulator has no Data Exchange, so
/// each one is [`data_exchange_id`] over the entry and the member name: a value in the
/// form a Data Exchange id takes, distinct per entry and per member, and identical on
/// every boot.
fn marketplace_configuration(row: &CatalogRow) -> ProviderMarketplaceConfiguration {
    ProviderMarketplaceConfiguration {
        data_set_id: data_exchange_id(row, "dataSetId"),
        revision_id: data_exchange_id(row, "revisionId"),
        asset_id: data_exchange_id(row, "assetId"),
        listing_id: data_exchange_id(row, "listingId"),
    }
}

/// A Data Exchange identifier for one member of one catalog entry: the first 16 bytes
/// of SHA-256 over `<providerName>/<providerServiceName>#<member>`, as 32 lowercase
/// hex characters — the form a Data Exchange data set, revision, asset or listing id
/// takes. Deterministic, so the id an entry reports never changes.
fn data_exchange_id(row: &CatalogRow, member: &str) -> String {
    let seed = format!(
        "{}/{}#{member}",
        row.provider_name, row.provider_service_name
    );
    hex::encode(&Sha256::digest(seed.as_bytes())[..16])
}

/// The entry's input schema (`ProviderComponentSchema`).
///
/// `providerSchemaAttributes` is the row's attribute list, and `schemas` — a list of
/// attribute-name lists — is the single schema those attributes make up, so the two
/// members cannot name different fields. A provider service's published schema names
/// each field by the `SchemaAttributeType` it accepts there, so the attribute's
/// `fieldName` and its `type` are the same token.
fn component_schema(row: &CatalogRow) -> ProviderComponentSchema {
    ProviderComponentSchema {
        schemas: Some(vec![row
            .attributes
            .iter()
            .map(|(attribute_type, _)| (*attribute_type).to_string())
            .collect()]),
        provider_schema_attributes: Some(
            row.attributes
                .iter()
                .map(|(attribute_type, hashing)| ProviderSchemaAttribute {
                    field_name: (*attribute_type).to_string(),
                    r#type: (*attribute_type).to_string(),
                    // `subType` is modelled optional and none of these attributes has
                    // one, so it is absent rather than echoed from `type`.
                    sub_type: None,
                    hashing: Some(*hashing),
                })
                .collect(),
        ),
    }
}

/// `providerConfigurationDefinition` — "the definition of the provider
/// configuration", i.e. the schema of the `providerConfiguration` document a workflow
/// hands this provider (`ProviderProperties$providerConfiguration`).
///
/// A JSON Schema object over the row's own
/// [`CatalogRow::configuration_properties`]: every property a string, and the
/// required ones listed in the order the row declares them. An entry that takes no
/// configuration has no definition and the optional member is absent.
fn configuration_definition(row: &CatalogRow) -> Option<Document> {
    if row.configuration_properties.is_empty() {
        return None;
    }
    Some(json_object([
        ("type", json_string("object")),
        (
            "properties",
            json_object(row.configuration_properties.iter().map(|(name, _)| {
                (*name, json_object([("type", json_string("string"))]))
            })),
        ),
        (
            "required",
            Document::Array(
                row.configuration_properties
                    .iter()
                    .filter(|(_, required)| *required)
                    .map(|(name, _)| json_string(name))
                    .collect(),
            ),
        ),
    ]))
}

/// `providerJobConfiguration` — "provider service job configurations", i.e. the
/// schema of the per-job options this provider service takes.
///
/// The one job-level option a provider service takes is the S3 location its job stages
/// data through (`ProviderProperties$intermediateSourceConfiguration`, whose
/// `intermediateS3Path` is @required,
/// `smithy_query(query="shape",shape="IntermediateSourceConfiguration")`), so exactly
/// the entries that need intermediate storage declare one and the rest leave the
/// optional member absent.
fn job_configuration(row: &CatalogRow) -> Option<Document> {
    if !row.intermediate_data_access {
        return None;
    }
    Some(json_object([
        ("type", json_string("object")),
        (
            "properties",
            json_object([(
                "intermediateSourceConfiguration",
                json_object([
                    ("type", json_string("object")),
                    (
                        "properties",
                        json_object([(
                            "intermediateS3Path",
                            json_object([("type", json_string("string"))]),
                        )]),
                    ),
                    (
                        "required",
                        Document::Array(vec![json_string("intermediateS3Path")]),
                    ),
                ]),
            )]),
        ),
        (
            "required",
            Document::Array(vec![json_string("intermediateSourceConfiguration")]),
        ),
    ]))
}

/// `providerEntityOutputDefinition` — "the definition of the provider entity output".
///
/// The provider answers one `entity` per input record, carrying the identifier that
/// provider holds for it ([`CatalogRow::output_attribute`]), so the definition is a
/// JSON Schema object requiring `entity` and declaring that one property under it.
fn entity_output_definition(row: &CatalogRow) -> Document {
    json_object([
        ("type", json_string("object")),
        (
            "properties",
            json_object([(
                "entity",
                json_object([
                    ("type", json_string("object")),
                    (
                        "properties",
                        json_object([(
                            row.output_attribute,
                            json_object([("type", json_string("string"))]),
                        )]),
                    ),
                ]),
            )]),
        ),
        ("required", Document::Array(vec![json_string("entity")])),
    ])
}

/// `providerIdNameSpaceConfiguration` — "the provider configuration required for
/// different ID namespace types".
///
/// An ID namespace is a `SOURCE` or a `TARGET` of an ID-MAPPING workflow, so only an
/// `ID_MAPPING` entry has one; an `ASSIGNMENT` entry leaves the optional member absent.
fn id_name_space_configuration(row: &CatalogRow) -> Option<ProviderIdNameSpaceConfiguration> {
    if row.service_type != SERVICE_TYPE_ID_MAPPING {
        return None;
    }
    let definition = |role: &str| {
        Some(json_object([
            ("type", json_string("object")),
            (
                "properties",
                json_object([(
                    "idNamespaceRole",
                    json_object([("const", json_string(role))]),
                )]),
            ),
        ]))
    };
    Some(ProviderIdNameSpaceConfiguration {
        description: Some(format!(
            "{} ID namespace configuration",
            row.display_name
        )),
        provider_source_configuration_definition: definition("SOURCE"),
        provider_target_configuration_definition: definition("TARGET"),
    })
}

/// `providerIntermediateDataAccessConfiguration` — the accounts and S3 actions "that
/// are required by some providers to create an S3 bucket for intermediate data
/// storage", so it is present only for an entry that needs one.
///
/// The account is the emulator's own ([`::emulator_server::EMULATOR_ACCOUNT_ID`]),
/// which is the only account this emulator has and satisfies the modelled
/// `AwsAccountId` pattern `^\d{12}$`; the actions are the S3 operations a provider
/// performs on that bucket — it reads the input it was handed, writes its output and
/// lists the prefix.
fn intermediate_data_access_configuration(
    row: &CatalogRow,
) -> Option<ProviderIntermediateDataAccessConfiguration> {
    if !row.intermediate_data_access {
        return None;
    }
    Some(ProviderIntermediateDataAccessConfiguration {
        aws_account_ids: Some(vec![
            ::emulator_server::EMULATOR_ACCOUNT_ID.to_string()
        ]),
        // The S3 actions a provider performs on the intermediate bucket: it writes
        // its output there, reads the input it was handed, lists and cleans up the
        // prefix, and resolves the bucket itself before any of that.
        required_bucket_actions: Some(vec![
            "s3:PutObject".to_string(),
            "s3:GetObject".to_string(),
            "s3:ListBucket".to_string(),
            "s3:DeleteObject".to_string(),
            "s3:GetBucketLocation".to_string(),
            "s3:GetBucketAcl".to_string(),
        ]),
    })
}

/// A `Document` object from `(name, value)` pairs.
fn json_object<'a>(members: impl IntoIterator<Item = (&'a str, Document)>) -> Document {
    Document::Object(
        members
            .into_iter()
            .map(|(name, value)| (name.to_string(), value))
            .collect(),
    )
}

/// A `Document` string.
fn json_string(value: &str) -> Document {
    Document::String(value.to_string())
}

/// The `ServiceType` value for a service that maps ids between two ID namespaces.
pub const SERVICE_TYPE_ID_MAPPING: &str = "ID_MAPPING";

/// The text this service puts in a response `message` member.
///
/// Every message AWS Entity Resolution returns on these paths is the SERVICE'S OWN
/// per-operation text: a fixed string that names the resource TYPE the way the API
/// spells it (`SchemaMapping`, `MatchingWorkflow`), with the requested name
/// substituted only where that operation's own template includes it. None of it is a
/// sentence composed from the request or from the stored record, and none of it is
/// shared between operations: a missing workflow reads `MatchingWorkflow Not Found`
/// from `GetMatchingWorkflow` but `MatchingWorkflow <workflowName> does not exist`
/// from `DeleteMatchingWorkflow`, so collapsing the two into one string would be
/// wrong. Each path therefore gets its OWN item below, derived from no other.
pub mod messages {
    /// `DeleteSchemaMappingOutput$message` — modelled @required, target `String`, doc
    /// "A successful operation message"
    /// (`smithy_query(query="operation",operation="DeleteSchemaMapping")`). The value
    /// is this fixed service string, observed on the wire from AWS.
    pub const DELETE_SCHEMA_MAPPING_SUCCESS: &str = "SchemaMapping Deleted.";

    /// `DeleteMatchingWorkflowOutput$message` — modelled @required, target `String`,
    /// doc "A successful operation message". Not itself recorded; it is this
    /// operation's own constant, spelled with the resource type the API uses, in the
    /// same form as the recorded [`DELETE_SCHEMA_MAPPING_SUCCESS`].
    pub const DELETE_MATCHING_WORKFLOW_SUCCESS: &str = "MatchingWorkflow Deleted.";

    /// `GetSchemaMapping`'s `ResourceNotFoundException$message` (HTTP 404). AWS
    /// answers this constant and does NOT name the schema that was asked for.
    pub const GET_SCHEMA_MAPPING_NOT_FOUND: &str = "SchemaMapping does not exist";

    /// `GetMatchingWorkflow`'s `ResourceNotFoundException$message` (HTTP 404). AWS
    /// answers this constant — deliberately worded differently from
    /// [`delete_matching_workflow_not_found`] — and does not name the workflow.
    pub const GET_MATCHING_WORKFLOW_NOT_FOUND: &str = "MatchingWorkflow Not Found";

    /// `GetProviderService`'s `ResourceNotFoundException$message` (HTTP 404), answered
    /// when no catalog entry has the requested `providerName` and
    /// `providerServiceArn`. This fixed string is what AWS answers on the wire for
    /// that miss — it names the catalog entry by the service's OWN internal term for
    /// one rather than by the API's `ProviderService`, and, like
    /// [`GET_SCHEMA_MAPPING_NOT_FOUND`], it composes nothing of the request into
    /// itself: neither the provider nor the service that was asked for.
    pub const GET_PROVIDER_SERVICE_NOT_FOUND: &str = "PartnerComponent Not Found";

    /// `CreateSchemaMapping`'s `ConflictException$message` (HTTP 400) when the
    /// `schemaName` is already taken. AWS answers this constant with no resource name
    /// in it; `ConflictException`'s own doc gives "Schema already exists" as the
    /// example of the condition.
    pub const CREATE_SCHEMA_MAPPING_CONFLICT: &str = "SchemaMapping already exists.";

    /// The `ValidationException$message` (HTTP 400) both schema-mapping write paths
    /// answer when `mappedInputFields` does not declare exactly
    /// [`super::REQUIRED_UNIQUE_ID_ATTRIBUTES`] attribute of type
    /// [`super::UNIQUE_ID_ATTRIBUTE_TYPE`] (see [`super::unique_id_cardinality_defect`]).
    ///
    /// The text is AWS's own for this refusal, built from the two constants that STATE
    /// the rule rather than written out as a sentence, so the required count and the
    /// attribute type cannot drift from the check that enforces them. Nothing of the
    /// request is composed into it: the schema's name and its field names are not part
    /// of what AWS answers here.
    pub fn schema_mapping_unique_id_cardinality() -> String {
        format!(
            "SchemaMapping must contain {} attribute with type: {}",
            super::REQUIRED_UNIQUE_ID_ATTRIBUTES,
            super::UNIQUE_ID_ATTRIBUTE_TYPE
        )
    }

    /// The `ValidationException$message` (HTTP 400) both schema-mapping write paths answer
    /// when `mappedInputFields` names one input column TWICE (see
    /// [`super::duplicate_field_name_defect`]).
    ///
    /// AWS's own text for this refusal, a TEMPLATE over the request: the field names that are
    /// repeated are what tells the caller which entries to fix, so they are taken from the
    /// request's own list — in the order the list states them, once each — and never from any
    /// one recorded run.
    pub fn schema_mapping_duplicate_field_names(duplicates: &[String]) -> String {
        format!(
            "SchemaMapping is not allowed to have repeated input fields in attributes: {}",
            duplicates.join(", ")
        )
    }

    /// The `ValidationException$message` (HTTP 400) a write whose input source configuration
    /// names a schema mapping this account does not hold answers (see
    /// [`super::ComAmazonawsEntityresolutionAwsveniceserviceState::missing_schema_mappings`]).
    ///
    /// A TEMPLATE over the request and the store: AWS names the schema mapping(s) that could
    /// not be resolved, which is what tells the caller which reference to fix, so the names
    /// come from the request's own `inputSourceConfig` and never from any one recorded run.
    pub fn references_missing_schema_mappings(missing: &[String]) -> String {
        format!(
            "References schema mapping(s) which does not exist: [{}]",
            missing.join(", ")
        )
    }

    /// The `AccessDeniedException$message` (HTTP 403) a WORKFLOW write that hands this
    /// service a caller-supplied `roleArn` answers when IAM shows the service cannot
    /// assume it (see [`super::role_refusal`], which states which writes ask).
    ///
    /// A TEMPLATE over the request: AWS names the role the caller passed, which is what
    /// tells a caller WHICH value to fix when a request carries more than one role, so
    /// the ARN is taken from the request and never from any one recorded run.
    pub fn role_not_assumable(role_arn: &str) -> String {
        format!("Exception in assuming the passed role: {role_arn}")
    }

    /// `CreateMatchingWorkflow`'s `ConflictException$message` (HTTP 400) when the
    /// `workflowName` is already taken. Not itself recorded; it is this operation's
    /// own constant, in the same form as the recorded
    /// [`CREATE_SCHEMA_MAPPING_CONFLICT`] and spelled with the resource type
    /// `ConflictException`'s doc names ("Workflow already exists").
    pub const CREATE_MATCHING_WORKFLOW_CONFLICT: &str = "MatchingWorkflow already exists.";

    /// `StartMatchingJob`'s `ResourceNotFoundException$message` (HTTP 404) when no
    /// matching workflow of the requested name exists — the condition this operation's
    /// own doc describes ("The workflow must have previously been created using the
    /// CreateMatchingWorkflow endpoint"). Not itself recorded; this operation's OWN
    /// constant, spelled with the resource type the API uses and in the same constant
    /// form as the recorded [`GET_MATCHING_WORKFLOW_NOT_FOUND`].
    pub const START_MATCHING_JOB_WORKFLOW_NOT_FOUND: &str = "MatchingWorkflow Not Found";

    /// `StartMatchingJob`'s `ConflictException$message` (HTTP 400) when a job of that
    /// workflow is still in progress — the condition `ConflictException`'s own doc
    /// gives as an example ("Workflow is currently running"). This operation's own
    /// constant: no value from the request is composed into it.
    pub const START_MATCHING_JOB_ALREADY_RUNNING: &str = "MatchingWorkflow is currently running";

    /// `GetMatchingJob`'s `ResourceNotFoundException$message` (HTTP 404) when the
    /// `workflowName` names no matching workflow. This reader's own constant, in the
    /// same form as [`GET_MATCHING_WORKFLOW_NOT_FOUND`].
    pub const GET_MATCHING_JOB_WORKFLOW_NOT_FOUND: &str = "MatchingWorkflow Not Found";

    /// `GetMatchingJob`'s `ResourceNotFoundException$message` (HTTP 404) when the
    /// workflow exists but holds no job of the requested `jobId` — including a job id
    /// that belongs to ANOTHER workflow, which this workflow does not have. This
    /// reader's own constant, naming the resource type that was not found.
    pub const GET_MATCHING_JOB_NOT_FOUND: &str = "MatchingJob Not Found";

    /// `DeleteMatchingWorkflow`'s `ConflictException$message` (HTTP 400) when the
    /// workflow still has a matching job in progress — the condition
    /// `ConflictException`'s own doc gives as an example ("Workflow is currently
    /// running").
    ///
    /// Like [`delete_matching_workflow_not_found`], this delete path's text is a
    /// TEMPLATE over the request: the `workflowName` @httpLabel the caller asked for
    /// is substituted into it, so the refusal names the workflow whose run blocks the
    /// delete. The template is the rule — any name goes through it unchanged.
    pub fn delete_matching_workflow_running(workflow_name: &str) -> String {
        format!("Job associated with workflow {workflow_name} is currently running, failed to delete")
    }

    /// `CreateIdMappingWorkflow`'s `ConflictException$message` (HTTP 400) when the
    /// `workflowName` is already taken — the refusal its own doc describes ("Each
    /// IdMappingWorkflow must have a unique workflow name"). Not itself recorded; it is
    /// this operation's own constant, in the same form as the recorded
    /// [`CREATE_SCHEMA_MAPPING_CONFLICT`] and spelled with the resource type the API
    /// uses for it (`IdMappingWorkflow`).
    pub const CREATE_ID_MAPPING_WORKFLOW_CONFLICT: &str = "IdMappingWorkflow already exists.";

    /// `GetIdMappingWorkflow`'s `ResourceNotFoundException$message` (HTTP 404). Not
    /// itself recorded; this reader follows the same constant, no-name form as the
    /// recorded [`GET_SCHEMA_MAPPING_NOT_FOUND`], spelled with the resource type the
    /// API uses for it.
    pub const GET_ID_MAPPING_WORKFLOW_NOT_FOUND: &str = "IdMappingWorkflow Not Found";

    /// `DeleteIdMappingWorkflowOutput$message` — modelled @required, target `String`,
    /// doc "A successful operation message"
    /// (`smithy_query(query="operation",operation="DeleteIdMappingWorkflow")`). Not
    /// itself recorded; it is this operation's own constant, spelled with the resource
    /// type the API uses, in the same form as the recorded
    /// [`DELETE_SCHEMA_MAPPING_SUCCESS`]. It carries nothing of the record that was
    /// removed.
    pub const DELETE_ID_MAPPING_WORKFLOW_SUCCESS: &str = "IdMappingWorkflow Deleted.";

    /// `DeleteIdMappingWorkflow`'s `ResourceNotFoundException$message` (HTTP 404) — the
    /// refusal its own doc describes ("This operation returns a
    /// ResourceNotFoundException if a workflow with the given name does not exist").
    ///
    /// Not itself recorded; a DELETE path names the resource it could not delete, which
    /// is the form every recorded delete refusal in this service takes
    /// ([`delete_matching_workflow_not_found`], [`delete_id_namespace_not_found`]), so
    /// this one is the same template over the `workflowName` @httpLabel — and
    /// deliberately not the reader's constant ([`GET_ID_MAPPING_WORKFLOW_NOT_FOUND`]).
    pub fn delete_id_mapping_workflow_not_found(workflow_name: &str) -> String {
        format!("IdMappingWorkflow {workflow_name} does not exist")
    }

    /// `DeleteSchemaMapping`'s `ConflictException$message` (HTTP 400) when a matching
    /// workflow still names the schema in its `inputSourceConfig` — the refusal this
    /// operation's own doc describes ("This operation will fail if there is a
    /// MatchingWorkflow object that references the SchemaMapping in the workflow's
    /// InputSourceConfig").
    ///
    /// A TEMPLATE over the store: AWS identifies the workflow that BLOCKS the delete, which
    /// is what tells the caller which workflow to delete before retrying, so the name comes
    /// from the referencing workflow this store holds
    /// ([`super::ComAmazonawsEntityresolutionAwsveniceserviceState::referencing_workflow`])
    /// and never from any one recorded run.
    pub fn delete_schema_mapping_in_use(workflow_name: &str) -> String {
        format!("SchemaMapping associated with Matching Workflow: {workflow_name}")
    }

    /// `UpdateSchemaMapping`'s `ConflictException$message` (HTTP 400) when a matching
    /// workflow still names the schema in its `inputSourceConfig` — the refusal this
    /// operation's own doc describes ("A schema is immutable if it is being used by a
    /// workflow. Therefore, you can't update a schema mapping if it's associated with a
    /// workflow").
    ///
    /// The same template as [`delete_schema_mapping_in_use`], and for the same reason: both
    /// refuse on the same relationship, and a caller is told which workflow holds the schema
    /// so it can act on it.
    pub fn update_schema_mapping_in_use(workflow_name: &str) -> String {
        delete_schema_mapping_in_use(workflow_name)
    }

    /// `UpdateSchemaMapping`'s `ResourceNotFoundException$message` (HTTP 404) when no
    /// schema mapping carries the `schemaName` @httpLabel. Not itself recorded; it is
    /// this operation's own constant. The update addresses the SAME resource by the SAME
    /// key as `GetSchemaMapping`, whose recorded text names no schema either, so it
    /// follows the recorded [`GET_SCHEMA_MAPPING_NOT_FOUND`] form rather than the
    /// templated delete one.
    pub const UPDATE_SCHEMA_MAPPING_NOT_FOUND: &str = "SchemaMapping does not exist";

    /// `DeleteMatchingWorkflow`'s `ResourceNotFoundException$message` (HTTP 404).
    ///
    /// This is the one path where the service's text is a TEMPLATE over the request:
    /// the `workflowName` @httpLabel the caller asked for is substituted into it. The
    /// template is the rule — any name goes through it unchanged.
    pub fn delete_matching_workflow_not_found(workflow_name: &str) -> String {
        format!("MatchingWorkflow {workflow_name} does not exist")
    }

    /// `CreateIdNamespace`'s `ConflictException$message` (HTTP 400) when the
    /// `idNamespaceName` is already taken — the refusal this operation's own doc
    /// describes ("Each ID namespace must have a unique name"). Not itself recorded;
    /// it is this operation's own constant, in the same form as the recorded
    /// [`CREATE_SCHEMA_MAPPING_CONFLICT`] and spelled with the resource type the API
    /// uses.
    pub const CREATE_ID_NAMESPACE_CONFLICT: &str = "IdNamespace already exists.";

    /// `GetIdNamespace`'s `ResourceNotFoundException$message` (HTTP 404). The recorded
    /// corpus shows AWS answering this CONSTANT for an ID namespace that is not there
    /// — the same `<ResourceType> Not Found` form as the recorded
    /// [`GET_MATCHING_WORKFLOW_NOT_FOUND`] — and, like every other reader here, it does
    /// NOT name the namespace that was asked for.
    pub const GET_ID_NAMESPACE_NOT_FOUND: &str = "IdNamespace Not Found";

    /// `UpdateMatchingWorkflow`'s `ResourceNotFoundException$message` (HTTP 404) when
    /// no workflow carries the `workflowName` @httpLabel. Not itself recorded; it is
    /// this operation's own constant. `UpdateMatchingWorkflow` addresses the SAME
    /// resource by the SAME key as `GetMatchingWorkflow` ("The workflow must already
    /// exist for this operation to succeed"), and AWS's text for that lookup names no
    /// resource name, so this follows the recorded
    /// [`GET_MATCHING_WORKFLOW_NOT_FOUND`] form rather than the templated delete one.
    pub const UPDATE_MATCHING_WORKFLOW_NOT_FOUND: &str = "MatchingWorkflow Not Found";

    /// `UpdateIdNamespace`'s `ResourceNotFoundException$message` (HTTP 404) when no ID
    /// namespace is stored under the `idNamespaceName` @httpLabel. Not itself recorded;
    /// it is this operation's own constant, in the same constant, no-name form as the
    /// recorded [`GET_ID_NAMESPACE_NOT_FOUND`] — the update addresses the SAME resource
    /// by the SAME key as `GetIdNamespace`, whose recorded text names no namespace
    /// either.
    pub const UPDATE_ID_NAMESPACE_NOT_FOUND: &str = "IdNamespace Not Found";

    /// `UpdateIdMappingWorkflow`'s `ResourceNotFoundException$message` (HTTP 404) when
    /// no ID mapping workflow carries the `workflowName` @httpLabel. Not itself
    /// recorded; it is this operation's own constant. `UpdateIdMappingWorkflow`
    /// addresses the SAME resource by the SAME key as `GetIdMappingWorkflow` ("the
    /// IdMappingWorkflow must already exist for the method to succeed"), so it follows
    /// the same constant, no-name form as [`GET_ID_MAPPING_WORKFLOW_NOT_FOUND`].
    pub const UPDATE_ID_MAPPING_WORKFLOW_NOT_FOUND: &str = "IdMappingWorkflow Not Found";

    /// `ListTagsForResource`'s `ResourceNotFoundException$message` (HTTP 404) when the
    /// `resourceArn` names no resource this service holds.
    ///
    /// Not recorded, and AWS documents no template for it, so this is this
    /// operation's own constant; the HTTP 404 and the `ResourceNotFoundException`
    /// code are what a client branches on. It names the resource TYPES the tagging
    /// surface covers the way the API spells them, and carries no value from the
    /// request.
    pub const LIST_TAGS_FOR_RESOURCE_NOT_FOUND: &str =
        "The resource ARN does not identify a SchemaMapping, MatchingWorkflow, \
         IdMappingWorkflow or IdNamespace that exists";

    /// `TagResource`'s `ResourceNotFoundException$message` (HTTP 404) for the same
    /// condition, kept as this operation's OWN constant rather than shared with
    /// [`LIST_TAGS_FOR_RESOURCE_NOT_FOUND`]: nothing says AWS words the two alike.
    pub const TAG_RESOURCE_NOT_FOUND: &str =
        "The resource ARN does not identify a SchemaMapping, MatchingWorkflow, \
         IdMappingWorkflow or IdNamespace that can be tagged";

    /// `UntagResource`'s `ResourceNotFoundException$message` (HTTP 404), this
    /// operation's own constant for the same reason.
    pub const UNTAG_RESOURCE_NOT_FOUND: &str =
        "The resource ARN does not identify a SchemaMapping, MatchingWorkflow, \
         IdMappingWorkflow or IdNamespace that can be untagged";

    /// `DeleteSchemaMapping`'s `ResourceNotFoundException$message` (HTTP 404) — the
    /// refusal its doc describes ("This operation returns a ResourceNotFoundException
    /// if a schema with the given name does not exist"). Not itself recorded; this
    /// delete path follows the same request-templated form as the recorded
    /// [`delete_matching_workflow_not_found`], with the `schemaName` @httpLabel
    /// substituted.
    pub fn delete_schema_mapping_not_found(schema_name: &str) -> String {
        format!("SchemaMapping {schema_name} does not exist")
    }

    /// `DeleteIdNamespaceOutput$message` — modelled @required, target `String`, doc "A
    /// successful operation message"
    /// (`smithy_query(query="operation",operation="DeleteIdNamespace")`). The whole
    /// string is this operation's own fixed text as the recorded corpus shows AWS
    /// wording it — `deleted` lowercase, which is NOT the casing of the recorded
    /// [`DELETE_SCHEMA_MAPPING_SUCCESS`], so the text is reproduced per operation and
    /// never title-cased by a shared formatter. It carries nothing of the record that
    /// was removed.
    pub const DELETE_ID_NAMESPACE_SUCCESS: &str = "IdNamespace deleted.";

    /// `DeleteIdNamespace`'s `ResourceNotFoundException$message` (HTTP 404) — the
    /// refusal its own doc describes ("This operation returns a
    /// ResourceNotFoundException if an ID namespace with the given name does not
    /// exist").
    ///
    /// A DELETE path names the resource it could not delete: the text is a TEMPLATE
    /// over the request, with the `idNamespaceName` @httpLabel the caller asked for
    /// substituted into it, exactly as [`delete_matching_workflow_not_found`] and
    /// [`delete_schema_mapping_not_found`] do for their own resource types. The READER
    /// of the same resource is worded differently and names nothing
    /// ([`GET_ID_NAMESPACE_NOT_FOUND`]), so the two paths do not share a string.
    pub fn delete_id_namespace_not_found(id_namespace_name: &str) -> String {
        format!("IdNamespace {id_namespace_name} does not exist")
    }

    /// `GenerateMatchId`'s `ResourceNotFoundException$message` (HTTP 404) when the
    /// `workflowName` @httpLabel names no matching workflow — the precondition its own
    /// doc states ("The rule-based matching workflow must exist and be active before
    /// calling this operation"). Not recorded; this operation's own constant, spelled
    /// with the resource type the API uses, in the same form as the recorded
    /// [`GET_MATCHING_WORKFLOW_NOT_FOUND`].
    pub const GENERATE_MATCH_ID_WORKFLOW_NOT_FOUND: &str = "MatchingWorkflow Not Found";

    /// `GetMatchId`'s `ResourceNotFoundException$message` (HTTP 404) when the
    /// `workflowName` @httpLabel names no matching workflow. This operation's own
    /// constant, derived from no other.
    pub const GET_MATCH_ID_WORKFLOW_NOT_FOUND: &str = "MatchingWorkflow Not Found";

    /// `BatchDeleteUniqueId`'s `ResourceNotFoundException$message` (HTTP 404) when the
    /// `workflowName` @httpLabel names no matching workflow. This operation's own
    /// constant, spelled with the resource type the API uses and in the same form as
    /// the recorded [`GET_MATCHING_WORKFLOW_NOT_FOUND`].
    pub const BATCH_DELETE_UNIQUE_ID_WORKFLOW_NOT_FOUND: &str = "MatchingWorkflow Not Found";

    /// `BatchDeleteUniqueId`'s `ValidationException$message` (HTTP 400) when the
    /// workflow it names is not a RULE_MATCHING one.
    ///
    /// Composed from the request, exactly as [`generate_match_id_not_rule_based`] and
    /// [`get_match_id_not_rule_based`] are: the unique ids this operation deletes are
    /// the records a RULE-BASED workflow has indexed, so a workflow of any other
    /// `resolutionType` holds nothing this operation is defined over.
    pub fn batch_delete_unique_id_not_rule_based(
        workflow_name: &str,
        resolution_type: &str,
    ) -> String {
        format!(
            "BatchDeleteUniqueId requires a rule-based matching workflow: MatchingWorkflow \
             {workflow_name} has resolutionTechniques.resolutionType {resolution_type}, not \
             RULE_MATCHING"
        )
    }

    /// `BatchDeleteUniqueId`'s `ValidationException$message` (HTTP 400) when the
    /// request carries no unique id to delete.
    ///
    /// `uniqueIds` is @required and HEADER-serialized
    /// (`smithy_query(query="operation",operation="BatchDeleteUniqueId")`), so an EMPTY
    /// list arrives as an absent header — there is no header that spells "a list of no
    /// elements" — and the refusal is the one an absent @required member earns:
    /// `1 validation error detected: Value null at 'uniqueIds' failed to satisfy
    /// constraint: Member must not be null`. It is RECORDED behaviour, and it is built
    /// through the framework's own rendering of that violation class rather than
    /// written out, so this refusal and the boundary validator's cannot drift.
    pub fn batch_delete_unique_id_missing_unique_ids() -> String {
        ::aws_smithy_schema_server::constraints::missing_member_message(super::UNIQUE_IDS_MEMBER)
    }

    /// `GenerateMatchId`'s `ValidationException$message` (HTTP 400) when the workflow
    /// it names is not a RULE_MATCHING one.
    ///
    /// Composed from the request, which is what the message rule allows: it names the
    /// member that is wrong and the constraint it fails, the way the boundary
    /// validator does. The constraint is the one `GenerateMatchId`'s own doc states —
    /// "The rule-based matching workflow must exist and be active before calling this
    /// operation" — against the `resolutionType` the named workflow was created with.
    pub fn generate_match_id_not_rule_based(workflow_name: &str, resolution_type: &str) -> String {
        format!(
            "GenerateMatchId requires a rule-based matching workflow: MatchingWorkflow \
             {workflow_name} has resolutionTechniques.resolutionType {resolution_type}, not \
             RULE_MATCHING"
        )
    }

    /// `GenerateMatchId`'s `ValidationException$message` (HTTP 400) when the named
    /// rule-based workflow was created (or updated) with
    /// `resolutionTechniques.enableRealTimeMatching` EXPLICITLY `false` — the
    /// constraint that member states ("When you enable real-time matching, you can
    /// use the GenerateMatchId operation with the workflow"). An absent member is not
    /// this case; see [`super::real_time_matching_explicitly_disabled`]. Composed from
    /// the request, naming the member and the constraint.
    pub fn generate_match_id_real_time_matching_disabled(workflow_name: &str) -> String {
        format!(
            "GenerateMatchId requires real-time matching: MatchingWorkflow {workflow_name} was \
             created with resolutionTechniques.enableRealTimeMatching disabled"
        )
    }

    /// `GetMatchId`'s `ValidationException$message` (HTTP 400) when the workflow it
    /// names is not a RULE_MATCHING one — the constraint its own doc states
    /// ("Returns the corresponding Match ID of a customer record if the record has
    /// been processed in a rule-based matching workflow"). Composed from the request,
    /// naming the member and the constraint.
    pub fn get_match_id_not_rule_based(workflow_name: &str, resolution_type: &str) -> String {
        format!(
            "GetMatchId requires a rule-based matching workflow: MatchingWorkflow \
             {workflow_name} has resolutionTechniques.resolutionType {resolution_type}, not \
             RULE_MATCHING"
        )
    }

    /// `GenerateMatchId`'s `ValidationException$message` (HTTP 400) for a `records`
    /// list outside the modelled `length` range. Composed from the request: it names
    /// the member, the bound it failed and the value that failed it, the way the
    /// boundary validator does for a modelled `length`.
    pub fn generate_match_id_batch_out_of_range(
        submitted: usize,
        minimum: usize,
        maximum: usize,
    ) -> String {
        let bound = if submitted > maximum {
            format!("Member must have length less than or equal to {maximum}")
        } else {
            format!("Member must have length greater than or equal to {minimum}")
        };
        format!(
            "1 validation error detected: value at 'records' failed to satisfy constraint: \
             {bound}, got {submitted}"
        )
    }

    /// `GenerateMatchId`'s `ValidationException$message` (HTTP 400) for a
    /// `processingType` outside the modelled `ProcessingType` enum. Composed from the
    /// request, naming the member and the values it admits.
    pub fn generate_match_id_unknown_processing_type(value: &str, admitted: &[&str]) -> String {
        format!(
            "1 validation error detected: value {value:?} at 'processingType' failed to satisfy \
             constraint: Member must satisfy enum value set: [{}]",
            admitted.join(", ")
        )
    }

    /// `GenerateMatchId`'s `ValidationException$message` (HTTP 400) when the workflow
    /// has reached the number of records this emulator lets one rule-based workflow
    /// index ([`super::MAX_INDEXED_RECORDS_PER_WORKFLOW`]). Composed from the request
    /// and the bound, and naming the bound so the caller can act on it;
    /// `DeleteMatchingWorkflow` releases the index, so the bound is self-healing.
    pub fn generate_match_id_index_full(workflow_name: &str, maximum: usize) -> String {
        format!(
            "MatchingWorkflow {workflow_name} has reached the quota of {maximum} for \
             \"{}\"; no further record can be indexed",
            super::INDEXED_RECORDS_QUOTA_NAME
        )
    }

    /// The `ResourceNotFoundException$message` (HTTP 404) every policy operation
    /// answers when the `arn` @httpLabel names no resource this account holds — a
    /// resource-based policy belongs to a resource, so there is nothing to read or
    /// write a policy on.
    ///
    /// Not recorded. A TEMPLATE over the request, like
    /// [`delete_matching_workflow_not_found`]: the ARN the caller asked for is
    /// substituted into it, so the refusal names the resource that was not found. The
    /// template is the rule — any ARN goes through it unchanged.
    pub fn policy_resource_not_found(arn: &str) -> String {
        format!("Resource {arn} does not exist")
    }

    /// The `ValidationException$message` (HTTP 400) a policy write answers when this
    /// store already holds [`super::MAX_RESOURCE_POLICIES`] policies and the resource
    /// written to carries none yet.
    ///
    /// A DEFENSIVE refusal rather than an AWS rule: a policy belongs to a live resource
    /// and there is at most one entry per resource, each resource collection carrying
    /// its own quota, so a caller cannot reach this bound through the API. It is
    /// enforced so the collection stays bounded even if the pruning predicate
    /// regresses, and the message names the bound so the condition is diagnosable.
    pub fn resource_policies_at_capacity(maximum: usize) -> String {
        format!(
            "this emulator holds the maximum of {maximum} resource-based policies; \
             delete a resource that carries one before attaching another"
        )
    }

    /// The `ResourceNotFoundException$message` (HTTP 404) the policy paths answer when
    /// the resource exists but carries no resource-based policy: there is no policy
    /// revision to report.
    ///
    /// RECORDED from AWS on a `GetPolicy` for a resource whose policy had just been
    /// emptied by `DeletePolicyStatement`. It is a fixed service string that names
    /// neither the resource nor the condition, in the same constant, no-name form as
    /// the recorded [`GET_SCHEMA_MAPPING_NOT_FOUND`], and the statement paths answer
    /// the same text for the same condition.
    pub const GET_POLICY_NOT_FOUND: &str = "The resource specified in the request does not exist.";

    /// `AddPolicyStatement`'s `ValidationException$message` (HTTP 400) when the policy
    /// already holds a statement with the requested `statementId`.
    ///
    /// Both the code and the text are recorded behaviour: AWS refuses a repeated
    /// `statementId` as a `ValidationException` and not as a `ConflictException`, even
    /// though the operation models both, and answers this fixed string, which names
    /// neither the identifier nor the resource. That fits what the member documents —
    /// `statementId` is "a statement identifier that differentiates the statement from
    /// others in the same policy", so a value that differentiates nothing fails that
    /// member's own rule rather than conflicting with the resource's state — and it is
    /// in the same per-operation constant form as [`CREATE_SCHEMA_MAPPING_CONFLICT`].
    pub const DUPLICATE_STATEMENT_ID: &str = "Statement ID already exists in document";

    /// `AddPolicyStatement`'s `ValidationException$message` (HTTP 400) when the
    /// statement would take the policy past what a `PolicyDocument` can carry.
    ///
    /// Not itself recorded. Built from the request's own limit: the message names the
    /// length the policy would reach and the modelled maximum it would exceed
    /// ([`super::MAX_POLICY_DOCUMENT_LENGTH`]), which is what a caller needs to fix the
    /// call.
    pub fn policy_document_too_large(length: usize) -> String {
        format!(
            "the policy this statement would produce is {length} characters, past the maximum \
             of {} a policy document can carry",
            super::MAX_POLICY_DOCUMENT_LENGTH
        )
    }

    /// `AddPolicyStatement`'s `ValidationException$message` (HTTP 400) when
    /// `condition` is not a policy condition block.
    ///
    /// The model constrains that member only by length (`StatementCondition`,
    /// `length 1..40960`), so a value that is not a JSON object of condition keys
    /// reaches the handler; the message names the member and the constraint it failed,
    /// the way the boundary validator's own refusals do.
    pub fn invalid_statement_condition(reason: &str) -> String {
        format!("condition must be a JSON object of policy condition keys: {reason}")
    }

    /// `PutPolicy`'s `ConflictException$message` (HTTP 400) when the request carries a
    /// `token` that is not the current revision of the resource's policy — the
    /// optimistic-concurrency refusal the member's own doc implies ("a unique
    /// identifier for the CURRENT revision of the policy").
    ///
    /// RECORDED from AWS: the text is FIXED and carries nothing of the request. In
    /// particular it does not echo the token the caller sent, which is a value a write
    /// mints per revision, so one refusal reads the same as every other.
    pub const STALE_POLICY_TOKEN: &str = "Policy put request failed. Incorrect token.";

    /// `PutPolicy`'s `ValidationException$message` (HTTP 400) when `policy` is not a
    /// JSON document.
    ///
    /// A `PolicyDocument` is modelled only as a bounded string, so the boundary
    /// validator cannot refuse a document that is not JSON at all; a resource-based
    /// policy IS a JSON policy document, and the service parses it. Not recorded; the
    /// message names the member and the constraint it failed, which is what a caller
    /// needs to fix the request.
    pub const POLICY_NOT_JSON: &str = "policy must be a JSON policy document";
}

/// How many schema mappings this emulator lets exist at once, past which
/// `CreateSchemaMapping` answers the modelled `ExceedsLimitException` (HTTP 402, "The
/// request was rejected because it attempted to create resources beyond the current
/// Entity Resolution account limits").
///
/// AWS Entity Resolution publishes an adjustable per-account, per-Region quota named
/// "Schema mappings"; this is the default this emulator enforces for it. The bound is
/// what keeps the store from growing without limit on repeated creates, and it is
/// self-healing: `DeleteSchemaMapping` releases an entry, so room comes back.
pub const MAX_SCHEMA_MAPPINGS: usize = 100;

/// The name of the quota [`MAX_SCHEMA_MAPPINGS`] enforces, reported on
/// `ExceedsLimitException$quotaName`.
pub const SCHEMA_MAPPINGS_QUOTA_NAME: &str = "Schema mappings";

/// How many matching workflows this emulator lets exist at once, past which
/// `CreateMatchingWorkflow` answers the modelled `ExceedsLimitException` (HTTP 402).
///
/// AWS Entity Resolution publishes an adjustable per-account, per-Region quota named
/// "Matching workflows"; this is the default this emulator enforces for it. As with
/// the schema-mapping bound, `DeleteMatchingWorkflow` releases an entry, so the bound
/// needs no TTL or eviction.
pub const MAX_MATCHING_WORKFLOWS: usize = 100;

/// The name of the quota [`MAX_MATCHING_WORKFLOWS`] enforces, reported on
/// `ExceedsLimitException$quotaName`.
pub const MATCHING_WORKFLOWS_QUOTA_NAME: &str = "Matching workflows";

/// How many ID namespaces this emulator lets exist at once, past which
/// `CreateIdNamespace` answers the modelled `ExceedsLimitException` (HTTP 402).
///
/// AWS Entity Resolution publishes an adjustable per-account, per-Region quota named
/// "ID namespaces"; this is the default this emulator enforces for it. As with the
/// other two bounds, deleting a namespace releases an entry, so the bound is
/// self-healing and needs no TTL or eviction.
pub const MAX_ID_NAMESPACES: usize = 100;

/// The name of the quota [`MAX_ID_NAMESPACES`] enforces, reported on
/// `ExceedsLimitException$quotaName`.
pub const ID_NAMESPACES_QUOTA_NAME: &str = "ID namespaces";

/// How many ID mapping workflows this emulator lets exist at once, past which
/// `CreateIdMappingWorkflow` answers the modelled `ExceedsLimitException` (HTTP 402).
///
/// AWS Entity Resolution publishes an adjustable per-account, per-Region quota named
/// "ID mapping workflows"; this is the default this emulator enforces for it. As with
/// the matching-workflow bound, `DeleteIdMappingWorkflow` releases an entry, so the
/// bound needs no TTL or eviction.
pub const MAX_ID_MAPPING_WORKFLOWS: usize = 100;

/// The name of the quota [`MAX_ID_MAPPING_WORKFLOWS`] enforces, reported on
/// `ExceedsLimitException$quotaName`.
pub const ID_MAPPING_WORKFLOWS_QUOTA_NAME: &str = "ID mapping workflows";

/// How many tags one resource may carry, past which `TagResource` refuses the merge
/// whole with the modelled `ValidationException` (HTTP 400).
///
/// It is the maximum the model states for a tag map: `TagMap` is `length 0..200`
/// (`smithy_query(query="shape",shape="TagMap")`), which is both the most pairs one
/// request may carry and the most `ListTagsForResource` may report, since its `tags`
/// member targets that same shape. A merge is therefore bounded by the same figure as
/// a single request, so repeated `TagResource` calls cannot grow a record's tag map
/// without limit. The bound is self-healing: `UntagResource` releases keys, so room
/// comes back without a TTL or eviction.
pub const MAX_TAGS_PER_RESOURCE: usize = 200;

/// A quota value as `ExceedsLimitException$quotaValue` models it (an `Integer`).
///
/// Every maximum is a small constant, so the conversion cannot fail; it is written
/// without a panic so a handler stays total.
pub fn quota_value(maximum: usize) -> Option<i32> {
    i32::try_from(maximum).ok()
}

/// The resolution type `GenerateMatchId` and `GetMatchId` require of a workflow, as
/// `ResolutionType` spells it (`smithy_query(query="shape",shape="ResolutionType")` →
/// `RULE_MATCHING | ML_MATCHING | PROVIDER`): both operations are defined only over a
/// RULE-BASED matching workflow.
pub const RULE_MATCHING: &str = "RULE_MATCHING";

/// `ProcessingType$EVENTUAL_NO_LOOKUP` — the mode that "generates new match IDs
/// without checking existing matches"
/// (`smithy_query(query="doc",shape="GenerateMatchIdInput",member="processingType")`),
/// i.e. the one mode that does NOT consult the workflow's match index.
pub const EVENTUAL_NO_LOOKUP: &str = "EVENTUAL_NO_LOOKUP";

/// Every value `ProcessingType` admits
/// (`smithy_query(query="shape",shape="ProcessingType")`), in model order.
///
/// `GenerateMatchId` refuses anything else with the modelled `ValidationException`
/// rather than silently reading it as the default mode.
pub const PROCESSING_TYPES: &[&str] = &["CONSISTENT", "EVENTUAL", EVENTUAL_NO_LOOKUP];

/// How many records one `GenerateMatchId` call may submit: the modelled
/// `GenerateMatchIdInput$records` `length` range, enforced as a closed interval.
///
/// The constraint is on the MEMBER, not on `RecordList` itself, which is why the
/// generated contract header in `ops/generate_match_id.rs` does not render it;
/// `smithy_query(query="operation",operation="GenerateMatchId")` reports it verbatim:
/// `"records": {"target": "RecordList", "length": {"min": 1, "max": 1}, "required":
/// true}`. So a batch of none or of more than one record is input the model does NOT
/// admit, and refusing it with the modelled `ValidationException` is the faithful
/// answer as well as the bound that keeps this operation's work and growth
/// independent of a caller-chosen count. The maximum of one is also why the capacity
/// check inside the loop precedes the only insert the call can make.
pub const MIN_RECORDS_PER_CALL: usize = 1;

/// The upper bound of the modelled `records` `length` range — see
/// [`MIN_RECORDS_PER_CALL`].
pub const MAX_RECORDS_PER_CALL: usize = 1;

/// The member `BatchDeleteUniqueId` takes the ids to delete on
/// (`BatchDeleteUniqueIdInput$uniqueIds`), as the validation envelope spells a member
/// path. Named here because the refusal of a request that carries none states it.
pub const UNIQUE_IDS_MEMBER: &str = "uniqueIds";

/// `DeleteUniqueIdStatus$COMPLETED` — "the status of the batch delete unique ID
/// operation" when the deletion is DONE by the time the call answers
/// (`smithy_query(query="shape",shape="DeleteUniqueIdStatus")` → `COMPLETED |
/// ACCEPTED`).
///
/// This emulator's real-time match index lives in memory and is updated inside the
/// call, so a `BatchDeleteUniqueId` that returns has already applied every deletion it
/// reports: nothing is left running behind the response, which is what the other value
/// (`ACCEPTED`) would claim.
pub const DELETE_UNIQUE_ID_COMPLETED: &str = "COMPLETED";

/// `DeleteUniqueIdErrorType$VALIDATION_ERROR` — the per-id error type whose own doc is
/// "the deletion operation couldn't complete because of invalid input parameters or
/// data" (`smithy_query(query="shape",shape="DeleteUniqueIdError")`).
///
/// The type reported for a `uniqueId` the workflow holds no record for: the request
/// names data that is not there, so that id's deletion cannot complete. The other
/// value, `SERVICE_ERROR`, is "an internal service-side problem", which is not what a
/// request naming an unknown record is.
pub const DELETE_UNIQUE_ID_VALIDATION_ERROR: &str = "VALIDATION_ERROR";

/// The order this service STATES the `IdNamespaceType` value set in when it refuses a
/// value outside it.
///
/// A service states an enum's value set in its own published order, which is not the
/// order that set reaches a refusal in (the order the MODEL declares the enum's members
/// in): `IdNamespaceType` is declared `SOURCE, TARGET`
/// (`smithy_query(query="shape",shape="IdNamespaceType")`), and the refusal of a value
/// outside it lists the set in THIS order. It is FIXED text — no member of the request
/// and nothing in the store varies it.
///
/// It holds exactly the values the model declares for that enum, reordered and nothing
/// else, which the `enum_value_set_order` suite pins against
/// `crate::ops::types::ID_NAMESPACE_TYPE_VALUES`.
pub const ID_NAMESPACE_TYPE_VALUE_SET: [&str; 2] = ["TARGET", "SOURCE"];

/// Every enum value set this service publishes an order for.
///
/// One row per enum whose stated order is known to differ from its declaration order;
/// a value set absent from this table is stated as the model declares it, because this
/// service has shown no other order for it.
const PUBLISHED_ENUM_VALUE_SETS: &[&[&str]] = &[&ID_NAMESPACE_TYPE_VALUE_SET];

/// The order this service states `values` in, when `values` is the value set of an
/// enum whose order it publishes ([`PUBLISHED_ENUM_VALUE_SETS`]); `None` for any other
/// value set.
///
/// An enum is recognised by its value SET rather than by the member that carried the
/// rejected value, so every member targeting one enum — of any operation, at any depth
/// — states that enum's set in the one order, and a set this service publishes no
/// order for keeps the order it was presented in. The match is on membership and size,
/// never on position, so a model revision that reordered its own declaration of the
/// same enum would still be stated in the published order.
pub fn published_enum_value_set(values: &[String]) -> Option<&'static [&'static str]> {
    PUBLISHED_ENUM_VALUE_SETS.iter().copied().find(|published| {
        published.len() == values.len()
            && published
                .iter()
                .all(|value| values.iter().any(|presented| presented == value))
    })
}

/// What the comparison value of a matching key joins its fields' values with.
///
/// A matching key can name SEVERAL schema fields (every field the schema mapping gave
/// that `matchKey`), so the value compared for it is those fields' values in schema
/// order. The separator is the ASCII unit separator, which an attribute value does not
/// carry, so two records cannot compare equal merely because one field's value spills
/// into the next field's position.
const MATCHING_KEY_FIELD_SEPARATOR: &str = "\u{1f}";

/// `value` as a comparison reads it when normalization is asked for.
///
/// `InputSource$applyNormalization` and `GetMatchIdInput$applyNormalization` both say
/// the attributes defined in the schema are normalized in the input data before they
/// are matched, so under normalization a difference of letter case, or of leading,
/// trailing and repeated whitespace, is not a difference in the value. The per-type
/// output FORMATTING AWS additionally applies (rendering a phone number as
/// `(123)-456-7890`) is a property of the S3 output table, which this emulator does
/// not write, not of this comparison.
fn normalized(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// The rules a workflow matches records with, in the order they were declared.
///
/// A workflow whose `resolutionTechniques` carries no `ruleBasedProperties` has none,
/// so nothing can match and every submitted record opens its own group.
pub fn workflow_rules(workflow: &MatchingWorkflow) -> &[Rule] {
    workflow
        .resolution_techniques
        .rule_based_properties
        .as_ref()
        .map_or(&[][..], |properties| properties.rules.as_slice())
}

/// Did the workflow's creator EXPLICITLY turn real-time matching off?
///
/// `ResolutionTechniques$enableRealTimeMatching` is an OPTIONAL `Boolean` with no
/// modelled `@default` (`smithy_query(query="shape",shape="ResolutionTechniques")`),
/// so it has three states on the wire — `true`, `false` and absent — and only the
/// explicit `false` is a statement that real-time matching is off. An ABSENT member
/// says nothing, and the model states no rule that makes a request it admits
/// (`enableRealTimeMatching` omitted) refuse an operation, so a precondition must not
/// read absence as the refusing value: a rule-based workflow created without the
/// member serves `GenerateMatchId`.
///
/// The doc sentence "When you enable real-time matching, you can use the
/// `GenerateMatchId` operation with the workflow" is what keeps the refusal for the
/// explicit `false`.
pub fn real_time_matching_explicitly_disabled(workflow: &MatchingWorkflow) -> bool {
    workflow.resolution_techniques.enable_real_time_matching == Some(false)
}

/// Does the workflow ask for normalization of the records that arrive from
/// `input_source_arn`?
///
/// `applyNormalization` is a property of the INPUT SOURCE a record comes from, so the
/// flag that governs a submitted record is the one on the workflow's
/// `inputSourceConfig` entry whose `inputSourceARN` the record names. A record from a
/// source the workflow does not name (and an entry whose optional member is absent)
/// is not normalized.
pub fn normalization_for_source(workflow: &MatchingWorkflow, input_source_arn: &str) -> bool {
    workflow
        .input_source_config
        .iter()
        .find(|source| source.input_source_arn == input_source_arn)
        .and_then(|source| source.apply_normalization)
        .unwrap_or(false)
}

/// The value a record compares under `fields`, or `None` when it carries a value for
/// NONE of them — a key the record says nothing about cannot match.
///
/// A field the record omits holds its position as an empty value, so a record that
/// omits one of a grouped key's fields does not compare equal to one that fills it.
fn matching_key_value(
    fields: &[String],
    attributes: &HashMap<String, String>,
    normalize: bool,
) -> Option<String> {
    let mut present = false;
    let parts: Vec<String> = fields
        .iter()
        .map(|field| match attributes.get(field) {
            Some(value) => {
                present = true;
                if normalize {
                    normalized(value)
                } else {
                    value.clone()
                }
            }
            None => String::new(),
        })
        .collect();
    present.then(|| parts.join(MATCHING_KEY_FIELD_SEPARATOR))
}

/// A newly minted `matchId`.
///
/// The value is opaque to a client — `MatchGroup$matchId` is a plain `String` with no
/// modelled form — and it is minted ONCE per group and stored, so every later read of
/// that group answers the same id and two groups never share one.
///
/// The entropy is a random v4 UUID's, reached through `emulator_telemetry`'s
/// `BatchId::random()` constructor (`uuid::Uuid::new_v4().as_hyphenated()` verbatim,
/// `vendor/emulator-telemetry/src/types.rs`) — the same in-crate generator
/// [`new_job_id`] and [`new_policy_token`] borrow, because `emulator-telemetry` is
/// already one of this crate's dependencies, so nothing new enters the workspace
/// lockfile. Only the generator is borrowed; the newtype's telemetry meaning is
/// dropped here.
pub fn new_match_id() -> String {
    ::emulator_telemetry::BatchId::random().as_str().to_string()
}

/// The resource-type segment every `IdNamespaceArn` carries
/// (`arn:…:entityresolution:<region>:<account>:idnamespace/<name>`). Named here
/// because the create mints an ARN with it and [`id_namespace_key`] matches one
/// against it, so the two cannot drift.
pub const ID_NAMESPACE_RESOURCE_PREFIX: &str = "idnamespace/";

/// The `idNamespaceName` a request names, reduced to the key the store holds it
/// under, or `None` when the value cannot name a namespace served for this request.
///
/// Every ID-namespace path member is an `EntityNameOrIdNamespaceArn`
/// (`^[a-zA-Z_0-9-=+/]*$|^arn:(aws|aws-us-gov|aws-cn):entityresolution:<region>:<account>:idnamespace/[a-zA-Z_0-9-]{1,255}$`,
/// `smithy_query(query="shape",shape="EntityNameOrIdNamespaceArn")`), so a caller may
/// name a namespace either by its name or by the ARN the create minted. A name is the
/// key itself; an ARN names a namespace only when it is EXACTLY the ARN this service
/// mints for that name while serving this request, so one differing in its partition,
/// service, account or region segment names NOTHING here rather than resolving to the
/// local record of the same name. (The store is keyed by name, not by region, so this
/// scopes an ARN against the request it arrived on; it is not isolation between
/// regions, which this emulator does not model.)
pub fn id_namespace_key<'a>(name_or_arn: &'a str, region: &str) -> Option<&'a str> {
    if !name_or_arn.starts_with("arn:") {
        return Some(name_or_arn);
    }
    let (_, name) = name_or_arn.rsplit_once(ID_NAMESPACE_RESOURCE_PREFIX)?;
    let minted = ::emulator_server::emulator_arn(
        "entityresolution",
        region,
        &format!("{ID_NAMESPACE_RESOURCE_PREFIX}{name}"),
    );
    (name_or_arn == minted).then_some(name)
}

/// The four resource types a `VeniceGlobalArn` can name, i.e. the resources that can
/// carry a resource-based policy.
///
/// The modelled `VeniceGlobalArn` pattern admits exactly these and nothing else
/// (`arn:(aws|aws-us-gov|aws-cn):entityresolution:<region>:<account>:(schemamapping|matchingworkflow|idmappingworkflow|idnamespace)/<name>`,
/// `smithy_query(query="shape",shape="VeniceGlobalArn")`), so a value that reaches a
/// handler always carries one of them. Each is the same segment the matching create
/// mints its ARN with — [`ID_NAMESPACE_RESOURCE_PREFIX`] is
/// [`ID_NAMESPACE_RESOURCE_TYPE`] plus `/`.
pub const SCHEMA_MAPPING_RESOURCE_TYPE: &str = "schemamapping";
/// See [`SCHEMA_MAPPING_RESOURCE_TYPE`].
pub const MATCHING_WORKFLOW_RESOURCE_TYPE: &str = "matchingworkflow";
/// See [`SCHEMA_MAPPING_RESOURCE_TYPE`].
pub const ID_MAPPING_WORKFLOW_RESOURCE_TYPE: &str = "idmappingworkflow";
/// See [`SCHEMA_MAPPING_RESOURCE_TYPE`].
pub const ID_NAMESPACE_RESOURCE_TYPE: &str = "idnamespace";

/// The `(resourceType, name)` a `VeniceGlobalArn` names, or `None` when the value
/// cannot name a resource served for a request signed for `region`.
///
/// An ARN names a resource here only when it is EXACTLY the ARN this service mints for
/// that resource while serving this request, so one differing in its partition,
/// service, account or region segment names NOTHING rather than resolving to the local
/// record of the same name — the same scoping rule [`id_namespace_key`] applies to the
/// one ARN-or-name label in this API.
pub fn venice_global_arn_resource<'a>(arn: &'a str, region: &str) -> Option<(&'a str, &'a str)> {
    let (_, resource) = arn.rsplit_once(':')?;
    let minted = ::emulator_server::emulator_arn("entityresolution", region, resource);
    if arn != minted {
        return None;
    }
    resource.split_once('/')
}

/// The key the resource `arn` names is stored under in
/// [`ComAmazonawsEntityresolutionAwsveniceserviceState::policies`] — its
/// `<resourceType>/<name>` segment — or `None` when the value cannot name a resource
/// served for a request signed for `region`.
///
/// The ARN is validated exactly as [`venice_global_arn_resource`] validates it, so an
/// ARN scoped to another partition, region or account still names nothing; what the key
/// drops is the scoping segments themselves, so a stored entry carries no
/// request-supplied data and can be resolved against the live resource without a
/// request to take a region from.
pub fn policy_key<'a>(arn: &'a str, region: &str) -> Option<&'a str> {
    venice_global_arn_resource(arn, region)?;
    arn.rsplit_once(':').map(|(_, resource)| resource)
}

/// How many resource-based policies this store holds at once, past which a policy
/// write refuses.
///
/// A policy belongs to a live resource and there is at most one entry per resource
/// ([`ComAmazonawsEntityresolutionAwsveniceserviceState::policies`]), so this is the
/// sum of the four resource quotas: the bound the collection already has structurally,
/// made explicit and enforced, so the collection stays bounded even if the pruning
/// predicate regresses.
pub const MAX_RESOURCE_POLICIES: usize =
    MAX_SCHEMA_MAPPINGS + MAX_MATCHING_WORKFLOWS + MAX_ID_NAMESPACES + MAX_ID_MAPPING_WORKFLOWS;

/// A freshly minted policy revision token: a hyphenated v4 UUID, which is exactly what
/// the modelled `PolicyToken` requires (`length 36..36`, pattern
/// `^[a-fA-F0-9]{8}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{12}$`,
/// `smithy_query(query="shape",shape="PolicyToken")`).
///
/// The entropy is the same audited generator [`new_job_id`] borrows —
/// `emulator_telemetry`'s `BatchId::random()`, `uuid::Uuid::new_v4().as_hyphenated()`
/// verbatim — so nothing new enters the workspace lockfile for it, and only the
/// generator is borrowed: the newtype's telemetry meaning is dropped here. A revision
/// token is an identifier, never a credential.
pub fn new_policy_token() -> String {
    ::emulator_telemetry::BatchId::random().as_str().to_string()
}

/// The `ResolutionType` for which matching workflows have no incremental processing.
///
/// One of the values `ResolutionType` admits
/// (`RESOLUTION_TYPE_VALUES` = `ML_MATCHING | PROVIDER | RULE_MATCHING`); named here
/// because [`incremental_run_conflict`] is the one place the service branches on it.
pub const PROVIDER_RESOLUTION_TYPE: &str = "PROVIDER";

/// Why a matching-workflow request pairing an `incrementalRunConfig` with
/// `resolutionTechniques.resolutionType` `PROVIDER` is refused — `None` when the
/// combination is allowed.
///
/// Both `CreateMatchingWorkflow` and `UpdateMatchingWorkflow` document the same rule,
/// on the operation itself and on the member: "For workflows where `resolutionType`
/// is `PROVIDER`, incremental processing is not supported"
/// (`smithy_query(query="doc",shape="UpdateMatchingWorkflowInput",member="incrementalRunConfig")`).
/// The model cannot express a condition ACROSS two members, so the boundary validator
/// passes such a request and the handlers refuse it with the modelled
/// `ValidationException` (HTTP 400). The message is built from the request: it names
/// the member that cannot be carried and the `resolutionType` value the request
/// carried, which is what a caller needs to fix it.
pub fn incremental_run_conflict(
    resolution_type: &str,
    incremental_run_config: Option<&IncrementalRunConfig>,
) -> Option<String> {
    (incremental_run_config.is_some() && resolution_type == PROVIDER_RESOLUTION_TYPE).then(|| {
        format!(
            "incrementalRunConfig is not supported for a MatchingWorkflow whose \
             resolutionTechniques.resolutionType is {resolution_type}"
        )
    })
}

/// The `SchemaAttributeType` that marks the attribute holding a record's own
/// identifier.
///
/// One of the values `SchemaAttributeType` admits
/// (`SCHEMA_ATTRIBUTE_TYPE_VALUES` = `ADDRESS … UNIQUE_ID`,
/// `smithy_query(query="shape",shape="SchemaAttributeType")`), named here because
/// [`unique_id_cardinality_defect`] is the one place the service branches on it.
pub const UNIQUE_ID_ATTRIBUTE_TYPE: &str = "UNIQUE_ID";

/// How many attributes of type [`UNIQUE_ID_ATTRIBUTE_TYPE`] a schema mapping declares.
///
/// A schema mapping's `UNIQUE_ID` attribute is the column that identifies ONE input
/// record, which is what every id the matching surface answers is keyed by
/// (`GenerateMatchId` takes "the unique ID of a record" and `BatchDeleteUniqueId`
/// deletes by it), so a schema with none names no record and a schema with two names
/// a record by two different columns at once. Exactly one, therefore.
pub const REQUIRED_UNIQUE_ID_ATTRIBUTES: usize = 1;

/// Why a schema mapping whose `mappedInputFields` does not declare exactly
/// [`REQUIRED_UNIQUE_ID_ATTRIBUTES`] attribute of type [`UNIQUE_ID_ATTRIBUTE_TYPE`] is
/// refused — `None` when the list satisfies the rule.
///
/// The model constrains `SchemaInputAttributes` only by LENGTH (2..60) and each
/// member's `type` only by the `SchemaAttributeType` enum, so the model cannot express
/// a condition over the WHOLE list and the boundary validator passes such a request:
/// both write paths that take the list (`CreateSchemaMapping` and
/// `UpdateSchemaMapping`, whose `mappedInputFields` is @required on each) refuse it
/// here with the modelled `ValidationException` (HTTP 400) before anything is stored,
/// so no schema this service holds is one AWS would have refused.
pub fn unique_id_cardinality_defect(
    mapped_input_fields: &[SchemaInputAttribute],
) -> Option<String> {
    let declared = mapped_input_fields
        .iter()
        .filter(|attribute| attribute.r#type == UNIQUE_ID_ATTRIBUTE_TYPE)
        .count();
    (declared != REQUIRED_UNIQUE_ID_ATTRIBUTES)
        .then(messages::schema_mapping_unique_id_cardinality)
}

/// Why a schema mapping whose `mappedInputFields` names one input column TWICE is refused —
/// `None` when every `fieldName` in the list is distinct.
///
/// A `SchemaInputAttribute` "corresponds to a column the source data table" and its
/// `fieldName` is the @required name of that column
/// (`smithy_query(query="shape",shape="SchemaInputAttribute")`), so two entries carrying one
/// name describe the same column twice and contradict each other: the column would have two
/// attribute types, two match keys and two group names at once. The model constrains
/// `SchemaInputAttributes` only by LENGTH (2..60) and each `fieldName` only by its own
/// pattern and length, so it cannot express a condition ACROSS the list and the boundary
/// validator passes such a request; both write paths that take the list
/// (`CreateSchemaMapping` and `UpdateSchemaMapping`, whose `mappedInputFields` is @required
/// on each) refuse it here with the modelled `ValidationException` (HTTP 400) before
/// anything is stored, so no schema this service holds is one AWS would have refused.
///
/// The names are reported in the order the list states them, each named once however many
/// times it repeats, and the comparison is on the whole `fieldName` as the request spelled
/// it — the key a schema's columns are addressed by elsewhere in this API
/// (`Rule$matchingKeys`, `GenerateMatchId`'s record attributes).
pub fn duplicate_field_name_defect(
    mapped_input_fields: &[SchemaInputAttribute],
) -> Option<String> {
    let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
    for attribute in mapped_input_fields {
        *seen.entry(attribute.field_name.as_str()).or_default() += 1;
    }
    let mut duplicates: Vec<String> = Vec::new();
    for attribute in mapped_input_fields {
        let repeated = seen
            .get(attribute.field_name.as_str())
            .is_some_and(|count| *count > 1);
        if repeated && !duplicates.iter().any(|named| named == &attribute.field_name) {
            duplicates.push(attribute.field_name.clone());
        }
    }
    (!duplicates.is_empty())
        .then(|| messages::schema_mapping_duplicate_field_names(&duplicates))
}

/// The service principal a caller-supplied `roleArn` has to let assume it.
///
/// Entity Resolution reads the caller's records and writes its output ITSELF, acting as
/// the role the request hands it ("Entity Resolution assumes this role to create
/// resources on your behalf as part of workflow execution",
/// `smithy_query(query="doc",shape="CreateMatchingWorkflowInput",member="roleArn")`), so
/// the role's trust policy has to name this principal.
pub const SERVICE_PRINCIPAL: &str = "entityresolution.amazonaws.com";

/// Why a request naming a `roleArn` this service cannot assume is refused — `None` when
/// the request names no role, or when nothing is KNOWN about the one it names.
///
/// Asked by the WORKFLOW writes — `CreateMatchingWorkflow`, `UpdateMatchingWorkflow`,
/// `CreateIdMappingWorkflow` and `UpdateIdMappingWorkflow` — before they write, and by
/// those only: their `roleArn` is the role "Entity Resolution assumes … to create
/// resources on your behalf as part of workflow execution"
/// (`smithy_query(query="doc",shape="CreateMatchingWorkflowInput",member="roleArn")`), so
/// the resource each of them stores is one this service itself runs AS that role, and a
/// request this answers for is refused with the operation's modelled
/// `AccessDeniedException` (HTTP 403) with nothing persisted. The ID-NAMESPACE writes do
/// NOT ask: a namespace's role is one "Entity Resolution assumes … to access the resources
/// defined in this `IdNamespace` on your behalf as part of the workflow run"
/// (`smithy_query(query="doc",shape="CreateIdNamespaceInput",member="roleArn")`) — assumed
/// by the ID mapping workflow run that uses the namespace as a Source or a Target, which
/// the namespace's resource policy may share with another account — so that write records
/// the role instead of resolving it. `roleArn` is optional on the ID-mapping writes
/// (modelled `default: ""`), and a request that names NO role hands the service nothing to
/// assume, so there is nothing to resolve for it.
///
/// The whole question — which role of this account the value names, what IAM answers
/// about it, and whether the trust document it carries admits a service principal — is
/// the ONE reading every emulated service that resolves a caller-supplied role shares
/// (`service_common::iam_role::role_is_unassumable`), so this crate states only WHICH
/// principal has to be admitted ([`SERVICE_PRINCIPAL`]) and WHICH operations ask. IAM is
/// reached the way any emulated service reaches another: the AWS SDK over the emulator's
/// OWN in-process callback ([`smithy_server_runtime::OperationContext::callback`]), so
/// this crate names no other service crate and nothing here opens a socket.
///
/// `None` — do not refuse — for every case where nothing is KNOWN about the role: a
/// value that names no IAM role of this account (another account's IAM is not this
/// emulator's to ask), or an emulator with no IAM mounted to ask at all. A refusal has
/// to be grounded in what IAM answered, never assumed. Awaited with NO store guard held.
pub async fn role_refusal(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    role_arn: Option<&str>,
) -> Option<String> {
    let named = role_arn.filter(|value| !value.is_empty())?;
    ::service_common::iam_role::role_is_unassumable(ctx, region, named, SERVICE_PRINCIPAL)
        .await
        .then(|| messages::role_not_assumable(named))
}

impl ComAmazonawsEntityresolutionAwsveniceserviceState {
    /// The instant the resource `arn` names was created, or `None` when it names no
    /// resource this store holds for a request served in `region`.
    ///
    /// A resource-based policy belongs TO a resource, so every policy operation is
    /// about one of the four resources a `VeniceGlobalArn` can name (see
    /// [`venice_global_arn_resource`]) and each type is looked up in its own
    /// collection. An ARN scoped to another partition, region or account names nothing
    /// here. The `createdAt` is returned rather than a bare `bool` because it is also
    /// the incarnation stamp a policy entry carries
    /// ([`ResourcePolicy::resource_created_at`]), so existence and incarnation are
    /// read in ONE lookup and cannot disagree.
    pub fn policy_resource_created_at(
        &self,
        arn: &str,
        region: &str,
    ) -> Option<::aws_smithy_types::DateTime> {
        let (resource_type, name) = venice_global_arn_resource(arn, region)?;
        self.resource_created_at(resource_type, name)
    }

    /// The instant the resource `resource_type`/`name` was created, or `None` when this
    /// store holds no such resource.
    ///
    /// The one lookup both the ARN-addressed reader ([`Self::policy_resource_created_at`])
    /// and the key-addressed pruner ([`Self::prune_orphaned_policies`]) go through, so
    /// the two can never disagree about whether a policy's resource is live.
    fn resource_created_at(
        &self,
        resource_type: &str,
        name: &str,
    ) -> Option<::aws_smithy_types::DateTime> {
        match resource_type {
            SCHEMA_MAPPING_RESOURCE_TYPE => {
                self.schema_mappings.get(name).map(|record| record.created_at)
            }
            MATCHING_WORKFLOW_RESOURCE_TYPE => {
                self.matching_workflows.get(name).map(|record| record.created_at)
            }
            ID_MAPPING_WORKFLOW_RESOURCE_TYPE => {
                self.id_mapping_workflows.get(name).map(|record| record.created_at)
            }
            ID_NAMESPACE_RESOURCE_TYPE => {
                self.id_namespaces.get(name).map(|record| record.created_at)
            }
            _ => None,
        }
    }

    /// The instant the resource a STORED policy key names was created, or `None` when
    /// that resource is gone — the pruner's question, answered without a request,
    /// because a key carries the resource's own identity and nothing else.
    fn keyed_resource_created_at(&self, key: &str) -> Option<::aws_smithy_types::DateTime> {
        let (resource_type, name) = key.split_once('/')?;
        self.resource_created_at(resource_type, name)
    }

    /// Has this store room to attach a policy to the resource `key` names?
    ///
    /// Replacing the policy of a resource that already carries one does not grow the
    /// collection, so it is always allowed; a resource that carries none is admitted
    /// only below [`MAX_RESOURCE_POLICIES`].
    pub fn has_room_for_policy(&self, key: &str) -> bool {
        self.policies.contains_key(key) || self.policies.len() < MAX_RESOURCE_POLICIES
    }

    /// The resource-based policy currently attached to the resource `arn` names, or
    /// `None` when that resource does not exist or carries no policy.
    ///
    /// An entry stamped with another incarnation of the resource is not served: see
    /// [`ResourcePolicy::resource_created_at`].
    pub fn resource_policy(&self, arn: &str, region: &str) -> Option<&ResourcePolicy> {
        let created_at = self.policy_resource_created_at(arn, region)?;
        self.policies
            .get(policy_key(arn, region)?)
            .filter(|policy| policy.resource_created_at == created_at)
    }

    /// Drop every policy entry whose resource no longer exists, and answer how many
    /// were dropped.
    ///
    /// A resource-based policy is NOT a resource of its own: it has no create API, no
    /// ARN and no quota, and AWS deletes it with the resource it is attached to. The
    /// delete of each of the four resource types is its own operation (and its own
    /// file), so this store collects the orphan when the policy path next runs: every
    /// write prunes first, which keeps the collection bounded by the LIVE resources —
    /// at most one entry per resource, and each resource collection has its own quota
    /// ([`MAX_SCHEMA_MAPPINGS`], [`MAX_MATCHING_WORKFLOWS`], [`MAX_ID_NAMESPACES`],
    /// [`MAX_ID_MAPPING_WORKFLOWS`]) — however many resources a caller creates,
    /// policies and deletes.
    ///
    /// An entry is an orphan when the resource its key names is gone, and also when
    /// that resource is a LATER incarnation of the name (its `createdAt` differs),
    /// which is the same condition [`resource_policy`] refuses to serve
    /// ([`ResourcePolicy::resource_created_at`]). A key is the resource's own
    /// `<resourceType>/<name>` identity ([`policy_key`]), so it resolves against the
    /// very record the resource collections hold — never against a region or account a
    /// request supplied, which is what makes every orphan reachable by this prune.
    ///
    /// [`resource_policy`]: Self::resource_policy
    pub fn prune_orphaned_policies(&mut self) -> usize {
        let orphaned: Vec<String> = self
            .policies
            .iter()
            .filter(|(key, policy)| {
                self.keyed_resource_created_at(key)
                    .is_none_or(|created_at| created_at != policy.resource_created_at)
            })
            .map(|(key, _)| key.clone())
            .collect();
        for arn in &orphaned {
            self.policies.remove(arn);
        }
        orphaned.len()
    }

    /// Is `schema_name` referenced by the `inputSourceConfig` of any stored matching
    /// workflow?
    ///
    /// `InputSource$schemaName` is the modelled reference from a workflow to a schema
    /// mapping ("The name of the schema to be retrieved",
    /// `smithy_query(query="shape",shape="InputSource")`), so this is what both
    /// `hasWorkflows` and `DeleteSchemaMapping`'s conflict check read.
    pub fn schema_has_workflows(&self, schema_name: &str) -> bool {
        self.referencing_workflow(schema_name).is_some()
    }

    /// The `workflowName` of the matching workflow whose `inputSourceConfig` names
    /// `schema_name`, or `None` when no stored workflow does.
    ///
    /// The one reading of that relationship: `hasWorkflows` is derived from it
    /// ([`Self::schema_has_workflows`]) and the two refusals that are ABOUT it —
    /// `DeleteSchemaMapping`'s and `UpdateSchemaMapping`'s `ConflictException` — word their
    /// message with the workflow it names, so a caller is told which workflow holds the
    /// schema. `matching_workflows` is ordered by `workflowName`, so a schema several
    /// workflows reference resolves to the FIRST of them in that order rather than to
    /// whichever one a hash happened to put first.
    pub fn referencing_workflow(&self, schema_name: &str) -> Option<&str> {
        self.matching_workflows
            .values()
            .find(|workflow| {
                workflow
                    .input_source_config
                    .iter()
                    .any(|source| source.schema_name == schema_name)
            })
            .map(|workflow| workflow.workflow_name.as_str())
    }

    /// The names among `schema_names` that this store holds NO schema mapping for, in the
    /// order they were given and each named once.
    ///
    /// `InputSource$schemaName` is a matching workflow's @required reference to the schema
    /// mapping that describes one of its input tables ("The name of the schema to be
    /// retrieved", `smithy_query(query="shape",shape="InputSource")`), so a name no schema
    /// mapping carries is a reference to nothing: the workflow could not read its input at
    /// all. The writes that take such a configuration resolve every name through this and
    /// refuse with the modelled `ValidationException` (HTTP 400) naming what is missing
    /// ([`messages::references_missing_schema_mappings`]) before anything is stored.
    pub fn missing_schema_mappings<'a, I>(&self, schema_names: I) -> Vec<String>
    where
        I: IntoIterator<Item = &'a str>,
    {
        let mut missing: Vec<String> = Vec::new();
        for name in schema_names {
            if !self.schema_mappings.contains_key(name) && !missing.iter().any(|held| held == name) {
                missing.push(name.to_string());
            }
        }
        missing
    }

    /// The record attributes a rule's `matchingKey` compares, in schema order.
    ///
    /// `Rule$matchingKeys` is documented as "A list of MatchingKeys. The MatchingKeys
    /// must have been defined in the SchemaMapping", and
    /// `SchemaInputAttribute$matchKey` is "A key that allows grouping of multiple
    /// input attributes into a unified matching group", so a matching key resolves to
    /// EVERY field of the workflow's schemas that carries that `matchKey` — a group of
    /// columns compared as one value — else to the schema field of that name.
    ///
    /// A key the workflow's present schemas define NEITHER way resolves to nothing, so
    /// no record can match on it: the modelled constraint above says such a key should
    /// not exist. The one exception is a workflow one of whose schema mappings has
    /// since been DELETED — then the key cannot be looked up at all, and it falls back
    /// to the record attribute of that name so the workflow keeps matching on it.
    ///
    /// Bounded by the model: at most 20 input sources
    /// (`CreateMatchingWorkflow`'s `inputSourceConfig` `length 1..20`) of at most 60
    /// `mappedInputFields` each, so one call to this is `O(20 × 60)` at worst. It is
    /// resolved ONCE per rule per request by [`Self::rule_fields`], never per candidate
    /// record.
    pub fn matching_key_fields(
        &self,
        workflow: &MatchingWorkflow,
        matching_key: &str,
    ) -> Vec<String> {
        let mut grouped: Vec<String> = Vec::new();
        let mut named: Vec<String> = Vec::new();
        let mut schema_missing = false;
        for source in &workflow.input_source_config {
            let Some(schema) = self.schema_mappings.get(&source.schema_name) else {
                schema_missing = true;
                continue;
            };
            for attribute in &schema.mapped_input_fields {
                let target = if attribute.match_key.as_deref() == Some(matching_key) {
                    &mut grouped
                } else if attribute.field_name == matching_key {
                    &mut named
                } else {
                    continue;
                };
                if !target.contains(&attribute.field_name) {
                    target.push(attribute.field_name.clone());
                }
            }
        }
        if !grouped.is_empty() {
            grouped
        } else if !named.is_empty() {
            named
        } else if schema_missing {
            vec![matching_key.to_string()]
        } else {
            Vec::new()
        }
    }

    /// The fields each `matchingKey` of `rule` compares, resolved ONCE for a request.
    ///
    /// Hoisting the resolution out of the candidate scan is what keeps the comparison
    /// cost linear in the number of indexed records rather than multiplying the schema
    /// walk by it.
    fn rule_fields(&self, workflow: &MatchingWorkflow, rule: &Rule) -> Vec<Vec<String>> {
        rule.matching_keys
            .iter()
            .map(|key| self.matching_key_fields(workflow, key))
            .collect()
    }

    /// The comparison values of EVERY `matchingKey` of a rule for `attributes`, or
    /// `None` when the record carries no value for one of them.
    ///
    /// A rule matches only when all of its keys do — "Two records are considered to
    /// match according to this rule if all of the MatchingKeys match"
    /// (`smithy_query(query="shape",shape="Rule")`) — so a key the record says nothing
    /// about makes the whole rule inapplicable to it.
    fn rule_values(
        fields: &[Vec<String>],
        attributes: &HashMap<String, String>,
        normalize: bool,
    ) -> Option<Vec<String>> {
        fields
            .iter()
            .map(|key_fields| matching_key_value(key_fields, attributes, normalize))
            .collect()
    }

    /// The already-indexed record of `workflow_name` that `attributes` match, and the
    /// `ruleName` of the rule that matched them.
    ///
    /// The FIRST rule, in the order the workflow declares them, that matches an
    /// indexed record of that workflow decides it. `excluding` leaves one `uniqueId`
    /// out of the comparison, which is how a SUBMITTED record is matched against the
    /// OTHER records of the workflow rather than against the copy of itself a previous
    /// call indexed.
    ///
    /// Every factor of the cost is bounded: at most 25 rules of at most 15 matching
    /// keys (the modelled `RuleList` and `MatchingKeys` `length` maxima), each key's
    /// fields resolved once per call by [`Self::rule_fields`], over at most
    /// [`MAX_INDEXED_RECORDS_PER_WORKFLOW`] candidates.
    fn find_by_rules(
        &self,
        workflow_name: &str,
        workflow: &MatchingWorkflow,
        attributes: &HashMap<String, String>,
        normalize: bool,
        excluding: Option<&str>,
    ) -> Option<(&IndexedRecord, String)> {
        let indexed = self.match_index.get(workflow_name)?;
        for rule in workflow_rules(workflow) {
            let fields = self.rule_fields(workflow, rule);
            // A rule none of whose matching keys the record carries is simply not
            // applicable to it; the NEXT rule still can be.
            let Some(values) = Self::rule_values(&fields, attributes, normalize) else {
                continue;
            };
            let matched = indexed
                .values()
                .filter(|record| excluding != Some(record.unique_id.as_str()))
                .find(|record| {
                    Self::rule_values(&fields, &record.record_attribute_map, normalize)
                        .is_some_and(|other| other == values)
                });
            if let Some(record) = matched {
                return Some((record, rule.rule_name.clone()));
            }
        }
        None
    }

    /// The group a SUBMITTED record joins in `workflow_name`, as `(matchId, ruleName)`:
    /// the `matchId` of the record it matched, and the rule that joined the two.
    ///
    /// This is the writer's question. `excluding` is the submitted record's own
    /// `uniqueId`, so a record is matched against the OTHER records of the workflow.
    pub fn resolve_by_rules(
        &self,
        workflow_name: &str,
        workflow: &MatchingWorkflow,
        attributes: &HashMap<String, String>,
        normalize: bool,
        excluding: Option<&str>,
    ) -> Option<(String, String)> {
        self.find_by_rules(workflow_name, workflow, attributes, normalize, excluding)
            .map(|(record, rule_name)| (record.match_id.clone(), rule_name))
    }

    /// The record of `workflow_name` that `attributes` identify, exactly as it is
    /// STORED.
    ///
    /// This is the reader's question, and it is answered with the stored record rather
    /// than with the rule that found it: a reader reports the `matchId` AND the
    /// `matchRule` the workflow recorded for that record, so the writer and the reader
    /// can never disagree about a group's rule. Nothing is excluded from the
    /// comparison — a reader submits no record of its own, so every indexed record is
    /// a candidate.
    pub fn identify_record(
        &self,
        workflow_name: &str,
        workflow: &MatchingWorkflow,
        attributes: &HashMap<String, String>,
        normalize: bool,
    ) -> Option<IndexedRecord> {
        self.find_by_rules(workflow_name, workflow, attributes, normalize, None)
            .map(|(record, _)| record.clone())
    }

    /// How many records `workflow_name` has indexed, against which
    /// [`MAX_INDEXED_RECORDS_PER_WORKFLOW`] is enforced.
    pub fn indexed_record_count(&self, workflow_name: &str) -> usize {
        self.match_index
            .get(workflow_name)
            .map_or(0, BTreeMap::len)
    }

    /// Has `workflow_name` room to index a record it does not already hold?
    ///
    /// Re-indexing a `uniqueId` the workflow already holds REPLACES that entry, so it
    /// does not grow the store and is always allowed; a new `uniqueId` is allowed only
    /// below the bound.
    pub fn has_room_to_index(&self, workflow_name: &str, unique_id: &str) -> bool {
        self.match_index
            .get(workflow_name)
            .is_none_or(|indexed| {
                indexed.contains_key(unique_id)
                    || indexed.len() < MAX_INDEXED_RECORDS_PER_WORKFLOW
            })
    }

    /// Forget everything `workflow_name` had indexed.
    ///
    /// A workflow's match index belongs to the workflow, so deleting the workflow
    /// releases it — which is what makes [`MAX_INDEXED_RECORDS_PER_WORKFLOW`]
    /// self-healing, exactly as `DeleteMatchingWorkflow` releasing an entry makes
    /// [`MAX_MATCHING_WORKFLOWS`] self-healing.
    pub fn release_match_index(&mut self, workflow_name: &str) {
        self.match_index.remove(workflow_name);
    }

    /// Forget the ONE record `unique_id` names in `workflow_name`'s index, and report
    /// it as it was stored — `None` when the workflow had indexed no such record.
    ///
    /// The per-record counterpart of [`Self::release_match_index`], and the only way a
    /// record leaves the index while its workflow lives: `BatchDeleteUniqueId` is
    /// "Deletes multiple unique IDs in a matching workflow"
    /// (`smithy_query(query="doc",shape="BatchDeleteUniqueId")`), so what it removes is
    /// the indexed record, after which no reader of the index resolves that record any
    /// more. A workflow left holding nothing drops its (now empty) index entry, so the
    /// map stays bounded by the workflows that actually hold a record.
    pub fn forget_indexed_record(
        &mut self,
        workflow_name: &str,
        unique_id: &str,
    ) -> Option<IndexedRecord> {
        let indexed = self.match_index.get_mut(workflow_name)?;
        let removed = indexed.remove(unique_id);
        if indexed.is_empty() {
            self.match_index.remove(workflow_name);
        }
        removed
    }

    /// Is the record `unique_id` names in `workflow_name` still joined to another
    /// record of that workflow by one of its rules?
    ///
    /// Asked of a record that SURVIVED a deletion, to decide whether it is still
    /// connected to anything: a record no rule of the workflow joins to any OTHER
    /// indexed record stands alone, which is what makes it a disconnected one. The
    /// comparison is the same rule walk every other answer in this module comes from
    /// ([`Self::resolve_by_rules`]), with the record itself excluded, and the
    /// normalization is the one its own input source asks for, so a record's
    /// connectedness is judged exactly as its matching was.
    pub fn is_still_connected(&self, workflow_name: &str, unique_id: &str) -> bool {
        let Some(record) = self
            .match_index
            .get(workflow_name)
            .and_then(|indexed| indexed.get(unique_id))
        else {
            return false;
        };
        let Some(workflow) = self.matching_workflows.get(workflow_name) else {
            return false;
        };
        let normalize = normalization_for_source(workflow, &record.input_source_arn);
        self.resolve_by_rules(
            workflow_name,
            workflow,
            &record.record_attribute_map,
            normalize,
            Some(unique_id),
        )
        .is_some()
    }

    /// Every record of `workflow_name` that belongs to the group `match_id`, in
    /// `uniqueId` order.
    ///
    /// This is what a `MatchGroup$records` list is built from: the group is the set of
    /// indexed records sharing the id, not just the record this call submitted. The
    /// list is bounded by [`MAX_INDEXED_RECORDS_PER_WORKFLOW`], and a response assembles
    /// it once per DISTINCT `matchId`, never once per submitted record.
    pub fn group_records(&self, workflow_name: &str, match_id: &str) -> Vec<IndexedRecord> {
        self.match_index
            .get(workflow_name)
            .into_iter()
            .flat_map(|indexed| indexed.values())
            .filter(|record| record.match_id == match_id)
            .cloned()
            .collect()
    }

    /// The tags stored on the resource `resource_arn` names, or `None` when no stored
    /// resource carries that ARN.
    ///
    /// The tagging surface is keyed by ARN, not by name: `resourceArn` is modelled as
    /// a `VeniceGlobalArn`
    /// (`^arn:…:entityresolution:<region>:<account>:((schemamapping|matchingworkflow|idmappingworkflow|idnamespace)/<name>)$`,
    /// `smithy_query(query="operation",operation="TagResource")`), so the lookup
    /// compares against the ARN that was MINTED for a record when it was created. A
    /// well-formed ARN whose partition, region, account, type or name does not match a
    /// stored record therefore names nothing, which is the 404 condition for all three
    /// tagging operations. Resolution spans EVERY resource type that pattern admits: a
    /// schema mapping, a matching workflow, an ID mapping workflow and an ID namespace
    /// each carry their own `tags` member, so each is reached here.
    ///
    /// `tags` is optional on a stored record (a create that carried none stores
    /// `None`), while `ListTagsForResourceOutput$tags` is @required, so an untagged
    /// resource reads back as the EMPTY map rather than as a missing member.
    pub fn resource_tags(&self, resource_arn: &str) -> Option<HashMap<String, String>> {
        if let Some(mapping) = self
            .schema_mappings
            .values()
            .find(|mapping| mapping.schema_arn == resource_arn)
        {
            return Some(mapping.tags.clone().unwrap_or_default());
        }
        if let Some(workflow) = self
            .matching_workflows
            .values()
            .find(|workflow| workflow.workflow_arn == resource_arn)
        {
            return Some(workflow.tags.clone().unwrap_or_default());
        }
        if let Some(workflow) = self
            .id_mapping_workflows
            .values()
            .find(|workflow| workflow.workflow_arn == resource_arn)
        {
            return Some(workflow.tags.clone().unwrap_or_default());
        }
        self.id_namespaces
            .values()
            .find(|namespace| namespace.id_namespace_arn == resource_arn)
            .map(|namespace| namespace.tags.clone().unwrap_or_default())
    }

    /// The tag SLOT of the resource `resource_arn` names, for a writer.
    ///
    /// Resolution is exactly [`Self::resource_tags`]'s, so a tag write and a tag read
    /// agree about which record an ARN names. The slot is handed back as the stored
    /// `Option` rather than as a map, so a caller decides whether a write is owed at
    /// all: materialising an empty map here would make `GetSchemaMapping` report
    /// `tags: {}` on a resource whose tags were never touched.
    pub fn resource_tags_mut(
        &mut self,
        resource_arn: &str,
    ) -> Option<&mut Option<HashMap<String, String>>> {
        if let Some(mapping) = self
            .schema_mappings
            .values_mut()
            .find(|mapping| mapping.schema_arn == resource_arn)
        {
            return Some(&mut mapping.tags);
        }
        if let Some(workflow) = self
            .matching_workflows
            .values_mut()
            .find(|workflow| workflow.workflow_arn == resource_arn)
        {
            return Some(&mut workflow.tags);
        }
        if let Some(workflow) = self
            .id_mapping_workflows
            .values_mut()
            .find(|workflow| workflow.workflow_arn == resource_arn)
        {
            return Some(&mut workflow.tags);
        }
        self.id_namespaces
            .values_mut()
            .find(|namespace| namespace.id_namespace_arn == resource_arn)
            .map(|namespace| &mut namespace.tags)
    }

    /// Materialise the clock-owed transitions of the job named by
    /// `(workflow_name, job_id)` and hand it back.
    ///
    /// A job of ANOTHER workflow is matched out first, so a call about one workflow
    /// never reads — or advances — another workflow's job. The only write is the one
    /// the clock had already made due at `now`, which is the state the caller would
    /// have seen had nobody called at all.
    pub fn matching_job_mut(
        &mut self,
        workflow_name: &str,
        job_id: &str,
        now: i64,
    ) -> Option<&mut MatchingJob> {
        let job = self.matching_jobs.get_mut(job_id)?;
        if job.workflow_name != workflow_name {
            return None;
        }
        job.materialize_due(now);
        Some(job)
    }

    /// Every job OF ONE WORKFLOW, in `jobId` order, with its clock-owed transitions
    /// materialised.
    ///
    /// The workflow filter runs before the materialisation, so a call about one
    /// workflow never advances another workflow's jobs.
    pub fn matching_jobs_of_mut(
        &mut self,
        workflow_name: &str,
        now: i64,
    ) -> impl Iterator<Item = &mut MatchingJob> {
        let workflow_name = workflow_name.to_string();
        self.matching_jobs.values_mut().filter_map(move |job| {
            if job.workflow_name != workflow_name {
                return None;
            }
            job.materialize_due(now);
            Some(job)
        })
    }

    /// Drop `workflow_name`'s oldest finished jobs until it holds at most `retain` of
    /// them, and report how many were dropped.
    ///
    /// Oldest by `startTime` (ties broken by `jobId`, so the order is total and the
    /// outcome does not depend on map iteration luck). A job still in progress is
    /// never dropped — retention ages out HISTORY, it does not cancel work — so a
    /// workflow whose jobs are all in progress keeps every one of them.
    pub fn retain_newest_jobs(&mut self, workflow_name: &str, retain: usize) -> usize {
        let mut finished: Vec<(i64, String)> = self
            .matching_jobs
            .values()
            .filter(|job| job.workflow_name == workflow_name && !job.is_in_progress())
            .map(|job| (job.start_time, job.job_id.clone()))
            .collect();
        let held = self
            .matching_jobs
            .values()
            .filter(|job| job.workflow_name == workflow_name)
            .count();
        let excess = held.saturating_sub(retain).min(finished.len());
        if excess == 0 {
            return 0;
        }
        finished.sort_unstable();
        for (_, job_id) in finished.into_iter().take(excess) {
            self.matching_jobs.remove(&job_id);
        }
        excess
    }

    /// Remove EVERY job of `workflow_name`, in progress or not, and report how many
    /// were removed.
    ///
    /// The cascade `DeleteMatchingWorkflow` performs: a job belongs to the workflow it
    /// runs, so leaving its jobs behind would let a workflow recreated under the same
    /// name inherit runs it never had. Unlike
    /// [`retain_newest_jobs`](Self::retain_newest_jobs), which ages out HISTORY and
    /// therefore never touches a job in progress, this says the workflow is GONE, so
    /// nothing of it is kept.
    pub fn remove_jobs_of(&mut self, workflow_name: &str) -> usize {
        let doomed: Vec<String> = self
            .matching_jobs
            .values()
            .filter(|job| job.workflow_name == workflow_name)
            .map(|job| job.job_id.clone())
            .collect();
        let removed = doomed.len();
        for job_id in doomed {
            self.matching_jobs.remove(&job_id);
        }
        removed
    }
}

/// How many records this emulator lets ONE matching workflow hold in its real-time
/// match index, past which `GenerateMatchId` refuses to index a further record with
/// the modelled `ValidationException` (HTTP 400 — the operation models no
/// `ExceedsLimitException`).
///
/// AWS publishes no quota for the size of a rule-based workflow's real-time match
/// index, so this is the emulator's OWN bound, and it is what keeps the store from
/// growing without limit on repeated data-plane calls: the index is the one map here
/// a caller can grow without creating a resource. It is self-healing in two ways —
/// re-submitting a `uniqueId` the workflow already holds replaces that entry instead
/// of adding one, and `DeleteMatchingWorkflow` releases the whole index through
/// [`ComAmazonawsEntityresolutionAwsveniceserviceState::release_match_index`].
pub const MAX_INDEXED_RECORDS_PER_WORKFLOW: usize = 200;

/// The name of the bound [`MAX_INDEXED_RECORDS_PER_WORKFLOW`] enforces, as the refusal
/// message names it.
pub const INDEXED_RECORDS_QUOTA_NAME: &str = "Records per rule-based matching workflow";

/// The region every ARN this service mints is scoped to when the request carries no
/// SigV4 credential scope to read it from. It is the region the emulator's own shared
/// AWS config names, so an unsigned call and a default-configured client agree.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The region this request was signed for, defaulting to [`DEFAULT_REGION`].
///
/// A resource ARN is region-scoped, and the only region a request carries is the one
/// in its SigV4 credential scope, so that is what an ARN minted while serving it uses.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map_or_else(|| DEFAULT_REGION.to_string(), |scope| scope.region)
}

/// Now, truncated to milliseconds.
///
/// restJson1 writes a timestamp as epoch seconds with a fractional part, i.e. as a
/// JSON number: an `f64` holds today's epoch seconds to about a quarter of a
/// microsecond, so a nanosecond-precision instant would not survive the round trip
/// and `createdAt` would differ between the create response and a later read.
/// Milliseconds round-trip exactly, which keeps a stored timestamp stable across reads.
pub fn now() -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_millis(now_millis())
}

/// Now as epoch milliseconds: the stored form of every instant a job record keeps, so
/// a due instant can be compared against the clock and still round-trip through a
/// modelled `Timestamp` member exactly (see [`now`]).
pub fn now_millis() -> i64 {
    ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}

/// A freshly minted `jobId`: 32 lowercase hex characters, as the modelled `JobId`
/// (`^[a-f0-9]{32}$`, `smithy_query(query="shape",shape="JobId")`) requires.
///
/// The entropy is a random v4 UUID's — the `uuid` crate's generator, reached through
/// `emulator_telemetry`'s `BatchId::random()` constructor
/// (`uuid::Uuid::new_v4().as_hyphenated()` verbatim, `vendor/emulator-telemetry/src/types.rs`)
/// because `emulator-telemetry` is already one of this crate's dependencies, so
/// nothing new enters the workspace lockfile for it. A hyphenated UUID is exactly 32
/// lowercase hex digits plus four hyphens, so dropping the hyphens yields the modelled
/// pattern; only the generator is borrowed, and the newtype's telemetry meaning is
/// dropped here.
pub fn new_job_id() -> String {
    ::emulator_telemetry::BatchId::random()
        .as_str()
        .chars()
        .filter(|ch| *ch != '-')
        .collect()
}

/// The page size to serve for a `@paginated` operation whose `maxResults` member is
/// `max_results` and whose modelled `range` maximum is `modelled_max`.
///
/// `maxResults` is optional on both list operations, so an absent member is "the
/// caller set no bound", which is served with the modelled maximum rather than with
/// an empty page. The modelled `range` declares only a maximum, so zero and negative
/// values reach a handler too; they are read the same way, because a page size of
/// zero would answer an empty page with a continuation token forever. A positive
/// caller-supplied bound is already inside the modelled range by the time a handler
/// runs (the boundary validator refuses anything else); clamping again costs nothing
/// and keeps this total.
pub fn page_size(max_results: Option<i32>, modelled_max: usize) -> usize {
    match max_results.map(usize::try_from) {
        None | Some(Ok(0)) | Some(Err(_)) => modelled_max,
        Some(Ok(bound)) => bound.min(modelled_max),
    }
}

/// One page of an ordered `(key, record)` sequence, plus the `nextToken` to continue
/// from — `None` when the page reached the end.
///
/// The token is the KEY OF THE FIRST RECORD NOT RETURNED, and a resumed call starts
/// at the first key at or after the token. Resuming is therefore well defined even
/// when the record the token names was deleted between the two calls: the scan simply
/// continues at the next key.
///
/// A key is an `EntityName` (`^[a-zA-Z_0-9-]*$`, 1..255 characters), which is a strict
/// subset of the modelled `NextToken` (`^[a-zA-Z_0-9-=+/]*$`, 1..1024), so the token
/// this returns always satisfies the token member's own constraints.
pub fn page<T>(
    ordered: impl IntoIterator<Item = (String, T)>,
    next_token: Option<&str>,
    page_size: usize,
) -> (Vec<T>, Option<String>) {
    let mut records = Vec::new();
    for (key, record) in ordered {
        if next_token.is_some_and(|token| key.as_str() < token) {
            continue;
        }
        if records.len() == page_size {
            return (records, Some(key));
        }
        records.push(record);
    }
    (records, None)
}

/// Could `value` be a `nextToken` this service issued?
///
/// Every token either list operation issues is a resource name, i.e. an `EntityName`:
/// `length 1..255, pattern ^[a-zA-Z_0-9-]*$`
/// (`smithy_query(query="operation",operation="ListMatchingWorkflows")`). The modelled
/// `NextToken` is wider (it also admits `=`, `+` and `/`), so a token the boundary
/// validator accepted can still be one this service never minted — refuse that rather
/// than serve an arbitrary suffix of the collection for it.
///
/// A token naming a record that has since been DELETED still satisfies this and is
/// still honoured — the scan resumes at the next name — so a page boundary survives a
/// concurrent delete, which is the behaviour a paging client depends on.
pub fn is_issued_page_token(value: &str) -> bool {
    !value.is_empty()
        && value.chars().count() <= 255
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
}

/// Could `value` be a `nextToken` `ListProviderServices` issued?
///
/// A provider-service page token is a catalog key —
/// `<providerName>/<providerServiceName>`, two `EntityName`s — so it is checked as
/// exactly that. The modelled `NextToken` admits far more (`=`, `+`, `/` anywhere, up
/// to 1024 characters), so a token the boundary validator accepted can still be one
/// this service never minted: refuse that rather than serve an arbitrary suffix of the
/// catalog for it.
pub fn is_issued_provider_service_token(value: &str) -> bool {
    value.split_once('/').is_some_and(|(provider_name, provider_service_name)| {
        is_issued_page_token(provider_name) && is_issued_page_token(provider_service_name)
    })
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsEntityresolutionAwsveniceserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsEntityresolutionAwsveniceserviceState>)
}
