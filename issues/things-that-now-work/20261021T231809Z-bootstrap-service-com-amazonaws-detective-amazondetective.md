Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 16fab4a727e8e34e48fe5cd80cb0080f7932bd9f

# Bootstrap the Detective emulator

_Filed: 2026-10-21T23:18:09Z_
_Operation: service-com-amazonaws-detective-amazondetective | Class: missing_

> _Note: this issue was identified against the code base at commit `16fab4a727e8e34e48fe5cd80cb0080f7932bd9f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 29 Detective operations is an unmodified scaffold answering HTTP 501 `NotImplemented` with zero stored state, so no client workflow works at all. Implement a FIRST, SMALL slice: the behavior-graph + member-account lifecycle, backed by real persisted state (restJson1 wire protocol, signing name `detective`, endpoint prefix `api.detective`).

Happy path to implement, in order:

1. **CreateGraph** — `POST /graph`. Input: optional `Tags` (map<string,string>, 1..50). Must mint a new graph ARN matching the modeled pattern `^arn:aws[-\w]{0,10}?:detective:[-\w]{2,20}?:\d{12}?:graph:[abcdef\d]{32}?$` (i.e. 32 lowercase hex chars as the graph id), record `CreatedTime`, store the `Tags`, and return the generated `GraphArn`. The ARN must be a real key in the store, not a throwaway random string.

2. **ListGraphs** — `POST /graphs/list`. Input: `NextToken`, `MaxResults` (1..200). Output `GraphList` (list<Graph> = `{Arn, CreatedTime}`) built from the stored graphs — every graph created in step 1 must appear with the same ARN and the `CreatedTime` recorded at creation. Honor `MaxResults` by truncating and returning a `NextToken` that resumes the listing; no token (or exhausted list) means `NextToken` is absent.

3. **CreateMembers** — `POST /graph/members`. Input: `GraphArn` (required), `Accounts` (list<Account> = `{AccountId 12 digits, EmailAddress}`, 1..50, required), optional `Message`, `DisableEmailNotification` (default false). Must resolve `GraphArn` against stored graphs and return `ResourceNotFoundException` (404) for an unknown ARN. For each accepted account, persist a member record and return it in `Members` as a `MemberDetail` with `AccountId`, `EmailAddress`, `GraphArn`, `AdministratorId` (and legacy `MasterId`), `Status` = `INVITED` (or `VERIFICATION_IN_PROGRESS`), `InvitationType` = `INVITATION`, `InvitedTime`/`UpdatedTime`. Duplicate or malformed accounts go into `UnprocessedAccounts`.

4. **ListMembers** — `POST /graph/members/list`. Input: `GraphArn` (required), `NextToken`, `MaxResults` (1..200). Output `MemberDetails` read back from the member records stored in step 3 — same account ids, emails and statuses; pagination behaves as in step 2. Unknown `GraphArn` → `ResourceNotFoundException`.

5. **GetMembers** — `POST /graph/members/get`. Input: `GraphArn` (required), `AccountIds` (1..50, required). Returns `MemberDetails` for exactly the requested, existing members; account ids with no stored membership in that graph must be returned in `UnprocessedAccounts` (with a reason), NOT fabricated as members.

6. **DeleteGraph** — `POST /graph/removal`. Input: `GraphArn` (required). Removes the graph, its member records and its tags from the store, returns an empty 200 body. A subsequent `ListGraphs` must not list the ARN, and `ListMembers`/`GetMembers`/`CreateMembers` on that ARN must return `ResourceNotFoundException` (404).

All six must compute their responses from persisted state shared across requests — no canned bodies, no echoing the request back, no minting an ARN or member list that is never stored. `GraphArn` inputs must be validated against the modeled pattern (→ `ValidationException`, 400) before lookup, and 12-digit `^[0-9]+$` account ids likewise.

## State Model

- `DetectiveState { graphs: Map<GraphArn /* String */, Graph> , admin_account: Option<String> }` guarded by the service's usual state lock.
- `Graph { arn: String /* arn:aws:detective:<region>:<account>:graph:<32 hex> */, created_time: DateTime, tags: Map<String, String>, members: Map<AccountId /* String, 12 digits */, Member> }`
- `Member { account_id: String, email_address: String, graph_arn: String, administrator_id: String /* = graph owner account */, status: MemberStatus /* INVITED | VERIFICATION_IN_PROGRESS | ENABLED | ACCEPTED_BUT_DISABLED */, invitation_type: InvitationType /* INVITATION | ORGANIZATION */, invited_time: DateTime, updated_time: DateTime, disabled_reason: Option<MemberDisabledReason> }`
- Pagination: `NextToken` encodes an opaque offset/cursor into the deterministically ordered graph list or member list (len 1..1024 per the model).
- `tags` on `Graph` is the same map later reused by TagResource/UntagResource/ListTagsForResource, so store it on the graph record now even though those three ops stay stubbed.

The other 23 operations (invitations, investigations, datasource packages, organization configuration, tagging) remain 501 for now and will be filed as individual gap issues.

## Evidence

OPERATIONS.md at the crate root (services/service-com-amazonaws-detective-amazondetective/OPERATIONS.md) lists all 29 operations with routes, input/output members and errors — CreateGraph (`POST /graph`, out `GraphArn`), ListGraphs (`POST /graphs/list`, out `GraphList`/`NextToken`, @paginated), CreateMembers (`POST /graph/members`, in `GraphArn`+`Accounts`, out `Members`/`UnprocessedAccounts`), ListMembers (`POST /graph/members/list`, @paginated), GetMembers (`POST /graph/members/get`), DeleteGraph (`POST /graph/removal`). Shape details for `Graph {Arn, CreatedTime}`, `Account {AccountId, EmailAddress}` and `MemberDetail {AccountId, EmailAddress, GraphArn, AdministratorId, MasterId, Status, InvitationType, InvitedTime, UpdatedTime, ...}` come from smithy_query on the model. `grep -n 'SCAFFOLDED, NOT IMPLEMENTED' src/ops/` matches all 29 handler files (create_graph.rs, list_graphs.rs, create_members.rs, list_members.rs, get_members.rs, delete_graph.rs included), each returning `Scaffolded::not_implemented(..)` → HTTP 501, proving no state exists anywhere in the crate.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI Detective client at it (`--endpoint-url`).
2. `aws detective create-graph --tags Env=test` → expect 200 with a `GraphArn` matching `arn:aws:detective:<region>:<account>:graph:<32 hex>`; actual: HTTP 501 NotImplemented.
3. `aws detective list-graphs` → expect the ARN from step 2 in `GraphList` with its `CreatedTime`; actual: 501.
4. `aws detective create-members --graph-arn <arn> --accounts AccountId=111122223333,EmailAddress=member@example.com` → expect 200 with one `MemberDetail` (AccountId 111122223333, Status INVITED, GraphArn = <arn>); actual: 501.
5. `aws detective list-members --graph-arn <arn>` → expect the member persisted in step 4 to be returned; actual: 501.
6. `aws detective get-members --graph-arn <arn> --account-ids 111122223333 999999999999` → expect the first in `MemberDetails` and the second in `UnprocessedAccounts`; actual: 501.
7. `aws detective delete-graph --graph-arn <arn>` → expect empty 200; then `list-graphs` must no longer show the ARN and `list-members --graph-arn <arn>` must return ResourceNotFoundException (404); actual: 501 for all.
