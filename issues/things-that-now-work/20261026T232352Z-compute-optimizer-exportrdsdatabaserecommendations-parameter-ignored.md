Issue-Severity: LOW
Issue-Gap-Class: parameter-ignored
Issue-Commit: f2163892bf40cea6da7d1662f28c14ec75c797c6

# ExportRDSDatabaseRecommendations drops recommendationPreferences while its reader honours it

_Filed: 2026-10-26T23:23:52Z_
_Operation: ExportRDSDatabaseRecommendations | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `f2163892bf40cea6da7d1662f28c14ec75c797c6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ExportRDSDatabaseRecommendationsRequest` declares `recommendationPreferences`, and the sibling reader of the same analysis, `GetRDSDatabaseRecommendations`, both resolves it (feeding `cpuVendorArchitectures` into the recommendation it builds) and folds it into its page-token selection fingerprint. The export handler never reads `input.recommendation_preferences`, so the member is accepted, never recorded on the export plan or the job, and reflected nowhere — neither in the exported rows nor in the metadata file that otherwise states "what the export job itself was".

## Evidence

The reader resolves the member — src/ops/get_rds_database_recommendations.rs:243-249 and :321-325:
```
        let selection_fingerprint = crate::state::fingerprint(&(
            Self::NAME,
            &resource_arns,
            &account_id,
            &filters,
            &input.recommendation_preferences,
        ));
...
                    input
                        .recommendation_preferences
                        .as_ref()
                        .and_then(|preferences| preferences.cpu_vendor_architectures.as_ref()),
```
and reports it back at src/ops/get_rds_database_recommendations.rs:541-546 (`cpu_vendor_architectures: requested_cpu_vendor_architectures ...`).

The export handler never mentions it — src/ops/export_rds_database_recommendations.rs:219-233 passes only `account_ids`, `include_member_accounts`, `filters`, `destination`, `fields_to_export`, `exportable_fields`, `file_format`; a repository-wide grep for `recommendation_preferences` under `src/ops` returns no hit in any `export_*.rs` handler. `ExportRequest` (src/state/export.rs:276-306) and `ExportPlan` (src/state/export.rs:146-173) declare no field for it, and the metadata file (src/state/export.rs:524-547) states `accountIds`, `includeMemberAccounts`, `fileFormat` and `columns` but nothing about preferences.

No test pins the drop: tests/all/rds_database_recommendations_export.rs:458 only asserts a request carrying `{"cpuVendorArchitectures": ["CURRENT"]}` is handled, never inspecting the exported rows or metadata.

## Steps to Reproduce

1. Opt in with `UpdateEnrollmentStatus(status=Active)` and create the destination S3 bucket.
2. Call `GetRDSDatabaseRecommendations` with `recommendationPreferences={"cpuVendorArchitectures":["AWS_ARM64"]}` — the reported `effectiveRecommendationPreferences.cpuVendorArchitectures` echoes the request.
3. Call `ExportRDSDatabaseRecommendations` with the same `recommendationPreferences`.
4. Poll `DescribeRecommendationExportJobs` to `Complete` and `GetObject` both the export file and the metadata file.
5. Neither object differs in any way from an export issued without `recommendationPreferences`: the member was accepted and discarded.
