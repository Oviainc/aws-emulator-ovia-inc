// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-clouddirectory-amazonclouddirectory-20170111 service.
//!
//! Cloud Directory is a three-stage store: a schema is created in the DEVELOPMENT
//! stage, frozen into a PUBLISHED copy, and an APPLIED copy of a published schema is
//! attached to each directory. Directory data is a graph of objects joined by named
//! parent -> child links, addressed by the documented `ObjectReference.Selector`
//! forms. Everything the handlers answer with is computed from the records below.

use std::collections::{BTreeMap, HashMap};

use crate::ops::types::{FacetAttribute, SchemaFacet, TypedAttributeValue};

/// The ARN namespace and SigV4 signing name of this service (`awsService.arnNamespace`
/// in the Smithy model), used as the service segment of every ARN this service mints.
pub const ARN_NAMESPACE: &str = "clouddirectory";

/// The Region an ARN is minted in when the request carries no SigV4 credential scope
/// to read one from (an unsigned request routed by endpoint host).
pub const FALLBACK_REGION: &str = "us-east-1";

// ---------------------------------------------------------------------------
// Store and per-request bounds
//
// Every map below lives for the life of the process and every request may add to
// it, so each growth site is capped — and so is the per-request work whose size a
// caller chooses, including the PRODUCT of two caller-chosen counts. Cloud
// Directory publishes adjustable account quotas and this emulator models neither
// Service Quotas nor an account's adjustments, so these are the emulator's OWN
// bounds rather than a claim about any account's limit. Each store bound covers the
// WHOLE map rather than one Region's share of it, because the Region comes from the
// request: a per-Region bound would still let a caller grow the store without end
// by varying the Region it signs for. Each sits far above any workflow an emulator
// serves, and the release paths are real — `DeleteDirectory` frees a directory's
// objects and its applied schemas through [`DirectoryRecord::release`], and the
// oldest tombstone is evicted by
// [`ComAmazonawsClouddirectoryAmazonclouddirectory20170111State::prune_directory_tombstones`].
// ---------------------------------------------------------------------------

/// How many schema records (development, published and applied together) the store
/// holds before `CreateSchema`, `PublishSchema` and `CreateDirectory` refuse to add
/// another with the modelled `LimitExceededException`.
pub const SCHEMA_QUOTA: usize = 1_000;

/// How many facets one schema holds before `CreateFacet` refuses with the modelled
/// `LimitExceededException`.
pub const FACETS_PER_SCHEMA_QUOTA: usize = 1_000;

/// How many attributes one facet defines before `CreateFacet` refuses with the
/// modelled `LimitExceededException`.
///
/// This is the second factor of the per-request work `CreateObject` does over a
/// caller-chosen facet list, so it is capped here where the facet is stored, not only
/// where it is read.
pub const ATTRIBUTES_PER_FACET_QUOTA: usize = 1_000;

/// How many LIVE (enabled or disabled) directories the store holds before
/// `CreateDirectory` refuses with the modelled `LimitExceededException`. A deleted
/// directory does not count against it: deleting one frees capacity immediately.
pub const DIRECTORY_QUOTA: usize = 100;

/// How many deleted-directory tombstones the store keeps, oldest evicted first.
///
/// A tombstone holds only the four members `GetDirectory` projects, and keeping it is
/// what lets a caller see `DELETED` rather than "not found" after deleting. Keeping
/// them for ever would still be unbounded growth, so the store retains this many and
/// evicts the oldest beyond it.
pub const DIRECTORY_TOMBSTONE_QUOTA: usize = 100;

/// How many objects one directory holds before `CreateObject` refuses with the
/// modelled `LimitExceededException`. The directory root counts towards it.
pub const OBJECTS_PER_DIRECTORY_QUOTA: usize = 10_000;

/// How many distinct facets one object carries before `CreateObject` refuses with the
/// modelled `LimitExceededException`. Bounds the first factor of its per-request work.
pub const SCHEMA_FACETS_PER_OBJECT_QUOTA: usize = 64;

/// How many attribute values one `CreateObject` request carries before it is refused
/// with the modelled `LimitExceededException`.
pub const ATTRIBUTES_PER_OBJECT_QUOTA: usize = 1_000;

/// The phase a schema is in. From `CreateSchema`'s own documentation: a schema is
/// created in the mutable *development* phase, `PublishSchema` freezes an immutable
/// *published* copy, and `CreateDirectory` attaches an *applied* copy to a directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaStage {
    Development,
    Published,
    Applied,
}

/// One facet of a schema: its object type, its style and its attribute definitions.
#[derive(Debug, Clone)]
pub struct FacetRecord {
    pub name: String,
    pub object_type: Option<String>,
    pub facet_style: Option<String>,
    pub attributes: Vec<FacetAttribute>,
}

impl FacetRecord {
    /// The attribute this facet defines under `name`, if any.
    pub fn attribute(&self, name: &str) -> Option<&FacetAttribute> {
        self.attributes.iter().find(|attr| attr.name == name)
    }
}

/// A schema in one stage, holding its OWN copy of the facets at that stage, so an
/// edit to one stage never reaches another.
///
/// Which copies accept an edit is `CreateSchema`'s own documentation: "Published:
/// Published schemas are immutable and have a version associated with them. Applied:
/// Applied schemas are mutable in a way that allows you to add new schema facets."
#[derive(Debug, Clone)]
pub struct SchemaRecord {
    pub arn: String,
    pub name: String,
    pub stage: SchemaStage,
    pub version: Option<String>,
    pub minor_version: Option<String>,
    pub facets: HashMap<String, FacetRecord>,
}

/// The lifecycle state of a directory, as the model's `DirectoryState` enum names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectoryLifecycle {
    Enabled,
    Disabled,
    Deleted,
}

impl DirectoryLifecycle {
    /// The wire value, one of `DIRECTORY_STATE_VALUES`.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Enabled => "ENABLED",
            Self::Disabled => "DISABLED",
            Self::Deleted => "DELETED",
        }
    }
}

/// One object in a directory's graph.
///
/// `children` and `parents` are `BTreeMap`s so child listing order — and therefore
/// the offsets a pagination token names — is stable across calls.
#[derive(Debug, Clone, Default)]
pub struct ObjectRecord {
    pub object_identifier: String,
    pub schema_facets: Vec<SchemaFacet>,
    /// `(schema arn, facet name, attribute name) -> value`.
    pub attributes: BTreeMap<(String, String, String), TypedAttributeValue>,
    /// `link name -> child object identifier`.
    pub children: BTreeMap<String, String>,
    /// `parent object identifier -> link name`.
    pub parents: BTreeMap<String, String>,
}

/// A directory: its applied schemas, its implicit root object and its object graph.
#[derive(Debug, Clone)]
pub struct DirectoryRecord {
    pub arn: String,
    pub name: String,
    pub state: DirectoryLifecycle,
    pub creation_date_time: ::aws_smithy_types::DateTime,
    pub applied_schema_arns: Vec<String>,
    pub root_object_id: String,
    pub objects: HashMap<String, ObjectRecord>,
}

impl DirectoryRecord {
    /// Project the `Directory` wire shape from this record.
    pub fn to_wire(&self) -> crate::ops::types::Directory {
        crate::ops::types::Directory {
            name: Some(self.name.clone()),
            directory_arn: Some(self.arn.clone()),
            state: Some(self.state.as_wire().to_string()),
            creation_date_time: Some(self.creation_date_time),
        }
    }

    /// Free everything a deleted directory no longer serves, and report the applied
    /// schema ARNs the caller must drop from the schema registry.
    ///
    /// A deleted directory keeps only the four members `GetDirectory` projects — name,
    /// ARN, `DELETED` state and creation time. Its object graph and the per-directory
    /// copies of its applied schemas are unreachable once it is deleted (every data
    /// operation answers `DirectoryNotEnabledException` and the applied ARN can no
    /// longer be used to create an object), so they are released here rather than
    /// pinned for the life of the process.
    pub fn release(&mut self) -> Vec<String> {
        self.objects = HashMap::new();
        self.root_object_id = String::new();
        std::mem::take(&mut self.applied_schema_arns)
    }

    /// Resolve one of the documented `ObjectReference.Selector` forms to an object
    /// identifier in this directory.
    ///
    /// The three forms the model documents are `$<ObjectIdentifier>` (optionally
    /// wrapped in double quotes), `/some/path` built from link names starting at the
    /// directory root, and `#SomeBatchReference`, which only names an object inside a
    /// batch call and so is malformed here.
    pub fn resolve_selector(&self, selector: &str) -> Result<String, SelectorError> {
        let selector = selector.trim();
        if let Some(identifier) = selector.strip_prefix('$') {
            // The docs wrap an ObjectIdentifier selector in double quotes; exactly one
            // matched pair is stripped, so a multiply-quoted selector is not accepted.
            let identifier = identifier
                .strip_prefix('"')
                .and_then(|rest| rest.strip_suffix('"'))
                .unwrap_or(identifier);
            if self.objects.contains_key(identifier) {
                return Ok(identifier.to_string());
            }
            return Err(SelectorError::NotFound(format!(
                "no object with identifier {identifier} exists in directory {}",
                self.arn
            )));
        }
        if let Some(path) = selector.strip_prefix('/') {
            let mut current = self.root_object_id.clone();
            for segment in path.split('/').filter(|s| !s.is_empty()) {
                let object = self
                    .objects
                    .get(&current)
                    .ok_or_else(|| SelectorError::NotFound(format!("{selector} does not resolve")))?;
                current = object.children.get(segment).cloned().ok_or_else(|| {
                    SelectorError::NotFound(format!(
                        "no child is linked as {segment} on the path {selector}"
                    ))
                })?;
            }
            return Ok(current);
        }
        if selector.starts_with('#') {
            return Err(SelectorError::Malformed(
                "ObjectReference.Selector: a #BatchReference selector only names an object \
                 inside a BatchRead or BatchWrite call"
                    .to_string(),
            ));
        }
        Err(SelectorError::Malformed(format!(
            "ObjectReference.Selector: {selector:?} is not a path (/some/path), an object \
             identifier ($ObjectIdentifier) or a batch reference (#SomeBatchReference)"
        )))
    }
}

/// Why a selector did not name an object.
#[derive(Debug)]
pub enum SelectorError {
    /// The selector is not one of the documented forms.
    Malformed(String),
    /// Well formed, but it names no object in this directory.
    NotFound(String),
}

/// Per-service state for com-amazonaws-clouddirectory-amazonclouddirectory-20170111. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsClouddirectoryAmazonclouddirectory20170111State {
    /// Every schema in the account, keyed by its own ARN — development, published and
    /// the applied copies attached to directories.
    pub schemas: HashMap<String, SchemaRecord>,
    /// Every directory in the account, keyed by directory ARN.
    pub directories: HashMap<String, DirectoryRecord>,
    /// The monotonic source of the opaque identifiers this service mints. Private, so
    /// an identifier can only be obtained from [`Self::mint_identifier`] and is
    /// therefore never reused.
    next_identifier: u64,
}

impl ComAmazonawsClouddirectoryAmazonclouddirectory20170111State {
    /// A fresh opaque identifier for a directory or an object.
    ///
    /// Cloud Directory documents `ObjectIdentifier` as "an opaque string provided by
    /// Amazon Cloud Directory [...] no two objects will ever share the same object
    /// identifier": the value carries no structure a caller may parse, and it is unique
    /// because the counter behind it only ever moves forward within this store. The
    /// identifier is minted here and stored by the caller, so a value a caller sees is
    /// always one this service holds.
    pub fn mint_identifier(&mut self) -> String {
        self.next_identifier = self.next_identifier.wrapping_add(1);
        format!("AQ{:016x}", self.next_identifier)
    }

    /// How many directories still exist — the count [`DIRECTORY_QUOTA`] bounds.
    ///
    /// A deleted directory's tombstone is excluded, so deleting one frees capacity
    /// for a new directory straight away.
    pub fn live_directory_count(&self) -> usize {
        self.directories
            .values()
            .filter(|directory| directory.state != DirectoryLifecycle::Deleted)
            .count()
    }

    /// Drop the oldest deleted-directory tombstones beyond [`DIRECTORY_TOMBSTONE_QUOTA`].
    ///
    /// Oldest by `CreationDateTime`, with the ARN breaking a tie so the choice is
    /// deterministic. Called before a directory is created, which is the only moment
    /// the store grows.
    pub fn prune_directory_tombstones(&mut self) {
        let mut tombstones: Vec<(i64, String)> = self
            .directories
            .values()
            .filter(|directory| directory.state == DirectoryLifecycle::Deleted)
            .map(|directory| {
                (
                    directory.creation_date_time.to_millis().unwrap_or(0),
                    directory.arn.clone(),
                )
            })
            .collect();
        if tombstones.len() <= DIRECTORY_TOMBSTONE_QUOTA {
            return;
        }
        tombstones.sort_unstable();
        let excess = tombstones.len() - DIRECTORY_TOMBSTONE_QUOTA;
        for (_, arn) in tombstones.into_iter().take(excess) {
            self.directories.remove(&arn);
        }
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsClouddirectoryAmazonclouddirectory20170111State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsClouddirectoryAmazonclouddirectory20170111State>)
}

/// The Region this request is scoped to: the Region field of its SigV4 credential
/// scope, or [`FALLBACK_REGION`] when the request carries none.
pub fn request_region(parts: &http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| FALLBACK_REGION.to_string())
}

/// The Region segment of an ARN (`arn:<partition>:<service>:<region>:…`), when it has
/// one. A resource's own ARN is where its Region comes from, so an operation that acts
/// on an existing resource reads the Region from there rather than from the request.
pub fn arn_region(arn: &str) -> Option<&str> {
    let mut fields = arn.split(':');
    if fields.next()? != "arn" {
        return None;
    }
    let _partition = fields.next()?;
    let _service = fields.next()?;
    let region = fields.next()?;
    (!region.is_empty()).then_some(region)
}

/// The current wall-clock instant as the modelled `Timestamp` type.
///
/// Lives beside [`DirectoryRecord`] because it produces the stored
/// `creation_date_time` the record projects: every writer of a directory record uses
/// this one clock conversion. The value is stored once at creation, so every later
/// read of the directory reports the same instant.
pub fn now() -> ::aws_smithy_types::DateTime {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0);
    ::aws_smithy_types::DateTime::from_millis(millis)
}

/// `arn:aws:clouddirectory:<region>:<account>:schema/development/<name>`.
pub fn development_schema_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("schema/development/{name}"))
}

/// `arn:aws:clouddirectory:<region>:<account>:schema/published/<name>/<version>`,
/// with the minor version appended as a further segment when one was published.
pub fn published_schema_arn(
    region: &str,
    name: &str,
    version: &str,
    minor_version: Option<&str>,
) -> String {
    let resource = match minor_version {
        Some(minor) => format!("schema/published/{name}/{version}/{minor}"),
        None => format!("schema/published/{name}/{version}"),
    };
    ::engine::emulator_arn(ARN_NAMESPACE, region, &resource)
}

/// `arn:aws:clouddirectory:<region>:<account>:directory/<directory id>`.
pub fn directory_arn(region: &str, directory_id: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("directory/{directory_id}"))
}

/// `<directory arn>/schema/<name>/<version>` — the applied copy of a published schema,
/// which lives under the directory that applied it.
pub fn applied_schema_arn(directory_arn: &str, name: &str, version: &str) -> String {
    format!("{directory_arn}/schema/{name}/{version}")
}

/// Whether `object` can hold children — the condition `ListObjectChildren` and the
/// other node-only operations require.
///
/// An object is a node unless every facet it carries declares an `ObjectType` other
/// than `NODE`; the directory root, which carries no facet at all, is a node.
pub fn object_is_node(
    schemas: &HashMap<String, SchemaRecord>,
    object: &ObjectRecord,
) -> bool {
    let mut declared = false;
    for facet_ref in &object.schema_facets {
        let (Some(schema_arn), Some(facet_name)) =
            (facet_ref.schema_arn.as_deref(), facet_ref.facet_name.as_deref())
        else {
            continue;
        };
        let Some(facet) = schemas
            .get(schema_arn)
            .and_then(|schema| schema.facets.get(facet_name))
        else {
            continue;
        };
        match facet.object_type.as_deref() {
            Some("NODE") => return true,
            Some(_) => declared = true,
            // A facet that declares no object type says nothing about node-ness.
            None => {}
        }
    }
    !declared
}

/// An opaque, resumable pagination token naming an offset into one listing.
///
/// `scope` identifies what is being listed (the object whose children are paged, say),
/// so a token issued for one listing is refused on another; a token this emulator never
/// issued does not decode at all.
pub fn encode_page_token(scope: &str, offset: usize) -> String {
    // serde_json is the codec: the token is the serialized payload, so nothing about
    // its format is hand-rolled and a token this service did not issue does not parse.
    ::serde_json::json!({ "scope": scope, "offset": offset }).to_string()
}

/// The offset a token names, or `None` when it was not issued by [`encode_page_token`]
/// for this `scope`.
pub fn decode_page_token(token: &str, scope: &str) -> Option<usize> {
    let value: ::serde_json::Value = ::serde_json::from_str(token).ok()?;
    if value.get("scope")?.as_str()? != scope {
        return None;
    }
    usize::try_from(value.get("offset")?.as_u64()?).ok()
}
