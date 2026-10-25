Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: b6f8a34592cbbf71690dc012f4c1a0ed1cdf21ee

# Refuse a channelRoleArn that names an account other than the caller's

_Filed: 2026-10-24T19:57:06Z_
_Operation: CreateSlackChannelConfiguration | Class: validation_

> _Note: this issue was identified against the code base at commit `b6f8a34592cbbf71690dc012f4c1a0ed1cdf21ee`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`channelRoleArn` is documented as "The Amazon Resource Name (ARN) of an IAM role that you want to use to perform operations on Amazon Web Services" — a role the Support App assumes in the CALLER's own account (see "Managing access to the Amazon Web Services Support App", which has you create the role with the supportapp trust policy in your account). The model's `@pattern ^arn:aws:iam::[0-9]{12}:role/(.+)$` only constrains the ARN's SHAPE: any 12-digit account id satisfies it.

Expected: a create whose `channelRoleArn` names a 12-digit account that is not the caller's account is refused with a modelled client error — the pass-role refusal AWS answers for a role that cannot be passed (`AccessDeniedException`, 403, which this operation models; `ValidationException`, 400, is the other modelled client channel) — and nothing is stored.

Actual: the handler never looks at the ARN's account segment. It stores `input.channel_role_arn` verbatim (`create_slack_channel_configuration.rs`, the `store.slack_channel_configurations.insert(..)` call), answers 200, and `ListSlackChannelConfigurations` / `UpdateSlackChannelConfiguration` then report a `channelRoleArn` for a foreign account as the account's live configuration — a configuration real AWS would never have accepted.

The emulator presents exactly one account and already exposes it as `engine::emulator_account_id()` / `engine::EMULATOR_ACCOUNT_ID` (used by this crate for the workspace-conflict and alias-not-found messages), so the caller's account is known at handler time.

## Evidence

Spec: `smithy_query(query="doc", shape="CreateSlackChannelConfigurationRequest", member="channelRoleArn")` — the role is one the Support App uses to perform operations on YOUR account; `@pattern ^arn:aws:iam::[0-9]{12}:role/(.+)$` constrains only the ARN form, not the account. The operation models `AccessDeniedException (403)` and `ValidationException (400)`, so a modelled channel for the refusal exists.

Handler: `services/service-com-amazonaws-supportapp-supportapp/src/ops/create_slack_channel_configuration.rs` — `handle` validates only (a) the documented severity/notification-flag combination, (b) the workspace-registry precondition, (c) the already-configured conflict, (d) the 20-channel quota. `channelRoleArn` is passed straight into `crate::state::SlackChannelConfig { .. channel_role_arn: input.channel_role_arn }`. `grep emulator_account_id` over the crate shows the create handler never consults the account.

Tests: no test covers a cross-account role ARN. `tests/all/slack_channel_configurations.rs::role_arn()` always builds the ARN with `engine::EMULATOR_ACCOUNT_ID`, and the only malformed-ARN row (`tests/all/error_message_templates.rs`, `arn:aws:iam::ABCDEFGHIJKL:role/SupportAppRole`) exercises the model `@pattern`, not the account identity. So the gap is unasserted.

Well-established behavior: passing an IAM role that lives in another account is a pass-role authorization failure across AWS; sibling emulators in this repo already encode it (e.g. `service-com-amazonaws-dsql-dsql` CreateStream and `service-com-amazonaws-eks-awswesleyfrontend` refuse "a role of another account" with 403, `service-com-amazonaws-fis-faultinjectionsimulator` with 400).

## Steps to Reproduce

1. POST /control/register-slack-workspace-for-organization with {"teamId":"T012ABCDEFG"} so the workspace precondition is satisfied.
2. POST /control/create-slack-channel-configuration with {"teamId":"T012ABCDEFG","channelId":"C01234A5BCD","notifyOnCaseSeverity":"none","channelRoleArn":"arn:aws:iam::999999999999:role/SupportAppRole"} — a well-formed ARN naming an account that is not the emulator's account.
3. Observe HTTP 200 and an empty body instead of a modelled client refusal (403 AccessDeniedException / 400 ValidationException).
4. POST /control/list-slack-channel-configurations and observe the configuration is stored and reported with the foreign-account channelRoleArn.
