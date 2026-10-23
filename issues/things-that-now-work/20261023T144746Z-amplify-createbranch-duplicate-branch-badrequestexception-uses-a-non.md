Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: amplify.concurrent-duplicate-branch-creation
Issue-Shape-Id: com.amazonaws.amplify#Amplify
Issue-Key: amplify.concurrent-duplicate-branch-creation|run#11|AssertionError|assert failures[0][1] == (
Issue-Commit: e2dfe022d58b39493003b3d90859f84840a12f00

# Amplify CreateBranch duplicate-branch BadRequestException uses a non-AWS message template

> _Note: this issue was identified against the code base at commit `e2dfe022d58b39493003b3d90859f84840a12f00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateBranch

## Description
One root cause, grounded in 1 failing behavioral check.

Cause 1 of 1: when CreateBranch is called for an appId+branchName that already exists, the emulator raises the right error code with the wrong message template.

AWS: Smithy model com.amazonaws.amplify#CreateBranch — POST /apps/{appId}/branches, modeled errors BadRequestException (httpError 400), DependentServiceFailureException (503), InternalFailureException (500), LimitExceededException (429), NotFoundException (404), UnauthorizedException (401); `appId` (required, httpLabel, pattern ^d[a-z0-9]+$) and `branchName` (required, len 1..255) are the two members that identify the branch. The model does not spell out the duplicate-name message, so this is recorded behavior only: the recording shows the refusal carried error code `BadRequestException` with a message built from the request's own members in a fixed template — "Failed to create branch. The branch <branchName> already exists for the app <appId>". The requirement is that template (literal prefix "Failed to create branch. The branch ", then the request's branchName, then " already exists for the app ", then the request's appId), not the particular recorded ids; the status (400) and code (BadRequestException) already match.

Emulator: the same conflict is refused with code `BadRequestException`, but with the message "Branch <branchName> already exists on app <appId>" — a different template with a different prefix and the word "on" instead of "for the app". Observed from the replay: ('BadRequestException', 'Branch dup-branch-eed449069a80 already exists on app d1kbo6g0s84864').

Why: the code and status are right, so SDK error-type dispatch still works; what breaks is anything that reads the message. Amplify's duplicate-branch message is request-derived and stable, so callers and agent harnesses do substring-match it to distinguish "this branch already exists" from the many other conditions Amplify also reports as a bare BadRequestException (rate exceeded, invalid branch name, invalid app). Against the emulator those matches fail and the conflict is misclassified as a generic bad request.

Fix: the CreateBranch handler's duplicate-name rejection path — services/service-com-amazonaws-amplify-amplify/src/ops/create_branch.rs — should format the BadRequestException message from the request's branchName and appId using AWS's template. Nothing else about the response changes: status stays 400 and the code stays BadRequestException. The rest of the check (exactly one of two concurrent creates wins, ListBranches shows one branch, DeleteBranch then makes GetBranch/UpdateBranch answer NotFoundException and frees the name for reuse) passed and is not part of this issue.

Traces: amplify.concurrent-duplicate-branch-creation.

## Evidence
Expected (recorded AWS): the losing one of two concurrent CreateBranch(appId, branchName) calls is refused with ('BadRequestException', 'Failed to create branch. The branch <branchName> already exists for the app <appId>') — message built from the request members.
Actual (emulator): ('BadRequestException', 'Branch dup-branch-eed449069a80 already exists on app d1kbo6g0s84864').

Replay assertion:
  File "check_amplify_concurrent_duplicate_branch_creation.py", line 41, in run
    assert failures[0][1] == (...)
  AssertionError: [('error', ('BadRequestException', 'Branch dup-branch-eed449069a80 already exists on app d1kbo6g0s84864'))]

Error code and HTTP status (400 BadRequestException) match AWS; only the message template diverges, and that template is request-derived and therefore reproducible.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. amplify.create_app(name='<any>') -> appId.
3. Issue two concurrent amplify.create_branch(appId=appId, branchName='dup-branch-x') calls (ThreadPoolExecutor, 2 workers).
4. Exactly one fails — inspect its response['Error']: Code is BadRequestException (correct) but Message is 'Branch dup-branch-x already exists on app <appId>' instead of 'Failed to create branch. The branch dup-branch-x already exists for the app <appId>'.
5. Equivalently, a second sequential create_branch with the same appId+branchName reproduces it without concurrency.
