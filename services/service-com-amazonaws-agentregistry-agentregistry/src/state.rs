// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-agentregistry-agentregistry service.
//!
//! The Agent Registry model this crate serves binds only the three *discovery
//! read* operations (`ListDiscoverableRegistryRecords`,
//! `BatchGetDiscoverableRegistryRecord`, `SearchDiscoverableRegistryRecords`);
//! registry and record authoring lives in a different model. So the store here is
//! populated by [`ComAmazonawsAgentregistryAgentregistryState::put_record`] — the
//! seam a test or a fixture loader seeds through — and every response is computed
//! from it.

use std::collections::BTreeMap;

use crate::ops::types::{
    Descriptors, DiscoverableRegistryRecordSummary, RegistryRecordSummary,
    ResourceNotFoundException, ValidationException, ValidationExceptionField,
};
use ::aws_smithy_types::{DateTime, Document};

/// The region every ARN this service mints is scoped to.
///
/// Registry records are region-scoped resources, and nothing in a discovery read
/// names a region, so the store records the one it was seeded in.
pub const DEFAULT_REGION: &str = "us-east-1";

/// This service's ARN namespace (`arnNamespace` / sigv4 signing name in the model).
pub const ARN_SERVICE: &str = "agent-registry";

/// The only `RegistryRecordStatus` that is discoverable.
///
/// From the model's own `RegistryRecordStatus` doc: a record is "`APPROVED` once it
/// is approved and discoverable", while "`REJECTED` and `DEPRECATED` records are not
/// discoverable" and `DRAFT` / `PENDING_APPROVAL` / `CREATING` / `UPDATING` /
/// `CREATE_FAILED` / `UPDATE_FAILED` describe a record that has not become
/// discoverable yet.
pub const STATUS_APPROVED: &str = "APPROVED";

/// The descriptor entry keys present on `descriptors`, derived from the structure
/// itself rather than stored a second time.
///
/// The keys are the member names the `Descriptors` structure models — the values
/// `DiscoverableRegistryRecordSummary$descriptorTypes` carries ("Each value
/// corresponds to a descriptor entry key on the approved record") and the values a
/// `descriptorType` filter matches against. `Descriptors` is destructured
/// exhaustively, so a member added to the model fails to compile here until its
/// entry key is named.
pub fn descriptor_types(descriptors: &Descriptors) -> Vec<String> {
    let Descriptors {
        mcp_server,
        a2a_agent_card,
        agent_skills_definition,
        custom,
        http,
        agui,
    } = descriptors;
    let entries = [
        ("mcpServer", mcp_server.is_some()),
        ("a2aAgentCard", a2a_agent_card.is_some()),
        ("agentSkillsDefinition", agent_skills_definition.is_some()),
        ("custom", custom.is_some()),
        ("http", http.is_some()),
        ("agui", agui.is_some()),
    ];
    entries
        .into_iter()
        .filter(|(_, present)| *present)
        .map(|(key, _)| key.to_string())
        .collect()
}

/// The registry id inside a `RegistryIdentifier`, which the model lets a caller
/// supply either as a bare id or as the registry's full ARN
/// (`arn:aws:agent-registry:<region>:<account>:registry/<id>`).
pub fn registry_id_from_identifier(identifier: &str) -> &str {
    match identifier.rsplit_once(":registry/") {
        Some((_, registry_id)) => registry_id,
        None => identifier,
    }
}

/// `ValidationExceptionReason$FieldValidationFailed`, one of the model's own
/// `VALIDATION_EXCEPTION_REASON_VALUES`.
pub const REASON_FIELD_VALIDATION_FAILED: &str = "FieldValidationFailed";

/// `ValidationExceptionReason$CannotParse`, one of the model's own
/// `VALIDATION_EXCEPTION_REASON_VALUES`.
pub const REASON_CANNOT_PARSE: &str = "CannotParse";

/// The modelled `ValidationException` naming the member that failed and why.
///
/// Shared by every operation so one refusal is worded one way; each operation
/// lifts it into its own error channel with `.into()`.
pub fn field_validation(member: &str, message: &str) -> ValidationException {
    ValidationException {
        message: format!("{member}: {message}"),
        reason: REASON_FIELD_VALIDATION_FAILED.to_string(),
        field_list: Some(vec![ValidationExceptionField {
            name: member.to_string(),
            message: message.to_string(),
        }]),
    }
}

/// The fixed text of the refusal a discovery read answers for a registry this
/// service does not hold. The identifier follows it and is the only variable part.
const REGISTRY_NOT_FOUND: &str = "Registry not found: ";

/// The modelled `ResourceNotFoundException` a discovery read answers when the
/// `RegistryIdentifier` it was given names no registry this store holds.
///
/// Shared by every operation that resolves a registry, so one refusal is worded
/// one way. The identifier is stated exactly as the request supplied it — a bare
/// registry id or the registry's full ARN — never the id parsed back out of it,
/// so a caller reads its own value back.
pub fn registry_not_found(identifier: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!("{REGISTRY_NOT_FOUND}{identifier}")),
    }
}

/// The modelled `ValidationException` for a request value this service cannot
/// parse at all (an opaque token it did not mint, say).
pub fn cannot_parse(message: &str) -> ValidationException {
    ValidationException {
        message: message.to_string(),
        reason: REASON_CANNOT_PARSE.to_string(),
        field_list: None,
    }
}

/// The registry and record a `RecordIdentifier` names: a record ARN carries its
/// own `registry/<id>` segment, while a bare record id carries none.
pub fn record_identifier_parts(identifier: &str) -> (Option<&str>, &str) {
    match identifier.split_once("/record/") {
        Some((prefix, record_id)) => (Some(registry_id_from_identifier(prefix)), record_id),
        None => (None, identifier),
    }
}

/// The record id inside a `RecordIdentifier`, which the model lets a caller supply
/// either as a bare id or as the record's full ARN
/// (`arn:aws:agent-registry:<region>:<account>:registry/<id>/record/<recordId>`).
pub fn record_id_from_identifier(identifier: &str) -> &str {
    record_identifier_parts(identifier).1
}

/// One registry record, as the registry stores it.
#[derive(Debug, Clone)]
pub struct RegistryRecord {
    pub record_id: String,
    /// Derived by [`ComAmazonawsAgentregistryAgentregistryState::put_record`], so
    /// every operation reports the same ARN for the same record.
    pub record_arn: String,
    /// The owning registry's ARN, derived the same way.
    pub registry_arn: String,
    pub name: String,
    pub display_name: Option<String>,
    pub description: Option<String>,
    pub record_type: String,
    pub record_version: String,
    pub status: String,
    pub descriptors: Descriptors,
    pub custom_metadata: Option<Document>,
    pub created_at: DateTime,
    pub updated_at: DateTime,
}

impl RegistryRecord {
    /// A record with the members the model marks `@required`. The two ARNs are
    /// filled in when the record is inserted into a registry, so a caller never
    /// spells them out (and cannot spell them inconsistently).
    pub fn new(
        record_id: impl Into<String>,
        name: impl Into<String>,
        record_type: impl Into<String>,
        record_version: impl Into<String>,
        status: impl Into<String>,
        created_at: DateTime,
    ) -> Self {
        Self {
            record_id: record_id.into(),
            record_arn: String::new(),
            registry_arn: String::new(),
            name: name.into(),
            display_name: None,
            description: None,
            record_type: record_type.into(),
            record_version: record_version.into(),
            status: status.into(),
            descriptors: Descriptors::default(),
            custom_metadata: None,
            created_at,
            updated_at: created_at,
        }
    }

    #[must_use]
    pub fn with_display_name(mut self, display_name: impl Into<String>) -> Self {
        self.display_name = Some(display_name.into());
        self
    }

    #[must_use]
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    #[must_use]
    pub fn with_descriptors(mut self, descriptors: Descriptors) -> Self {
        self.descriptors = descriptors;
        self
    }

    #[must_use]
    pub fn with_custom_metadata(mut self, custom_metadata: Document) -> Self {
        self.custom_metadata = Some(custom_metadata);
        self
    }

    #[must_use]
    pub fn with_updated_at(mut self, updated_at: DateTime) -> Self {
        self.updated_at = updated_at;
        self
    }

    /// Whether a discovery read may return this record at all.
    pub fn is_discoverable(&self) -> bool {
        self.status == STATUS_APPROVED
    }

    /// The descriptor entry keys present on this record.
    pub fn descriptor_types(&self) -> Vec<String> {
        descriptor_types(&self.descriptors)
    }

    /// The full `RegistryRecordSummary` — what `BatchGetDiscoverableRegistryRecord`
    /// and `SearchDiscoverableRegistryRecords` return. Both go through this one
    /// conversion, so the same record renders identically in either response.
    pub fn summary(&self) -> RegistryRecordSummary {
        RegistryRecordSummary {
            registry_arn: self.registry_arn.clone(),
            record_arn: self.record_arn.clone(),
            record_id: self.record_id.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
            display_name: self.display_name.clone(),
            record_type: self.record_type.clone(),
            descriptors: self.descriptors.clone(),
            record_version: self.record_version.clone(),
            status: self.status.clone(),
            created_at: self.created_at,
            updated_at: self.updated_at,
            custom_metadata: self.custom_metadata.clone(),
        }
    }

    /// The list summary — `DiscoverableRegistryRecordSummary` carries no
    /// `descriptors` (the model's doc: "This summary does not include
    /// descriptors") but does carry their entry keys.
    pub fn discoverable_summary(&self) -> DiscoverableRegistryRecordSummary {
        DiscoverableRegistryRecordSummary {
            registry_arn: self.registry_arn.clone(),
            record_arn: self.record_arn.clone(),
            record_id: self.record_id.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
            display_name: self.display_name.clone(),
            record_type: self.record_type.clone(),
            record_version: self.record_version.clone(),
            status: self.status.clone(),
            created_at: self.created_at,
            updated_at: self.updated_at,
            descriptor_types: Some(self.descriptor_types()),
        }
    }
}

/// One registry and the records it owns, keyed by record id so iteration order is
/// stable and a pagination token can resume from a record id.
#[derive(Debug, Clone)]
pub struct Registry {
    pub registry_id: String,
    pub registry_arn: String,
    pub records: BTreeMap<String, RegistryRecord>,
}

impl Registry {
    /// This registry's discoverable records, in record-id order.
    pub fn discoverable_records(&self) -> impl Iterator<Item = &RegistryRecord> {
        self.records
            .values()
            .filter(|record| record.is_discoverable())
    }

    /// A discoverable record by id; `None` for an unknown id and for one whose
    /// status keeps it out of the discovery surface.
    pub fn discoverable_record(&self, record_id: &str) -> Option<&RegistryRecord> {
        self.records
            .get(record_id)
            .filter(|record| record.is_discoverable())
    }
}

/// Per-service state for com-amazonaws-agentregistry-agentregistry. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsAgentregistryAgentregistryState {
    /// Registries keyed by registry id.
    pub registries: BTreeMap<String, Registry>,
    /// The region this store's ARNs are scoped to.
    pub region: String,
}

impl Default for ComAmazonawsAgentregistryAgentregistryState {
    fn default() -> Self {
        Self {
            registries: BTreeMap::new(),
            region: DEFAULT_REGION.to_string(),
        }
    }
}

impl ComAmazonawsAgentregistryAgentregistryState {
    /// The account every ARN this store mints carries. There is exactly one source
    /// of truth for it, in the vendored runtime.
    pub fn account_id(&self) -> &'static str {
        ::engine::emulator_account_id()
    }

    /// `arn:aws:agent-registry:<region>:<account>:registry/<registryId>`.
    pub fn registry_arn(&self, registry_id: &str) -> String {
        ::engine::emulator_arn(ARN_SERVICE, &self.region, &format!("registry/{registry_id}"))
    }

    /// `arn:aws:agent-registry:<region>:<account>:registry/<registryId>/record/<recordId>`.
    pub fn record_arn(&self, registry_id: &str, record_id: &str) -> String {
        ::engine::emulator_arn(
            ARN_SERVICE,
            &self.region,
            &format!("registry/{registry_id}/record/{record_id}"),
        )
    }

    /// The registry a `RegistryIdentifier` names (bare id or full ARN), or `None`
    /// when this store holds no such registry.
    pub fn registry(&self, identifier: &str) -> Option<&Registry> {
        self.registries
            .get(registry_id_from_identifier(identifier))
    }

    /// Create the registry if it is absent, and return it.
    pub fn put_registry(&mut self, registry_id: &str) -> &mut Registry {
        let registry_arn = self.registry_arn(registry_id);
        self.registries
            .entry(registry_id.to_string())
            .or_insert_with(|| Registry {
                registry_id: registry_id.to_string(),
                registry_arn,
                records: BTreeMap::new(),
            })
    }

    /// Insert `record` into `registry_id`, creating that registry if needed, and
    /// deriving both ARNs from the stored ids, region and account so every response
    /// reports the same ones.
    pub fn put_record(&mut self, registry_id: &str, mut record: RegistryRecord) {
        record.registry_arn = self.registry_arn(registry_id);
        record.record_arn = self.record_arn(registry_id, &record.record_id);
        let registry = self.put_registry(registry_id);
        registry.records.insert(record.record_id.clone(), record);
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsAgentregistryAgentregistryState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsAgentregistryAgentregistryState>)
}
