Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: quicksight.iam-policy-assignment-concurrent-creation-contention
Issue-Shape-Id: com.amazonaws.quicksight#QuickSight_20180401
Issue-Key: quicksight.iam-policy-assignment-concurrent-creation-contention|run#11|AssertionError|assert outcome in ("success", "ConcurrentUpdatingException"), (name, outcome)
Issue-Commit: 482573dd66a8fdd59e0387c95d89889e67d3b0e1

# CreateIAMPolicyAssignment refuses concurrent creates with NotImplemented (501) instead of succeeding or raising ConcurrentUpdatingException

> _Note: this issue was identified against the code base at commit `482573dd66a8fdd59e0387c95d89889e67d3b0e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateIAMPolicyAssignment

## Description
Cause 1 of 1: quicksight CreateIAMPolicyAssignment answers every call — including the concurrent ones this check fires — with the scaffold refusal `NotImplemented`, so the operation's documented contention semantics are never exercised and no assignment is ever persisted.

AWS: Smithy model (com.amazonaws.quicksight#QuickSight_20180401) binds CreateIAMPolicyAssignment to POST /accounts/{AwsAccountId}/namespaces/{Namespace}/iam-policy-assignments with HTTP 200 on success, returning AssignmentName, AssignmentId, AssignmentStatus, PolicyArn and Identities. Its modeled errors are AccessDeniedException (401), ConcurrentUpdatingException (500), InternalFailureException (500), InvalidParameterValueException (400), ResourceExistsException (409), ResourceNotFoundException (404), ThrottlingException (429) — `NotImplemented` is not among them. When several creates in the same AwsAccountId/Namespace race, each call either succeeds (200, assignment subsequently visible via DescribeIAMPolicyAssignment with the AssignmentName, AssignmentStatus and PolicyArn exactly as submitted) or is rejected with ConcurrentUpdatingException; a racer rejected that way must leave NO assignment behind, so a later DescribeIAMPolicyAssignment for it must fail with ResourceNotFoundException (404) whose message is the fixed text `Cannot find an assignment named ` + the requested AssignmentName + `.` (the assignment name is the only variable part, taken from the request; the rest is fixed text).

Emulator: every racer's create is refused with error code `NotImplemented` and the message `operation CreateIAMPolicyAssignment is scaffolded but not implemented in this emulator` (HTTP 501). The check's assertion `outcome in ("success", "ConcurrentUpdatingException")` fails with `AssertionError: ('s2r-race-0-<suffix>', 'NotImplemented')`. Nothing is written, so neither the success path (Describe returns the assignment) nor the contention path (Describe returns ResourceNotFoundException with the message above) can be observed.

Why: a client cannot create IAM policy assignments at all, and a client written against AWS's contention contract (retry on ConcurrentUpdatingException, treat everything else as fatal) sees an unmodeled error code with a 5xx/501 status it cannot classify, so it aborts rather than retrying, and any store-integrity invariant it relies on (a rejected create leaves no assignment) is untestable.

Fix: the CreateIAMPolicyAssignment handler and the service's assignment state — services/service-com-amazonaws-quicksight-quicksight-20180401/src/ops/create_iam_policy_assignment.rs must stop returning the scaffold error and must write the assignment into state under (account, namespace, assignment name) with serialization/contention handling so concurrent creates either commit or are rejected with the modeled ConcurrentUpdatingException and commit nothing; DescribeIAMPolicyAssignment must read that state back and raise ResourceNotFoundException with AWS's message wording for an absent name.

NOTE: the open issue `20261029T191733Z-quicksight-createiampolicyassignment-stub.md` asks for the same handler to be implemented, but its Issue-Trace-Ids list only `quicksight.iam-policy-assignment-status-transitions`; this check was reported as a separate surviving mismatch and is not carried by that issue, and it adds the concurrency/contention and rejected-racer-leaves-nothing requirements above. Fixing the stub is necessary but not sufficient to close this one — the concurrent-creation check must be replayed too.

Traces: quicksight.iam-policy-assignment-concurrent-creation-contention.

## Evidence
Expected (AWS): six concurrent CreateIAMPolicyAssignment calls with AssignmentStatus=DRAFT and PolicyArn=arn:aws:iam::aws:policy/ReadOnlyAccess in account/namespace `default` each return HTTP 200 or the modeled ConcurrentUpdatingException; at least one succeeds; every succeeded name is then returned by DescribeIAMPolicyAssignment with AssignmentName == the requested name, AssignmentStatus == DRAFT, PolicyArn == the requested policy ARN; every rejected name yields ResourceNotFoundException (HTTP 404) with message `Cannot find an assignment named <AssignmentName>.`

Actual (emulator): every call fails with error code `NotImplemented`, message `operation CreateIAMPolicyAssignment is scaffolded but not implemented in this emulator`, HTTP 501. Replay output:

  File "check_quicksight_iam_policy_assignment_concurrent_creation_contention.py", line 53, in run
    assert outcome in ("success", "ConcurrentUpdatingException"), (name, outcome)
  AssertionError: ('s2r-race-0-0d9e986306a5', 'NotImplemented')

(The `s2r-race-0-...` suffix is a per-run generated name and is not part of the requirement.)

## Steps to Reproduce
1. Boot the quicksight emulator.
2. Ensure the account subscription exists (DescribeAccountSubscription / CreateAccountSubscription with Edition=ENTERPRISE, AuthenticationMethod=IAM_ONLY).
3. From 6 threads concurrently call CreateIAMPolicyAssignment(AwsAccountId=<account>, Namespace="default", AssignmentName=<unique per thread>, AssignmentStatus="DRAFT", PolicyArn="arn:aws:iam::aws:policy/ReadOnlyAccess").
4. Observe every call returns HTTP 501 / error code NotImplemented instead of a 200 or ConcurrentUpdatingException.
5. Call DescribeIAMPolicyAssignment for each name and observe no assignment was persisted, and check the ResourceNotFoundException message wording.
6. Equivalent: replay check quicksight.iam-policy-assignment-concurrent-creation-contention.
