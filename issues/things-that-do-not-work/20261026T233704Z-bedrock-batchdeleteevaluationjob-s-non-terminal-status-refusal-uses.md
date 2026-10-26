Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock.batch-delete-evaluation-job-running-conflict bedrock.deploy-inference-profile-cfn-routes-invocation
Issue-Shape-Id: com.amazonaws.bedrock#AmazonBedrockControlPlaneService
Issue-Key: bedrock|BatchDeleteEvaluationJob|ValidationException
Issue-Key: bedrock|CreateStack|ValidationError
Issue-Commit: 281440c6fd2fb349634dcc135fbec4e15edad452

# bedrock: BatchDeleteEvaluationJob's non-terminal-status refusal uses the emulator's own wording, and CloudFormation refuses AWS::Bedrock::ApplicationInferenceProfile as an unrecognized resource type (2 causes)

> _Note: this issue was identified against the code base at commit `281440c6fd2fb349634dcc135fbec4e15edad452`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchDeleteEvaluationJob, CreateStack

## Description
Two net-new root causes in the bedrock behavioral surface. Both services answer (bedrock is bootstrapped), and neither cause is covered by an open issue: the existing 20261024T230826Z-bedrock-an-accepted-automated-evaluation-job-is-pinned-inprogress.md covers the evaluation-job LIFECYCLE (status never leaving InProgress), not the refusal wording below, and 20261023T004328Z-bedrock-agent-runtime-lambda-s3vectors-cloudformation-stacks-declaring-aws-lambda-function-reach-create.md covers AWS::Lambda::Function stacks that DO reach CREATE_COMPLETE, which is the opposite symptom of cause 2.

=== Cause 1 of 2: BatchDeleteEvaluationJob refuses a non-terminal job with a message AWS does not use ===
Failing check: bedrock.batch-delete-evaluation-job-running-conflict

AWS: Smithy BatchDeleteEvaluationJob (POST /evaluation-jobs/batch-delete, success 202) models ValidationException (HTTP 400) and documents "An evaluation job can only be deleted if it has following status FAILED, COMPLETED, and STOPPED." For a batch whose jobs are not all in one of those statuses, Amazon Bedrock refuses the WHOLE request with a top-level 400 ValidationException whose message is, per the recorded behavior this suite replays, entirely FIXED text with no variable parts at all:
  "All provided evaluation jobs in the request are not supported by batch deletion. Please re-submit with evaluation jobs that exists and under FAILED COMPLETED STOPPED status."
Note what that message does and does not say: it does not name the offending job, and it does not name the status the job currently holds — it is a single request-level statement about the batch. The two sentences are the requirement; no id, ARN, status or count from the request or from service state is interpolated into it.

Emulator: the status code and error code are right (400 ValidationException), but the message is the emulator's own composition, with two variable parts interpolated from the request and from stored state — the addressed job's ARN and its current status:
  "Evaluation job: <jobIdentifier ARN> is in status <stored status> and cannot be deleted. An evaluation job can only be deleted if it has following status FAILED, COMPLETED, and STOPPED."
(As replayed: "Evaluation job: arn:aws:bedrock:us-west-2:000000000000:evaluation-job/000001bux901 is in status InProgress and cannot be deleted. ..." — the ARN and "InProgress" there are that one run's values filling the emulator's template, not part of any requirement.) So the divergence is: wrong fixed text, plus two interpolated values AWS's message for this error class does not carry.

Why: a client that keys on this message — or a test/asserting SDK wrapper that matches AWS's documented refusal string — cannot recognise the refusal, and a client that scrapes the message for the offending job id will read a field AWS never supplies here and break the day the emulator is swapped for AWS (or vice versa). The refusal class itself is reachable, so this is a pure wording divergence on an error a caller is expected to handle.

Fix: the operation's handler — services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/ops/batch_delete_evaluation_job.rs, the ValidationException built in the per-identifier deletability loop. The requirement is one fixed, request-level sentence pair for the whole batch, not a per-job sentence built from the identifier and its status.
Hypothesis (NOT observed, because the check aborted at the message assertion above): the same check's later positive-control leg keys the response's evaluationJobs/errors entries on the SHORT job id (job_arn.rsplit('/')[-1]) because AWS does not echo the ARN back there, while the handler pushes job.arn into BatchDeleteEvaluationJobItem.jobIdentifier. If so that is a second divergence in the same handler and should be closed with it; re-replaying the check after fixing the message will confirm or refute it.

=== Cause 2 of 2: CloudFormation does not recognise AWS::Bedrock::ApplicationInferenceProfile, so CreateStack rejects the template outright ===
Failing check: bedrock.deploy-inference-profile-cfn-routes-invocation

AWS: AWS::Bedrock::ApplicationInferenceProfile is a published CloudFormation resource type. CreateStack therefore ACCEPTS a template declaring it — template validation does not reject the type — and the stack proceeds to provisioning, where (recorded behavior only, reproduced across accounts, and what this check pins) the resource handler refuses with AccessDenied: the stack ends in a CREATE_FAILED/ROLLBACK state and DescribeStackEvents reports a *_FAILED event for the logical resource whose ResourceStatusReason names AccessDenied for AWS::Bedrock::ApplicationInferenceProfile, and no inference profile is left behind. The positive control in the same check establishes the account is otherwise healthy: the direct bedrock CreateInferenceProfile call succeeds and returns status ACTIVE.

Emulator: CreateStack never gets as far as provisioning. It refuses the request at template-format validation with
  400 ValidationError: "Template format error: Unrecognized resource types: [AWS::Bedrock::ApplicationInferenceProfile]"
so there is no stack, no stack events and no CREATE_FAILED reason to read. The fixed text of that message is CloudFormation's own "Template format error: Unrecognized resource types: [...]" form and the bracketed list is the variable part taken from the template — i.e. the emulator is emitting a correctly-shaped message for a condition that must not hold for this type.

Why: every client that deploys Bedrock application inference profiles through infrastructure-as-code is blocked at the wrong layer and gets the wrong diagnosis. A caller expecting AWS's shape sees CreateStack fail synchronously instead of a stack it can describe, so it cannot distinguish "this resource type does not exist" from "creating it was denied" — the two have completely different remediations — and any template that merely MENTIONS the type (even under a false Condition, or alongside resources the emulator does support) is rejected whole.

Fix: the CloudFormation crate's recognised-resource-type registry and the template validator that emits "Unrecognized resource types" (services/service-com-amazonaws-cloudformation-cloudformation/src/state/template.rs and the provisioning path it hands off to, services/service-com-amazonaws-cloudformation-cloudformation/src/state/provisioning.rs). The type must be recognised at parse/validate time so the stack is created, and its provisioning must surface the refusal as a resource-level *_FAILED event with an AccessDenied ResourceStatusReason naming the type, leaving no inference profile behind.

Traces: bedrock.batch-delete-evaluation-job-running-conflict (cause 1), bedrock.deploy-inference-profile-cfn-routes-invocation (cause 2).

## Evidence
Cause 1 — bedrock.batch-delete-evaluation-job-running-conflict [mismatch]
  Expected (AWS, fixed text, no interpolated values): code=ValidationException (HTTP 400), message=
    "All provided evaluation jobs in the request are not supported by batch deletion. Please re-submit with evaluation jobs that exists and under FAILED COMPLETED STOPPED status."
  Actual (emulator): code=ValidationException (HTTP 400), message=
    "Evaluation job: arn:aws:bedrock:us-west-2:000000000000:evaluation-job/000001bux901 is in status InProgress and cannot be deleted. An evaluation job can only be deleted if it has following status FAILED, COMPLETED, and STOPPED."
  Replay assertion: "AssertionError: expected the rejection to name the terminal statuses it requires, got: Evaluation job: arn:... is in status InProgress and cannot be deleted. ..."
  Source of the divergence: services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/ops/batch_delete_evaluation_job.rs:214-218 formats the message from `{identifier}` and `{status}`.
  (The ARN, the account id and "InProgress" above are that run's values; the requirement is the fixed sentence pair.)

Cause 2 — bedrock.deploy-inference-profile-cfn-routes-invocation [error]
  Expected (AWS): CreateStack accepts the template declaring AWS::Bedrock::ApplicationInferenceProfile; the stack is created and then fails during provisioning, DescribeStackEvents reporting a *_FAILED event whose ResourceStatusReason contains "AccessDenied" and names AWS::Bedrock::ApplicationInferenceProfile; no inference profile named by the template exists afterwards.
  Actual (emulator): CreateStack raises synchronously —
    botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::Bedrock::ApplicationInferenceProfile]
  Positive control in the same check passed up to that point: bedrock.CreateInferenceProfile(modelSource.copyFrom=<SYSTEM_DEFINED profile ARN>) succeeded with status ACTIVE, so the Bedrock side of the type works and only the CloudFormation path is unreachable.

## Steps to Reproduce
Cause 1 (BatchDeleteEvaluationJob message):
1. CreateEvaluationJob with an automated evaluationConfig (dataset Builtin.BoolQ, metric Builtin.Accuracy), an inferenceConfig naming a foundation-model ARN, and an s3 outputDataConfig.
2. GetEvaluationJob and confirm status is a non-terminal status (InProgress).
3. Call BatchDeleteEvaluationJob(jobIdentifiers=[<job ARN>]).
4. Observe the 400 ValidationException. Expected message is exactly "All provided evaluation jobs in the request are not supported by batch deletion. Please re-submit with evaluation jobs that exists and under FAILED COMPLETED STOPPED status."; the emulator instead returns a per-job sentence interpolating the identifier and its stored status.
Replay: run_behavioral_suite -> bedrock.batch-delete-evaluation-job-running-conflict

Cause 2 (CFN resource type):
1. bedrock.ListInferenceProfiles(typeEquals="SYSTEM_DEFINED", maxResults=1) and take the first inferenceProfileArn; CreateInferenceProfile(modelSource={copyFrom: <that ARN>}) as a positive control — it succeeds, status ACTIVE.
2. cloudformation.CreateStack with a template whose Resources declares one `Type: AWS::Bedrock::ApplicationInferenceProfile` with InferenceProfileName and ModelSource.CopyFrom, and an Output using !GetAtt Profile.InferenceProfileArn.
3. Observe CreateStack itself fails with ValidationError "Template format error: Unrecognized resource types: [AWS::Bedrock::ApplicationInferenceProfile]" instead of creating a stack that then reaches a CREATE_FAILED state whose DescribeStackEvents reason names AccessDenied for that type.
Replay: run_behavioral_suite -> bedrock.deploy-inference-profile-cfn-routes-invocation
