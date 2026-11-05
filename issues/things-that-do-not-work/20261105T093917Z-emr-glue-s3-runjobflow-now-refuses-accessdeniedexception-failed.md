Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr__glue__s3.emr-cluster-instance-role-trust-mismatch-negative
Issue-Key: emr|RunJobFlow|AccessDeniedException
Issue-Commit: 6da489cbc4c2969d809761cd42ffd786edefa992

# emr__glue__s3: RunJobFlow now refuses AccessDeniedException 'Failed to authorize instance profile' for an instance profile that exists, so a cluster AWS launches is never created

> _Note: this issue was identified against the code base at commit `6da489cbc4c2969d809761cd42ffd786edefa992`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunJobFlow

## Description
REOPENS 20261102T083234Z-elasticmapreduce-emr-accepts-requests-aws-refuses-and-rewords-cluster.md: its fix left the call refused.

Cause 1 of 1: the instance-profile precondition added for that issue's Cause 2 now rejects a profile that exists and is resolvable, because its role's trust policy does not name ec2.amazonaws.com.
  AWS: RunJobFlow with a JobFlowRole naming an EXISTING instance profile is accepted (200 with JobFlowId/ClusterArn) even when the profile's role trusts a service other than ec2.amazonaws.com. The trust mismatch is not an API-time refusal: the cluster launches and the mismatch surfaces later, on the node, as the instance failing to obtain instance-metadata credentials — so a step that needs those credentials does not complete and writes nothing, while the sibling cluster whose role trusts ec2.amazonaws.com completes its step. AWS's AccessDeniedException "Failed to authorize instance profile <instanceProfileArn>" (fixed text plus the ARN derived from the request's JobFlowRole) belongs to a profile EMR cannot authorize at all — not to a profile that exists with a different trust principal. Smithy: RunJobFlow.JobFlowRole -> XmlString; RunJobFlow's modelled error list carries only InternalServerError, so the AccessDeniedException envelope is recorded behavior only.
  Emulator: 400 AccessDeniedException "Failed to authorize instance profile arn:aws:iam::<account>:instance-profile/<profile-name>" for the freshly created profile, so no cluster is created and the check cannot reach its steps at all.
  Why: a client testing IAM wiring (or any caller using a profile whose role trusts a non-EC2 principal, however deliberately) is refused at RunJobFlow where AWS launches the cluster; the whole negative-delivery scenario — step fails on the mistrusted role, succeeds on the correct one — becomes unreachable, and the precondition over-refuses rather than under-refuses.
  Fix: the instance-profile precondition in the RunJobFlow handler — it must refuse only when the named instance profile cannot be resolved/authorized at all, and must not treat the role's trust-policy principal as an API-time authorization failure.
  Traces: emr__glue__s3.emr-cluster-instance-role-trust-mismatch-negative.

## Evidence
Replay of simparity-behavioral-aws-v1, '### emr__glue__s3' (3/46 passed):

emr__glue__s3.emr-cluster-instance-role-trust-mismatch-negative — error: botocore.exceptions.ClientError: An error occurred (AccessDeniedException) when calling the RunJobFlow operation: Failed to authorize instance profile arn:aws:iam::000000000000:instance-profile/sim2real-emr-ec2-a8e5bda7e26a.

The check creates (via IAM) an EMR service role, an instance profile whose role trusts glue.amazonaws.com and a second whose role trusts ec2.amazonaws.com, then calls RunJobFlow for each with a step that writes a marker to S3 using instance-metadata credentials. Expected: both RunJobFlow calls return 200; the mistrusted cluster's step does not COMPLETE and 'proof/bad-ok.txt' stays absent (HeadObject 404), while the correctly-trusted cluster's step reaches COMPLETED and 'proof/good-ok.txt' contains 'ok\n'. Observed: the first RunJobFlow is refused 400 AccessDeniedException, so no cluster exists and nothing after it runs.

The fixed issue 20261102T083234Z-… Cause 2 required this refusal only for an instance profile that cannot be authorized (its recorded fixture used a profile EMR could not authorize at all); the implemented gate now also refuses an existing profile whose role trust names another service.

## Steps to Reproduce
1. git clone the repo and let the behavioral suite build and boot the emulator.
2. With IAM, create an EMR service role trusting elasticmapreduce.amazonaws.com, and two instance profiles: one whose role's AssumeRolePolicyDocument trusts glue.amazonaws.com, one whose role trusts ec2.amazonaws.com.
3. Call RunJobFlow with JobFlowRole set to the glue-trusting instance profile (plus a valid subnet and the service role) — expect 200 with a JobFlowId; observe 400 AccessDeniedException 'Failed to authorize instance profile <arn>'.
4. Repeat with the ec2-trusting profile to confirm the gate's discrimination.
5. Replay emr__glue__s3.emr-cluster-instance-role-trust-mismatch-negative to confirm.
