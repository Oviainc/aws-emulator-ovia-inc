Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codecommit__codepipeline__glue__iam__lambda.refarch-glue-fresh-repository-has-no-branches-or-default codecommit__codepipeline__glue__iam__lambda.refarch-glue-branch-via-put-file-and-create-commit-agree codecommit__codepipeline__glue__iam__lambda.refarch-glue-pipeline-creation-triggers-one-failed-source-execution codecommit__codepipeline__glue__iam__lambda.refarch-glue-deploy-reports-success-before-the-glue-run-finishes codecommit__codepipeline__glue__iam__lambda.refarch-glue-pipeline-source-and-deploy-configuration-round-trips
Issue-Key: codecommit__codepipeline__glue__iam__lambda|ListBranches|NotImplemented
Issue-Key: codecommit__codepipeline__glue__iam__lambda|CreateJob|NotImplemented
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# CodeCommit ListBranches and Glue CreateJob answer 501 NotImplemented, blocking the CodeCommit/CodePipeline/Glue reference-architecture suite (2 causes)

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListBranches

## Description
Two causes, both scaffold stubs reached on the first call of their phase, so none of the cross-service assertions in these five checks is ever evaluated.

Cause 1 of 2: codecommit ListBranches answers 501 NotImplemented.
AWS: Smithy (com-amazonaws-codecommit-codecommit-20150413) ListBranches takes repositoryName (@required) plus nextToken, is @paginated (items: branches) and returns 200 with a branches list; it models RepositoryDoesNotExistException / InvalidContinuationTokenException (client, 400) for bad input. Recorded behavior: a created-but-unseeded repository returns 200 with an EMPTY branches list (that is how a caller distinguishes an empty repository from a missing one), and after one commit on main the same call returns ['main'].
Emulator: 'An error occurred (NotImplemented) when calling the ListBranches operation: operation ListBranches is scaffolded but not implemented in this emulator'.
Why: branch enumeration is the first thing a pipeline's Source stage and any repo-inspection tool do; an unreachable ListBranches means a caller cannot tell an empty repository from a missing one, which is exactly the diagnosis these reference-architecture checks reproduce (a Glue pipeline failing because no main branch exists). It also aborts the checks before their CodePipeline/Glue/IAM assertions run.
Fix: the scaffold stub services/service-com-amazonaws-codecommit-codecommit-20150413/src/ops/list_branches.rs (including its paginated empty-list answer).
Traces: codecommit__codepipeline__glue__iam__lambda.refarch-glue-fresh-repository-has-no-branches-or-default, codecommit__codepipeline__glue__iam__lambda.refarch-glue-branch-via-put-file-and-create-commit-agree, codecommit__codepipeline__glue__iam__lambda.refarch-glue-pipeline-creation-triggers-one-failed-source-execution.

Cause 2 of 2: glue CreateJob answers 501 NotImplemented.
AWS: glue CreateJob returns 200 with {Name} for a valid Command/Role and refuses bad input with InvalidInputException / duplicates with AlreadyExistsException (400). Recorded behavior in these checks: the job is created and the pipeline's deploy stage then starts a run against it.
Emulator: 'An error occurred (NotImplemented) when calling the CreateJob operation: operation CreateJob is scaffolded but not implemented in this emulator'.
Why: the deploy half of the reference architecture is unreachable — the pipeline cannot be pointed at a Glue job, so neither the configuration round-trip nor the "deploy reports success before the Glue run finishes" timing property can be observed. Triage note: Glue CreateJob is already named as a 501 stub by 20261022T131025Z-glue-createjob-is-a-501-stub-gettable-ignores and by 20261022T154846Z-emr-glue-s3-cross-service-suite-cfn-declared (cause 3); it is recorded here because these two checks are on neither issue's replay list.
Fix: the scaffold stub services/service-com-amazonaws-glue-awsglue/src/ops/create_job.rs.
Traces: codecommit__codepipeline__glue__iam__lambda.refarch-glue-deploy-reports-success-before-the-glue-run-finishes, codecommit__codepipeline__glue__iam__lambda.refarch-glue-pipeline-source-and-deploy-configuration-round-trips.

## Evidence
Cause 1 — expected 200 with branches == [] for an unseeded repository and ['main'] after one commit; actual botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListBranches operation: operation ListBranches is scaffolded but not implemented in this emulator (3 checks, raised through the list_branches paginator).
Cause 2 — expected 200 {Name}; actual botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateJob operation: operation CreateJob is scaffolded but not implemented in this emulator (2 checks).
AWS answers these calls with HTTP 200 (or a modeled 400 for bad input); the emulator answers HTTP 501.

## Steps to Reproduce
1. Boot the emulator and point botocore at it.
2. cc.create_repository(repositoryName=R, repositoryDescription='ETL source code repository for Glue jobs').
3. list(cc.get_paginator('list_branches').paginate(repositoryName=R)) -> expect one page with branches == []; observe 501 NotImplemented.
4. cc.put_file(repositoryName=R, branchName='main', filePath='etl.py', fileContent=b'print("x")\n', commitMessage='seed'); repeat step 3 -> expect ['main'].
5. glue.create_job(Name=J, Role=<role arn>, Command={'Name':'glueetl','ScriptLocation':'s3://b/etl.py'}) -> expect 200 {'Name': J}; observe 501 NotImplemented.
6. Re-replay the check ids in trace_ids.
