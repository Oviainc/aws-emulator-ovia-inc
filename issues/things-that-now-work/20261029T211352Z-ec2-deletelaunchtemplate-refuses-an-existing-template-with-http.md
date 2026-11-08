Issue-Source: trace-sweep-divergence
Issue-Trace: autoscaling-deletion-protection-blocks-delete-until-cleared
Issue-Step: teardown#10
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: autoscaling-deletion-protection-blocks-delete-until-cleared autoscaling-lifecycle-hook-defaults-then-deleted-list-empty autoscaling-notification-configuration-defaults-then-deleted-list-empty autoscaling-op-contract-describe-policies autoscaling-tags-create-describe-delete autoscaling-update-group-partial-merges-and-clamps-desired-capacity autoscaling-warm-pool-defaults-then-deleted
Issue-Shape-Id: com.amazonaws.autoscaling#AutoScaling_2011_01_01
Issue-Key: ec2|DeleteLaunchTemplate|2xx->4xx|status
Issue-Commit: e6d9764b7c2fffef8d3218e52b84d80407a24432

# EC2 DeleteLaunchTemplate refuses an existing template with HTTP 400 instead of answering 200

> _Note: this issue was identified against the code base at commit `e6d9764b7c2fffef8d3218e52b84d80407a24432`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteLaunchTemplate

## Description
One net-new cause (1 of 1).

AWS: DeleteLaunchTemplate naming a launch template that exists — by LaunchTemplateId or by LaunchTemplateName, exactly one of the two — answers HTTP 200 with DeleteLaunchTemplateResponse/launchTemplate echoing the template as it was at deletion, and deletes all of its versions. Smithy (services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json): DeleteLaunchTemplate input {DryRun, LaunchTemplateId, LaunchTemplateName (len 3..128, pattern ^[a-zA-Z0-9\(\)\.\-/_]+$)}, output member LaunchTemplate (ec2QueryName LaunchTemplate), and the operation models NO error shapes — a not-found is an unmodeled ec2Query client error, so AWS reaching 200 here is the recorded contract for a template that is present.

Emulator: the same request answers HTTP 400 — a client error — so the template is never deleted. The divergence is a status-only mismatch: the recorded 200 body is never reached.

Why: a caller cannot delete a launch template it just created, so every workflow that provisions a launch template and cleans it up leaves the template behind and reports a failed teardown; a client treating 400 as a permanent request defect stops retrying and leaks the resource. In this corpus it fails the teardown phase of seven Auto Scaling fixtures.

Hypothesis (neither the model nor the recording states the mechanism, and the replay carries no body for this step): the handler's template resolution refuses before the removal — the ops handler resolves the id/name pair through resolve_launch_template_id and turns a failed resolution into a client error, so a template created earlier in the fixture is either not found under the name the delete uses or is no longer in the launch-template store by teardown time.

Fix: the EC2 DeleteLaunchTemplate handler's id/name resolution and the launch-template store it reads — a template that CreateLaunchTemplate accepted must resolve by either LaunchTemplateId or LaunchTemplateName until it is deleted.

Traces: autoscaling-deletion-protection-blocks-delete-until-cleared#teardown#10 (representative) plus autoscaling-lifecycle-hook-defaults-then-deleted-list-empty#teardown#10, autoscaling-notification-configuration-defaults-then-deleted-list-empty#teardown#11, autoscaling-op-contract-describe-policies#teardown#10, autoscaling-tags-create-describe-delete#teardown#10, autoscaling-update-group-partial-merges-and-clamps-desired-capacity#teardown#10, autoscaling-warm-pool-defaults-then-deleted#teardown#10 (same cluster, 7 steps). Code: services/service-com-amazonaws-ec2-amazonec2/src/ops/delete_launch_template.rs lines 139-187 (resolution at line 168-169, removal at 177-183) and crate::state::resolve_launch_template_id in services/service-com-amazonaws-ec2-amazonec2/src/state.rs.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the booted emulator (emulator_up true, /__admin/health 200): service ec2, operation DeleteLaunchTemplate, verdict mismatch, divergence_key 'service=ec2|DeleteLaunchTemplate|status=200->400|keys=status'. detail: 'status: expected 200, got 400'. Expected status 200 (recorded AWS answer, body DeleteLaunchTemplateResponse/launchTemplate), actual status 400. Representative step autoscaling-deletion-protection-blocks-delete-until-cleared#teardown#10; 7 steps in the cluster, all teardown deletes of a launch template the same fixture created. No stub_path and no owning issue on this cluster.

## Steps to Reproduce
1) Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1; watch the teardown phase of autoscaling-deletion-protection-blocks-delete-until-cleared (step teardown#10).
2) Directly: POST Action=CreateLaunchTemplate with a LaunchTemplateName and LaunchTemplateData, note the returned LaunchTemplateId and name.
3) POST Action=DeleteLaunchTemplate with that LaunchTemplateName (and separately with that LaunchTemplateId).
4) Expected: HTTP 200 with launchTemplate echoing the deleted template. Observed: HTTP 400, the template is not deleted.
