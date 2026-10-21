Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 16fab4a727e8e34e48fe5cd80cb0080f7932bd9f

# Bootstrap the PartnerCentralChannel emulator

_Filed: 2026-10-21T23:18:13Z_
_Operation: service-com-amazonaws-partnercentralchannel-partnercentralchannel | Class: missing_

> _Note: this issue was identified against the code base at commit `16fab4a727e8e34e48fe5cd80cb0080f7932bd9f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 17 operations in `services/service-com-amazonaws-partnercentralchannel-partnercentralchannel/src/ops/` is an unmodified scaffold returning HTTP 501 `NotImplemented` with zero stored state, so no client workflow can run. Implement a FIRST SMALL SLICE: the program-management-account -> relationship lifecycle, which is this service's primary resource chain (a Relationship is modeled as a child of a ProgramManagementAccount: its ARN is `.../program-management-account/pma-xxxxxxxxxxxxx/relationship/rs-xxxxxxxxxxxxx`).

Happy path to implement, in order (protocol: awsJson1_0, `POST /` with `X-Amz-Target: PartnerCentralChannel.<Operation>`):

1. **CreateProgramManagementAccount** — input `catalog` (required, `^[a-zA-Z]*$`), `program` (required, DISTRIBUTION | DISTRIBUTION_SELLER | SOLUTION_PROVIDER), `displayName` (required, 1..30), `accountId` (required, 12 digits), optional `clientToken`, `tags`. Must mint a fresh id matching `^pma-[a-z0-9]{13}$`, build the ARN `arn:aws:partnercentral:<region>:<accountId>:catalog/<catalog>/program-management-account/<id>`, persist the record (program, displayName, accountId, catalog, status, revision="1", createdAt/updatedAt/startDate, tags), and return `programManagementAccountDetail{id, arn}` — the id/arn returned MUST be the one stored, not echoed input.
2. **CreateRelationship** — input `catalog`, `associationType` (DOWNSTREAM_SELLER | END_CUSTOMER | INTERNAL), `programManagementAccountIdentifier` (pma id or ARN), `associatedAccountId` (12 digits), `displayName`, `sector` (COMMERCIAL | GOVERNMENT | GOVERNMENT_EXCEPTION), optional `resaleAccountModel`, `clientToken`, `tags`, `requestedSupportPlan`. Must resolve the parent PMA from stored state (404 `ResourceNotFoundException` when it does not exist), mint `^rs-[a-z0-9]{13}$`, build the child ARN under the PMA, persist the record with revision="1" and timestamps, and return `relationshipDetail{id, arn}`.
3. **GetRelationship** — input `catalog`, `programManagementAccountIdentifier`, `identifier` (rs id or full ARN). Must look the record up in state and return the full `relationshipDetail` — `arn`, `id`, `revision`, `catalog`, `associationType`, `programManagementAccountId`, `associatedAccountId`, `displayName`, `resaleAccountModel`, `sector`, `createdAt`, `updatedAt`, `startDate` — with exactly the values supplied at create time. Unknown identifier => `ResourceNotFoundException` (404).
4. **ListRelationships** — input `catalog`, `maxResults` (default 20), optional `associatedAccountIds`, `associationTypes`, `displayNames`, `programManagementAccountIdentifiers`, `nextToken`. Must return `items` (list of `RelationshipSummary`: arn, id, revision, catalog, associationType, programManagementAccountId, associatedAccountId, displayName, sector, createdAt, updatedAt, startDate) built from stored records, honour the equality filters, truncate at `maxResults`, and emit an opaque `nextToken` that resumes the scan on the next call (absent when the page is the last one).
5. **UpdateRelationship** — input `catalog`, `identifier`, `programManagementAccountIdentifier`, optional `revision`, `displayName`, `requestedSupportPlan`. Must mutate the stored record (new displayName/support plan), bump `revision` and `updatedAt`, and return `updateRelationshipDetail`; when the caller passes a `revision` that is not the current one, fail with `ConflictException` (409). A following GetRelationship must observe the new values.
6. **DeleteRelationship** — input `catalog`, `identifier`, `programManagementAccountIdentifier`, optional `clientToken`. Must remove the record from state and return an empty body; a following GetRelationship must return `ResourceNotFoundException` (404) and the record must be gone from ListRelationships.

All six must compute their responses from persisted, process-wide state shared across requests — no canned bodies, no echoing the request, no minting an id that is never stored. Required-member absence and pattern/length violations must raise `ValidationException` (400); creating a second PMA with the same (catalog, program, accountId) must raise `ConflictException` (409).

## State Model

- `PartnerCentralChannelState` (behind the service's shared lock), containing:
  - `program_management_accounts: Map<String /* pma id */, ProgramManagementAccount>` where `ProgramManagementAccount { id, arn, catalog, program, display_name, account_id, status, revision: u32, created_at, updated_at, start_date, tags: Map<String,String> }`
  - `relationships: Map<String /* rs id */, Relationship>` where `Relationship { id, arn, catalog, program_management_account_id, association_type, associated_account_id, display_name, resale_account_model: Option, sector, requested_support_plan: Option, revision: u32, created_at, updated_at, start_date, tags: Map<String,String> }`
  - `idempotency: Map<String /* clientToken */, String /* created resource id */>` so a replayed `clientToken` on the @idempotent creates returns the first result instead of a duplicate
  - helpers: identifier resolution accepting either a bare id (`pma-…`/`rs-…`) or the full ARN, and an opaque `nextToken` encoding the list offset for the paginated list ops

The remaining 11 operations (channel handshake create/list/accept/cancel/reject, program-management-account update/delete/list, Tag/Untag/ListTagsForResource) stay stubbed at 501 for now and are tracked as separate gap issues.

## Evidence

`services/.../OPERATIONS.md` lists all 17 operations with their input/output members: CreateProgramManagementAccount -> `programManagementAccountDetail`, CreateRelationship -> `relationshipDetail`, GetRelationship (@readonly) -> `RelationshipDetail`, ListRelationships (@paginated inputToken=nextToken, outputToken=nextToken, items=items), UpdateRelationship -> `UpdateRelationshipDetail` with `ConflictException`, DeleteRelationship (empty output). smithy_query on `CreateProgramManagementAccountDetail`, `CreateRelationshipDetail`, `RelationshipDetail`, `RelationshipSummary` gives the exact field sets and the id patterns `^pma-[a-z0-9]{13}$` / `^rs-[a-z0-9]{13}$`. `grep '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches all 17 handler files (create_program_management_account.rs, create_relationship.rs, get_relationship.rs, list_relationships.rs, update_relationship.rs, delete_relationship.rs, …), each returning `crate::scaffolded::Scaffolded::not_implemented(..)` => HTTP 501. No tests exist for these ops.

## Steps to Reproduce

1. Start the emulator and point an awsJson1_0 client at the `partnercentral-channel` endpoint.
2. CreateProgramManagementAccount{catalog:"AWS", program:"SOLUTION_PROVIDER", displayName:"Acme PMA", accountId:"123456789012"} — expect 200 with programManagementAccountDetail.id matching ^pma-[a-z0-9]{13}$ and a matching ARN; currently HTTP 501.
3. CreateRelationship{catalog:"AWS", associationType:"END_CUSTOMER", programManagementAccountIdentifier:<pma id>, associatedAccountId:"210987654321", displayName:"Acme Customer", sector:"COMMERCIAL"} — expect 200 with relationshipDetail.id matching ^rs-[a-z0-9]{13}$.
4. GetRelationship{catalog:"AWS", programManagementAccountIdentifier:<pma id>, identifier:<rs id>} — expect the stored displayName "Acme Customer", associationType END_CUSTOMER, sector COMMERCIAL, revision "1" and the createdAt timestamp.
5. ListRelationships{catalog:"AWS", maxResults:20} — expect items to contain exactly the created relationship summary; repeat with maxResults:1 after creating two relationships and verify nextToken pages through both without duplicates.
6. UpdateRelationship{catalog:"AWS", programManagementAccountIdentifier:<pma id>, identifier:<rs id>, displayName:"Acme Renamed"} then GetRelationship — expect displayName "Acme Renamed" and a bumped revision; passing a stale revision must return ConflictException (409).
7. DeleteRelationship{catalog:"AWS", programManagementAccountIdentifier:<pma id>, identifier:<rs id>} then GetRelationship — expect ResourceNotFoundException (404) and an empty ListRelationships result.
8. GetRelationship with identifier "rs-0000000000000" — expect ResourceNotFoundException (404), not 501 and not a fabricated 200.
