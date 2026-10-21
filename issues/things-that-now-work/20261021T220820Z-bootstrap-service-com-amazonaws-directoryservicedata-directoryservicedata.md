Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 95c91931c960f7f391e4448ce7f3ebc67d41fa17

# Bootstrap the Directory Service Data emulator

_Filed: 2026-10-21T22:08:20Z_
_Operation: service-com-amazonaws-directoryservicedata-directoryservicedata | Class: missing_

> _Note: this issue was identified against the code base at commit `95c91931c960f7f391e4448ce7f3ebc67d41fa17`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

All 17 operations in `src/ops/` are unmodified scaffolds that answer HTTP 501 `NotImplemented` and hold no state. Implement a first SMALL stateful slice covering the user lifecycle plus one group-membership step, so a realistic client script works end to end against the restJson1 `ds-data` endpoint.

### Happy path (in order)
1. **CreateUser** — `POST /Users/CreateUser?DirectoryId=d-xxxxxxxxxx`. Input: `DirectoryId` (required, `^d-[0-9a-f]{10}$`), `SAMAccountName` (required, len 1..20, `^[\w\-.]+$`), optional `EmailAddress`, `GivenName`, `Surname`, `OtherAttributes` (map<string,AttributeValue>), `ClientToken`. Must persist a new user record under (DirectoryId, SAMAccountName) and return `DirectoryId`, `SAMAccountName` and a freshly MINTED-AND-STORED `SID` (len 1..256, e.g. `S-1-5-21-<rand>-<seq>`), with `Enabled = true` by default. A second CreateUser with the same SAMAccountName must fail `ConflictException` (409), not silently overwrite.
2. **DescribeUser** — `POST /Users/DescribeUser`. Input: `DirectoryId` (required), `SAMAccountName` (required), optional `Realm`, `OtherAttributes` (list of attribute names to project). Must read the stored record and return `DirectoryId`, `Realm`, `SID`, `SAMAccountName`, `DistinguishedName` (derived, e.g. `CN=<SAMAccountName>,CN=Users,DC=...`), `UserPrincipalName`, `EmailAddress`, `GivenName`, `Surname`, `Enabled`, and only the requested `OtherAttributes`. Unknown user → `ResourceNotFoundException` (404). The values returned must be the values stored by CreateUser/UpdateUser — not echoed from this request.
3. **ListUsers** — `POST /Users/ListUsers`, `@paginated(inputToken=NextToken, outputToken=NextToken, items=Users)`. Input: `DirectoryId` (required), optional `Realm`, `NextToken`, `MaxResults` (1..250). Must return `Users` (list<UserSummary>) built from the stored users of that directory, honour `MaxResults` as a hard page size, emit a `NextToken` only when more remain, and resume exactly after the previous page's last item when a token is supplied. A user created in step 1 must appear; a deleted user must not.
4. **CreateGroup** + **AddGroupMember** — `POST /Groups/CreateGroup` stores a group under (DirectoryId, SAMAccountName) with `GroupType` (Distribution|Security), `GroupScope` (BuiltinLocal|DomainLocal|Global|Universal), `OtherAttributes`, minted `SID`, and returns `DirectoryId`, `SAMAccountName`, `SID`. `POST /GroupMemberships/AddGroupMember` (Input: `DirectoryId`, `GroupName`, `MemberName`, optional `MemberRealm`, `ClientToken`; empty output) must append the member to that group's membership set; unknown group or unknown member → `ResourceNotFoundException` (404); adding an existing member → `ConflictException` (409).
5. **ListGroupMembers** — `POST /GroupMemberships/ListGroupMembers`, paginated on `Members`. Input: `DirectoryId` (required), `SAMAccountName` (required = group), optional `Realm`, `MemberRealm`, `NextToken`, `MaxResults`. Must return `Members` (list<Member> with the member's `SAMAccountName`, `SID`, `MemberType`) computed from the stored membership set, so the member added in step 4 is visible. Unknown group → 404.
6. **DeleteUser** — `POST /Users/DeleteUser`. Input: `DirectoryId`, `SAMAccountName`, optional `ClientToken`; empty output. Must remove the stored user AND its group memberships. A following DescribeUser must return `ResourceNotFoundException` (404) and ListUsers must no longer include it. Deleting an unknown user → 404.

Every one of these must compute its response from persisted state. Hardcoded responses, echoing the request back, or minting a SID without storing it are failures — in particular DescribeUser must not reconstruct its answer from its own input, and the two List operations must not return empty/fixed lists.

### Validation to enforce in the slice
- `DirectoryId` must match `^d-[0-9a-f]{10}$`; user `SAMAccountName` len 1..20 / `^[\w\-.]+$`; group/member names len 1..64 / 1..63 with the `^[^:;|=+"*?<>/\\,\[\]@]+$` pattern; `MaxResults` in 1..250; `OtherAttributes` map size 1..25. Violations → `ValidationException` (400).
- Missing required members → `ValidationException` (400).
- Unknown resource → `ResourceNotFoundException` (404); duplicate create / duplicate membership → `ConflictException` (409).

### State Model
Store in the shared per-service state (`src/state.rs`, reached via `crate::state::service_state(&*self.engine)`):

- `directories: HashMap<String /*DirectoryId*/, Directory>`
- `Directory { realm: String /* default realm, e.g. "corp.example.com" */, users: HashMap<String /*SAMAccountName, case-insensitive key*/, User>, groups: HashMap<String /*SAMAccountName*/, Group>, next_rid: u64 /* RID counter for SID minting */ }`
- `User { sam_account_name: String, sid: String, realm: String, distinguished_name: String, user_principal_name: String, email_address: Option<String>, given_name: Option<String>, surname: Option<String>, enabled: bool, other_attributes: HashMap<String, AttributeValue>, created_at: SystemTime }`
- `Group { sam_account_name: String, sid: String, realm: String, distinguished_name: String, group_type: Option<GroupType>, group_scope: Option<GroupScope>, other_attributes: HashMap<String, AttributeValue>, members: Vec<Membership> }`
- `Membership { member_name: String, member_sid: String, member_realm: String, member_type: MemberType }` — plus a reverse index `memberships_by_member: HashMap<String /*member SAMAccountName*/, BTreeSet<String /*group SAMAccountName*/>>` so ListGroupsForMember and DeleteUser cleanup are O(1).
- `client_tokens: HashMap<String /*ClientToken*/, OperationResultFingerprint>` so an `@idempotencyToken` replay returns the first result instead of a 409.
- Pagination: `NextToken` is an opaque, server-encoded cursor (e.g. base64 of the last returned key + page kind), decoded and validated on input — an unparseable token → `ValidationException` (400).

List/key ordering must be deterministic (sort by SAMAccountName) so pagination is stable across calls.

### Out of scope for this first slice
`UpdateUser`, `UpdateGroup`, `DisableUser`, `DescribeGroup`, `ListGroups`, `ListGroupsForMember`, `RemoveGroupMember`, `SearchUsers`, `SearchGroups`, `DeleteGroup` stay 501 and will be filed as individual gap issues later.

### Done when
A client script runs CreateUser → DescribeUser → ListUsers → CreateGroup → AddGroupMember → ListGroupMembers → DeleteUser → DescribeUser(404) and every response body is correct and derived from state, not just 2xx.

## Evidence

OPERATIONS.md for service-com-amazonaws-directoryservicedata-directoryservicedata lists all 17 operations with routes, members and errors: CreateUser (`POST /Users/CreateUser`, output DirectoryId/SID/SAMAccountName), DescribeUser (`@readonly`, output incl. SID, DistinguishedName, UserPrincipalName, Enabled, OtherAttributes, error ResourceNotFoundException 404), ListUsers (`@paginated(inputToken=NextToken, outputToken=NextToken, items=Users)`, MaxResults range 1..250), CreateGroup (output DirectoryId/SAMAccountName/SID), AddGroupMember (`POST /GroupMemberships/AddGroupMember`, empty output, errors ConflictException 409 / ResourceNotFoundException 404), ListGroupMembers (paginated on Members), DeleteUser (empty output, ResourceNotFoundException 404). Wire protocol per OPERATIONS.md: restJson1, sigv4 name `ds-data`. Constraint patterns quoted above (DirectoryId `^d-[0-9a-f]{10}$`, user SAMAccountName len 1..20 `^[\w\-.]+$`) come from the same file. Current state: `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-directoryservicedata-directoryservicedata/src/ops/` matches all 17 handler files (add_group_member.rs, create_group.rs, create_user.rs, delete_group.rs, delete_user.rs, describe_group.rs, describe_user.rs, disable_user.rs, list_group_members.rs, list_groups.rs, list_groups_for_member.rs, list_users.rs, remove_group_member.rs, search_groups.rs, search_users.rs, update_group.rs, update_user.rs), each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` → HTTP 501. No tests exercise any behaviour (only the scaffolded `tests/state_roundtrip.rs`).

## Steps to Reproduce

1. Start the emulator and point an AWS SDK `ds-data` client at it.
2. Call CreateUser with DirectoryId=d-1234567890, SAMAccountName="jdoe", GivenName="J", Surname="Doe", EmailAddress="jdoe@corp.example.com" — expect 200 with DirectoryId, SAMAccountName="jdoe" and a non-empty SID; today: HTTP 501 NotImplemented.
3. Call DescribeUser with the same DirectoryId and SAMAccountName — expect 200 returning the stored SID, DistinguishedName, GivenName="J", Surname="Doe", EmailAddress, Enabled=true; today: HTTP 501.
4. Call ListUsers with DirectoryId=d-1234567890, MaxResults=1 — expect "jdoe" in Users and a NextToken only if more users exist; today: HTTP 501.
5. Call CreateGroup with SAMAccountName="engineers", GroupType=Security, GroupScope=Global — expect 200 with a stored SID; then AddGroupMember with GroupName="engineers", MemberName="jdoe" — expect 200/empty body; today: HTTP 501.
6. Call ListGroupMembers with SAMAccountName="engineers" — expect Members to contain jdoe with its SID; today: HTTP 501.
7. Call AddGroupMember again for the same (group, member) — expect ConflictException (409); call it with GroupName="nope" — expect ResourceNotFoundException (404).
8. Call DeleteUser for "jdoe", then DescribeUser for "jdoe" — expect ResourceNotFoundException (404), ListUsers no longer containing jdoe, and ListGroupMembers("engineers") no longer containing jdoe.
9. Call CreateUser with DirectoryId="bad-id" or SAMAccountName="a".repeat(21) — expect ValidationException (400).
