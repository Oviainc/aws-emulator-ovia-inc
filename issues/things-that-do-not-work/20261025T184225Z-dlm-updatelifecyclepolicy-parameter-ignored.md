Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: 1547d46e2415f486cf41cf952181485c961e01df

# UpdateLifecyclePolicy never reads PolicyDetails.PolicyLanguage, so a SIMPLIFIED configuration stays custom and a STANDARD one is reported SIMPLIFIED

_Filed: 2026-10-25T18:42:25Z_
_Operation: UpdateLifecyclePolicy | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `1547d46e2415f486cf41cf952181485c961e01df`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`PolicyDetails$PolicyLanguage` is the member that says which KIND of policy a configuration describes, and `CreateLifecyclePolicy`'s handler uses it exactly that way: a `PolicyDetails` whose `PolicyLanguage` is `SIMPLIFIED` makes the created record a DEFAULT policy, anything else a custom one. `UpdateLifecyclePolicy` stores the same member and never consults it — the word `policy_language` does not appear anywhere in `src/ops/update_lifecycle_policy.rs` — so the kind of a policy is frozen at create while the language inside the stored configuration is free to contradict it. Two observable consequences, both from the same omission:

(1) Update a CUSTOM policy with `PolicyDetails {PolicyLanguage: "SIMPLIFIED", ResourceTypes: [...], TargetTags: [...], Schedules: [...]}`. Nothing refuses it: the `[Default policies only]` refusal finds no default-only member stated, and `omitted_configuration_members` finds all three custom members present. The handler then stores it through `materialized_policy_details`, which RETURNS EARLY for a `SIMPLIFIED` language and therefore stamps none of the members every other custom policy's record carries (`PolicyType`, `ResourceLocations`, per-schedule `CopyTags`, the two interval zeros). The record keeps `default_policy == false`, so `effective_policy_details` reports the structure verbatim: a 200-answered policy that reads back with `DefaultPolicy: false` beside `PolicyLanguage: SIMPLIFIED`, with no `PolicyType` and no `ResourceLocations` — while `GetLifecyclePolicies` still reports `PolicyType: EBS_SNAPSHOT_MANAGEMENT` for it (from `policy_type()`'s fallback), so the two read projections of one policy disagree. The very same body sent to `CreateLifecyclePolicy` instead produces `DefaultPolicy: true`.

(2) Update a DEFAULT policy with a `PolicyDetails` stating `PolicyLanguage: "STANDARD"`. The handler accepts it, `materialized_policy_details` stamps the custom-policy members onto it, `replace_default_policy_configuration` wipes the policy's stored cadence back to the defaults, and then `effective_policy_details` OVERWRITES the stated language with `SIMPLIFIED` on every read. The caller's `STANDARD` is unobservable, and the write it accompanied silently reset `CreateInterval`/`RetainInterval`/`CopyTags`/`ExtendDeletion`.

In both directions the update is answered 200 and leaves a record whose stored `PolicyLanguage` and stored kind contradict each other, which is the one classification the crate derives from this member on the create path.

## Evidence

Model — `smithy_query(query="doc", shape="PolicyDetails", member="PolicyLanguage")`: "The type of policy to create. Specify one of the following: `SIMPLIFIED` To create a default policy. `STANDARD` To create a custom policy."

The crate's own statement of that rule — `src/state.rs:80-83`: "/// The `PolicyLanguage` value that marks a policy as a DEFAULT policy: /// `PolicyDetails$PolicyLanguage` documents `SIMPLIFIED` as \"To create a default /// policy\" and `STANDARD` as \"To create a custom policy\"."

Create applies it — `src/ops/create_lifecycle_policy.rs:193-204`: "// A default policy is one created either by naming `DefaultPolicy`, or by // a `PolicyDetails` whose `PolicyLanguage` is `SIMPLIFIED` (\"To create a // default policy\"); everything else is a custom policy." ... "let is_default = input.default_policy.is_some() || policy_language_is_simplified(input.policy_details.as_ref());", stored at `src/ops/create_lifecycle_policy.rs:397`: "default_policy: is_default,".

Update does not — `src/ops/update_lifecycle_policy.rs` reads only the STORED flag (`:228` "if !policy.default_policy {" and `:271` "if policy.default_policy && input.policy_details.is_some() {") and then stores the submitted structure unexamined, `:336-340`: "let mut details = crate::state::materialized_policy_details(policy_details); let configuration = crate::state::DefaultPolicyConfiguration::taken_from_policy_details(&mut details); policy.replace_default_policy_configuration(configuration); policy.policy_details = Some(details);". A grep of `policy_language` over `src/` matches `state.rs:322,330,507` and `create_lifecycle_policy.rs:420-422` only — never `update_lifecycle_policy.rs`.

The skipped materialization — `src/state.rs:321-328`: "pub fn materialized_policy_details(mut details: PolicyDetails) -> PolicyDetails { if details.policy_language.as_deref() == Some(SIMPLIFIED_POLICY_LANGUAGE) { // A DEFAULT policy's configuration ... return details; }".

The read that contradicts the stored language — `src/state.rs:503-507`: "if !self.default_policy { return self.policy_details.clone(); } let mut details = self.policy_details.clone().unwrap_or_default(); details.policy_language = Some(SIMPLIFIED_POLICY_LANGUAGE.to_string());" and `src/state.rs:568-571` (`policy_type()` falling back to `DEFAULT_POLICY_TYPE` for a custom record whose stored details carry no `PolicyType`).

Test coverage: `tests/all/lifecycle_policy_crud.rs:1516-1521` pins the create-side rule — "case {case:?}: both ways of stating it make a DEFAULT policy" for `{"PolicyDetails": {"PolicyLanguage": "SIMPLIFIED", "ResourceType": "VOLUME", ...}}` (lines 1499-1504); `tests/all/policy_read_projection.rs:486-489` pins "a custom policy's PolicyLanguage is {CUSTOM_POLICY_LANGUAGE:?}" (`STANDARD`). No update test states a language that contradicts the stored policy: `tests/all/default_policy_configuration_precedence.rs:215-225` (`update_request`) always restates `"PolicyLanguage": DEFAULT_POLICY_LANGUAGE` on a default policy, so both paths above are unasserted.

## Steps to Reproduce

1. POST /policies (SigV4-signed for dlm) with {"ExecutionRoleArn": <emulator-account role ARN>, "Description": "custom snapshot policy", "State": "ENABLED", "PolicyDetails": {"PolicyType": "EBS_SNAPSHOT_MANAGEMENT", "ResourceTypes": ["VOLUME"], "TargetTags": [{"Key": "backup", "Value": "daily"}], "Schedules": [{"Name": "daily", "CreateRule": {"Interval": 24, "IntervalUnit": "HOURS"}, "RetainRule": {"Count": 7}}]}} and note the PolicyId
2. PATCH /policies/{PolicyId} with {"PolicyDetails": {"PolicyLanguage": "SIMPLIFIED", "ResourceTypes": ["VOLUME"], "TargetTags": [{"Key": "backup", "Value": "daily"}], "Schedules": [{"Name": "daily", "CreateRule": {"Interval": 24, "IntervalUnit": "HOURS"}, "RetainRule": {"Count": 7}}]}} — observe HTTP 200
3. GET /policies/{PolicyId} — Policy.DefaultPolicy is false while Policy.PolicyDetails.PolicyLanguage is "SIMPLIFIED", and PolicyDetails carries no PolicyType and no ResourceLocations (the materialized members every other custom policy reports); GET /policies still reports PolicyType "EBS_SNAPSHOT_MANAGEMENT" for the same policy
4. Send the identical body of step 2 to POST /policies instead and GET it: DefaultPolicy is true, so create and update classify one configuration two ways
5. Now the reverse: POST /policies with {"ExecutionRoleArn": <role ARN>, "Description": "default volume policy", "State": "ENABLED", "DefaultPolicy": "VOLUME", "CreateInterval": 3, "RetainInterval": 14}, then PATCH /policies/{PolicyId} with {"PolicyDetails": {"PolicyLanguage": "STANDARD", "ResourceTypes": ["VOLUME"], "TargetTags": [{"Key": "backup", "Value": "daily"}], "Schedules": [{"Name": "daily", "CreateRule": {"Interval": 24, "IntervalUnit": "HOURS"}, "RetainRule": {"Count": 7}}]}} — HTTP 200
6. GET /policies/{PolicyId} — PolicyDetails.PolicyLanguage reports "SIMPLIFIED" (the stated STANDARD is dropped) and CreateInterval/RetainInterval have silently reverted to 1 and 7
