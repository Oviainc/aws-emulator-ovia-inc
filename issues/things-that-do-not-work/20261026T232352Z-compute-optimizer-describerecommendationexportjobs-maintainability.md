Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: f2163892bf40cea6da7d1662f28c14ec75c797c6

# The page-size rule and the offset-page rule are each re-implemented in every paginated handler

_Filed: 2026-10-26T23:23:52Z_
_Operation: DescribeRecommendationExportJobs | Class: maintainability_

> _Note: this issue was identified against the code base at commit `f2163892bf40cea6da7d1662f28c14ec75c797c6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Two paging rules this crate owns are written out verbatim in handler after handler instead of once in `crate::state`, where this crate already keeps its other shared rules (`decode_offset_page_token`, `encode_offset_page_token`, `PageCursor`, `decode_resource_page_token`, `fingerprint`, `scope_rejection`, `requested_account_id`, `resource_filter_rejection`).

(1) The `maxResults` rule — "a value of 0 or an absent value means the default, then cap at the maximum" — appears identically in twelve handlers.

(2) The offset-page tail — take `page_size + 1`, decide `has_more`, truncate, and mint a `PageCursor` at `offset + page.len()` — appears identically in five handlers.

Because each copy also names its own default and cap inline, the copies have already begun to diverge (`DEFAULT_PAGE_SIZE`/`MAX_PAGE_SIZE` in most, `IDLE_MAX_PAGE_SIZE` in one, `ENROLLMENT_STATUSES_MAX_PAGE_SIZE` in another), so a correction to the shared rule has to be made twelve times to take effect everywhere. Extracting `crate::state::page_size(max_results, default, cap)` and a `crate::state::offset_page(...)` helper that each handler calls with its own default and cap leaves every response byte-identical and every existing test unchanged.

## Evidence

Copy of rule (1) in src/ops/describe_recommendation_export_jobs.rs:231-236:
```
        let page_size = input
            .max_results
            .and_then(|requested| usize::try_from(requested).ok())
            .filter(|requested| *requested > 0)
            .unwrap_or(crate::state::DEFAULT_PAGE_SIZE)
            .min(crate::state::MAX_PAGE_SIZE);
```
The identical expression also occurs at src/ops/get_recommendation_summaries.rs:232-237, src/ops/get_recommendation_preferences.rs:231-236, src/ops/get_rds_database_recommendations.rs:273-278, src/ops/get_license_recommendations.rs:265-270, src/ops/get_lambda_function_recommendations.rs:230-235, src/ops/get_idle_recommendations.rs:269-274, src/ops/get_enrollment_statuses_for_organization.rs:226-231, src/ops/get_ec2_instance_recommendations.rs:247-252, src/ops/get_ecs_service_recommendations.rs:353-358, src/ops/get_ebs_volume_recommendations.rs:224-229 and src/ops/get_auto_scaling_group_recommendations.rs:281-286 (grep for `.filter(|requested| *requested > 0)` returns twelve hits).

Copy of rule (2) in src/ops/describe_recommendation_export_jobs.rs:304-316:
```
        let page: Vec<RecommendationExportJob> = selected
            .into_iter()
            .skip(offset)
            .take(page_size.saturating_add(1))
            .collect();
        let has_more = page.len() > page_size;
        let page: Vec<RecommendationExportJob> = page.into_iter().take(page_size).collect();
        let next_token = has_more.then(|| {
            crate::state::encode_offset_page_token(&crate::state::PageCursor {
                offset: offset.saturating_add(page.len()),
                selection_fingerprint,
            })
        });
```
The same block, differing only in the element type, appears at src/ops/get_rds_database_recommendations.rs:359-369, src/ops/get_license_recommendations.rs:357-369, src/ops/get_idle_recommendations.rs:405-417 and src/ops/get_enrollment_statuses_for_organization.rs:265-277 (grep for `let has_more = page.len() > page_size;` returns five hits).

Existing tests that cover these paths and must stay green unchanged: tests/all/recommendation_export_jobs.rs, tests/all/enrollment_and_preferences.rs ("`tests/all/enrollment_and_preferences.rs` pins both" — src/ops/get_recommendation_preferences.rs:230), tests/all/organization_enrollment_statuses.rs, tests/all/idle_recommendations.rs, tests/all/license_recommendations.rs, tests/all/rds_database_recommendations.rs and tests/all/recommendation_summaries.rs.

## Steps to Reproduce

1. grep -n '.filter(|requested| \*requested > 0)' services/service-com-amazonaws-computeoptimizer-computeoptimizerservice/src/ops — twelve handlers each re-derive the same `maxResults` rule.
2. grep -n 'let has_more = page.len() > page_size;' services/service-com-amazonaws-computeoptimizer-computeoptimizerservice/src/ops — five handlers each re-derive the same offset-page tail.
3. Compare src/ops/describe_recommendation_export_jobs.rs:231-236 with src/ops/get_license_recommendations.rs:265-270 and src/ops/get_idle_recommendations.rs:269-274: the same rule, three copies, two different cap constants spelled inline.
4. Run `cargo test -p service-com-amazonaws-computeoptimizer-computeoptimizerservice` before and after factoring the two rules into `crate::state`: the suite must pass unchanged.
