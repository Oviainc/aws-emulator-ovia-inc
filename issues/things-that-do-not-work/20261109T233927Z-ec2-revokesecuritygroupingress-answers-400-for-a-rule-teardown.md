Issue-Source: trace-sweep-divergence
Issue-Trace: emr-run-job-flow-step-writes-s3-output
Issue-Step: teardown#30
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: emr-run-job-flow-step-writes-s3-output
Issue-Shape-Id: com.amazonaws.emr#ElasticMapReduce
Issue-Key: ec2|RevokeSecurityGroupIngress|2xx->4xx|status
Issue-Commit: 01897137b22b96058352710896276b71185e2aeb

# EC2 RevokeSecurityGroupIngress answers 400 for a rule teardown AWS accepts with 200

> _Note: this issue was identified against the code base at commit `01897137b22b96058352710896276b71185e2aeb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RevokeSecurityGroupIngress

## Description
One net-new root cause.

Cause 1 of 1: RevokeSecurityGroupIngress rejects a valid revoke of ingress rules on the security groups used by an EMR cluster.
  AWS: 200 with RevokeSecurityGroupIngressResult.Return = true for a revoke whose target group and permission set are valid. Smithy (com.amazonaws.ec2#RevokeSecurityGroupIngress): input GroupId/GroupName, IpPermissions, SecurityGroupRuleIds, CidrIp/IpProtocol/FromPort/ToPort/SourceSecurityGroupName/SourceSecurityGroupOwnerId, DryRun; output Return (xmlName 'return'), UnknownIpPermissions (xmlName 'unknownIpPermissionSet') and RevokedSecurityGroupRules; the operation declares NO modeled errors. AWS is deliberately lenient here: a permission that is not present on the group is reported back in UnknownIpPermissions on a 200 response, not refused - so a 4xx is reserved for a malformed request or an unknown group, neither of which applies to the recorded teardown.
  Emulator: 400 where AWS answers 200. Both revokes in the fixture's teardown (two consecutive steps against the same cluster's groups) are refused identically; the replay reports the divergence on status alone, so the emulator's error code and message for this refusal are whatever its generic validation path emits and are not themselves recorded.
  Why: teardown is unusable. A client that created ingress rules (directly, or indirectly via a service such as EMR that provisions managed master/core security groups) cannot remove them, so the groups can never be emptied and a subsequent DeleteSecurityGroup is blocked by the dependency that the revoke was supposed to clear. Any scenario that cleans up after itself leaks security groups, and a caller that treats the documented 200/Return=true contract as success sees a hard failure on a request AWS never rejects.
  Hypothesis: the ingress rules the EMR cluster's managed security groups are described with are not materialised as revocable rules in EC2's own security-group state, so the revoke finds nothing to match and the handler refuses the request instead of answering 200 with the unmatched permissions echoed in UnknownIpPermissions.
  Fix: the RevokeSecurityGroupIngress handler and its validation in the ec2 service - a permission set that does not match a stored rule must be returned in UnknownIpPermissions on a 200 rather than refused, and the security-group rule state the EMR cluster provisioning path writes must be revocable through the same store.
  Traces: emr-run-job-flow-step-writes-s3-output#teardown#30 (same cluster also covers #teardown#31).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, bootstrap_signal=false); verdict=mismatch.

RevokeSecurityGroupIngress - emr-run-job-flow-step-writes-s3-output#teardown#30:
  status: expected 200, got 400
  divergence_key: service=ec2|RevokeSecurityGroupIngress|status=200->400|keys=status
  affected (2 steps in this cluster): emr-run-job-flow-step-writes-s3-output#teardown#30, emr-run-job-flow-step-writes-s3-output#teardown#31

The recorded response is a 200; the only diverging key is the status, so no expected-vs-actual body or error code is recorded for this step. AWS models no errors at all on this operation and reports unmatched permissions in UnknownIpPermissions on a 200, which is why the 400 is a divergence in itself.

## Steps to Reproduce
1. git clone the repository and boot the emulator built from that checkout.
2. Replay the simparity-wire-aws-v1 wire corpus.
3. Fixture emr-run-job-flow-step-writes-s3-output: run the fixture through its EMR cluster lifecycle so the cluster's security groups exist, then reach the teardown phase.
4. Step teardown#30 - RevokeSecurityGroupIngress against the cluster's security group with the ingress permission set the fixture recorded; observe 400 where AWS answers 200 with Return=true.
5. Step teardown#31 - the second revoke in the same teardown is refused the same way.
6. Re-replay emr-run-job-flow-step-writes-s3-output#teardown#30 to confirm the fix.
