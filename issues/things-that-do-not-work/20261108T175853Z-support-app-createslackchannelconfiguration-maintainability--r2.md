Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: f33ab3c27f32123414d328599fae51957c068e3a

# Hold the channelRoleArn two-step refusal order in one place instead of once per writing handler

_Filed: 2026-11-08T17:58:53Z_
_Operation: CreateSlackChannelConfiguration | Class: maintainability_

> _Note: this issue was identified against the code base at commit `f33ab3c27f32123414d328599fae51957c068e3a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The crate states ONE rule for the `channelRoleArn` member it takes off the request boundary: the declared length bound is judged first and answered as a 400 `ValidationException`, and only then is the pass-role account test applied and answered as a 403 `AccessDeniedException`. `crate::state` owns the two tests (`channel_role_arn_length_refusal`, `channel_role_account_refusal`) and documents the ORDER between them, but the order itself — which check runs first and which modelled error class each refusal is lifted into — is written out twice, once in `CreateSlackChannelConfiguration::handle` and once, in a second spelling, in `UpdateSlackChannelConfiguration::handle`. The two copies are the only thing keeping the two operations answering the same class for the same value: reordering (or dropping) one of the two calls in either copy silently makes one operation answer 403 where its sibling answers 400 for an over-long cross-account ARN, and nothing in `state.rs` — where the rule is documented — would change. This is exactly the drift the state module says it exists to prevent, and it is the same shape as the two maintainability gaps already fixed in this crate (`...createslackchannelconfiguration-maintainability.md`, `...registerslackworkspacefororganization-maintainability.md`), both of which folded handler-side logic back into the state helper that already implemented it. A single state-side entry point that returns WHICH of the two refusals applies (the message plus its class), with each handler mapping that answer onto its own operation error enum, keeps every status, every error shape and every message byte-identical.

## Evidence

The rule's home, and why it is a single implementation — services/service-com-amazonaws-supportapp-supportapp/src/state.rs:5-13:
```
//! Besides the state struct itself this module holds the few rules the Support App
//! handlers share — ... and the wording of the failures
//! several operations can answer with — so the operation files
//! agree on one implementation of each instead of carrying copies that can drift apart.
```

The ORDER rule, stated once in state.rs:535-537 (doc of `channel_role_arn_length_refusal`):
```
/// the modelled `ValidationException` (HTTP 400) rather than something stored: it is a
/// caller-input mistake, not a role this service may not pass, and it is settled before
/// the pass-role attempt for that reason.
```
and state.rs:502-504 (doc of `channel_role_account_refusal`): "EVERY operation that takes the member answers this refusal rather than persisting the value, before the account's store is consulted, so nothing is written when it holds."

Copy 1 — services/service-com-amazonaws-supportapp-supportapp/src/ops/create_slack_channel_configuration.rs:210-235:
```
        if let Some(message) =
            crate::state::channel_role_arn_length_refusal(&input.channel_role_arn)
        {
            return Err(CreateSlackChannelConfigurationErrorModeled::ValidationException(
...
        if let Some(message) =
            crate::state::channel_role_account_refusal(&input.channel_role_arn)
        {
            return Err(
                CreateSlackChannelConfigurationErrorModeled::AccessDeniedException(
```

Copy 2 — services/service-com-amazonaws-supportapp-supportapp/src/ops/update_slack_channel_configuration.rs:228-258:
```
        if let Some(message) = input
            .channel_role_arn
            .as_deref()
            .and_then(crate::state::channel_role_arn_length_refusal)
        {
            return Err(UpdateSlackChannelConfigurationErrorModeled::ValidationException(
...
        if let Some(message) = input
            .channel_role_arn
            .as_deref()
            .and_then(crate::state::channel_role_account_refusal)
        {
            return Err(
                UpdateSlackChannelConfigurationErrorModeled::AccessDeniedException(
```
The two blocks are the same two-step gate in two spellings; only the `Option` handling and the error enum differ.

Tests that pin the behaviour the fix must leave unchanged: tests/all/channel_role_pass_role_form.rs:17-29 drives the refused rows "on BOTH operations that take the member" and keeps the length maximum as a 400 ("its declared `@length` maximum still bounds what this API can carry, so a value past it is a 400 too — a caller-input mistake rather than a role the service may not pass"), with the refused table at lines 57-102; tests/all/channel_role_account.rs:8 covers the account segment on "every operation that takes the member".

## Steps to Reproduce

1. Read services/service-com-amazonaws-supportapp-supportapp/src/state.rs:5-13 and 535-537: the shared-rule module states the length-before-pass-role order once and says the handlers must "agree on one implementation of each instead of carrying copies that can drift apart".
2. Read src/ops/create_slack_channel_configuration.rs:210-235 and src/ops/update_slack_channel_configuration.rs:228-258: both handlers re-implement that order, each deciding independently which helper runs first and which modelled error class its message travels in.
3. Swap the two `if let` blocks in ONE of the two handlers (or delete the length block from one): the crate still compiles, state.rs is unchanged, and that operation now answers 403 AccessDeniedException where its sibling answers 400 ValidationException for an over-long ARN naming another account — the divergence the single shared rule was supposed to prevent.
4. Fix: expose one state-side entry point that returns which refusal applies (message + class) in the documented order, and have each handler map that answer onto its own error enum. Run the crate's test target: tests/all/channel_role_pass_role_form.rs and tests/all/channel_role_account.rs must pass unchanged, with identical statuses, error types and messages.
