Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 16fab4a727e8e34e48fe5cd80cb0080f7932bd9f

# Bootstrap the opensearchserverless emulator

_Filed: 2026-10-21T23:17:52Z_
_Operation: service-com-amazonaws-opensearchserverless-opensearchserverless | Class: missing_

> _Note: this issue was identified against the code base at commit `16fab4a727e8e34e48fe5cd80cb0080f7932bd9f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 46 operations in this crate is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no OpenSearch Serverless client can get past the first call. Implement a SMALL first slice: the collection lifecycle, which is the service's primary resource, plus the encryption security policy a real collection requires.

Happy path to implement, in order (wire protocol: `awsJson1_0`, `POST /` with `X-Amz-Target: OpenSearchServerless.<OperationName>`, sigv4 name `aoss`):

1. **CreateSecurityPolicy** — input `type` (`encryption`|`network`, required), `name` (required, 3..32, `^[a-z][a-z0-9-]+$`), `policy` (required JSON text), `description`, `clientToken`. Output `securityPolicyDetail` { type, name, policyVersion, description, policy, createdDate, lastModifiedDate }. Must store the policy; a second create with the same (type, name) must fail `ConflictException` (409).
2. **CreateCollection** — input `name` (required, 3..64, `^[a-z][a-z0-9-]+$`), `type` (`SEARCH`|`TIMESERIES`|`VECTORSEARCH`), `description`, `tags`, `standbyReplicas`, `deletionProtection`, `clientToken`. Output `createCollectionDetail` { id, name, status, type, description, arn, kmsKeyArn, standbyReplicas, deletionProtection, createdDate, lastModifiedDate }. Must mint a fresh `id` matching `^[a-z0-9]{3,40}$`, persist the record under that id (and name), set `status` to `CREATING` (then `ACTIVE` on the next read so clients that poll converge), and build `arn` as `arn:aws:aoss:<region>:<account>:collection/<id>`. Duplicate `name` => `ConflictException` (409).
3. **BatchGetCollection** — input `ids` and/or `names` (each 1..100). Output `collectionDetails` (list<CollectionDetail>: id, name, status, type, description, arn, kmsKeyArn, standbyReplicas, deletionProtection, createdDate, lastModifiedDate, collectionEndpoint, dashboardEndpoint) and `collectionErrorDetails` for ids/names that do not exist. Every field must come from the record stored by CreateCollection, not echoed from the request; `collectionEndpoint`/`dashboardEndpoint` must be derived deterministically from the collection id.
4. **ListCollections** — input `collectionFilters` { name, status, collectionGroupName }, `nextToken`, `maxResults`. Output `collectionSummaries` (list<CollectionSummary>: id, name, status, arn, kmsKeyArn) and `nextToken`. Must enumerate the persisted map, apply the filters, honour `maxResults` and return/consume a real `nextToken` (null when the last page is returned).
5. **DeleteCollection** — input `id` (required, `^[a-z0-9]{3,40}$`), `clientToken`. Output `deleteCollectionDetail` { id, name, status, deletionProtection }, with `status` = `DELETING`. Must remove the record so a following BatchGetCollection reports it in `collectionErrorDetails` and ListCollections no longer lists it. Unknown id => `ResourceNotFoundException` (404). A collection with `deletionProtection: ENABLED` => `ConflictException` (409).
6. **DeleteSecurityPolicy** — input `type`, `name` (required), `clientToken`; empty output. Must remove the stored policy; unknown (type, name) => `ResourceNotFoundException` (404).

No canned, hardcoded or echoed responses: every response field must be computed from the persisted record, and a random id returned without storing it counts as a failure. Modeled input constraints (name length/pattern, enum values for `type`, `standbyReplicas`, `deletionProtection`) must produce `ValidationException` (400) rather than being accepted.

## State Model

In `src/state.rs`, on the shared engine state struct:

- `collections: HashMap<String /* collection id */, Collection>` where
  `Collection { id: String, name: String, status: CollectionStatus /* CREATING|ACTIVE|DELETING|FAILED */, kind: Option<CollectionType>, description: Option<String>, arn: String, kms_key_arn: String, standby_replicas: Option<StandbyReplicas>, deletion_protection: DeletionProtection, vector_options: Option<VectorOptions>, collection_group_name: Option<String>, collection_endpoint: String, dashboard_endpoint: String, created_date: i64 /* epoch millis */, last_modified_date: i64, tags: Vec<(String, String)> }`
- `collection_name_index: HashMap<String /* name */, String /* id */>` so name-based BatchGetCollection lookups and duplicate-name conflict checks are O(1).
- `security_policies: HashMap<(SecurityPolicyType, String /* name */), SecurityPolicy>` where
  `SecurityPolicy { kind: SecurityPolicyType, name: String, policy_version: String, description: Option<String>, policy: Document, created_date: i64, last_modified_date: i64 }`; `policy_version` is a generated base64-ish token matching the modeled `^([0-9a-zA-Z+/]{4})*(([0-9a-zA-Z+/]{2}==)|([0-9a-zA-Z+/]{3}=))?$` and must change on every update.
- `idempotency_tokens: HashMap<String /* clientToken */, String /* resource id */>` so a replayed `clientToken` on the `@idempotent` creates returns the original record instead of creating a second one.

Everything else (collection groups, access policies, lifecycle policies, security configs, VPC endpoints, indexes, tagging, account settings) stays scaffolded at 501 and will be filed as individual gap issues later.

## Evidence

`services/service-com-amazonaws-opensearchserverless-opensearchserverless/OPERATIONS.md` lists all 46 operations with their input/output members and errors (CreateCollection / BatchGetCollection / ListCollections / DeleteCollection / CreateSecurityPolicy / DeleteSecurityPolicy quoted above verbatim), and documents the `awsJson1_0` + `X-Amz-Target: OpenSearchServerless.<Op>` wire contract. `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` in that crate matches all 46 handler files (e.g. `src/ops/create_collection.rs`, `src/ops/batch_get_collection.rs`, `src/ops/list_collections.rs`, `src/ops/delete_collection.rs`, `src/ops/create_security_policy.rs`), each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` => HTTP 501. Output shape fields come from the Smithy model shapes `CreateCollectionDetail`, `CollectionDetail`, `CollectionSummary`, `DeleteCollectionDetail`, `SecurityPolicyDetail` and `CollectionFilters`. No test asserts any of this behaviour; the only test is `tests/state_roundtrip.rs`, a state-API example.

## Steps to Reproduce

1. Start the emulator and point an `aoss` client at it (`aws opensearchserverless --endpoint-url <emulator>`).
2. `CreateSecurityPolicy --type encryption --name demo-encryption --policy '{"Rules":[{"ResourceType":"collection","Resource":["collection/demo*"]}],"AWSOwnedKey":true}'` — expect 200 with `securityPolicyDetail.name == demo-encryption` and a non-empty `policyVersion`; today it returns HTTP 501 NotImplemented.
3. `CreateCollection --name demo-collection --type SEARCH --description 'bootstrap demo'` — expect 200 with a generated `createCollectionDetail.id` matching `^[a-z0-9]{3,40}$`, `status == CREATING`, and an `arn` ending in `collection/<id>`.
4. `BatchGetCollection --ids <id>` — expect `collectionDetails[0]` to carry the name, type and description that were created plus a derived `collectionEndpoint`/`dashboardEndpoint`, and empty `collectionErrorDetails`.
5. `ListCollections` — expect `collectionSummaries` to contain exactly the created collection; repeat with `--max-results 1` on two collections and verify a usable `nextToken` round-trips to the second page.
6. `CreateCollection --name demo-collection` again — expect `ConflictException` (HTTP 409).
7. `DeleteCollection --id <id>` — expect `deleteCollectionDetail.status == DELETING`; then `BatchGetCollection --ids <id>` must report it in `collectionErrorDetails` and `ListCollections` must no longer list it.
8. `DeleteCollection --id <id>` a second time — expect `ResourceNotFoundException` (HTTP 404).
9. `DeleteSecurityPolicy --type encryption --name demo-encryption` — expect 200, then the same call again must return `ResourceNotFoundException` (HTTP 404).
