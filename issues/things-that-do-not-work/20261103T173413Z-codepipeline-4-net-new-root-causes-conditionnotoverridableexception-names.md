Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codepipeline.condition-not-overridable-rejected codepipeline.override-stage-condition-conflict-rejected codepipeline.put-job-success-result-completed-job-rejected codepipeline.put-job-success-result-wrong-job-state-rejected codepipeline.list-rule-executions-matches-condition-evaluation-order codepipeline.override-stage-condition-not-overridable-rejected codepipeline.job-output-variables-precision-preserved codepipeline.stop-completed-execution-rejected
Issue-Shape-Id: com.amazonaws.codepipeline#CodePipeline_20150709
Issue-Key: codepipeline.condition-not-overridable-rejected|run#13|AssertionError|assert message == (
Issue-Key: codepipeline.override-stage-condition-conflict-rejected|run#15|AssertionError|assert refusal == (
Issue-Key: codepipeline|PutJobSuccessResult|InvalidJobStateException
Issue-Key: codepipeline.list-rule-executions-matches-condition-evaluation-order|run#21|AssertionError|assert statuses == {name: "Succeeded" for name in declared_rule_names}, statuses
Issue-Key: codepipeline.override-stage-condition-not-overridable-rejected|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Key: codepipeline.job-output-variables-precision-preserved|run#21|Timeout|details = _poll_until(_probe, _lambda_action_done, timeout_s=120.0)
Issue-Key: codepipeline.stop-completed-execution-rejected|_poll_until#6|Timeout|time.sleep(interval_s)
Issue-Commit: d5f23b12615bd3516afbad21c99e563e38efe994

# CodePipeline: 4 net-new root causes — ConditionNotOverridableException names the execution id instead of the pipeline, a repeat PutJobSuccessResult on a Succeeded job is refused instead of idempotent, the second rule of a beforeEntry condition is Abandoned, and three executions never reach the state they are polled for

> _Note: this issue was identified against the code base at commit `d5f23b12615bd3516afbad21c99e563e38efe994`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
OverrideStageCondition

## Description
FOUR net-new root causes in the CodePipeline condition/override path, the job-result lifecycle, the condition rule evaluator and the execution plane. A fixer must close ALL FOUR before submitting; each cause names its own failing check ids below.

None of the eight failing checks grounding this issue is marked ALREADY FILED or OWNED in this replay, so no open issue carries their replay keys. Related but NOT covering: 20261029T183239Z-codepipeline-declaration-validation-and-response-shapes-10-net.md touches OverrideStageCondition but for other check ids and for a NotImplemented answer, not for this message template; 20261028T112857Z-codepipeline-job-worker-lifecycle-and-refusal-wording-8.md and 20261029T183239Z-codepipeline-refusal-classification-and-wording-10-net-new.md describe the same FAMILY as Cause 2 for OTHER check ids (put-job-success-invalid-job-state-rejected, put-job-failure-* …) and neither carries the two ids below; 20261031T033058Z-codepipeline-3-net-new-root-causes-a-terminated.md owns the stop-not-stoppable MESSAGE for codepipeline.stop-completed-execution-rejected, but in this replay that check never reaches the stop call at all (it is listed here under Cause 4 as a stall, a different cause and a different replay key); 20261025T032228Z-codepipeline-execution-engine-still-stalls-an-action-never.md describes the same FAMILY as Cause 4 from a different corpus (simparity-behavioral-ga-prod-v1) and the checks it lists now fail on new, concrete divergences (Causes 1-3) rather than on its recorded keys. A fixer closing any of those must still close these four causes and re-replay the ids in trace_ids.

=== Cause 1 of 4: ConditionNotOverridableException's message interpolates the pipelineExecutionId where AWS names the PIPELINE ===
AWS: Smithy (com.amazonaws.codepipeline#CodePipeline_20150709) models OverrideStageCondition(pipelineName, stageName, pipelineExecutionId, conditionType: BEFORE_ENTRY|ON_SUCCESS — all required) with ConditionNotOverridableException (client, HTTP 400, member `message`), documented "Unable to override because the condition does not allow overrides." A condition that cannot be overridden — one configured with a Skip result, one already resolved/Overridden, or a conditionType the stage does not declare — is refused ConditionNotOverridableException with the message `The condition type <conditionType> in stage <stageName> cannot be overridden now. Either it contains no failed or in-progress rule, the execution '<pipelineName>' is already superseded, or the pipeline definition has changed since the rules(s) failed.` Fixed text: everything outside the three substitutions. Variable parts: the conditionType and stageName FROM THE REQUEST, and — recorded behavior, confirmed independently by two checks — the PIPELINE NAME in the single-quoted "the execution '…'" slot, not the execution id.
Emulator: code is right (ConditionNotOverridableException, HTTP 400) and all the fixed text matches, but the single-quoted slot is filled with the pipelineExecutionId from the request instead of the pipeline name.
Why: the message is part of the refusal contract. A client or operator log parser that extracts the pipeline name from AWS's wording reads a UUID instead, and a test or alarm matching the documented text does not match. Because the id is also per-run, the response is not reproducible for anything keying off the message.
Fix: the message construction for the not-overridable refusal in the OverrideStageCondition handler of the codepipeline crate — one fixed template filled with the request's conditionType, stageName and pipelineName.
Traces: codepipeline.condition-not-overridable-rejected, codepipeline.override-stage-condition-conflict-rejected.

=== Cause 2 of 4: repeating PutJobSuccessResult on a job already Succeeded is refused InvalidJobStateException instead of being an idempotent HTTP 200 ===
AWS: Smithy models PutJobSuccessResult(jobId required, @pattern ^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$; currentRevision, continuationToken, executionDetails, outputVariables) with an empty output and errors InvalidJobStateException, JobNotFoundException, OutputVariablesSizeExceededException, ValidationException (all client, HTTP 400). Recorded behavior: re-sending the IDENTICAL success result for a job that already reached Succeeded is accepted — HTTP 200 with an empty body — and has no further effect: the pipeline still shows exactly one execution and exactly one Build action execution, both Succeeded. InvalidJobStateException is reserved for a state TRANSITION that is not allowed, e.g. a success result for a job that already terminated Failed, where AWS's message is `Job with id <jobId> has already terminated` (fixed text + the jobId from the request, unquoted, no trailing period).
Emulator: the repeat is refused with InvalidJobStateException and the emulator-authored message `The status of job with id <jobId> is already set to the desired value. It cannot be updated with the same status, but different result or output variables.` — raised even though the repeat carries the same (empty) result and no output variables. The exception propagates to the caller, so the at-least-once job-worker pattern cannot complete.
Why: every CodePipeline custom-action job worker is an at-least-once reporter: the documented way to recover from a lost response is to re-send the same PutJobSuccessResult. Against the emulator that benign retry becomes a 400 the worker must special-case, and the downstream negative class (a genuine Failed -> Succeeded flip, which MUST be InvalidJobStateException with AWS's "has already terminated" wording) is indistinguishable from it because both carry the same invented message.
Fix: the job-state classification in the PutJobSuccessResult handler of the codepipeline crate (and the job state store it reads): an identical success result for a job already in Succeeded is a no-op 200; only a conflicting transition from a terminal state is refused, with AWS's `Job with id <jobId> has already terminated` message.
Traces: codepipeline.put-job-success-result-completed-job-rejected, codepipeline.put-job-success-result-wrong-job-state-rejected.

=== Cause 3 of 4: the second rule of a beforeEntry condition is recorded Abandoned instead of Succeeded — the rules of one condition are not evaluated together ===
AWS: Smithy models ListRuleExecutions -> ruleExecutionDetails, with RuleExecution$status over RuleExecutionStatus = InProgress|Abandoned|Succeeded|Failed. The rules of a single condition are evaluated TOGETHER: a stage-entry condition declaring N rules yields exactly N rule-execution records for that pipeline execution, each carrying the outcome of its own predicate, and Abandoned is for a rule whose run was given up (e.g. the execution was superseded/stopped), not for a sibling that was not reached. With SrcVars.RepoName = 'MyDemoRepo', both declared VariableCheck rules hold (EQ 'MyDemoRepo' and CONTAINS 'Demo'), so both records must be Succeeded.
Emulator: GetPipelineState reports the condition terminal and ListRuleExecutions returns both records, but only the first rule is Succeeded and the second is Abandoned: {'RuleAlpha': 'Succeeded', 'RuleBeta': 'Abandoned'}. Hypothesis (neither the model nor the recording states the mechanism): the evaluator short-circuits after the first rule resolves the condition and marks the remaining siblings Abandoned instead of running them.
Why: a client reading the rule-execution history cannot tell which predicates actually held, so a multi-rule gate is unauditable; a rule that would have FAILED is also hidden behind Abandoned, which inverts the gate's meaning for any consumer that treats Abandoned as "not evaluated, not blocking".
Fix: the condition/rule evaluator in the codepipeline crate's execution plane — evaluate every rule of a condition and persist each rule execution's own outcome — and the records ListRuleExecutions/GetPipelineState publish from it.
Traces: codepipeline.list-rule-executions-matches-condition-evaluation-order.

=== Cause 4 of 4: executions of three further pipeline shapes never reach the state the check polls for, so each exhausts the 60 s per-check ceiling ===
AWS: recorded behavior only (the model types the calls but not the timing). The service DRIVES an execution: a beforeEntry condition with result SKIP whose rule does not hold settles the stage to Skipped and GetPipelineState reports it; a Lambda-invoke action is dispatched, the function's PutJobSuccessResult(outputVariables=…) completes it, and ListActionExecutions reports the action terminal with its outputVariables byte-identical (no numeric coercion, unicode preserved); a custom-action Build job completed with PutJobSuccessResult drives its execution to a terminal Succeeded that GetPipelineExecution reports. All of these settle in seconds against the recording.
Emulator: each of the three checks below stops at the suite's ceiling — 'Error: Check timed out after 60.0s' — while polling for that next state (the GateSkip stage reaching 'Skipped'; the InvokeStage action execution reaching Succeeded/Failed; the custom-Build execution reaching a terminal status before the stop-rejection can be exercised). No state is observed, so the negative classes behind them (ConditionNotOverridableException on a Skip condition, PipelineExecutionNotStoppableException on a finished execution) are unreachable.
Why: the client-visible effect is a pipeline that hangs: starting an execution and waiting for the documented next state never returns, and every assertion that depends on a settled execution — output-variable fidelity, Skipped stages, stop refusals — is unobservable.
Fix: the codepipeline crate's execution plane (the driver behind StartPipelineExecution and the state it publishes to GetPipelineState/GetPipelineExecution/ListActionExecutions), specifically the SKIP-result condition resolution, the Lambda-invoke action dispatch/completion path, and the terminal-status settle for custom-action jobs.
Traces: codepipeline.override-stage-condition-not-overridable-rejected, codepipeline.job-output-variables-precision-preserved, codepipeline.stop-completed-execution-rejected.

## Evidence
All strings below are verbatim from this replay's failing-check details. Pipeline names, execution ids and job ids are generated per run: they FILL the message templates and are not themselves requirements.

Cause 1 — codepipeline.condition-not-overridable-rejected [mismatch], at check_codepipeline_condition_not_overridable_rejected.py line 94:
  assert message == (
      "The condition type BEFORE_ENTRY in stage GateSkip cannot be overridden now. "
      "Either it contains no failed or in-progress rule, "
      f"the execution '{pipeline_name}' is already superseded, "
      "or the pipeline definition has changed since the rules(s) failed."
  )
  AssertionError: unexpected message: "The condition type BEFORE_ENTRY in stage GateSkip cannot be overridden now. Either it contains no failed or in-progress rule, the execution '1c0b907d-166d-4c1d-a86b-f77da7f285e2' is already superseded, or the pipeline definition has changed since the rules(s) failed."
  The preceding `assert code == "ConditionNotOverridableException"` PASSED, so only the quoted slot diverges (a pipelineExecutionId where the pipeline name belongs).
  codepipeline.override-stage-condition-conflict-rejected [mismatch], at check_codepipeline_override_stage_condition_conflict_rejected.py line 60, same template (the check's own comment: "AWS names the pipeline, not the execution id, in the 'execution ...' slot"):
  AssertionError: ('ConditionNotOverridableException', "The condition type BEFORE_ENTRY in stage Approval cannot be overridden now. Either it contains no failed or in-progress rule, the execution '1ac5b887-1391-47f9-b59a-7e3eee042621' is already superseded, or the pipeline definition has changed since the rules(s) failed.")
  Here the stage name (Approval vs GateSkip) and the conditionType (BEFORE_ENTRY, and ON_SUCCESS in a sibling assertion) do vary correctly with the request — only the pipeline-name substitution is wrong.

Cause 2 — codepipeline.put-job-success-result-completed-job-rejected [error]:
  botocore.errorfactory.InvalidJobStateException: An error occurred (InvalidJobStateException) when calling the PutJobSuccessResult operation: The status of job with id 1ee74723-ca2e-4b4d-a093-9ce04b80117f is already set to the desired value. It cannot be updated with the same status, but different result or output variables.
  Raised at check line 93: `repeat = cp.put_job_success_result(jobId=job["id"])` followed by `assert repeat["ResponseMetadata"]["HTTPStatusCode"] == 200`. The positive control immediately before it PASSED: the first put_job_success_result succeeded, the execution reached 'Succeeded', and exactly one Build action execution was recorded — so the job and pipeline state are correct and only the repeat is refused.
  codepipeline.put-job-success-result-wrong-job-state-rejected [error]: identical uncaught exception, message 'The status of job with id 40a95762-9750-4002-bb0f-c5b27c4520a8 is already set to the desired value. It cannot be updated with the same status, but different result or output variables.', raised at the check's documented idempotent repeat (`cp.put_job_success_result(jobId=job2)  # idempotent repeat: does not raise`) after that job's action had already settled 'Succeeded'.
  Expected for the repeat: HTTP 200, empty body, no new execution and no new action execution. Expected for the genuine bad transition the same check exercises earlier: InvalidJobStateException with message exactly 'Job with id <jobId> has already terminated'.

Cause 3 — codepipeline.list-rule-executions-matches-condition-evaluation-order [mismatch], at check_codepipeline_list_rule_executions_matches_condition_evaluation_order.py line 111:
  assert statuses == {name: "Succeeded" for name in declared_rule_names}, statuses
  AssertionError: {'RuleBeta': 'Abandoned', 'RuleAlpha': 'Succeeded'}
  The stage declares one beforeEntry condition (result SKIP) with two AWS/VariableCheck rules: RuleAlpha (Variable '#{SrcVars.RepoName}', Value 'MyDemoRepo', Operator EQ) and RuleBeta (same variable, Value 'Demo', Operator CONTAINS). The Source job was completed with output variable RepoName='MyDemoRepo', so both predicates hold. The preceding assertions PASSED: both rule names are present and exactly two records are returned — only RuleBeta's status diverges (Abandoned instead of Succeeded).

Cause 4 — three checks report, identically, 'Error: Check timed out after 60.0s' with no state observed:
  codepipeline.override-stage-condition-not-overridable-rejected — polling `_stage_state(cp, name, 'GateSkip')` for latestExecution.status == 'Skipped' on a V2 pipeline whose GateSkip stage has beforeEntry {result: SKIP, rules: [VariableCheck '#{SourceVars.ETag}' EQ 'definitely-mismatched-value-xyz']}; the stage never reaches 'Skipped'.
  codepipeline.job-output-variables-precision-preserved — polling `cp.get_pipeline_state(name=…)` for the InvokeStage (Lambda invoke action) to publish a latestExecution, then ListActionExecutions for that action to be Succeeded/Failed with outputVariables {'BigInteger': '123456789012345678901234567890123456789', 'PreciseDecimal': '3.14159265358979323846264338327950288', 'UnicodeText': 'héllo-世界-café'}; never reached.
  codepipeline.stop-completed-execution-rejected — polling `cp.get_pipeline_execution(...)['pipelineExecution']['status']` for a terminal status after driving the custom Build job to success, before the PipelineExecutionNotStoppableException assertion; never reached. (An earlier replay recorded this check reaching that assertion and failing on the message, which 20261031T033058Z-… owns; in this run it does not get that far.)

Run context: codepipeline per-service pass rate in this replay 109/160 (68%), 51 failed; every other codepipeline failure in this run is marked OWNED by an existing open issue.

## Steps to Reproduce
Boot the emulator and point botocore at it: cp = boto3.client('codepipeline'), s3 = boto3.client('s3'), iam = boto3.client('iam'), lam = boto3.client('lambda').

Cause 1 (codepipeline.condition-not-overridable-rejected, codepipeline.override-stage-condition-conflict-rejected):
1. cp.create_custom_action_type for a Source category (0 input / 1 output artifact) and a Build category (1 input / 0 output). Create an artifact bucket and a pipeline role trusted by codepipeline.amazonaws.com.
2. cp.create_pipeline (pipelineType='V2') with a Source stage and a second stage 'GateSkip' carrying beforeEntry = {conditions: [{result: 'SKIP', rules: [{name: 'SkipRule', ruleTypeId: {category: 'Rule', owner: 'AWS', provider: 'VariableCheck', version: '1'}, configuration: {Variable: '#{Vars.Flag}', Value: 'yes', Operator: 'EQ'}}]}]}.
3. exec_id = cp.start_pipeline_execution(name=P)['pipelineExecutionId']; claim the Source job via PollForJobs/AcknowledgeJob and complete it with PutJobSuccessResult(outputVariables={'Flag': 'no'}) so the rule does not hold and the stage resolves to Skipped.
4. cp.override_stage_condition(pipelineName=P, stageName='GateSkip', pipelineExecutionId=exec_id, conditionType='BEFORE_ENTRY').
   EXPECT: ConditionNotOverridableException (already correct) with message exactly "The condition type BEFORE_ENTRY in stage GateSkip cannot be overridden now. Either it contains no failed or in-progress rule, the execution '<PIPELINE NAME>' is already superseded, or the pipeline definition has changed since the rules(s) failed."
   OBSERVE: the same sentence with the pipelineExecutionId in the quoted slot.
5. Repeat with conditionType='ON_SUCCESS' (a condition type the stage does not declare) and on an already-Overridden condition of a finished execution: same template, same substitution rule.

Cause 2 (codepipeline.put-job-success-result-completed-job-rejected, codepipeline.put-job-success-result-wrong-job-state-rejected):
6. Create an artifact bucket with source/source.zip, a pipeline role, a custom Build action type, and a pipeline with an S3 Source stage plus that Build stage.
7. Start an execution; cp.poll_for_jobs for the Build action type, cp.acknowledge_job(jobId=J, nonce=N), then cp.put_job_success_result(jobId=J) -> succeeds and the execution reaches 'Succeeded' with exactly one Build action execution.
8. cp.put_job_success_result(jobId=J) again, with the identical (empty) result.
   EXPECT: HTTP 200, empty body; cp.list_pipeline_executions still shows one execution and cp.list_action_executions one Build action execution, both Succeeded.
   OBSERVE: InvalidJobStateException 'The status of job with id <J> is already set to the desired value. It cannot be updated with the same status, but different result or output variables.'
9. Negative class that must still refuse: drive another job to terminal Failed with cp.put_job_failure_result, then cp.put_job_success_result on it -> InvalidJobStateException with message exactly 'Job with id <jobId> has already terminated'.

Cause 3 (codepipeline.list-rule-executions-matches-condition-evaluation-order):
10. Create a V2 pipeline with a custom Source action (namespace 'SrcVars') and a Build stage whose beforeEntry has ONE condition (result 'SKIP') declaring TWO AWS/VariableCheck rules: RuleAlpha {Variable '#{SrcVars.RepoName}', Value 'MyDemoRepo', Operator 'EQ'} and RuleBeta {Variable '#{SrcVars.RepoName}', Value 'Demo', Operator 'CONTAINS'}.
11. Let CreatePipeline's execution start, claim the Source job and complete it with PutJobSuccessResult(outputVariables={'RepoName': 'MyDemoRepo'}).
12. Poll cp.get_pipeline_state until the Build stage's condition is terminal, then cp.list_rule_executions(pipelineName=P, filter={'pipelineExecutionId': exec_id}).
   EXPECT two records, {'RuleAlpha': 'Succeeded', 'RuleBeta': 'Succeeded'}.
   OBSERVE {'RuleAlpha': 'Succeeded', 'RuleBeta': 'Abandoned'}.

Cause 4 (codepipeline.override-stage-condition-not-overridable-rejected, codepipeline.job-output-variables-precision-preserved, codepipeline.stop-completed-execution-rejected):
13. S3-source + 'GateSkip' stage with beforeEntry {result: 'SKIP', rules: [VariableCheck '#{SourceVars.ETag}' EQ 'definitely-mismatched-value-xyz']} and a manual-approval action; cp.start_pipeline_execution and poll cp.get_pipeline_state -> EXPECT GateSkip latestExecution.status 'Skipped' within seconds; OBSERVE no such state until the 60 s ceiling.
14. S3-source + 'InvokeStage' with an AWS/Lambda invoke action whose UserParameters carry output variables; the function calls cp.put_job_success_result(jobId=…, outputVariables=…). Poll cp.get_pipeline_state for InvokeStage's latestExecution and cp.list_action_executions for the action to settle -> EXPECT Succeeded with the output variables echoed byte-identical; OBSERVE nothing settles.
15. S3-source + custom Build stage; drive the Build job to success and poll cp.get_pipeline_execution for a terminal status -> EXPECT 'Succeeded' within seconds, so StopPipelineExecution(abandon=True) on it can be refused PipelineExecutionNotStoppableException; OBSERVE no terminal status.

Then re-replay the eight check ids listed in trace_ids.
