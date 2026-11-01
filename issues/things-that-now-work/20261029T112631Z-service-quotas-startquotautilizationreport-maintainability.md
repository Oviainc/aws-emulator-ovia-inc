Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 6eca256d767263dcce8cda9bce6eb5315c5aad3d

# The utilization snapshot re-derives the value-in-force rule instead of calling state's single value_in_force helper

_Filed: 2026-10-29T11:26:31Z_
_Operation: StartQuotaUtilizationReport | Class: maintainability_

> _Note: this issue was identified against the code base at commit `6eca256d767263dcce8cda9bce6eb5315c5aad3d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate single-homes one rule — "the value in force for a quota is the applied overlay when a decided request wrote one, else the AWS default" — in `ComAmazonawsServicequotasServicequotasv20190624State::value_in_force`, and every reader of a quota's value goes through it: `applied_value` (the public wrapper `RequestServiceQuotaIncrease` compares a `DesiredValue` against) and `applied_quota_view` (what `GetServiceQuota` / `ListServiceQuotas` report).

`utilization_snapshot`, the function that builds every `QuotaUtilizationInfo` record a quota utilization report publishes, is the ONE reader that does not call it. It reaches past the helper to the raw overlay lookup `applied_entry` and writes its own copy of the fallback (`.map_or(default.value, |entry| entry.value)`).

So the same rule is implemented twice in this crate's own code, and the two copies are already not the same expression: `value_in_force` resolves the overlay through the context hierarchy `contexts_in_force` (the resource's own value, then the value applied to every resource with none of its own, then the account-wide value, then the AWS default), while the report's copy resolves the exact `None` key only. The two agree TODAY for the account-wide read the report makes, so no response changes; they will silently disagree the moment the fallback chain changes (the `*`/every-resource rung in `contexts_in_force` is exactly the kind of change that has already been added once), and then a report's `AppliedValue` would stop agreeing with the `GetServiceQuota` read of the same pair — which is what `tests/all/utilization_report.rs` asserts.

The fix is to call `self.value_in_force(region, default, None)` from `utilization_snapshot`: it is a method on the same `impl`, it returns the identical `f64` for a `None` context, and no response or test changes.

## Evidence

The single-homed rule — `src/state.rs:923-930`:
```
    /// [`Self::applied_value`] for a quota whose catalog row the caller already holds,
    /// so a view does not look the same row up twice.
    fn value_in_force(&self, region: &str, default: &DefaultQuota, context: Option<&str>) -> f64 {
        contexts_in_force(context)
            .into_iter()
            .find_map(|candidate| self.applied_entry(region, default, candidate))
            .map_or(default.value, |applied| applied.value)
    }
```
Its two callers — `src/state.rs:920` (`applied_value`, reached from `src/ops/request_service_quota_increase.rs:230-237`) and `src/state.rs:944`:
```
        quota.value = Some(self.value_in_force(region, default, context));
```

The second copy — `src/state.rs:1539-1541`, inside `utilization_snapshot`:
```
                let applied = self
                    .applied_entry(region, default, None)
                    .map_or(default.value, |entry| entry.value);
```
where `applied_entry` (`src/state.rs:1688-1691`) is documented as the raw overlay lookup, not the rule: "The applied overlay in force for a quota in one context, read from where that quota's scope keeps it" — it returns `Option<&AppliedQuota>` and knows nothing about the default fallback or the context hierarchy.

The module doc states the rule once, `src/state.rs:12-14`: "the **applied quota overlay** (`GetServiceQuota`, `ListServiceQuotas`) — written only when a quota increase request is decided, and read with the default as the fallback."

The call path that makes this `StartQuotaUtilizationReport`'s: `src/ops/start_quota_utilization_report.rs:188` (`quotas_state.start_utilization_report(&region, now, &usage)`) -> `src/state.rs:1450` (`let quotas = self.utilization_snapshot(region, usage);`).

Test coverage that pins the behaviour the refactor must not change: `tests/all/utilization_report.rs:428` `every_report_record_agrees_with_the_quota_reads_for_the_same_pair`, asserting at lines 458-462
```
        assert_eq!(
            record["AppliedValue"].as_f64(),
            applied["Value"].as_f64(),
            "case {case}: AppliedValue must be the value in force, got {record}"
        );
```
plus `tests/all/utilization_report.rs:558` `a_report_is_a_snapshot_of_the_quotas_when_it_was_started` (AppliedValue at line 648) and `tests/all/quota_utilization_usage.rs:221-225` (the `AppliedValue` denominator helper).

## Steps to Reproduce

1. Open `services/service-com-amazonaws-servicequotas-servicequotasv20190624/src/state.rs` and read `value_in_force` (lines 923-930): the overlay-else-default rule, resolved through `contexts_in_force`.
2. Read its callers at line 920 (`applied_value`) and line 944 (`applied_quota_view`) — every quota read goes through the one helper.
3. Read `utilization_snapshot` at lines 1539-1541: it calls `applied_entry(region, default, None)` and re-writes the fallback as `.map_or(default.value, |entry| entry.value)` instead of calling `value_in_force(region, default, None)`.
4. Confirm the duplicate is reached by this operation: `src/ops/start_quota_utilization_report.rs:188` -> `start_utilization_report` -> `src/state.rs:1450` `utilization_snapshot`.
5. Replace lines 1539-1541 with `let applied = self.value_in_force(region, default, None);` and run `tests/all/utilization_report.rs` and `tests/all/quota_utilization_usage.rs`: every assertion, including the `AppliedValue == GetServiceQuota Value` equality at lines 458-462, still passes, showing the two copies are one rule.
