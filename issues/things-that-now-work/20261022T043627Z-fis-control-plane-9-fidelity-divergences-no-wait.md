Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: fis.create-experiment-template-wait-duration-exceeds-max-rejected fis.start-experiment-already-running-conflict fis.start-experiment-insufficient-role-permissions-denied fis.start-experiment-service-quota-exceeded fis.experiment-lifecycle-pending-to-completed fis.start-experiment-transitions-to-running-then-completed fis.stop-completed-experiment-rejected fis.stop-experiment-already-stopped-rejected fis.get-experiment-malformed-id-validation-rejected fis.list-experiments-invalid-filter-rejected fis.list-experiments-malformed-filter-rejected fis.get-experiment-unknown-id-not-found fis.start-experiment-missing-template-not-found fis.start-experiment-nonexistent-template-not-found fis.update-target-account-configuration-nonexistent-not-found
Issue-Shape-Id: com.amazonaws.fis#FaultInjectionSimulator
Issue-Key: fis.create-experiment-template-wait-duration-exceeds-max-rejected|_error#3|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded and returned {result!r}")
Issue-Key: fis.start-experiment-already-running-conflict|_error#14|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded and returned {result!r}")
Issue-Key: fis.start-experiment-insufficient-role-permissions-denied|run#16|AssertionError|assert denied_code == "ValidationException", (
Issue-Key: fis.start-experiment-service-quota-exceeded|run#11|AssertionError|assert last_error_code == "ServiceQuotaExceededException", (
Issue-Key: fis.experiment-lifecycle-pending-to-completed|run#12|Timeout|time.sleep(3)
Issue-Key: fis.start-experiment-transitions-to-running-then-completed|_poll_until#11|Timeout|time.sleep(interval_s)
Issue-Key: fis.stop-completed-experiment-rejected|_poll_until#7|Timeout|time.sleep(interval_s)
Issue-Key: fis.stop-experiment-already-stopped-rejected|_error#12|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: fis.get-experiment-malformed-id-validation-rejected|run#5|AssertionError|assert message == "Maximum length exceeded: id", f"expected 'Maximum length exceeded: id', got {message!r}"
Issue-Key: fis.list-experiments-invalid-filter-rejected|run#6|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: fis.list-experiments-malformed-filter-rejected|run#14|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: fis.get-experiment-unknown-id-not-found|run#11|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: fis.start-experiment-missing-template-not-found|run#7|AssertionError|assert get_message == _TEMPLATE_NOT_FOUND, f"expected message {_TEMPLATE_NOT_FOUND!r}, got {get_message!r}"
Issue-Key: fis.start-experiment-nonexistent-template-not-found|run#5|AssertionError|assert message == "ExperimentTemplate not found.", f"expected 'ExperimentTemplate not found.', got {message!r}"
Issue-Key: fis.update-target-account-configuration-nonexistent-not-found|run#6|AssertionError|assert code == "ResourceNotFoundException", code
Issue-Commit: bffba5cba440022693dbdf16c8135b8d2c175737

# FIS control plane: 9 fidelity divergences (no wait-duration/role-trust/quota/conflict enforcement, experiments never reach 'completed', terminal-state stop accepted, wrong validation & not-found message rules, UpdateTargetAccountConfiguration unimplemented)

> _Note: this issue was identified against the code base at commit `bffba5cba440022693dbdf16c8135b8d2c175737`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExperiment

## Description
Nine distinct root causes in the emulated FIS (Fault Injection Service) control plane, all grounded in failing behavioral checks of the replay suite. Smithy model cited below is services/service-com-amazonaws-fis-faultinjectionsimulator/smithy-model/com-amazonaws-fis-faultinjectionsimulator.json. A fixer must close ALL NINE causes before submitting.

Cause 1 of 9: CreateExperimentTemplate accepts an aws:fis:wait duration beyond the documented maximum.
AWS: the aws:fis:wait action's `duration` must be within 1 to 720 minutes (12 h). A request beyond that bound is refused with ValidationException (Smithy: CreateExperimentTemplate errors include ValidationException, errorKind client, httpError 400). Recorded message example: "'duration' must be within a range of 1 to 720 minutes." The requirement is the status 400 / code ValidationException and the rule that the duration bound is enforced at template-create time; the exact text is an example.
Emulator: CreateExperimentTemplate with actions={"waitAction":{"actionId":"aws:fis:wait","parameters":{"duration":"PT13H"}}} answers HTTP 200 and persists the template (parameters.duration == 'PT13H').
Why: a client that relies on AWS rejecting an out-of-range wait at create time gets a template that AWS would never have accepted, and the divergence surfaces only much later (or never) at run time, so negative-path tests silently pass against the emulator.
Fix: CreateExperimentTemplate's input validation (action-parameter validation for the built-in aws:fis:wait action).
Traces: fis.create-experiment-template-wait-duration-exceeds-max-rejected

Cause 2 of 9: StartExperiment's clientToken idempotency does not detect a parameter mismatch.
AWS: clientToken is modelled @idempotencyToken (Smithy: StartExperiment.clientToken required, @length 1..1024). Replaying a token with the SAME experimentTemplateId returns the same experiment; reusing that token with a DIFFERENT experimentTemplateId is a parameter mismatch and is refused with ConflictException (Smithy: StartExperiment errors include ConflictException, httpError 409). Recorded message example: "clientToken <token> is not assigned to this resource."
Emulator: the second StartExperiment, same clientToken but a different experimentTemplateId, answers HTTP 200 and returns the FIRST template's experiment (response carries experimentTemplateId 'EXT000000000001' although the request named the second template). No 409 is ever produced, and the token replay is keyed on the token alone, ignoring the request parameters.
Why: a client that reuses a token against the wrong template believes its experiment started on that template; it then polls/stops an experiment belonging to a different template instead of receiving the 409 AWS returns.
Fix: StartExperiment's handler/idempotency-record state (store the request parameters with the token and compare them on replay).
Traces: fis.start-experiment-already-running-conflict

Cause 3 of 9: StartExperiment does not validate that FIS can assume the template's execution role.
AWS: StartExperiment validates the template's roleArn up front and refuses with ValidationException (Smithy: StartExperiment errors include ValidationException, httpError 400) when the role's trust policy does not grant fis.amazonaws.com. Recorded message example: "Unable to assume role <roleArn>. Check the trust policy grants access to 'fis.amazonaws.com'". No experiment is created.
Emulator: a template whose role trusts only ec2.amazonaws.com starts normally — StartExperiment answers 200 and an experiment appears in ListExperiments for that template; no error is produced at all (the check observed code=None).
Why: a client gets an experiment id for a run that could never execute under real AWS; up-front permission failures are laundered into apparent successes, so role/trust misconfiguration is undetectable against the emulator.
Fix: StartExperiment's validation (resolve the template roleArn against IAM state and check the trust policy before creating the experiment).
Traces: fis.start-experiment-insufficient-role-permissions-denied

Cause 4 of 9: the active-experiments quota is not enforced.
AWS: the 'Active experiments' quota is a fixed, non-adjustable 5 per account per region (FIS user-guide quotas page); the StartExperiment call that would exceed it is refused with ServiceQuotaExceededException (Smithy: StartExperiment errors include ServiceQuotaExceededException, errorKind client, httpError 402) and creates no experiment. Recorded message example: "The number of active experiments cannot exceed 5."
Emulator: six experiments were started back to back from six templates (existing_active=0) and every StartExperiment answered 200; no ServiceQuotaExceededException was ever returned.
Why: a client's quota-handling/back-off path is never exercised, and capacity-limit behaviour that real AWS enforces per region is invisible.
Fix: StartExperiment's handler plus the account-level experiment state (count experiments in pending/initiating/running/stopping and refuse beyond the quota).
Traces: fis.start-experiment-service-quota-exceeded

Cause 5 of 9: experiments never advance to the 'completed' terminal state.
AWS: an experiment whose only action is aws:fis:wait progresses pending -> initiating -> running -> completed with no targets or permissions required, and GetExperiment reports 'completed' with an endTime once the wait elapses (Smithy: ExperimentStatus enum = pending, initiating, running, completed, stopping, stopped, failed, cancelled).
Emulator: the emulator does reach 'running' within seconds (other checks observe it, and an explicit StopExperiment moves it to 'stopped' with an endTime ~4 s after creation), but GetExperiment polling never observes 'completed': three checks exhaust their whole observation window polling for it and are reported as timeouts. The time-driven completion of the wait action is missing, so an experiment stays active indefinitely.
Why: every client workflow that waits for an experiment to finish (and anything that distinguishes completed from stopped) hangs against the emulator; it also keeps active-experiment slots occupied forever.
Fix: the experiment state machine / background progression for the experiment's actions (schedule the completion of aws:fis:wait and set state.status='completed' plus endTime).
Traces: fis.experiment-lifecycle-pending-to-completed, fis.start-experiment-transitions-to-running-then-completed, fis.stop-completed-experiment-rejected

Cause 6 of 9: StopExperiment on an experiment already in a final state is accepted instead of refused.
AWS: StopExperiment on an experiment in a final state is refused with ValidationException (Smithy: StopExperiment errors = ResourceNotFoundException httpError 404, ValidationException httpError 400) and the experiment's state/endTime are untouched. Recorded message example: "Cannot modify the state of an experiment in final state: <experimentId>."
Emulator: a second StopExperiment on an experiment already in state 'stopped' answers HTTP 200 and returns the experiment body again (state {'status':'stopped','reason':'Stopped by a StopExperiment request'}), i.e. the call is treated as idempotent.
Why: a client cannot tell a stop it actually performed from one AWS would have rejected, and retry/cleanup logic that depends on the 400 takes the wrong branch.
Fix: StopExperiment's handler (terminal-state guard before mutating experiment state).
Traces: fis.stop-experiment-already-stopped-rejected

Cause 7 of 9: length-constraint ValidationException messages use a generic framework rendering instead of FIS's per-member rule.
AWS: FIS refuses a member that exceeds its modelled @length with ValidationException (400) whose message names the member in the form "Maximum length exceeded: <member>" — e.g. "Maximum length exceeded: id" for GetExperiment.id (Smithy: GetExperiment.id target ExperimentId, @length max 64) and "Maximum length exceeded: experimentTemplateId" for ListExperiments.experimentTemplateId (Smithy: @length max 64). The message is built from the request (it names the offending member), so that rule is the requirement.
Emulator: the code is right (ValidationException) but the message is the generic framework rendering, e.g. "1 validation error detected: Value 'xxxx…' at 'id' failed to satisfy constraint: Member must have length less than or equal to 64" — it echoes the whole offending value and uses the Smithy-style template rather than FIS's "Maximum length exceeded: <member>" form.
Why: clients and operators that match on FIS's message rule mis-handle the rejection, and the emulator leaks the full rejected value back into the error body.
Fix: the shared FIS validation/error serializer for length constraints (per-member message rendering).
Traces: fis.get-experiment-malformed-id-validation-rejected, fis.list-experiments-invalid-filter-rejected, fis.list-experiments-malformed-filter-rejected

Cause 8 of 9: ResourceNotFoundException messages for experiments and experiment templates use the wrong rule.
AWS: a well-formed but unknown experiment id yields ResourceNotFoundException (Smithy: GetExperiment errors include ResourceNotFoundException, httpError 404) with a message built from the request in the form "Experiment with ID <id> not found."; an unknown or deleted experiment template on StartExperiment yields ResourceNotFoundException (Smithy: StartExperiment errors include ResourceNotFoundException, httpError 404) with the fixed message "ExperimentTemplate not found." — notably WITHOUT the identifier. Those rules are the requirement.
Emulator: the codes are right but the messages are the emulator's own: 'No experiment found with id EXP-bogus-4b5125c16edd' instead of 'Experiment with ID EXP-bogus-4b5125c16edd not found.', and 'No experiment template found with id EXT000000000001' / 'No experiment template found with id nonexistente4c6c937f48d427d' instead of 'ExperimentTemplate not found.' (the emulator additionally echoes the identifier where AWS does not).
Why: clients that key on FIS's not-found message rule mis-classify the failure, and the emulator discloses request identifiers in a message AWS keeps identifier-free.
Fix: the not-found error construction in the experiment and experiment-template lookups (GetExperiment, StartExperiment and the shared template resolver).
Traces: fis.get-experiment-unknown-id-not-found, fis.start-experiment-missing-template-not-found, fis.start-experiment-nonexistent-template-not-found

Cause 9 of 9: UpdateTargetAccountConfiguration is unimplemented.
AWS: PATCH /experimentTemplates/{experimentTemplateId}/targetAccountConfigurations/{accountId} either updates the existing configuration (200, returning targetAccountConfiguration) or, when no configuration exists for that accountId, refuses with ResourceNotFoundException (Smithy: UpdateTargetAccountConfiguration errors = ResourceNotFoundException httpError 404, ValidationException httpError 400) — there are no upsert semantics. Recorded message example: "TargetAccountConfiguration not found."
Emulator: the call answers with error code 'NotImplemented' ("operation UpdateTargetAccountConfiguration is scaffolded but not implemented in this emulator") rather than 404 ResourceNotFoundException / 200. This is a different operation from CreateTargetAccountConfiguration, whose own stub issue is already filed for this suite.
Why: no client can update a multi-account target configuration, and the no-upsert contract cannot be observed at all.
Fix: the UpdateTargetAccountConfiguration handler and the target-account-configuration state (services/service-com-amazonaws-fis-faultinjectionsimulator/src/ops/).
Traces: fis.update-target-account-configuration-nonexistent-not-found

## Evidence
Cause 1: CreateExperimentTemplate(duration='PT13H') -> HTTP 200 with experimentTemplate.actions.waitAction.parameters.duration == 'PT13H'. Expected: 400 ValidationException (example message "'duration' must be within a range of 1 to 720 minutes.").
Cause 2: StartExperiment(clientToken=shared, experimentTemplateId=<second template>) -> HTTP 200, body experiment {'id': 'EXP000000000001', 'experimentTemplateId': 'EXT000000000001', 'state': {'status': 'initiating'}}, i.e. the first template's run. Expected: 409 ConflictException (example message "clientToken <token> is not assigned to this resource.").
Cause 3: template roleArn trusts ec2.amazonaws.com only; StartExperiment -> accepted, experiment created (check recorded code=None message=None after a 60 s retry window). Expected: 400 ValidationException (example message "Unable to assume role <arn>. Check the trust policy grants access to 'fis.amazonaws.com'").
Cause 4: 6 StartExperiment calls from 6 fresh templates with existing_active=0 -> all 200 ("got None having started 6 of 5 expected slots"). Expected: the 6th call refused 402 ServiceQuotaExceededException (example message "The number of active experiments cannot exceed 5.").
Cause 5: wait-only template (duration PT1M) started; GetExperiment status polling never returns 'completed' — three checks exhaust their window and are reported as timeouts. The same emulator reaches 'running' in seconds and, on an explicit StopExperiment, 'stopped' with endTime ~4 s after creationTime (creationTime 04:29:23, endTime 04:29:27 in the recorded body), so progression exists but completion of aws:fis:wait never fires. Expected: status reaches 'completed' with an endTime, having passed through 'running'.
Cause 6: StopExperiment on an experiment already in state 'stopped' -> HTTP 200 with experiment body (state {'status': 'stopped', 'reason': 'Stopped by a StopExperiment request'}). Expected: 400 ValidationException (example message "Cannot modify the state of an experiment in final state: <id>.").
Cause 7: GetExperiment(id='x'*100) -> ValidationException with message "1 validation error detected: Value 'xxxxxxxx…' at 'id' failed to satisfy constraint: Member must have length less than or equal to 64"; expected "Maximum length exceeded: id". ListExperiments(experimentTemplateId='y'*100 / 'x'*65) -> same generic rendering at 'experimentTemplateId'; expected "Maximum length exceeded: experimentTemplateId".
Cause 8: GetExperiment(id='EXP-bogus-4b5125c16edd') -> ResourceNotFoundException, message 'No experiment found with id EXP-bogus-4b5125c16edd'; expected 'Experiment with ID EXP-bogus-4b5125c16edd not found.'. StartExperiment on a deleted/unknown template -> messages 'No experiment template found with id EXT000000000001' and 'No experiment template found with id nonexistente4c6c937f48d427d'; expected 'ExperimentTemplate not found.'.
Cause 9: UpdateTargetAccountConfiguration(experimentTemplateId=<multi-account template>, accountId='111122223333', roleArn=…) -> error code 'NotImplemented' ("operation UpdateTargetAccountConfiguration is scaffolded but not implemented in this emulator"). Expected: 404 ResourceNotFoundException (example message "TargetAccountConfiguration not found.") when no configuration exists, 200 once it does.

## Steps to Reproduce
Boot the emulator and point a botocore fis client at it (region us-west-2), then:
1. Cause 1: create_experiment_template(clientToken=…, description=…, stopConditions=[{'source':'none'}], targets={}, actions={'waitAction':{'actionId':'aws:fis:wait','parameters':{'duration':'PT13H'}}}, roleArn=<role>) — observe 200 instead of ValidationException.
2. Cause 2: create two wait templates; start_experiment(clientToken=T, experimentTemplateId=tpl1); then start_experiment(clientToken=T, experimentTemplateId=tpl2) — observe 200 returning tpl1's experiment instead of 409 ConflictException.
3. Cause 3: create an IAM role trusting ec2.amazonaws.com only, create a wait template with it, start_experiment — observe success instead of ValidationException.
4. Cause 4: start six experiments from six fresh wait templates with no other active experiment — observe all six succeed instead of the sixth being refused with ServiceQuotaExceededException.
5. Cause 5: start a wait-only experiment (duration PT1M) and poll get_experiment(id=…)['experiment']['state']['status'] — observe it never becomes 'completed'.
6. Cause 6: start a wait experiment, wait for 'running', stop_experiment(id=…), wait for 'stopped', then stop_experiment(id=…) again — observe 200 instead of 400 ValidationException.
7. Cause 7: get_experiment(id='x'*100) and list_experiments(experimentTemplateId='y'*100) — compare the ValidationException messages to "Maximum length exceeded: <member>".
8. Cause 8: get_experiment(id='EXP-bogus-0123456789ab') and start_experiment against a deleted/unknown experimentTemplateId — compare the ResourceNotFoundException messages to 'Experiment with ID <id> not found.' and 'ExperimentTemplate not found.'.
9. Cause 9: create a multi-account wait template, then update_target_account_configuration(experimentTemplateId=…, accountId='111122223333', roleArn=…) — observe NotImplemented instead of ResourceNotFoundException.
Equivalently, re-replay the behavioral checks listed in trace_ids.
