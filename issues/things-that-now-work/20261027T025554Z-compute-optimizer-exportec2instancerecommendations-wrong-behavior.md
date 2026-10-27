Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 4e0be56b24c0fc13c424c1a1b18bb5f0120957ed

# Export rows list InferredWorkloadTypes even when the inferred-workload-types preference is Inactive, contradicting the reader and the sibling export row

_Filed: 2026-10-27T02:55:54Z_
_Operation: ExportEC2InstanceRecommendations | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `4e0be56b24c0fc13c424c1a1b18bb5f0120957ed`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`analysed_resource_row` — the row builder for every findings-catalog export (Ec2Instance, EbsVolume, LambdaFunction, EcsService, RdsDBInstance) — writes the `InferredWorkloadTypes` column unconditionally from the stored analysis. Every other place in the crate gates that value on the effective preference: the Auto Scaling group export row does (`if preferences.infers_workload_types()`), and so does the reader of the same instance recommendations (`inferred_workload_types: preferences.infers_workload_types().then(...)`). After a PutRecommendationPreferences that sets `inferredWorkloadTypes=Inactive` for Ec2Instance, GetEC2InstanceRecommendations omits the inferred types while an export file of the same recommendations still lists them — and the SAME row reports `EffectiveRecommendationPreferencesInferredWorkloadTypes=Inactive` beside them, so the file contradicts itself. Because the export filter matcher reads the row's columns, an `InferredWorkloadTypes` filter also still selects on types the account switched off.

## Evidence

Unconditional in the findings-catalog row — services/service-com-amazonaws-computeoptimizer-computeoptimizerservice/src/state/export.rs:1216-1221:
"    put_list(\n        &mut row,\n        \"InferredWorkloadTypes\",\n        &resource.inferred_workload_types,\n    );\n    put_preferences(&mut row, preferences);"
where put_preferences writes the contradicting cell, src/state/export.rs:692-696: `put(row, "EffectiveRecommendationPreferencesInferredWorkloadTypes", preferences.inferred_workload_types.clone())`.

Gated one line of code away, in the sibling row builder — src/state/export.rs:1003-1008: "if preferences.infers_workload_types() { put_list(&mut row, \"InferredWorkloadTypes\", &group.inferred_workload_types,".

Gated in the reader of the same recommendations — src/ops/get_ec2_instance_recommendations.rs:634-639: "// \"The applications that might be running on the instance as inferred by Compute Optimizer\" — reported unless the account deactivated the inference with a recommendation preference.\n        inferred_workload_types: preferences.infers_workload_types().then(|| instance.inferred_workload_types.clone()),".

The rule the export states for itself — src/state/export.rs:26-28: "Each cell is that field's value for that recommendation, computed from the stored analysis the same way the recommendation READER computes it, so an exported row and a `Get*Recommendations` response cannot disagree."

No test pins the Inactive case: grep for `inferred_workload|Inactive` under tests/all/ec2_instance_recommendations_export.rs matches only the filter-vocabulary row (:379) and an opt-out status (:592).

## Steps to Reproduce

1. Enroll the account (UpdateEnrollmentStatus status=Active) and create the destination bucket.
2. PutRecommendationPreferences {"resourceType":"Ec2Instance","inferredWorkloadTypes":"Inactive"}.
3. Call GetEC2InstanceRecommendations and observe `inferredWorkloadTypes` is absent from each recommendation.
4. Call ExportEC2InstanceRecommendations with an s3DestinationConfig, poll DescribeRecommendationExportJobs to Complete and read the CSV.
5. Observe the row still fills the InferredWorkloadTypes column with the analysis' inferred types while EffectiveRecommendationPreferencesInferredWorkloadTypes reads Inactive in the same row.
