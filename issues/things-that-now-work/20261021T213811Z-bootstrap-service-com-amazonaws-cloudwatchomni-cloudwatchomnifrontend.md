Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 09acb2538adf9fea9bce04c8d04105f42be236df

# Bootstrap the CloudWatchOmni emulator

_Filed: 2026-10-21T21:38:11Z_
_Operation: service-com-amazonaws-cloudwatchomni-cloudwatchomnifrontend | Class: missing_

> _Note: this issue was identified against the code base at commit `09acb2538adf9fea9bce04c8d04105f42be236df`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 61 operations in `services/service-com-amazonaws-cloudwatchomni-cloudwatchomnifrontend/src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no client can do anything with this emulator. This issue defines the FIRST buildable slice: the domain -> space lifecycle, which is CloudWatchOmni's primary resource hierarchy (a Space must belong to a Domain, and every other resource in the service - access profiles, access grants, alerts, views, dashboards, integrations, telemetry queries - is keyed by `spaceId`, so nothing else can be built until this works).

### Happy path to implement (in order)

1. **CreateDomain** - input `name` (required, len 3..63, `^[a-z0-9]+(-[a-z0-9]+)*$`), `identityProviders` (required, list of IdentityProvider enum `IAM|IDC`, 1..2 unique items), optional `identityProviderConfiguration`, `tags`, `clientToken` (@idempotencyToken). Output `domain` (Domain, required). Must mint a fresh `domainId` matching `^d-[0-9a-z]{1,25}$`, build `domainArn` in the `cloudwatch` ARN namespace, set `domainEndpointUrl`, `region`, `createdAt`/`updatedAt`, `status` (DomainStatus), persist the whole record, and return it.
2. **GetDomain** - input `domainId` (required). Output `domain` (Domain). Must return the EXACT record persisted by CreateDomain (same id/arn/name/identityProviders/createdAt), and `ResourceNotFoundException` (404) for an unknown or already-deleted `domainId`.
3. **CreateSpace** - input `name` (required, len 3..64, `^[a-z0-9]+(-[a-z0-9]+)*$`), `domainId` (required), `dataAccessRoleArn` (required, `^arn:.+$`), optional `agentCoreEvaluationRoleArn`, `encryptionConfiguration`, `tags`, `clientToken`. Output `space` (Space, required). Must look the parent domain up in state and fail with `ResourceNotFoundException` (404) when `domainId` is unknown; on success mint a UUID-shaped `spaceId` (`^[0-9a-f]{8}-[0-9a-f]{4}-...$`), set `spaceArn`, `domainArn` (copied from the parent domain), `region`, `ownerAccountId` (12 digits), `dataAccessRoleArn`, `createdAt`/`updatedAt`, `status` (SpaceStatus), and `encryptionConfiguration` - which per the model is ALWAYS populated: a space created without a customer key must report `encryptionStrategy: AWS_OWNED` and no `kmsKeyArn`.
4. **GetSpace** - input `spaceId` (required). Output `space` (Space). Must return the persisted space verbatim (including the derived `domainArn` and `encryptionConfiguration`), `ResourceNotFoundException` (404) otherwise.
5. **ListSpaces** - input optional `domainId`, `nextToken` (len 1..2048), `maxResults` (1..100). Output `items` (list<SpaceSummary>, required) + `nextToken`. Must enumerate spaces from state, filter by `domainId` when supplied, project each stored space into SpaceSummary (`spaceId`, `name`, `spaceArn`, `domainArn`, `region`, `ownerAccountId`, `status`, `statusReason`, `createdAt`, `updatedAt`), honour `maxResults` as a real page bound and return an opaque `nextToken` that resumes exactly where the previous page stopped (per @paginated inputToken=nextToken/outputToken=nextToken/items=items). An empty store must return `items: []`, never an error.
6. **DeleteSpace** - input `spaceId` (required), empty output, @idempotent. Must remove the space from state so a subsequent GetSpace returns 404 and ListSpaces no longer includes it. (DeleteDomain, same store, should refuse with `ConflictException` (409) while the domain still has spaces - implement it alongside if cheap, otherwise it stays stubbed.)

### Requirements

- NEVER STUBS: each of these six operations must compute its response from persisted state. No hardcoded Domain/Space structs, no echoing the request back without storing it, no minting an id that is never recorded. A create followed by a get/list must return what was created.
- Input validation must be real: required members missing, or `name`/`domainId`/`spaceId`/ARN pattern violations, or `maxResults` outside 1..100, produce `ValidationException` (400). Unknown parent/target ids produce `ResourceNotFoundException` (404). Duplicate domain `name` produces `ConflictException` (409).
- `clientToken` (@idempotencyToken) on CreateDomain/CreateSpace must be honoured: a replay with the same token returns the previously created resource instead of creating a second one.
- Timestamps must be set once at create and only bumped by updates; `createdAt` must stay stable across repeated reads.

## State Model

A single service store (behind the usual lock) holding:

- `domains: Map<DomainId, Domain>` where `Domain { domain_id: String /* d-xxxxx */, domain_arn: String, name: String, identity_providers: Vec<IdentityProvider /* IAM|IDC */>, identity_provider_configuration: Option<IdentityProviderConfiguration>, domain_endpoint_url: String, custom_endpoint_urls: Vec<String>, identity_center_application_arn: Option<String>, region: String, created_at: Timestamp, updated_at: Timestamp, status: DomainStatus, tags: Map<String,String> }`
- `spaces: Map<SpaceId, Space>` where `Space { space_id: Uuid, name: String, space_arn: String, domain_id: DomainId, domain_arn: Option<String>, region: String, owner_account_id: String, data_access_role_arn: String, agent_core_evaluation_role_arn: Option<String>, encryption_configuration: EncryptionConfiguration { encryption_strategy, kms_key_arn: Option<String> }, status: SpaceStatus, status_reason: Option<String>, created_at: Timestamp, updated_at: Timestamp, tags: Map<String,String> }`
- `domain_name_index: Map<String /* name */, DomainId>` for the 409 duplicate-name check.
- `idempotency: Map<ClientToken, ResourceId>` so CreateDomain/CreateSpace replays are idempotent.
- Insertion order preserved (or a stable sort key) so ListSpaces pagination tokens are deterministic across pages.

Done = a client script can run CreateDomain -> GetDomain -> CreateSpace -> GetSpace -> ListSpaces -> DeleteSpace end to end, with every response body derived from the store and the deleted space verifiably gone. The other 55 operations keep returning 501 and are tracked as individual gap issues.

## Evidence

Spec: `services/service-com-amazonaws-cloudwatchomni-cloudwatchomnifrontend/smithy-model/com-amazonaws-cloudwatchomni-cloudwatchomnifrontend.json` (SDK id `CloudWatchOmni`, sigv4 signing name `cloudwatch`, endpoint prefix `cloudwatch-omni`). Field lists and errors come from the crate's `OPERATIONS.md` (61 operations) plus smithy_query on CreateDomain/GetDomain/CreateSpace/GetSpace/ListSpaces/DeleteSpace and the Domain/Space/SpaceSummary/DomainSummary/IdentityProvider shapes: CreateDomain -> `domain` (Domain, required); GetDomain(`domainId`) -> `domain` with ResourceNotFoundException 404; CreateSpace(`name`,`domainId`,`dataAccessRoleArn`) -> `space` (Space, required) with ResourceNotFoundException 404 + ConflictException 409; GetSpace(`spaceId`) -> `space`; ListSpaces is @paginated(inputToken=nextToken, outputToken=nextToken, pageSize=maxResults, items=items) with `maxResults` range 1..100; DeleteSpace(`spaceId`) is @idempotent with empty output and ConflictException 409. The Space model doc states `encryptionConfiguration` is "Always populated: a space with no customer managed key reports `encryptionStrategy` AWS_OWNED and no `kmsKeyArn`".

Implementation: `grep -rn 'SCAFFOLDED, NOT IMPLEMENTED' services/service-com-amazonaws-cloudwatchomni-cloudwatchomnifrontend/src/ops/` matches all 61 handler files, including `create_domain.rs`, `get_domain.rs`, `create_space.rs`, `get_space.rs`, `list_spaces.rs` and `delete_space.rs`; each returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` -> HTTP 501. There is no state module and no test coverage for any of these operations.

## Steps to Reproduce

1. Start the emulator and point an AWS client at the `cloudwatch-omni` endpoint.
2. Call CreateDomain with name="my-omni-domain", identityProviders=["IAM"] — expected: 200 with `domain.domainId` matching ^d-[0-9a-z]{1,25}$, a domainArn, domainEndpointUrl, region, createdAt/updatedAt and status; actual: HTTP 501 NotImplemented.
3. Call GetDomain with the returned domainId — expected: 200 returning the identical Domain record just created; actual: 501.
4. Call CreateSpace with name="my-space", domainId=<the created domainId>, dataAccessRoleArn="arn:aws:iam::123456789012:role/OmniData" — expected: 200 with a UUID-shaped spaceId, spaceArn, the parent's domainArn, ownerAccountId, status and encryptionConfiguration{encryptionStrategy=AWS_OWNED}; actual: 501.
5. Call CreateSpace with domainId="d-doesnotexist" — expected: ResourceNotFoundException (404); actual: 501.
6. Call GetSpace with the returned spaceId — expected: 200 with the same Space body as CreateSpace returned; actual: 501.
7. Call ListSpaces with domainId=<the created domainId> and maxResults=1 — expected: 200 with items containing exactly that space's SpaceSummary, and a nextToken only when more spaces remain; actual: 501.
8. Call DeleteSpace with the spaceId, then GetSpace on it again — expected: 200/empty then ResourceNotFoundException (404), and ListSpaces no longer lists it; actual: 501 for every call and no state is retained anywhere.
