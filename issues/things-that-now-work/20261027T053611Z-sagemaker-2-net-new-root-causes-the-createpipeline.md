Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sagemaker.create-pipeline-cyclic-dependson-rejected sagemaker.training-job-empty-vs-absent-hyperparameter-preserved
Issue-Shape-Id: com.amazonaws.sagemaker#SageMaker
Issue-Key: sagemaker.create-pipeline-cyclic-dependson-rejected|_reject#12|AssertionError|assert message == expected_message, (
Issue-Key: sagemaker.training-job-empty-vs-absent-hyperparameter-preserved|run#12|AssertionError|assert "empty_hp" not in hp, (
Issue-Commit: 934cd745d9c704f390241637edb1d9ca74153131

# sagemaker: 2 net-new root causes — the CreatePipeline DAG-cycle refusal carries invented wording, and DescribeTrainingJob reports an empty-string hyperparameter AWS drops

> _Note: this issue was identified against the code base at commit `934cd745d9c704f390241637edb1d9ca74153131`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePipeline

## Description
Two independent, net-new SageMaker fidelity defects, both grounded in failing behavioral checks. Neither is covered by an open issue: the closest open items (20261024T160943Z-sagemaker-runs-no-job-work-processing-pipeline-step.md, 20261025T101032Z-sagemaker-training-jobs-never-execute-every-accepted-createtrainingjob.md, 20261022T210606Z-sagemaker-associatetrialcomponent-deletenotebookinstance-createpipeline-from-s3-answer-notimplemented.md) are about pipeline/training EXECUTION and about CreatePipeline's S3-location input path, not about the two representation/validation-wording defects below. Both causes must be closed before submitting.

=== Cause 1 of 2: CreatePipeline's cyclic-DependsOn refusal uses emulator-authored wording instead of SageMaker's diagnosis ===
Failing check: sagemaker.create-pipeline-cyclic-dependson-rejected

AWS: CreatePipeline is a definition compiler — SageMaker resolves the step DAG synchronously and stores nothing when the definition is defective. Smithy (services/service-com-amazonaws-sagemaker-sagemaker/smithy-model/com-amazonaws-sagemaker-sagemaker.json): CreatePipeline input PipelineDefinition (len 1..1048576), modeled errors ConflictException / ResourceLimitExceeded / ResourceNotFound only — the DAG-validation refusal is the service-common ValidationException (HTTP 400), so the message text is not in the model. Recorded behavior (the check's measured values, verified against real SageMaker us-west-2): a definition whose two Lambda steps depend on each other is refused 400 ValidationException with the message EXACTLY `Detected cycle in the DAG.` — that string is entirely FIXED text, with no variable part: AWS does not name the steps or the cycle path. The same check records the sibling diagnoses AWS emits for the other definition defects, each with its own fixed text plus request-derived variable parts (the step names as sent): self-dependency -> `Step: <stepName> can not depend on itself.`; DependsOn a step that is not declared -> `Step: <stepName> depends on unknown Step: <missingStepName>`; DependsOn a terminal Fail step -> `Step: <stepName> can not depends on FailStep: <targetStepName>` (Condition steps the same, with `ConditionStep:`); a reference to an undeclared pipeline parameter -> `Unknown property reference [Parameters.<parameterName>].` Each defect class must carry its own reason so a caller can tell one definition defect from another.

Emulator: the code is right (ValidationException/400) but the message is invented: `PipelineDefinition declares a circular dependency between its steps: StepA -> StepB -> StepA` — different fixed text, and it adds a step-path enumeration AWS never emits. Because the cycle case is asserted first, the check never reached the self-dependency, unknown-step, FailStep-target and unknown-property-reference assertions, so those messages are unverified here and may diverge the same way; all five are produced by the same validator and should be fixed together.

Why: a client (and the AWS CLI/SDK error-handling code and CI gates built on it) that matches on the refusal reason to distinguish a cycle from a dangling DependsOn from a bad parameter reference sees an unrecognized string, so it cannot classify the definition defect and reports a generic failure — the whole point of synchronous definition compilation is lost. Tooling that asserts on AWS's documented/observed text fails against the emulator.

Fix: belongs in CreatePipeline's definition validation, not the handler's control flow. services/service-com-amazonaws-sagemaker-sagemaker/src/ops/create_pipeline.rs:199-201 delegates to `crate::state::pipeline_definition_violation(&definition)` and lifts whatever string it returns into `Scaffolded::validation`; the diverging wording is authored in that validator in crate::state. Each defect class should return AWS's own fixed text with the step/parameter names taken from the request.

=== Cause 2 of 2: DescribeTrainingJob preserves an explicit empty-string hyperparameter that AWS drops ===
Failing check: sagemaker.training-job-empty-vs-absent-hyperparameter-preserved

AWS: Smithy: HyperParameters is a map (len 0..100) of HyperParameterKey (len 0..256) to HyperParameterValue (@length min 0, max 2500), so CreateTrainingJob legitimately ACCEPTS an empty-string value — there is no validation error here. Recorded behavior only (the model says nothing about echo semantics, and the check notes real AWS refuted the opposite expectation on first replay): DescribeTrainingJob omits a hyperparameter whose supplied value was the empty string entirely from the HyperParameters map, making it indistinguishable from a key that was never sent. A key supplied with a non-empty value still round-trips unchanged, and a key never supplied is still absent.

Emulator: DescribeTrainingJob returns `{'present_hp': '42', 'empty_hp': ''}` — it keeps `empty_hp` as a distinct key with an empty value, i.e. it echoes the request more faithfully than AWS itself does. Status and shape are otherwise correct; only the map's key set diverges.

Why: a client that reads back HyperParameters to decide whether a knob was configured (a common pattern in training-pipeline orchestration, SDK estimators and drift/diff checks between a submitted and a described job) sees a key present on the emulator and absent on AWS. Code keyed on `in`/`has_key` takes the configured branch on the emulator and the default branch on AWS, and any describe-vs-submit diff or hash of the job description differs between the two.

Fix: belongs in CreateTrainingJob's state write / DescribeTrainingJob's serialization of the training-job record — empty-valued hyperparameter entries must be dropped so the described map cannot distinguish them from never-supplied keys (non-empty values unchanged). The handlers live under services/service-com-amazonaws-sagemaker-sagemaker/src/ops/ (create_training_job.rs, describe_training_job.rs) with the record in crate::state.

Traces: sagemaker.create-pipeline-cyclic-dependson-rejected (Cause 1), sagemaker.training-job-empty-vs-absent-hyperparameter-preserved (Cause 2).

## Evidence
Cause 1 — sagemaker.create-pipeline-cyclic-dependson-rejected [mismatch]
  check_sagemaker_create_pipeline_cyclic_dependson_rejected.py, line 258 -> _reject("two-node Lambda DependsOn cycle", ...), line 239:
  AssertionError: two-node Lambda DependsOn cycle: the refusal must name its own reason, so that a caller can tell one definition defect from another; expected 'Detected cycle in the DAG.', got 'PipelineDefinition declares a circular dependency between its steps: StepA -> StepB -> StepA'
  Expected: HTTP 400, code ValidationException, message 'Detected cycle in the DAG.' (all fixed text, no variable part).
  Actual:   HTTP 400, code ValidationException (code matched — the code assertion at line 237 passed), message 'PipelineDefinition declares a circular dependency between its steps: <step path>' — fixed text differs and a step-path enumeration is added. 'StepA'/'StepB' in the observed message are this run's step names, not part of any required template.
  Not reached, same validator, likely to diverge identically: 'Step: <stepName> can not depend on itself.', 'Step: <stepName> depends on unknown Step: <missingStepName>', 'Step: <stepName> can not depends on FailStep: <targetStepName>', 'Unknown property reference [Parameters.<parameterName>].' The control (valid acyclic two-Lambda-step definition) was accepted and its definition round-tripped, so only the refusal wording diverges.

Cause 2 — sagemaker.training-job-empty-vs-absent-hyperparameter-preserved [mismatch]
  check_sagemaker_training_job_empty_vs_absent_hyperparameter_preserved.py, line 76:
  AssertionError: an explicit empty-string hyperparameter was preserved as a distinct key -- real AWS drops it, treating it exactly like a key that was never supplied: {'present_hp': '42', 'empty_hp': ''}
  Request: CreateTrainingJob HyperParameters={'empty_hp': '', 'present_hp': '42'} (accepted, 200, as AWS does).
  Expected DescribeTrainingJob HyperParameters: 'empty_hp' ABSENT, 'present_hp' == '42', any never-supplied key absent.
  Actual DescribeTrainingJob HyperParameters: {'present_hp': '42', 'empty_hp': ''} — 'empty_hp' present with ''. The key names are the check's; the requirement is that an empty-valued entry is indistinguishable from an absent one.

## Steps to Reproduce
Cause 1:
1. CreateRole a sagemaker.amazonaws.com-assumable execution role.
2. CreatePipeline with a valid acyclic definition of two Lambda steps (StepB DependsOn StepA) — succeeds, and DescribePipeline round-trips the definition (control passes).
3. CreatePipeline (new PipelineName, new ClientRequestToken, same RoleArn) with a definition of two Lambda steps forming a cycle: StepA DependsOn [StepB] and StepB DependsOn [StepA].
4. Observe the ClientError: code is ValidationException (correct) but Message is 'PipelineDefinition declares a circular dependency between its steps: StepA -> StepB -> StepA' instead of AWS's 'Detected cycle in the DAG.'
5. (Same call path, not yet reached by the check) repeat with a self-dependency, a DependsOn naming an undeclared step, a DependsOn targeting a Fail step, and a reference to an undeclared Parameters.<name>, and compare each message against the AWS texts listed in the description.

Cause 2:
1. CreateRole a sagemaker.amazonaws.com-assumable execution role.
2. CreateTrainingJob with HyperParameters={'empty_hp': '', 'present_hp': '42'}, AlgorithmSpecification.TrainingImage = the region's built-in linear-learner image, TrainingInputMode 'File', ResourceConfig ml.m5.large/1/1GiB, StoppingCondition MaxRuntimeInSeconds 120, an OutputDataConfig S3OutputPath (retrying only role-propagation refusals).
3. DescribeTrainingJob on that job name and read HyperParameters.
4. Observe 'empty_hp' present with value '' — AWS omits that key entirely, exactly as it omits a key never supplied.
