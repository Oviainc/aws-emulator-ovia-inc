// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-iotfleetwise-iotautobahncontrolplane service.
//!
//! One store per service, reached by every handler through [`service_state`]. The three
//! resources this crate persists so far — signal catalogs, model manifests (vehicle
//! models) and vehicles — live in `BTreeMap`s keyed by their names, so enumeration is
//! stable and a pagination token can name the key a page resumes at rather than an
//! offset into a map whose iteration order could change.

use std::collections::{BTreeMap, BTreeSet};

use crate::ops::types::{InternalServerException, Node, NodeCounts, StateTemplateAssociation, Tag};

/// The ARN namespace (and SigV4 signing name) this service's own resources carry.
/// `OPERATIONS.md` "Wire protocol" records both as `iotfleetwise`.
pub const ARN_SERVICE: &str = "iotfleetwise";

/// The ARN namespace an IoT thing carries. `CreateVehicle` reports the thing it
/// associates a vehicle with as an `iot` ARN, not an `iotfleetwise` one.
pub const IOT_ARN_SERVICE: &str = "iot";

/// The region used when a request carries no SigV4 credential scope to read one from
/// (an anonymous or host-routed request). Every generated emulator defaults here.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The ARN resource-type segment of a signal catalog.
pub const SIGNAL_CATALOG_RESOURCE_TYPE: &str = "signal-catalog";
/// The ARN resource-type segment of a model manifest (vehicle model).
pub const MODEL_MANIFEST_RESOURCE_TYPE: &str = "model-manifest";
/// The ARN resource-type segment of a vehicle.
pub const VEHICLE_RESOURCE_TYPE: &str = "vehicle";

// ---------------------------------------------------------------------------
// What bounds each store's growth
//
// Every record below is created by a request and lives until the caller deletes it, so
// each store needs a ceiling or an unattended client loop grows it without limit. AWS
// meters all three resources per account per Region and every create operation models
// `LimitExceededException` ("A service quota was exceeded") for exactly that case, which
// is what these handlers answer with on the ceiling. The real quotas are adjustable
// through Service Quotas, so the NUMBERS here are this emulator's own ceilings rather
// than values read off a doc page: each sits far above any realistic client workflow, so
// a caller never meets one by accident.
// ---------------------------------------------------------------------------

/// The most signal catalogs this emulator holds.
pub const MAX_SIGNAL_CATALOGS: usize = 1_000;

/// The most model manifests (vehicle models) this emulator holds.
pub const MAX_MODEL_MANIFESTS: usize = 10_000;

/// The most vehicles this emulator holds.
pub const MAX_VEHICLES: usize = 100_000;

/// The most `attributes` entries one vehicle record holds. `attributesMap` carries no
/// modelled `@length`, so without this one request could store an arbitrarily large map;
/// its KEYS are modelled at 1..150 characters, and [`MAX_ATTRIBUTE_VALUE_LENGTH`] bounds
/// the values, so this count is what bounds one record's size.
pub const MAX_VEHICLE_ATTRIBUTES: usize = 500;

/// The longest `attributes` value one vehicle record holds, in characters. The modelled
/// `attributeValue` declares no length, and an attribute is static information about a
/// vehicle (`"engineType": "1.3 L R2"`), so this emulator's ceiling is generous.
pub const MAX_ATTRIBUTE_VALUE_LENGTH: usize = 4_096;

/// The most nodes one model manifest selects. This is the model's own bound —
/// `CreateModelManifestRequest$nodes` carries `@length(max: 500)` — restated here so the
/// membership scan the create performs is explicitly capped in the handler too.
pub const MAX_MODEL_MANIFEST_NODES: usize = 500;

/// The modelled server error a handler answers with when the engine it was constructed
/// from is not the one that holds this service's store. One constructor so the wording
/// and the retry hint have a single home.
///
/// `retryAfterSeconds` is 0: there is no delay after which the answer would differ.
pub fn state_unavailable() -> InternalServerException {
    InternalServerException {
        message: "the IoTFleetWise state store is unavailable".to_string(),
        retry_after_seconds: 0,
    }
}

/// A signal catalog as `CreateSignalCatalog` stored it.
#[derive(Debug, Clone)]
pub struct SignalCatalogRecord {
    pub name: String,
    pub arn: String,
    pub description: Option<String>,
    pub nodes: Vec<Node>,
    pub tags: Vec<Tag>,
    pub creation_time: ::aws_smithy_types::DateTime,
    pub last_modification_time: ::aws_smithy_types::DateTime,
}

impl SignalCatalogRecord {
    /// The fully qualified name of one node, whichever member of the `Node` union it is.
    /// Every variant of `com.amazonaws.iotfleetwise#Node` carries a
    /// `fullyQualifiedName`, which is how a model manifest names the signals it selects.
    pub fn node_fully_qualified_name(node: &Node) -> &str {
        match node {
            Node::Branch(branch) => branch.fully_qualified_name.as_str(),
            Node::Sensor(sensor) => sensor.fully_qualified_name.as_str(),
            Node::Actuator(actuator) => actuator.fully_qualified_name.as_str(),
            Node::Attribute(attribute) => attribute.fully_qualified_name.as_str(),
            Node::Struct(custom_struct) => custom_struct.fully_qualified_name.as_str(),
            Node::Property(property) => property.fully_qualified_name.as_str(),
        }
    }

    /// The fully qualified names this catalog declares, as a set, so checking a list of
    /// requested nodes against it costs one lookup per requested node rather than a scan
    /// of the whole node vector.
    pub fn node_names(&self) -> BTreeSet<&str> {
        self.nodes
            .iter()
            .map(Self::node_fully_qualified_name)
            .collect()
    }

    /// The per-type node tally `GetSignalCatalog` reports, DERIVED from the stored
    /// nodes rather than stored beside them, so it can never disagree with them.
    pub fn node_counts(&self) -> NodeCounts {
        let mut counts = NodeCounts::default();
        for node in &self.nodes {
            counts.total_nodes += 1;
            match node {
                Node::Branch(_) => counts.total_branches += 1,
                Node::Sensor(_) => counts.total_sensors += 1,
                Node::Actuator(_) => counts.total_actuators += 1,
                Node::Attribute(_) => counts.total_attributes += 1,
                Node::Struct(_) => counts.total_structs += 1,
                Node::Property(_) => counts.total_properties += 1,
            }
        }
        counts
    }
}

/// A model manifest (vehicle model) as `CreateModelManifest` stored it.
#[derive(Debug, Clone)]
pub struct ModelManifestRecord {
    pub name: String,
    pub arn: String,
    pub description: Option<String>,
    pub signal_catalog_arn: String,
    pub nodes: Vec<String>,
    /// A `ManifestStatus` value; a freshly created manifest is `DRAFT`.
    pub status: String,
    pub tags: Vec<Tag>,
    pub creation_time: ::aws_smithy_types::DateTime,
    pub last_modification_time: ::aws_smithy_types::DateTime,
}

/// A vehicle as `CreateVehicle` stored it.
#[derive(Debug, Clone)]
pub struct VehicleRecord {
    pub vehicle_name: String,
    pub arn: String,
    pub thing_arn: String,
    pub model_manifest_arn: String,
    pub decoder_manifest_arn: String,
    /// `None` when the create request carried no `attributes` member at all, which is
    /// a different answer from an empty map and is reported back as such.
    pub attributes: Option<BTreeMap<String, String>>,
    pub state_templates: Option<Vec<StateTemplateAssociation>>,
    /// A `VehicleAssociationBehavior` value when the request chose one.
    pub association_behavior: Option<String>,
    pub tags: Vec<Tag>,
    pub creation_time: ::aws_smithy_types::DateTime,
    pub last_modification_time: ::aws_smithy_types::DateTime,
}

/// Per-service state for com-amazonaws-iotfleetwise-iotautobahncontrolplane. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsIotfleetwiseIotautobahncontrolplaneState {
    /// Signal catalogs by `name`.
    pub signal_catalogs: BTreeMap<String, SignalCatalogRecord>,
    /// Model manifests by `name`.
    pub model_manifests: BTreeMap<String, ModelManifestRecord>,
    /// Vehicles by `vehicleName`.
    pub vehicles: BTreeMap<String, VehicleRecord>,
}

impl ComAmazonawsIotfleetwiseIotautobahncontrolplaneState {
    /// The signal catalog an ARN names, for the existence check `CreateModelManifest`
    /// makes on `signalCatalogArn`.
    pub fn signal_catalog_by_arn(&self, arn: &str) -> Option<&SignalCatalogRecord> {
        self.signal_catalogs
            .values()
            .find(|catalog| catalog.arn == arn)
    }

    /// The model manifest an ARN names, for the existence check `CreateVehicle` makes
    /// on `modelManifestArn`.
    pub fn model_manifest_by_arn(&self, arn: &str) -> Option<&ModelManifestRecord> {
        self.model_manifests
            .values()
            .find(|manifest| manifest.arn == arn)
    }
}

/// Build the ARN of one of this service's own resources:
/// `arn:aws:iotfleetwise:<region>:<account>:<type>/<name>`.
///
/// The account segment comes from the single canonical source
/// ([`engine::emulator_arn`]), never a literal.
pub fn resource_arn(region: &str, resource_type: &str, name: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("{resource_type}/{name}"))
}

/// The ARN of the IoT thing a vehicle is associated with:
/// `arn:aws:iot:<region>:<account>:thing/<name>`.
pub fn thing_arn(region: &str, thing_name: &str) -> String {
    ::engine::emulator_arn(IOT_ARN_SERVICE, region, &format!("thing/{thing_name}"))
}

/// The region this request was signed for, which is the region the ARNs it mints
/// carry. Falls back to [`DEFAULT_REGION`] for a request with no SigV4 scope.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// What one `nextToken` carries: the key the next page resumes at, and a digest of the
/// filter set the page it continues was produced under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageCursor {
    /// [`filter_digest`] of the filters the previous page applied.
    pub filter_digest: String,
    /// The store key the next page starts at, inclusive.
    pub resume_key: String,
}

/// A short digest of a list operation's canonical filter rendering, so a token can be
/// recognised as belonging to one filter set.
///
/// Eight bytes of SHA-256, hex-encoded: this distinguishes one filter set from another,
/// it is not a security boundary, and keeping it short is what keeps a minted token
/// inside the modelled `nextToken` length however long the filter values were.
pub fn filter_digest(canonical_filter: &str) -> String {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(canonical_filter.as_bytes());
    hex::encode(&digest[..8])
}

/// Encode a [`PageCursor`] into an opaque `nextToken`.
///
/// Hex rather than the bare key so the token is not a documented, hand-editable
/// resource name: a client must hand back what the previous page returned.
pub fn encode_page_token(cursor: &PageCursor) -> String {
    let payload = ::serde_json::json!({"f": cursor.filter_digest, "k": cursor.resume_key});
    hex::encode(payload.to_string())
}

/// The longest `nextToken` this service decodes. The modelled `nextToken` is
/// `@length(1..4096)` and the boundary validator enforces it; restating the bound in the
/// decoder keeps it local to the one place a caller-supplied token is parsed.
pub const MAX_PAGE_TOKEN_LENGTH: usize = 4_096;

/// The cursor a `nextToken` carries, or `None` when the token was not one this service
/// minted (the caller gets a `ValidationException`).
pub fn decode_page_token(token: &str) -> Option<PageCursor> {
    if token.len() > MAX_PAGE_TOKEN_LENGTH {
        return None;
    }
    let payload = String::from_utf8(hex::decode(token).ok()?).ok()?;
    let value: ::serde_json::Value = ::serde_json::from_str(&payload).ok()?;
    Some(PageCursor {
        filter_digest: value.get("f")?.as_str()?.to_string(),
        resume_key: value.get("k")?.as_str()?.to_string(),
    })
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsIotfleetwiseIotautobahncontrolplaneState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsIotfleetwiseIotautobahncontrolplaneState>)
}
