Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: d6c12c566adecd4ccd11eee5271004209bddbecc

# clientToken replay ignores the request body: a token reused with different parameters returns the stored configuration instead of ConflictException

_Filed: 2026-10-24T15:37:55Z_
_Operation: CreateTargetAccountConfiguration | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d6c12c566adecd4ccd11eee5271004209bddbecc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`clientToken` on CreateTargetAccountConfiguration is an `@idempotencyToken`, so it identifies a REQUEST, not just a resource. The handler compares only the token value, never the request it was first used with:

`src/ops/create_target_account_configuration.rs` (lines ~199-218) looks up the existing configuration for `accountId` and calls `existing.is_replay_of(input.client_token.as_deref())`; `src/state.rs::TargetAccountConfigurationRecord::is_replay_of` (lines ~728-733) is `stored_token == offered_token` and nothing else.

Two observable consequences:

1. Same template + same accountId + same clientToken but a DIFFERENT `roleArn` (or `description`): the emulator answers HTTP 200 and reports the configuration the FIRST call created, silently discarding the roleArn/description the second request carried. Expected: the token is bound to the first request, so a request that differs in any member is a parameter mismatch and is refused with ConflictException (HTTP 409); nothing is changed. Actual: 200 with the stale body, and a caller that believes it just set a new role is wrong — GetTargetAccountConfiguration keeps reporting the old roleArn.

2. The same clientToken reused for a DIFFERENT accountId on the same template creates a SECOND configuration under that token (the token lives on the per-account record, so cross-account reuse cannot even be noticed). Expected: the reuse of a token for a different request is a conflict, not a second resource.

The crate already implements the correct rule for its other two `@idempotencyToken` creates — `CreateExperimentTemplate` (`template_fingerprint` + `store.replayed(...)` -> `TokenReplay::Mismatch` -> ConflictException) and `StartExperiment` (`start_experiment_fingerprint`) — so the three creates of this one service disagree about what an idempotency token means.

## Evidence

Smithy model: CreateTargetAccountConfiguration.clientToken -> ClientToken with `@idempotencyToken` (len 1..1024, pattern ^[\S]+$); the operation declares ConflictException (errorKind client, httpError 409) "The request could not be processed because of a conflict", which is the modeled channel for a token/parameter mismatch and is currently only ever raised for a duplicate account, never for a token mismatch.
Handler: services/service-com-amazonaws-fis-faultinjectionsimulator/src/ops/create_target_account_configuration.rs lines ~199-218 (replay decided solely by `existing.is_replay_of(input.client_token)`, which returns the stored record and ignores `input.role_arn` / `input.description`); src/state.rs lines ~723-733 (`is_replay_of` compares token strings only) versus src/state.rs `template_fingerprint` / `start_experiment_fingerprint` + `ComAmazonawsFisFaultinjectionsimulatorState::replayed` returning `TokenReplay::Mismatch`, which the other two creates use to answer ConflictException.
Tests: tests/all/control_plane_rules.rs asserts the fingerprint rule for CreateExperimentTemplate (`reusing_a_client_token_for_a_different_create_request_is_a_conflict`) but no test in tests/all/target_account_configurations.rs or tests/all/target_account_configuration_errors.rs exercises a target-account-configuration create that reuses one token with different parameters, or one token across two accounts.

## Steps to Reproduce

1. Boot the emulator and point a botocore `fis` client at it.
2. create_experiment_template(... experimentOptions={'accountTargeting': 'multi-account'} ...) and keep the returned template id.
3. create_target_account_configuration(experimentTemplateId=tpl, accountId='111122223333', roleArn='arn:aws:iam::111122223333:role/first', clientToken='tok-1') -> 200.
4. Call it again with the SAME clientToken='tok-1' and accountId='111122223333' but roleArn='arn:aws:iam::111122223333:role/second' -> observe HTTP 200 reporting roleArn .../first instead of ConflictException, and get_target_account_configuration still showing .../first.
5. Call create_target_account_configuration with clientToken='tok-1' again but accountId='222233334444' and roleArn='arn:aws:iam::222233334444:role/third' -> observe HTTP 200 and a SECOND configuration created under the already-used token.
