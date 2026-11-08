Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codecommit.approval-rule-name-conflict-rejected codecommit.cannot-delete-approval-rule-from-template-blocked codecommit.client-request-token-required-rejected codecommit.create-pull-request-missing-token-rejected codecommit.update-pull-request-title-after-close-rejected
Issue-Shape-Id: com.amazonaws.codecommit#CodeCommit_20150413
Issue-Key: codecommit|CreatePullRequest|SourceAndDestinationAreSameException
Issue-Commit: 5772abe834f24ca0a7d173ddfaf967982b48def5

# CodeCommit CreatePullRequest refuses two differently-named branches whose tips are the same commit with SourceAndDestinationAreSameException

> _Note: this issue was identified against the code base at commit `5772abe834f24ca0a7d173ddfaf967982b48def5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePullRequest

## Description
Cause 1 of 1: CreatePullRequest applies SourceAndDestinationAreSameException to a second, unmodeled condition — the two references naming DIFFERENT branches that happen to point at the same tip commit — and so refuses a pull request AWS opens. Failing checks: codecommit.approval-rule-name-conflict-rejected, codecommit.cannot-delete-approval-rule-from-template-blocked, codecommit.client-request-token-required-rejected, codecommit.create-pull-request-missing-token-rejected, codecommit.update-pull-request-title-after-close-rejected.

AWS: SourceAndDestinationAreSameException is modeled for ONE condition. Smithy: CodeCommit CreatePullRequest error SourceAndDestinationAreSameException (HTTP 400), documentation "The source branch and destination branch for the pull request are the same. You must specify different branches for the source and destination." The branch identity is the condition — the names (after the usual refs/heads/ qualification) must differ; the model says nothing about the two branches' tip commits, and the recorded behavior of all five checks is that a pull request whose sourceReference is a branch created at the destination branch's current tip is CREATED and returned with pullRequestStatus OPEN (HTTP 200), then appears in ListPullRequests with pullRequestStatus=OPEN. Each of those checks first does PutFile/CreateCommit on `main`, then CreateBranch of a second branch AT that commit id, then CreatePullRequest(source=<second branch>, destination=main) and proceeds to assert on the created pull request — that is the recorded AWS behavior the checks encode.

Emulator: CreatePullRequest compares the two resolved tip commit ids and, when they are equal, answers HTTP 400 SourceAndDestinationAreSameException with the message "The source branch name and the destination branch name are either the same, or the tips of both branches are identical. Make sure that your source branch and destination branch have unique names and that the tips of the branches are different." (fixed text; the message has no variable parts). No pull request is created. The same refusal is raised for the modeled condition (identical reference names), which is correct; it is only the identical-tip arm that is a defect. Note the refusal happens AFTER the idempotency-token checks, so in two of the checks the token rejection it asserts passes and only the following positive-control CreatePullRequest is refused.

Why: a client that opens a pull request as the FIRST step of a review workflow — the normal "branch off main, open the PR, then push commits / add approval rules / close it" order — cannot create the pull request at all against the emulator; it receives a 400 for a request AWS accepts. Everything downstream of the pull request id (approval rules, approval-rule templates applied on creation, UpdatePullRequestTitle/Description, UpdatePullRequestStatus, pull-request events) becomes unreachable, which is why five otherwise unrelated checks die on this single call. Code that distinguishes SourceAndDestinationAreSameException (a caller mistake, worth surfacing) from a transient failure will report a bogus configuration error to the user.

Fix: the pre-write validation in the CreatePullRequest handler — drop the identical-tip arm and raise SourceAndDestinationAreSameException only when the source and destination references resolve to the same branch/reference name; a pull request between distinct branches at the same tip must be created normally (it simply has no differences). The shared message constant and the same-name arm stay as they are.

Traces: codecommit.approval-rule-name-conflict-rejected, codecommit.cannot-delete-approval-rule-from-template-blocked, codecommit.client-request-token-required-rejected, codecommit.create-pull-request-missing-token-rejected, codecommit.update-pull-request-title-after-close-rejected. Handler and arm cited: services/service-com-amazonaws-codecommit-codecommit-20150413/src/ops/create_pull_request.rs lines 574-580 (the `if source_commit == destination_commit` guard), with the intent stated in the module header at lines 7-12 and in the helper doc at lines 238-251; the correct same-name arm is at lines 553-558.

## Evidence
Expected (AWS, as recorded by all five checks): CreatePullRequest(title=..., targets=[{repositoryName: <repo>, sourceReference: "feature", destinationReference: "main"}]) where `feature` was created with CreateBranch at `main`'s current tip returns HTTP 200 with pullRequest.pullRequestId set and pullRequestStatus "OPEN"; the id then appears in ListPullRequests(pullRequestStatus="OPEN").

Actual (emulator): HTTP 400
  Code: SourceAndDestinationAreSameException
  Message: "The source branch name and the destination branch name are either the same, or the tips of both branches are identical. Make sure that your source branch and destination branch have unique names and that the tips of the branches are different."
and no pull request is created. botocore surfaces it as:
  botocore.errorfactory.SourceAndDestinationAreSameException: An error occurred (SourceAndDestinationAreSameException) when calling the CreatePullRequest operation: The source branch name and the destination branch name are either the same, or the tips of both branches are identical. ...

Smithy model for this error (codecommit 2015-04-13): SourceAndDestinationAreSameException, HTTP 400, documentation "The source branch and destination branch for the pull request are the same. You must specify different branches for the source and destination." — branch identity only; no tip-equality condition is modeled.

All five failing checks die on this one call: three ([error] approval-rule-name-conflict-rejected, cannot-delete-approval-rule-from-template-blocked) on the pull request they need as a fixture, and two ([error] client-request-token-required-rejected, create-pull-request-missing-token-rejected, update-pull-request-title-after-close-rejected) on the positive-control CreatePullRequest that follows an otherwise-correct token rejection.

## Steps to Reproduce
1. codecommit.CreateRepository(repositoryName=<repo>).
2. codecommit.PutFile(repositoryName=<repo>, branchName="main", filePath="README.md", fileContent=b"init") — note the returned commitId as <base>.
3. codecommit.CreateBranch(repositoryName=<repo>, branchName="feature", commitId=<base>) — both branches now point at <base>.
4. codecommit.CreatePullRequest(title="any title", targets=[{"repositoryName": <repo>, "sourceReference": "feature", "destinationReference": "main"}]).
   Expected: HTTP 200 with a pullRequest whose pullRequestStatus is OPEN; it is listed by ListPullRequests(repositoryName=<repo>, pullRequestStatus="OPEN").
   Observed: HTTP 400 SourceAndDestinationAreSameException with the "...or the tips of both branches are identical..." message, and ListPullRequests returns no new id.
5. Control that must keep its current behavior: the same call with sourceReference == destinationReference (both "main") still returns SourceAndDestinationAreSameException.
6. Replay: the five check ids in trace_ids.
