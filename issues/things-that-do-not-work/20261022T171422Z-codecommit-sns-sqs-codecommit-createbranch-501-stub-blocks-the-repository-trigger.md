Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codecommit__sns__sqs.repository-trigger-fires-on-a-real-ref-update
Issue-Key: codecommit__sns__sqs|CreateBranch|NotImplemented
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# CodeCommit CreateBranch 501 stub blocks the repository-trigger-to-SNS/SQS notification check

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateBranch

## Description
Cause 1 of 1: codecommit CreateBranch answers 501 NotImplemented, and it is the setup call that gives this check a second, unlisted ref, so no part of the trigger-notification behavior is exercised.

AWS: Smithy (com-amazonaws-codecommit-codecommit-20150413) CreateBranch takes repositoryName, branchName and commitId (all @required) and returns 200 with an empty body, creating refs/heads/<branchName> at that commit; it models BranchNameExistsException, CommitDoesNotExistException and BranchNameIsTagNameException (client, httpError 400). Recorded behavior downstream of it: a repository trigger scoped to 'main' with an SNS destination delivers, for a real ref update, one record per update with eventSource 'aws:codecommit', eventName 'ReferenceChanges', the eventTriggerName and the eventTriggerConfigId that PutRepositoryTriggers returned, and references[0].ref 'refs/heads/main' — and nothing at all for a commit on the unlisted branch.

Emulator: 'An error occurred (NotImplemented) when calling the CreateBranch operation: operation CreateBranch is scaffolded but not implemented in this emulator' — HTTP 501 where AWS answers 200.

Why: a client cannot create a branch through the API at all, so the branch-scoping half of the trigger contract (deliver for main, stay silent for every other ref) is unverifiable and no notification assertion can run. Triage note: the per-operation stub issue filed from the checkout names CreateBranch for the single-service codecommit suite; this check is not on its replay list.

Fix: the scaffold stub services/service-com-amazonaws-codecommit-codecommit-20150413/src/ops/create_branch.rs.

Traces: codecommit__sns__sqs.repository-trigger-fires-on-a-real-ref-update.

## Evidence
Expected: codecommit.create_branch(repositoryName=R, branchName=<other>, commitId=<existing commit>) returns HTTP 200 with an empty body.
Actual: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateBranch operation: operation CreateBranch is scaffolded but not implemented in this emulator (HTTP 501).

## Steps to Reproduce
1. Boot the emulator and point botocore at it.
2. cc.create_repository(repositoryName=R); cc.put_file(repositoryName=R, branchName='main', filePath='f.txt', fileContent=b'x', commitMessage='seed') -> note the commitId.
3. cc.create_branch(repositoryName=R, branchName='unlisted', commitId=<that id>) -> expect 200; observe 501 NotImplemented.
4. Re-replay the check id in trace_ids.
