Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 1a78f0cc1d943875c45c11a36243ccaf94b7f40f

# The export-job admission and creation rule is copy-pasted into all nine Export*Recommendations handlers

_Filed: 2026-10-25T21:23:47Z_
_Operation: ExportAutoScalingGroupRecommendations | Class: maintainability_

> _Note: this issue was identified against the code base at commit `1a78f0cc1d943875c45c11a36243ccaf94b7f40f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The nine `Export*Recommendations` handlers each contain their own verbatim copy of the same ~90-line block: the `accountIds`/`includeMemberAccounts` mutual-exclusion check, the 12-digit account-id check, the `s3DestinationConfig.bucket` trim/emptiness check, `request_region(ctx)` + `now_secs()`, the opt-in gate, `export_jobs.prune_expired(now)`, the one-job-in-progress-per-Region check, the `export_jobs.can_hold()` check, `mint_job_id()`, `export_destination_keys(..)`, the `ExportJob { .. }` literal and `export_jobs.insert(job)`. Only three things differ between copies: the resource-type constant, which `*_filter_rejection` helper is called, and the wording of the `LimitExceededException` message.

The copies can (and already do) drift: `export_idle_recommendations.rs` has no `EXPORTED_RESOURCE_TYPE` constant and inlines `crate::state::RESOURCE_TYPE_IDLE` at three separate call sites, while the other eight route it through one named constant. One copy gaining or losing a check (say, the bucket trim, or the `prune_expired` call that the comment says a writer owes) would silently make one export family behave differently from the other eight.

The shared part belongs beside the record it writes — `crate::state` already owns every other rule over an export job ("So the job record and every rule over it live here ONCE, whichever export operation created it", src/state.rs:5093-5097). Moving the block there as one helper that takes the resource type, the parsed destination and the per-operation messages leaves every response byte-identical and every existing test unchanged.

## Evidence

Copy 1 — `src/ops/export_auto_scaling_group_recommendations.rs:218-234, 258-340`:

        let account_ids = input.account_ids.unwrap_or_default();
        if !account_ids.is_empty() && input.include_member_accounts {
            return Err(invalid_parameter(
                "The value supplied for parameter accountIds is not valid: accountIds and \
                 includeMemberAccounts are mutually exclusive."
                    .to_string(),
            ));
        }
        …
        let job_id = guard.export_jobs.mint_job_id();
        let (key, metadata_key) = crate::state::export_destination_keys(
            input.s3_destination_config.key_prefix.as_deref(),
            ::engine::emulator_account_id(),
            EXPORTED_RESOURCE_TYPE,
            &job_id,
        );

Copy 2 — `src/ops/export_ebs_volume_recommendations.rs:220-236, 258-339`: the same statements, character for character, with `ebs_filter_rejection` in place of `resource_filter_rejection`.

The other seven copies, at the same offsets: `export_ec2_instance_recommendations.rs:218-322`, `export_ecs_service_recommendations.rs:220-322`, `export_idle_recommendations.rs:218-316`, `export_lambda_function_recommendations.rs:219-321`, `export_license_recommendations.rs:218-319`, `export_rds_database_recommendations.rs:226-327`. The drift already present in one of them — `src/ops/export_idle_recommendations.rs:291, 314, 320` inline `crate::state::RESOURCE_TYPE_IDLE` where the siblings use `EXPORTED_RESOURCE_TYPE` (`export_ebs_volume_recommendations.rs:353`).

Where the crate says this logic belongs — `src/state.rs:5090-5097`:

    // Every `Export*Recommendations` operation of this service does the same thing: it QUEUES
    // one asynchronous export job against the destination bucket the request names, and answers
    // the job's id plus the object keys the export file and its metadata file will be written
    // under. … So the job record and every rule over it live here ONCE, whichever export
    // operation created it

Test coverage that must stay green and unchanged: `tests/all/auto_scaling_group_recommendations_export.rs`, `tests/all/ebs_volume_recommendations_export.rs`, `tests/all/ec2_instance_recommendations_export.rs`, `tests/all/ecs_service_recommendations_export.rs`, `tests/all/lambda_function_recommendations_export.rs`, `tests/all/license_recommendations_export.rs`, `tests/all/rds_database_recommendations_export.rs` (each with an `only_one_*_export_job_runs_per_region_at_a_time` case) and `tests/all/recommendation_export_jobs.rs`.

## Steps to Reproduce

1. Open `src/ops/export_auto_scaling_group_recommendations.rs` and `src/ops/export_ebs_volume_recommendations.rs` side by side and diff lines 218-340 against 220-339: the only differences are the filter helper, the resource-type constant and the limit message.
2. Repeat for the other seven `src/ops/export_*_recommendations.rs` files — nine copies of one rule.
3. Note that `src/ops/export_idle_recommendations.rs` already diverges in form (no `EXPORTED_RESOURCE_TYPE` constant; `crate::state::RESOURCE_TYPE_IDLE` inlined at lines 291, 314 and 320), showing the copies drift.
4. Run the export test suites listed in the evidence: they pass today and must pass byte-identically after the block is hoisted into `crate::state`.
