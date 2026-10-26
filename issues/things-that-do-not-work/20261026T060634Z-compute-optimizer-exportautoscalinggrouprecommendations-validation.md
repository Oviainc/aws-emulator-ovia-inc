Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 0e9a1926b382543791c56cc4902ed328671b8b8b

# Export operations accept an accountIds naming another account and export this account's rows under it

_Filed: 2026-10-26T06:06:34Z_
_Operation: ExportAutoScalingGroupRecommendations | Class: validation_

> _Note: this issue was identified against the code base at commit `0e9a1926b382543791c56cc4902ed328671b8b8b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The export handlers validate `accountIds` only against `crate::state::is_account_id` (12 digits) and never against `crate::state::is_foreign_account`, which is the crate's ONE test for "an account this service cannot answer about" and which every reader applies through `requested_account_id`. The export therefore succeeds for an id naming another account: the job is created, and the export file holds THIS account's recommendations (the row builder is hard-coded to `engine::emulator_account_id()`), while the metadata object states `"accountIds": [<the other account>]`. A request the readers refuse with 400 is turned into a successful export of someone else's data under the requested account's name. The same code appears in all eight `Export*Recommendations` handlers (export_auto_scaling_group_recommendations.rs:230, export_ebs_volume_recommendations.rs:230, export_ec2_instance_recommendations.rs:228, export_ecs_service_recommendations.rs:230, export_idle_recommendations.rs:228, export_lambda_function_recommendations.rs:229, export_license_recommendations.rs:228, export_rds_database_recommendations.rs:236).

## Evidence

Export handler — src/ops/export_auto_scaling_group_recommendations.rs:230-238: "if let Some(value) = account_ids.iter().find(|value| !crate::state::is_account_id(value)) { return Err(invalid_parameter(format!(\"The value supplied for parameter accountIds is not valid: {value} is not a 12-digit Amazon Web Services account ID.\"))); }" — no `is_foreign_account` check follows.

The crate's rule it skips — src/state.rs:812-822: "Whether `account_id` ... names an account OTHER than the one this service answers for. This emulator presents exactly one standalone account ... so a resource owned by any other account is one no request can be answered about: a reader refuses it with [`CROSS_ACCOUNT_MESSAGE`] ... ONE test for the whole crate"; and src/state.rs:858-860: "if is_foreign_account(account_id) { return Err(CROSS_ACCOUNT_MESSAGE.to_string()); }".

What the accepted export then writes — src/state/export.rs:516-520: "The rows are the ONE account this emulator presents ([`engine::emulator_account_id`]): `accountIds` and `includeMemberAccounts` select whose recommendations go into the file, and this emulator ... holds no other account's analysis, so neither of them selects any row away or adds one" — while src/state/export.rs:292-296 writes "accountIds": plan.account_ids.clone() into the metadata file.

Test coverage of the rule for readers only — tests/all/cross_account_requests.rs:162-177 asserts every reader refuses another account with the cross-account message; the export operations are not in its `READERS` table (tests/all/cross_account_requests.rs:37-66).

## Steps to Reproduce

1. Create an S3 bucket in the emulator, UpdateEnrollmentStatus(status=Active) and wait for the analysis delay
2. Call GetAutoScalingGroupRecommendations with `{"accountIds": ["210987654321"]}` — 400 InvalidParameterValueException, "Invalid permission to access another account..."
3. Call ExportAutoScalingGroupRecommendations with `{"accountIds": ["210987654321"], "s3DestinationConfig": {"bucket": "<bucket>"}}`
4. Observe 200 with a jobId; poll DescribeRecommendationExportJobs to Complete and GetObject the export file: it holds the emulator account's Auto Scaling group rows, and the metadata object reports accountIds=["210987654321"]
