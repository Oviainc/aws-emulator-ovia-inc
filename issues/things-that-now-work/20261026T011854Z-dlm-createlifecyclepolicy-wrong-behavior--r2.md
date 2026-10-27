Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: d25a8df08abf64a2b119d49e5e317e50f9894e06

# A default-policy create whose PolicyDetails omits PolicyLanguage: SIMPLIFIED is stored with the custom-policy materialization

_Filed: 2026-10-26T01:18:54Z_
_Operation: CreateLifecyclePolicy | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d25a8df08abf64a2b119d49e5e317e50f9894e06`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateLifecyclePolicy` decides the KIND of policy it is creating from two members (`DefaultPolicy` or a `SIMPLIFIED` `PolicyDetails$PolicyLanguage`), but it hands the submitted `PolicyDetails` to `crate::state::materialized_policy_details`, which decides the same question from `PolicyLanguage` ALONE. For a request that is a default-policy request because it names `DefaultPolicy`, yet carries a `PolicyDetails` that does not state `PolicyLanguage: SIMPLIFIED` (e.g. states no language at all), the two disagree: the record is stored with `default_policy: true` while its configuration structure is stamped with the CUSTOM-policy materialization — `PolicyLanguage: STANDARD`, `PolicyType: EBS_SNAPSHOT_MANAGEMENT`, and `ResourceLocations: ["CLOUD"]`.

Two consequences, both from this one disagreement:

1. The stored `PolicyLanguage` (`STANDARD`) contradicts the record's own `default_policy: true` flag, and `LifecyclePolicyRecord::effective_policy_details` then OVERWRITES it with `SIMPLIFIED` on every read — so the handler writes a value no read can ever report, which is exactly the shadowing the state module's own design notes say the write paths resolve away.
2. `ResourceLocations` is NOT overwritten on read, so a DEFAULT policy reports a member the model documents `[Custom snapshot and AMI policies only]`. Worse, it makes two requests that create the same kind of policy read back differently: `{"DefaultPolicy": "VOLUME"}` reports no `ResourceLocations`, while `{"DefaultPolicy": "VOLUME", "PolicyDetails": {}}` reports `ResourceLocations: ["CLOUD"]`.

The guard in `materialized_policy_details` exists precisely to skip the custom stamping for a default policy's configuration (its comment says so), but it keys on the configuration's language instead of on the policy's kind, which only the handler knows.

This is the CREATE path. The open issue `20261025T184225Z-dlm-updatelifecyclepolicy-parameter-ignored` covers the Update path and explicitly asserts the create path applies the rule correctly, so a fixer working from it would not touch `create_lifecycle_policy.rs`.

## Evidence

Model — `smithy_query(query="doc", shape="PolicyDetails", member="ResourceLocations")`: "**[Custom snapshot and AMI policies only]** The location of the resources to backup."

The handler's own notion of the policy's kind — `src/ops/create_lifecycle_policy.rs:203-204`:
```
let is_default = input.default_policy.is_some()
    || policy_language_is_simplified(input.policy_details.as_ref());
```
stored at `src/ops/create_lifecycle_policy.rs:397`: `default_policy: is_default,`

...but the configuration is materialized without that fact — `src/ops/create_lifecycle_policy.rs:358-360`:
```
let mut policy_details = input
    .policy_details
    .map(crate::state::materialized_policy_details);
```

and `materialized_policy_details` re-derives the kind from the language alone — `src/state.rs:321-331`:
```
pub fn materialized_policy_details(mut details: PolicyDetails) -> PolicyDetails {
    if details.policy_language.as_deref() == Some(SIMPLIFIED_POLICY_LANGUAGE) {
        // A DEFAULT policy's configuration. Its `[Default policies only]`
        // defaults are the documented ones, which
        // [`LifecyclePolicyRecord::effective_policy_details`] reports from the
        // record's own body parameters.
        return details;
    }
    details
        .policy_language
        .get_or_insert_with(|| STANDARD_POLICY_LANGUAGE.to_string());
```
and then stamps the custom-only member it says is custom-only — `src/state.rs:353-355`: "// Resource locations and schedules are `[Custom snapshot and AMI policies // only]`; an event-based policy configures an `EventSource` and `Actions` // instead, so it carries neither." followed by `src/state.rs:364-366`:
```
details
    .resource_locations
    .get_or_insert_with(|| vec![CLOUD_LOCATION.to_string()]);
```

The read then overwrites the language it just stored but leaves the locations — `src/state.rs:502-507`:
```
pub fn effective_policy_details(&self) -> Option<PolicyDetails> {
    if !self.default_policy {
        return self.policy_details.clone();
    }
    let mut details = self.policy_details.clone().unwrap_or_default();
    details.policy_language = Some(SIMPLIFIED_POLICY_LANGUAGE.to_string());
```

The crate's statement of the rule being broken — `src/state.rs:80-88`: "/// The `PolicyLanguage` value that marks a policy as a DEFAULT policy: /// `PolicyDetails$PolicyLanguage` documents `SIMPLIFIED` as \"To create a default /// policy\" and `STANDARD` as \"To create a custom policy\"." — and `src/state.rs:393-396`: "Two stored copies of one member would need a precedence between them, and any fixed precedence makes a write to the losing place unobservable, so the write paths resolve the two into this ONE value and the read reports it."

Test coverage: NOT pinned. Every test that combines `DefaultPolicy` with a `PolicyDetails` restates the language — `tests/all/policy_type.rs:327-337` ("a default {default_type} policy with a SIMPLIFIED structure", body `{"DefaultPolicy": default_type, "PolicyDetails": {"PolicyLanguage": "SIMPLIFIED"}}`) and `tests/all/default_policy_configuration_precedence.rs:430-443` (`"PolicyLanguage": DEFAULT_POLICY_LANGUAGE`). The cases in `tests/all/default_policy_only_parameters.rs:349-366` that use a `PolicyDetails` "stating no PolicyLanguage" all omit `DefaultPolicy`, so they take the refused `!is_default` branch instead. `tests/all/policy_read_projection.rs:499-513` pins `ResourceLocations` for CUSTOM policies only.

## Steps to Reproduce

1. POST /policies (SigV4-signed for dlm) with {"ExecutionRoleArn": <emulator-account role ARN>, "Description": "default volume policy", "State": "ENABLED", "DefaultPolicy": "VOLUME", "PolicyDetails": {}} — observe HTTP 200 and note the PolicyId
2. GET /policies/{PolicyId} — Policy.DefaultPolicy is true and Policy.PolicyDetails.PolicyLanguage reports "SIMPLIFIED" (the STANDARD the create stamped into the record is unobservable), yet Policy.PolicyDetails.ResourceLocations reports ["CLOUD"], a member documented [Custom snapshot and AMI policies only]
3. POST /policies again with the same body but WITHOUT the "PolicyDetails": {} member, then GET it — the same kind of default VOLUME policy now reports no ResourceLocations, so two creates of one kind of policy read back with different configuration members
