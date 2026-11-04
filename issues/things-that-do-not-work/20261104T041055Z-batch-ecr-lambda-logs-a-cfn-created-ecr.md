Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: batch__ecr__lambda__logs.e2e-diagnose-batch-job-no-logs-lifecycle batch__ecr__lambda__logs.refarch-batch-fargate-compute-environment-omits-vcpu-floors batch__ecr__lambda__logs.refarch-batch-submitted-job-has-no-execution-record batch__ecr__lambda__logs.refarch-batch-job-fails-before-any-container-on-empty-repository
Issue-Key: batch__ecr__lambda__logs|ListImages|RepositoryNotFoundException
Issue-Key: batch__ecr__lambda__logs.refarch-batch-fargate-compute-environment-omits-vcpu-floors|run#7|KeyError|assert cr['instanceTypes'] == [], (
Issue-Key: batch__ecr__lambda__logs.refarch-batch-submitted-job-has-no-execution-record|run#19|KeyError|assert job['attempts'] == [], (
Issue-Key: batch__ecr__lambda__logs.refarch-batch-job-fails-before-any-container-on-empty-repository|_poll#25|Timeout|return last
Issue-Commit: c2d690adb5c99cf0fe6b339d9b4e7b6ccf196d09

# batch__ecr__lambda__logs: a CFN-created ECR repository is invisible to ECR reads, DescribeComputeEnvironments/DescribeJobs omit empty collections, and an unpullable-image job never fails (4 causes)

> _Note: this issue was identified against the code base at commit `c2d690adb5c99cf0fe6b339d9b4e7b6ccf196d09`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeComputeEnvironments

## Description
Four net-new behavioral root causes grounded in failing checks of the simparity-behavioral-aws-v1 replay. Each must be closed before submitting.

Cause 1 of 4 — an ECR repository created by a CloudFormation stack does not exist as far as the ECR API is concerned.
AWS: a stack containing AWS::ECR::Repository that reaches CREATE_COMPLETE has created the repository; its Ref output is the repository name, and ecr ListImages on that name answers HTTP 200 with imageIds: [] for a repository that holds no images. ListImages only answers RepositoryNotFoundException when the repository really is absent from the registry.
Emulator: the stack reaches CREATE_COMPLETE and its EcrRepositoryName output is populated, but the very next read is refused — RepositoryNotFoundException "The repository with name '<repository name>' does not exist in the registry with id '<account id>'". The fixed text there is "The repository with name '" … "' does not exist in the registry with id '" … "'", with the repository name from the request and the registry id from the caller's account.
Why: the whole estate is built from that stack. A CREATE_COMPLETE stack whose resource is unreadable makes every downstream step (the job definition's image URI, the image count that explains the job's failure, the cleanup) fail against an object the operator was told exists. Related open issue 20261102T090835Z-batch-ecr-lambda-logs-batch-job-definition-validation-and-defaults-diverge-and.md reports that the ECR repository namespace is not region-scoped; it does not carry this check and does not cover a CFN-created repository being absent from the registry entirely.
Fix: the AWS::ECR::Repository resource provider in the cloudformation crate must create the repository in the ECR crate's registry state (same registry/account the ECR API reads) before reporting the resource created.
Traces: batch__ecr__lambda__logs.e2e-diagnose-batch-job-no-logs-lifecycle.

Cause 2 of 4 — DescribeComputeEnvironments omits the empty collections AWS returns in computeResources.
AWS: ComputeEnvironmentDetail.computeResources targets ComputeResource, whose EC2-only list members (instanceTypes, and likewise ec2Configuration) are present-but-empty for a FARGATE compute environment: a client reads computeResources["instanceTypes"] == [] and concludes no instance types and therefore no vCPU floors are configured. The member is a StringList, not an optional scalar.
Emulator: the member is absent from the response — the check fails with KeyError: 'instanceTypes' when reading computeResources['instanceTypes'] of a FARGATE CE.
Why: "absent" and "empty" are different answers. Idiomatic client code indexes a modeled list member directly, so an omitted empty list raises a KeyError/NPE instead of reading as "nothing configured", and any audit that asserts a Fargate CE declares no instance types or vCPU floors cannot run.
Fix: the DescribeComputeEnvironments response serializer in the batch crate — emit the modeled collection members as empty lists rather than omitting them.
Traces: batch__ecr__lambda__logs.refarch-batch-fargate-compute-environment-omits-vcpu-floors.

Cause 3 of 4 — DescribeJobs omits `attempts` for a job that has not attempted execution yet.
AWS: JobDetail carries `attempts` (AttemptDetails list). For a job that has been submitted but has made no attempt, AWS returns attempts: [] — an empty list, present — and a client reads len(job["attempts"]) to decide whether anything ran.
Emulator: the member is absent — the check fails with KeyError: 'attempts' when reading job['attempts'] of a freshly submitted job.
Why: the empty list is the evidence that the job has no execution record yet; omitting it turns that read into an exception, so a caller cannot distinguish "nothing has run" from a malformed response, and no attempt-count assertion can be made.
Fix: the DescribeJobs response serializer in the batch crate — always emit `attempts` (empty when the job has no attempt).
Traces: batch__ecr__lambda__logs.refarch-batch-submitted-job-has-no-execution-record.

Cause 4 of 4 — a job whose image cannot be pulled never reaches FAILED.
AWS: a Fargate job whose job definition points at an empty ECR repository goes SUBMITTED → RUNNABLE → STARTING → FAILED with a statusReason naming the image pull, and DescribeJobs reports exactly one attempt whose container carries no exitCode (no process ran).
Emulator: the job never settles — the check waiting on the terminal status exhausts the harness budget with no FAILED status and no statusReason. (Same family as the batch-service cause "a submitted container job never reaches a terminal status" filed for service heading `batch` in this sweep; filed here because this check is attributed to batch__ecr__lambda__logs and carries its own replay key.)
Why: the failure-diagnosis path of the service is unreachable: no terminal status, no statusReason, no attempt, so an operator cannot tell an image-pull failure from a job that is simply still queued.
Fix: the job state machine in the batch crate — place the job, fail it with an image-pull statusReason, and record the attempt.
Traces: batch__ecr__lambda__logs.refarch-batch-job-fails-before-any-container-on-empty-repository.

## Evidence
Behavioral replay (simparity-behavioral-aws-v1) against this checkout; batch__ecr__lambda__logs 7/15 passed.

Cause 1: _PhaseError: [phase 1/6 deploy] RepositoryNotFoundException: An error occurred (RepositoryNotFoundException) when calling the ListImages operation: The repository with name 'sim2real-refarch-batch-repo-18c59732056b' does not exist in the registry with id '000000000000' — raised after the check asserted the stack reached CREATE_COMPLETE and read its EcrRepositoryName output.

Cause 2: KeyError: 'instanceTypes' at `assert cr['instanceTypes'] == []` against the computeResources of a FARGATE compute environment.

Cause 3: KeyError: 'attempts' at `assert job['attempts'] == []` against a freshly submitted job.

Cause 4: Error: Check timed out after 60.0s while waiting for the submitted job to reach FAILED with an image-pull statusReason.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Cause 1: create a stack with an AWS::ECR::Repository (Ref output = name), wait for CREATE_COMPLETE, then aws ecr list-images --repository-name <that name> -> RepositoryNotFoundException; AWS answers 200 {"imageIds": []}.
3. Cause 2: create a MANAGED CE with computeResources.type FARGATE, then aws batch describe-compute-environments --compute-environments <name> -> computeResources has no instanceTypes member; AWS returns "instanceTypes": [].
4. Cause 3: submit a job and immediately aws batch describe-jobs --jobs <jobId> -> no attempts member; AWS returns "attempts": [].
5. Cause 4: point a FARGATE job definition at an empty ECR repository's URI, submit, and poll describe-jobs -> never FAILED.
6. Re-replay the check ids in trace_ids.
