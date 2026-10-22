// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-appmesh-appmesh service.
//!
//! App Mesh's control plane is a tree: a mesh owns virtual nodes, virtual routers,
//! virtual services and virtual gateways. Every value the handlers return —
//! `MeshData`, `MeshRef`, `VirtualNodeData`, `VirtualNodeRef`, and every
//! `ResourceMetadata` inside them — is rendered from this tree by the `to_data` /
//! `to_ref` methods below, so a read after a write reports exactly what was stored
//! rather than a value re-derived from the request.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use crate::ops::types::{
    MeshData, MeshRef, MeshSpec, MeshStatus, ResourceMetadata, TagRef, VirtualGatewaySpec,
    VirtualNodeData, VirtualNodeRef, VirtualNodeSpec, VirtualNodeStatus, VirtualRouterSpec,
    VirtualServiceSpec,
};

/// The ARN namespace and SigV4 signing name of this service, from
/// `smithy_query(query="service_info")`: `awsService.arnNamespace` = `appmesh`.
const ARN_NAMESPACE: &str = "appmesh";

/// The region an ARN is minted in when the request carries no SigV4 credential
/// scope to read one from (an unsigned call in a test, say).
pub const DEFAULT_REGION: &str = "us-east-1";

/// The status a resource App Mesh has finished creating carries. The enum is
/// `ACTIVE | INACTIVE | DELETED` — `smithy_query(query="shape",
/// shape="MeshStatusCode,VirtualNodeStatusCode")`; a create/describe answers with
/// the first, because App Mesh hands back a mesh and a virtual node already active.
pub const STATUS_ACTIVE: &str = "ACTIVE";

/// The number of meshes this emulator holds for the account, matching App Mesh's
/// published (adjustable) default service quota "Meshes per account". A create past
/// it is refused with the modelled `LimitExceededException` ("You have exceeded a
/// service limit for your account") rather than growing the store without limit.
pub const MAX_MESHES_PER_ACCOUNT: usize = 15;

/// The number of virtual nodes a mesh holds, matching App Mesh's published
/// (adjustable) default service quota "Virtual nodes per mesh". Bounds the per-mesh
/// child count the same way [`MAX_MESHES_PER_ACCOUNT`] bounds the store.
pub const MAX_VIRTUAL_NODES_PER_MESH: usize = 200;

/// The page size a list uses when the caller sends no `limit`, and the ceiling on any
/// page: the maximum the model allows for `limit`
/// (`ListMeshesLimit`/`ListVirtualNodesLimit` have range 1..100). So an omitted `limit`
/// cannot render the whole store into one response, and the walk under the state lock
/// is bounded by a constant.
pub const MAX_PAGE_SIZE: usize = 100;

/// Per-service state for com-amazonaws-appmesh-appmesh. A handler reaches it through [`service_state`].
///
/// `BTreeMap` rather than `HashMap` so `ListMeshes`/`ListVirtualNodes` iterate in
/// name order: the model paginates both with an opaque `nextToken`, which can only
/// resume a page from a stable ordering.
#[derive(Debug, Default)]
pub struct ComAmazonawsAppmeshAppmeshState {
    /// Every mesh in the account, keyed by `meshName`.
    pub meshes: BTreeMap<String, Mesh>,
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsAppmeshAppmeshState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsAppmeshAppmeshState>)
}

/// The region the caller signed for, so an ARN this emulator mints carries the same
/// region the client addressed. Falls back to [`DEFAULT_REGION`] when the request
/// carries no SigV4 credential scope.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// `arn:aws:appmesh:<region>:<account>:mesh/<meshName>`.
pub fn mesh_arn(region: &str, mesh_name: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("mesh/{mesh_name}"))
}

/// `arn:aws:appmesh:<region>:<account>:mesh/<meshName>/virtualNode/<virtualNodeName>`.
pub fn virtual_node_arn(region: &str, mesh_name: &str, virtual_node_name: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("mesh/{mesh_name}/virtualNode/{virtual_node_name}"),
    )
}

/// The pagination cursor for a page that starts at `name`.
///
/// Hex of the key rather than the key itself, so a caller can read no structure into
/// the token and has nothing to build one from: a `nextToken` is opaque and only ever
/// travels back unchanged.
pub fn encode_cursor(name: &str) -> String {
    ::hex::encode(name.as_bytes())
}

/// The inclusive start key a cursor minted by [`encode_cursor`] carries, or `None`
/// when the token was not minted by this service (which a list operation refuses
/// rather than silently treating as a start key).
pub fn decode_cursor(token: &str) -> Option<String> {
    String::from_utf8(::hex::decode(token).ok()?).ok()
}

/// `SystemTime` as the modelled `Timestamp`, keeping sub-second precision so a
/// describe reports the instant that was stored.
fn as_timestamp(instant: SystemTime) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(instant)
}

/// A process-unique sequence number, so two resources created inside the same
/// nanosecond still get distinct uids.
static UID_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// A fresh `ResourceMetadata$uid`.
///
/// The model asks only for "the unique identifier for the resource", so this is unique
/// BY CONSTRUCTION rather than by chance: SHA-256 (the `sha2` crate) over the
/// resource's ARN, its creation instant and a process-unique counter, truncated to 128
/// bits and stamped as an RFC 9562 version 8 UUID — the version the standard reserves
/// for an implementation-defined derivation — then rendered in the canonical hyphenated
/// form clients expect.
fn mint_uid(arn: &str, created_at: SystemTime) -> String {
    use ::sha2::{Digest, Sha256};

    let nanos = created_at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let sequence = UID_SEQUENCE.fetch_add(1, Ordering::Relaxed);

    let mut hasher = Sha256::new();
    hasher.update(arn.as_bytes());
    hasher.update(nanos.to_be_bytes());
    hasher.update(sequence.to_be_bytes());
    let digest = hasher.finalize();

    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    // RFC 9562: version 8 (custom) in the high nibble of octet 6, variant 0b10 in the
    // top bits of octet 8.
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
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

/// The identity and revision App Mesh keeps for every resource; maps 1:1 onto the
/// modelled `ResourceMetadata`.
#[derive(Debug, Clone)]
pub struct ResourceMeta {
    pub arn: String,
    pub uid: String,
    pub version: i64,
    pub created_at: SystemTime,
    pub last_updated_at: SystemTime,
    pub mesh_owner: String,
    pub resource_owner: String,
}

impl ResourceMeta {
    /// Metadata for a resource being created now.
    ///
    /// `version` starts at 1 and `createdAt == lastUpdatedAt`, because the model says
    /// of `ResourceMetadata$version`: "Resources are created at version 1, and this
    /// version is incremented each time that they're updated."
    pub fn new(arn: String) -> Self {
        let now = SystemTime::now();
        let account = ::engine::emulator_account_id().to_string();
        Self {
            uid: mint_uid(&arn, now),
            arn,
            version: 1,
            created_at: now,
            last_updated_at: now,
            mesh_owner: account.clone(),
            resource_owner: account,
        }
    }

    /// Record an update to the resource this metadata describes: the version is
    /// incremented and `lastUpdatedAt` moves, while `arn`, `uid` and `createdAt`
    /// stay as they were — they identify the resource, not the revision.
    pub fn record_update(&mut self) {
        self.version += 1;
        self.last_updated_at = SystemTime::now();
    }

    /// This metadata as the modelled shape.
    pub fn to_modeled(&self) -> ResourceMetadata {
        ResourceMetadata {
            arn: self.arn.clone(),
            version: self.version,
            uid: self.uid.clone(),
            created_at: as_timestamp(self.created_at),
            last_updated_at: as_timestamp(self.last_updated_at),
            mesh_owner: self.mesh_owner.clone(),
            resource_owner: self.resource_owner.clone(),
        }
    }
}

/// A resource that lives inside a mesh, as stored.
///
/// Generic over the resource's modelled spec shape, because a virtual node, router,
/// service and gateway differ only in that spec: one struct keeps the metadata,
/// status and tag handling identical for all four.
#[derive(Debug, Clone)]
pub struct MeshResource<S> {
    pub name: String,
    pub mesh_name: String,
    pub spec: S,
    pub tags: Vec<TagRef>,
    pub metadata: ResourceMeta,
    pub status: String,
    /// The `@idempotencyToken` the create call carried, so a replay of that same
    /// call can be answered with the stored resource instead of a conflict.
    pub client_token: Option<String>,
}

/// A virtual node, as stored under its mesh.
pub type VirtualNode = MeshResource<VirtualNodeSpec>;
/// A virtual router, as stored under its mesh.
pub type VirtualRouter = MeshResource<VirtualRouterSpec>;
/// A virtual service, as stored under its mesh.
pub type VirtualService = MeshResource<VirtualServiceSpec>;
/// A virtual gateway, as stored under its mesh.
pub type VirtualGateway = MeshResource<VirtualGatewaySpec>;

impl<S: ::std::fmt::Debug> MeshResource<S> {
    /// A structural digest of this record's specification and tags.
    ///
    /// The spec shapes are plain `#[derive(Debug)]` data structures, so their
    /// rendering is total and deterministic — enough to tell "the same specification"
    /// from "a different one" without a `PartialEq` bound on the whole shape closure.
    fn spec_digest(&self) -> String {
        format!("{:?}|{:?}", self.spec, self.tags)
    }

    /// Whether `candidate` is a replay of the create call that made this record: the
    /// same `@idempotencyToken` AND the same specification.
    ///
    /// The model documents `ConflictException` as "a client token that was used for a
    /// previous update resource call with different specifications", so a token reused
    /// with a different specification is a conflict, not a replay.
    pub fn is_replay_of(&self, candidate: &Self) -> bool {
        self.client_token.is_some()
            && self.client_token == candidate.client_token
            && self.spec_digest() == candidate.spec_digest()
    }

    /// Apply `spec` as this record's specification and record a new revision — unless
    /// `spec` is the one already stored.
    ///
    /// An update that changes nothing is not a new revision, so a retried update (the
    /// usual SDK behaviour on a timeout, and what the operation's `@idempotencyToken`
    /// exists for) leaves `version` and `lastUpdatedAt` where they are instead of
    /// advancing them for a call that altered no state.
    pub fn apply_spec(&mut self, spec: S) {
        if format!("{:?}|{:?}", spec, self.tags) == self.spec_digest() {
            return;
        }
        self.spec = spec;
        self.metadata.record_update();
    }
}

impl VirtualNode {
    /// A virtual node being created now, in `mesh_name`, with an ARN minted for
    /// `region`.
    pub fn new(
        region: &str,
        mesh_name: &str,
        name: String,
        spec: VirtualNodeSpec,
        tags: Vec<TagRef>,
        client_token: Option<String>,
    ) -> Self {
        let arn = virtual_node_arn(region, mesh_name, &name);
        Self {
            name,
            mesh_name: mesh_name.to_string(),
            spec,
            tags,
            metadata: ResourceMeta::new(arn),
            status: STATUS_ACTIVE.to_string(),
            client_token,
        }
    }

    /// The stored node as `VirtualNodeData` — what Create/Describe/Update/Delete
    /// put in their `@httpPayload`.
    pub fn to_data(&self) -> VirtualNodeData {
        VirtualNodeData {
            mesh_name: self.mesh_name.clone(),
            virtual_node_name: self.name.clone(),
            spec: self.spec.clone(),
            metadata: self.metadata.to_modeled(),
            status: VirtualNodeStatus {
                status: self.status.clone(),
            },
        }
    }

    /// The stored node as the `VirtualNodeRef` `ListVirtualNodes` returns.
    pub fn to_ref(&self) -> VirtualNodeRef {
        VirtualNodeRef {
            mesh_name: self.mesh_name.clone(),
            virtual_node_name: self.name.clone(),
            mesh_owner: self.metadata.mesh_owner.clone(),
            resource_owner: self.metadata.resource_owner.clone(),
            arn: self.metadata.arn.clone(),
            version: self.metadata.version,
            created_at: as_timestamp(self.metadata.created_at),
            last_updated_at: as_timestamp(self.metadata.last_updated_at),
        }
    }
}

/// A service mesh and everything inside it.
#[derive(Debug, Clone)]
pub struct Mesh {
    pub name: String,
    pub spec: MeshSpec,
    pub tags: Vec<TagRef>,
    pub metadata: ResourceMeta,
    pub status: String,
    /// See [`MeshResource::client_token`].
    pub client_token: Option<String>,
    pub virtual_nodes: BTreeMap<String, VirtualNode>,
    /// The three collections below are what makes `DeleteMesh`'s in-use check a real
    /// question about state rather than a constant: the mesh refuses to delete while
    /// ANY child resource remains, whichever kind it is. The operations that create
    /// them are still scaffolded, so today only `virtual_nodes` is ever filled.
    pub virtual_routers: BTreeMap<String, VirtualRouter>,
    pub virtual_services: BTreeMap<String, VirtualService>,
    pub virtual_gateways: BTreeMap<String, VirtualGateway>,
}

impl Mesh {
    /// See [`MeshResource::spec_digest`].
    fn spec_digest(&self) -> String {
        format!("{:?}|{:?}", self.spec, self.tags)
    }

    /// See [`MeshResource::is_replay_of`].
    pub fn is_replay_of(&self, candidate: &Self) -> bool {
        self.client_token.is_some()
            && self.client_token == candidate.client_token
            && self.spec_digest() == candidate.spec_digest()
    }

    /// See [`MeshResource::apply_spec`].
    pub fn apply_spec(&mut self, spec: MeshSpec) {
        if format!("{:?}|{:?}", spec, self.tags) == self.spec_digest() {
            return;
        }
        self.spec = spec;
        self.metadata.record_update();
    }

    /// A mesh being created now, with an ARN minted for `region`.
    pub fn new(
        region: &str,
        name: String,
        spec: MeshSpec,
        tags: Vec<TagRef>,
        client_token: Option<String>,
    ) -> Self {
        let arn = mesh_arn(region, &name);
        Self {
            name,
            spec,
            tags,
            metadata: ResourceMeta::new(arn),
            status: STATUS_ACTIVE.to_string(),
            client_token,
            virtual_nodes: BTreeMap::new(),
            virtual_routers: BTreeMap::new(),
            virtual_services: BTreeMap::new(),
            virtual_gateways: BTreeMap::new(),
        }
    }

    /// The stored mesh as `MeshData` — what Create/Describe/Update/Delete put in
    /// their `@httpPayload`.
    pub fn to_data(&self) -> MeshData {
        MeshData {
            mesh_name: self.name.clone(),
            spec: self.spec.clone(),
            metadata: self.metadata.to_modeled(),
            status: MeshStatus {
                status: Some(self.status.clone()),
            },
        }
    }

    /// The stored mesh as the `MeshRef` `ListMeshes` returns.
    pub fn to_ref(&self) -> MeshRef {
        MeshRef {
            mesh_name: self.name.clone(),
            mesh_owner: self.metadata.mesh_owner.clone(),
            resource_owner: self.metadata.resource_owner.clone(),
            arn: self.metadata.arn.clone(),
            version: self.metadata.version,
            created_at: as_timestamp(self.metadata.created_at),
            last_updated_at: as_timestamp(self.metadata.last_updated_at),
        }
    }

    /// How many resources the mesh still contains. `DeleteMesh` refuses with
    /// `ResourceInUseException` while this is non-zero.
    pub fn child_resource_count(&self) -> usize {
        self.virtual_nodes.len()
            + self.virtual_routers.len()
            + self.virtual_services.len()
            + self.virtual_gateways.len()
    }

    /// Whether a request's `meshOwner` names this mesh's owner.
    ///
    /// `meshOwner` is an optional 12-digit `AccountId` on the shared-mesh
    /// operations; absent means "the caller's own account", which is the only
    /// account this emulator owns resources for.
    pub fn owned_by(&self, mesh_owner: Option<&str>) -> bool {
        mesh_owner.is_none_or(|owner| owner == self.metadata.mesh_owner)
    }
}

/// One page of a `@paginated` App Mesh list operation.
pub struct Page<T> {
    /// The page's items, in key order.
    pub items: Vec<T>,
    /// The opaque cursor the next page resumes from, or `None` when this page is the
    /// last.
    pub next_token: Option<String>,
}

/// Read one page out of a name-keyed collection.
///
/// The model paginates `ListMeshes`/`ListVirtualNodes` with
/// `@paginated(inputToken: nextToken, outputToken: nextToken, pageSize: limit)`, so:
/// items come back in key order, at most `limit` of them, and the page resumes at
/// `start` — the key a caller's `nextToken` decoded to, via [`decode_cursor`]. `limit`
/// is modelled `1..100` and is refused outside that range before a handler runs; an
/// absent or larger one is clamped to [`MAX_PAGE_SIZE`], so no response renders more
/// than one page however large the store is. The returned token is already encoded.
pub fn page<V, T>(
    source: &BTreeMap<String, V>,
    start: Option<&str>,
    limit: Option<i32>,
    render: impl Fn(&V) -> T,
) -> Page<T> {
    let cap = limit
        .filter(|requested| *requested > 0)
        .and_then(|requested| usize::try_from(requested).ok())
        .unwrap_or(MAX_PAGE_SIZE)
        .min(MAX_PAGE_SIZE);
    let start = start.unwrap_or_default().to_string();

    let mut items = Vec::new();
    let mut resume_from = None;
    for (key, value) in source.range(start..) {
        if items.len() >= cap {
            resume_from = Some(encode_cursor(key));
            break;
        }
        items.push(render(value));
    }

    Page {
        items,
        next_token: resume_from,
    }
}
