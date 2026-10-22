Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 0761ec9a912f1ad457725ac2bfa14369ca92ad04

# Bootstrap the Route53Profiles emulator

_Filed: 2026-10-21T23:12:57Z_
_Operation: service-com-amazonaws-route53profiles-route53profiles | Class: missing_

> _Note: this issue was identified against the code base at commit `0761ec9a912f1ad457725ac2bfa14369ca92ad04`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 16 operations in `services/service-com-amazonaws-route53profiles-route53profiles/src/ops/` is an unmodified scaffold: each carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` => HTTP 501, and the service holds no state at all. Implement a FIRST, SMALL, stateful slice: the Profile lifecycle plus a VPC association, so a realistic client script runs end to end.

## Happy path to implement (in order, restJson1)

1. **CreateProfile** — `POST /profile`. Input: `Name` (required, len 0..64, pattern `^(?!^[0-9]+$)([a-zA-Z0-9\-_' ']+)$`), `ClientToken` (required, @idempotencyToken, len 1..255), `Tags` (list<Tag>). Must MINT and PERSIST a new Profile and return `Profile` with `Id` (generated, e.g. `rp-<16 hex>`), `Arn` (`arn:aws:route53profiles:<region>:<account>:profile/<Id>`), `Name` (as given), `OwnerId` (12-digit account), `Status` = `COMPLETE` (enum ProfileStatus: COMPLETE|DELETING|UPDATING|CREATING|DELETED|FAILED), `ShareStatus` = `NOT_SHARED`, `CreationTime`/`ModificationTime` (RFC3339), `ClientToken` echoed from input. Repeating the same `ClientToken` must be idempotent: return the SAME stored Profile, not a second one.
2. **GetProfile** — `GET /profile/{ProfileId}`. Must look the Profile up in the store and return exactly the persisted fields (same `Id`/`Arn`/`Name`/`CreationTime` as CreateProfile returned). Unknown id => `ResourceNotFoundException` (HTTP 400 per the model), NOT 501 and NOT an empty 200.
3. **ListProfiles** — `GET /profiles`. Output `ProfileSummaries` (list<ProfileSummary>: `Id`, `Arn`, `Name`, `ShareStatus`) built by iterating the stored profiles; honour `maxResults` (1..100) and return/accept `NextToken` for the remainder (opaque token; a bad token => `InvalidNextTokenException`). A just-created profile MUST appear; a deleted one MUST NOT.
4. **AssociateProfile** — `POST /profileassociation`. Input: `ProfileId` (required), `ResourceId` (required, the VPC id), `Name` (required), `Tags`. Must validate the `ProfileId` exists (`ResourceNotFoundException` otherwise), reject a duplicate (ProfileId, ResourceId) pair with `ConflictException`/`ResourceExistsException`, and persist a ProfileAssociation returning `ProfileAssociation` with generated `Id`, `Name`, `OwnerId`, `ProfileId`, `ResourceId`, `Status` = `COMPLETE`, `CreationTime`/`ModificationTime`. The association must then be readable via **GetProfileAssociation** (`GET /profileassociation/{ProfileAssociationId}`) and listable via **ListProfileAssociations** (`GET /profileassociations`, filterable by `profileId`/`resourceId` query params).
5. **DisassociateProfile** — `DELETE /profileassociation/Profileid/{ProfileId}/resourceid/{ResourceId}`. Must remove the stored association and return the removed `ProfileAssociation` (with `Status` = `DELETING`). Unknown pair => `ResourceNotFoundException`. After this, ListProfileAssociations must no longer return it (idempotent per @idempotent trait).
6. **DeleteProfile** — `DELETE /profile/{ProfileId}`. Must remove the Profile from the store and return the removed `Profile` with `Status` = `DELETING`. A Profile that still has associations must fail with `ConflictException`. After deletion, GetProfile => `ResourceNotFoundException` and ListProfiles omits it.

NO canned, echoed, hardcoded, or random-id-without-storage responses: every field in every response above must be computed from the persisted record created by an earlier call. A create followed by a get/list must return what was created.

## State Model

In `src/state.rs` (`ComAmazonawsRoute53profilesRoute53profilesState`, reached via `crate::state::service_state(&*self.engine)` and its lock guard):

- `profiles: HashMap<String /*ProfileId*/, Profile>` where `Profile { id: String, arn: String, name: String, owner_id: String, status: ProfileStatus, status_message: Option<String>, share_status: ShareStatus, creation_time: OffsetDateTime, modification_time: OffsetDateTime, client_token: String }`
- `client_tokens: HashMap<String /*ClientToken*/, String /*ProfileId*/>` — for CreateProfile idempotency.
- `profile_associations: HashMap<String /*AssociationId*/, ProfileAssociation>` where `ProfileAssociation { id, name, owner_id, profile_id, resource_id, status, status_message, creation_time, modification_time }`, plus an index/scan to resolve the (profile_id, resource_id) pair used by DisassociateProfile and the `profileId`/`resourceId` filters of ListProfileAssociations.
- `tags: HashMap<String /*ResourceArn*/, BTreeMap<String,String>>` — populated by the `Tags` input of CreateProfile/AssociateProfile so the later Tag/Untag/ListTagsForResource slice reads real data.
- `next_token_cursors`: encode list pagination as an opaque token over the stable (sorted-by-id) iteration order of `profiles` / `profile_associations`.

## Out of scope for this slice (stay 501, filed separately)

AssociateResourceToProfile, DisassociateResourceFromProfile, GetProfileResourceAssociation, ListProfileResourceAssociations, UpdateProfileResourceAssociation, TagResource, UntagResource, ListTagsForResource.

## Done when

An integration test drives CreateProfile -> GetProfile -> ListProfiles -> AssociateProfile -> GetProfileAssociation/ListProfileAssociations -> DisassociateProfile -> DeleteProfile -> GetProfile(404-equivalent) and asserts the BODIES, not just 2xx.

## Evidence

`OPERATIONS.md` (crate root) documents all 16 operations with routes, required inputs and modeled errors: CreateProfile `POST /profile` (Name, ClientToken required; output `Profile`), GetProfile `GET /profile/{ProfileId}` (@readonly; errors incl. ResourceNotFoundException), ListProfiles `GET /profiles` (@paginated inputToken=NextToken/outputToken=NextToken/items=ProfileSummaries, MaxResults range 1..100, InvalidNextTokenException), AssociateProfile `POST /profileassociation` (ProfileId/ResourceId/Name required; ConflictException, ResourceExistsException, ResourceNotFoundException), DisassociateProfile `DELETE /profileassociation/Profileid/{ProfileId}/resourceid/{ResourceId}` (@idempotent), DeleteProfile `DELETE /profile/{ProfileId}` (@idempotent; ConflictException). Shape members cited from the Smithy model via smithy_query: `Profile` {Id, Arn, Name, OwnerId, Status, StatusMessage, ShareStatus, CreationTime, ModificationTime, ClientToken}, `ProfileSummary` {Id, Arn, Name, ShareStatus}, `ProfileAssociation` {Id, Name, OwnerId, ProfileId, ResourceId, Status, StatusMessage, CreationTime, ModificationTime}, `ProfileStatus` = [COMPLETE, DELETING, UPDATING, CREATING, DELETED, FAILED], `ShareStatus` = [NOT_SHARED, SHARED_WITH_ME, SHARED_BY_ME]. All-stub status confirmed by grep: all 16 files under `src/ops/` match `^//! SCAFFOLDED, NOT IMPLEMENTED:` at line 4 and return `Scaffolded::not_implemented(..)` (HTTP 501). No tests exist beyond the generated `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI client at it with service `route53profiles` (restJson1, sigv4 name `route53profiles`).
2. `CreateProfile` with Name="test-profile" and ClientToken="tok-1" — expect 200 with a Profile carrying a generated Id/Arn, Status=COMPLETE, ShareStatus=NOT_SHARED and the Name given; currently HTTP 501 NotImplemented.
3. Repeat the identical CreateProfile call with ClientToken="tok-1" — expect the SAME Profile Id back (idempotency), not a new profile.
4. `GetProfile` with that Id — expect the same Id/Arn/Name/CreationTime as the create response.
5. `ListProfiles` — expect ProfileSummaries to contain the new profile; also call with maxResults=1 and follow NextToken.
6. `AssociateProfile` with ProfileId=<Id>, ResourceId="vpc-12345678", Name="assoc-1" — expect 200 with a ProfileAssociation (generated Id, Status=COMPLETE); then `GetProfileAssociation` on that association Id and `ListProfileAssociations?profileId=<Id>` must both return it.
7. Repeat AssociateProfile for the same (ProfileId, ResourceId) — expect ConflictException/ResourceExistsException, not a duplicate.
8. `DeleteProfile` on the profile while the association still exists — expect ConflictException.
9. `DisassociateProfile` for (<Id>, vpc-12345678) — expect the removed ProfileAssociation back; `ListProfileAssociations?profileId=<Id>` is now empty.
10. `DeleteProfile` on the profile — expect the removed Profile back; then `GetProfile` returns ResourceNotFoundException and `ListProfiles` no longer lists it.
