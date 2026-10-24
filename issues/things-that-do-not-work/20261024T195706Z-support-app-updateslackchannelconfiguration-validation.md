Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: b6f8a34592cbbf71690dc012f4c1a0ed1cdf21ee

# Refuse an update whose channelRoleArn names an account other than the caller's

_Filed: 2026-10-24T19:57:06Z_
_Operation: UpdateSlackChannelConfiguration | Class: validation_

> _Note: this issue was identified against the code base at commit `b6f8a34592cbbf71690dc012f4c1a0ed1cdf21ee`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateSlackChannelConfiguration` accepts an optional `channelRoleArn` with the same `roleArn` target as create (`len 31..2048`, `@pattern ^arn:aws:iam::[0-9]{12}:role/(.+)$`) and the same documented meaning: an IAM role the Support App uses to perform operations on the caller's account.

Expected: an update that re-points a configuration at a role in a different 12-digit account is refused with one of the operation's modelled client errors (`AccessDeniedException` 403 — the pass-role refusal — or `ValidationException` 400) and the stored record is left unchanged.

Actual: the handler merges `input.channel_role_arn` into the stored record with no account check (`update_slack_channel_configuration.rs`: `channel_role_arn: input.channel_role_arn.unwrap_or_else(|| stored.channel_role_arn.clone())`), answers 200 echoing the foreign ARN back, and persists it — so a configuration that was created with a legitimate same-account role can be mutated into one real AWS would reject. This is the same missing pass-role check as on create, on the write path that can also overwrite an already-validated value.

## Evidence

Spec: `smithy_query(query="operation", operation="UpdateSlackChannelConfiguration")` — `channelRoleArn` targets `roleArn` with `@pattern ^arn:aws:iam::[0-9]{12}:role/(.+)$` (form only, any account id passes); errors include `AccessDeniedException (403)` and `ValidationException (400)`. The member doc (shared with the create request) describes the role as one used "to perform operations on Amazon Web Services" for the caller's account, created per "Managing access to the Amazon Web Services Support App".

Handler: `services/service-com-amazonaws-supportapp-supportapp/src/ops/update_slack_channel_configuration.rs` — after the `ResourceNotFoundException` lookup, `handle` validates only the merged severity/notification-flag rule (`crate::state::notification_rule_violation`). `channel_role_arn` is merged and written with `store.slack_channel_configurations.insert(key, merged)` and echoed in the output; `engine::emulator_account_id()` is never consulted.

Tests: `tests/all/slack_channel_configurations.rs::update_applies_only_the_members_the_request_carries` updates `channelRoleArn` with `role_arn("UpdatedRole")` / `role_arn("FullyUpdatedRole")`, both built from `engine::EMULATOR_ACCOUNT_ID`. No row uses a foreign account, so no test asserts this behavior either way.

Well-established behavior: a role ARN naming another account cannot be passed to a service in this account; sibling emulators in this repo refuse it on both create and update paths (e.g. `service-com-amazonaws-transfer-transferservice` create_user/update_user, `service-com-amazonaws-aiops-aiops` update_investigation_group).

## Steps to Reproduce

1. POST /control/register-slack-workspace-for-organization with {"teamId":"T012ABCDEFG"}.
2. POST /control/create-slack-channel-configuration with {"teamId":"T012ABCDEFG","channelId":"C01234A5BCD","notifyOnCaseSeverity":"none","channelRoleArn":"arn:aws:iam::<emulator-account-id>:role/SupportAppRole"} — accepted (200).
3. POST /control/update-slack-channel-configuration with {"teamId":"T012ABCDEFG","channelId":"C01234A5BCD","channelRoleArn":"arn:aws:iam::999999999999:role/SomeoneElsesRole"}.
4. Observe HTTP 200 whose body echoes the foreign-account channelRoleArn instead of a modelled client refusal.
5. POST /control/list-slack-channel-configurations and observe the persisted configuration now carries the other account's role ARN.
