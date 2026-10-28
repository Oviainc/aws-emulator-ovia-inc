Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 2f325610dba287ab6eb0f722c6fb72ebe3ad3d74

# Deduplicate AssociatedResources stored by CreateBenefitApplication

_Filed: 2026-10-28T00:35:48Z_
_Operation: CreateBenefitApplication | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `2f325610dba287ab6eb0f722c6fb72ebe3ad3d74`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateBenefitApplication` stores the request's `AssociatedResources` list verbatim, so a create naming the same ARN twice puts two copies on the record. The sibling `AssociateBenefitApplicationResource` handler maintains that very list as a SET — its own comment states "an ARN it already carries is stored once rather than twice" — and the create does normalise its OTHER repeated-key list (`Tags`) on the adjacent lines, with a comment explaining that a repeated key is stored once. Only `AssociatedResources` escapes normalisation.

The consequences are observable:

* `GetBenefitApplication` and the `ListBenefitApplications` summary report the same ARN twice, while the same record reached through `AssociateBenefitApplicationResource` can never contain a repeat.
* The duplicates consume slots against `MAX_ASSOCIATED_RESOURCES_PER_APPLICATION`, which `AssociateBenefitApplicationResource` enforces as the count of resources "one benefit application carries". A create carrying the same ARN ten times fills the record with ONE distinct resource and then makes every later `AssociateBenefitApplicationResource` fail with the `ValidationException` from `record_full()` — the application is permanently unable to associate a second resource.
* `DisassociateBenefitApplicationResource` drops "every occurrence", so a single disassociation silently removes a different number of entries depending on which path put them there.

## Evidence

The create stores the list unnormalised —
src/ops/create_benefit_application.rs:463:
            associated_resources: input.associated_resources.unwrap_or_default(),
while on the adjacent lines it DOES normalise the other repeated-key list —
src/ops/create_benefit_application.rs:471-475:
            // The tag set is a MAP, so a list that repeats a key stores it once; its size
            // is bounded by the per-resource cap ...
            tags: crate::state::tag_map(input.tags.as_ref()),

The sibling handler states the set invariant the create breaks —
src/ops/associate_benefit_application_resource.rs:281-285:
        // A set union: the named ARN joins the associations the application already
        // carries, and an ARN it already carries is stored once rather than twice. The
        // revision and `UpdatedAt` move only when the record actually changed, so a request
        // naming a resource this application is already linked to reports the revision it
        // already had.
and refuses a further association once the (duplicate-inflated) list is full —
src/ops/associate_benefit_application_resource.rs:296-300:
            if application.associated_resources.len()
                >= crate::state::MAX_ASSOCIATED_RESOURCES_PER_APPLICATION
            {
                return Err(record_full());
            }
against a bound the state module defines as a count of distinct resources —
src/state.rs:429-441:
    /// The most AWS resources one benefit application carries.
    ... "so a create and an `AssociateBenefitApplicationResource` agree on how many resources one application can
    /// be linked to, and a stored record never holds more than the read can report."
    pub const MAX_ASSOCIATED_RESOURCES_PER_APPLICATION: usize = 10;

Test coverage: tests/all/disassociate_benefit_application_resource.rs:40-47 (`RESOURCE_ARNS`) and its `seed()` at :61-83 drive the create path with distinct ARNs only, and tests/all/idempotent_parameter_mismatch.rs:130/211/218 vary ORDER, not repetition — so no existing test pins the storing of duplicates.

## Steps to Reproduce

1. Call CreateBenefitApplication with `AssociatedResources` = ["arn:aws:s3:::partner-proposals", "arn:aws:s3:::partner-proposals"] (the modelled max is 10, so the list is accepted).
2. Call GetBenefitApplication and observe `AssociatedResources` reports the same ARN twice.
3. Repeat the create with the same ARN ten times, then call AssociateBenefitApplicationResource with a different ARN: it is refused by `record_full()` although the record links only ONE distinct resource.
4. Compare with AssociateBenefitApplicationResource called twice with the same ARN, which stores it once (src/ops/associate_benefit_application_resource.rs:286-305).
