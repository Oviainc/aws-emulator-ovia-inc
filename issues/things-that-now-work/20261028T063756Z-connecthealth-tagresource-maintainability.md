Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 9cd18327c0c450f2b112925aa5dc4d2c35c16eda

# Domain liveness rule is spelled out four times in state.rs while the sibling Subscription rule is single-homed in `Subscription::is_live()`

_Filed: 2026-10-28T06:37:56Z_
_Operation: TagResource | Class: maintainability_

> _Note: this issue was identified against the code base at commit `9cd18327c0c450f2b112925aa5dc4d2c35c16eda`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`src/state.rs` states the "is this record still live?" rule for a Subscription exactly once, as `Subscription::is_live()`, and every reader calls it. The equivalent rule for a Domain (`status == DomainStatus::Active`) has no such helper: the comparison is written out four separate times inside the same module — in `domain_created_by`, `holds_resource_arn`, `live_domain` and `live_domain_mut` — so the four copies can drift from one another.

The copy in `holds_resource_arn` is the one that contradicts its own doc: the doc sentence directly above it says a Subscription is resolved "through its own reader's liveness (`Self::live_subscription`)", but the body never calls `live_subscription`; it re-implements that reader's two-clause predicate inline (`domain.status == DomainStatus::Active && subscription.is_live()`). The three ARN-addressed tagging operations (`TagResource`, `UntagResource`, `ListTagsForResource`) all resolve a resource through that copy, so if the domain-liveness rule is ever changed in `live_domain`/`live_domain_mut` (the readers `GetSubscription`, `CreateSubscription`, `ListSubscriptions`, `Activate/DeactivateSubscription`, `Start/GetPatientInsightsJob` use) the tagging operations silently keep the old rule and start disagreeing with the resource's own reader — exactly the disagreement the `holds_resource_arn` doc claims cannot happen.

Fix shape: give `Domain` an `is_live()` method beside `Subscription::is_live()` and have all four sites call it (and have `holds_resource_arn` express the subscription clause through the same single-homed liveness it documents). No response and no existing test changes.

## Evidence

Single-homed sibling rule — services/service-com-amazonaws-connecthealth-connecthealth/src/state.rs:726-731:
```
impl Subscription {
    /// Whether this record is still live: one `DeleteDomain` already marked `DELETED`
    /// is gone as far as every reader and every transition is concerned.
    pub fn is_live(&self) -> bool {
        self.status != SubscriptionStatus::Deleted
    }
```

The same rule for a Domain, written out four times in the same file (grep of `DomainStatus::Active` in src/ yields exactly these four, plus the create that sets the field):
- state.rs:832 (`domain_created_by`): `&& domain.status == DomainStatus::Active`
- state.rs:1013 (`holds_resource_arn`): `|| (domain.status == DomainStatus::Active`
- state.rs:1044 (`live_domain`): `.filter(|domain| domain.status == DomainStatus::Active)`
- state.rs:1051 (`live_domain_mut`): `.filter(|domain| domain.status == DomainStatus::Active)`

The doc that names the helper the code does not call — state.rs:1006-1018:
```
/// A `Subscription` is resolved through its own reader's liveness
/// ([`Self::live_subscription`]): a subscription already `DELETED` is gone for that
/// reader and gone here, which covers every subscription of a domain being deleted —
/// `DeleteDomain` marks them all `DELETED` as it records the deletion.
pub fn holds_resource_arn(&self, region: &str, arn: &str) -> bool {
    self.domains_in(region).any(|domain| {
        domain.arn == arn
            || (domain.status == DomainStatus::Active
                && domain
                    .subscriptions
                    .values()
                    .any(|subscription| subscription.is_live() && subscription.arn == arn))
    })
}
```
vs. the reader it says it goes through — state.rs:1081-1091:
```
pub fn live_subscription(&self, region: &str, domain_id: &str, subscription_id: &str) -> Option<&Subscription> {
    self.live_domain(region, domain_id)?
        .subscriptions
        .get(subscription_id)
        .filter(|subscription| subscription.is_live())
}
```

Call sites of the duplicated predicate: src/ops/tag_resource.rs:221 `if !store.holds_resource_arn(&region, &input.resource_arn)`, src/ops/untag_resource.rs:217 and src/ops/list_tags_for_resource.rs:177 (same call); src/ops/get_subscription.rs:164, src/ops/create_subscription.rs:184/202, src/ops/activate_subscription.rs:167, src/ops/deactivate_subscription.rs:166, src/ops/list_subscriptions.rs:187, src/ops/start_patient_insights_job.rs:218/297, src/ops/get_patient_insights_job.rs:219 go through the `live_domain*` copies.

Existing test coverage that must stay green and unchanged: tests/all/arn_resolution_during_domain_deletion.rs (header: "an ARN-addressed operation resolves a resource for exactly as long as the service HOLDS its record … `ListTagsForResource` (and the `TagResource` that writes the set it reports) resolves an ARN at precisely the instants the resource's OWN reader resolves the resource"), tests/all/resource_tagging.rs, tests/all/resource_tag_removal.rs, tests/all/list_tags_for_resource.rs, tests/all/domain_subscription_lifecycle.rs.

## Steps to Reproduce

1. Open services/service-com-amazonaws-connecthealth-connecthealth/src/state.rs and grep for `DomainStatus::Active`: four independent comparisons appear (lines 832, 1013, 1044, 1051), none of them going through a shared helper.
2. Compare with `Subscription::is_live()` at state.rs:729, the single-homed sibling rule every subscription reader calls.
3. Read the doc comment at state.rs:1006-1009, which claims `holds_resource_arn` resolves a subscription "through its own reader's liveness ([`Self::live_subscription`])", then read the body at state.rs:1010-1019: `live_subscription` is never called; both of its clauses are re-implemented inline.
4. Change the domain-liveness rule in `live_domain`/`live_domain_mut` only (as a maintainer would) and observe that `holds_resource_arn` — the resolution TagResource/UntagResource/ListTagsForResource use — keeps the old rule, so the tagging operations and the resource's own reader disagree about whether an ARN names a resource.
