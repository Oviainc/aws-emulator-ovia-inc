Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 57742dc737335ed8f07f6b76d23b07c0a9c73918

# Bootstrap the schemas (EventBridge Schema Registry) emulator

_Filed: 2026-10-21T21:28:05Z_
_Operation: service-com-amazonaws-schemas-schemas | Class: missing_

> _Note: this issue was identified against the code base at commit `57742dc737335ed8f07f6b76d23b07c0a9c73918`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 31 operations in `services/service-com-amazonaws-schemas-schemas/src/ops/` is an unmodified scaffold: the handler carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` → HTTP 501, holding zero state. Implement a FIRST minimal slice: the registry → schema lifecycle (restJson1, sigv4 name `schemas`).

### Happy path to implement (in order)

1. **CreateRegistry** — `POST /v1/registries/name/{RegistryName}` → 201. Input: `RegistryName` (@httpLabel, required), `Description` (0..256), `Tags`. Output must echo the PERSISTED registry: `RegistryName`, `RegistryArn` (synthesize `arn:aws:schemas:<region>:<account>:registry/<RegistryName>`), `Description`, `Tags`. Creating a name that already exists → `ConflictException` (409).
2. **CreateSchema** — `POST /v1/registries/name/{RegistryName}/schemas/name/{SchemaName}` → 201. Input: `RegistryName`, `SchemaName` (@httpLabel, required), `Content` (1..100000, required), `Type` (`JSONSchemaDraft4` | `OpenApi3`, required), `Description`, `Tags`. Must store version `"1"` and return `SchemaArn`, `SchemaName`, `SchemaVersion="1"`, `Type`, `Description`, `LastModified`, `VersionCreatedDate` computed from stored state (timestamps set at write time, not fabricated per read).
3. **DescribeSchema** — `GET /v1/registries/name/{RegistryName}/schemas/name/{SchemaName}` → 200, optional `?schemaVersion=`. Must return the exact `Content` that CreateSchema/UpdateSchema stored for the requested version (latest when omitted), plus `SchemaArn`, `SchemaVersion`, `Type`, `Description`, `LastModified`, `VersionCreatedDate`, `Tags`. Unknown registry/schema/version → `NotFoundException` (404).
4. **ListSchemas** — `GET /v1/registries/name/{RegistryName}/schemas` → 200. Must enumerate the schemas actually stored in that registry as `SchemaSummary` items, honouring `SchemaNamePrefix`, `Limit` and `NextToken` (@paginated inputToken=NextToken/outputToken=NextToken/items=Schemas); `NextToken` absent on the last page. Unknown registry → `NotFoundException`.
5. **DeleteSchema** — `DELETE /v1/registries/name/{RegistryName}/schemas/name/{SchemaName}` → 204, no body. Must remove the schema (all versions) so DescribeSchema afterwards is 404 and ListSchemas no longer lists it.
6. **DeleteRegistry** — `DELETE /v1/registries/name/{RegistryName}` → 204. Must remove the registry so DescribeRegistry/ListSchemas on it → 404 and it disappears from ListRegistries.

Useful 7th if cheap: **DescribeRegistry** (`GET /v1/registries/name/{RegistryName}` → 200) and **ListRegistries** (`GET /v1/registries` → 200, paginated, `RegistryNamePrefix`), since the lifecycle verification above reads through them.

NEVER STUBS: every one of these must compute its response from the persisted store. No canned responses, no echoing the request back without storing it, no random ARNs/versions that are not recorded — a create followed by a describe/list must return exactly what was created, and after a delete the reads must 404.

## State Model

```
SchemasState {
    registries: BTreeMap<String /*RegistryName*/, Registry>,
}

Registry {
    name: String,
    arn: String,                       // arn:aws:schemas:<region>:<account>:registry/<name>
    description: Option<String>,
    tags: BTreeMap<String, String>,
    schemas: BTreeMap<String /*SchemaName*/, Schema>,
}

Schema {
    name: String,
    arn: String,                       // .../schema/<registry>/<name>
    description: Option<String>,
    r#type: String,                    // JSONSchemaDraft4 | OpenApi3
    tags: BTreeMap<String, String>,
    latest_version: u64,               // rendered as the decimal string SchemaVersion
    last_modified: SystemTime,
    versions: BTreeMap<String /*"1","2",...*/, SchemaVersion>,
}

SchemaVersion {
    version: String,
    content: String,                   // returned verbatim by DescribeSchema
    r#type: String,
    description: Option<String>,
    created_date: SystemTime,
}
```

State lives behind the engine guard reached via `crate::state::service_state(&*self.engine)` (as the stub doc-comments describe) and must persist across requests. Pagination tokens should encode the last-returned key so the next page resumes deterministically.

Out of scope for this slice (stay 501, filed separately later): discoverers (Create/Describe/Update/Delete/Start/StopDiscoverer, ListDiscoverers, GetDiscoveredSchema), code bindings (PutCodeBinding, DescribeCodeBinding, GetCodeBindingSource), ExportSchema, SearchSchemas, resource policies (Put/Get/DeleteResourcePolicy), tagging APIs (TagResource, UntagResource, ListTagsForResource), UpdateRegistry/UpdateSchema/DeleteSchemaVersion/ListSchemaVersions.

## Evidence

`services/service-com-amazonaws-schemas-schemas/OPERATIONS.md` lists all 31 operations with routes, success statuses, required members and modeled errors — CreateRegistry (POST /v1/registries/name/{RegistryName} → 201, ConflictException 409), CreateSchema (→ 201, Content+Type required), DescribeSchema (GET → 200, Content/SchemaVersion out, NotFoundException 404), ListSchemas (@paginated items=Schemas, Limit/NextToken/SchemaNamePrefix), DeleteSchema and DeleteRegistry (DELETE → 204, NotFoundException 404). `src/ops/create_registry.rs` lines 4-6 and 165 show the scaffold marker and `Err(crate::scaffolded::Scaffolded::not_implemented("CreateRegistry"))`; all 31 op files are generated identically, so no operation holds state today. Grounded additionally in well-known EventBridge Schema Registry semantics: schema versions start at "1" and increment on content change, DescribeSchema returns the stored content, and deleted resources 404.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `schemas` client at it.
2. `aws schemas create-registry --registry-name demo-registry --description 'demo'` → expect 201 with RegistryName=demo-registry and a schemas ARN (today: HTTP 501 NotImplemented).
3. `aws schemas create-schema --registry-name demo-registry --schema-name demo-schema --type OpenApi3 --content '{"openapi":"3.0.0"}'` → expect 201 with SchemaVersion="1".
4. `aws schemas describe-schema --registry-name demo-registry --schema-name demo-schema` → expect 200 with Content byte-identical to what was sent, Type=OpenApi3, SchemaVersion="1".
5. `aws schemas list-schemas --registry-name demo-registry` → expect demo-schema in Schemas; repeat with `--limit 1` and follow NextToken to confirm pagination terminates with no NextToken.
6. `aws schemas delete-schema --registry-name demo-registry --schema-name demo-schema` → expect 204; then describe-schema → NotFoundException (404) and list-schemas → empty.
7. `aws schemas delete-registry --registry-name demo-registry` → expect 204; then describe-registry → NotFoundException (404) and list-registries no longer contains demo-registry.
8. Re-run create-registry twice for the same name → second call must be ConflictException (409).
