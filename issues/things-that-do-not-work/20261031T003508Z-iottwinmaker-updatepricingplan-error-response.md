Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 59fa4dd4e7f88de8e618073fff0b2e626be0bdb4

# UpdatePricingPlan answers the Region ceiling with ThrottlingException, the one error its own module doc states it never returns

_Filed: 2026-10-31T00:35:08Z_
_Operation: UpdatePricingPlan | Class: error-response_

> _Note: this issue was identified against the code base at commit `59fa4dd4e7f88de8e618073fff0b2e626be0bdb4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler's module doc tells a reader that this operation never answers `ThrottlingException`, because nothing in it is rate limited. The handle body does answer `ThrottlingException` (HTTP 429): it is the error the Region-ceiling refusal is sent as. The doc and the code therefore disagree about the set of errors a caller of this operation can see, and the error class chosen is the transient one — an SDK's default retry policy treats 429 as retryable — for a condition that never clears on its own, while the two sibling creates that enforce the SAME ceiling (`CreateWorkspace`, `CreateMetadataTransferJob`) answer it with `ServiceQuotaExceededException`. Either the doc is stale or the refusal class is, but both cannot stand.

## Evidence

The doc, `services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/ops/update_pricing_plan.rs:51`:

    //! Authorization is not emulated, so the modelled `AccessDeniedException` is
    //! never returned, and nothing here is rate limited, so neither is
    //! `ThrottlingException`.

The code under it, update_pricing_plan.rs:250:

        if state.region_ceiling_reached(&region) {
            return Err(UpdatePricingPlanErrorModeled::ThrottlingException(
                ThrottlingException {
                    message: Some(crate::state::region_ceiling_message()),
                },
            )
            .into());
        }

The same condition, in the sibling writers of this crate — `src/ops/create_workspace.rs:249`:

        if state.region_ceiling_reached(&region) {
            return Err(CreateWorkspaceErrorModeled::ServiceQuotaExceededException(
                ServiceQuotaExceededException {
                    message: Some(crate::state::region_ceiling_message()),

and `src/ops/create_metadata_transfer_job.rs:214`:

        if state.region_ceiling_reached(&region) {
            return Err(
                CreateMetadataTransferJobErrorModeled::ServiceQuotaExceededException(

(`crate::state::region_ceiling_reached` is documented at state.rs:959 as "Whether this store has no room to give `region` its FIRST resource" — a ceiling, not a rate.)

Test coverage: `tests/all/pricing_plan.rs`, `tests/all/pricing_plan_update.rs` and `tests/all/pricing_plan_tier_and_pending.rs` exercise the plan's modes, tiers and pending change; none of them drives the Region ceiling, so no test pins the 429 or the doc sentence.

## Steps to Reproduce

1. Read `src/ops/update_pricing_plan.rs:51-53`: the module doc states `ThrottlingException` is never returned by this operation.
2. Read `src/ops/update_pricing_plan.rs:250-257`: the handler returns `ThrottlingException` for `state.region_ceiling_reached(&region)`.
3. Compare with `src/ops/create_workspace.rs:249-256` and `src/ops/create_metadata_transfer_job.rs:214-223`, where the identical `region_ceiling_reached` condition is answered with `ServiceQuotaExceededException`.
4. Drive UpdatePricingPlan under more than `crate::state::MAX_REGIONS` credential-scope Regions that each hold a resource: the call is refused 429, a status an SDK retries, for a condition that will not clear.
