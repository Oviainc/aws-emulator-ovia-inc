// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-athena-amazonathena service.
//!
//! Athena's workgroup registry lives here. `CreateWorkGroup` writes a
//! [`WorkGroupRecord`] into [`ComAmazonawsAthenaAmazonathenaState::work_groups`] and
//! `GetWorkGroup` answers out of that same record, so every value a read reports is
//! one some earlier write persisted rather than one derived from the reading request.
//!
//! A workgroup has no lifecycle the clock owes: Athena answers `CreateWorkGroup` with
//! the workgroup already usable, so the record is stored in its terminal state and a
//! read never has to materialise a pending transition.

use std::collections::{BTreeMap, HashSet};
use std::time::SystemTime;

use crate::ops::types::{EngineVersion, Tag, WorkGroup, WorkGroupConfiguration};

/// The `WorkGroupState` a freshly created workgroup is in. One of
/// `WORK_GROUP_STATE_VALUES` in [`crate::ops::types`] (the model's `WorkGroupState`
/// enum, whose `WorkGroup$State` documentation reads "The state of the workgroup:
/// ENABLED or DISABLED").
pub const WORK_GROUP_STATE_ENABLED: &str = "ENABLED";
/// The other `WorkGroupState`; see [`WORK_GROUP_STATE_ENABLED`].
pub const WORK_GROUP_STATE_DISABLED: &str = "DISABLED";

/// The engine version that asks Athena to choose one.
///
/// `EngineVersion$SelectedEngineVersion` documents: "Possible values are determined by
/// the output of `ListEngineVersions`, including AUTO. The default is AUTO."
pub const ENGINE_VERSION_AUTO: &str = "AUTO";

/// The engine version this emulator resolves [`ENGINE_VERSION_AUTO`] to.
///
/// `EngineVersion$EffectiveEngineVersion` documents: "If the user requests Auto, the
/// effective engine version is chosen by Athena." Athena's own documentation names
/// Athena engine version 3 as the version new workgroups run on, so that is what this
/// emulator chooses. It is NOT read from the request: a caller that asks for AUTO gets
/// this version whatever else the request says.
pub const DEFAULT_EFFECTIVE_ENGINE_VERSION: &str = "Athena engine version 3";

/// How many tags one Athena resource carries.
///
/// Athena's own documentation states the limit: "You can have up to 50 tags per
/// resource." The model's `TagList` carries no length trait, so the limit is enforced
/// here, and a longer list is refused BEFORE anything walks or stores it.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The largest caller-supplied open collection a stored workgroup configuration may
/// hold, in bytes of keys and values (see [`open_configuration_bytes`]).
///
/// This is the EMULATOR's own bound and has NO AWS counterpart: Athena publishes no
/// limit on these members, so the number is chosen generously — far above any plausible
/// Spark property set, engine classification list or CloudWatch log-type map — and
/// exists only so that [`MAX_WORK_GROUPS`] times the size of one record is a bounded
/// product rather than one bounded factor times an unbounded one. Most
/// `WorkGroupConfiguration` members are already bounded by the model (a 1024-byte
/// description, a 128-byte `AdditionalConfiguration`, a 2048-byte `ExecutionRole`); its
/// Spark properties, engine classifications and CloudWatch log types are the open maps
/// and lists the model bounds in neither length nor element count. A configuration past
/// this bound is refused with the operation's own `InvalidRequestException`.
pub const MAX_WORK_GROUP_CONFIGURATION_BYTES: usize = 64 * 1024;

/// How many workgroups one account holds in this emulator.
///
/// Athena's published service quota is 1000 workgroups per Region per account; the
/// quota is not carried in the model, so this bound is enforced here and a write past
/// it is refused with the operation's own `InvalidRequestException` rather than
/// growing the emulator's heap without limit.
pub const MAX_WORK_GROUPS: usize = 1000;

/// One workgroup, as this service persists it.
///
/// The configuration is held already normalised (see
/// [`resolved_engine_version`]), so the create path decides the engine version once
/// and every later read reports that same decision.
#[derive(Debug, Clone)]
pub struct WorkGroupRecord {
    /// The workgroup name, which is also its key in the registry.
    pub name: String,
    /// A `WorkGroupState` value: [`WORK_GROUP_STATE_ENABLED`] or
    /// [`WORK_GROUP_STATE_DISABLED`].
    pub state: String,
    /// The description the create call carried, if any.
    pub description: Option<String>,
    /// The workgroup configuration, with the engine version resolved.
    pub configuration: WorkGroupConfiguration,
    /// When the workgroup was created, reported as `WorkGroup$CreationTime`.
    pub created_at: SystemTime,
    /// The tags the create call carried; keys are unique within this list.
    pub tags: Vec<Tag>,
}

impl WorkGroupRecord {
    /// The record as the modelled `WorkGroup` shape a read answers with.
    pub fn to_work_group(&self) -> WorkGroup {
        WorkGroup {
            name: self.name.clone(),
            state: Some(self.state.clone()),
            configuration: Some(self.configuration.clone()),
            description: self.description.clone(),
            creation_time: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            // Athena fills this only for an IAM Identity Center enabled workgroup,
            // whose Identity Center application this emulator does not create.
            identity_center_application_arn: None,
        }
    }
}

/// The engine version a workgroup runs on, given the one the request selected.
///
/// `EffectiveEngineVersion` is documented "Read only" and "When a request to update the
/// engine version is made by a `CreateWorkGroup` or `UpdateWorkGroup` operation, the
/// `EffectiveEngineVersion` field is ignored" — so whatever the caller put there is
/// dropped, and the effective version is the selected one, or
/// [`DEFAULT_EFFECTIVE_ENGINE_VERSION`] when the selection is
/// [`ENGINE_VERSION_AUTO`] (or absent, AUTO being the documented default).
pub fn resolved_engine_version(requested: Option<&EngineVersion>) -> EngineVersion {
    let selected = requested
        .and_then(|version| version.selected_engine_version.clone())
        .unwrap_or_else(|| ENGINE_VERSION_AUTO.to_string());
    let effective = if selected == ENGINE_VERSION_AUTO {
        DEFAULT_EFFECTIVE_ENGINE_VERSION.to_string()
    } else {
        selected.clone()
    };
    EngineVersion {
        selected_engine_version: Some(selected),
        effective_engine_version: Some(effective),
    }
}

/// Whether any tag in `tags` carries no key.
///
/// `Tag$Key` is 1 to 128 characters, so a present key is already checked at the
/// boundary; the member itself is optional in the model, and a tag with no key at all
/// names nothing to tag. Refused on its own terms, before keys are compared for
/// uniqueness.
pub fn tag_without_key(tags: &[Tag]) -> bool {
    tags.iter()
        .any(|tag| tag.key.as_deref().unwrap_or_default().is_empty())
}

/// The first tag key that appears more than once in `tags`, if any.
///
/// `Tag$Key` documents "Tag keys are case-sensitive and must be unique per resource",
/// so a tag list carrying one key twice describes no resource Athena would accept.
/// Linear in the list length: the keys already seen are held in a set, never rescanned.
pub fn first_duplicate_tag_key(tags: &[Tag]) -> Option<&str> {
    let mut seen: HashSet<&str> = HashSet::with_capacity(tags.len());
    for tag in tags {
        let key = tag.key.as_deref().unwrap_or_default();
        if !seen.insert(key) {
            return Some(key);
        }
    }
    None
}

/// The bytes of keys and values held in `configuration`'s OPEN collections — the ones
/// the model bounds neither in length nor in element count: the engine configuration's
/// additional configs, Spark properties and classifications, and the CloudWatch logging
/// configuration's log types. Checked against
/// [`MAX_WORK_GROUP_CONFIGURATION_BYTES`] before a configuration is stored.
///
/// Linear in the size of the request it measures, and it allocates nothing.
pub fn open_configuration_bytes(configuration: &WorkGroupConfiguration) -> usize {
    let mut bytes = 0usize;
    if let Some(engine) = configuration.engine_configuration.as_ref() {
        for map in [
            engine.additional_configs.as_ref(),
            engine.spark_properties.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            for (key, value) in map {
                bytes = bytes.saturating_add(key.len()).saturating_add(value.len());
            }
        }
        for classification in engine.classifications.iter().flatten() {
            bytes = bytes
                .saturating_add(classification.name.as_ref().map_or(0, String::len));
            for (key, value) in classification.properties.iter().flatten() {
                bytes = bytes.saturating_add(key.len()).saturating_add(value.len());
            }
        }
    }
    let log_types = configuration
        .monitoring_configuration
        .as_ref()
        .and_then(|monitoring| monitoring.cloud_watch_logging_configuration.as_ref())
        .and_then(|logging| logging.log_types.as_ref());
    for (key, values) in log_types.into_iter().flatten() {
        bytes = bytes.saturating_add(key.len());
        for value in values {
            bytes = bytes.saturating_add(value.len());
        }
    }
    bytes
}

/// Per-service state for com-amazonaws-athena-amazonathena. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsAthenaAmazonathenaState {
    /// Every workgroup this account holds, keyed by its name. A `BTreeMap` so a
    /// listing walks the registry in one stable order.
    pub work_groups: BTreeMap<String, WorkGroupRecord>,
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsAthenaAmazonathenaState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsAthenaAmazonathenaState>)
}
