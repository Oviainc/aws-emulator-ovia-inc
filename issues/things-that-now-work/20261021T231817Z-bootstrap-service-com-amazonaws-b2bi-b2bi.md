Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 16fab4a727e8e34e48fe5cd80cb0080f7932bd9f

# Bootstrap the b2bi emulator

_Filed: 2026-10-21T23:18:17Z_
_Operation: service-com-amazonaws-b2bi-b2bi | Class: missing_

> _Note: this issue was identified against the code base at commit `16fab4a727e8e34e48fe5cd80cb0080f7932bd9f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

All 30 operations of `service-com-amazonaws-b2bi-b2bi` are untouched scaffolds that answer HTTP 501 `NotImplemented` and hold no state. Implement a FIRST, SMALL stateful slice: the AWS B2B Data Interchange trading-partner onboarding loop, built around the primary resources Profile and Partnership (with Capability as the thing a partnership references).

### Happy path to implement (in order)

1. **CreateProfile** — input `name` (required), `phone` (required), `businessName` (required), `logging` (required, DISABLED|ENABLED), optional `email`, `clientToken`, `tags`. Must mint a new `profileId` (1..64, `^[a-zA-Z0-9_-]+$`), build `profileArn` (`arn:aws:b2bi:<region>:<account>:profile/<profileId>`), set `createdAt` to now, derive `logGroupName` only when `logging == ENABLED`, store the record, and echo back the STORED values. HTTP 201.
2. **CreateCapability** — input `name` (required), `type` (required, `edi`), `configuration` (CapabilityConfiguration, required), optional `instructionsDocuments` (0..5 S3Location), `clientToken`, `tags`. Mint `capabilityId`, `capabilityArn`, `createdAt`; persist the full `configuration` and `instructionsDocuments` verbatim so GetCapability returns them. HTTP 201.
3. **CreatePartnership** — input `profileId` (required), `name` (required), `email` (required, must match `^[\w\.\-]+@[\w\.\-]+$`), `capabilities` (required list of capability ids), optional `phone`, `capabilityOptions`, `clientToken`, `tags`. Must look the `profileId` up in the profile store and return `ResourceNotFoundException` (404) when absent — not a success. Mint `partnershipId`, `partnershipArn`, a `tradingPartnerId`, `createdAt`; persist `capabilities` and `capabilityOptions`. HTTP 201.
4. **GetPartnership** — input `partnershipId` only. Must return the persisted `profileId`, `partnershipArn`, `name`, `email`, `phone`, `capabilities`, `capabilityOptions`, `tradingPartnerId`, `createdAt`, and `modifiedAt` (set only after an update) exactly as created. Unknown id -> `ResourceNotFoundException` (404).
5. **ListPartnerships** — input optional `profileId`, `nextToken`, `maxResults` (1..100). Must enumerate the partnership store as `partnerships` (list<PartnershipSummary>), filter by `profileId` when given, honour `maxResults` as a real page size, and return an opaque `nextToken` that resumes the enumeration (absent on the last page). Also implement **GetProfile** / **ListProfiles** symmetry for the profile store so the created profile is readable and listable.
6. **DeleteProfile** / **DeletePartnership** — remove the record; a subsequent GetProfile/GetPartnership for that id must return `ResourceNotFoundException` (404), and ListProfiles/ListPartnerships must no longer include it. HTTP 200 with an empty body.

Every one of these must compute its response from the persisted store reachable via `crate::state::service_state(&*self.engine)` — no canned bodies, no echoing the request back without storing it, no minting an id that is never written to state. A create followed by a get must return what was created.

### State Model

Add to `ComAmazonawsB2biB2biState` (in `src/state.rs`), all under the single shared engine:

- `profiles: HashMap<String /*profileId*/, Profile>` where `Profile { profile_id, profile_arn, name, email: Option<String>, phone, business_name, logging, log_group_name: Option<String>, created_at, modified_at: Option<Timestamp>, tags: Vec<Tag> }`
- `capabilities: HashMap<String /*capabilityId*/, Capability>` where `Capability { capability_id, capability_arn, name, r#type, configuration: CapabilityConfiguration, instructions_documents: Vec<S3Location>, created_at, modified_at: Option<Timestamp>, tags: Vec<Tag> }`
- `partnerships: HashMap<String /*partnershipId*/, Partnership>` where `Partnership { partnership_id, partnership_arn, profile_id, name, email, phone: Option<String>, capabilities: Vec<String>, capability_options: Option<CapabilityOptions>, trading_partner_id, created_at, modified_at: Option<Timestamp>, tags: Vec<Tag> }`
- `idempotency: HashMap<String /*clientToken*/, String /*resource id*/>` so a repeated create with the same `clientToken` returns the SAME resource instead of a duplicate (all three creates carry `@idempotencyToken` and declare `ConflictException`).
- Insertion order for the list operations: keep a `Vec<String>` of ids per collection (or an ordered map) so `nextToken` paging is stable across calls.

Out of scope for this first slice (stays 501, filed separately later): the transformer resource (CreateTransformer/GetTransformer/UpdateTransformer/DeleteTransformer/ListTransformers), StartTransformerJob/GetTransformerJob, the mapping/test surface (GenerateMapping, CreateStarterMappingTemplate, TestMapping, TestParsing, TestConversion), the Update* operations, and TagResource/UntagResource/ListTagsForResource.

## Evidence

`services/service-com-amazonaws-b2bi-b2bi/OPERATIONS.md` lists 30 operations with these exact input/output members and errors: CreateProfile (POST /profiles -> 201, required name/phone/businessName/logging, output profileId/profileArn/createdAt/logGroupName), GetProfile (GET /profiles/{profileId}), ListProfiles (@paginated nextToken/maxResults, items=profiles), CreateCapability (POST /capabilities -> 201, required name/type/configuration), CreatePartnership (POST /partnerships -> 201, required profileId/name/email/capabilities, output partnershipId/partnershipArn/tradingPartnerId/createdAt, declares ResourceNotFoundException 404), GetPartnership (GET /partnerships/{partnershipId}), ListPartnerships (@paginated, optional profileId filter), DeleteProfile/DeletePartnership (DELETE -> 200, no output members, ResourceNotFoundException 404). Protocol per OPERATIONS.md: `awsJson1_0`, SDK id `b2bi`.

Every handler under `services/service-com-amazonaws-b2bi-b2bi/src/ops/` carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker (grep returns all 30 files, e.g. `create_profile.rs:4`, `create_partnership.rs:4`, `get_partnership.rs:4`, `list_partnerships.rs:4`), so the service holds no state and every call answers 501. No tests exist beyond the generated `tests/state_roundtrip.rs` example.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK b2bi client at it.
2. CreateProfile(name='Acme', phone='555-0100', businessName='Acme Corp', logging='ENABLED') -> expect 201 with a fresh profileId, profileArn, createdAt, and a logGroupName; today returns HTTP 501 NotImplemented.
3. GetProfile(profileId=<from step 2>) -> expect the same name/phone/businessName/logging/createdAt that were stored.
4. ListProfiles(maxResults=1) -> expect the new profile in `profiles`, page size respected, and a usable nextToken when more exist.
5. CreateCapability(name='edi-850', type='edi', configuration={...}) -> expect 201 with capabilityId/capabilityArn/createdAt.
6. CreatePartnership(profileId=<step 2>, name='Partner One', email='partner@example.com', capabilities=[<step 5 capabilityId>]) -> expect 201 with partnershipId/partnershipArn/tradingPartnerId.
7. CreatePartnership with profileId='doesnotexist1' -> expect ResourceNotFoundException (HTTP 404), not a success.
8. GetPartnership(partnershipId=<step 6>) -> expect the stored profileId, email, and capabilities list returned verbatim.
9. ListPartnerships(profileId=<step 2>) -> expect exactly the partnership created in step 6.
10. DeletePartnership(partnershipId=<step 6>) then GetPartnership(same id) -> expect 200 then ResourceNotFoundException (404).
11. DeleteProfile(profileId=<step 2>) then GetProfile(same id) and ListProfiles -> expect 404 and the profile absent from the listing.
