Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 1547d46e2415f486cf41cf952181485c961e01df

# The target-account-configuration page walk is implemented twice (ListTargetAccountConfigurations and ListExperimentTargetAccountConfigurations), so the two copies can drift

_Filed: 2026-10-25T18:43:22Z_
_Operation: ListTargetAccountConfigurations | Class: maintainability_

> _Note: this issue was identified against the code base at commit `1547d46e2415f486cf41cf952181485c961e01df`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Both target-account-configuration list operations page over a `BTreeMap<String, TargetAccountConfigurationRecord>` keyed by `accountId`, and each one hand-rolls the same five-step rule in its own handler: decode the `nextToken` through `crate::state::target_account_page_token_account` and refuse an undecodable one with `crate::state::invalid_next_token()`; size the page with `crate::state::page_size`; skip every entry whose key sorts before the resumed account id; stop once the page is full and mint the next token with `crate::state::target_account_page_token(account_id)`; otherwise push the record's summary.

The two copies are character-identical apart from which map they walk (`template.target_account_configurations` vs `experiment.target_account_configurations`), which not-found error they raise, where the page size comes from (`input.max_results` vs `None`, since the experiment-scoped operation models no `maxResults`) and which summary they build (`to_summary()` vs `to_experiment_summary()`). Because the walk is written out twice, a fix to one — a different resume comparison, a token minted from a different entry, an off-by-one in the cut — silently leaves the other paging differently, which is exactly the drift the crate already removed once for the template-admissibility sequence (see the fixed issue 'The template-admissibility refusal sequence is implemented twice (create and update)').

Stating it once (a helper beside the existing token codec in `state.rs`, taking the map, the decoded resume point, the page size and a record->summary mapper, and handing back the page and the next token) leaves every response byte-for-byte identical and both existing test files passing unchanged.

## Evidence

Copy 1 — services/service-com-amazonaws-fis-faultinjectionsimulator/src/ops/list_target_account_configurations.rs:162-208:
  "let resume_at = match input.next_token.as_deref() { None => None, Some(token) => match crate::state::target_account_page_token_account(token) { Some(account_id) => Some(account_id.to_string()), None => return Err(ListTargetAccountConfigurationsErrorModeled::ValidationException(crate::state::invalid_next_token()).into()) } };
   let page_size = crate::state::page_size(input.max_results);
   ...
   for (account_id, record) in &template.target_account_configurations {
       if resume_at.as_deref().is_some_and(|from| account_id.as_str() < from) { continue; }
       if target_account_configurations.len() >= page_size { next_token = Some(crate::state::target_account_page_token(account_id)); break; }
       target_account_configurations.push(record.to_summary());
   }"

Copy 2 — src/ops/list_experiment_target_account_configurations.rs:161-213:
  "let resume_at = match input.next_token.as_deref() { None => None, Some(token) => match crate::state::target_account_page_token_account(token) { Some(account_id) => Some(account_id.to_string()), None => return Err(ListExperimentTargetAccountConfigurationsErrorModeled::ValidationException(crate::state::invalid_next_token()).into()) } };
   // This operation models no `maxResults`, so the page is the largest one this
   // service serves rather than a size the caller chose.
   let page_size = crate::state::page_size(None);
   ...
   for (account_id, record) in &experiment.target_account_configurations {
       if resume_at.as_deref().is_some_and(|from| account_id.as_str() < from) { continue; }
       if target_account_configurations.len() >= page_size { next_token = Some(crate::state::target_account_page_token(account_id)); break; }
       target_account_configurations.push(record.to_experiment_summary());
   }"

Both walk the same record type and the same key space — src/state.rs:1174-1179 (`pub target_account_configurations: BTreeMap<String, TargetAccountConfigurationRecord>` on the template, "A `BTreeMap` is what gives `ListTargetAccountConfigurations` a stable total order") and state.rs:1464-1470 (the experiment's snapshot of the same map).

Tests that pin the behaviour and must stay unchanged: tests/all/list_target_account_configurations.rs and tests/all/list_experiment_target_account_configurations.rs (plus tests/all/experiment_target_account_configurations.rs and tests/all/target_account_configurations.rs for the records themselves).

## Steps to Reproduce

1. Open services/service-com-amazonaws-fis-faultinjectionsimulator/src/ops/list_target_account_configurations.rs and read lines 162-208.
2. Open src/ops/list_experiment_target_account_configurations.rs and read lines 161-213 — the token decode, the resume comparison, the page cut and the token mint are the same code over a different map.
3. Change the resume comparison in one file (for example to `<=`) and run `cargo test -p service-com-amazonaws-fis-faultinjectionsimulator`: only that operation's test file fails, showing the two paging rules are independent copies.
4. Confirm the fix is behaviour-preserving: after extracting one shared page walk, both tests/all/list_target_account_configurations.rs and tests/all/list_experiment_target_account_configurations.rs must pass with no edits.
