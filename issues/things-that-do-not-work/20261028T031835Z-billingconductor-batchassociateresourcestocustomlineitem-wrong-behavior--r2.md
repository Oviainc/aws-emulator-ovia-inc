Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 8e0ac146ffee686dd3a46765dec581c593b08a30

# A re-association naming billing periods the stored association does not cover is reported as successful but writes nothing

_Filed: 2026-10-28T03:18:35Z_
_Operation: BatchAssociateResourcesToCustomLineItem | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `8e0ac146ffee686dd3a46765dec581c593b08a30`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler's idempotency short-circuit keys on the RESOURCE alone, ignoring the request's `BillingPeriodRange`: `if target.association(&resource_arn).is_some()` pushes the resource into `SuccessfullyAssociatedResources` and `continue`s, and `CustomLineItemRecord::association` matches on the ARN only (`.find(|association| association.arn == arn)`), never on the billing periods that association is in effect over.

Everywhere else in this crate an association is a SET of billing periods, not a boolean: `ResourceAssociationRecord` holds `spans: Vec<BillingPeriodSpan>`, `ListResourcesAssociatedToCustomLineItem` skips an association whose span does not cover the period being read, and the sibling `BatchDisassociateResourcesFromCustomLineItem` applies the request's range to those spans so that "the association RESUMES at the range's `ExclusiveEndBillingPeriod`" — i.e. the crate deliberately supports an association that is in effect in some periods and not in others.

So the one write path that could put periods BACK into an association silently refuses to, while answering 200 and listing the resource among the ones it "associated". After the sequence associate → bounded disassociate → re-associate, the resource is permanently unassociated in the periods the bounded disassociation took, and no call can restore it: every later `BatchAssociate` for those periods is short-circuited as "already holds". The handler comment's premise ("the association this call asks for already holds") is false on exactly that path, and the module header's promise that `SuccessfullyAssociatedResources` carries "the ones it associated" is broken.

## Evidence

The short-circuit, which never looks at the request's range — `src/ops/batch_associate_resources_to_custom_line_item.rs:340-349`:
```
            // `@idempotent`: the association this call asks for already holds, so the
            // call adds nothing and leaves the stored range as the earlier call set
            // it — and `AssociationSize` does not double-count.
            if target.association(&resource_arn).is_some() {
                associated.push(AssociateResourceResponseElement {
                    arn: Some(requested.clone()),
                    error: None,
                });
                continue;
            }
```
The lookup it decides on ignores the periods — `src/state.rs:1850-1855`:
```
    /// The association for `arn`, if this item already holds one.
    pub fn association(&self, arn: &str) -> Option<&ResourceAssociationRecord> {
        self.associated_resources
            .iter()
            .find(|association| association.arn == arn)
    }
```
The same crate treats an association as a set of periods — `src/state.rs:1528-1535`:
```
    /// Is this association in effect during `billing_period`? The start is
    /// inclusive and the end exclusive, as the member names say
    /// (`InclusiveStartBillingPeriod` / `ExclusiveEndBillingPeriod`); an absent end is
    /// open, so an association nothing has ended covers every period from the one it
    /// was made in on ([`Self::new`]).
    pub fn covers(&self, billing_period: &str) -> bool {
        self.spans.iter().any(|span| span.covers(billing_period))
    }
```
The sibling write path DOES apply the request's range to those spans — `src/ops/batch_disassociate_resources_from_custom_line_item.rs:184-187` and `:370`:
```
/// The range is half-open, so the association keeps every period outside it: a range
/// bounded on both sides takes only the periods between its bounds and the association
/// RESUMES at the exclusive end, while an open-ended range (and the fully open span a
/// request naming no range disassociates over) takes every period from its start on.
...
                match apply_disassociation(&mut target.associated_resources, index, &removed) {
```
And the read reports only the periods a span covers — `src/ops/list_resources_associated_to_custom_line_item.rs:213-217`:
```
            // An association is listed for a period its range covers, the start
            // inclusive and the end exclusive.
            if !association.covers(&billing_period) {
                continue;
            }
```
while the handler header claims the opposite of what the short-circuit does — `src/ops/batch_associate_resources_to_custom_line_item.rs:4-8`:
```
//! PERSISTS the association of each requested resource to a percentage custom line
//! item, and reports the outcome PER RESOURCE: the ones it associated in
//! `SuccessfullyAssociatedResources`, ...
```
Test coverage: NO test exercises a second association with a different range. `tests/all/percentage_custom_line_item_resource_associations.rs:388-418` (`re_associating_a_resource_is_idempotent`) sends every repeat with `None` for the range (`emulator.associate(&target, &resources, None)`), and `tests/all/resource_association_billing_periods.rs:306-363` builds a fresh `Emulator` per row and associates exactly once. So the behaviour is unpinned, and the bounded-range semantics the crate does pin are the disassociation side only (`tests/all/batch_disassociate_resources_from_custom_line_item.rs:953` "so it resumes at the range's `ExclusiveEndBillingPeriod`").

## Steps to Reproduce

1. Designate the emulated account as the payer so the control plane is reachable.
2. POST /create-pricing-plan, POST /create-billing-group, then POST /create-custom-line-item with a Percentage ChargeDetails; keep the item Arn (the target) and the billing group Arn (the resource).
3. PUT /batch-associate-resources-to-custom-line-item with {"TargetArn":"<item>","ResourceArns":["<group arn>"]} and no BillingPeriodRange — the association is in effect from the current billing period on.
4. PUT /batch-disassociate-resources-from-custom-line-item with the same TargetArn/ResourceArns and BillingPeriodRange {"InclusiveStartBillingPeriod":"<current period>","ExclusiveEndBillingPeriod":"<next period>"} — the association now covers the next period onward but not the current one.
5. POST /list-resources-associated-to-custom-line-item with {"Arn":"<item>"} (current period) and observe the resource is absent, as intended by the bounded disassociation.
6. PUT /batch-associate-resources-to-custom-line-item again with {"TargetArn":"<item>","ResourceArns":["<group arn>"]} (or with BillingPeriodRange {"InclusiveStartBillingPeriod":"<current period>"}).
7. Observe HTTP 200 with the resource in SuccessfullyAssociatedResources and FailedAssociatedResources empty.
8. POST /list-resources-associated-to-custom-line-item again for the current period: the resource is STILL absent, so the call reported an association it never made, and no request can ever restore the resource for that period.
