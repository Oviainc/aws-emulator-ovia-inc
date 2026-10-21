Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 9fd9433686ca60725b6b64638320351edab7ca3b

# Bootstrap the App Mesh emulator

_Filed: 2026-10-21T21:53:30Z_
_Operation: service-com-amazonaws-appmesh-appmesh | Class: stub_

> _Note: this issue was identified against the code base at commit `9fd9433686ca60725b6b64638320351edab7ca3b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 38 operations in `services/service-com-amazonaws-appmesh-appmesh/src/ops/` is an unmodified scaffold: it carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and answers HTTP 501 `NotImplemented` while holding zero state. No client workflow can succeed. Implement a FIRST, SMALL, stateful slice covering the mesh -> virtual-node lifecycle (restJson1, sigv4 name `appmesh`, URI prefix `/v20190125`).

### Happy path to implement (in order)

1. **CreateMesh** — `PUT /v20190125/meshes`. Input: `meshName` (required, len 1..255), `spec` (MeshSpec), `tags`, `clientToken`. Output: `mesh` (MeshData, `@httpPayload`, required) = `{meshName, spec, metadata: ResourceMetadata{arn, version, uid, createdAt, lastUpdatedAt, meshOwner, resourceOwner}, status: MeshStatus{status}}`. Must persist the mesh, mint a real ARN (`arn:aws:appmesh:<region>:<account>:mesh/<meshName>`) and uid, set `version = 1`, `createdAt`/`lastUpdatedAt` to now, `status.status = ACTIVE`. Re-creating an existing `meshName` returns `ConflictException` (409).
2. **DescribeMesh** — `GET /v20190125/meshes/{meshName}`. Output `mesh` must be byte-for-byte the stored record (same arn, uid, version, timestamps, the `spec` as submitted) — not a re-derived or echoed body. Unknown `meshName` -> `NotFoundException` (404).
3. **ListMeshes** — `GET /v20190125/meshes`, `@paginated(inputToken=nextToken, outputToken=nextToken, items=meshes)`, `limit` 1..100. Output `meshes` (list<MeshRef>, required) with each `MeshRef{meshName, meshOwner, resourceOwner, arn, version, createdAt, lastUpdatedAt}` built from stored state; a created mesh must appear and a deleted one must not. Honour `limit` and round-trip `nextToken`.
4. **CreateVirtualNode** — `PUT /v20190125/meshes/{meshName}/virtualNodes`. Input: `virtualNodeName` (required), `meshName` (`@httpLabel`, required), `spec` (VirtualNodeSpec, required), `tags`, `clientToken`, `meshOwner` (`@httpQuery`). Output: `virtualNode` (VirtualNodeData, `@httpPayload`) = `{meshName, virtualNodeName, spec, metadata: ResourceMetadata, status: VirtualNodeStatus{status}}`. Must be stored under the parent mesh; creating in a non-existent mesh -> `NotFoundException` (404); duplicate name -> `ConflictException` (409). **DescribeVirtualNode** (`GET .../virtualNodes/{virtualNodeName}`) and **ListVirtualNodes** must read the same stored record back.
5. **DeleteVirtualNode** — `DELETE /v20190125/meshes/{meshName}/virtualNodes/{virtualNodeName}`. Removes the node from state and returns the last-known `virtualNode` (VirtualNodeData) in the response payload. A subsequent DescribeVirtualNode -> `NotFoundException` (404).
6. **DeleteMesh** — `DELETE /v20190125/meshes/{meshName}`. Returns the deleted `mesh` (MeshData) and removes it. While the mesh still contains child resources (virtual nodes/routers/services/gateways) it must fail with `ResourceInUseException` (409) rather than deleting; after the child is gone the delete succeeds and DescribeMesh -> `NotFoundException` (404).

Every one of these must compute its response from persisted state. No canned bodies, no echoing the input back, no minting an ARN/uid that is never stored. `metadata.version` must start at 1 and increment on update; `lastUpdatedAt` must change on update while `createdAt` and `uid` stay fixed.

## State Model

In `src/state.rs` (`ComAmazonawsAppmeshAppmeshState`, reached via `crate::state::service_state(&*self.engine)`):

- `meshes: BTreeMap<String /*meshName*/, Mesh>` — ordered so ListMeshes/pagination is deterministic.
- `struct Mesh { name: String, spec: MeshSpec, tags: Vec<TagRef>, metadata: ResourceMeta, status: MeshStatusCode /*ACTIVE|INACTIVE|DELETED*/, virtual_nodes: BTreeMap<String, VirtualNode>, virtual_routers: BTreeMap<String, VirtualRouter>, virtual_services: BTreeMap<String, VirtualService>, virtual_gateways: BTreeMap<String, VirtualGateway> }` (the latter three may stay empty maps in this slice — they exist so DeleteMesh's ResourceInUse check is real).
- `struct VirtualNode { name: String, mesh_name: String, spec: VirtualNodeSpec, tags: Vec<TagRef>, metadata: ResourceMeta, status: VirtualNodeStatusCode }`.
- `struct ResourceMeta { arn: String, uid: String /*uuid*/, version: i64, created_at: SystemTime, last_updated_at: SystemTime, mesh_owner: String /*12-digit account*/, resource_owner: String }` — maps 1:1 onto the modeled `ResourceMetadata`.
- Optional for idempotency: `client_tokens: HashMap<String /*clientToken*/, String /*resource arn*/>` so a repeated `@idempotencyToken` returns the first result instead of a 409.

All remaining operations (virtual routers, routes, virtual services, virtual gateways, gateway routes, Update*, TagResource/UntagResource/ListTagsForResource) stay 501 for now and are tracked as individual gap issues.

## Evidence

1) `services/service-com-amazonaws-appmesh-appmesh/OPERATIONS.md` lists all 38 operations with routes/members: CreateMesh `PUT /v20190125/meshes` (`meshName` required, output `mesh` MeshData `@httpPayload` required, errors include ConflictException 409); DescribeMesh `GET /v20190125/meshes/{meshName}` (@readonly, NotFoundException 404); ListMeshes `GET /v20190125/meshes` (@paginated items=meshes, `limit` 1..100, output `meshes` list<MeshRef> required); CreateVirtualNode/DescribeVirtualNode/DeleteVirtualNode under `/v20190125/meshes/{meshName}/virtualNodes`; DeleteMesh declares `ResourceInUseException (HTTP 409)`.
2) `smithy_query` on MeshData/MeshRef/VirtualNodeData/VirtualNodeRef/ResourceMetadata confirms the required response fields (`metadata.arn`, `uid`, `version` starting at 1 and incrementing on update, `createdAt`, `lastUpdatedAt`, `meshOwner`, `resourceOwner`) that can only come from persisted state.
3) `grep -l '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-appmesh-appmesh/src/ops/` matches all 38 handler files — including create_mesh.rs, describe_mesh.rs, list_meshes.rs, create_virtual_node.rs, delete_virtual_node.rs, delete_mesh.rs — each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501).
4) README.md documents restJson1 wiring and that stubs deliberately return 501 rather than a hollow 200; no tests exist beyond the generated `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. Start the emulator and point an App Mesh client/CLI at it (`aws appmesh --endpoint-url <emulator>`).
2. `aws appmesh create-mesh --mesh-name demo` — expect 200 with `mesh.meshName=demo`, a real `mesh.metadata.arn`/`uid`, `metadata.version=1`, `status.status=ACTIVE`; actual: HTTP 501 NotImplemented.
3. `aws appmesh describe-mesh --mesh-name demo` — expect the identical stored MeshData (same arn/uid/version/createdAt); actual: 501.
4. `aws appmesh list-meshes` — expect `meshes` to contain a MeshRef for `demo` with matching arn/version/timestamps; actual: 501.
5. `aws appmesh create-virtual-node --mesh-name demo --virtual-node-name node1 --spec '{"listeners":[{"portMapping":{"port":8080,"protocol":"http"}}]}'` — expect 200 with the spec echoed from storage and `metadata.version=1`; then `describe-virtual-node --mesh-name demo --virtual-node-name node1` must return the same record; actual: 501.
6. `aws appmesh create-virtual-node --mesh-name nosuchmesh --virtual-node-name x --spec '{}'` — expect NotFoundException (404); actual: 501.
7. `aws appmesh delete-mesh --mesh-name demo` while node1 exists — expect ResourceInUseException (409); actual: 501.
8. `aws appmesh delete-virtual-node --mesh-name demo --virtual-node-name node1` then `delete-mesh --mesh-name demo` — expect both 200 returning the deleted resource payloads, then `describe-mesh --mesh-name demo` -> NotFoundException (404) and `list-meshes` no longer lists `demo`; actual: 501 for all.
