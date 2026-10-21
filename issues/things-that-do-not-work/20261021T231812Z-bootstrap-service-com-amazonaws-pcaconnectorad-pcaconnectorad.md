Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 16fab4a727e8e34e48fe5cd80cb0080f7932bd9f

# Bootstrap the pca-connector-ad emulator

_Filed: 2026-10-21T23:18:12Z_
_Operation: service-com-amazonaws-pcaconnectorad-pcaconnectorad | Class: missing_

> _Note: this issue was identified against the code base at commit `16fab4a727e8e34e48fe5cd80cb0080f7932bd9f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 25 operations in `services/service-com-amazonaws-pcaconnectorad-pcaconnectorad/src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no client workflow works at all. Implement a first, minimal, STATEFUL slice around the service's primary resource (the Connector) and its child Templates. Protocol is `restJson1`, signing name `pca-connector-ad`.

### Happy path to implement (in order)

1. **CreateConnector** — `POST /connectors -> 202`. Input: `DirectoryId` (required, `^d-[0-9a-f]{10}$`), `CertificateAuthorityArn` (required, acm-pca CA ARN pattern), `VpcInformation` (required; `SecurityGroupIds` 1..4 unique, optional `IpAddressType`), optional `ClientToken` (@idempotencyToken) and `Tags`. Must mint a NEW connector ARN of the modeled form `arn:aws:pca-connector-ad:<region>:<account>:connector/<uuid>` from a freshly generated UUID, persist the whole record (directory id, CA ARN, VPC info, tags, `CreatedAt`/`UpdatedAt`, `Status`), and return only `ConnectorArn`.
2. **GetConnector** — `GET /connectors/{ConnectorArn} -> 200`. Must look the stored record up by ARN and return the full `Connector` structure populated from state: `Arn`, `CertificateAuthorityArn`, `DirectoryId`, `VpcInformation`, `CertificateEnrollmentPolicyServerEndpoint`, `Status` (`ConnectorStatus`: CREATING|ACTIVE|DELETING|FAILED — settle to `ACTIVE`), `CreatedAt`, `UpdatedAt`. Unknown/deleted ARN -> `ResourceNotFoundException` (HTTP 404), not 501 and not an empty 200.
3. **ListConnectors** — `GET /connectors -> 200`, @paginated(items=`Connectors`). Must return one `ConnectorSummary` per persisted connector (same fields as `Connector`), honour `MaxResults` (1..1000) and round-trip an opaque `NextToken` that matches the modeled base64url pattern; return no `NextToken` on the last page.
4. **CreateTemplate** — `POST /templates -> 200`. Input: `ConnectorArn` (required), `Name` (required, 1..64, unique per connector), `Definition` (required `TemplateDefinition` union), optional `ClientToken`, `Tags`. Must reject an unknown `ConnectorArn` with `ResourceNotFoundException`, reject a duplicate `Name` on the same connector with `ConflictException` (409), mint a template ARN of the modeled child form `<connectorArn>/template/<uuid>`, persist the definition plus `Revision` (major/minor), `PolicySchema`, `Status`, `CreatedAt`/`UpdatedAt`, and return `TemplateArn`.
5. **GetTemplate** — `GET /templates/{TemplateArn} -> 200`. Must return the `Template` structure read back from state: `Arn`, `ConnectorArn`, `Name`, `Definition` byte-for-byte as supplied to CreateTemplate, `ObjectIdentifier`, `PolicySchema`, `Status`, `Revision`, `CreatedAt`, `UpdatedAt`. Unknown ARN -> `ResourceNotFoundException`.
6. **DeleteConnector** — `DELETE /connectors/{ConnectorArn} -> 202`, @idempotent. Must remove the connector (and cascade-remove its templates) from state so a subsequent GetConnector returns 404 and ListConnectors no longer lists it. Deleting an already-absent ARN must stay a success (idempotent), not 500.

Every one of these must compute its response from the persisted store reached via `crate::state::service_state(&*self.engine)` — no canned bodies, no echoing the request back, and no minting an ARN without storing the record behind it. A `CreateX` followed by `GetX` must return exactly what was created, and after `DeleteConnector` the reads must observe it gone.

## State Model

In `src/state.rs` (`ComAmazonawsPcaconnectoradPcaconnectoradState`):

- `connectors: HashMap<String /* connector ARN */, Connector>` where
  `Connector { arn: String, certificate_authority_arn: String, directory_id: String, vpc_information: VpcInformation { security_group_ids: Vec<String>, ip_address_type: Option<IpAddressType> }, certificate_enrollment_policy_server_endpoint: String, status: ConnectorStatus, status_reason: Option<ConnectorStatusReason>, created_at: SystemTime, updated_at: SystemTime, tags: HashMap<String,String> }`
- `templates: HashMap<String /* template ARN */, Template>` where
  `Template { arn: String, connector_arn: String, name: String, definition: TemplateDefinition, object_identifier: String, policy_schema: i32, status: TemplateStatus, revision: TemplateRevision { major: i32, minor: i32 }, created_at: SystemTime, updated_at: SystemTime, tags: HashMap<String,String> }`
  (index or filter by `connector_arn` so ListTemplates/cascade delete are cheap)
- `idempotency: HashMap<String /* ClientToken */, String /* ARN returned */>` so a repeated `ClientToken` on CreateConnector/CreateTemplate returns the original ARN instead of creating a second resource.
- Insertion order (or a monotonic sequence per record) must be preserved for stable list ordering and `NextToken` paging.

Out of scope for this first slice (they stay 501 and will be filed as individual gap issues): DirectoryRegistration ops, ServicePrincipalName ops, TemplateGroupAccessControlEntry ops, UpdateTemplate, DeleteTemplate, ListTemplates, and the Tag/Untag/ListTagsForResource trio.

## Evidence

OPERATIONS.md (crate root) lists all 25 operations with routes, required inputs and errors: CreateConnector `POST /connectors -> 202` returning `ConnectorArn`; GetConnector `GET /connectors/{ConnectorArn}` returning `Connector` with `ResourceNotFoundException (HTTP 404)`; ListConnectors `GET /connectors` with @paginated(inputToken=NextToken, outputToken=NextToken, items=Connectors) and `MaxResults` range 1..1000; CreateTemplate `POST /templates` returning `TemplateArn` with `ConflictException (HTTP 409)`; GetTemplate `GET /templates/{TemplateArn}` returning `Template`; DeleteConnector `DELETE /connectors/{ConnectorArn} -> 202` @idempotent. Smithy shapes `Connector`, `ConnectorSummary`, `VpcInformation`, `Template`, `TemplateSummary`, `ConnectorStatus` (CREATING|ACTIVE|DELETING|FAILED) give the exact response members and the ARN patterns (template ARN is the connector ARN plus `/template/<uuid>`). `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:'` matches all 25 files in `src/ops/`, confirming the whole service is scaffold-only with zero persisted state.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `pca-connector-ad` client at it.
2. CreateConnector with DirectoryId=d-0123456789, a valid acm-pca CertificateAuthorityArn and VpcInformation{SecurityGroupIds:["sg-0123456789abcdef0"]} — expect 202 and a connector ARN matching ^arn:[^:]+:pca-connector-ad:[^:]+:[0-9]+:connector/<uuid>$ (currently 501).
3. GetConnector with that ARN — expect 200 and a Connector whose DirectoryId, CertificateAuthorityArn and VpcInformation equal what was submitted, with Status=ACTIVE and CreatedAt set.
4. ListConnectors — expect the new connector present exactly once in Connectors; repeat with MaxResults=1 and follow NextToken to page through all connectors with no duplicates or omissions.
5. CreateTemplate against the connector ARN with Name="MyTemplate" and a TemplateDefinition — expect 200 and a template ARN prefixed by the connector ARN; repeat the same Name on the same connector and expect ConflictException (409).
6. GetTemplate with the template ARN — expect 200 and the Definition/Name/ConnectorArn/Revision read back from state.
7. DeleteConnector with the connector ARN — expect 202; then GetConnector returns ResourceNotFoundException (404), ListConnectors no longer lists it, and a second DeleteConnector still succeeds.
8. GetConnector on a well-formed but never-created connector ARN — expect ResourceNotFoundException (404).
