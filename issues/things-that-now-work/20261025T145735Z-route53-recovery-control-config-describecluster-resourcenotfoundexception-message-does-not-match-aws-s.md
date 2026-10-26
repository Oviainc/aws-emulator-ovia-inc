Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53-recovery-control-config.describe-cluster-nonexistent-not-found
Issue-Shape-Id: com.amazonaws.route53recoverycontrolconfig#Route53RecoveryControlConfig
Issue-Key: route53-recovery-control-config.describe-cluster-nonexistent-not-found|run#9|AssertionError|assert message == f"No record corresponding to cluster Arn({bogus_arn})", (
Issue-Commit: 881ecd6ad77d32fb02ed5ba0e17f4d2faca09b25

# DescribeCluster ResourceNotFoundException message does not match AWS's 'No record corresponding to cluster Arn(<arn>)' wording

> _Note: this issue was identified against the code base at commit `881ecd6ad77d32fb02ed5ba0e17f4d2faca09b25`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeCluster

## Description
ONE root cause in the route53-recovery-control-config emulator, grounded in one failing behavioral check.

=== Cause 1 of 1: DescribeCluster on a syntactically-valid but nonexistent ClusterArn returns the right class and status but the wrong message text ===

AWS: recorded behavior only for the message text. DescribeCluster against a well-formed ClusterArn in the caller's own account that names no existing cluster is refused with error code ResourceNotFoundException at HTTP 404, and the message is a fixed sentence template whose only variable part is the ClusterArn taken verbatim from the request:
    No record corresponding to cluster Arn(<ClusterArn>)
Fixed text (a requirement, exactly as written): the literal prefix `No record corresponding to cluster Arn(` and the literal closing `)`. Variable part: `<ClusterArn>` — the request's ClusterArn value, echoed unchanged. Nothing else in the message varies; there is no id, timestamp or generated name in it. Smithy (services/service-com-amazonaws-route53recoverycontrolconfig-route53recoverycontrolconfig/smithy-model/com-amazonaws-route53recoverycontrolconfig-route53recoverycontrolconfig.json): DescribeCluster is GET /cluster/{ClusterArn} with ClusterArn an @httpLabel, required; its modeled errors include ResourceNotFoundException (errorKind client, httpError 404, member Message -> jsonName 'message', required). The model fixes the class and the status (both of which the emulator already gets right) but says nothing about the sentence, so the sentence above is the recorded requirement.

Emulator: returns the correct code (ResourceNotFoundException) and status (404) but a different sentence, of the form `no cluster is stored under <arn>` — lower-case, different wording, and the ARN appended bare instead of wrapped in `Arn(...)`. Observed message: 'no cluster is stored under arn:aws:route53-recovery-control::000000000000:cluster/00000000-0000-0000-0000-000000000000'. The check's positive control (DescribeCluster on a real DEPLOYED cluster) passed, so the lookup path itself works and only the not-found message is wrong.

Why: a client relying on AWS's behavior gets the wrong human- and machine-readable diagnosis. ARC's own documentation and runbooks quote this sentence; tooling and tests that assert on or parse the not-found message (extracting the ARN from the `Arn(...)` capture, or matching the 'No record corresponding to' prefix to distinguish a missing cluster from other 404s) do not match at all, so error-path assertions written against AWS fail against the emulator and log scrapers mis-classify the failure. The message is part of the error contract, so a divergence in it is a defect even though the code and status are right.

Fix: the DescribeCluster handler's cluster-lookup not-found branch (the error construction for the unknown-ClusterArn case) must build the ResourceNotFoundException message from the fixed template `No record corresponding to cluster Arn(<ClusterArn>)`, substituting the request's ClusterArn. If this wording is emitted by a shared not-found helper for cluster ARNs in this service, that helper is the place to fix it — but do not change the class or the status, which already match.

Relation to existing open issues (neither covers this): 20261023T065046Z-route53-recovery-control-config-wire-error-class-and.md carries a DescribeCluster message key, but its DescribeCluster cause is the CROSS-ACCOUNT ARN path, which must become 403 AccessDeniedException — a different request class and a different expected class/status from this one, which is a same-account nonexistent ARN that must stay 404 ResourceNotFoundException. 20261022T140811Z-route53-recovery-control-config-wrong-error-classes-on.md covers CreateCluster/CreateControlPanel/CreateRoutingControl/DeleteCluster error classes and does not touch DescribeCluster. Fixing either of those does not change this message.

Traces: route53-recovery-control-config.describe-cluster-nonexistent-not-found.

## Evidence
Behavioral suite run (suite simparity-behavioral-ga-prod-v1; emulator serving, bootstrap_signal=false; route53-recovery-control-config 4/5, 1 failed).

Failing check [mismatch] route53-recovery-control-config.describe-cluster-nonexistent-not-found
  check_route53_recovery_control_config_describe_cluster_nonexistent_not_found.py line 35:
    assert message == f"No record corresponding to cluster Arn({bogus_arn})"
  AssertionError: expected the exact ResourceNotFoundException message AWS returns, got 'no cluster is stored under arn:aws:route53-recovery-control::000000000000:cluster/00000000-0000-0000-0000-000000000000'

Per-field comparison for DescribeCluster(ClusterArn=<well-formed, nonexistent ARN in the caller's own account>):
  error code: expected 'ResourceNotFoundException', got 'ResourceNotFoundException'  (matches — the check's earlier `assert code == "ResourceNotFoundException"` passed)
  HTTP status: 404 expected and 404 observed (matches; ResourceNotFoundException httpError 404)
  message: expected 'No record corresponding to cluster Arn(<ClusterArn>)' — fixed text 'No record corresponding to cluster Arn(' + the request's ClusterArn + ')' — got 'no cluster is stored under <ClusterArn>'

In the expected message the ONLY value drawn from the request is the ClusterArn; the recorded instance 'arn:aws:route53-recovery-control::000000000000:cluster/00000000-0000-0000-0000-000000000000' is one filling of that slot (account id and the all-zero cluster uuid come from the check's own request), not part of the requirement.

Positive control in the same check passed: DescribeCluster on the suite's shared DEPLOYED cluster returned Cluster.ClusterArn equal to the requested ARN, so this is specifically the not-found message and not a broken DescribeCluster.

Model references used: DescribeCluster GET /cluster/{ClusterArn}, ClusterArn @httpLabel required; errors AccessDeniedException(403), ConflictException(409), InternalServerException(500), ResourceNotFoundException(404), ThrottlingException(429), ValidationException(400). ResourceNotFoundException: structure, errorKind client, httpError 404, required member Message (jsonName 'message').

## Steps to Reproduce
Against the emulator's route53-recovery-control-config endpoint (boto3 client 'route53-recovery-control-config'):

1. Positive control — create (or adopt) a cluster and wait until DescribeCluster reports Status == 'DEPLOYED'; DescribeCluster(ClusterArn=<that arn>) must return Cluster.ClusterArn == <that arn>. This passes today.
2. Build a well-formed but nonexistent ARN in the CALLER'S OWN account (so this is not the cross-account path): acct = sts.get_caller_identity()['Account']; bogus = f"arn:aws:route53-recovery-control::{acct}:cluster/00000000-0000-0000-0000-000000000000".
3. Call DescribeCluster(ClusterArn=bogus) and capture the ClientError.
4. Observed: code 'ResourceNotFoundException' at HTTP 404 (both correct) with Error.Message 'no cluster is stored under <bogus>'.
   Expected: code 'ResourceNotFoundException' at HTTP 404 with Error.Message exactly f'No record corresponding to cluster Arn({bogus})'.

Re-replay check route53-recovery-control-config.describe-cluster-nonexistent-not-found; it must pass before this issue is closed.
