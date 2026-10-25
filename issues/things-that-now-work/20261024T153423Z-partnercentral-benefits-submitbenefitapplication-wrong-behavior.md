Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 0f7bb362ed1d3a4f050d8269ef5300e336140f8b

# Submitting an application records no Stage, so Stage is always absent and the modelled Stages filter can never match

_Filed: 2026-10-24T15:34:23Z_
_Operation: SubmitBenefitApplication | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `0f7bb362ed1d3a4f050d8269ef5300e336140f8b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`SubmitBenefitApplication` moves the application's `Status` from PENDING_SUBMISSION to IN_REVIEW and bumps `UpdatedAt`, but it never records a workflow `Stage` — and no other handler does either (`CreateBenefitApplication` stores `stage: None` and nothing assigns it afterwards).

Expected: a submitted application that AWS is reviewing reports its position in the review workflow through `Stage`, which the model binds on `GetBenefitApplicationOutput` and on `BenefitApplicationSummary` ("The current stage in the benefit application processing workflow") and which `ListBenefitApplications` exposes as the `Stages` filter. Filtering submitted applications by their stage returns those applications.

Actual: `Stage` is absent from every `GetBenefitApplication` and every `ListBenefitApplications` summary for the lifetime of the application, and because the filter compares against an empty slice (`application.stage.as_slice()`), `ListBenefitApplications` with any non-empty `Stages` value returns an empty page — the modelled filter is permanently unmatchable. The related `StatusReason`/`StatusReasonCodes` members are likewise never populated for a submitted application.

## Evidence

Model: `GetBenefitApplicationOutput$Stage` -> `BenefitApplicationStage` documented "The current stage in the benefit application processing workflow"; `BenefitApplicationSummary$Stage` is the same member; `ListBenefitApplicationsInput$Stages` is a `list<BenefitApplicationStage>` documented "Filter benefit applications by their current processing stage" (smithy_query shape=ListBenefitApplicationsInput). The stage vocabulary is an unconstrained string in the model, so the exact value AWS assigns is not modelled — the gap is that NO stage is ever recorded, not that a particular literal is wrong.

Handler: src/ops/submit_benefit_application.rs lines ~181-196 set only `application.status = STATUS_IN_REVIEW` and `application.touch(now)`. `grep -n 'stage' src/` shows the field is written exactly once, as `stage: None` in src/ops/create_benefit_application.rs line ~335; src/state.rs ~903-905 documents "The workflow stage. AWS sets it as its own review workflow advances and no input carries it", i.e. the field is read-only state nothing in the emulator ever advances. It is read back in src/ops/get_benefit_application.rs ~234 and src/ops/list_benefit_applications.rs ~272-273/301.

Test coverage: no test asserts a non-absent Stage. tests/all/benefit_application_lifecycle.rs ~1543-1544 only asserts the negative case ("a stage no application has", `Stages: ["QUALIFY"]` -> empty page), and tests/all/validation_error_body.rs ~138-143 only checks the modelled length bound on the filter.

## Steps to Reproduce

1. Start the emulator with the partner entitlement in place.
2. CreateBenefitApplication against an ACTIVE benefit in Catalog=AWS, then SubmitBenefitApplication for the returned Id.
3. GetBenefitApplication on that Id: Status is IN_REVIEW but the response carries no Stage member at all (and no StatusReason/StatusReasonCodes).
4. ListBenefitApplications with {"Catalog":"AWS"}: the summary for that application also omits Stage.
5. ListBenefitApplications with {"Catalog":"AWS","Stages":["<any value>"]}: always an empty BenefitApplicationSummaries list, so the modelled Stages filter can never select a submitted application.
