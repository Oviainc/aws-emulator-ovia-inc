Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: fis.e2e-experiment-create-start-observe-stop-teardown
Issue-Shape-Id: com.amazonaws.fis#FaultInjectionSimulator
Issue-Key: fis.e2e-experiment-create-start-observe-stop-teardown|run#25|AssertionError|assert message == _TEMPLATE_NOT_FOUND, f"expected message {_TEMPLATE_NOT_FOUND!r}, got {message!r}"
Issue-Commit: 659e25b97046a77449047daf7fe310a5a83846e1

# FIS: GetExperimentTemplate not-found message uses the emulator's own identifier-echoing prose instead of FIS's fixed 'ExperimentTemplate not found.' rule, breaking the end-to-end create/start/stop/teardown flow

> _Note: this issue was identified against the code base at commit `659e25b97046a77449047daf7fe310a5a83846e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetExperimentTemplate

## Description
One root cause, grounded in a failing behavioral check of the replay suite that no open issue carries as a replay key.

Cause 1 of 1: the ResourceNotFoundException raised by GetExperimentTemplate for a deleted (or otherwise unknown) experiment template does not follow FIS's not-found message rule.

AWS: GetExperimentTemplate on an id that does not resolve is refused with ResourceNotFoundException (Smithy: services/service-com-amazonaws-fis-faultinjectionsimulator/smithy-model/com-amazonaws-fis-faultinjectionsimulator.json — GetExperimentTemplate errors = ResourceNotFoundException errorKind client httpError 404, ValidationException httpError 400; ResourceNotFoundException.message -> ExceptionMessage @length 0..1024, doc "The specified resource cannot be found."). The model does not state the text, but this corpus's recordings establish the rule for the experiment-template resolver: the message is the FIXED, identifier-free string "ExperimentTemplate not found." — three independent recorded checks assert exactly that string for an unresolvable experimentTemplateId (this check plus fis.start-experiment-missing-template-not-found and fis.start-experiment-nonexistent-template-not-found). The requirements are therefore the 404 / ResourceNotFoundException classification AND the rule that the message is the fixed form which does NOT echo the requested identifier.

Emulator: the status and the error code are correct, but the message is the emulator's own prose built by echoing the requested id — e.g. 'No experiment template found with id EXT000000000001' where AWS answers 'ExperimentTemplate not found.'. In this check the whole lifecycle up to that point behaves (template created, experiment started, reached running, resolved targets reported, stopped, template deleted); the ONLY divergence that fails the check is this message rendering on the post-delete read-back.

Why: a client that keys on FIS's not-found message rule (log/alarm matching, error classification, or a golden-file comparison of the error body) mis-classifies the failure, and the emulator additionally discloses the caller's requested identifier in a message AWS deliberately keeps identifier-free. Because this is the last assertion of the end-to-end flow, the entire happy-path e2e check reports as a failure even though the lifecycle itself is correct — masking the health of that flow.

Fix: the not-found error construction in the shared experiment-template resolver used by GetExperimentTemplate (the ops handler for GetExperimentTemplate under services/service-com-amazonaws-fis-faultinjectionsimulator/src/ops/, i.e. the same resolver that builds the template not-found error for StartExperiment).

Relation to already-open issues (named here so a fixer does not treat this as untouched ground): open issue 20261022T043627Z-fis-control-plane-9-fidelity-divergences-no-wait.md describes the same emulator string and the same expected string in its Cause 8 (not-found message rules for experiments and experiment templates), but its replay keys cover only fis.get-experiment-unknown-id-not-found, fis.start-experiment-missing-template-not-found and fis.start-experiment-nonexistent-template-not-found — it does NOT carry the e2e check below, so fixing Cause 8 there must also be verified against this check's read path (GetExperimentTemplate, not StartExperiment). If that issue's Cause 8 is fixed in the shared resolver, this check should pass with it.

Traces: fis.e2e-experiment-create-start-observe-stop-teardown

## Evidence
Check fis.e2e-experiment-create-start-observe-stop-teardown, [mismatch]:

  File "check_fis_e2e_experiment_create_start_observe_stop_teardown.py", line 86, in run
    assert message == _TEMPLATE_NOT_FOUND, f"expected message {_TEMPLATE_NOT_FOUND!r}, got {message!r}"
  AssertionError: expected message 'ExperimentTemplate not found.', got 'No experiment template found with id EXT000000000001'

Expected (AWS): HTTP 404, Error.Code = 'ResourceNotFoundException', Error.Message = the fixed identifier-free form 'ExperimentTemplate not found.'
Actual (emulator): HTTP 404, Error.Code = 'ResourceNotFoundException' (both correct), Error.Message = 'No experiment template found with id <requested id>' — the recorded instance was 'No experiment template found with id EXT000000000001'; the generated id itself is environment-specific and is not the requirement, the identifier-echoing form is.

Everything the check asserts before that point passed against the emulator: create_experiment_template with an aws:iam:role target and an aws:fis:inject-api-internal-error action, start_experiment, GetExperiment reaching 'running', list_experiment_resolved_targets returning exactly one aws:iam:role target whose targetInformation contains the target role ARN, stop_experiment reaching 'stopped', and delete_experiment_template followed by a 404 ResourceNotFoundException on the read-back.

## Steps to Reproduce
Boot the emulator and point a botocore 'fis' client (plus an 'iam' client) at it in region us-west-2, then:
1. Create an IAM role trusting fis.amazonaws.com (execution role) and any second role to use as a target resource.
2. create_experiment_template(clientToken=<tok>, description=<desc>, stopConditions=[{'source':'none'}], targets={'roleTarget':{'resourceType':'aws:iam:role','resourceArns':[<target role arn>],'selectionMode':'ALL'}}, actions={'injectAction':{'actionId':'aws:fis:inject-api-internal-error','parameters':{'duration':'PT1M','service':'ec2','percentage':'10','operations':'DescribeRegions'},'targets':{<action target key>:'roleTarget'}}}, roleArn=<exec role arn>).
3. delete_experiment_template(id=<template id>) — any deleted or never-created well-formed template id works; no experiment is needed to reproduce the message divergence.
4. get_experiment_template(id=<that template id>) and inspect the ClientError: the code is ResourceNotFoundException (correct) but response['Error']['Message'] is 'No experiment template found with id <id>' instead of the fixed 'ExperimentTemplate not found.'
Equivalently, re-replay the behavioral check listed in trace_ids.
