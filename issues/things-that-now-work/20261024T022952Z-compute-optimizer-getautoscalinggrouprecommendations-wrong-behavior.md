Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: d5cb6525aec300e57e039668d831e20baadf847e

# Report Auto Scaling group ARNs in the Region of the endpoint the request addressed

_Filed: 2026-10-24T02:29:52Z_
_Operation: GetAutoScalingGroupRecommendations | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d5cb6525aec300e57e039668d831e20baadf847e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Compute Optimizer is a regional service: the endpoint a request addresses is the Region whose resources it answers for, and every ARN the answer carries names that Region. `GetAutoScalingGroupRecommendations` ignores the request's Region entirely — it discards `ctx` (`let _ = ctx;`) and reports the ARN exactly as seeded, which `seed_auto_scaling_group_analyses()` hardcodes to `us-east-1`.

Expected (and what this service's other readers do): a call signed for `us-west-2` reports the same groups under `arn:aws:autoscaling:us-west-2:...` ARNs, and a group ARN naming another Region selects nothing at this endpoint.
Actual: a call signed for `us-west-2` reports `arn:aws:autoscaling:us-east-1:...` ARNs; conversely a client that correctly passes the `us-west-2` ARN of a group in `autoScalingGroupArns` gets it reported in `errors` with code `ResourceNotFound`, while a `us-east-1` ARN is honoured from any endpoint.

`GetEBSVolumeRecommendations`, `GetLambdaFunctionRecommendations` and `GetRDSDatabaseRecommendations` all resolve `crate::state::request_region(ctx)` and rewrite/match the reported ARN against it (each with a dedicated test), so this reader is the odd one out and two readers of the same emulator disagree about where the account's resources live.

## Evidence

Spec/semantics: Compute Optimizer is a regional endpoint service (`endpointPrefix` compute-optimizer, per-Region endpoints) and `AutoScalingGroupRecommendation$autoScalingGroupArn` is "The Amazon Resource Name (ARN) of the Auto Scaling group" — a Region-qualified identifier. The crate states the rule itself in `src/state.rs`: "a row records a resource's REGION-FREE identity, because Compute Optimizer is regional: the ARN a reader reports names the region of the endpoint the request addressed (`AnalysedResource::arn_in`, `request_region`)". Handler: `src/ops/get_auto_scaling_group_recommendations.rs` line 185 `let _ = ctx;` and lines 316-321/574 use `group.arn` verbatim; `src/state.rs` `seed_auto_scaling_group_analyses()` fixes `let region = "us-east-1"`. Contrast `src/ops/get_ebs_volume_recommendations.rs` line 184, `src/ops/get_lambda_function_recommendations.rs` line 186 and `src/ops/get_rds_database_recommendations.rs` line 310. Tests: `tests/all/ebs_volume_recommendations.rs::a_recommendation_names_its_volume_in_the_region_the_request_was_signed_for`, `tests/all/lambda_function_recommendations.rs::the_arns_reported_name_the_region_the_request_addressed` and `tests/all/rds_database_recommendations.rs::the_reported_arn_names_the_region_the_endpoint_serves` pin this rule for the other readers; `tests/all/auto_scaling_group_recommendations.rs` never calls `call_in_region`, so nothing covers it here.

## Steps to Reproduce

1. Boot the compute-optimizer emulator and opt the account in with UpdateEnrollmentStatus(status=Active)
2. Call GetEBSVolumeRecommendations against the us-west-2 endpoint (SigV4 credential scope us-west-2) — reported volume ARNs are arn:aws:ec2:us-west-2:...
3. Call GetAutoScalingGroupRecommendations against the same us-west-2 endpoint — observe autoScalingGroupArn values of arn:aws:autoscaling:us-east-1:...
4. Call GetAutoScalingGroupRecommendations against us-west-2 passing autoScalingGroupArns=[the us-west-2 ARN of a reported group] — observe it comes back in errors with code ResourceNotFound instead of as a recommendation
