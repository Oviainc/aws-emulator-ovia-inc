Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: e7f0ec181ceb81d0f84ceec5a3401ac3ad90708e

# Bootstrap the rolesanywhere emulator

_Filed: 2026-10-21T22:20:58Z_
_Operation: service-com-amazonaws-rolesanywhere-rolesanywhere | Class: stub_

> _Note: this issue was identified against the code base at commit `e7f0ec181ceb81d0f84ceec5a3401ac3ad90708e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 30 operations in this crate is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no IAM Roles Anywhere client can do anything against the emulator. Implement a FIRST, SMALL, stateful slice: the trust-anchor + profile lifecycle, which is the core workflow of this service (you create a trust anchor from a certificate bundle, create a profile naming the roles that may be assumed, read them back, list them, then delete).

Happy path to implement, in order (restJson1 wire protocol, sigv4 signing name `rolesanywhere`):

1. **CreateTrustAnchor** — `POST /trustanchors` -> 201. Input: `name` (required, 1..255, `^[ a-zA-Z0-9-_]*$`), `source` (required, `Source{sourceType: CERTIFICATE_BUNDLE|AWS_ACM_PCA|SELF_SIGNED_REPOSITORY, sourceData: union{x509CertificateData|acmPcaArn}}`), `enabled` (optional, default true), `tags`, `notificationSettings`. Output: `trustAnchor` (`TrustAnchorDetail`, required) with a freshly generated `trustAnchorId` matching `[a-f0-9]{8}-([a-z0-9]{4}-){3}[a-z0-9]{12}`, `trustAnchorArn` = `arn:aws:rolesanywhere:<region>:<account>:trust-anchor/<trustAnchorId>`, the stored `name`/`source`/`enabled`, and `createdAt`/`updatedAt` ISO-8601 timestamps. The record must be inserted into service state.
2. **GetTrustAnchor** — `GET /trustanchor/{trustAnchorId}` (@readonly). Must look the id up in state and return the SAME `TrustAnchorDetail` that CreateTrustAnchor returned (id, arn, name, source, enabled, timestamps). Unknown or already-deleted id -> `ResourceNotFoundException` (HTTP 404), never a canned success.
3. **CreateProfile** — `POST /profiles` -> 201. Input: `name` (required), `roleArns` (required, 0..250), optional `durationSeconds` (default 3600 when absent), `enabled` (default true), `sessionPolicy`, `managedPolicyArns`, `requireInstanceProperties`, `acceptRoleSessionName`, `tags`. Output: `profile` (`ProfileDetail`) with generated `profileId` (same uuid pattern), `profileArn` matching `^arn:aws(-[^:]+)?:rolesanywhere(:.*){2}(:profile.*)$`, the stored `roleArns`/`durationSeconds`/`enabled`/`sessionPolicy`, `createdBy` (the caller account), and `createdAt`/`updatedAt`. Stored in state.
4. **GetProfile** — `GET /profile/{profileId}` (@readonly). Returns the persisted `ProfileDetail` byte-for-byte equal to what CreateProfile returned (notably `roleArns` and `durationSeconds` must survive the round trip, not be dropped or defaulted). Unknown id -> `ResourceNotFoundException` (404).
5. **ListProfiles** — `GET /profiles` (@paginated inputToken=nextToken, outputToken=nextToken, items=profiles). Must enumerate the profiles actually in state, honour `pageSize` as a hard page bound and return an opaque `nextToken` when more remain, resuming exactly where the previous page ended (no duplicates, no skips). An empty store returns an empty `profiles` list and no `nextToken`. Implement **ListTrustAnchors** (`GET /trustanchors`, items=trustAnchors) the same way if cheap — it shares the code path.
6. **DeleteProfile** — `DELETE /profile/{profileId}` (@idempotent). Removes the profile from state and returns the deleted `ProfileDetail` in `profile`. A subsequent GetProfile/ListProfiles must show it gone (404 / absent from the list). Mirror this for **DeleteTrustAnchor** (`DELETE /trustanchor/{trustAnchorId}`) so the workflow can clean up and so delete-then-get returns 404.

Every one of these must compute its response from persisted state. No canned bodies, no echoing the request back, and no generating an id that is never stored — a create followed by a get/list must return exactly what was created, and a delete followed by a get must 404. Shared requirements for the slice: ids generated to the modeled uuid pattern, ARNs built from the request's region/account, `createdAt`/`updatedAt` maintained (updatedAt bumped on any mutation), and `@length`/`@pattern` violations on `name`, `roleArns`, and the uuid path labels rejected with `ValidationException` (HTTP 400) rather than silently accepted.

## State Model

In `src/state.rs` (`ComAmazonawsRolesanywhereRolesanywhereState`, reached via `crate::state::service_state(&*self.engine)` and mutated under its lock):

- `trust_anchors: BTreeMap<String /* trustAnchorId */, TrustAnchor>` where
  `TrustAnchor { trust_anchor_id: String, trust_anchor_arn: String, name: String, source: Source /* source_type + source_data union */, enabled: bool, created_at: DateTime, updated_at: DateTime, notification_settings: Vec<NotificationSettingDetail>, tags: Vec<Tag> }`
- `profiles: BTreeMap<String /* profileId */, Profile>` where
  `Profile { profile_id: String, profile_arn: String, name: String, role_arns: Vec<String>, managed_policy_arns: Vec<String>, session_policy: Option<String>, duration_seconds: i32 /* default 3600 */, enabled: bool, require_instance_properties: bool, accept_role_session_name: bool, created_by: String /* account id */, created_at: DateTime, updated_at: DateTime, attribute_mappings: Vec<AttributeMapping>, tags: Vec<Tag> }`
- (reserved for the next slice, create the maps now if convenient) `crls: BTreeMap<String /* crlId */, Crl { crl_id, crl_arn, name, crl_data: Vec<u8>, trust_anchor_arn, enabled, created_at, updated_at }>`
- `list_tokens`: pagination cursors for the `@paginated` list ops — an opaque token encoding the last-returned key, so ListProfiles/ListTrustAnchors can resume deterministically (BTreeMap key order gives a stable sort).

Operations outside this slice (ImportCrl/GetCrl/UpdateCrl/DeleteCrl/ListCrls/Enable/DisableCrl, Enable/DisableProfile, Enable/DisableTrustAnchor, UpdateProfile, UpdateTrustAnchor, Put/DeleteAttributeMapping, Put/ResetNotificationSettings, GetSubject, ListSubjects, Tag/Untag/ListTagsForResource) correctly remain 501 and will be filed as individual gap issues later.

## Evidence

`services/service-com-amazonaws-rolesanywhere-rolesanywhere/OPERATIONS.md` lists all 30 modeled operations with routes, input/output members and errors (CreateTrustAnchor `POST /trustanchors -> 201` with required `name`+`source` and required `trustAnchor` output; GetTrustAnchor `GET /trustanchor/{trustAnchorId}` @readonly with ResourceNotFoundException 404; CreateProfile `POST /profiles -> 201` with required `name`+`roleArns`; GetProfile `GET /profile/{profileId}`; ListProfiles `GET /profiles` @paginated(inputToken=nextToken, outputToken=nextToken, items=profiles) with `pageSize`; DeleteProfile `DELETE /profile/{profileId}` @idempotent returning `profile`). Field lists for `TrustAnchorDetail`, `ProfileDetail`, `Source`/`SourceData` taken from the Smithy model via smithy_query (ProfileDetail documents `durationSeconds` default 3600). `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-rolesanywhere-rolesanywhere/src/ops/` matches all 30 handler files, including `create_trust_anchor.rs`, `get_trust_anchor.rs`, `create_profile.rs`, `get_profile.rs`, `list_profiles.rs`, `delete_profile.rs` — so every operation currently answers HTTP 501 and no state is kept. No tests exist beyond the generated `tests/state_roundtrip.rs` example.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `rolesanywhere` client at it.
2. `aws rolesanywhere create-trust-anchor --name demo-anchor --source 'sourceType=CERTIFICATE_BUNDLE,sourceData={x509CertificateData=<PEM>}' --enabled` — expect 201 with trustAnchor.trustAnchorId (uuid), trustAnchorArn, name=demo-anchor, enabled=true, createdAt/updatedAt. Today: HTTP 501 NotImplemented.
3. `aws rolesanywhere get-trust-anchor --trust-anchor-id <id from step 2>` — expect 200 returning the identical TrustAnchorDetail (same id/arn/name/source/enabled).
4. `aws rolesanywhere create-profile --name demo-profile --role-arns arn:aws:iam::111122223333:role/DemoRole --duration-seconds 900` — expect 201 with profileId, profileArn, roleArns echoed from state, durationSeconds=900, enabled=true, createdAt.
5. `aws rolesanywhere get-profile --profile-id <id from step 4>` — expect 200 with roleArns and durationSeconds=900 preserved.
6. `aws rolesanywhere list-profiles --page-size 1` — expect exactly 1 profile plus a nextToken when more exist; paging with that token returns the remaining profiles with no repeats.
7. `aws rolesanywhere delete-profile --profile-id <id>` — expect 200 returning the deleted ProfileDetail; then `get-profile` on that id must return ResourceNotFoundException (HTTP 404) and `list-profiles` must no longer include it.
8. `aws rolesanywhere delete-trust-anchor --trust-anchor-id <id>` then `get-trust-anchor` on it — expect ResourceNotFoundException (HTTP 404).
9. `aws rolesanywhere get-profile --profile-id not-a-uuid` — expect ValidationException (HTTP 400), not a 501 or a success.
