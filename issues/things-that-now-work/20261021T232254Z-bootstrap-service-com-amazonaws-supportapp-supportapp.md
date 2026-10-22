Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 96234e472a4dbed875261b6f3ae0a9ed15fa6e7c

# Bootstrap the supportapp (AWS Support App in Slack) emulator

_Filed: 2026-10-21T23:22:54Z_
_Operation: service-com-amazonaws-supportapp-supportapp | Class: missing_

> _Note: this issue was identified against the code base at commit `96234e472a4dbed875261b6f3ae0a9ed15fa6e7c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 10 operations in `services/service-com-amazonaws-supportapp-supportapp/src/ops/` is an unmodified scaffold: each carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` -> HTTP 501, holding zero state. This issue defines the FIRST small slice to implement statefully; the remaining operations stay 501 and will be filed as individual gap issues later.

## Happy path to implement (restJson1, sigv4 signing name `supportapp`)

1. **CreateSlackChannelConfiguration** — `POST /control/create-slack-channel-configuration`. Input: `teamId` (required, 1..256, `^\S+$`), `channelId` (required, 1..256, `^\S+$`), `channelName`, `notifyOnCreateOrReopenCase`, `notifyOnAddCorrespondenceToCase`, `notifyOnResolveCase`, `notifyOnCaseSeverity` (required; one of `all|high|none`), `channelRoleArn` (required, 31..2048, `^arn:aws:iam::[0-9]{12}:role/(.+)$`). Output has no members, so the ONLY observable effect is persistence: store a `SlackChannelConfiguration` keyed by `(teamId, channelId)`. Re-creating the same `(teamId, channelId)` must fail with `ConflictException` (409). Missing/ill-formed required members must fail with `ValidationException` (400) rather than being accepted.
2. **ListSlackChannelConfigurations** — `POST /control/list-slack-channel-configurations`. Output `slackChannelConfigurations` (required list of `SlackChannelConfiguration{teamId, channelId, channelName, notifyOnCreateOrReopenCase, notifyOnAddCorrespondenceToCase, notifyOnResolveCase, notifyOnCaseSeverity, channelRoleArn}`) must be built from the stored map, echoing exactly the values that were created — never a canned/empty list. Honour `nextToken` in/out (`@paginated`): with a small page size, return an opaque `nextToken` and resume from it; omit `nextToken` on the last page.
3. **UpdateSlackChannelConfiguration** — `POST /control/update-slack-channel-configuration`. Input: `teamId` + `channelId` required, all other members optional. Must look up the stored config, apply ONLY the members present (leave the others at their stored values), persist, and return the merged post-update record (`teamId`, `channelId`, `channelName`, the three `notifyOn*` booleans, `notifyOnCaseSeverity`, `channelRoleArn`) computed from state — not an echo of the request. Unknown `(teamId, channelId)` -> `ResourceNotFoundException` (404). A subsequent ListSlackChannelConfigurations must show the updated values.
4. **PutAccountAlias** — `POST /control/put-account-alias`. Input `accountAlias` (required, 1..30, `^[\w\- ]+$`). Persist per-account; reject a value violating length/pattern with `ValidationException` (400).
5. **GetAccountAlias** — `POST /control/get-account-alias`. `@readonly`, no input. Return the persisted `accountAlias`; when none has ever been put (or after DeleteAccountAlias) return a 200 with `accountAlias` absent — that is the modeled behaviour (the op declares only `InternalServerException`). Must read state, never a hardcoded alias.
6. **DeleteSlackChannelConfiguration** — `POST /control/delete-slack-channel-configuration`. Input `teamId` + `channelId` (both required). Remove the entry from state; a later List must not contain it and a later Update of it must return `ResourceNotFoundException` (404). Deleting an unknown `(teamId, channelId)` must itself return `ResourceNotFoundException` (404), not 200.

## Non-negotiable: NEVER STUBS

Every operation above must compute its response from persisted state held in the shared engine reached via `crate::state::service_state(&*self.engine)` (see README.md "State"). Hardcoded outputs, echoing the request back without storing it, and generating identifiers without storing them are failures. Because Create/Delete/Put have empty output shapes, the read operations (Get/List/Update) are the only proof the writes happened — they must reflect them.

## State Model

In `src/state.rs` (`ComAmazonawsSupportappSupportappState`):

- `slack_channel_configurations: BTreeMap<(String /*teamId*/, String /*channelId*/), SlackChannelConfig>` where `SlackChannelConfig { team_id: String, channel_id: String, channel_name: Option<String>, notify_on_create_or_reopen_case: Option<bool>, notify_on_add_correspondence_to_case: Option<bool>, notify_on_resolve_case: Option<bool>, notify_on_case_severity: String /* all|high|none */, channel_role_arn: String }`. BTreeMap gives deterministic list ordering for stable pagination.
- `account_alias: Option<String>` — set by PutAccountAlias, read by GetAccountAlias, cleared by DeleteAccountAlias (DeleteAccountAlias itself may stay stubbed in this slice, but the field is where it will write).
- `slack_workspace_configurations: BTreeMap<String /*teamId*/, SlackWorkspaceConfig>` where `SlackWorkspaceConfig { team_id: String, team_name: Option<String>, allow_organization_member_account: Option<bool> }` — declared now so ListSlackWorkspaceConfigurations / RegisterSlackWorkspaceForOrganization / DeleteSlackWorkspaceConfiguration (out of scope for this slice, still 501) have a home.

Done = an integration test runs Create -> List (sees the created config with every field as submitted) -> Update (merged fields returned and visible in a re-List) -> PutAccountAlias -> GetAccountAlias (returns the alias) -> Delete -> List (gone) -> Update (404), all with correct bodies, not just 2xx.

## Evidence

OPERATIONS.md for this crate lists all 10 operations with routes, members and errors: CreateSlackChannelConfiguration (`POST /control/create-slack-channel-configuration`, required teamId/channelId/notifyOnCaseSeverity/channelRoleArn, errors incl. ConflictException 409 and ValidationException 400), ListSlackChannelConfigurations (`@paginated(inputToken=nextToken, outputToken=nextToken)`, required `slackChannelConfigurations`), UpdateSlackChannelConfiguration (required teamId/channelId, full config echoed in output, ResourceNotFoundException 404), DeleteSlackChannelConfiguration (ResourceNotFoundException 404), PutAccountAlias (`accountAlias` required, len 1..30, pattern `^[\w\- ]+$`), GetAccountAlias (`@readonly`, optional `accountAlias` output). Member lists for `SlackChannelConfiguration` and `SlackWorkspaceConfiguration` come from smithy_query on the model at `services/service-com-amazonaws-supportapp-supportapp/smithy-model/com-amazonaws-supportapp-supportapp.json`. Stub census: `grep -n '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-supportapp-supportapp/src/ops/*.rs` matches all 10 handler files, so there is no existing behaviour and no test coverage to duplicate.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `support-app` client at it.
2. `CreateSlackChannelConfiguration` with teamId=T012ABCDEFG, channelId=C01234A5BCD, channelName="aws-support", notifyOnCaseSeverity="high", channelRoleArn="arn:aws:iam::123456789012:role/SupportAppRole", notifyOnCreateOrReopenCase=true — expect HTTP 200 (empty body); today HTTP 501 NotImplemented.
3. `ListSlackChannelConfigurations` — expect one entry with exactly the submitted teamId/channelId/channelName/notifyOnCaseSeverity/channelRoleArn/notifyOn* values; today 501.
4. Repeat the same `CreateSlackChannelConfiguration` — expect ConflictException (HTTP 409).
5. `UpdateSlackChannelConfiguration` with the same teamId/channelId and only notifyOnCaseSeverity="all" — expect the returned record to show notifyOnCaseSeverity="all" while channelName and channelRoleArn keep their stored values; re-List and confirm the merge persisted.
6. `PutAccountAlias` accountAlias="my-team" then `GetAccountAlias` — expect accountAlias="my-team" read back from state; today both 501.
7. `DeleteSlackChannelConfiguration` for that teamId/channelId — expect 200, then `ListSlackChannelConfigurations` returns an empty list and `UpdateSlackChannelConfiguration` for it returns ResourceNotFoundException (HTTP 404).
8. `DeleteSlackChannelConfiguration` for a teamId/channelId that was never created — expect ResourceNotFoundException (HTTP 404), not a success.
