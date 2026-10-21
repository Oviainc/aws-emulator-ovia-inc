Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 7b44f3f32d1f58d662874eb8ca0f546e296cee00

# Bootstrap the endusermessaging emulator

_Filed: 2026-10-21T21:08:34Z_
_Operation: service-com-amazonaws-endusermessaging-endusermessaging | Class: missing_

> _Note: this issue was identified against the code base at commit `7b44f3f32d1f58d662874eb8ca0f546e296cee00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

All 27 operations in `services/service-com-amazonaws-endusermessaging-endusermessaging/src/ops/` are untouched scaffolds that answer HTTP 501 `NotImplemented` and hold no state, so no client can do anything with this service. This issue defines the FIRST, SMALL slice: the stateful CRUD lifecycle of the service's primary resource, the **brand profile** (restJson1, sigv4 signing name `end-user-messaging`).

### Happy path to implement (in order)

1. **CreateBrandProfile** — `POST /v1/brand-profiles`. Input: `brandProfileName` (required, 1..64, pattern `^[A-Za-z0-9_ -]*[A-Za-z0-9_-][A-Za-z0-9_ -]*$`), `deletionProtectionEnabled` (bool), `clientToken` (@idempotencyToken), `tags`. Must mint a new `brandProfileId` and `brandProfileArn` (`arn:aws:end-user-messaging:<region>:<account>:brand-profile/<brandProfileId>`), persist the profile, and return all required output members computed from what was stored: `brandProfileId`, `brandProfileArn`, `brandProfileName`, `status` (`ACTIVE`), `deletionProtectionEnabled` (false when the input omitted it), `createdAt`, `updatedAt`, `attributesCreated` (0 for a fresh profile).
2. **GetBrandProfile** — `GET /v1/brand-profiles/{brandProfileId+}`. Must look the profile up in state and return the stored `brandProfileId`/`brandProfileArn`/`brandProfileName`/`status`/`deletionProtectionEnabled`/`createdAt`/`updatedAt`. Unknown id ⇒ `ResourceNotFoundException` (HTTP 404), not 501 and not a synthesized profile.
3. **ListBrandProfiles** — `GET /v1/brand-profiles`. Must return one `BrandProfileInfo` per profile actually in state, honour `maxResults` (1..100) by truncating, and emit an opaque `nextToken` when more remain — a token that, fed back as `nextToken`, resumes after the last returned item. No token ⇒ start at the beginning; a created profile must be visible here immediately and a deleted one must be absent.
4. **UpdateBrandProfile** — `PUT /v1/brand-profiles/{brandProfileId+}`. Must mutate the persisted record: apply `brandProfileName` and/or `deletionProtectionEnabled` when present, leave omitted members unchanged, bump `updatedAt`, and return the post-update record. A subsequent GetBrandProfile must reflect the change. Unknown id ⇒ `ResourceNotFoundException`.
5. **DeleteBrandProfile** — `DELETE /v1/brand-profiles/{brandProfileId+}`. Must remove the profile from state and return `brandProfileId` + `brandProfileArn` of the deleted profile. If `deletionProtectionEnabled` is true the profile must NOT be removed and the call must fail with `ConflictException` (HTTP 409). Unknown id ⇒ `ResourceNotFoundException`. After a successful delete, GetBrandProfile ⇒ 404 and ListBrandProfiles must no longer list it.

Every one of these five must compute its response from the persisted store reached through `crate::state::service_state(&*self.engine)` (see README.md "State"). No canned bodies, no echoing the input back, no minting an id that is never stored — a create followed by a get/list must return exactly what was created. Delete the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker line from each of the five files as it is implemented.

Also required for the slice to be honest: `brandProfileName` length/pattern and `maxResults` range violations must produce `ValidationException` (HTTP 400), and a repeated `clientToken` on CreateBrandProfile must return the first result rather than creating a second profile.

## State Model

In `src/state.rs` (`ComAmazonawsEndusermessagingEndusermessagingState`):

- `brand_profiles: BTreeMap<String /* brandProfileId */, BrandProfile>` — ordered so list pagination is stable.
- `BrandProfile { id: String, arn: String, name: String, status: Status /* ACTIVE */, deletion_protection_enabled: bool, created_at: SystemTime, updated_at: SystemTime, tags: Vec<Tag>, attributes: BTreeMap<String /* attributeName */, BrandProfileAttribute> }` — `attributes` starts empty and backs `attributesCreated`; the attribute operations stay stubbed for now.
- `create_tokens: HashMap<String /* clientToken */, String /* brandProfileId */>` — CreateBrandProfile idempotency.
- `next_id: u64` (or a random-suffix generator) so ids/ARNs match `^[A-Za-z0-9_:/-]+$` and are unique per create.

Out of scope for this first slice (stay 501 and will be filed as individual gap issues): the brand-profile attribute operations, the registration-import/job operations (Create/Update RegistrationsFromBrandProfile, CreateBrandProfileFromRegistration, UpdateBrandProfileFromRegistration, GetJob, ListJobs, ListRegistrationsFromBrandProfile), the notify-code-configuration operations, SendNotifyCodeVerification/ValidateNotifyCodeVerification, and the tagging operations.

## Evidence

OPERATIONS.md (crate root) documents all 27 operations, their routes, members and errors: CreateBrandProfile `POST /v1/brand-profiles` with required outputs brandProfileId/brandProfileArn/brandProfileName/status/deletionProtectionEnabled/createdAt/updatedAt/attributesCreated; GetBrandProfile `GET /v1/brand-profiles/{brandProfileId+}` (@readonly, ResourceNotFoundException 404); ListBrandProfiles `GET /v1/brand-profiles` (@paginated items=brandProfiles, nextToken/maxResults 1..100); UpdateBrandProfile `PUT /v1/brand-profiles/{brandProfileId+}` (@idempotent, ConflictException 409); DeleteBrandProfile `DELETE /v1/brand-profiles/{brandProfileId+}` (@idempotent, returns brandProfileId+brandProfileArn, ConflictException 409 — the modeled failure for deletion protection). `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-endusermessaging-endusermessaging/src/ops/` returns all 27 handler files, including create_brand_profile.rs, get_brand_profile.rs, list_brand_profiles.rs, update_brand_profile.rs and delete_brand_profile.rs, so nothing is implemented and no test asserts any of this. README.md documents the restJson1 wire protocol and the shared-engine state API the handlers must use.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI client for `end-user-messaging` at it.
2. CreateBrandProfile with brandProfileName="demo-brand" and deletionProtectionEnabled=false — expect 200 with a fresh brandProfileId, an arn:aws:end-user-messaging:...:brand-profile/<id>, status=ACTIVE, attributesCreated=0, createdAt/updatedAt set (currently 501).
3. GetBrandProfile with the returned brandProfileId — expect the same name/arn/status/createdAt that create returned.
4. ListBrandProfiles — expect the new profile present; repeat with maxResults=1 against two created profiles and expect exactly one item plus a nextToken that resumes at the second.
5. UpdateBrandProfile with brandProfileName="demo-brand-renamed" and deletionProtectionEnabled=true — expect the new name and a bumped updatedAt; GetBrandProfile must then show the new name and deletionProtectionEnabled=true.
6. DeleteBrandProfile on that profile — expect ConflictException (HTTP 409) because deletion protection is on, and the profile still returned by GetBrandProfile.
7. UpdateBrandProfile with deletionProtectionEnabled=false, then DeleteBrandProfile — expect 200 with brandProfileId+brandProfileArn, then GetBrandProfile ⇒ ResourceNotFoundException (404) and ListBrandProfiles no longer lists it.
8. GetBrandProfile on a never-created id — expect ResourceNotFoundException (404).
