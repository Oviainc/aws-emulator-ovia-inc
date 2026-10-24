Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 1f773e23b3e0bd2b9ce8ba90d57212de869e61a4

# Retention-rule store is not partitioned by Region: rules created through one Region's endpoint are listed, read and mutated through every other

_Filed: 2026-10-24T08:46:21Z_
_Operation: ListRules | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `1f773e23b3e0bd2b9ce8ba90d57212de869e61a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Recycle Bin is a regional service: a retention rule exists only in the Region it was created in. ListRules against eu-west-1 must not report a rule created in us-east-1, GetRule/UpdateRule/DeleteRule/LockRule against another Region's endpoint must answer ResourceNotFoundException for it, and the "retention rules per Region" quota is counted per Region.

The emulator keeps ONE global collection. `ComAmazonawsRbinAmazonrecyclebinState` holds `rules: BTreeMap<String, Rule>` keyed by the 11-character identifier alone, plus an `arn_index`, and no Region key anywhere. `crate::state::request_region(ctx.parts())` (the SigV4 credential scope) is read in exactly ONE place — `create_rule.rs`, only to compose the stored `RuleArn` — and in no reader. Consequences a client can observe:

* ListRules signed for Region B returns rules created in Region A, and each leaked `RuleSummary.RuleArn` even carries Region A in its ARN segment, proving the rule belongs to another Region.
* GetRule / UpdateRule / LockRule / UnlockRule / DeleteRule resolve an identifier minted in another Region and act on it (200/204), where the real service answers 404 ResourceNotFoundException because that Region holds no such rule.
* The rules-per-Region quota (`MAX_RULES`, documented in state.rs as "The number of retention rules this Region holds") is a count of the GLOBAL map, so filling Region A to 1000 rules makes CreateRule in Region B answer 402 ServiceQuotaExceededException even though Region B holds nothing.

Expected: rules are stored and resolved under (Region, Identifier) — created, listed, read, updated, locked and deleted only through the Region the request is signed for; the per-Region quota counts only that Region's rules. Actual: one global namespace shared by every Region.

## Evidence

Spec/semantics: Recycle Bin retention rules are Regional — the model's own prose says a Region-level rule "retains all deleted resources of the specified resource type in the Region in which the rule is created" (`com.amazonaws.rbin#CreateRuleRequest$ResourceTags`), and `RuleArn` (`^arn:\S+:rbin:[a-z\-0-9]{0,63}:[0-9]{12}:rule/[0-9a-zA-Z]{11}...$`) carries the Region segment, so a rule is identified per Region. The emulator itself words its 404 as "a retention rule the request addressed and this Region does not hold" (state.rs `rule_not_found`) while holding no Region at all.

Handler/state: `services/service-com-amazonaws-rbin-amazonrecyclebin/src/state.rs` — `pub struct ComAmazonawsRbinAmazonrecyclebinState { pub rules: BTreeMap<String, Rule>, pub arn_index: HashMap<String, String>, pub next_identifier_seed: u64 }` (no Region dimension); `Rule` has `rule_arn` but no `region` field; `page_from()` ranges over the whole map. `src/ops/list_rules.rs:199-229` — `let store = state.lock(); for (identifier, rule) in store.page_from(...)` filters only on `ResourceType`, `LockState`, `ResourceTags` and `ExcludeResourceTags`. `src/ops/get_rule.rs:181` / `update_rule.rs:208` / `lock_rule.rs:186` / `unlock_rule.rs:186` / `delete_rule.rs:150` — `store.rules.get(_mut)(&input.identifier)` with no Region check. `src/ops/create_rule.rs:229,254,268` — `request_region` is used only for the ARN, and the quota check is `store.rules.len() >= MAX_RULES` over the global map. grep for `region` under `src/ops` matches only those two lines in create_rule.rs.

Test coverage: none. Every suite signs a single Region (`const REGION: &str = "eu-west-2"` in tests/all/retention_rule_lifecycle.rs, resource_tags.rs, tag_arn_label_encoding.rs); no test issues two requests signed for different Regions. `tests/all/error_response_bodies.rs::an_addressed_rule_this_region_does_not_hold_is_reported_as_not_existing` only rewrites the Region segment of an ARN passed to the tag operations (which resolve by whole-ARN string), and never addresses a rule by Identifier from a second Region.

## Steps to Reproduce

1. POST /rules signed with a credential scope naming us-east-1 (body {"ResourceType":"EBS_SNAPSHOT","RetentionPeriod":{"RetentionPeriodValue":7,"RetentionPeriodUnit":"DAYS"}}) — 201; note Identifier and RuleArn (ARN carries us-east-1).
2. POST /list-rules signed with a credential scope naming eu-west-1, body {"ResourceType":"EBS_SNAPSHOT"} — expect Rules to be empty (eu-west-1 holds no rule); actual the us-east-1 rule is returned, its RuleArn still naming us-east-1.
3. GET /rules/{Identifier} signed for eu-west-1 — expect 404 ResourceNotFoundException ("Recycle Bin Rule '…' does not exist."); actual HTTP 200 with the rule created in us-east-1.
4. DELETE /rules/{Identifier} signed for eu-west-1 — expect 404; actual 204, and the rule is gone from us-east-1 too.
5. Create rules in Region A until CreateRule answers 402 ServiceQuotaExceededException, then POST /rules signed for Region B — expect 201 (Region B holds no rules); actual 402, because the per-Region quota is counted over the single global map.
