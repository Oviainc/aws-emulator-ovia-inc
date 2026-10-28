Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: f2163892bf40cea6da7d1662f28c14ec75c797c6

# ExportEC2InstanceRecommendations drops recommendationPreferences

_Filed: 2026-10-26T23:23:52Z_
_Operation: ExportEC2InstanceRecommendations | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `f2163892bf40cea6da7d1662f28c14ec75c797c6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler builds its `ExportRequest` from `accountIds`, `includeMemberAccounts`, `filters`, `s3DestinationConfig`, `fieldsToExport` and `fileFormat` and never reads `input.recommendation_preferences`; `ExportRequest` has no field for it either, so the preference is accepted, never stored on the export plan, and has no effect on the rows the export file holds. The model states the behaviour explicitly for this operation, and the sibling reader of the same analysis (`GetEC2InstanceRecommendations`) does honour the same member, so the export and the read of one analysis disagree under the same preference.

## Evidence

The model states the rule for THIS operation — `RecommendationPreferences$cpuVendorArchitectures`: "For example, when you specify `AWS_ARM64` with: ... A `ExportEC2InstanceRecommendations` or `ExportAutoScalingGroupRecommendations` request, Compute Optimizer exports recommendations that consist of Graviton instance types only." The member is modelled on the input: `ExportEC2InstanceRecommendationsRequest$recommendationPreferences` — "An object to specify the preferences for the Amazon EC2 instance recommendations to export."

The handler never reads it — src/ops/export_ec2_instance_recommendations.rs:210-232 passes only:
```
                account_ids: input.account_ids,
                include_member_accounts: input.include_member_accounts,
                filters: input.filters,
                destination: &input.s3_destination_config,
                fields_to_export: input.fields_to_export.as_deref(),
                exportable_fields: EXPORTABLE_INSTANCE_FIELD_VALUES,
                file_format: input.file_format,
```
while its own comment claims otherwise — src/ops/export_ec2_instance_recommendations.rs:202-209:
```
        // `fileFormat` is a `FileFormat` ("The only export file format currently supported
        // is Csv"), `fieldsToExport` a list of `ExportableInstanceField` and
        // `recommendationPreferences$cpuVendorArchitectures` a list of
        // `CpuVendorArchitecture` ... Those members select WHAT the export file holds
```
`ExportRequest` declares no member for it (src/state/export.rs:276-306), and `ExportPlan` (src/state/export.rs:146-173) likewise carries none, so nothing downstream can apply it. The sibling reader does apply it — src/ops/get_ec2_instance_recommendations.rs:224-228 / :541-548 filter `instance.candidates` by `requested_architectures`.

No test pins the drop: tests/all/ec2_instance_recommendations_export.rs:496-504 only asserts an undeclared `CpuVendorArchitecture` value is refused, and :530-550 only asserts a request carrying `{"cpuVendorArchitectures": ["AWS_ARM64"]}` is accepted and answers a job id — neither inspects the exported rows.

## Steps to Reproduce

1. Opt in with `UpdateEnrollmentStatus(status=Active)` and sleep past `ANALYSIS_DELAY`; create the destination S3 bucket.
2. Call `GetEC2InstanceRecommendations` with `recommendationPreferences={"cpuVendorArchitectures":["AWS_ARM64"]}` and note the restricted set of recommendation options reported.
3. Call `ExportEC2InstanceRecommendations` with the same `recommendationPreferences` and an `s3DestinationConfig` naming the bucket.
4. Poll `DescribeRecommendationExportJobs` until the job is `Complete`, then `GetObject` the export file.
5. The file is byte-identical to one exported with no `recommendationPreferences` at all — the preference selected nothing.
