Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka.list-scram-secrets-permission-denied kafka.e2e-cluster-policy-cross-account-grant-then-revoke-flow
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka.list-scram-secrets-permission-denied|_error_code#9|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: kafka.e2e-cluster-policy-cross-account-grant-then-revoke-flow|run#18|TimeoutError|raise TimeoutError("GetBootstrapBrokers refused with AccessDeniedException before any grant: the run budget ended first; not graded")
Issue-Commit: ffd5f693012d77fe631946d21242c3542f59dce1

# Identity-based authorization is not evaluated for MSK reads: a zero-permission principal's ListScramSecrets and GetBootstrapBrokers succeed instead of being denied (2 causes)

> _Note: this issue was identified against the code base at commit `ffd5f693012d77fe631946d21242c3542f59dce1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListScramSecrets

## Description
TWO distinct root causes, both on the authorization path of an MSK read operation: a principal with no kafka permissions (or an explicit deny) is served a 200. Each operation's gate must be closed separately; both must be closed before this issue is submitted. The resolved issue 20261029T202809Z-kafka-msk-resource-authorization-is-never-evaluated-so-denied.md claimed MSK authorization evaluation; these two read paths are still unauthorized-but-served, and neither check id appears in it.

Cause 1 of 2: ListScramSecrets ignores an explicit identity-policy deny.
AWS: a caller whose identity policy explicitly denies kafka:ListScramSecrets is refused with AccessDeniedException (a plain IAM identity-policy denial; ForbiddenException is reserved for MSK's own resource-policy checks). The message follows AWS's standard explicit-deny template: the fixed connectives 'User: ', ' is not authorized to perform: ', ' on resource: ', ' with an explicit deny in an identity-based policy', with the variable parts taken from the caller and the request — the assumed-role session ARN of the signing principal, the action name 'kafka:ListScramSecrets' and the ClusterArn from the request. Removing the deny and granting the action makes the identical call succeed.
Emulator: HTTP 200 with {'SecretArnList': []} for the restricted principal — the request is served as if the deny did not exist.
Why: every negative authorization test against this operation passes vacuously, and a client that validates its least-privilege policies against the emulator ships a role that AWS refuses. Worse, the emulator discloses a cluster's secret list to a principal that must not read it, so an access-control regression is invisible.
Fix: the authorization decision for kafka:ListScramSecrets must be evaluated (identity-based policies of the signing principal, including explicit denies) in the handler's gate before the cluster's secret set is paged, raising the templated AccessDeniedException when it denies.
Traces: kafka.list-scram-secrets-permission-denied.

Cause 2 of 2: GetBootstrapBrokers by a principal with no identity permissions is never denied, so the cluster-policy grant/revoke flow is unobservable.
AWS: an assumed role with no kafka identity permissions and no cluster policy is refused GetBootstrapBrokers with AccessDeniedException and the message template 'User: <assumed-role session ARN> is not authorized to perform: kafka:GetBootstrapBrokers on resource: <the ClusterArn> because no identity-based policy allows the kafka:GetBootstrapBrokers action' — fixed connectives with the principal, action and cluster ARN filled from the caller and request. PutClusterPolicy granting that principal an explicit Allow makes the same call succeed purely through the resource policy, and DeleteClusterPolicy makes it fail again with the same denial.
Emulator: the denial never happens — the check polls for AccessDeniedException until its budget is spent and reports 'GetBootstrapBrokers refused with AccessDeniedException before any grant: the run budget ended first; not graded', i.e. the call kept succeeding for the zero-permission role, so neither the grant nor the revoke step can be measured.
Why: a client cannot use the emulator to verify that a cluster policy is what grants cross-account bootstrap access; the whole grant-then-revoke contract is untested, and an account that is in fact unauthorized reads broker endpoints.
Fix: the same authorization gate in GetBootstrapBrokers' handler — evaluate the signing principal's identity policies and the cluster's resource policy before answering, and deny with the templated AccessDeniedException when neither allows the action.
Traces: kafka.e2e-cluster-policy-cross-account-grant-then-revoke-flow.

## Evidence
Both checks are kafka behavioral checks (verdicts mismatch and error respectively); neither is marked ALREADY FILED or OWNED.

Cause 1 — kafka.list-scram-secrets-permission-denied: the default caller's ListScramSecrets succeeds (positive control), then a role created with an explicit deny of kafka:ListScramSecrets assumes and calls the same operation.
  Expected: ('AccessDeniedException', 'User: arn:aws:sts::<account>:assumed-role/<role>/<session> is not authorized to perform: kafka:ListScramSecrets on resource: <cluster ARN> with an explicit deny in an identity-based policy').
  Actual: AssertionError 'expected the call to be rejected, but it SUCCEEDED and returned {... HTTPStatusCode: 200 ..., \'SecretArnList\': []}'. The role, session and cluster ARN in the expected message are per-run values filling AWS's template.

Cause 2 — kafka.e2e-cluster-policy-cross-account-grant-then-revoke-flow: a role with no kafka identity permissions calls GetBootstrapBrokers on the cluster.
  Expected: AccessDeniedException with 'User: arn:aws:sts::<account>:assumed-role/<role>/<session> is not authorized to perform: kafka:GetBootstrapBrokers on resource: <cluster ARN> because no identity-based policy allows the kafka:GetBootstrapBrokers action'; then success after PutClusterPolicy Allow; then denial again after DeleteClusterPolicy.
  Actual: the polled call never returned AccessDeniedException — 'TimeoutError: GetBootstrapBrokers refused with AccessDeniedException before any grant: the run budget ended first; not graded'. The emulator answered the unauthorized caller instead of denying it.

## Steps to Reproduce
1. Boot the emulator, create a PROVISIONED cluster and wait for ACTIVE.
2. Cause 1: ListScramSecrets as the default caller and confirm 200 with an empty SecretArnList (positive control). Create an IAM role whose inline policy explicitly Denies kafka:ListScramSecrets, assume it, and call ListScramSecrets(ClusterArn=...) with those credentials. Assert AccessDeniedException with the explicit-deny message template. Then replace the deny with an Allow of kafka:ListScramSecrets and assert the identical call succeeds.
3. Cause 2: create a role with NO kafka permissions, assume it, and call GetBootstrapBrokers(ClusterArn=...). Assert AccessDeniedException with the 'because no identity-based policy allows the kafka:GetBootstrapBrokers action' message. Then PutClusterPolicy granting that role's ARN an Allow of kafka:GetBootstrapBrokers on the cluster and assert the same call now succeeds; DeleteClusterPolicy and assert it is denied again.
4. Replay kafka.list-scram-secrets-permission-denied and kafka.e2e-cluster-policy-cross-account-grant-then-revoke-flow.
