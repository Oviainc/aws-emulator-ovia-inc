Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 34e6e58e29d15417fc6ff05c31f4f08593fa6d9a

# Bootstrap the VerifiedPermissions emulator

_Filed: 2026-10-21T21:18:41Z_
_Operation: service-com-amazonaws-verifiedpermissions-verifiedpermissions | Class: stub_

> _Note: this issue was identified against the code base at commit `34e6e58e29d15417fc6ff05c31f4f08593fa6d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 34 modeled operations in `services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/src/ops/` is an unmodified scaffold: it carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501), and the crate holds no state. Two operations are worse off: `CreatePolicyStore` and `GetPolicyStore` have NO handler file at all (`src/ops/SKIPPED_STUBS.md`: "SKIPPED: CreatePolicyStore — shape missing from model: smithy.api#Unit", same for GetPolicyStore), so they must be hand-authored from a rendered sibling (e.g. `create_policy.rs`) and added to the service's `operations(...)` allow-list.

This issue scopes the FIRST buildable slice only: the policy-store → policy → authorization loop, implemented statefully over awsJson1_0 (`POST /`, `X-Amz-Target: VerifiedPermissions.<OperationName>`). Everything else (identity sources, policy templates, aliases, batch APIs, tagging, update/list variants) stays 501 and is filed separately later.

### Happy path to implement (in order)

1. **CreatePolicyStore** (hand-author; `@idempotent`) — in: `validationSettings` (required, `ValidationSettings{mode: OFF|STRICT}`), `description`, `deletionProtection` (DISABLED|ENABLED), `clientToken`, `tags`. Must mint a fresh `policyStoreId` matching `^[a-zA-Z0-9-/_]*$` (AWS style: `PS`+random, len 1..200), build `arn` of the form `arn:aws:verifiedpermissions::<account>:policy-store/<policyStoreId>`, set `createdDate`/`lastUpdatedDate` (ISO-8601 date-time), and INSERT the store into state. Out: `policyStoreId`, `arn`, `createdDate`, `lastUpdatedDate` (all required).
2. **GetPolicyStore** (hand-author; `@readonly`) — in: `policyStoreId` (required), `tags` (boolean, default false). Must look the store up in state and echo back what CreatePolicyStore persisted: `policyStoreId`, `arn`, `validationSettings`, `createdDate`, `lastUpdatedDate`, plus `description`/`deletionProtection` when they were supplied, and `tags` only when the request asked for them. Unknown id → `ResourceNotFoundException` (HTTP 404).
3. **CreatePolicy** (`@idempotent`) — in: `policyStoreId` (required), `definition` (required union `PolicyDefinition{static: StaticPolicyDefinition{statement (required), description} | templateLinked}`), `name`, `clientToken`. For the first slice support the `static` arm: store the Cedar `statement` verbatim under a newly minted `policyId`, set `policyType = STATIC`, derive `effect` (`Permit`/`Forbid`) and the scope `principal`/`resource`/`actions` from the statement head, and persist against the store. Out must be computed from what was stored: `policyStoreId`, `policyId`, `policyType`, `effect`, `principal`, `resource`, `actions`, `createdDate`, `lastUpdatedDate`. Unknown `policyStoreId` → `ResourceNotFoundException` (404); duplicate `name` within the store → `ConflictException` (409).
4. **GetPolicy** (`@readonly`) — in: `policyStoreId`, `policyId` (both required). Must return the persisted policy, including `definition` as `PolicyDefinitionDetail{static:{statement, description}}` with the EXACT statement text passed to CreatePolicy, plus `policyType`, `effect`, `principal`, `resource`, `actions`, `name`, `createdDate`, `lastUpdatedDate`. Unknown store or policy → `ResourceNotFoundException` (404).
5. **ListPolicies** (`@readonly`, `@paginated(items=policies, inputToken/outputToken=nextToken)`) — in: `policyStoreId` (required), `maxResults` (range 1..), `nextToken`, `filter`. Must enumerate the store's persisted policies as `PolicyItem`s (required members `policyStoreId`, `policyId`, `policyType`, `definition`, `createdDate`, `lastUpdatedDate`), honour `maxResults` by truncating and returning an opaque `nextToken` matching `^[A-Za-z0-9-_=+/\.]*$` that resumes exactly where the page stopped, and omit `nextToken` on the last page. Unknown store → 404.
6. **IsAuthorized** (`@readonly`) — in: `policyStoreId` (required), `principal`/`resource` (`EntityIdentifier{entityType, entityId}`), `action` (`ActionIdentifier{actionType, actionId}`), `context`, `entities`. Must EVALUATE the store's persisted static policies, not return a canned verdict: default-deny (`decision = DENY`, empty `determiningPolicies`) when the store has no matching policy; `ALLOW` when at least one `Permit` policy's scope matches the request triple and no `Forbid` policy matches; `Forbid` wins over `Permit`. `determiningPolicies` must list the `policyId`s that decided the outcome, `errors` must be present (empty list when evaluation succeeded). Unknown store → `ResourceNotFoundException` (404).
7. **DeletePolicyStore** (`@idempotent`) — in: `policyStoreId` (required). Must remove the store and its policies from state so a following GetPolicyStore/GetPolicy/ListPolicies answers `ResourceNotFoundException` (404). A store created with `deletionProtection = ENABLED` must be refused with `InvalidStateException` (HTTP 406), the only error this operation models.

No canned, hardcoded or echoed responses anywhere in this slice: every response field must be read back out of persisted state, and a minted id/ARN that is never stored is a failure. Timestamps use the model's `date-time` timestampFormat. Done = a client script can run steps 1-7 back to back and observe its own writes (the statement it created comes back byte-identical, the authorization decision changes when the policy is created/deleted, and the store disappears after delete).

## State Model

Service state (behind `crate::state::service_state(&*self.engine)`'s lock):

- `policy_stores: BTreeMap<String /*policyStoreId*/, PolicyStore>` — insertion-ordered map so ListPolicyStores/ListPolicies pagination is stable.
- `aliases: BTreeMap<String /*aliasName*/, String /*policyStoreId*/>` — placeholder for the later alias slice; only needs to exist so `policyStoreId` lookups can accept `policy-store-alias/<name>` later.
- `PolicyStore { policy_store_id: String, arn: String, validation_settings: ValidationSettings /*mode: OFF|STRICT*/, description: Option<String>, deletion_protection: DeletionProtection /*default DISABLED*/, created_date: DateTime, last_updated_date: DateTime, tags: BTreeMap<String,String>, schema: Option<Schema>, policies: BTreeMap<String /*policyId*/, Policy>, templates: BTreeMap<String, PolicyTemplate>, identity_sources: BTreeMap<String, IdentitySource>, next_id_seq: u64 }` — `templates`/`identity_sources` stay empty in this slice but reserve the shape.
- `Policy { policy_id: String, policy_store_id: String, policy_type: PolicyType /*STATIC|TEMPLATE_LINKED*/, name: Option<String>, statement: String /*verbatim Cedar text*/, description: Option<String>, effect: Option<PolicyEffect /*Permit|Forbid*/>, principal: Option<EntityIdentifier{entity_type,entity_id}>, resource: Option<EntityIdentifier>, actions: Vec<ActionIdentifier{action_type,action_id}>, created_date: DateTime, last_updated_date: DateTime }`.
- `Schema { schema_json: String, namespaces: Vec<String>, created_date: DateTime, last_updated_date: DateTime }` — written by PutSchema in a later slice; `None` here.
- `client_tokens: BTreeMap<String /*clientToken*/, String /*created resource id*/>` — so an `@idempotencyToken` replay of CreatePolicyStore/CreatePolicy returns the first result instead of creating a second resource, and a replay with different parameters can raise `ConflictException`.

## Evidence

Spec: `services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/smithy-model/com-amazonaws-verifiedpermissions-verifiedpermissions.json`, denormalized in the crate's `OPERATIONS.md` — awsJson1_0, target prefix `VerifiedPermissions.`, 34 operations. Field lists above are taken from OPERATIONS.md (CreatePolicyStore lines 46-51, GetPolicyStore 53-58, DeletePolicyStore 67-72, IsAuthorized 102-107, CreatePolicy 165-170, GetPolicy 172-177, ListPolicies 193-198) and from `smithy_query shape=PolicyDefinition,StaticPolicyDefinition,ValidationSettings,PolicyItem,PolicyDefinitionDetail,PolicyStoreItem`. Implementation: `src/ops/create_policy.rs` (read in full) is an untouched scaffold — `handle` ends in `Err(crate::scaffolded::Scaffolded::not_implemented("CreatePolicy"))` after `let _ = (&input, ctx, &self.engine);`, and the same marker header is present on all 32 rendered op files. `src/ops/SKIPPED_STUBS.md` records CreatePolicyStore and GetPolicyStore as modeled-but-unwired (no handler file, answered by the framework's unroutable-operation path). No state module content and no tests assert any behaviour yet.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI verifiedpermissions client at it.
2. CreatePolicyStore with validationSettings={mode:'OFF'} and description='demo' — expect 200 with a fresh policyStoreId, an arn, createdDate and lastUpdatedDate (today: the request is not even routed, so it fails at the framework's unroutable-operation path).
3. GetPolicyStore with that policyStoreId — expect the same arn/validationSettings/description that were just created (today: unroutable).
4. CreatePolicy with that policyStoreId and definition={static:{statement:'permit(principal == User::\"alice\", action == Action::\"view\", resource == Photo::\"vacation\");'}} — expect 200 with a policyId, policyType='STATIC' and effect='Permit' (today: HTTP 501 NotImplemented).
5. GetPolicy with policyStoreId+policyId — expect definition.static.statement byte-identical to what was sent (today: 501).
6. ListPolicies with policyStoreId and maxResults=1 — expect exactly the created policy and no nextToken (today: 501).
7. IsAuthorized with principal={entityType:'User',entityId:'alice'}, action={actionType:'Action',actionId:'view'}, resource={entityType:'Photo',entityId:'vacation'} — expect decision='ALLOW' with determiningPolicies containing the created policyId; repeat with entityId:'bob' — expect decision='DENY' with empty determiningPolicies (today: 501).
8. DeletePolicy then IsAuthorized again — expect decision='DENY'.
9. DeletePolicyStore, then GetPolicyStore — expect ResourceNotFoundException (HTTP 404). Re-run the whole flow with deletionProtection='ENABLED' and expect DeletePolicyStore to fail with InvalidStateException (HTTP 406).
