Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sagemaker.associate-trial-component-nonexistent-trial-rejected
Issue-Shape-Id: com.amazonaws.sagemaker#SageMaker
Issue-Key: sagemaker.associate-trial-component-nonexistent-trial-rejected|run#10|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Commit: f9dabc8a716cd35aa702facadefbd80952bd273f

# SageMaker trial-not-found ResourceNotFound message omits the single quotes AWS puts around the trial ARN

> _Note: this issue was identified against the code base at commit `f9dabc8a716cd35aa702facadefbd80952bd273f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateTrialComponent

## Description
One root cause, affecting every reader that reports a missing experiment trial.

Cause 1 of 1: the `ResourceNotFound` message template for a trial that does not exist is rendered without the single quotes AWS wraps the trial ARN in. Failing check id: sagemaker.associate-trial-component-nonexistent-trial-rejected.

AWS: Smithy (services/service-com-amazonaws-sagemaker-sagemaker/smithy-model/com-amazonaws-sagemaker-sagemaker.json) models `AssociateTrialComponent` with errors `ResourceNotFound` (client, HTTP 400) and `ResourceLimitExceeded`, and `DescribeTrial` with `ResourceNotFound` (client, HTTP 400); the model documents the error only as "Resource being access is not found" and does not state the message text, so the message itself is recorded behavior only. The recorded AWS response for naming a trial the account does not hold is HTTP 400, error code `ResourceNotFound`, message `Trial '<trial-arn>' does not exist.` — the fixed text is `Trial '` + `' does not exist.` (note the single-quote delimiters), and the one variable part is the trial's ARN, derived from the request's `TrialName` plus the partition/region/account of the caller and the `experiment-trial` resource kind the modeled `TrialArn` pattern fixes (`^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:experiment-trial/`). The ARN value in any one recording (e.g. an account id of all zeros, a `sim2real-…` name) is a fill-in, not part of the requirement.

Emulator: returns the right status and the right code (`ResourceNotFound`, 400) but renders the sentence with the ARN unquoted — `Trial arn:aws:sagemaker:<region>:<account>:experiment-trial/<name> does not exist.` The same unquoted sentence is produced by both readers of a missing trial, so the divergence is not specific to one operation.

Why: a client that matches, parses or asserts on the refusal text — the common pattern for distinguishing "trial missing" from other `ResourceNotFound` causes, since the code is shared by trials, trial components, experiments and more — fails against the emulator, and any test or tooling pinned to AWS's wording sees a message mismatch on an otherwise correct refusal. The quotes are also what delimit the ARN when the name contains no separator, so naive extraction of the resource identifier from the message breaks.

Fix: the two message templates in the trial-not-found refusal paths of the `AssociateTrialComponent` and `DescribeTrial` handlers (the refusal text each builds for a trial absent from `crate::state`), which should share one wording helper so the quoted form is stated once. Note for the fixer: this is NOT the already-closed stub work — `20261025T101032Z-sagemaker-associatetrialcomponent-stub.md` is fixed and the operation now implements the lookup and refuses with the correct code; the residual defect is only the message wording, which the earlier message-template passes (`20261022T210606Z-sagemaker-rejection-fidelity-duplicate-model-packages-get-conflictexception.md`, `20261024T185157Z-sagemaker-rejection-fidelity-cluster-node-algorithm-model-package.md`, both fixed) did not cover for the Trial template. The open issue `20261022T210606Z-sagemaker-associatetrialcomponent-deletenotebookinstance-createpipeline-from-s3-answer-notimplemented.md` names this operation but is about it answering NotImplemented, which it no longer does, so it does not cover this.

Traces: sagemaker.associate-trial-component-nonexistent-trial-rejected.

## Evidence
Check sagemaker.associate-trial-component-nonexistent-trial-rejected [mismatch]:

  AssertionError: expected message "Trial 'arn:aws:sagemaker:us-west-2:000000000000:experiment-trial/sim2real-sm-notrial-0314fccec514' does not exist.", got 'Trial arn:aws:sagemaker:us-west-2:000000000000:experiment-trial/sim2real-sm-notrial-0314fccec514 does not exist.'

The check's preceding assertion (`code == "ResourceNotFound"`) PASSED, so status and error code match AWS; only the message diverges.

Expected (AWS): HTTP 400, code ResourceNotFound, message `Trial '<trial-arn>' does not exist.`
Actual (emulator): HTTP 400, code ResourceNotFound, message `Trial <trial-arn> does not exist.`
Fixed text: `Trial '` … `' does not exist.`  Variable part: the trial ARN, built from the request's TrialName and the caller's partition/region/account with resource kind `experiment-trial`.

Source of the divergence in the checkout:
- services/service-com-amazonaws-sagemaker-sagemaker/src/ops/associate_trial_component.rs:168 — `message: Some(format!("Trial {trial_arn} does not exist."))`
- services/service-com-amazonaws-sagemaker-sagemaker/src/ops/describe_trial.rs:162 — `message: Some(format!("Trial {arn} does not exist."))`

The same check asserts the identical quoted sentence again for DescribeTrial (its line 36, `expected_missing`), so that second site diverges for the same reason and is reached once the first assertion is fixed.

## Steps to Reproduce
1. CreateTrialComponent with a fresh name, e.g. TrialComponentName="tc-1" (succeeds; note the returned TrialComponentArn's partition/region/account prefix).
2. Call AssociateTrialComponent(TrialComponentName="tc-1", TrialName="<a trial name that was never created>").
3. Observe the refusal: HTTP 400 / ResourceNotFound (correct), but the message renders the trial ARN WITHOUT the surrounding single quotes, where AWS returns `Trial '<arn>' does not exist.`
4. Call DescribeTrial(TrialName="<the same never-created name>") and observe the same unquoted sentence.
5. Re-replay sagemaker.associate-trial-component-nonexistent-trial-rejected to confirm; it asserts the quoted form at both sites and must then reach its positive control (associate to a real trial, then ListTrialComponents).
