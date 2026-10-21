Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: e44f583f966e818f9abdd86a5b0ab9a8f2f88230

# Bootstrap the identitystore emulator

_Filed: 2026-10-21T23:27:54Z_
_Operation: service-com-amazonaws-identitystore-awsidentitystore | Class: missing_

> _Note: this issue was identified against the code base at commit `e44f583f966e818f9abdd86a5b0ab9a8f2f88230`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

All 22 operations of the AWS Identity Store (`AWSIdentityStore`, awsJson1_1, `X-Amz-Target: AWSIdentityStore.<Op>`) are unmodified scaffolds that answer HTTP 501 `NotImplemented` and hold no state. Implement a FIRST stateful slice covering the primary resources (user, group, group membership) so a realistic client script works end to end.

### Happy path to implement (in order)

1. **CreateUser** — input `IdentityStoreId` (required), `UserName`, `Name`, `DisplayName`, `Emails`, and the rest of the SCIM-ish profile members; output `IdentityStoreId`, `UserId`, `UserArn`, `Revision` (all required). Mint a UUID `UserId`, an ARN of the form `arn:aws:identitystore:::user/<UserId>` (the model constrains `UserArn` to len 60..100 and pattern `^arn:aws[a-z-]*:identitystore:::(user|group|membership)/...`), a numeric `Revision` string (pattern `^[0-9]+$`), and PERSIST the whole submitted profile plus `CreatedAt`/`CreatedBy`. A second CreateUser with the same `UserName` in the same identity store must fail `ConflictException` (409), not create a duplicate.
2. **DescribeUser** — input `IdentityStoreId` + `UserId`; must return the stored profile exactly as created: `UserId`, `UserArn`, `Revision`, `UserName`, `Name`, `DisplayName`, `Emails`, `CreatedAt`, `UpdatedAt`. Unknown `UserId` → `ResourceNotFoundException` (404). No echoing of request fields and no synthesized placeholder profile.
3. **CreateGroup** — input `IdentityStoreId` (required), `DisplayName`, `Description`; output `IdentityStoreId`, `GroupId`, `GroupArn`, `Revision` (all required). Persist the group record (display name, description, created/updated timestamps). Duplicate `DisplayName` in one store → `ConflictException` (409). A later **DescribeGroup** on that `GroupId` must return the stored `DisplayName`/`Description`/`Revision`.
4. **CreateGroupMembership** — input `IdentityStoreId`, `GroupId`, `MemberId` (the `MemberId` union, in practice `{"UserId": "..."}`); output `IdentityStoreId`, `MembershipId`, `MembershipArn`. Must validate that both the group and the member user exist in that identity store (→ `ResourceNotFoundException` 404 otherwise), must reject a membership that already exists with `ConflictException` (409), and must store the membership keyed by `MembershipId` plus indexes by group and by member.
5. **ListGroupMemberships** — input `IdentityStoreId`, `GroupId`, `MaxResults` (1..100, default 100), `NextToken`; output `GroupMemberships` (required list of `GroupMembership{IdentityStoreId, MembershipId, GroupId, MemberId}`) + `NextToken`. Must list exactly the memberships created in step 4 for that group, honour `MaxResults` as a page bound and round-trip an opaque `NextToken` (pattern `^[-a-zA-Z0-9+=/:_]*$`) so a paginating client walks every item exactly once.
6. **DeleteGroupMembership** — input `IdentityStoreId`, `MembershipId`; empty output. Must remove the membership from the store and its indexes so the next ListGroupMemberships no longer returns it. Unknown `MembershipId` → `ResourceNotFoundException` (404) per the modeled errors.

Every one of these six MUST compute its response from persisted state held in the shared engine (`crate::state::service_state(&*self.engine)` — see README "State"). Hardcoded ids/ARNs without storage, echoed inputs, or a success that stores nothing are failures of this issue, not implementations of it. IDs must be stable: the `UserId`/`GroupId`/`MembershipId` returned by a create is the one every later read, list and delete accepts.

### State Model

In `ComAmazonawsIdentitystoreAwsidentitystoreState`:

- `stores: HashMap<String /*IdentityStoreId*/, IdentityStoreData>` — auto-vivify (or seed) a default store such as `d-1234567890` so clients need no provisioning call; `IdentityStoreData { identity_store_id, identity_store_arn }`.
- `IdentityStoreData.users: HashMap<String /*UserId*/, User>` where `User { user_id, user_arn, revision: u64, user_name: Option<String>, name: Option<Name>, display_name, nick_name, profile_url, emails: Vec<Email>, addresses, phone_numbers, user_type, title, preferred_language, locale, timezone, user_status, photos, website, birthdate, roles, external_ids, created_at, updated_at, created_by, updated_by }`.
- `IdentityStoreData.user_name_index: HashMap<String /*UserName*/, String /*UserId*/>` — backs the CreateUser conflict check and later `GetUserId` by `AlternateIdentifier`.
- `IdentityStoreData.groups: HashMap<String /*GroupId*/, Group>` where `Group { group_id, group_arn, revision: u64, display_name, description, external_ids, created_at, updated_at, created_by, updated_by }`, plus `group_display_name_index: HashMap<String, String>`.
- `IdentityStoreData.memberships: HashMap<String /*MembershipId*/, GroupMembershipRecord>` where `GroupMembershipRecord { membership_id, membership_arn, group_id, member_id: MemberIdValue /*UserId variant*/, created_at, updated_at }`, plus `memberships_by_group: HashMap<String /*GroupId*/, Vec<String>>` (insertion-ordered, so list ordering is deterministic and pagination is stable) and `memberships_by_member: HashMap<String /*UserId*/, Vec<String>>` (ready for `ListGroupMembershipsForMember` / `IsMemberInGroups` in a later slice).
- Revisions are monotonic per record: a create sets `revision = 1`; any later `UpdateUser`/`UpdateGroup` bumps it, so an optimistic-concurrency `Revision` mismatch can be answered with `ConflictException`.

### Out of scope for this slice

UpdateUser, UpdateGroup, DeleteUser, DeleteGroup, ListUsers, ListGroups, GetUserId, GetGroupId, GetGroupMembershipId, DescribeGroupMembership, IsMemberInGroups, ListGroupMembershipsForMember, DescribeIdentityStore, UpdateIdentityStore and ListIdentityStores may keep answering 501 and will be filed as individual gap issues; the state model above is deliberately shaped so they drop in without a rewrite.

## Evidence

All 22 handler files under `services/service-com-amazonaws-identitystore-awsidentitystore/src/ops/` carry the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and return `Err(crate::scaffolded::Scaffolded::not_implemented(..))` → HTTP 501, so no CreateUser/CreateGroup/CreateGroupMembership call can be followed by a successful Describe/List. Operation contracts (required members, modeled errors with their HTTP codes, `@paginated(inputToken=NextToken, outputToken=NextToken, items=GroupMemberships)`, `MaxResults` range 1..100 default 100, ARN/Revision patterns) are taken from `services/service-com-amazonaws-identitystore-awsidentitystore/OPERATIONS.md`, rendered from `smithy-model/com-amazonaws-identitystore-awsidentitystore.json`. Wire protocol (`awsJson1_1`, `POST /`, `X-Amz-Target: AWSIdentityStore.<Op>`) from the same file's "Wire protocol" section. No tests exist beyond the generated `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. Start the emulator and point an identitystore client at it with IdentityStoreId=d-1234567890.
2. CreateUser {IdentityStoreId, UserName: "alice", DisplayName: "Alice Example", Name:{GivenName:"Alice",FamilyName:"Example"}, Emails:[{Value:"alice@example.com",Primary:true}]} → expect 200 with a fresh UserId, a matching UserArn and Revision; today 501.
3. DescribeUser {IdentityStoreId, UserId: <from step 2>} → expect the stored UserName/DisplayName/Name/Emails and the same Revision; today 501.
4. CreateGroup {IdentityStoreId, DisplayName: "engineering", Description: "Eng team"} → expect 200 with GroupId/GroupArn/Revision; then DescribeGroup on that GroupId returns the stored DisplayName and Description.
5. CreateGroupMembership {IdentityStoreId, GroupId: <group>, MemberId: {UserId: <user>}} → expect 200 with MembershipId/MembershipArn; repeat the identical call → expect ConflictException (409).
6. ListGroupMemberships {IdentityStoreId, GroupId: <group>, MaxResults: 1} → expect exactly the created membership with the same MembershipId and MemberId; with more than one member, expect a NextToken that walks the remaining items without repeats.
7. DeleteGroupMembership {IdentityStoreId, MembershipId: <from step 5>} → expect 200 empty body; ListGroupMemberships for the group now returns an empty list, and DeleteGroupMembership on an unknown MembershipId returns ResourceNotFoundException (404).
