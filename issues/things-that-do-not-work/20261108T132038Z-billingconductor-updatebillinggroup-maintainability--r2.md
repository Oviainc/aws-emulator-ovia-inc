Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 77fc38481d0944218df9b5c2964437ecad54bbf8

# The members that decide whether a billing-group write changed anything are enumerated twice — once in BillingGroupRecord::current_version and again inline in UpdateBillingGroup — unlike the three sibling resources that keep that list in one place

_Filed: 2026-11-08T13:20:38Z_
_Operation: UpdateBillingGroup | Class: maintainability_

> _Note: this issue was identified against the code base at commit `77fc38481d0944218df9b5c2964437ecad54bbf8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every versioned resource of this crate decides "did this write modify the resource?" by comparing exactly the members that ONE GENERATION of the resource carries, i.e. the members `VersionedResource::current_version` projects. Three of the four resources expose that comparison as a method on the record next to `current_version`, and each of the three states in its own doc comment that the list is named there once SO THE TWO CANNOT DRIFT:

* `PricingPlanRecord::holds_same_members_as` (state.rs:1240), used by `UpdatePricingPlan` (update_pricing_plan.rs:246)
* `PricingRuleRecord::holds_same_members_as` (state.rs:1421), used by `UpdatePricingRule` (update_pricing_rule.rs:251)
* `CustomLineItemRecord::holds_same_members_as` (state.rs:2320), used by `UpdateCustomLineItem` (update_custom_line_item.rs:246)

`BillingGroupRecord` is the one exception: it has no such method, and the member list of `BillingGroupVersion` is written out a SECOND time, as an inline boolean expression inside the `UpdateBillingGroup` handler (update_billing_group.rs:375-379), in a different file from the `current_version` projection it has to agree with (state.rs:1669-1678).

The two copies agree today, so there is no behavioural impact: the handler's expression covers every member of `BillingGroupVersion` (name, description, pricing_plan_arn, auto_associate, and requested_status through the derived `status()`), and `last_modified_time_millis` is the instant the comparison exists to decide. The defect is the drift the sibling records' own comments name: a member added to `BillingGroupVersion`/`current_version` — the shape that exists precisely so period-scoped reads report the group as it stood — silently keeps the handler's `changed` verdict stale, so a write that really did change the group would move no `LastModifiedTime` and freeze no generation, which is the bug class `...-billingconductor-updatebillinggroup-wrong-behavior.md` and `...-billingconductor-updatepricingplan-wrong-behavior.md` were both filed for.

Fix: give `BillingGroupRecord` a `holds_same_members_as(&self, other: &Self)` beside `current_version`, comparing the same five members the handler compares today — including `self.status() != other.status()` rather than the raw `requested_status`, because the derived status is what a read reports (this is the rule `...-billingconductor-updatebillinggroup-wrong-behavior.md` fixed) — and have the handler call `!updated.holds_same_members_as(&stored)`, exactly as the three sibling updaters do. Every response and every existing test stays byte-for-byte the same.

## Evidence

The second copy, in the handler — `services/service-com-amazonaws-billingconductor-awsbillingconductor/src/ops/update_billing_group.rs:375-379`:
```rust
        let changed = updated.name != stored.name
            || updated.description != stored.description
            || updated.pricing_plan_arn != stored.pricing_plan_arn
            || updated.status() != stored.status()
            || updated.auto_associate != stored.auto_associate;
```

The first copy, the generation projection it must agree with — `src/state.rs:1669-1678`:
```rust
    fn current_version(&self) -> BillingGroupVersion {
        BillingGroupVersion {
            name: self.name.clone(),
            description: self.description.clone(),
            pricing_plan_arn: self.pricing_plan_arn.clone(),
            auto_associate: self.auto_associate,
            requested_status: self.requested_status.clone(),
            last_modified_time_millis: self.last_modified_time_millis,
        }
    }
```

The rule the three sibling records state — `src/state.rs:1232-1242` (`PricingPlanRecord`):
```rust
    /// Do this record and `other` hold the SAME values for every member a write can
    /// change — exactly the members one generation of the plan carries
    /// ([`VersionedResource::current_version`]), named here once so the two cannot drift?
    ...
    pub fn holds_same_members_as(&self, other: &Self) -> bool {
        self.name == other.name && self.description == other.description
    }
```
`src/state.rs:1421-1427` (`PricingRuleRecord::holds_same_members_as`, same doc sentence) and `src/state.rs:2320-2329` (`CustomLineItemRecord::holds_same_members_as`, "the rule the sibling resources' updaters apply").

The three sibling handlers calling it, where `UpdateBillingGroup` inlines instead:
* `src/ops/update_pricing_plan.rs:246` — `let changed = !updated.holds_same_members_as(&stored);`
* `src/ops/update_pricing_rule.rs:251` — `let changed = !updated.holds_same_members_as(&stored);`
* `src/ops/update_custom_line_item.rs:246` — `let changed = !updated.holds_same_members_as(&stored);`

Test coverage that pins the behaviour the fix must not change: `tests/all/update_billing_group_no_op_generations.rs:7-19` ("RULE: whether a call modified the group is decided by comparing the values a READ of it reports before and after... so a request naming only values the group already reports modifies nothing, which means it leaves `LastModifiedTime` where it was and opens no generation of the group", driven per member class through the real `restJson1` wire), `tests/all/update_billing_group.rs` ("owns the `LastModifiedTime` half of the rule over every member class a request can name"), and `tests/all/versioned_resource_generations.rs`.

## Steps to Reproduce

1. Read `src/ops/update_billing_group.rs:375-379`: the five members that decide `changed` are written out inline in the handler.
2. Read `src/state.rs:1669-1678`: `BillingGroupRecord::current_version` enumerates the same five members (plus the instant the comparison decides) in a different file.
3. Read `src/state.rs:1240`, `:1421` and `:2320`: the three sibling records each expose `holds_same_members_as` beside their own `current_version`, documented as 'exactly the members one generation ... carries ([`VersionedResource::current_version`]), named here once so the two cannot drift'. `BillingGroupRecord` (impl block at `src/state.rs:1573`) has no such method.
4. Read `src/ops/update_pricing_plan.rs:246`, `src/ops/update_pricing_rule.rs:251`, `src/ops/update_custom_line_item.rs:246`: the three sibling updaters call that one method; `UpdateBillingGroup` is the only updater that does not.
5. Add a member to `BillingGroupVersion` and to `BillingGroupRecord::current_version`: the handler's `changed` expression still compiles and still answers from the old member list, so a write changing only the new member reports `unchanged`, moves no `LastModifiedTime` and freezes no generation.
