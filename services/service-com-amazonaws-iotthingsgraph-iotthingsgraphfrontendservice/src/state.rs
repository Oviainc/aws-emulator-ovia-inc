// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-iotthingsgraph-iotthingsgraphfrontendservice service.
//!
//! One store per service, reached by every handler through [`service_state`]. The one
//! resource this crate persists so far is the workflow (flow template), held in a
//! `BTreeMap` keyed by its `urn:tdm:…:workflow:…` id so enumeration is stable and a
//! pagination token can name the id a page resumes at rather than an offset into a map
//! whose iteration order could change.
//!
//! A workflow is VERSIONED: every `CreateFlowTemplate`/`UpdateFlowTemplate` appends a
//! revision, and a revision is never rewritten, which is what lets `GetFlowTemplate`
//! answer for an older `revisionNumber` after an update.

use std::collections::{BTreeMap, BTreeSet};

use apollo_parser::cst::{self, CstNode};
use apollo_parser::{Parser, SyntaxKind};

use crate::ops::types::{FlowTemplateSummary, InternalFailureException};

/// The ARN namespace (and SigV4 signing name) this service's resources carry.
/// `smithy_query(query="service_info")` reports both as `iotthingsgraph`.
pub const ARN_SERVICE: &str = "iotthingsgraph";

/// The region used when a request carries no SigV4 credential scope to read one from
/// (an anonymous or host-routed request). Every generated emulator defaults here.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The name of the user's own namespace, which is the namespace a workflow is created
/// in: workflows "can be created only in the user's namespace" (`CreateFlowTemplate`
/// documentation), and the public namespace holds entities only.
pub const USER_NAMESPACE_NAME: &str = "default";

/// The URN prefix every Thing Definition Model id carries, from the `Urn` shape's
/// modelled pattern (`smithy_query(query="shape",shape="Urn")`).
pub const URN_PREFIX: &str = "urn:tdm:";

/// The URN type segment of a workflow.
pub const WORKFLOW_URN_TYPE: &str = "workflow";

/// The URN type segment of a device model. `SearchFlowTemplates`'s only modelled filter
/// is `DEVICE_MODEL_ID` (`smithy_query(query="shape",shape="FlowTemplateFilterName")`),
/// so this is the entity type a filter value has to name for it to narrow anything.
pub const DEVICE_MODEL_URN_TYPE: &str = "deviceModel";

/// The ARN resource-type segment of a workflow.
pub const WORKFLOW_ARN_RESOURCE_TYPE: &str = "Workflow";

/// The longest id this service mints or accepts, from `@length(0..160)` on the modelled
/// `Urn` shape. A workflow name long enough to push the derived id past it is refused
/// rather than stored under a truncated id.
pub const MAX_URN_LENGTH: usize = 160;

// ---------------------------------------------------------------------------
// What bounds this store's growth
//
// Every record below is created by a request and lives until the caller deletes it, so
// each store needs a ceiling or an unattended client loop grows it without limit. The
// real quotas are adjustable through Service Quotas, so the NUMBERS here are this
// emulator's own ceilings rather than values read off a doc page: each sits far above
// any realistic client workflow, so a caller never meets one by accident.
// ---------------------------------------------------------------------------

/// The most workflows this emulator holds. `CreateFlowTemplate` models
/// `LimitExceededException`, which is what it answers on this ceiling.
pub const MAX_FLOW_TEMPLATES: usize = 1_000;

/// The revision number `CreateFlowTemplate` stores a workflow's first definition under.
pub const FIRST_REVISION_NUMBER: i64 = 1;

/// The most revisions one workflow holds. `UpdateFlowTemplate` models no quota error, so
/// it refuses a revision past this ceiling as an invalid request.
pub const MAX_FLOW_TEMPLATE_REVISIONS: usize = 100;

/// The most definition text this emulator holds across every revision of every stored
/// workflow, in bytes. The record counts above do not bound the store's SIZE on their
/// own: `DefinitionText` is modelled at up to 1 MiB, so the bytes one request stores are
/// the caller's choice and this is what caps their total.
pub const MAX_STORED_DEFINITION_BYTES: usize = 64 * 1024 * 1024;

/// The largest page either paginated flow-template operation returns, and therefore the
/// page size a request that names no `maxResults` gets. It is the modelled
/// `@range(1..250)` maximum on `MaxResults`, and it is what caps how much one response
/// amplifies.
pub const MAX_PAGE_SIZE: usize = 250;

/// The most filters one `SearchFlowTemplates` request carries, and the most values one of
/// them names. `FlowTemplateFilters` and `FlowTemplateFilterValues` carry no modelled
/// `@length`, so without these the work one search costs is the PRODUCT of the stored
/// workflows and the criteria the request chose to match each of them against.
pub const MAX_SEARCH_FILTERS: usize = 10;
/// See [`MAX_SEARCH_FILTERS`].
pub const MAX_SEARCH_FILTER_VALUES: usize = 25;

/// The deepest nesting the GraphQL parser descends into one caller-supplied definition
/// document, and the most tokens it reads from one. `DefinitionText` is modelled at up to
/// 1 MiB, so without these a single request chooses how much stack and CPU one parse
/// costs; past either bound the parse fails and the request is refused.
pub const DEFINITION_RECURSION_LIMIT: usize = 64;
/// See [`DEFINITION_RECURSION_LIMIT`].
pub const DEFINITION_TOKEN_LIMIT: usize = 100_000;

/// The longest `nextToken` this service decodes. `NextToken` carries no modelled length,
/// so this is the decoder's own bound on a caller-supplied token.
pub const MAX_PAGE_TOKEN_LENGTH: usize = 4_096;

/// The modelled server error a handler answers with when the engine it was constructed
/// from is not the one that holds this service's store. One constructor so the wording
/// has a single home.
pub fn state_unavailable() -> InternalFailureException {
    InternalFailureException {
        message: Some("the IoTThingsGraph state store is unavailable".to_string()),
    }
}

/// One `urn:tdm:<namespace>:<type>:<name>` id, split into the three segments the
/// modelled `Urn` grammar gives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityUrn {
    /// The namespace the entity lives in — `<region>/<account>/default` for the user's
    /// own namespace, a path like `aws/examples` for a public one.
    pub namespace: String,
    /// The entity kind: `workflow`, `deviceModel`, `device`, …
    pub entity_type: String,
    /// The entity's own name.
    pub name: String,
}

/// The segments of `id`, or `None` when `id` is not a Thing Definition Model URN at all.
///
/// Splitting rather than matching: the modelled `Urn` pattern is
/// `urn:tdm:<namespace>:<type>:<name>` where the type is alphabetic and neither the
/// namespace nor the name may contain `:`, so three `:`-separated segments after the
/// prefix is the whole grammar. Total — it returns `None`, never panics, on anything a
/// caller sends.
pub fn parse_entity_urn(id: &str) -> Option<EntityUrn> {
    let body = id.strip_prefix(URN_PREFIX)?;
    let mut segments = body.split(':');
    let namespace = segments.next()?;
    let entity_type = segments.next()?;
    let name = segments.next()?;
    if segments.next().is_some() {
        return None;
    }
    if !is_namespace_path(namespace) || !is_entity_name(name) {
        return None;
    }
    if !entity_type.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    Some(EntityUrn {
        namespace: namespace.to_string(),
        entity_type: entity_type.to_string(),
        name: name.to_string(),
    })
}

/// The workflow name `id` carries, or `None` when `id` does not name a workflow —
/// either because it is not a URN or because its type segment names another entity kind.
pub fn parse_workflow_urn(id: &str) -> Option<EntityUrn> {
    parse_entity_urn(id).filter(|urn| urn.entity_type == WORKFLOW_URN_TYPE)
}

/// True when every `/`-separated segment is non-empty and holds only the characters the
/// modelled `Urn` pattern allows a namespace path: alphanumerics and `_`, plus the `-`
/// that a `<region>/<account>/<name>` namespace's region segment carries.
fn is_namespace_path(namespace: &str) -> bool {
    !namespace.is_empty()
        && namespace.split('/').all(|segment| {
            !segment.is_empty()
                && segment
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        })
}

/// True when every `/`-separated segment is non-empty and holds only the alphanumerics
/// and `_` the modelled `Urn` pattern allows an entity name.
fn is_entity_name(name: &str) -> bool {
    !name.is_empty()
        && name.split('/').all(|segment| {
            !segment.is_empty()
                && segment.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
}

/// The user's own namespace in `region`: `<region>/<account>/default`.
///
/// The account segment comes from the single canonical source
/// ([`engine::emulator_account_id`]), never a literal.
pub fn user_namespace_path(region: &str) -> String {
    format!(
        "{region}/{}/{USER_NAMESPACE_NAME}",
        ::engine::emulator_account_id()
    )
}

/// The id of the workflow called `name` in `namespace`:
/// `urn:tdm:<namespace>:workflow:<name>`.
pub fn workflow_id(namespace: &str, name: &str) -> String {
    format!("{URN_PREFIX}{namespace}:{WORKFLOW_URN_TYPE}:{name}")
}

/// The ARN of the workflow called `name` in the user's namespace:
/// `arn:aws:iotthingsgraph:<region>:<account>:Workflow/default/<name>`.
pub fn workflow_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{WORKFLOW_ARN_RESOURCE_TYPE}/{USER_NAMESPACE_NAME}/{name}"),
    )
}

/// The region this request was signed for, which is the region the ids and ARNs it mints
/// carry. Falls back to [`DEFAULT_REGION`] for a request with no SigV4 scope.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// One stored revision of a workflow. Never rewritten once appended.
#[derive(Debug, Clone)]
pub struct FlowTemplateRevision {
    /// 1 for the revision `CreateFlowTemplate` stored, then one more per update.
    pub revision_number: i64,
    /// The `DefinitionDocument$language` the caller uploaded, stored verbatim so the
    /// document a later read returns is byte-identical to the one that was uploaded.
    pub definition_language: String,
    /// The `DefinitionDocument$text` the caller uploaded, stored verbatim for the same
    /// reason.
    pub definition_text: String,
    /// The namespace version the request asked the workflow to be compatible with, when
    /// it named one.
    pub compatible_namespace_version: Option<i64>,
    /// The entity ids this revision's definition references, derived from the definition
    /// document when the revision was stored. `SearchFlowTemplates`'s `DEVICE_MODEL_ID`
    /// filter is answered from this set.
    pub referenced_entity_ids: BTreeSet<String>,
    pub created_at: ::aws_smithy_types::DateTime,
}

impl FlowTemplateRevision {
    /// The namespace version this revision was validated against: the one the request
    /// named, if it named one.
    ///
    /// `CreateFlowTemplateRequest$compatibleNamespaceVersion` is documented as "the
    /// namespace version in which the workflow is to be created", and
    /// `FlowTemplateDescription$validatedNamespaceVersion` as "the version of the user's
    /// namespace against which the workflow was validated" — so a request that names a
    /// version names the one its workflow is validated against, and that is the one
    /// reported back.
    ///
    /// A request that names none asks for "the latest version", which this emulator has
    /// no answer for yet: the latest version is a property of the namespace, which only
    /// `UploadEntityDefinitions` creates and `DescribeNamespace` reports, and both are
    /// still scaffolded. Reporting nothing is then the honest answer.
    pub fn validated_namespace_version(&self) -> Option<i64> {
        self.compatible_namespace_version
    }

    /// True when this revision's definition references the device model `id` names.
    ///
    /// The `DEVICE_MODEL_ID` filter selects on a DEVICE MODEL, so a value that names
    /// another entity kind narrows the search to nothing rather than matching a
    /// same-named reference of a different type.
    pub fn references_device_model(&self, id: &str) -> bool {
        parse_entity_urn(id).is_some_and(|urn| urn.entity_type == DEVICE_MODEL_URN_TYPE)
            && self.referenced_entity_ids.contains(id)
    }
}

/// A workflow as `CreateFlowTemplate` stored it, plus every revision appended since.
///
/// The first revision is held on its own rather than as the head of a vector, so "a
/// workflow always has at least one revision" is a property of the TYPE: every read of
/// the latest revision is then total, with no unwrap on a vector that could be empty.
#[derive(Debug, Clone)]
pub struct FlowTemplateRecord {
    pub id: String,
    pub arn: String,
    /// The revision `CreateFlowTemplate` stored, numbered 1.
    pub first_revision: FlowTemplateRevision,
    /// The revisions `UpdateFlowTemplate` appended, in ascending `revision_number`.
    pub appended_revisions: Vec<FlowTemplateRevision>,
}

impl FlowTemplateRecord {
    /// Every stored revision, oldest first.
    pub fn revisions(&self) -> impl Iterator<Item = &FlowTemplateRevision> {
        ::std::iter::once(&self.first_revision).chain(self.appended_revisions.iter())
    }

    /// How many revisions this workflow holds.
    pub fn revision_count(&self) -> usize {
        1 + self.appended_revisions.len()
    }

    /// The newest revision, which is what a read with no `revisionNumber` answers from.
    pub fn latest_revision(&self) -> &FlowTemplateRevision {
        self.appended_revisions.last().unwrap_or(&self.first_revision)
    }

    /// The revision numbered `revision_number`, or `None` when this workflow has none.
    pub fn revision(&self, revision_number: i64) -> Option<&FlowTemplateRevision> {
        self.revisions()
            .find(|revision| revision.revision_number == revision_number)
    }

    /// The number the next appended revision carries.
    pub fn next_revision_number(&self) -> i64 {
        self.latest_revision().revision_number.saturating_add(1)
    }

    /// The `FlowTemplateSummary` that reports one revision of this workflow.
    pub fn summary(&self, revision: &FlowTemplateRevision) -> FlowTemplateSummary {
        FlowTemplateSummary {
            id: Some(self.id.clone()),
            arn: Some(self.arn.clone()),
            revision_number: Some(revision.revision_number),
            created_at: Some(revision.created_at),
        }
    }
}

/// Per-service state for com-amazonaws-iotthingsgraph-iotthingsgraphfrontendservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsIotthingsgraphIotthingsgraphfrontendserviceState {
    /// Workflows by their `urn:tdm:…:workflow:…` id.
    pub flow_templates: BTreeMap<String, FlowTemplateRecord>,
    /// How much definition text the store currently holds, in bytes: the sum over every
    /// revision of every workflow, maintained as revisions are stored and removed so the
    /// ceiling can be checked without walking the whole store.
    pub stored_definition_bytes: usize,
}

impl ComAmazonawsIotthingsgraphIotthingsgraphfrontendserviceState {
    /// The total definition text the store would hold with `bytes` more, or `None` when
    /// that would take it past [`MAX_STORED_DEFINITION_BYTES`] — in which case the caller
    /// refuses the request instead of storing it.
    ///
    /// The caller assigns the returned total to `stored_definition_bytes` once the
    /// revision really is stored, so a refusal between the two cannot leave the count
    /// claiming bytes nothing holds.
    pub fn definition_bytes_after(&self, bytes: usize) -> Option<usize> {
        let total = self.stored_definition_bytes.saturating_add(bytes);
        (total <= MAX_STORED_DEFINITION_BYTES).then_some(total)
    }

    /// Give back the definition text every revision of `record` held, as the record
    /// leaves the store.
    pub fn release_definition_bytes(&mut self, record: &FlowTemplateRecord) {
        let bytes: usize = record
            .revisions()
            .map(|revision| revision.definition_text.len())
            .sum();
        self.stored_definition_bytes = self.stored_definition_bytes.saturating_sub(bytes);
    }
}

/// What one workflow definition document says: the workflow it defines, and the entities
/// it references.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedDefinition {
    /// The name of the GraphQL operation the document defines, which is the name of the
    /// workflow.
    pub workflow_name: String,
    /// Every Thing Definition Model URN the document names, as the set
    /// [`FlowTemplateRevision::references_device_model`] answers from.
    pub referenced_entity_ids: BTreeSet<String>,
}

/// Why a definition document could not be read as a workflow definition. Each case is a
/// client error: the caller has to change the document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DefinitionError {
    /// The text is not a GraphQL document, or exceeded one of the parser's bounds.
    Unparseable(String),
    /// The document defines no single named operation to take the workflow's name from.
    NoWorkflowName,
}

impl DefinitionError {
    /// What to tell the caller, naming the member at fault.
    pub fn message(&self) -> String {
        match self {
            Self::Unparseable(detail) => format!(
                "Value at 'definition.text' is not a valid GraphQL document: {detail}"
            ),
            Self::NoWorkflowName => {
                "Value at 'definition.text' must define exactly one named GraphQL \
                 operation, whose name names the workflow"
                    .to_string()
            }
        }
    }
}

/// Read a workflow definition document: parse the GraphQL text and report the workflow it
/// names and the entities it references.
///
/// The workflow's name is the name of the document's operation definition, which is what
/// the minted `urn:tdm:…:workflow:<name>` id carries. GraphQL names are
/// `[_A-Za-z][_0-9A-Za-z]*`, a subset of the names the modelled `Urn` pattern allows, so
/// a parsed name is always a usable id segment; only the length of the whole id still has
/// to be checked against [`MAX_URN_LENGTH`].
///
/// The parse is bounded by [`DEFINITION_RECURSION_LIMIT`] and
/// [`DEFINITION_TOKEN_LIMIT`], so the depth and the amount of work one caller-supplied
/// document can cost do not come from the request.
pub fn parse_definition(text: &str) -> Result<ParsedDefinition, DefinitionError> {
    let tree = Parser::new(text)
        .recursion_limit(DEFINITION_RECURSION_LIMIT)
        .token_limit(DEFINITION_TOKEN_LIMIT)
        .parse();
    if let Some(error) = tree.errors().next() {
        return Err(DefinitionError::Unparseable(error.message().to_string()));
    }
    let document = tree.document();

    let mut operation_names: Vec<Option<String>> = Vec::new();
    for definition in document.definitions() {
        if let cst::Definition::OperationDefinition(operation) = definition {
            operation_names.push(operation.name().map(|name| name.text().to_string()));
        }
    }
    let workflow_name = match operation_names.as_slice() {
        [Some(name)] if !name.is_empty() => name.clone(),
        _ => return Err(DefinitionError::NoWorkflowName),
    };

    // Every entity a workflow step or variable refers to is named by its URN, and a URN
    // appears in the document as a string value — so the referenced set is the string
    // values that parse as one.
    let mut referenced_entity_ids = BTreeSet::new();
    for node in document.syntax().descendants() {
        if node.kind() != SyntaxKind::STRING_VALUE {
            continue;
        }
        let Some(value) = cst::StringValue::cast(node) else {
            continue;
        };
        let value = String::from(value);
        if parse_entity_urn(&value).is_some() {
            referenced_entity_ids.insert(value);
        }
    }

    Ok(ParsedDefinition {
        workflow_name,
        referenced_entity_ids,
    })
}

/// What one `nextToken` carries: a digest of the enumeration it continues, and the key the
/// next page resumes at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageCursor {
    /// [`page_scope_digest`] of the operation and selection the previous page was taken
    /// under.
    pub scope_digest: String,
    /// The key the next page starts at, inclusive: a workflow id for a template search,
    /// a revision number for a revision listing.
    pub resume_key: String,
}

/// A short digest of the enumeration a page belongs to, so a token can be recognised as
/// continuing that one listing and no other.
///
/// Eight bytes of SHA-256, hex-encoded: this distinguishes one scope from another, it is
/// not a security boundary, and keeping it short is what keeps a minted token small
/// however long the filter values that produced it were.
pub fn page_scope_digest(parts: &[&str]) -> String {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    for part in parts {
        // Length-prefixed, so two different part lists cannot hash the same bytes.
        hasher.update(part.len().to_be_bytes());
        hasher.update(part.as_bytes());
    }
    hex::encode(&hasher.finalize()[..8])
}

/// Encode a [`PageCursor`] into an opaque `nextToken`.
///
/// Hex rather than the bare key so the token is not a documented, hand-editable resource
/// name: a client must hand back what the previous page returned.
pub fn encode_page_token(cursor: &PageCursor) -> String {
    let payload = ::serde_json::json!({"s": cursor.scope_digest, "k": cursor.resume_key});
    hex::encode(payload.to_string())
}

/// The cursor a `nextToken` carries, or `None` when the token was not one this service
/// minted.
pub fn decode_page_token(token: &str) -> Option<PageCursor> {
    if token.len() > MAX_PAGE_TOKEN_LENGTH {
        return None;
    }
    let payload = String::from_utf8(hex::decode(token).ok()?).ok()?;
    let value: ::serde_json::Value = ::serde_json::from_str(&payload).ok()?;
    Some(PageCursor {
        scope_digest: value.get("s")?.as_str()?.to_string(),
        resume_key: value.get("k")?.as_str()?.to_string(),
    })
}

/// The page size a request asked for, or `None` when `maxResults` is outside the modelled
/// `@range(1..250)`.
///
/// A request that names no `maxResults` gets [`MAX_PAGE_SIZE`], which is what bounds the
/// response of a request that chose no bound of its own.
pub fn page_size(max_results: Option<i32>) -> Option<usize> {
    match max_results {
        None => Some(MAX_PAGE_SIZE),
        Some(requested) => usize::try_from(requested)
            .ok()
            .filter(|size| (1..=MAX_PAGE_SIZE).contains(size)),
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsIotthingsgraphIotthingsgraphfrontendserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsIotthingsgraphIotthingsgraphfrontendserviceState>)
}
