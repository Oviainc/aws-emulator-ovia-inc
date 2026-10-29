Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: f2163892bf40cea6da7d1662f28c14ec75c797c6

# ExportAutoScalingGroupRecommendations drops recommendationPreferences

_Filed: 2026-10-26T23:23:52Z_
_Operation: ExportAutoScalingGroupRecommendations | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `f2163892bf40cea6da7d1662f28c14ec75c797c6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler builds its `ExportRequest` without `input.recommendation_preferences`, so `cpuVendorArchitectures` never reaches the export plan and never restricts the rows written. The Auto Scaling group export emits one row per candidate configuration of every analysed group and fills `EffectiveRecommendationPreferencesCpuVendorArchitectures` with every architecture the group's candidates hold, regardless of what the request asked for. The model states the behaviour for this operation by name, and the sibling reader `GetAutoScalingGroupRecommendations` does honour the same member.

## Evidence

Model text naming this operation — `RecommendationPreferences$cpuVendorArchitectures`: "when you specify `AWS_ARM64` with: ... A `ExportEC2InstanceRecommendations` or `ExportAutoScalingGroupRecommendations` request, Compute Optimizer exports recommendations that consist of Graviton instance types only." The member is modelled on the input (`ExportAutoScalingGroupRecommendationsRequest$recommendationPreferences`, target `RecommendationPreferences`).

The handler's comment claims the member is honoured — src/ops/export_auto_scaling_group_recommendations.rs:206-213:
```
        // `fileFormat` is a `FileFormat` ("The only export file format currently supported
        // is Csv"), `fieldsToExport` a list of `ExportableAutoScalingGroupField` and
        // `recommendationPreferences$cpuVendorArchitectures` a list of
        // `CpuVendorArchitecture` ... Those members select WHAT the export file holds
```
but the call it introduces omits it — src/ops/export_auto_scaling_group_recommendations.rs:217-235:
```
                account_ids: input.account_ids,
                include_member_accounts: input.include_member_accounts,
                filters: input.filters,
                destination: &input.s3_destination_config,
                fields_to_export: input.fields_to_export.as_deref(),
                exportable_fields: EXPORTABLE_AUTO_SCALING_GROUP_FIELD_VALUES,
                file_format: input.file_format,
```
The row builder consequently emits every candidate — src/state/export.rs:830-854:
```
            for candidate in &group.candidates {
                if !architectures.contains(&candidate.architecture) {
                    architectures.push(candidate.architecture.clone());
...
            for candidate in &group.candidates {
                rows.push(auto_scaling_group_row(
```
while the reader filters them — src/ops/get_auto_scaling_group_recommendations.rs:465-472: `.filter(|candidate| requested_architectures.is_empty() || requested_architectures.contains(&candidate.architecture))`.

No test pins the drop: tests/all/auto_scaling_group_recommendations_export.rs:499 and :541 only exercise refusal of an undeclared value and acceptance of `AWS_ARM64`; tests/all/recommendation_export_objects.rs:393-507 (the row-count test) sends no `recommendationPreferences` at all, so honouring it leaves that test unchanged.

## Steps to Reproduce

1. Opt in with `UpdateEnrollmentStatus(status=Active)`, sleep past `ANALYSIS_DELAY`, create the destination S3 bucket.
2. Call `GetAutoScalingGroupRecommendations` with `recommendationPreferences={"cpuVendorArchitectures":["AWS_ARM64"]}` and note how many `recommendationOptions` each group reports.
3. Call `ExportAutoScalingGroupRecommendations` with the same `recommendationPreferences` and an `s3DestinationConfig` naming the bucket.
4. Poll `DescribeRecommendationExportJobs` to `Complete` and `GetObject` the export file.
5. The file still holds one row per candidate of every group (and `EffectiveRecommendationPreferencesCpuVendorArchitectures` lists every architecture), not the Graviton-only subset the reader reported.
