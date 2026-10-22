// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-entityresolution-awsveniceservice service.
//!
//! Three control-plane resources are persisted here, and every member of each record
//! is the MODELLED type of the member it is returned on, so a reader clones what the
//! writer stored and the two cannot drift: `CreateSchemaMapping` followed by
//! `GetSchemaMapping` returns the created record field-for-field.
//!
//! `hasWorkflows` is deliberately NOT a stored field. It is a property of the
//! relationship between the two maps — "is this schema mapping referenced by some
//! matching workflow's `inputSourceConfig`" — so storing it would let a workflow
//! create or delete leave a stale flag behind. It is derived on read by
//! [`ComAmazonawsEntityresolutionAwsveniceserviceState::schema_has_workflows`].

use std::collections::{BTreeMap, HashMap};

use crate::ops::types::{
    IdMappingIncrementalRunConfig, IdMappingTechniques, IdMappingWorkflowInputSource,
    IdMappingWorkflowOutputSource, IncrementalRunConfig, InputSource, OutputSource,
    ResolutionTechniques, SchemaInputAttribute,
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

/// Per-service state for com-amazonaws-entityresolution-awsveniceservice. A handler reaches it through [`service_state`].
///
/// Every map is a `BTreeMap`, not a `HashMap`, because the list operations are
/// `@paginated` and a page boundary is only meaningful over a stable order: the
/// `nextToken` a page returns is the key of the first record it did NOT return, so
/// the next call resumes at that key.
#[derive(Debug, Default)]
pub struct ComAmazonawsEntityresolutionAwsveniceserviceState {
    /// Every schema mapping that exists, keyed by `schemaName`.
    pub schema_mappings: BTreeMap<String, SchemaMapping>,
    /// Every matching workflow that exists, keyed by `workflowName`.
    pub matching_workflows: BTreeMap<String, MatchingWorkflow>,
    /// Every ID mapping workflow that exists, keyed by `workflowName`.
    ///
    /// A SEPARATE namespace from [`Self::matching_workflows`]: the two resources have
    /// their own ARN types (`matchingworkflow/…` vs `idmappingworkflow/…`), their own
    /// create/get/list/delete operations and their own URI prefixes, so the same name
    /// can name one of each.
    pub id_mapping_workflows: BTreeMap<String, IdMappingWorkflow>,
}

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

    /// `CreateSchemaMapping`'s `ConflictException$message` (HTTP 400) when the
    /// `schemaName` is already taken. AWS answers this constant with no resource name
    /// in it; `ConflictException`'s own doc gives "Schema already exists" as the
    /// example of the condition.
    pub const CREATE_SCHEMA_MAPPING_CONFLICT: &str = "SchemaMapping already exists.";

    /// `CreateMatchingWorkflow`'s `ConflictException$message` (HTTP 400) when the
    /// `workflowName` is already taken. Not itself recorded; it is this operation's
    /// own constant, in the same form as the recorded
    /// [`CREATE_SCHEMA_MAPPING_CONFLICT`] and spelled with the resource type
    /// `ConflictException`'s doc names ("Workflow already exists").
    pub const CREATE_MATCHING_WORKFLOW_CONFLICT: &str = "MatchingWorkflow already exists.";

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

    /// `DeleteSchemaMapping`'s `ConflictException$message` (HTTP 400) when a matching
    /// workflow still names the schema in its `inputSourceConfig` — the refusal this
    /// operation's own doc describes ("This operation will fail if there is a
    /// MatchingWorkflow object that references the SchemaMapping in the workflow's
    /// InputSourceConfig"). Not recorded; a constant, not a composed sentence.
    pub const DELETE_SCHEMA_MAPPING_IN_USE: &str =
        "SchemaMapping is referenced by the InputSourceConfig of a MatchingWorkflow";

    /// `DeleteMatchingWorkflow`'s `ResourceNotFoundException$message` (HTTP 404).
    ///
    /// This is the one path where the service's text is a TEMPLATE over the request:
    /// the `workflowName` @httpLabel the caller asked for is substituted into it. The
    /// template is the rule — any name goes through it unchanged.
    pub fn delete_matching_workflow_not_found(workflow_name: &str) -> String {
        format!("MatchingWorkflow {workflow_name} does not exist")
    }

    /// `UpdateMatchingWorkflow`'s `ResourceNotFoundException$message` (HTTP 404) when
    /// no workflow carries the `workflowName` @httpLabel. Not itself recorded; it is
    /// this operation's own constant. `UpdateMatchingWorkflow` addresses the SAME
    /// resource by the SAME key as `GetMatchingWorkflow` ("The workflow must already
    /// exist for this operation to succeed"), and AWS's text for that lookup names no
    /// resource name, so this follows the recorded
    /// [`GET_MATCHING_WORKFLOW_NOT_FOUND`] form rather than the templated delete one.
    pub const UPDATE_MATCHING_WORKFLOW_NOT_FOUND: &str = "MatchingWorkflow Not Found";

    /// `DeleteSchemaMapping`'s `ResourceNotFoundException$message` (HTTP 404) — the
    /// refusal its doc describes ("This operation returns a ResourceNotFoundException
    /// if a schema with the given name does not exist"). Not itself recorded; this
    /// delete path follows the same request-templated form as the recorded
    /// [`delete_matching_workflow_not_found`], with the `schemaName` @httpLabel
    /// substituted.
    pub fn delete_schema_mapping_not_found(schema_name: &str) -> String {
        format!("SchemaMapping {schema_name} does not exist")
    }
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

/// A quota value as `ExceedsLimitException$quotaValue` models it (an `Integer`).
///
/// Every maximum is a small constant, so the conversion cannot fail; it is written
/// without a panic so a handler stays total.
pub fn quota_value(maximum: usize) -> Option<i32> {
    i32::try_from(maximum).ok()
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

impl ComAmazonawsEntityresolutionAwsveniceserviceState {
    /// Is `schema_name` referenced by the `inputSourceConfig` of any stored matching
    /// workflow?
    ///
    /// `InputSource$schemaName` is the modelled reference from a workflow to a schema
    /// mapping ("The name of the schema to be retrieved",
    /// `smithy_query(query="shape",shape="InputSource")`), so this is what both
    /// `hasWorkflows` and `DeleteSchemaMapping`'s conflict check read.
    pub fn schema_has_workflows(&self, schema_name: &str) -> bool {
        self.matching_workflows.values().any(|workflow| {
            workflow
                .input_source_config
                .iter()
                .any(|source| source.schema_name == schema_name)
        })
    }
}

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
    let millis = ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        });
    ::aws_smithy_types::DateTime::from_millis(millis)
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
