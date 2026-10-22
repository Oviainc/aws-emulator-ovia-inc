Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codecommit__events__sqs.pull-request-lifecycle-reaches-eventbridge-scoped-to-one-repository codecommit__events__sqs.a-commit-refused-for-a-stale-parent-moves-no-reference-and-emits-no-state-change-event
Issue-Key: codecommit__events__sqs|CreateBranch|NotImplemented
Issue-Key: codecommit__events__sqs.a-commit-refused-for-a-stale-parent-moves-no-reference-and-emits-no-state-change-event|_drain#16|Timeout|resp = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# CodeCommit CreateBranch is a 501 stub and no CodeCommit repository/pull-request events reach EventBridge (2 causes)

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateBranch

## Description
Two causes behind the codecommit__events__sqs failures.

Cause 1 of 2: codecommit CreateBranch answers 501 NotImplemented, so the pull-request eventing check never reaches an assertion.
AWS: Smithy (com-amazonaws-codecommit-codecommit-20150413) CreateBranch takes repositoryName, branchName and commitId (all @required) and returns 200 with an empty body, creating refs/heads/<branchName> at that commit; it models BranchNameExistsException / CommitDoesNotExistException / BranchNameIsTagNameException (client, 400) for bad input.
Emulator: 'An error occurred (NotImplemented) when calling the CreateBranch operation: operation CreateBranch is scaffolded but not implemented in this emulator'.
Why: a pull request needs a source branch, so nothing downstream — CreatePullRequest, the pullRequestCreated / pullRequestSourceBranchUpdated / pullRequestStatusChanged events on the default bus, and the per-repository scoping of the EventBridge rule — can be exercised at all.
Fix: the scaffold stub services/service-com-amazonaws-codecommit-codecommit-20150413/src/ops/create_branch.rs.
Traces: codecommit__events__sqs.pull-request-lifecycle-reaches-eventbridge-scoped-to-one-repository.

Cause 2 of 2: a CodeCommit write that moves a reference publishes no 'CodeCommit Repository State Change' event, so a consumer waiting on the default bus waits forever.
AWS: recorded behavior only (eventing is not in the Smithy model). CodeCommit sends 'CodeCommit Repository State Change' events with source 'aws.codecommit' to the default event bus with no opt-in; the accepted PutFile that moves refs/heads/main produces an event whose detail carries event (referenceCreated/referenceUpdated), referenceName 'refs/heads/main' and the commitId PutFile returned, while a PutFile refused for a stale parentCommitId (ParentCommitIdOutdatedException) moves no reference and emits nothing.
Emulator: the check does not complete — the suite reports 'Check timed out after 60.0s' while it long-polls the SQS target of an EventBridge rule matching that detail-type. Hypothesis: the codecommit service publishes no EventBridge events at all, so the positive leg's poll can never be satisfied and the check burns its whole deadline.
Why: every event-driven CodeCommit integration (a build triggered by a push, a mirroring consumer, an audit pipeline) sees nothing happen; worse, a consumer cannot distinguish "no event because the write was refused" — the property under test — from "this service never emits events".
Fix: event publication on reference-moving writes in the codecommit crate's write path (src/ops/put_file.rs, delete_file.rs, create_commit.rs) into the EventBridge default bus, emitted only after the write is accepted.
Traces: codecommit__events__sqs.a-commit-refused-for-a-stale-parent-moves-no-reference-and-emits-no-state-change-event.

## Evidence
Cause 1 — expected 200 from codecommit.CreateBranch; actual botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateBranch operation: operation CreateBranch is scaffolded but not implemented in this emulator (HTTP 501 where AWS answers 200).
Cause 2 — expected at least one SQS message carrying an EventBridge envelope with detail-type 'CodeCommit Repository State Change', source 'aws.codecommit', detail.referenceName 'refs/heads/main' and detail.commitId equal to the commit PutFile returned; actual: 'Check timed out after 60.0s' with no delivery observed.

## Steps to Reproduce
1. Boot the emulator and point botocore at it.
2. cc.create_repository(repositoryName=R); cc.put_file(... branchName='main' ...) to get a commit id.
3. cc.create_branch(repositoryName=R, branchName='feature', commitId=<that id>) -> expect 200; observe 501 NotImplemented.
4. Create an SQS queue, an EventBridge rule on {'source': ['aws.codecommit'], 'detail-type': ['CodeCommit Repository State Change']} with the queue as target, then run an accepted PutFile on main -> expect a matching message within seconds; observe none (the poll runs to the deadline).
5. Re-replay the check ids in trace_ids.
