Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail__elbv2.obs-elbv2-3-refused-target-group-calls-are-audited-with-exception-suffixed-codes
Issue-Key: cloudtrail__elbv2|DescribeTags|NotImplemented
Issue-Commit: a59989a9f63c403b66485a4e93f54ab6df22c2a6

# ELBv2 DescribeTags answers NotImplemented

> _Note: this issue was identified against the code base at commit `a59989a9f63c403b66485a4e93f54ab6df22c2a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTags

## Description
There is 1 cause in this issue.

Cause 1 of 1 — elasticloadbalancingv2 DescribeTags is an unimplemented scaffold stub.
AWS: Smithy models DescribeTags on Elastic Load Balancing v2 (service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10) taking ResourceArns and returning TagDescriptions, with client errors such as TargetGroupNotFoundException / LoadBalancerNotFoundException (HTTP 400) for ARNs that name nothing. NotImplemented is not a modelled ELBv2 error code.
Emulator: ClientError (NotImplemented) when calling the DescribeTags operation: 'operation DescribeTags is scaffolded but not implemented in this emulator'. The check aborts there, before it can assert the exception-suffixed error codes that refused target-group calls are audited with.
Why: every tag read on a load balancer, target group, listener or rule fails with a code no SDK models, so no client can enumerate or verify ELBv2 tags, and tag-driven resource discovery is impossible.
Fix: implement DescribeTags in the ELBv2 crate's ops module (services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10/src/ops), returning TagDescriptions from tag state and the modelled not-found errors for unknown ARNs.
Traces: cloudtrail__elbv2.obs-elbv2-3-refused-target-group-calls-are-audited-with-exception-suffixed-codes.

The check id carries the composite service token 'cloudtrail__elbv2', which the per-service bootstrap gate cannot place by exact equality; both cloudtrail and elasticloadbalancingv2 are stood up and neither appears in the pending set, so this is filed rather than deferred.

## Evidence
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the DescribeTags operation: operation DescribeTags is scaffolded but not implemented in this emulator. Expected: HTTP 200 with a TagDescriptions list (or a modelled 400 not-found error for an unknown ARN).

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite.
2. Replay cloudtrail__elbv2.obs-elbv2-3-refused-target-group-calls-are-audited-with-exception-suffixed-codes.
3. Manually: create a target group with elbv2 and call elbv2 DescribeTags(ResourceArns=[<its ARN>]) -> expect 200 with TagDescriptions, observe NotImplemented.
