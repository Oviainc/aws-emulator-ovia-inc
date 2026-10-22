// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-iotsitewise-awsiotsitewise service.
//!
//! The asset-model / asset / property-value and gateway slices of IoT SiteWise
//! live here: one map per resource plus the indexes the modelled lookup forms need
//! (`externalId:<id>` labels, property aliases, a gateway's platform device). A
//! resource's tags live on that resource's own record, reached by ARN through
//! [`ComAmazonawsIotsitewiseAwsiotsitewiseState::tags_for_resource`], so the
//! tagging operations and the creates share one tag set per resource. A data
//! stream no asset property owns lives in its own map
//! ([`ComAmazonawsIotsitewiseAwsiotsitewiseState::disassociated_time_series`]),
//! which is what `DisassociateTimeSeriesFromAssetProperty` leaves behind. Every
//! handler reads and writes THIS struct through [`service_state`]; nothing in
//! `src/ops/` keeps state of its own.

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    AggregatedValue, Aggregates, AssetHierarchy, AssetModelCompositeModel,
    AssetModelCompositeModelPathSegment, AssetModelCompositeModelSummary, AssetModelHierarchy,
    AssetModelProperty, AssetModelPropertyDefinition, AssetModelStatus, AssetProperty,
    AssetPropertyValue, AssetStatus, AssetSummary, ExpressionVariable, GatewayCapabilitySummary,
    GatewayPlatform, PropertyType, TimeInNanos, Variant,
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

/// How many property definitions one asset model may carry.
///
/// A PUBLISHED AWS quota, stated by both `CreateAssetModel`'s and
/// `UpdateAssetModel`'s `assetModelProperties` member: "You can specify up to 200
/// properties per asset model." A write that would leave a model carrying more is
/// refused with the modelled `LimitExceededException` (HTTP 410).
pub const MAX_PROPERTIES_PER_ASSET_MODEL: usize = 200;

/// How many hierarchy definitions one asset model may carry.
///
/// A PUBLISHED AWS quota, stated by both `CreateAssetModel`'s and
/// `UpdateAssetModel`'s `assetModelHierarchies` member: "You can specify up to 10
/// hierarchies per asset model." Refused the same way as
/// [`MAX_PROPERTIES_PER_ASSET_MODEL`].
pub const MAX_HIERARCHIES_PER_ASSET_MODEL: usize = 10;

/// The `AssetModelVersionType` value naming a model's latest version, and the
/// documented default of every `matchForVersionType` member.
pub const VERSION_TYPE_LATEST: &str = "LATEST";

/// The `AssetModelVersionType` value naming a model's active version.
pub const VERSION_TYPE_ACTIVE: &str = "ACTIVE";

/// The one value a `SelectAll` member (`ifNoneMatch`) admits: its modelled
/// pattern is `^\*$`.
pub const SELECT_ALL: &str = "*";

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

/// The greatest number of tags one IoT SiteWise resource holds.
///
/// The model's own ceiling: `TagMap` carries `@length(min: 1, max: 50)` wherever
/// it is used — `TagResource.tags`, `CreateAsset.tags`, `CreateAssetModel.tags`
/// and `ListTagsForResource.tags` — so no single call may carry more than this
/// many tags and no listing may report more. A `TagResource` call that would
/// leave a resource above it is refused with the modelled `TooManyTagsException`
/// (HTTP 400), whose own documentation is "You've reached the quota for the
/// number of tags allowed for a resource".
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The `TagKey` bounds: `TagMap`'s key targets `TagKey`, which the model
/// declares as `@length(min: 1, max: 128)`.
pub const TAG_KEY_MIN_LENGTH: usize = 1;
/// See [`TAG_KEY_MIN_LENGTH`].
pub const TAG_KEY_MAX_LENGTH: usize = 128;

/// The ARN resource-type segment of an asset ARN, as [`asset_arn`] builds it.
pub const ASSET_RESOURCE_TYPE: &str = "asset";
/// The ARN resource-type segment of an asset-model ARN, as [`asset_model_arn`]
/// builds it.
pub const ASSET_MODEL_RESOURCE_TYPE: &str = "asset-model";
/// The ARN resource-type segment of a gateway ARN, as [`gateway_arn`] builds it.
pub const GATEWAY_RESOURCE_TYPE: &str = "gateway";

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

/// How many DISASSOCIATED time series this store keeps.
///
/// `DisassociateTimeSeriesFromAssetProperty` keeps the data stream it detaches
/// (see [`DisassociatedTimeSeriesRecord`]) and `DeleteTimeSeries` — the operation
/// that forgets one — is still a scaffold stub, so repeatedly binding and
/// detaching aliases would otherwise grow this map without limit. Like a series'
/// samples ([`MAX_SAMPLES_PER_SERIES`]) and unlike a resource an operation can
/// delete, an eviction policy IS available here: past this many series the least
/// recently updated one goes, since `DisassociateTimeSeriesFromAssetProperty`
/// declares no limit error to refuse with. The emulator's own bound, not a
/// published AWS quota, and high enough that no ordinary workflow meets it.
pub const MAX_DISASSOCIATED_TIME_SERIES: usize = 1_000;

/// A time series (data stream) that no asset property owns.
///
/// IoT SiteWise models a time series as a resource in its own right, separate
/// from the asset property it may be attached to: `ListTimeSeries`'s
/// `timeSeriesType` is `ASSOCIATED` or `DISASSOCIATED`
/// (`smithy_query(query="shape", shape="ListTimeSeriesType")`), a
/// `TimeSeriesSummary` carries `assetId` and `propertyId` only when it is
/// associated, and `DeleteTimeSeries` — not the disassociate call — is what
/// forgets one ("If the time series isn't associated with an asset property,
/// specify the alias of the time series"). So detaching a series from its asset
/// property leaves THIS record behind: the stream keeps the identity, alias, data
/// type and creation date it had while associated, and its last-update instant
/// moves to the call that detached it.
#[derive(Debug, Clone)]
pub struct DisassociatedTimeSeriesRecord {
    /// The alias that identifies the stream, and the only way to address it once
    /// no `(assetId, propertyId)` pair names it.
    pub alias: String,
    /// The stream's own `TimeSeriesId`: a fresh `ID`-shaped value, inside the
    /// modelled `TimeSeriesId` bound of 36..73 characters.
    ///
    /// Minted rather than carried over from the pair that owned it, because the
    /// asset property the stream left still has a series of its own that its ids
    /// name ([`time_series_id`]): two series are two ids, so a detached stream
    /// cannot keep reporting the pair-derived one. Stored, so it is stable across
    /// every read of this stream.
    pub time_series_id: String,
    /// A `PropertyDataType` value: the data type of the stream's samples.
    pub data_type: String,
    /// Set only for a `STRUCT` stream, as `TimeSeriesSummary$dataTypeSpec` states.
    pub data_type_spec: Option<String>,
    pub creation_date: SystemTime,
    pub last_update_date: SystemTime,
    /// The stream's samples, kept sorted ascending by timestamp exactly as the
    /// property's own series is. A time series IS the data stream, so the values
    /// written while it was associated travel with it rather than staying on the
    /// asset property, and come back to a property the alias is bound to again.
    pub values: Vec<AssetPropertyValue>,
}

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

    /// `id` and every composite model nested inside it, at any depth.
    ///
    /// Exactly the composite models that stop being reachable when `id` goes
    /// away: a nested composite model is addressed only through the chain of
    /// parents that contains it, so `DeleteAssetModelCompositeModel` removes the
    /// whole subtree rather than leaving records no path reaches. Each id is
    /// listed once, so a link that somehow pointed into a cycle cannot spin here.
    pub fn composite_model_subtree(&self, id: &str) -> Vec<String> {
        let mut subtree = vec![id.to_string()];
        let mut at = 0;
        while at < subtree.len() {
            for child in self.composite_model_children(&subtree[at]) {
                if !subtree.contains(&child) {
                    subtree.push(child);
                }
            }
            at += 1;
        }
        subtree
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

    /// The hierarchy of this asset a `CustomID` label names: a raw hierarchy id,
    /// or `externalId:<externalId>` resolved against the hierarchy's external id.
    ///
    /// The same two label forms every other id member of this service accepts, so
    /// a hierarchy is addressable the way the model says ids are.
    pub fn hierarchy(&self, label: &str) -> Option<&AssetHierarchy> {
        self.hierarchies.iter().find(|hierarchy| {
            matches_id_label(hierarchy.id.as_deref(), hierarchy.external_id.as_deref(), label)
        })
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

/// Whether a stored `(id, externalId)` pair is the one a `CustomID` label names:
/// the raw id, or `externalId:` followed by the external id.
///
/// ONE definition of the two label forms, so every nested resource this service
/// addresses by label — a hierarchy on an asset, a hierarchy on its model — is
/// resolved the same way.
pub fn matches_id_label(id: Option<&str>, external_id: Option<&str>, label: &str) -> bool {
    match label.strip_prefix(EXTERNAL_ID_PREFIX) {
        Some(external) => external_id == Some(external),
        None => id == Some(label),
    }
}

/// A `(parent asset, hierarchy, child asset)` triple resolved to stored ids —
/// what the association operations address.
#[derive(Debug, Clone)]
pub struct HierarchyTriple {
    /// The parent asset's stored `assetId`.
    pub asset_id: String,
    /// The hierarchy's stored id, taken from the parent asset's hierarchy list.
    pub hierarchy_id: String,
    /// The child asset's stored `assetId`.
    pub child_asset_id: String,
}

/// Why an association triple does not name anything the store can act on.
///
/// Described structurally rather than as one error type, because the two
/// association operations declare their own modelled error sets and each maps
/// these onto its own variants; [`TripleRefusal::message`] is the shared wording.
#[derive(Debug, Clone)]
pub enum TripleRefusal {
    /// No stored asset answers to this label (the parent or the child).
    AssetNotFound(String),
    /// The parent asset carries no hierarchy this label names.
    HierarchyNotFound(String),
}

impl TripleRefusal {
    /// What the caller is told. Every variant here is an absent resource, so the
    /// wording is the service's not-found rule: the resource kind, then the
    /// identifier the request supplied.
    pub fn message(&self) -> String {
        match self {
            Self::AssetNotFound(label) => not_found_message(RESOURCE_ASSET, label),
            // A hierarchy is not a top-level resource, and AWS words its absence
            // as a hierarchy that "doesn't exist"; the identifier is still the
            // one the request supplied.
            Self::HierarchyNotFound(label) => format!("A hierarchy with ID {label} doesn't exist"),
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

/// A stored record the tagging operations reach by ARN: it knows the ARN it was
/// minted with and owns the one tag set every operation that touches its tags
/// reads and writes.
///
/// Implemented once per taggable record, so
/// [`ComAmazonawsIotsitewiseAwsiotsitewiseState::tags_for_resource`] and its
/// `_mut` twin hold only the list of kinds and cannot drift in how a kind is
/// resolved.
pub trait TaggedRecord {
    /// The ARN this record was minted with.
    fn arn(&self) -> &str;
    /// The record's tag set.
    fn tags(&self) -> &HashMap<String, String>;
    /// The same tag set, for writing.
    fn tags_mut(&mut self) -> &mut HashMap<String, String>;
}

/// Implement [`TaggedRecord`] for a record carrying an `arn` and a `tags` field.
macro_rules! tagged_record {
    ($record:ty) => {
        impl TaggedRecord for $record {
            fn arn(&self) -> &str {
                &self.arn
            }

            fn tags(&self) -> &HashMap<String, String> {
                &self.tags
            }

            fn tags_mut(&mut self) -> &mut HashMap<String, String> {
                &mut self.tags
            }
        }
    };
}

tagged_record!(AssetRecord);
tagged_record!(AssetModelRecord);
tagged_record!(GatewayRecord);

/// `record`'s tag set, but only when `record` is the one `arn` names exactly —
/// same partition, account and region, not merely the same id.
fn tags_of<'a, R: TaggedRecord>(
    record: Option<&'a R>,
    arn: &str,
) -> Option<&'a HashMap<String, String>> {
    record
        .filter(|record| record.arn() == arn)
        .map(TaggedRecord::tags)
}

/// The same, for writing. See [`tags_of`].
fn tags_of_mut<'a, R: TaggedRecord>(
    record: Option<&'a mut R>,
    arn: &str,
) -> Option<&'a mut HashMap<String, String>> {
    record
        .filter(|record| record.arn() == arn)
        .map(TaggedRecord::tags_mut)
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
    /// The time series no asset property owns, by the alias that identifies each
    /// one. Disjoint from [`Self::property_alias_index`] by construction: an
    /// alias names an associated stream or a disassociated one, never both — see
    /// [`Self::set_property_alias`] and [`Self::disassociate_time_series`].
    pub disassociated_time_series: HashMap<String, DisassociatedTimeSeriesRecord>,
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
        let property_data_type = property.data_type.clone();
        let property_data_type_spec = property.data_type_spec.clone();
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
            // An alias names ONE stream: binding it to a property makes that
            // stream associated, so a disassociated one standing under the same
            // alias is that same stream — it stops being listed apart from the
            // property that now owns it, and the samples it holds become that
            // property's history again, merged through the quality-aware
            // single-value-per-timestamp write.
            //
            // Only a stream of the property's OWN data type is that property's
            // data: AWS requires the two to match to associate a time series with
            // an asset property, and every write path here checks a TQV's variant
            // against the property's `PropertyDataType`, so samples of another type
            // are never carried into a history that may not hold them.
            // A stream whose data type is not this property's is NOT destroyed
            // here: it stays where it is, samples and all. `UpdateAssetProperty`
            // refuses such a request with the modelled refusal before reaching
            // this helper — the same shape as the alias-conflict case above — so
            // the only way to arrive here is a caller whose own rule allows the
            // rebind, and nothing a caller stored is dropped on its behalf.
            let same_type = self
                .disassociated_time_series
                .get(&alias)
                .is_some_and(|series| {
                    series.data_type == property_data_type
                        && series.data_type_spec == property_data_type_spec
                });
            if same_type {
                if let Some(series) = self.disassociated_time_series.remove(&alias) {
                    for value in series.values {
                        self.record_property_value(asset_id, &resolved_property_id, value);
                    }
                }
            }
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

    /// The `(dataType, dataTypeSpec)` of the disassociated stream `alias` names,
    /// when one stands under it.
    ///
    /// What an operation that binds an alias to an asset property compares with
    /// that property's own data type: a time series is associated with an asset
    /// property only where the two match, so a binding that would mix another
    /// type's samples into the property's history is refused by the operation
    /// rather than resolved by dropping stored data.
    pub fn disassociated_stream_type(&self, alias: &str) -> Option<(&str, Option<&str>)> {
        self.disassociated_time_series.get(alias).map(|series| {
            (
                series.data_type.as_str(),
                series.data_type_spec.as_deref(),
            )
        })
    }

    /// Detach the time series the `(asset_id, property_id)` property's alias
    /// names from that property, and keep the series as a disassociated one.
    ///
    /// The alias leaves the property's own `alias` member and
    /// [`Self::property_alias_index`] through [`Self::set_property_alias`], so
    /// nothing resolves the property by it afterwards, while the stream itself
    /// survives as a [`DisassociatedTimeSeriesRecord`] addressable by that alias —
    /// what IoT SiteWise reports as a `DISASSOCIATED` time series, and what
    /// `DeleteTimeSeries` rather than this call forgets.
    ///
    /// The SAMPLES go with the stream, not with the property: a time series is the
    /// data stream, and an asset property reports the values of the stream it is
    /// associated with, so a property the stream has left reports no history until
    /// a stream is associated with it again. They are not dropped — re-binding the
    /// alias to an asset property brings them back ([`Self::set_property_alias`]).
    /// The detached stream gets its own `TimeSeriesId`, since the property's ids
    /// still name a series of their own.
    ///
    /// `asset_id` is a stored id, not a label: resolve it with
    /// [`Self::resolve_asset_id`] first. `property_id` takes either `CustomID`
    /// form. Answers `false` — writing nothing — when no stored asset holds that
    /// property or the property carries no alias, so there is no association to
    /// break.
    pub fn disassociate_time_series(
        &mut self,
        asset_id: &str,
        property_id: &str,
        now: SystemTime,
    ) -> bool {
        let Some(asset) = self.assets.get(asset_id) else {
            return false;
        };
        let Some(property) = asset.property(property_id) else {
            return false;
        };
        let Some(alias) = property.alias.clone() else {
            return false;
        };
        let resolved_property_id = property.id.clone();
        let record = DisassociatedTimeSeriesRecord {
            alias,
            time_series_id: new_id(),
            data_type: property.data_type.clone(),
            data_type_spec: property.data_type_spec.clone(),
            creation_date: asset.creation_date,
            last_update_date: now,
            values: self
                .property_values
                .get_mut(asset_id)
                .and_then(|properties| properties.remove(&resolved_property_id))
                .unwrap_or_default(),
        };
        self.set_property_alias(asset_id, property_id, None);
        self.keep_disassociated_time_series(record);
        true
    }

    /// Store `record` under its alias, bounded at
    /// [`MAX_DISASSOCIATED_TIME_SERIES`] by dropping the least recently updated
    /// series.
    ///
    /// The eviction exists only because nothing in this emulator forgets a
    /// disassociated stream yet; it goes away when `DeleteTimeSeries` — the
    /// operation AWS gives a caller for that — is implemented.
    fn keep_disassociated_time_series(&mut self, record: DisassociatedTimeSeriesRecord) {
        self.disassociated_time_series
            .insert(record.alias.clone(), record);
        while self.disassociated_time_series.len() > MAX_DISASSOCIATED_TIME_SERIES {
            let Some(oldest) = self
                .disassociated_time_series
                .values()
                .min_by_key(|series| series.last_update_date)
                .map(|series| series.alias.clone())
            else {
                break;
            };
            self.disassociated_time_series.remove(&oldest);
        }
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
    /// The insert itself is [`record_sample`], shared with the write path an
    /// alias-addressed ingest of a disassociated stream takes, so the
    /// quality-aware single-value-per-timestamp rule and the
    /// [`MAX_SAMPLES_PER_SERIES`] bound hold for every stream alike.
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
        record_sample(series, value);
    }

    /// Append `value` to the stream `key` names, answering `false` when no stream
    /// does.
    ///
    /// The one write path for a time series, so an alias-addressed ingest reaches
    /// a disassociated stream on exactly the terms
    /// [`Self::record_property_value`] writes an associated one on — the
    /// quality-aware single-value-per-timestamp rule and the
    /// [`MAX_SAMPLES_PER_SERIES`] bound.
    pub fn record_stream_value(&mut self, key: &SeriesKey, value: AssetPropertyValue) -> bool {
        match key {
            SeriesKey::Property(asset_id, property_id) => {
                let asset_id = asset_id.clone();
                let property_id = property_id.clone();
                self.record_property_value(&asset_id, &property_id, value);
                true
            }
            SeriesKey::Disassociated(alias) => {
                match self.disassociated_time_series.get_mut(alias) {
                    Some(series) => {
                        record_sample(&mut series.values, value);
                        true
                    }
                    None => false,
                }
            }
        }
    }

    /// The stream a `propertyAlias` names: the asset property it is associated
    /// with, or the disassociated series standing under it. `None` when no stream
    /// answers to that alias at all.
    ///
    /// An alias names ONE data stream whether or not an asset property is
    /// associated with it — IoT SiteWise reports a series as `ASSOCIATED` or
    /// `DISASSOCIATED` and `DeleteTimeSeries` identifies a disassociated one "if
    /// the time series isn't associated with an asset property, specify the alias
    /// of the time series" — so every operation addressed by an alias resolves it
    /// here and reaches the same stream.
    pub fn resolve_alias_series(&self, alias: &str) -> Option<SeriesKey> {
        if let Some((asset_id, property_id)) = self.property_alias_index.get(alias) {
            return Some(SeriesKey::Property(asset_id.clone(), property_id.clone()));
        }
        self.disassociated_time_series
            .contains_key(alias)
            .then(|| SeriesKey::Disassociated(alias.to_string()))
    }

    /// The samples of the stream `key` names, oldest first. Empty for a stream
    /// nothing has been written to, and for one `key` names no longer.
    pub fn series(&self, key: &SeriesKey) -> &[AssetPropertyValue] {
        match key {
            SeriesKey::Property(asset_id, property_id) => self
                .property_values
                .get(asset_id)
                .and_then(|properties| properties.get(property_id))
                .map_or(&[][..], Vec::as_slice),
            SeriesKey::Disassociated(alias) => self
                .disassociated_time_series
                .get(alias)
                .map_or(&[][..], |series| series.values.as_slice()),
        }
    }

    /// The highest-timestamp value stored for `(asset_id, property_id)`, if any.
    ///
    /// The property's OWN series, borrowing the two ids the way
    /// [`Self::property_values`] is nested for. A caller that wants the stream an
    /// alias names, associated or not, resolves it with
    /// [`Self::resolve_alias_series`] and reads [`Self::series`].
    pub fn latest_property_value(
        &self,
        asset_id: &str,
        property_id: &str,
    ) -> Option<&AssetPropertyValue> {
        self.property_values.get(asset_id)?.get(property_id)?.last()
    }

    /// The tag set of the resource `arn` names, for reading.
    ///
    /// Resolved through [`parse_resource_arn`] and then by exact ARN equality, so
    /// an ARN that names a stored id in another partition, account or region
    /// resolves to nothing — the resource it names is not one this store holds.
    /// Every record this store holds that carries tags is reachable: the arms
    /// below are the whole list of taggable kinds, and each is one line because
    /// the per-record work lives in [`TaggedRecord`].
    pub fn tags_for_resource(&self, arn: &str) -> Option<&HashMap<String, String>> {
        let (resource_type, id) = parse_resource_arn(arn)?;
        match resource_type {
            ASSET_RESOURCE_TYPE => tags_of(self.assets.get(id), arn),
            ASSET_MODEL_RESOURCE_TYPE => tags_of(self.asset_models.get(id), arn),
            GATEWAY_RESOURCE_TYPE => tags_of(self.gateways.get(id), arn),
            _ => None,
        }
    }

    /// The same tag set, for writing. See [`Self::tags_for_resource`].
    pub fn tags_for_resource_mut(&mut self, arn: &str) -> Option<&mut HashMap<String, String>> {
        let (resource_type, id) = parse_resource_arn(arn)?;
        match resource_type {
            ASSET_RESOURCE_TYPE => tags_of_mut(self.assets.get_mut(id), arn),
            ASSET_MODEL_RESOURCE_TYPE => tags_of_mut(self.asset_models.get_mut(id), arn),
            GATEWAY_RESOURCE_TYPE => tags_of_mut(self.gateways.get_mut(id), arn),
            _ => None,
        }
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

    /// Re-materialise every asset created from asset model `asset_model_id`
    /// against that model's CURRENT property and hierarchy definitions.
    ///
    /// This is the inheritance `UpdateAssetModel` documents: "Updates an asset
    /// model and all of the assets that were created from the model. Each asset
    /// created from the model inherits the updated asset model's property and
    /// hierarchy definitions", and "If you remove a property from an asset model,
    /// IoT SiteWise deletes all previous data for that property".
    ///
    /// So, per asset of that model: a property the model still defines keeps the
    /// asset-level bindings the model does not define (its alias and its
    /// notification state) and takes the definition's name, data type and unit; a
    /// property the model no longer defines goes, and with it its stored time
    /// series and its alias binding; a hierarchy the model no longer defines goes,
    /// and with it the child associations recorded under it. Each affected asset
    /// reports `UPDATING` until the transition this call owes falls due, exactly as
    /// [`AssetRecord::begin_update`] records it for a direct `UpdateAsset`.
    ///
    /// `asset_model_id` is a stored id, not a label: resolve it with
    /// [`Self::resolve_asset_model_id`] first. Does nothing when no stored model
    /// has that id, or when no asset was created from it.
    pub fn propagate_asset_model_definitions(&mut self, asset_model_id: &str, now: SystemTime) {
        let Some(model) = self.asset_models.get(asset_model_id) else {
            return;
        };
        // Cloned so the per-asset writes below are not borrowing the model.
        let definitions = model.properties.clone();
        let hierarchy_definitions = model.hierarchies.clone();
        let asset_ids: Vec<String> = self
            .assets
            .values()
            .filter(|asset| asset.asset_model_id == asset_model_id)
            .map(|asset| asset.id.clone())
            .collect();

        for asset_id in asset_ids {
            let Some(asset) = self.assets.get_mut(&asset_id) else {
                continue;
            };
            let carried = ::std::mem::take(&mut asset.properties);
            // A stored model property without an id would make an unaddressable
            // asset property, so it is skipped rather than materialised with an
            // empty id — the same filter `CreateAsset` applies.
            let properties: Vec<AssetProperty> = definitions
                .iter()
                .filter_map(|definition| {
                    let id = definition.id.clone().filter(|id| !id.is_empty())?;
                    let held = carried.iter().find(|property| property.id == id);
                    Some(AssetProperty {
                        id,
                        external_id: definition.external_id.clone(),
                        name: definition.name.clone(),
                        alias: held.and_then(|property| property.alias.clone()),
                        notification: held.and_then(|property| property.notification.clone()),
                        data_type: definition.data_type.clone(),
                        data_type_spec: definition.data_type_spec.clone(),
                        unit: definition.unit.clone(),
                        path: None,
                    })
                })
                .collect();
            let hierarchies: Vec<AssetHierarchy> = hierarchy_definitions
                .iter()
                .map(|hierarchy| AssetHierarchy {
                    id: hierarchy.id.clone(),
                    external_id: hierarchy.external_id.clone(),
                    name: hierarchy.name.clone(),
                })
                .collect();
            let dropped_hierarchies: Vec<String> = asset
                .hierarchies
                .iter()
                .filter_map(|hierarchy| hierarchy.id.clone())
                .filter(|id| {
                    !hierarchies
                        .iter()
                        .any(|kept| kept.id.as_deref() == Some(id.as_str()))
                })
                .collect();
            let dropped_properties: Vec<AssetProperty> = carried
                .into_iter()
                .filter(|property| {
                    !properties.iter().any(|kept| kept.id == property.id)
                })
                .collect();
            asset.properties = properties;
            asset.hierarchies = hierarchies;
            asset.begin_update(now);

            for property in dropped_properties {
                if let Some(series) = self.property_values.get_mut(&asset_id) {
                    series.remove(&property.id);
                    if series.is_empty() {
                        self.property_values.remove(&asset_id);
                    }
                }
                if let Some(alias) = property.alias {
                    let ours = self
                        .property_alias_index
                        .get(&alias)
                        .is_some_and(|(bound_asset, bound_property)| {
                            *bound_asset == asset_id && *bound_property == property.id
                        });
                    if ours {
                        self.property_alias_index.remove(&alias);
                    }
                }
            }
            for hierarchy_id in dropped_hierarchies {
                self.asset_hierarchy_children
                    .remove(&(asset_id.clone(), hierarchy_id));
            }
        }
    }

    /// The child assets associated with `(asset_id, hierarchy_id)`.
    pub fn hierarchy_children(&self, asset_id: &str, hierarchy_id: &str) -> &[String] {
        self.asset_hierarchy_children
            .get(&(asset_id.to_string(), hierarchy_id.to_string()))
            .map_or(&[], Vec::as_slice)
    }

    /// Resolve a `(parent, hierarchyId, childAssetId)` triple of `CustomID` labels
    /// to the stored ids it names.
    ///
    /// Every label takes either form the model admits (a raw id or
    /// `externalId:<externalId>`). The hierarchy is resolved against the PARENT
    /// ASSET'S MODEL — the same place `AssociateAssets` resolves it, since the
    /// model is what defines which groupings exist — so the operation that records
    /// an association and the one that removes it address exactly one set of
    /// hierarchies, even once a model's definition has changed since the asset was
    /// created. The asset's own copied hierarchy list is the fallback for a parent
    /// whose model is no longer stored.
    pub fn resolve_hierarchy_triple(
        &self,
        asset_label: &str,
        hierarchy_label: &str,
        child_label: &str,
    ) -> Result<HierarchyTriple, TripleRefusal> {
        let asset_id = self
            .resolve_asset_id(asset_label)
            .ok_or_else(|| TripleRefusal::AssetNotFound(asset_label.to_string()))?;
        let asset = self
            .assets
            .get(&asset_id)
            .ok_or_else(|| TripleRefusal::AssetNotFound(asset_label.to_string()))?;
        let hierarchy_id = self
            .asset_models
            .get(&asset.asset_model_id)
            .and_then(|model| {
                model.hierarchies.iter().find(|hierarchy| {
                    matches_id_label(
                        hierarchy.id.as_deref(),
                        hierarchy.external_id.as_deref(),
                        hierarchy_label,
                    )
                })
            })
            .and_then(|hierarchy| hierarchy.id.clone())
            .or_else(|| {
                asset
                    .hierarchy(hierarchy_label)
                    .and_then(|hierarchy| hierarchy.id.clone())
            })
            .ok_or_else(|| TripleRefusal::HierarchyNotFound(hierarchy_label.to_string()))?;
        let child_asset_id = self
            .resolve_asset_id(child_label)
            .ok_or_else(|| TripleRefusal::AssetNotFound(child_label.to_string()))?;
        Ok(HierarchyTriple {
            asset_id,
            hierarchy_id,
            child_asset_id,
        })
    }

    /// Remove `child_id` from `(asset_id, hierarchy_id)`, answering whether an
    /// association was there to remove. `false` means the triple was never
    /// associated, which is the caller's cue to refuse rather than to report a
    /// removal that did not happen.
    ///
    /// The write side of the association set lives in `AssociateAssets`, which
    /// owns the hierarchy rules it has to enforce before recording one; this is
    /// the removal `DisassociateAssets` performs.
    pub fn disassociate_child(
        &mut self,
        asset_id: &str,
        hierarchy_id: &str,
        child_id: &str,
    ) -> bool {
        let key = (asset_id.to_string(), hierarchy_id.to_string());
        let Some(children) = self.asset_hierarchy_children.get_mut(&key) else {
            return false;
        };
        let Some(at) = children.iter().position(|child| child == child_id) else {
            return false;
        };
        children.remove(at);
        // An emptied hierarchy keeps no entry, so the map holds only live
        // associations and cannot grow with keys nothing reads.
        if children.is_empty() {
            self.asset_hierarchy_children.remove(&key);
        }
        true
    }
}

/// The resource kind a `ResourceNotFoundException` / `ResourceAlreadyExistsException`
/// names for an asset, as AWS words it.
pub const RESOURCE_ASSET: &str = "Asset";

/// The resource kind those errors name for an asset model.
pub const RESOURCE_ASSET_MODEL: &str = "Asset Model";

/// The resource kind those errors name for a composite model of an asset model.
///
/// A composite model is not a top-level resource, so it is named as what it is
/// rather than as an "Asset Model"; it lives here beside its siblings so the
/// operations that address one cannot each invent a kind of their own.
pub const RESOURCE_ASSET_MODEL_COMPOSITE_MODEL: &str = "Asset Model Composite Model";

/// The message a not-found refusal carries: the resource kind, then the
/// identifier the REQUEST supplied (a raw id or an `externalId:` label, exactly
/// as sent).
///
/// One rule for every not-found path in this service, so no handler invents its
/// own template: AWS states the absence as `<Resource> with ID <id> not found`,
/// which is why the identifier is echoed rather than replaced by whatever the
/// handler resolved.
pub fn not_found_message(resource: &str, requested: &str) -> String {
    format!("{resource} with ID {requested} not found")
}

/// The message a uniqueness refusal carries: AWS names the DIMENSION of the
/// request that collided (`externalId`, `ID`), not an identifier the rejected
/// request would have minted.
pub fn already_using_message(resource: &str, dimension: &str) -> String {
    format!("Another {resource} is already using the same {dimension}")
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
        check_property_shape(
            &definition.name,
            &definition.data_type,
            definition.data_type_spec.as_deref(),
            &definition.r#type,
        )?;
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

/// Which of the four property types a `PropertyType` sets, named as the shape's
/// own member; `None` when it sets none of them or more than one.
///
/// The ONE place this service decides what "the type of a property" is, so the
/// rule that a definition carries exactly one type and the rule that an existing
/// property's type cannot change ask the same question of the same shape.
pub fn property_type_kind(property_type: &PropertyType) -> Option<&'static str> {
    let kinds = [
        ("attribute", property_type.attribute.is_some()),
        ("measurement", property_type.measurement.is_some()),
        ("transform", property_type.transform.is_some()),
        ("metric", property_type.metric.is_some()),
    ];
    let mut found = None;
    for (name, set) in kinds {
        if set {
            if found.is_some() {
                return None;
            }
            found = Some(name);
        }
    }
    found
}

/// The two rules the property shapes' own documentation states, checked wherever
/// a model property arrives — as a definition on a create, or as a full
/// `AssetModelProperty` on `UpdateAssetModel`:
///
/// * exactly ONE property type ("You can only specify one type in a property
///   definition"), and
/// * `dataTypeSpec` on a `STRUCT` ("This parameter is required on properties that
///   have the STRUCT data type").
///
/// The `Err` carries the refusal message, which the calling operation lifts into
/// its own modelled `InvalidRequestException`.
pub fn check_property_shape(
    name: &str,
    data_type: &str,
    data_type_spec: Option<&str>,
    property_type: &PropertyType,
) -> Result<(), String> {
    if property_type_kind(property_type).is_none() {
        let set = [
            property_type.attribute.is_some(),
            property_type.measurement.is_some(),
            property_type.transform.is_some(),
            property_type.metric.is_some(),
        ]
        .into_iter()
        .filter(|set| *set)
        .count();
        return Err(format!(
            "The property definition {name} must specify exactly one property type \
             (attribute, measurement, transform or metric), but specifies {set}."
        ));
    }
    if data_type == DATA_TYPE_STRUCT && data_type_spec.is_none() {
        return Err(format!(
            "The property definition {name} has the STRUCT data type, so dataTypeSpec is required."
        ));
    }
    Ok(())
}

/// The one `PropertyDataType` value that also requires `dataTypeSpec`; see
/// [`check_property_shape`].
pub const DATA_TYPE_STRUCT: &str = "STRUCT";

/// The `ETag` the `If-Match` / `If-None-Match` preconditions of an asset-model
/// write are evaluated against, as of `now`, or `None` when the version
/// `version_type` names does not exist.
///
/// `matchForVersionType` targets the modelled `AssetModelVersionType` enum
/// (`LATEST | ACTIVE`) and [`VERSION_TYPE_LATEST`] is its documented default.
/// This emulator keeps ONE version of an asset model, so that version is always
/// its latest one and is also its ACTIVE one once the transition the clock owes
/// has fallen due; while one is still pending the model has no active version at
/// all, which is the `None` here. Shared by every operation that carries those
/// headers, so `CreateAssetModelCompositeModel`, `UpdateAssetModel` and
/// `DeleteAssetModel` cannot disagree about which tag a precondition targets.
pub fn asset_model_target_etag(
    record: &AssetModelRecord,
    version_type: Option<&str>,
    now: SystemTime,
) -> Option<String> {
    let version_type = version_type.unwrap_or(VERSION_TYPE_LATEST);
    if version_type == VERSION_TYPE_ACTIVE && record.state(now) != STATE_ACTIVE {
        return None;
    }
    Some(asset_model_etag(record))
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

/// The expression variables of a property's formula: those of its `transform`
/// or of its `metric`.
///
/// An `attribute` or a `measurement` carries no formula and so contributes no
/// variables. ONE definition of what a property's formula depends on, so every
/// rule about formula dependencies asks the same question of the same members.
pub fn formula_variables(
    property: &AssetModelProperty,
) -> impl Iterator<Item = &ExpressionVariable> {
    let transform = property
        .r#type
        .transform
        .iter()
        .flat_map(|transform| transform.variables.iter());
    let metric = property
        .r#type
        .metric
        .iter()
        .flat_map(|metric| metric.variables.iter().flatten());
    transform.chain(metric)
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

/// Which stored data stream a request's identifiers name.
///
/// A time series is either ASSOCIATED with an asset property \u2014 the pair of ids
/// names it, and so does any alias bound to that property \u2014 or DISASSOCIATED,
/// which only its alias names. Resolved once in
/// [`ComAmazonawsIotsitewiseAwsiotsitewiseState::resolve_alias_series`] so every
/// operation addressed by an alias reads and writes the same stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SeriesKey {
    /// The stream of the `(assetId, propertyId)` asset property, by stored ids.
    Property(String, String),
    /// A stream no asset property owns, by the alias that identifies it.
    Disassociated(String),
}

/// Put `value` into `series`, which stays sorted ascending by timestamp.
///
/// A value at an already-stored timestamp is QUALITY-AWARE: "For each asset
/// property, IoT SiteWise overwrites TQVs with duplicate timestamps unless the
/// newer TQV has a different quality" (the `BatchPutAssetPropertyValue` model
/// doc), so the stored sample is replaced only when the two name the same
/// quality, and a differing quality leaves the series exactly as it was. Either
/// way the series stays single-valued at that timestamp.
///
/// The series is BOUNDED at [`MAX_SAMPLES_PER_SERIES`]: past that many samples the
/// oldest goes, so no sequence of writes can grow one series without limit. A
/// sample newer than everything stored \u2014 the shape real ingest takes \u2014 takes an
/// append-only fast path and costs no element move, so the sorted insert cannot be
/// driven to quadratic cost under the shared state lock.
fn record_sample(series: &mut Vec<AssetPropertyValue>, value: AssetPropertyValue) {
    let key = timestamp_key(&value.timestamp);
    if series
        .last()
        .is_none_or(|last| timestamp_key(&last.timestamp) < key)
    {
        series.push(value);
    } else {
        match series.binary_search_by(|existing| timestamp_key(&existing.timestamp).cmp(&key)) {
            Ok(at) => {
                if quality_of(&series[at]) != quality_of(&value) {
                    // Accepted and discarded: the stored TQV wins at a timestamp a
                    // differing-quality TQV re-puts.
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

/// The resource type and resource id an `AmazonResourceName` names, when the
/// string is an ARN of THIS service in the `<type>/<id>` resource form every
/// IoT SiteWise ARN takes (`asset/<assetId>`, `asset-model/<assetModelId>`,
/// `portal/<portalId>`, …).
///
/// `None` for anything that is not such an ARN — a string that is not an ARN at
/// all, or one whose service segment is another service's. The resource type is
/// returned as it was written rather than matched here, so a caller decides
/// which types it holds.
pub fn parse_resource_arn(arn: &str) -> Option<(&str, &str)> {
    // `arn:<partition>:<service>:<region>:<account>:<resource>` — the resource
    // section is the only one that may itself contain a `:`, so it takes the
    // remainder.
    let mut fields = arn.splitn(6, ':');
    if fields.next()? != "arn" {
        return None;
    }
    let _partition = fields.next()?;
    if fields.next()? != ARN_SERVICE {
        return None;
    }
    let _region = fields.next()?;
    let _account = fields.next()?;
    let (resource_type, id) = fields.next()?.split_once('/')?;
    if resource_type.is_empty() || id.is_empty() {
        return None;
    }
    Some((resource_type, id))
}

/// Refuse a tag key outside the modelled `TagKey` bounds, in the wording the
/// boundary validator uses for a map's keys.
///
/// The framework's constraint walk reaches a map's VALUES but not its KEYS, so
/// `TagMap`'s `@length(min: 1, max: 128)` on `TagKey` is the handler's to
/// enforce; the length is in characters, as `@length` counts them.
pub fn check_tag_key(key: &str) -> Result<(), String> {
    let length = key.chars().count();
    if (TAG_KEY_MIN_LENGTH..=TAG_KEY_MAX_LENGTH).contains(&length) {
        return Ok(());
    }
    Err(format!(
        "1 validation error detected: Value at 'tags' failed to satisfy constraint: \
         Map keys must satisfy constraint: [Member must have length greater than or equal to \
         {TAG_KEY_MIN_LENGTH}, Member must have length less than or equal to {TAG_KEY_MAX_LENGTH}]"
    ))
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

// ---------------------------------------------------------------------------
// Deriving one series' aggregates.
//
// HOISTED HERE from `src/ops/batch_get_asset_property_aggregates.rs`, which asked
// for exactly that once a second caller appeared: "the fixer that implements
// [`GetAssetPropertyAggregates`] should hoist this block into `crate::state` and
// call it from both". `GetAssetPropertyAggregates` and
// `BatchGetAssetPropertyAggregates` now read a series through this one engine, so
// the two cannot disagree about what a window covers or what it reduces to.
// ---------------------------------------------------------------------------

/// Every `Resolution` an aggregate query may name, with the length in seconds of
/// the window it names.
///
/// Source: the `Resolution` shape — `smithy_query(query="shape",
/// shape="Resolution")` reports length 2..3 and pattern `^1m|15m|1h|1d$`, i.e.
/// the four names below ("1m" one minute, "15m" fifteen minutes, "1h" one hour,
/// "1d" one day). The window LENGTHS are what the generated `Resolution` string
/// does not carry, which is why this table exists at all.
pub const RESOLUTIONS: [(&str, i64); 4] =
    [("1m", 60), ("15m", 900), ("1h", 3_600), ("1d", 86_400)];

/// The window length, in seconds, that `resolution` names — `None` when it is not
/// one of [`RESOLUTIONS`].
pub fn resolution_seconds(resolution: &str) -> Option<i64> {
    RESOLUTIONS
        .iter()
        .find(|(name, _)| *name == resolution)
        .map(|(_, seconds)| *seconds)
}

/// A `TimeInNanos` as whole nanoseconds since the Unix epoch, so a sample and a
/// modelled `Timestamp` bound compare at the precision the sample carries.
fn timestamp_nanos(timestamp: &TimeInNanos) -> i128 {
    i128::from(timestamp.time_in_seconds) * 1_000_000_000
        + i128::from(timestamp.offset_in_nanos.unwrap_or(0))
}

/// A modelled `Timestamp` as whole nanoseconds since the Unix epoch; see
/// [`timestamp_nanos`].
pub fn date_time_nanos(at: &::aws_smithy_types::DateTime) -> i128 {
    i128::from(at.secs()) * 1_000_000_000 + i128::from(at.subsec_nanos())
}

/// What one aggregate query asks of one stored series: the window grid to bucket
/// its samples into, the range to read, the quality to count and which aggregates
/// to compute.
///
/// Built only by [`AggregateQuery::new`], so `window_seconds` can only ever be a
/// length [`resolution_seconds`] returned — the invariant the bucketing arithmetic
/// relies on is therefore unforgeable rather than merely documented.
#[derive(Debug, Clone, Copy)]
pub struct AggregateQuery<'a> {
    /// The window length in seconds. Windows are aligned to the Unix epoch, so
    /// one window covers the same instants whoever asks. Always positive.
    window_seconds: i64,
    /// The EXCLUSIVE start of the queried range, in nanoseconds of Unix epoch
    /// time: "The exclusive start of the range from which to query historical
    /// data" (the `startDate` member of either aggregate operation).
    start_exclusive_nanos: i128,
    /// The INCLUSIVE end of the queried range: "The inclusive end of the range
    /// from which to query historical data" (the `endDate` member).
    end_inclusive_nanos: i128,
    /// The one `Quality` a sample must carry to be counted; an aggregate reports
    /// exactly one quality, and [`DEFAULT_QUALITY`] is what a request naming none
    /// asks for.
    quality: &'a str,
    /// The `AggregateType`s to compute. Every other member of `Aggregates` is
    /// left unset, because the caller did not ask for it.
    aggregate_types: &'a [String],
    /// `true` for `TimeOrdering` DESCENDING; ASCENDING is the modelled default.
    descending: bool,
}

impl<'a> AggregateQuery<'a> {
    /// The query a request's `resolution` names, or `None` when `resolution` is
    /// not one of [`RESOLUTIONS`] — which is the caller's cue to refuse the
    /// request with the modelled `InvalidRequestException` rather than answer it.
    pub fn new(
        resolution: &str,
        start_exclusive_nanos: i128,
        end_inclusive_nanos: i128,
        quality: &'a str,
        aggregate_types: &'a [String],
        descending: bool,
    ) -> Option<Self> {
        Some(Self {
            window_seconds: resolution_seconds(resolution)?,
            start_exclusive_nanos,
            end_inclusive_nanos,
            quality,
            aggregate_types,
            descending,
        })
    }
}

/// The aggregated values of the stream `stream` names under `query` — what either
/// aggregate operation reports for one data stream, associated with an asset
/// property or not.
///
/// DERIVED, never stored: the samples `BatchPutAssetPropertyValue` wrote are
/// bucketed into the epoch-aligned windows `query` names and each bucket is
/// reduced to the aggregates the caller asked for. A window that no in-range
/// sample of the asked-for quality falls in is not reported at all, so the length
/// of the answer is a function of the stored data and not of the width of the
/// queried range.
///
/// The pass is over the SAMPLES, not over the windows the range spans: a series is
/// bounded at [`MAX_SAMPLES_PER_SERIES`], so the work one query can ask for is
/// bounded however wide a range it names.
pub fn aggregated_values(
    store: &ComAmazonawsIotsitewiseAwsiotsitewiseState,
    stream: &SeriesKey,
    query: &AggregateQuery<'_>,
) -> Vec<AggregatedValue> {
    let series = store.series(stream);
    if series.is_empty() {
        return Vec::new();
    }

    // The series is kept sorted ascending, so one window's samples are contiguous
    // and a single `last_mut()` bucket is all the grouping needs.
    let mut windows: Vec<(i64, Bucket)> = Vec::new();
    for sample in series {
        let at = timestamp_nanos(&sample.timestamp);
        if at <= query.start_exclusive_nanos || at > query.end_inclusive_nanos {
            continue;
        }
        if quality_of(sample) != query.quality {
            continue;
        }
        // A `nullValue` reports a reading that is absent, so it is not a data
        // point of the window it falls in.
        if sample.value.null_value.is_some() {
            continue;
        }
        let seconds = sample.timestamp.time_in_seconds;
        // `window_seconds` is positive by construction (see
        // [`AggregateQuery::new`]), so this is total.
        let start = seconds.saturating_sub(seconds.rem_euclid(query.window_seconds));
        if windows.last().map(|(at, _)| *at) != Some(start) {
            windows.push((start, Bucket::default()));
        }
        if let Some((_, bucket)) = windows.last_mut() {
            bucket.count = bucket.count.saturating_add(1);
            if let Some(numeric) = numeric_value(&sample.value) {
                bucket.numeric.push(numeric);
            }
        }
    }

    let mut values: Vec<AggregatedValue> = windows
        .iter()
        .filter_map(|(start, bucket)| {
            let value = bucket.reduce(query.aggregate_types);
            reports_an_aggregate(&value).then(|| AggregatedValue {
                // A window is reported at the instant it starts.
                timestamp: ::aws_smithy_types::DateTime::from_secs(*start),
                quality: Some(query.quality.to_string()),
                value,
            })
        })
        .collect();
    // ASCENDING is the modelled default; DESCENDING is the same windows read the
    // other way.
    if query.descending {
        values.reverse();
    }
    values
}

/// One window's contributing samples, accumulated in a single pass over the
/// series.
#[derive(Debug, Default)]
struct Bucket {
    /// How many data points fell in the window, whatever their type: `count` is
    /// the count of the data points the window's aggregates were computed from.
    count: u64,
    /// The numeric value of each sample that has one. A `STRING` or `BOOLEAN`
    /// sample has none, so it is counted but cannot be summed or averaged.
    numeric: Vec<f64>,
}

impl Bucket {
    /// This window reduced to the requested aggregates. An aggregate the caller
    /// did not ask for is left unset, and so is a numeric aggregate that no sample
    /// in the window can feed.
    ///
    /// The arms name the `AggregateType` members the model declares, generated as
    /// `AGGREGATE_TYPE_VALUES` in `crate::ops::types` — which is what each
    /// operation checks a request against before a series is read.
    fn reduce(&self, aggregate_types: &[String]) -> Aggregates {
        let samples = self.numeric.len();
        let sum: f64 = self.numeric.iter().sum();
        let mean = (samples > 0).then(|| sum / samples as f64);
        let mut aggregates = Aggregates::default();
        for aggregate_type in aggregate_types {
            match aggregate_type.as_str() {
                "COUNT" => aggregates.count = Some(self.count as f64),
                "SUM" => aggregates.sum = (samples > 0).then_some(sum),
                "AVERAGE" => aggregates.average = mean,
                "MAXIMUM" => aggregates.maximum = self.numeric.iter().copied().reduce(f64::max),
                "MINIMUM" => aggregates.minimum = self.numeric.iter().copied().reduce(f64::min),
                // The POPULATION standard deviation: a window's data points are
                // all of that window's data, so nothing is being sampled from a
                // wider set and there is no degree of freedom to lose.
                "STANDARD_DEVIATION" => {
                    aggregates.standard_deviation = mean.map(|mean| {
                        (self
                            .numeric
                            .iter()
                            .map(|value| (value - mean) * (value - mean))
                            .sum::<f64>()
                            / samples as f64)
                            .sqrt()
                    });
                }
                // Unreachable: each operation checks a request's aggregate types
                // against the generated catalogue before the series is read.
                _ => {}
            }
        }
        aggregates
    }
}

/// Whether `aggregates` reports any value at all. A window that can feed none of
/// the requested aggregates is not reported, rather than reported as an empty
/// `Aggregates`.
fn reports_an_aggregate(aggregates: &Aggregates) -> bool {
    aggregates.average.is_some()
        || aggregates.count.is_some()
        || aggregates.maximum.is_some()
        || aggregates.minimum.is_some()
        || aggregates.sum.is_some()
        || aggregates.standard_deviation.is_some()
}

/// The numeric value a `Variant` holds, or `None` when it holds none. A `Variant`
/// sets exactly one member, and only `integerValue` and `doubleValue` are numbers.
fn numeric_value(value: &Variant) -> Option<f64> {
    value
        .double_value
        .or_else(|| value.integer_value.map(f64::from))
}

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
