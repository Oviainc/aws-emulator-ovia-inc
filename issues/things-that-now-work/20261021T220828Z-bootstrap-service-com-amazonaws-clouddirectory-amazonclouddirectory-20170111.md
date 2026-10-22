Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 95c91931c960f7f391e4448ce7f3ebc67d41fa17

# Bootstrap the clouddirectory emulator

_Filed: 2026-10-21T22:08:28Z_
_Operation: service-com-amazonaws-clouddirectory-amazonclouddirectory-20170111 | Class: missing_

> _Note: this issue was identified against the code base at commit `95c91931c960f7f391e4448ce7f3ebc67d41fa17`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 66 operations in this crate is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no Amazon Cloud Directory client can do anything at all. Implement a FIRST, SMALL, stateful slice: the schema-then-directory-then-object happy path, which is the minimum Cloud Directory requires before any data call is legal.

Happy path to implement, in order (restJson1, routes under `/amazonclouddirectory/2017-01-11/...`, ARNs/identifiers passed in the `x-amz-data-partition` header):

1. **CreateSchema** — `PUT /schema/create`. Input: `Name` (1..32, `^[a-zA-Z0-9._-]*$`, required). Output: `SchemaArn` — a freshly minted DEVELOPMENT schema ARN (`arn:aws:clouddirectory:<region>:<account>:schema/development/<Name>`) that is recorded in state. A second CreateSchema with the same name must fail `SchemaAlreadyExistsException` (400).
2. **CreateFacet** — `PUT /facet/create`. Input: `SchemaArn` (header, required), `Name` (1..64, required), `Attributes` (list<FacetAttribute>), `ObjectType` (INDEX|LEAF_NODE|NODE|POLICY), `FacetStyle`. Empty output body, but the facet and its attribute definitions must be stored on the named development schema; unknown `SchemaArn` → `ResourceNotFoundException` (404), duplicate facet name → `FacetAlreadyExistsException` (400).
3. **PublishSchema** — `PUT /schema/publish`. Input: `DevelopmentSchemaArn` (header, required), `Version` (required), optional `MinorVersion`, `Name`. Output: `PublishedSchemaArn` — a new published ARN (`.../schema/published/<name>/<Version>`) holding a frozen COPY of the development schema's facets. Republishing the same name+version → `SchemaAlreadyPublishedException`.
4. **CreateDirectory** — `PUT /directory/create`. Input: `Name` (1..64, required), `SchemaArn` (header, the PUBLISHED arn, required). Output: all four required members computed from the new state: `DirectoryArn`, `Name` (echo of the stored name), `ObjectIdentifier` of the implicitly created ROOT object, `AppliedSchemaArn` (the applied copy of the published schema, `.../directory/<id>/schema/<name>/<version>`). Duplicate name → `DirectoryAlreadyExistsException`; unknown schema arn → `ResourceNotFoundException`.
5. **CreateObject** — `PUT /object`. Input: `DirectoryArn` (header, required), `SchemaFacets` (list<SchemaFacet{SchemaArn,FacetName}>, required), `ObjectAttributeList` (list<AttributeKeyAndValue{Key{SchemaArn,FacetName,Name},Value}>), optional `ParentReference` (ObjectReference{Selector}) and `LinkName`. Output: the generated `ObjectIdentifier`, which must be stored along with the facets and the attribute values. When `ParentReference`+`LinkName` are supplied, record the parent→child link (reusing a LinkName under the same parent → `LinkNameAlreadyInUseException`); referencing a facet absent from the directory's applied schema → `FacetValidationException`; unknown directory → `ResourceNotFoundException`; a DISABLED directory → `DirectoryNotEnabledException`.
6. **ListObjectChildren** — `POST /object/children`. Input: `DirectoryArn` (header), `ObjectReference` (required; resolve at least the `/` root selector, a `/link/path` selector and a `$<objectIdentifier>` selector), `MaxResults`, `NextToken`. Output: `Children` as a map of LinkName → ObjectIdentifier built from the links actually recorded in step 5, plus `NextToken` when truncated by `MaxResults` (and `InvalidNextTokenException` for a token the emulator did not issue).
7. **DisableDirectory** then **DeleteDirectory** — `PUT /directory/disable` and `PUT /directory`, both taking `DirectoryArn` in the header and returning `DirectoryArn`. Disable flips the stored directory state to DISABLED; Delete only succeeds on a disabled directory (`DirectoryNotDisabledException` otherwise) and marks it DELETED so a later CreateObject/ListObjectChildren against it fails rather than succeeding.

Hard requirement: every one of these must compute its response from persisted state. No canned bodies, no echoing the request back, no minting an ARN or ObjectIdentifier that is never stored. A CreateDirectory followed by ListObjectChildren on `/` must show exactly the children created in between.

## State Model

In `src/state.rs` (reached through `crate::state::service_state(&*self.engine)`):

- `schemas: HashMap<String /*schema arn*/, Schema>` where `Schema { arn, name, stage: Development|Published|Applied, version: Option<String>, minor_version: Option<String>, facets: HashMap<String /*facet name*/, Facet> }`
- `Facet { name, object_type: Option<ObjectType>, facet_style: Option<FacetStyle>, attributes: Vec<FacetAttribute> }`
- `directories: HashMap<String /*directory arn*/, Directory>` where `Directory { arn, name, state: ENABLED|DISABLED|DELETED, creation_date_time, applied_schema_arns: Vec<String>, root_object_id: String, objects: HashMap<String /*object identifier*/, CdObject> }` — the `Directory` wire shape (Name, DirectoryArn, State, CreationDateTime) is projected from this for GetDirectory/ListDirectories later
- `CdObject { object_identifier, schema_facets: Vec<SchemaFacet>, attributes: HashMap<(schema_arn, facet_name, attr_name), TypedAttributeValue>, children: BTreeMap<String /*link name*/, String /*child object id*/>, parents: BTreeMap<String /*parent object id*/, String /*link name*/> }` (BTreeMap so child/parent listing order and pagination tokens are stable)
- a monotonic counter or UUID source for ObjectIdentifiers, plus `next_token` encoding (e.g. an opaque index into the ordered children list) so pagination is resumable

All other operations (BatchRead/BatchWrite, typed links, indices, policies, tagging, upgrades, the remaining List*/Get*/Update*/Detach*/Delete* calls) stay 501 for now and will be filed as individual gap issues.

## Evidence

`services/service-com-amazonaws-clouddirectory-amazonclouddirectory-20170111/OPERATIONS.md` documents all 66 operations, their routes, required inputs and errors — the fields cited above are taken verbatim from its CreateSchema (line 110), CreateFacet (89), PublishSchema (423), CreateDirectory (82), CreateObject (103), ListObjectChildren (336), DisableDirectory (187) and DeleteDirectory (124) sections, with shape members (Directory, ObjectReference.Selector, SchemaFacet, AttributeKeyAndValue, AttributeKey) read from the Smithy model. `grep -l '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches every handler file in the crate (66/66), including `create_schema.rs`, `create_directory.rs`, `create_object.rs` and `list_object_children.rs`, each of which returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` → HTTP 501. No integration tests exist beyond the generated `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. POST CreateSchema {"Name":"demo"} → expect 200 with a development SchemaArn (today: 501)
2. PUT CreateFacet with x-amz-data-partition: <devSchemaArn>, {"Name":"Node","ObjectType":"NODE","Attributes":[{"Name":"label",...}]} → expect 200
3. PUT PublishSchema with x-amz-data-partition: <devSchemaArn>, {"Version":"1"} → expect 200 with PublishedSchemaArn
4. PUT CreateDirectory with x-amz-data-partition: <publishedSchemaArn>, {"Name":"demo-dir"} → expect 200 with DirectoryArn, Name, root ObjectIdentifier and AppliedSchemaArn
5. PUT CreateObject with x-amz-data-partition: <directoryArn>, {"SchemaFacets":[{"SchemaArn":"<appliedSchemaArn>","FacetName":"Node"}],"ObjectAttributeList":[{"Key":{"SchemaArn":"<appliedSchemaArn>","FacetName":"Node","Name":"label"},"Value":{"StringValue":"hello"}}],"ParentReference":{"Selector":"/"},"LinkName":"child1"} → expect 200 with ObjectIdentifier
6. POST ListObjectChildren with x-amz-data-partition: <directoryArn>, {"ObjectReference":{"Selector":"/"}} → expect Children == {"child1": "<objectIdentifier from previous step>"}
7. Repeat CreateObject with LinkName "child1" → expect 400 LinkNameAlreadyInUseException
8. PUT DeleteDirectory on the still-enabled directory → expect 400 DirectoryNotDisabledException; then PUT DisableDirectory, then PUT DeleteDirectory → expect 200 with DirectoryArn, and a subsequent ListObjectChildren on it must no longer succeed
