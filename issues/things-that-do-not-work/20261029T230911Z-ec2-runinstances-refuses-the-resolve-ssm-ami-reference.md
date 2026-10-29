Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.asgeni-secondary-eni-cross-az-attach-rejected ec2.create-network-insights-access-scope
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|RunInstances|InvalidAMIID.Malformed
Issue-Key: ec2|CreateNetworkInsightsAccessScope|IdempotentParameterMismatch
Issue-Commit: 60c9edaff1d8a24fe639ae9745da5c1e78ca4ab9

# EC2: RunInstances refuses the resolve:ssm: AMI reference, and the CreateNetworkInsightsAccessScope idempotency-conflict message is emulator-authored (2 causes)

> _Note: this issue was identified against the code base at commit `60c9edaff1d8a24fe639ae9745da5c1e78ca4ab9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunInstances

## Description
Two independent, net-new fidelity defects in EC2 operations that DO answer (no scaffold stub is involved). BOTH must be closed before submitting.

Background for both: EC2 models no error shapes at all (Smithy services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json — RunInstances errors: [], CreateNetworkInsightsAccessScope errors: []), so the ec2Query error CODE plus its MESSAGE on the wire is the entire contract a client can branch on. A wrong code or a reworded message is a wrong response, not a cosmetic difference.

=== Cause 1 of 2: RunInstances rejects an SSM-parameter AMI reference instead of resolving it ===
Failing check id: ec2.asgeni-secondary-eni-cross-az-attach-rejected

AWS: RunInstances accepts, in ImageId, an SSM parameter reference of the form `resolve:ssm:<parameter-path>` (optionally `resolve:ssm:<parameter-path>:<version>` or `:<label>`) in place of a literal `ami-...` id, resolves that parameter to the AMI id it holds, and launches from the resolved AMI. The public AWS-published parameter namespace `/aws/service/ami-amazon-linux-latest/...` is the canonical way clients pin "latest Amazon Linux" without hard-coding a per-Region AMI id. The Smithy model says nothing about this form — RunInstancesRequest$ImageId documents only "The ID of the AMI. An AMI ID is required to launch an instance and must be specified here or in a launch template." and RunInstances models no errors — so this is recorded behavior only: the recording shows RunInstances called with ImageId = `resolve:ssm:/aws/service/ami-amazon-linux-latest/al2023-ami-kernel-default-x86_64` succeeding and returning a reservation, which is why the check gets past setup and on to its actual subject (secondary-ENI cross-AZ attachment). The general requirement: an ImageId beginning `resolve:ssm:` must be resolved against the SSM parameter it names BEFORE any `ami-`-prefix syntax check runs; only a path that resolves to nothing may be refused, and then for the unresolvable parameter, not for AMI-id syntax.

Emulator: the request is refused with HTTP 400, code `InvalidAMIID.Malformed`, message `Invalid id: "resolve:ssm:/aws/service/ami-amazon-linux-latest/al2023-ami-kernel-default-x86_64" (expecting "ami-...")`. In that message the fixed text is `Invalid id: "` … `" (expecting "ami-...")` and the only variable part is the ImageId echoed verbatim from the request — i.e. the handler ran a literal `ami-` prefix test over the parameter reference and never attempted resolution. Every launch that pins an AMI through the public SSM alias is therefore impossible.

Why: a client relying on AWS's behaviour cannot launch an instance at all through the idiomatic, Region-portable AMI reference. CDK, CloudFormation, launch templates and plain `aws ec2 run-instances` all emit `resolve:ssm:/aws/service/ami-amazon-linux-latest/...` by default, so the first call of almost every tutorial-shaped workload is refused with a code (`InvalidAMIID.Malformed`) that tells the client its own input is syntactically broken — so no retry, fallback or repair path can recover, and the failure is misattributed to the caller.

Fix: ImageId resolution in the RunInstances handler's input validation — resolve the `resolve:ssm:` form (against the SSM public-parameter namespace the emulator serves) before the AMI-id syntax check, and only refuse when the named parameter does not resolve. Related open issue that does NOT cover this: 20261022T235108Z-ec2-misclassifies-rejections-and-skips-validation-malformed-ids.md, whose Cause 8 is the opposite direction on a different operation (CreateLaunchTemplate ACCEPTS an ImageId alias that cannot resolve); it carries no replay key for this check and names no RunInstances resolution path.

Traces: ec2.asgeni-secondary-eni-cross-az-attach-rejected (grounding check; replay it). File citations: services/service-com-amazonaws-ec2-amazonec2/src/ops/run_instances.rs (the handler that emits the refusal; its module doc already states a LaunchTemplate launch is answered 501, confirming no alias/template resolution exists on this path).

=== Cause 2 of 2: CreateNetworkInsightsAccessScope's IdempotentParameterMismatch message is reworded ===
Failing check id: ec2.create-network-insights-access-scope

AWS: replaying a ClientToken that a previous CreateNetworkInsightsAccessScope used, with materially different request parameters, is refused with code `IdempotentParameterMismatch` and the message `The request has the same client token as a previous request, but the request parameters are not the same.` That message is entirely fixed text — it carries no value from the request or from state, so there is no template to fill. Smithy: CreateNetworkInsightsAccessScope.ClientToken targets String, is `required` with `clientOptional: true` and `@idempotencyToken`; the operation models errors: [], so the code and message are the whole observable contract.

Emulator: the code is right (`IdempotentParameterMismatch`) but the message is emulator-authored prose: `Parameters on this idempotent request are inconsistent with parameters used in previous request(s)`.

Why: a client (or an operator runbook, or a log-matching alarm) that recognises AWS's sentence does not recognise the emulator's, so idempotency-conflict handling validated against the emulator does not fire against AWS and vice versa. Because EC2 models no error shapes, the message is part of the response contract and not a diagnostic nicety.

Fix: the idempotency-conflict error construction in the CreateNetworkInsightsAccessScope handler (or the shared idempotency-mismatch message helper it calls) — emit AWS's fixed sentence verbatim.

Traces: ec2.create-network-insights-access-scope (grounding check; replay it). File citations: services/service-com-amazonaws-ec2-amazonec2/src/ops/create_network_insights_access_scope.rs.

## Evidence
Cause 1 — replay verdict for ec2.asgeni-secondary-eni-cross-az-attach-rejected: `botocore.exceptions.ClientError: An error occurred (InvalidAMIID.Malformed) when calling the RunInstances operation: Invalid id: "resolve:ssm:/aws/service/ami-amazon-linux-latest/al2023-ami-kernel-default-x86_64" (expecting "ami-...")`.
Expected (AWS, recorded): RunInstances with ImageId = `resolve:ssm:/aws/service/ami-amazon-linux-latest/al2023-ami-kernel-default-x86_64` answers 200 with a reservation whose instance was launched from the AMI that parameter resolves to; the check then proceeds to its real subject (attaching a secondary ENI at device index 1 in the instance's zone and having a cross-zone ENI refused with InvalidParameterCombination, leaving device indexes [0, 1]).
Actual (emulator): HTTP 400, code InvalidAMIID.Malformed, with the ImageId echoed back and the `(expecting "ami-...")` suffix — the `resolve:ssm:` reference was never resolved; it was run through a literal `ami-` prefix test.

Cause 2 — replay verdict for ec2.create-network-insights-access-scope: `AssertionError: expected message 'The request has the same client token as a previous request, but the request parameters are not the same.', got 'Parameters on this idempotent request are inconsistent with parameters used in previous request(s)'`.
Expected (AWS): code IdempotentParameterMismatch, message `The request has the same client token as a previous request, but the request parameters are not the same.` (all fixed text, no substituted values).
Actual (emulator): code IdempotentParameterMismatch (correct), message `Parameters on this idempotent request are inconsistent with parameters used in previous request(s)` (emulator-authored wording).

## Steps to Reproduce
Cause 1:
1. ec2:RunInstances with MinCount=1, MaxCount=1, InstanceType=t3.micro, SubnetId=<a subnet you created>, and ImageId='resolve:ssm:/aws/service/ami-amazon-linux-latest/al2023-ami-kernel-default-x86_64'.
2. Expected: 200 and a reservation whose instance uses the AMI that parameter resolves to. Actual: 400 InvalidAMIID.Malformed, 'Invalid id: "resolve:ssm:/aws/service/ami-amazon-linux-latest/al2023-ami-kernel-default-x86_64" (expecting "ami-...")'.
3. Replay the suite check ec2.asgeni-secondary-eni-cross-az-attach-rejected: it dies in setup on this call and never reaches its cross-AZ ENI assertion.

Cause 2:
1. ec2:CreateNetworkInsightsAccessScope with ClientToken='<some-token>' and one MatchPath set; it answers 200.
2. Call ec2:CreateNetworkInsightsAccessScope again with the SAME ClientToken but a materially different MatchPaths/ExcludePaths body.
3. Expected: code IdempotentParameterMismatch with message 'The request has the same client token as a previous request, but the request parameters are not the same.' Actual: the same code but message 'Parameters on this idempotent request are inconsistent with parameters used in previous request(s)'.
4. Replay the suite check ec2.create-network-insights-access-scope.
