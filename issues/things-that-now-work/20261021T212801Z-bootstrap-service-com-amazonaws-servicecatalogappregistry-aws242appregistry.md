Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 57742dc737335ed8f07f6b76d23b07c0a9c73918

# Bootstrap the Service Catalog AppRegistry emulator

_Filed: 2026-10-21T21:28:01Z_
_Operation: service-com-amazonaws-servicecatalogappregistry-aws242appregistry | Class: missing_

> _Note: this issue was identified against the code base at commit `57742dc737335ed8f07f6b76d23b07c0a9c73918`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 24 operations in `src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no AppRegistry client workflow works at all. Implement a FIRST stateful slice covering the application + attribute-group core loop (restJson1, endpoint prefix `servicecatalog-appregistry`, sigv4 name `servicecatalog`).

### Happy path to implement (in order)

1. **CreateApplication** — `POST /applications` -> **201**. Input: `name` (required, 1..256, `^[-.\w]+$`), `description` (0..1024), `tags` (map, 0..50), `clientToken` (required, idempotency token). Output: `application` (Application{id, arn, name, description, creationTime, lastUpdateTime, tags, applicationTag}). Must mint a NEW 26-char lowercase-alnum `id` matching `^[a-z0-9]+$`, build `arn` as `arn:aws:servicecatalog:<region>:<account>:/applications/<id>`, set `creationTime == lastUpdateTime` to now (ISO-8601), persist the record, and return `ConflictException` (409) when the name already exists in the region. Repeating the same `clientToken` with the same parameters must return the SAME stored application (not a second one); same token with different parameters is a 409.
2. **GetApplication** — `GET /applications/{application}` -> **200**. The `{application}` label accepts application name, id OR ARN — all three must resolve to the same stored record. Output must be read back from state: `id`, `arn`, `name`, `description`, `creationTime`, `lastUpdateTime`, `tags`, `associatedResourceCount` (required, computed as the number of currently associated resources — 0 right after create), `applicationTag` (e.g. `{"awsApplication": <applicationArn>}`). Unknown name/id/ARN -> `ResourceNotFoundException` (404).
3. **ListApplications** — `GET /applications` -> **200**, `@paginated(inputToken=nextToken, outputToken=nextToken, items=applications)`. Output `applications` is a list of ApplicationSummary{id, arn, name, description, creationTime, lastUpdateTime} built from every persisted application. Honour `maxResults` (1..100) by truncating and returning an opaque `nextToken` (must match `^[A-Za-z0-9+/=]+$`, e.g. base64) that resumes exactly where the previous page stopped; the last page omits `nextToken`. A created application must appear here, and a deleted one must not.
4. **CreateAttributeGroup** — `POST /attribute-groups` -> **201**. Input: `name` (required), `attributes` (required, 1..8000 JSON string), `description`, `tags`, `clientToken` (required). Output: `attributeGroup` (AttributeGroup{id, arn, name, description, creationTime, lastUpdateTime, tags}). Mint `id`, build `arn` as `.../attribute-groups/<id>`, store `attributes` VERBATIM so `GetAttributeGroup` returns the same string; duplicate name -> `ConflictException` (409).
5. **AssociateAttributeGroup** — `PUT /applications/{application}/attribute-groups/{attributeGroup}` -> **200**. Resolve both labels (name/id/ARN) from state; unknown either side -> `ResourceNotFoundException` (404). Record the link in the application's attribute-group set and return `applicationArn` + `attributeGroupArn` resolved from the stored records (never echoed from the path labels). A subsequent **ListAssociatedAttributeGroups** (`GET /applications/{application}/attribute-groups`) must return that attribute group's id in `attributeGroups`, paginated the same way as ListApplications.
6. **DeleteApplication** — `DELETE /applications/{application}` -> **200**. Output: `application` (ApplicationSummary of the record as it existed immediately before removal). Remove it from the store (plus its association links) so a following GetApplication and a following DeleteApplication both return `ResourceNotFoundException` (404), and ListApplications no longer lists it.

Every one of these MUST compute its response from persisted state. No canned bodies, no echoing the request back, no minting an id that is never stored. A `create` followed by a `get` must return exactly what was created, including timestamps and tags.

## State Model

In `src/state.rs`, on the shared engine (reached via `crate::state::service_state(&*self.engine)`):

- `applications: HashMap<String /* application id */, Application>` where `Application { id: String /*26 lowercase alnum*/, arn: String, name: String, description: Option<String>, creation_time: SystemTime, last_update_time: SystemTime, tags: BTreeMap<String,String>, attribute_group_ids: BTreeSet<String>, resources: BTreeMap<(ResourceType, String /*resource*/), ResourceAssociation>, application_tag: BTreeMap<String,String> }`
- `application_names: HashMap<String /* name */, String /* id */>` — enforces region-unique names (409 on collision) and lets name/id/ARN labels resolve to one record.
- `attribute_groups: HashMap<String /* attribute group id */, AttributeGroup>` where `AttributeGroup { id, arn, name, description: Option<String>, attributes: String /* stored verbatim */, creation_time, last_update_time, tags: BTreeMap<String,String>, created_by: Option<String> }`
- `attribute_group_names: HashMap<String /* name */, String /* id */>` — same uniqueness/resolution role.
- `idempotency: HashMap<String /* clientToken */, IdempotentResult { resource_id: String, request_fingerprint: u64 }>` — replay of a token returns the stored resource; a mismatched fingerprint is a 409.
- `pagination: HashMap<String /* nextToken */, Cursor { kind, offset }>` or an encoded offset token — whichever is used, the token must be opaque and base64-shaped.

All of it lives in the ONE shared engine so writes from one request are visible to the next.

Done = a client script can run CreateApplication -> GetApplication -> ListApplications -> CreateAttributeGroup -> AssociateAttributeGroup -> ListAssociatedAttributeGroups -> DeleteApplication -> GetApplication(404) with correct bodies at every step. The other 17 operations (AssociateResource, GetAssociatedResource, ListAssociatedResources, ListAttributeGroups, ListAttributeGroupsForApplication, Update*/Delete AttributeGroup, Disassociate*, SyncResource, Get/PutConfiguration, Tag/Untag/ListTagsForResource) stay 501 for now and will be filed as individual gap issues.

## Evidence

`services/service-com-amazonaws-servicecatalogappregistry-aws242appregistry/OPERATIONS.md` lists all 24 operations with their routes, input/output members and errors (CreateApplication `POST /applications -> 201`, GetApplication `GET /applications/{application} -> 200` with required `associatedResourceCount`, ListApplications `@paginated(items=applications)`, CreateAttributeGroup `POST /attribute-groups -> 201`, AssociateAttributeGroup `PUT /applications/{application}/attribute-groups/{attributeGroup} -> 200`, DeleteApplication `DELETE /applications/{application} -> 200` returning ApplicationSummary). Shape members for Application/ApplicationSummary/AttributeGroup/AttributeGroupSummary confirmed via smithy_query (ApplicationId is len 26..26 `^[a-z0-9]+$`; timestamps are `date-time`). Every file under `src/ops/` carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker — e.g. `src/ops/create_application.rs:4` with `Err(crate::scaffolded::Scaffolded::not_implemented("CreateApplication"))` at line 170 — so there is zero state and zero behaviour today. No tests exist beyond the generated `tests/state_roundtrip.rs` example.

## Steps to Reproduce

1. POST /applications with {"name":"my-app","description":"demo","tags":{"env":"dev"},"clientToken":"tok1"} -> expect 201 with application.id (26 lowercase alnum), application.arn ending in /applications/<id>, creationTime == lastUpdateTime; actual: 501 NotImplemented
2. Repeat the identical POST with clientToken "tok1" -> expect 201/200 returning the SAME application id (idempotent replay), and ListApplications showing exactly one application
3. GET /applications/my-app -> expect 200 with the stored name, description, tags, timestamps and associatedResourceCount 0; repeat with the id and with the full ARN and expect the same body
4. GET /applications?maxResults=1 after creating two applications -> expect 1 item plus an opaque nextToken; GET /applications?maxResults=1&nextToken=<token> -> expect the second item and no nextToken
5. POST /attribute-groups with {"name":"my-ag","attributes":"{\"team\":\"platform\"}","clientToken":"tok2"} -> expect 201 with attributeGroup.id/arn; GET /attribute-groups/my-ag -> expect the attributes string returned byte-for-byte
6. PUT /applications/my-app/attribute-groups/my-ag -> expect 200 with applicationArn and attributeGroupArn resolved from state; GET /applications/my-app/attribute-groups -> expect attributeGroups to contain the attribute group id
7. PUT /applications/my-app/attribute-groups/does-not-exist -> expect 404 ResourceNotFoundException
8. DELETE /applications/my-app -> expect 200 with the ApplicationSummary of the deleted application; then GET /applications/my-app -> expect 404 ResourceNotFoundException and GET /applications -> expect the application absent
