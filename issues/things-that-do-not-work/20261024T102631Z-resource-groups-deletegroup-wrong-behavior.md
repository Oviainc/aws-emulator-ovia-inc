Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 9f696bee74236d7bb111be1a5cc08ff08986f981

# DeleteGroup ignores the stored deletion-protection configuration parameter

_Filed: 2026-10-24T10:26:31Z_
_Operation: DeleteGroup | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `9f696bee74236d7bb111be1a5cc08ff08986f981`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

DeleteGroup removes the group record unconditionally once the identifier resolves. It never inspects the group's stored service `Configuration`, so the `deletion-protection` parameter a group may have been created with (`AWS::ResourceGroups::Generic` / application-group configuration, value `UNLESS_EMPTY`) has no effect.

Expected: a group configured with `deletion-protection = UNLESS_EMPTY` that still has members cannot be deleted — the real service refuses with `BadRequestException` (HTTP 400) and the group and its memberships remain. Deletion succeeds only once the group is empty (or when the parameter is `DISABLED`/absent).

Actual: HTTP 200, the record (including its memberships and tags) is dropped, and a subsequent GetGroup returns 404 — i.e. protected state is destroyed. The configuration is stored faithfully by CreateGroup, so the parameter is accepted and then silently ignored, which is worse than rejecting it: a client gets a success for an operation the real service blocks.

## Evidence

Spec/semantics: `GroupConfigurationItem$Type` and `GroupConfigurationParameter$Name` are documented against the AWS "Service configurations for Resource Groups" reference (linked from the model docs), which defines the `deletion-protection` parameter with value `UNLESS_EMPTY` as preventing deletion of a group that still contains resources; `DeleteGroup` declares `BadRequestException (HTTP 400)`. Handler: `src/ops/delete_group.rs` resolves the name and calls `store.groups.remove(&name)` with no reference to `record.configuration` or `record.members`. State: `src/state.rs` `GroupRecord.configuration` holds the configuration items/parameters verbatim. Tests: `tests/all/group_owner_configuration_and_update.rs` line ~190 creates a group carrying `deletion-protection=UNLESS_EMPTY` but that group has no members and the suite only asserts the unconditional 200 delete; no test covers deleting a protected, non-empty group.

## Steps to Reproduce

1. POST /groups with {"Name":"protected","Configuration":[{"Type":"AWS::ResourceGroups::ApplicationGroup"},{"Type":"AWS::ResourceGroups::Generic","Parameters":[{"Name":"deletion-protection","Values":["UNLESS_EMPTY"]}]}]} — 200.
2. POST /group-resources with {"Group":"protected","ResourceArns":["arn:aws:ec2:us-east-1:000000000000:instance/i-0123456789abcdef0"]} — 200, the group now has one member.
3. POST /delete-group with {"Group":"protected"} — observe 200 and the deleted Group in the body.
4. POST /get-group with {"Group":"protected"} — observe 404, the protected group is gone.
5. Expected at step 3: BadRequestException with HTTP 400 while the group is non-empty, with the group still readable afterwards; the delete should succeed only after the member is ungrouped.
