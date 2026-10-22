Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 34e6e58e29d15417fc6ff05c31f4f08593fa6d9a

# Bootstrap the cleanrooms emulator

_Filed: 2026-10-21T21:19:20Z_
_Operation: service-com-amazonaws-cleanrooms-awsbastioncontrolplaneservicelambda | Class: missing_

> _Note: this issue was identified against the code base at commit `34e6e58e29d15417fc6ff05c31f4f08593fa6d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 103 operations in this crate is an unmodified scaffold returning HTTP 501 `NotImplemented` and the service holds no state, so no client can do anything at all. Implement a FIRST, SMALL slice: the collaboration/membership lifecycle, which is the entry point for every other Clean Rooms resource (configured tables, analysis templates, protected queries all hang off a membership).

### Happy path to implement (in order)

1. **CreateCollaboration** — `POST /collaborations` -> 200. Inputs: `name` (required, 1..100), `description` (default ""), `creatorDisplayName` (required), `creatorMemberAbilities` (required list<MemberAbility>), `members` (required list<MemberSpecification>), `queryLogStatus` (required, DISABLED|ENABLED), optional `jobLogStatus`, `analyticsEngine`, `tags`, `creatorPaymentConfiguration`, `dataEncryptionMetadata`, `isMetricsEnabled`. Must mint a fresh lowercase-hex UUID `id` matching `^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`, an `arn` of the form `arn:aws:cleanrooms:<region>:<account>:collaboration/<id>`, set `createTime`/`updateTime`, `creatorAccountId`, `memberStatus=ACTIVE`, persist the record, and return it as `collaboration` (Collaboration). Creating the collaboration must also create the creator's own membership (so `membershipId`/`membershipArn` on the returned Collaboration are populated and GetMembership works on it), which is the real service's behavior.
2. **GetCollaboration** — `GET /collaborations/{collaborationIdentifier}` -> 200. Must look the stored record up by id and return the SAME `collaboration` that CreateCollaboration returned, field for field (id, arn, name, description, creatorAccountId, creatorDisplayName, createTime, updateTime, memberStatus, queryLogStatus, ...). Unknown id must be a modeled error, not a 200 with a fabricated body — the model declares only `ValidationException (400)`, `AccessDeniedException (403)`, `ThrottlingException (429)`, `InternalServerException (500)` on this operation, so use `ValidationException`.
3. **ListCollaborations** — `GET /collaborations` -> 200. Must return one `CollaborationSummary` per persisted collaboration (id, arn, name, creatorAccountId, creatorDisplayName, createTime, updateTime, memberStatus, membershipId/membershipArn, analyticsEngine) — never a hardcoded or empty list. Honor `maxResults` (1..100) by truncating and emitting an opaque `nextToken` that the next call resumes from, and filter on `memberStatus` (ACTIVE|INVITED) when supplied.
4. **CreateMembership** — `POST /memberships` -> 200. Inputs: `collaborationIdentifier` (required), `queryLogStatus` (required), optional `jobLogStatus`, `tags`, `defaultResultConfiguration`, `defaultJobResultConfiguration`, `paymentConfiguration`, `isMetricsEnabled`. Must reject an unknown `collaborationIdentifier` with `ResourceNotFoundException (404)` (declared) and reject a duplicate membership for the same (collaboration, caller account) with `ConflictException (409)` (declared). On success mint a UUID `id` + `arn` `arn:aws:cleanrooms:<region>:<account>:membership/<id>`, copy `collaborationId`, `collaborationArn`, `collaborationName`, `collaborationCreatorAccountId`, `collaborationCreatorDisplayName` FROM the stored collaboration (not from the request), set `status=ACTIVE`, `memberAbilities` from the collaboration's member specification, `createTime`/`updateTime`, persist, and return `membership` (Membership).
5. **GetMembership** — `GET /memberships/{membershipIdentifier}` -> 200. Returns the persisted Membership by id; unknown id -> `ResourceNotFoundException (404)` (declared).
6. **ListMemberships** — `GET /memberships` -> 200. Returns a `MembershipSummary` per persisted membership, honoring `maxResults`/`nextToken` and the `status` filter (ACTIVE|COLLABORATION_DELETED|REMOVED).
7. **DeleteCollaboration** — `DELETE /collaborations/{collaborationIdentifier}` -> 204, @idempotent. Must remove the stored collaboration so a following GetCollaboration/ListCollaborations no longer sees it, and transition its memberships' `status` to `COLLABORATION_DELETED`.

Also implement **DeleteMembership** (`DELETE /memberships/{membershipIdentifier}` -> 204) so the lifecycle can be torn down from either end.

EVERY one of these must compute its response from persisted state. No canned responses, no echoing the request back, no minting an id that is never stored. A create followed by a get must return exactly what was created; a delete followed by a get must fail.

## State Model

In `src/state.rs`, on the single shared engine store:

- `collaborations: HashMap<String /* collaboration id (uuid) */, Collaboration>` where `Collaboration { id, arn, name, description, creator_account_id, creator_display_name, create_time, update_time, member_status, membership_id: Option<String>, membership_arn: Option<String>, query_log_status, job_log_status, analytics_engine, data_encryption_metadata, auto_approved_change_types, allowed_result_regions, is_metrics_enabled, members: Vec<MemberSpecification>, tags: HashMap<String,String> }`
- `memberships: HashMap<String /* membership id (uuid) */, Membership>` where `Membership { id, arn, collaboration_id, collaboration_arn, collaboration_name, collaboration_creator_account_id, collaboration_creator_display_name, create_time, update_time, status, member_abilities: Vec<MemberAbility>, ml_member_abilities, query_log_status, job_log_status, default_result_configuration, default_job_result_configuration, payment_configuration, is_metrics_enabled, tags }`
- `membership_by_collaboration: HashMap<(String /* collaboration id */, String /* account id */), String /* membership id */>` — the index that makes the duplicate-membership `ConflictException` and the Collaboration's `membershipId` field computable instead of guessed.
- `tags: HashMap<String /* resource arn */, HashMap<String,String>>` — shared by collaboration and membership ARNs, so a later TagResource/ListTagsForResource slice has somewhere to land.

Insertion order (or an explicit `Vec<String>` of ids) must be preserved for the List operations so pagination tokens are stable across calls.

All 95+ remaining operations (configured tables, analysis templates, protected queries/jobs, privacy budgets, id mapping tables, intermediate tables, change requests, tagging) stay at 501 for now and will be filed as individual gap issues.

## Evidence

`services/service-com-amazonaws-cleanrooms-awsbastioncontrolplaneservicelambda/OPERATIONS.md` lists 103 operations for SDK id `CleanRooms`, protocol `restJson1`, sigv4 name `cleanrooms`. Routes/fields cited above come from OPERATIONS.md lines 85-121 (CreateCollaboration / GetCollaboration / UpdateCollaboration / DeleteCollaboration / ListCollaborations) and lines 633-669 (CreateMembership / GetMembership / UpdateMembership / DeleteMembership / ListMemberships). Required output members (Collaboration.id/arn/name/creatorAccountId/creatorDisplayName/createTime/updateTime/memberStatus/queryLogStatus; Membership.id/arn/collaborationArn/collaborationId/collaborationCreatorAccountId/collaborationCreatorDisplayName/collaborationName/createTime/updateTime/status/memberAbilities/paymentConfiguration) come from `smithy_query` on the Collaboration, Membership, CollaborationSummary and MembershipSummary shapes, including the UUID length-36 pattern and the collaboration/membership ARN patterns. Declared errors used above (ResourceNotFoundException 404 and ConflictException 409 on CreateMembership/GetMembership; only ValidationException 400 on GetCollaboration/DeleteCollaboration) are from the same OPERATIONS.md entries. Current state: `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches every handler file in the crate (e.g. `src/ops/create_collaboration.rs:4`, `src/ops/get_collaboration.rs:4`, `src/ops/create_membership.rs:4`, `src/ops/list_collaborations.rs:4`), each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` -> HTTP 501. No test asserts any of this behavior; only the generated `tests/state_roundtrip.rs` exists.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI cleanrooms client at it.
2. CreateCollaboration with name='demo-collab', creatorDisplayName='creator', creatorMemberAbilities=['CAN_QUERY'], queryLogStatus='ENABLED', members=[] — expect 200 with collaboration.id a 36-char uuid and collaboration.arn ending in collaboration/<id>. Today: HTTP 501 NotImplemented.
3. GetCollaboration with that collaborationIdentifier — expect 200 whose collaboration body matches the create response field for field (name, description, creatorDisplayName, queryLogStatus, createTime).
4. ListCollaborations — expect the new collaboration present in collaborationList; repeat with maxResults=1 and follow nextToken to page through two collaborations without duplicates or omissions.
5. CreateMembership with collaborationIdentifier=<id> and queryLogStatus='ENABLED' — expect 200 with membership.collaborationName='demo-collab' and collaborationArn copied from the stored collaboration; repeat the identical call and expect ConflictException (409).
6. CreateMembership with a syntactically valid but unknown collaborationIdentifier — expect ResourceNotFoundException (404), not 200.
7. GetMembership with the returned membership id — expect 200 with status=ACTIVE; ListMemberships — expect it in membershipSummaries.
8. DeleteCollaboration with <id> — expect 204; then GetCollaboration <id> expects a modeled ValidationException (400) and ListCollaborations no longer lists it, and ListMemberships shows the membership with status=COLLABORATION_DELETED.
