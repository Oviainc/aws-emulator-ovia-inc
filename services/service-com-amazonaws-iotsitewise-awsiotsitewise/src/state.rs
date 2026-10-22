// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-iotsitewise-awsiotsitewise service.
//!
//! The asset-model / asset / property-value and gateway slices of IoT SiteWise
//! live here: one map per resource plus the indexes the modelled lookup forms need
//! (`externalId:<id>` labels, property aliases, a gateway's platform device). Every handler reads and writes
//! THIS struct through [`service_state`]; nothing in `src/ops/` keeps state of its
//! own.

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    AssetHierarchy, AssetModelCompositeModel, AssetModelCompositeModelPathSegment,
    AssetModelCompositeModelSummary, AssetModelHierarchy, AssetModelProperty,
    AssetModelPropertyDefinition, AssetModelStatus, AssetProperty, AssetPropertyValue, AssetStatus,
    AssetSummary, GatewayCapabilitySummary, GatewayPlatform, TimeInNanos,
};

/// The `iotsitewise` ARN service namespace, as `OPERATIONS.md` reports it.
pub const ARN_SERVICE: &str = "iotsitewise";

/// The region every ARN falls back to when the request carries no SigV4
/// credential scope to read one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// How long after the call that starts it this emulator takes to settle a
/// resource's owed transition — `CREATING` -> `ACTIVE` after a create, and
/// `UPDATING` -> `ACTIVE` after an update.
///
/// CHOSEN, not documented: AWS publishes no duration for these transitions, only
/// that `CreateAsset`/`CreateAssetModel` answer with the resource in `CREATING`,
/// that `UpdateAsset` answers with it in `UPDATING`, and that it becomes `ACTIVE`
/// later. The span is deliberately non-zero so a caller that reads the resource
/// straight after the call still observes the transient state the real service
/// reports.
pub const ACTIVATION_DELAY: Duration = Duration::from_millis(500);

/// `AssetState`/`AssetModelState` value a freshly created resource reports.
pub const STATE_CREATING: &str = "CREATING";
/// `AssetState`/`AssetModelState` value a settled resource reports.
pub const STATE_ACTIVE: &str = "ACTIVE";
/// `AssetState`/`AssetModelState` value a resource reports while a mutating call
/// it already accepted settles.
pub const STATE_UPDATING: &str = "UPDATING";
/// `AssetState`/`AssetModelState` value a deleted resource's last answer carries.
pub const STATE_DELETING: &str = "DELETING";

/// How many characters an `ETag` carries: the modelled type is
/// `^[\w-]{43}$`, so every tag this service reports is exactly this long.
pub const ETAG_LENGTH: usize = 43;

/// The composite model types `assetModelCompositeModelType` accepts.
///
/// The member is modelled as a free `Name` string rather than an enum, so the
/// catalog comes from its documentation, which both
/// `CreateAssetModelCompositeModel` and `DescribeAssetModelCompositeModel` state
/// as "The composite model type. Valid values are AWS/ALARM, CUSTOM, or
/// AWS/L4E_ANOMALY".
pub const COMPOSITE_MODEL_TYPES: [&str; 3] = ["AWS/ALARM", "CUSTOM", "AWS/L4E_ANOMALY"];

/// The `AssetModelType` value an asset model must carry to be reused as the
/// component of a component-model-based composite model, as
/// `CreateAssetModelCompositeModel` documents it: "specify the
/// composedAssetModelId of an existing asset model with assetModelType of
/// COMPONENT_MODEL".
pub const MODEL_TYPE_COMPONENT: &str = "COMPONENT_MODEL";

/// How many property definitions one composite model may carry.
///
/// A PUBLISHED AWS quota, stated by `assetModelCompositeModelProperties`: "You
/// can specify up to 200 properties per composite model."
pub const MAX_PROPERTIES_PER_COMPOSITE_MODEL: usize = 200;

/// The ceiling on how many composite models one asset model holds.
///
/// The same bound, and for the same reason, as [`MAX_ASSET_MODELS`]: a create is
/// the only thing that adds a composite model and nothing evicts one, so the
/// per-model list needs a maximum entry count. The emulator's own bound, not a
/// published AWS default, and high enough that no ordinary workflow meets it;
/// `CreateAssetModelCompositeModel` refuses past it with the modelled
/// `LimitExceededException` (HTTP 410).
pub const MAX_COMPOSITE_MODELS_PER_ASSET_MODEL: usize = 100;

/// The prefix a `CustomID` label uses to name a resource by its external id.
pub const EXTERNAL_ID_PREFIX: &str = "externalId:";

/// The ceiling on how many asset models and how many assets this store holds:
/// the two caps that bound caller-driven growth of the resource maps.
///
/// A generated emulator keeps its resources in memory for the life of the process
/// and a create is the only thing that adds one, so the store needs a maximum
/// entry count — neither a TTL nor an eviction policy is available to it, because
/// `DescribeAssetModel`/`DescribeAsset` must still answer what the create created.
/// `CreateAssetModel` and `CreateAsset` refuse past these caps with the modelled
/// `LimitExceededException` (HTTP 410), which IoT SiteWise declares on exactly the
/// operations that grow the store and on no read. The numbers are the emulator's
/// own bound, not a published AWS default: AWS publishes both quotas as adjustable,
/// so no single value is the contract, and the caps are set high enough that no
/// ordinary workflow meets one.
pub const MAX_ASSET_MODELS: usize = 1_000;

/// See [`MAX_ASSET_MODELS`].
pub const MAX_ASSETS: usize = 10_000;

/// See [`MAX_ASSET_MODELS`]: the same bound, for the gateway map. `CreateGateway`
/// is the only thing that adds a gateway and it declares the modelled
/// `LimitExceededException` (HTTP 410), so that is how this store refuses past the
/// cap. The emulator's own bound, not a published AWS default.
pub const MAX_GATEWAYS: usize = 1_000;

/// The `gatewayVersion` a `CreateGateway` call that named none gets, as the member's
/// own documentation states: "If not specified, the default is 2 (Classic streams, V2
/// gateway)".
pub const DEFAULT_GATEWAY_VERSION: &str = "2";

/// The `gatewayVersion` whose GreengrassV2 platform must also carry
/// `coreDeviceOperatingSystem`, as `GatewayPlatform`'s documentation states: "If
/// creating a V3 gateway (gatewayVersion=3), you must also specify the
/// coreDeviceOperatingSystem".
pub const GATEWAY_VERSION_V3: &str = "3";

/// How many samples one `(assetId, propertyId)` series keeps.
///
/// Unlike the resource caps above, a time series DOES have an eviction policy
/// available to it: `GetAssetPropertyValue` reports the latest value, so the
/// OLDEST sample is the one that can go. Past this many samples
/// [`ComAmazonawsIotsitewiseAwsiotsitewiseState::record_property_value`] drops the
/// oldest, so repeated writes cannot grow one series without limit. The emulator's
/// own bound; AWS publishes a retention period rather than a sample count.
pub const MAX_SAMPLES_PER_SERIES: usize = 1_000;

/// A stored asset model: everything `DescribeAssetModel` reports, plus the
/// bookkeeping (`version`, `tags`, `active_at`) the other operations read.
#[derive(Debug, Clone)]
pub struct AssetModelRecord {
    pub id: String,
    pub external_id: Option<String>,
    pub arn: String,
    pub name: String,
    pub description: Option<String>,
    /// An `AssetModelType` value: `ASSET_MODEL`, `COMPONENT_MODEL` or `INTERFACE`.
    pub model_type: String,
    pub properties: Vec<AssetModelProperty>,
    pub hierarchies: Vec<AssetModelHierarchy>,
    pub composite_models: Vec<AssetModelCompositeModel>,
    /// `assetModelCompositeModelId` -> the links the modelled
    /// `AssetModelCompositeModel` shape has no member for: the composite model
    /// it is nested inside, and the component model it reuses. Absent for a
    /// top-level, inline composite model, which is what every composite model
    /// defined by `CreateAssetModel` is.
    pub composite_model_links: HashMap<String, CompositeModelLink>,
    pub creation_date: SystemTime,
    pub last_update_date: SystemTime,
    /// The instant the transition the clock owes — from [`Self::settling_state`]
    /// to `ACTIVE` — falls due.
    pub active_at: SystemTime,
    /// The `AssetModelState` this model reports until [`Self::active_at`] falls
    /// due: [`STATE_CREATING`] for a freshly created model, [`STATE_UPDATING`]
    /// while a mutating call it already accepted settles.
    pub settling_state: String,
    pub version: u32,
    pub tags: HashMap<String, String>,
}

/// What a composite model is attached to, beside the asset model that holds it.
#[derive(Debug, Clone, Default)]
pub struct CompositeModelLink {
    /// The `assetModelCompositeModelId` of the composite model this one is
    /// nested inside, when it is not top-level.
    pub parent_id: Option<String>,
    /// The `assetModelId` of the `COMPONENT_MODEL` this composite model reuses,
    /// for a component-model-based composite model. An inline one has none.
    pub composed_asset_model_id: Option<String>,
}

impl AssetModelRecord {
    /// The model's status as of `now`: its [`Self::settling_state`] until the
    /// owed transition falls due, `ACTIVE` from then on. Derived, so a read
    /// never has to write.
    pub fn status(&self, now: SystemTime) -> AssetModelStatus {
        AssetModelStatus {
            state: self.state(now).to_string(),
            error: None,
        }
    }

    /// The model's `AssetModelState` as of `now`.
    pub fn state(&self, now: SystemTime) -> &str {
        state_at(&self.settling_state, self.active_at, now)
    }

    /// Record that a mutating call — a composite model added to this model, say
    /// — landed at `now`: the definition carries a new version, and the model
    /// reports `UPDATING` until the transition that call owes falls due.
    ///
    /// The due instant is STORED, never slept on, exactly as
    /// [`AssetRecord::begin_update`] does it for an asset. A model that had not
    /// finished being created keeps reporting `CREATING`: the pending transition
    /// it already owed is the one that settles, and it is the same one.
    pub fn begin_update(&mut self, now: SystemTime) {
        self.last_update_date = now;
        self.version += 1;
        if self.state(now) != STATE_CREATING {
            self.settling_state = STATE_UPDATING.to_string();
        }
        self.active_at = now + ACTIVATION_DELAY;
    }

    /// The composite model stored under `id`, if this model holds one.
    pub fn composite_model(&self, id: &str) -> Option<&AssetModelCompositeModel> {
        self.composite_models
            .iter()
            .find(|composite| composite.id.as_deref() == Some(id))
    }

    /// Resolve a `CustomID` label — a raw id or `externalId:<externalId>` — to
    /// the `assetModelCompositeModelId` of a composite model of THIS model.
    pub fn resolve_composite_model_id(&self, label: &str) -> Option<String> {
        match label.strip_prefix(EXTERNAL_ID_PREFIX) {
            Some(external) => self
                .composite_models
                .iter()
                .find(|composite| composite.external_id.as_deref() == Some(external))
                .and_then(|composite| composite.id.clone()),
            None => self.composite_model(label).and_then(|composite| composite.id.clone()),
        }
    }

    /// The id of the composite model `id` is nested inside, if any.
    pub fn composite_model_parent(&self, id: &str) -> Option<&str> {
        self.composite_model_links
            .get(id)?
            .parent_id
            .as_deref()
    }

    /// The `COMPONENT_MODEL` asset model composite model `id` reuses, if it is
    /// component-model-based.
    pub fn composed_asset_model_id(&self, id: &str) -> Option<&str> {
        self.composite_model_links
            .get(id)?
            .composed_asset_model_id
            .as_deref()
    }

    /// The ids of the composite models nested DIRECTLY inside `id`.
    pub fn composite_model_children(&self, id: &str) -> Vec<String> {
        self.composite_models
            .iter()
            .filter_map(|composite| {
                let child = composite.id.as_deref()?;
                (self.composite_model_parent(child) == Some(id)).then(|| child.to_string())
            })
            .collect()
    }

    /// The ids of the composite models this model holds directly: the ones that
    /// are nested inside no other composite model.
    pub fn top_level_composite_model_ids(&self) -> Vec<String> {
        self.composite_models
            .iter()
            .filter_map(|composite| {
                let id = composite.id.as_deref()?;
                self.composite_model_parent(id)
                    .is_none()
                    .then(|| id.to_string())
            })
            .collect()
    }

    /// The `AssetModelCompositeModelPath` of composite model `id`: one segment
    /// per composite model from the outermost ancestor down to `id` itself, so
    /// the path of a top-level composite model is the composite model alone.
    pub fn composite_model_path(&self, id: &str) -> Vec<AssetModelCompositeModelPathSegment> {
        let mut path = Vec::new();
        let mut at = Some(id.to_string());
        // Bounded by the number of stored composite models, so a link that
        // somehow pointed into a cycle cannot spin here.
        while let Some(current) = at.take() {
            let Some(composite) = self.composite_model(&current) else {
                break;
            };
            path.push(AssetModelCompositeModelPathSegment {
                id: Some(current.clone()),
                name: Some(composite.name.clone()),
            });
            if path.len() >= self.composite_models.len() {
                break;
            }
            at = self
                .composite_model_parent(&current)
                .map(ToString::to_string);
        }
        path.reverse();
        path
    }

    /// Composite model `id` as the summary both `DescribeAssetModel` and
    /// `DescribeAssetModelCompositeModel` report it as, so the two views of one
    /// stored composite model cannot disagree.
    pub fn composite_model_summary(&self, id: &str) -> Option<AssetModelCompositeModelSummary> {
        let composite = self.composite_model(id)?;
        Some(AssetModelCompositeModelSummary {
            id: id.to_string(),
            external_id: composite.external_id.clone(),
            name: composite.name.clone(),
            r#type: composite.r#type.clone(),
            description: composite.description.clone(),
            path: Some(self.composite_model_path(id)),
        })
    }
}

/// A stored asset. `properties` is materialised from the referenced model's
/// property definitions at create time, so property writes and reads resolve.
#[derive(Debug, Clone)]
pub struct AssetRecord {
    pub id: String,
    pub external_id: Option<String>,
    pub arn: String,
    pub name: String,
    pub asset_model_id: String,
    pub description: Option<String>,
    pub properties: Vec<AssetProperty>,
    pub hierarchies: Vec<AssetHierarchy>,
    pub creation_date: SystemTime,
    pub last_update_date: SystemTime,
    /// The instant the transition the clock owes — from [`Self::settling_state`]
    /// to `ACTIVE` — falls due.
    pub active_at: SystemTime,
    /// The `AssetState` this asset reports until [`Self::active_at`] falls due:
    /// [`STATE_CREATING`] for a freshly created asset, [`STATE_UPDATING`] while a
    /// mutating call it already accepted settles.
    pub settling_state: String,
    pub tags: HashMap<String, String>,
}

impl AssetRecord {
    /// Where in `properties` the property a `CustomID` label names sits — a raw
    /// property id, or `externalId:<externalId>` for one that carries an external
    /// id — if this asset holds it.
    pub fn property_position(&self, label: &str) -> Option<usize> {
        match label.strip_prefix(EXTERNAL_ID_PREFIX) {
            Some(external) => self
                .properties
                .iter()
                .position(|property| property.external_id.as_deref() == Some(external)),
            None => self.properties.iter().position(|property| property.id == label),
        }
    }

    /// The asset property `label` names, resolved the way every `CustomID` member
    /// is — the same lookup as [`AssetRecord::property_position`], for a caller
    /// that wants the property rather than where it sits.
    ///
    /// `DescribeTimeSeries` models its `propertyId` as `CustomID`, so both forms
    /// reach a handler and both must name the same stored property.
    pub fn property(&self, label: &str) -> Option<&AssetProperty> {
        self.properties.get(self.property_position(label)?)
    }

    /// The asset's status as of `now`; see [`AssetModelRecord::status`].
    pub fn status(&self, now: SystemTime) -> AssetStatus {
        AssetStatus {
            state: self.state(now).to_string(),
            error: None,
        }
    }

    /// The asset's `AssetState` as of `now`: its [`Self::settling_state`] until
    /// the owed transition falls due, `ACTIVE` from then on. Derived, so a read
    /// never has to write.
    pub fn state(&self, now: SystemTime) -> &str {
        state_at(&self.settling_state, self.active_at, now)
    }

    /// Record that a mutating call landed at `now`: the asset reports `UPDATING`
    /// until the transition that call owes falls due, and `ACTIVE` after it.
    ///
    /// The due instant is STORED, never slept on, so the caller's own call
    /// returns immediately and the transient state is still what a read taken
    /// straight afterwards observes.
    pub fn begin_update(&mut self, now: SystemTime) {
        self.last_update_date = now;
        self.settling_state = STATE_UPDATING.to_string();
        self.active_at = now + ACTIVATION_DELAY;
    }

    /// This asset as a `ListAssets` summary, as of `now`.
    pub fn summary(&self, now: SystemTime) -> AssetSummary {
        AssetSummary {
            id: self.id.clone(),
            external_id: self.external_id.clone(),
            arn: self.arn.clone(),
            name: self.name.clone(),
            asset_model_id: self.asset_model_id.clone(),
            creation_date: to_date_time(self.creation_date),
            last_update_date: to_date_time(self.last_update_date),
            status: self.status(now),
            hierarchies: self.hierarchies.clone(),
            description: self.description.clone(),
        }
    }
}

/// `settling` before the owed transition falls due, `ACTIVE` once it has.
fn state_at(settling: &str, active_at: SystemTime, now: SystemTime) -> &str {
    if now < active_at {
        settling
    } else {
        STATE_ACTIVE
    }
}

/// A stored gateway: everything `DescribeGateway` reports, plus the `tags` the
/// tagging operations read.
///
/// `capability_summaries` starts EMPTY: a freshly created gateway has no
/// capability configured, and only `UpdateGatewayCapabilityConfiguration` adds one.
#[derive(Debug, Clone)]
pub struct GatewayRecord {
    pub id: String,
    pub arn: String,
    pub name: String,
    pub platform: GatewayPlatform,
    pub version: String,
    pub capability_summaries: Vec<GatewayCapabilitySummary>,
    pub creation_date: SystemTime,
    pub last_update_date: SystemTime,
    pub tags: HashMap<String, String>,
}

/// The platform device a gateway runs on, as a single comparable key: the
/// Greengrass V1 group ARN, the Greengrass V2 core device thing name or the Siemens
/// Industrial Edge IoT Core thing name, whichever the platform names.
///
/// `None` when the platform names no device at all — a request
/// [`GatewayPlatform`]'s documentation refuses, since one gateway specifies exactly
/// one platform type.
pub fn gateway_device_key(platform: &GatewayPlatform) -> Option<String> {
    if let Some(greengrass) = platform.greengrass.as_ref() {
        return Some(format!("greengrass:{}", greengrass.group_arn));
    }
    if let Some(greengrass_v2) = platform.greengrass_v2.as_ref() {
        return Some(format!(
            "greengrassV2:{}",
            greengrass_v2.core_device_thing_name
        ));
    }
    platform
        .siemens_ie
        .as_ref()
        .map(|siemens| format!("siemensIE:{}", siemens.iot_core_thing_name))
}

/// How many platform types `platform` names. Exactly one is valid; see
/// [`gateway_device_key`].
pub fn gateway_platform_count(platform: &GatewayPlatform) -> usize {
    [
        platform.greengrass.is_some(),
        platform.greengrass_v2.is_some(),
        platform.siemens_ie.is_some(),
    ]
    .into_iter()
    .filter(|named| *named)
    .count()
}

/// Per-service state for com-amazonaws-iotsitewise-awsiotsitewise. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsIotsitewiseAwsiotsitewiseState {
    /// Asset models by `assetModelId`.
    pub asset_models: HashMap<String, AssetModelRecord>,
    /// `assetModelExternalId` -> `assetModelId`, so `externalId:` labels resolve.
    pub asset_model_external_ids: HashMap<String, String>,
    /// Assets by `assetId`.
    pub assets: HashMap<String, AssetRecord>,
    /// `assetExternalId` -> `assetId`, so `externalId:` labels resolve.
    pub asset_external_ids: HashMap<String, String>,
    /// `(assetId, hierarchyId)` -> the child assets associated under that
    /// hierarchy. `ListAssets`'s `TOP_LEVEL` filter reads it to tell a root asset
    /// from one that is some other asset's child.
    pub asset_hierarchy_children: HashMap<(String, String), Vec<String>>,
    /// `assetId` -> `propertyId` -> the property's time series, kept sorted
    /// ascending by timestamp so the latest value is the last element.
    ///
    /// Nested rather than keyed by an `(assetId, propertyId)` tuple so a lookup
    /// borrows the two ids instead of allocating a pair of `String`s on every
    /// read and on every value of a batch write.
    pub property_values: HashMap<String, HashMap<String, Vec<AssetPropertyValue>>>,
    /// `propertyAlias` -> `(assetId, propertyId)`, so alias lookups resolve.
    pub property_alias_index: HashMap<String, (String, String)>,
    /// Gateways by `gatewayId`.
    pub gateways: HashMap<String, GatewayRecord>,
    /// [`gateway_device_key`] -> `gatewayId`: which gateway, if any, already runs
    /// on a given platform device. One device hosts at most one gateway, so this
    /// is what makes a second create naming the same device a conflict.
    pub gateway_devices: HashMap<String, String>,
}

impl ComAmazonawsIotsitewiseAwsiotsitewiseState {
    /// Resolve a `CustomID` label — a raw id or `externalId:<externalId>` — to a
    /// stored `assetModelId`.
    pub fn resolve_asset_model_id(&self, label: &str) -> Option<String> {
        match label.strip_prefix(EXTERNAL_ID_PREFIX) {
            Some(external) => self.asset_model_external_ids.get(external).cloned(),
            None => self.asset_models.contains_key(label).then(|| label.to_string()),
        }
    }

    /// Resolve a `CustomID` label — a raw id or `externalId:<externalId>` — to a
    /// stored `assetId`.
    pub fn resolve_asset_id(&self, label: &str) -> Option<String> {
        match label.strip_prefix(EXTERNAL_ID_PREFIX) {
            Some(external) => self.asset_external_ids.get(external).cloned(),
            None => self.assets.contains_key(label).then(|| label.to_string()),
        }
    }

    /// Make `alias` the one alias the `(asset_id, property_id)` property carries,
    /// or remove the property's alias with `None`.
    ///
    /// A binding lives in TWO places — the stored property's `alias` member, which
    /// `DescribeAsset` reports, and [`Self::property_alias_index`], which the alias
    /// form of a property lookup resolves through — so both move together here and
    /// nowhere else. The alias the property carried before is dropped from the
    /// index (only when the index still attributes it to THIS property, so a
    /// binding another property has since taken over survives).
    ///
    /// `asset_id` is a stored id, not a label: resolve it with
    /// [`Self::resolve_asset_id`] first. `property_id` takes either `CustomID`
    /// form. Answers `false` when no stored asset holds that property.
    ///
    /// A caller that binds an alias another property already carries displaces it:
    /// that property's own `alias` member is cleared here too, so no two stored
    /// properties can ever report one alias while the index names only the newer.
    /// `UpdateAssetProperty` refuses such a request with the modelled conflict
    /// before reaching this helper; this keeps the store consistent for any caller
    /// whose own rule allows the rebind.
    pub fn set_property_alias(
        &mut self,
        asset_id: &str,
        property_id: &str,
        alias: Option<String>,
    ) -> bool {
        let Some(asset) = self.assets.get_mut(asset_id) else {
            return false;
        };
        let Some(at) = asset.property_position(property_id) else {
            return false;
        };
        let property = &mut asset.properties[at];
        let resolved_property_id = property.id.clone();
        let previous = property.alias.take();
        property.alias = alias.clone();

        if let Some(previous) = previous {
            let ours = self
                .property_alias_index
                .get(&previous)
                .is_some_and(|(bound_asset, bound_property)| {
                    bound_asset == asset_id && *bound_property == resolved_property_id
                });
            if ours {
                self.property_alias_index.remove(&previous);
            }
        }
        if let Some(alias) = alias {
            // Whoever held this alias loses it, record and index together.
            if let Some((held_by_asset, held_by_property)) =
                self.property_alias_index.insert(
                    alias,
                    (asset_id.to_string(), resolved_property_id.clone()),
                )
            {
                let displaced = held_by_asset != asset_id
                    || held_by_property != resolved_property_id;
                if displaced {
                    if let Some(asset) = self.assets.get_mut(&held_by_asset) {
                        if let Some(at) = asset.property_position(&held_by_property) {
                            asset.properties[at].alias = None;
                        }
                    }
                }
            }
        }
        true
    }

    /// Whether ANY stored asset model holds a composite model under `id`.
    ///
    /// A composite model id is globally unique, so both the collision check on a
    /// caller-supplied id and the mint draw ask this one question.
    pub fn composite_model_id_taken(&self, id: &str) -> bool {
        self.asset_models
            .values()
            .any(|model| model.composite_model(id).is_some())
    }

    /// Append `value` to the `(asset_id, property_id)` series, keeping it sorted
    /// ascending by timestamp.
    ///
    /// A value at an already-stored timestamp is QUALITY-AWARE: "For each asset
    /// property, IoT SiteWise overwrites TQVs with duplicate timestamps unless the
    /// newer TQV has a different quality" (the `BatchPutAssetPropertyValue` model
    /// doc), so the stored sample is replaced only when the two name the same
    /// quality, and a differing quality leaves the series exactly as it was. Either
    /// way the series stays single-valued at that timestamp.
    ///
    /// The series is BOUNDED at [`MAX_SAMPLES_PER_SERIES`]: past that many samples
    /// the oldest goes, so no sequence of writes can grow one series without
    /// limit. A sample newer than everything stored — the shape real ingest takes
    /// — takes an append-only fast path and costs no element move, so the sorted
    /// insert cannot be driven to quadratic cost under the shared state lock.
    pub fn record_property_value(
        &mut self,
        asset_id: &str,
        property_id: &str,
        value: AssetPropertyValue,
    ) {
        let series = self
            .property_values
            .entry(asset_id.to_string())
            .or_default()
            .entry(property_id.to_string())
            .or_default();
        let key = timestamp_key(&value.timestamp);
        if series
            .last()
            .is_none_or(|last| timestamp_key(&last.timestamp) < key)
        {
            series.push(value);
        } else {
            match series.binary_search_by(|existing| timestamp_key(&existing.timestamp).cmp(&key))
            {
                Ok(at) => {
                    if quality_of(&series[at]) != quality_of(&value) {
                        // Accepted and discarded: the stored TQV wins at a
                        // timestamp a differing-quality TQV re-puts.
                        return;
                    }
                    series[at] = value;
                }
                Err(at) => series.insert(at, value),
            }
        }
        if series.len() > MAX_SAMPLES_PER_SERIES {
            let excess = series.len() - MAX_SAMPLES_PER_SERIES;
            series.drain(..excess);
        }
    }

    /// The highest-timestamp value stored for `(asset_id, property_id)`, if any.
    pub fn latest_property_value(
        &self,
        asset_id: &str,
        property_id: &str,
    ) -> Option<&AssetPropertyValue> {
        self.property_values
            .get(asset_id)?
            .get(property_id)?
            .last()
    }

    /// Forget an asset and everything keyed off it: its external-id entry, its
    /// property series and its property aliases.
    pub fn forget_asset(&mut self, asset_id: &str) -> Option<AssetRecord> {
        let record = self.assets.remove(asset_id)?;
        if let Some(external) = record.external_id.as_deref() {
            self.asset_external_ids.remove(external);
        }
        self.property_values.remove(asset_id);
        self.property_alias_index
            .retain(|_, (asset, _)| asset != asset_id);
        self.asset_hierarchy_children
            .retain(|(asset, _), _| asset != asset_id);
        for children in self.asset_hierarchy_children.values_mut() {
            children.retain(|child| child != asset_id);
        }
        Some(record)
    }

    /// Forget an asset model and its external-id entry.
    pub fn forget_asset_model(&mut self, asset_model_id: &str) -> Option<AssetModelRecord> {
        let record = self.asset_models.remove(asset_model_id)?;
        if let Some(external) = record.external_id.as_deref() {
            self.asset_model_external_ids.remove(external);
        }
        Some(record)
    }
}

/// Turn `AssetModelPropertyDefinition`s into the stored `AssetModelProperty`s,
/// minting an id for every definition that did not supply one.
///
/// Shared by every operation that takes property definitions, so one rule is
/// enforced in one place. Two rules the shapes' own documentation states are
/// enforced here: a property definition carries exactly ONE property type ("You
/// can only specify one type in a property definition"), and a `STRUCT` property
/// also carries `dataTypeSpec` ("This parameter is required on properties that
/// have the STRUCT data type"). The `Err` carries the refusal message, which the
/// calling operation lifts into its own modelled `InvalidRequestException`.
pub fn property_definitions_to_properties(
    definitions: &[AssetModelPropertyDefinition],
) -> Result<Vec<AssetModelProperty>, String> {
    let mut properties = Vec::with_capacity(definitions.len());
    for definition in definitions {
        let kinds = [
            definition.r#type.attribute.is_some(),
            definition.r#type.measurement.is_some(),
            definition.r#type.transform.is_some(),
            definition.r#type.metric.is_some(),
        ]
        .into_iter()
        .filter(|set| *set)
        .count();
        if kinds != 1 {
            return Err(format!(
                "The property definition {} must specify exactly one property type \
                 (attribute, measurement, transform or metric), but specifies {kinds}.",
                definition.name
            ));
        }
        if definition.data_type == "STRUCT" && definition.data_type_spec.is_none() {
            return Err(format!(
                "The property definition {} has the STRUCT data type, so dataTypeSpec is required.",
                definition.name
            ));
        }
        properties.push(AssetModelProperty {
            id: Some(definition.id.clone().unwrap_or_else(new_id)),
            external_id: definition.external_id.clone(),
            name: definition.name.clone(),
            data_type: definition.data_type.clone(),
            data_type_spec: definition.data_type_spec.clone(),
            unit: definition.unit.clone(),
            r#type: definition.r#type.clone(),
            path: None,
        });
    }
    Ok(properties)
}

/// The entity tag of an asset model's stored definition.
///
/// CONTENT-DERIVED, never minted: the SHA-256 digest of the fields a write to the
/// model changes, rendered as hex and cut to the 43 characters the modelled
/// `ETag` type fixes (`^[\w-]{43}$`, and a hex digit is a `\w`). Two reads of an
/// unchanged model therefore report the same tag and any change to the definition
/// reports a different one, which is what the `If-Match` / `If-None-Match`
/// preconditions on the asset-model writes compare against. 43 hex characters
/// carry 172 bits of the digest, and the tag is opaque to the caller: nothing but
/// equality is ever asked of it. Hex because this crate already mints its ids and
/// pagination tokens with `hex`, so the tag needs no further encoder.
///
/// The stored definition goes into the fingerprint through `{:?}`, which covers
/// every member of these shapes — a hand-listed set of fields would silently stop
/// covering a member the model later grows, and a tag that misses a change is the
/// one failure mode a precondition cannot tolerate. The exact bytes are not a
/// contract: a caller reads a tag and writes it back within one process.
///
/// This emulator keeps ONE version of each asset model, so its latest and its
/// active version share this tag; while an owed transition is still pending the
/// model has no active version at all (see [`AssetModelRecord::state`]).
pub fn asset_model_etag(record: &AssetModelRecord) -> String {
    // The links are a map, so they go into the fingerprint in key order: two
    // models with the same definition must hash to the same tag whatever order
    // their maps happen to iterate in.
    let mut links: Vec<_> = record.composite_model_links.iter().collect();
    links.sort_by_key(|(id, _)| *id);
    let fingerprint = format!(
        "{}|{}|{}|{}|{}|{:?}|{:?}|{:?}|{:?}",
        record.id,
        record.version,
        record.name,
        record.description.as_deref().unwrap_or(""),
        record.model_type,
        record.properties,
        record.hierarchies,
        record.composite_models,
        links,
    );
    let digest = <::sha2::Sha256 as ::sha2::Digest>::digest(fingerprint.as_bytes());
    let mut tag = ::hex::encode(digest);
    tag.truncate(ETAG_LENGTH);
    tag
}

/// Total order on a `TimeInNanos`: whole seconds first, then the nanosecond
/// offset, exactly as the modelled members are defined.
pub fn timestamp_key(timestamp: &TimeInNanos) -> (i64, i32) {
    (
        timestamp.time_in_seconds,
        timestamp.offset_in_nanos.unwrap_or(0),
    )
}

/// The `Quality` a TQV that names none carries.
///
/// `AssetPropertyValue$quality` is optional and the enum's values are `GOOD`,
/// `BAD` and `UNCERTAIN` (`smithy_query(query="shape", shape="Quality")`). An
/// ordinary device reading is `GOOD`, so a TQV ingested without an explicit
/// quality is treated as `GOOD`: an omitted quality and an explicit `GOOD` name
/// the SAME quality, and neither can shadow the other at a timestamp the series
/// already holds.
pub const DEFAULT_QUALITY: &str = "GOOD";

/// The quality `value` names, with [`DEFAULT_QUALITY`] standing in for an absent
/// one. This is the comparison the quality-aware write in
/// [`ComAmazonawsIotsitewiseAwsiotsitewiseState::record_property_value`] makes.
pub fn quality_of(value: &AssetPropertyValue) -> &str {
    value.quality.as_deref().unwrap_or(DEFAULT_QUALITY)
}

/// A wall-clock instant as the modelled `Timestamp` type.
pub fn to_date_time(at: SystemTime) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(at)
}

/// The region to build this request's ARNs in: the one in the caller's SigV4
/// credential scope, or [`DEFAULT_REGION`] for an unsigned request.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The canonical ARN of an asset model in `region`.
pub fn asset_model_arn(region: &str, asset_model_id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("asset-model/{asset_model_id}"))
}

/// The canonical ARN of an asset in `region`.
pub fn asset_arn(region: &str, asset_id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("asset/{asset_id}"))
}

/// The canonical ARN of a gateway in `region`, in the form `CreateGateway`
/// documents: `arn:${Partition}:iotsitewise:${Region}:${Account}:gateway/${GatewayId}`.
pub fn gateway_arn(region: &str, gateway_id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("gateway/{gateway_id}"))
}

/// The character joining the two ids a `TimeSeriesId` is built from; see
/// [`time_series_id`].
///
/// A `.`, because the id is carried inside `timeSeriesArn` and the modelled `ARN`
/// pattern is `^arn:aws(-cn|-us-gov)?:[a-zA-Z0-9-:\/_\.]+$` — a separator outside
/// that character class would make every ARN this service reports violate a
/// constraint its own model states.
pub const TIME_SERIES_ID_SEPARATOR: char = '.';

/// The `TimeSeriesId` of the time series (data stream) an asset property owns.
///
/// Every asset property IS a data stream in IoT SiteWise's time-series store, so
/// the series it owns needs an id that is stable across reads and unique per
/// `(assetId, propertyId)` pair. The modelled `TimeSeriesId` is 36..73 characters
/// — one 36-character `ID`, or two of them joined by a single character — so the
/// pair that owns the series names it, derived rather than minted: nothing has to
/// be written on a read for the id to stay the same. Both ids are modelled `ID`s,
/// whose pattern admits neither [`TIME_SERIES_ID_SEPARATOR`] nor any other
/// character outside `[0-9a-f-]`, so one pair cannot be spelled as another.
pub fn time_series_id(asset_id: &str, property_id: &str) -> String {
    format!("{asset_id}{TIME_SERIES_ID_SEPARATOR}{property_id}")
}

/// The canonical ARN of a time series in `region`, in the form the
/// `TimeSeriesSummary$timeSeriesArn` doc states:
/// `arn:${Partition}:iotsitewise:${Region}:${Account}:time-series/${TimeSeriesId}`.
pub fn time_series_arn(region: &str, time_series_id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("time-series/{time_series_id}"))
}

/// The MQTT topic IoT SiteWise publishes one asset property's value updates to.
///
/// The form is the one the IoT SiteWise User Guide documents for property-value
/// update notifications ("Interacting with other AWS services"):
/// `$aws/sitewise/asset-models/{assetModelId}/assets/{assetId}/properties/{propertyId}`.
/// Built from the three ids, so the topic a property reports always names the
/// property it belongs to rather than being stored and able to drift.
pub fn property_notification_topic(
    asset_model_id: &str,
    asset_id: &str,
    property_id: &str,
) -> String {
    format!(
        "$aws/sitewise/asset-models/{asset_model_id}/assets/{asset_id}/properties/{property_id}"
    )
}

/// The `PropertyNotificationState` a property takes when `UpdateAssetProperty`
/// omits one: the model's own documented default for that member.
pub const NOTIFICATION_STATE_DISABLED: &str = "DISABLED";

/// How many ids this process has minted. Part of every id's seed, so no two
/// minted ids collide even when two calls land in the same nanosecond.
static MINTED: ::std::sync::atomic::AtomicU64 = ::std::sync::atomic::AtomicU64::new(0);

/// A fresh UUIDv4 in the lowercase hyphenated form every `ID` member models.
///
/// Built the way the other services in this workspace mint ids (cf. `mint_hex` in
/// the ivs service): a seed of a monotonic mint counter, the wall clock and the
/// process id, hashed with SHA-256, then given RFC 4122's version-4 and variant
/// bits. Setting the version nibble also makes the all-zero UUID — the one value
/// the modelled `ID` pattern refuses — unreachable.
pub fn new_id() -> String {
    let minted = MINTED.fetch_add(1, ::std::sync::atomic::Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let seed = format!("iotsitewise-id|{minted}|{}|{nanos}", ::std::process::id());
    let digest = <::sha2::Sha256 as ::sha2::Digest>::digest(seed.as_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = ::hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsIotsitewiseAwsiotsitewiseState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsIotsitewiseAwsiotsitewiseState>)
}
