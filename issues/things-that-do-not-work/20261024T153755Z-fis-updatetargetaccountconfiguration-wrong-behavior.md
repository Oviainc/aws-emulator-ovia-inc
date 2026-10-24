Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: d6c12c566adecd4ccd11eee5271004209bddbecc

# An explicitly empty description is stored as a present empty string, so a description can never be cleared the way FIS clears it

_Filed: 2026-10-24T15:37:55Z_
_Operation: UpdateTargetAccountConfiguration | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d6c12c566adecd4ccd11eee5271004209bddbecc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateTargetAccountConfiguration.description` is modeled `@length(min: 0, max: 512)`, `pattern ^[\s\S]*$`, so an empty string is a legal value and is how a caller clears a description. Real FIS normalizes an explicitly empty description to ABSENT: the configuration comes back with no `description` member at all (the same representation as one that never had a description). The emulator instead keeps a third representation AWS does not have.

`src/state.rs::TargetAccountConfigurationRecord::apply_update` (lines ~748-755) does `self.description = Some(description.to_string())` for any carried value, including "", and `to_target_account_configuration` / `to_summary` / `to_experiment_summary` then serialize `description: ""` as a present member.

Expected: update with description='' leaves the configuration reporting NO description member from GetTargetAccountConfiguration, ListTargetAccountConfigurations and GetExperimentTargetAccountConfiguration. Actual: those reads report `"description": ""`, so a client that branches on the presence of the member (or diffs the configuration it just wrote) disagrees with AWS, and a description it tried to clear still looks set.

Note for the fixer: the CREATE path of the same normalization rule is Cause 4 of the open issue 20261022T063355Z-fis-8-fidelity-divergences-...; this issue is the UPDATE handler, which that issue carries no check for. Both paths have to normalize the same way.

## Evidence

Smithy model: UpdateTargetAccountConfiguration.description -> TargetAccountConfigurationDescription (@length 0..512, @pattern ^[\s\S]*$) and TargetAccountConfiguration.description with the same bounds — the model permits an empty string, so the empty-vs-absent collapse is a server rule. Recorded AWS behavior for this resource is cited in the open issue 20261022T063355Z (trace fis.target-account-configuration-empty-vs-absent-role-arn-preserved): on real FIS an explicitly empty-string description is normalized exactly like an omitted one and the 'description' key comes back ABSENT, while a non-empty description round-trips byte-exact.
Handler/state: services/service-com-amazonaws-fis-faultinjectionsimulator/src/ops/update_target_account_configuration.rs line ~198 (`record.apply_update(input.role_arn.as_deref(), input.description.as_deref())`); src/state.rs lines ~735-755 (`apply_update` stores `Some("")`, with the doc comment stating this on purpose: "An empty `description` IS carried ... so it replaces the stored description with the empty string rather than counting as 'omitted'"); src/state.rs lines ~757-795 (all three renderings pass `self.description.clone()` straight through).
Tests: tests/all/target_account_configurations.rs round-trips descriptions and tests/all/target_account_configuration_errors.rs asserts the emulator's current empty-vs-absent distinction; no test asserts AWS's normalization, and no filed issue covers the update path.

## Steps to Reproduce

1. Boot the emulator and point a botocore `fis` client at it.
2. create_experiment_template(... experimentOptions={'accountTargeting': 'multi-account'} ...) and keep the template id.
3. create_target_account_configuration(experimentTemplateId=tpl, accountId='111122223333', roleArn='arn:aws:iam::111122223333:role/r', description='initial').
4. update_target_account_configuration(experimentTemplateId=tpl, accountId='111122223333', description='') -> observe the response body carrying "description": "".
5. get_target_account_configuration(experimentTemplateId=tpl, accountId='111122223333') and list_target_account_configurations(experimentTemplateId=tpl) -> observe `description` present as an empty string instead of being absent.
