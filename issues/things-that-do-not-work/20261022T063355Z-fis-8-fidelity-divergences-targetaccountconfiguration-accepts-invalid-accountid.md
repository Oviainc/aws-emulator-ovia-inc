Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: fis.create-target-account-configuration-out-of-range-account-id-rejected fis.target-account-configuration-invalid-account-id-rejected fis.create-target-account-configuration-malformed-role-arn-rejected fis.target-account-configuration-invalid-role-arn-rejected fis.create-target-account-configuration-duplicate-conflict fis.target-account-configuration-empty-vs-absent-role-arn-preserved fis.fis-stop-condition-breach-halts-a-live-api-fault-and-the-fault-actually-stops fis.experiment-halts-on-stop-condition-breach fis.start-experiment-with-role-lacking-permissions-denied fis.e2e-experiment-full-flow-with-target-accounts
Issue-Shape-Id: com.amazonaws.fis#FaultInjectionSimulator
Issue-Key: fis.create-target-account-configuration-out-of-range-account-id-rejected|_error#8|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded and returned {result!r}")
Issue-Key: fis.target-account-configuration-invalid-account-id-rejected|_error#13|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: fis.create-target-account-configuration-malformed-role-arn-rejected|_error#6|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded and returned {result!r}")
Issue-Key: fis.target-account-configuration-invalid-role-arn-rejected|_error#8|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: fis.create-target-account-configuration-duplicate-conflict|run#12|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: fis.target-account-configuration-empty-vs-absent-role-arn-preserved|run#23|AssertionError|assert "description" not in got_empty, f"expected empty description normalized to absent, got {got_empty!r}"
Issue-Key: fis.fis-stop-condition-breach-halts-a-live-api-fault-and-the-fault-actually-stops|_poll_action_status#26|KeyError|last = fis.get_experiment(
Issue-Key: fis.experiment-halts-on-stop-condition-breach|_poll_experiment_status#16|Timeout|time.sleep(3.0)
Issue-Key: fis.start-experiment-with-role-lacking-permissions-denied|_poll_experiment_status#13|Timeout|time.sleep(3.0)
Issue-Key: fis.e2e-experiment-full-flow-with-target-accounts|_poll_experiment_terminal#28|Timeout|time.sleep(interval_s)
Issue-Commit: 360c4c1618594432a64f84ba94d0fa368ed8f619

# FIS: 8 fidelity divergences — TargetAccountConfiguration accepts invalid accountId/roleArn, wrong conflict message rule, empty description persisted, GetExperiment actions omit state, stop conditions/permission failures/action completion never modelled

> _Note: this issue was identified against the code base at commit `360c4c1618594432a64f84ba94d0fa368ed8f619`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTargetAccountConfiguration

## Description
Eight distinct root causes in the emulated FIS (Fault Injection Service) surface, each grounded in a failing behavioral check of the replay suite. A fixer must close ALL EIGHT causes before submitting. Smithy citations below are from services/service-com-amazonaws-fis-faultinjectionsimulator/smithy-model/com-amazonaws-fis-faultinjectionsimulator.json. Two already-open issues touch this service (20261022T043627Z-fis-control-plane-9-fidelity-divergences-no-wait.md and 20261022T043627Z-fis-describestacks-omits-the-outputs-member-for-a-stack.md); the causes below are the ones NOT carried by those issues, except where explicitly noted on Cause 8.

Cause 1 of 8: CreateTargetAccountConfiguration accepts an accountId that is not a 12-digit AWS account number.
AWS: accountId is modelled only loosely (Smithy: CreateTargetAccountConfiguration.accountId -> TargetAccountId, @httpLabel, required, @length 12..48, @pattern ^[\S]+$), so the 12-digit-numeric rule is enforced server-side: a value that passes the shape bound but is not a well-formed account number is refused with ValidationException (Smithy: CreateTargetAccountConfiguration errors include ValidationException, errorKind client, httpError 400) and no configuration is persisted. Recorded message examples: "The accountId parameter value is not valid. Specify a valid accountId." and, for a 13-digit value paired with this account's role, a ValidationException naming the role/account mismatch. The requirement is the 400 / ValidationException classification and the rule that a non-12-digit accountId never persists; the exact text is an example.
Emulator: the call answers HTTP 200 and persists the bad value — accountId 'abcdefghijkl' comes back in targetAccountConfiguration, and accountId '1234567890123' (13 digits) likewise returns 200 with the configuration stored.
Why: a client's negative path is never exercised and a configuration real AWS would never have accepted is stored; a typo'd or truncated account id silently becomes part of a multi-account experiment template instead of being refused at configure time.
Fix: CreateTargetAccountConfiguration's input validation (accountId format check before the state write).
Traces: fis.create-target-account-configuration-out-of-range-account-id-rejected, fis.target-account-configuration-invalid-account-id-rejected

Cause 2 of 8: CreateTargetAccountConfiguration accepts a roleArn that is not a valid IAM role ARN and does not check that the role belongs to accountId.
AWS: roleArn is modelled as RoleArn (@length 20..2048, @pattern ^[\S]+$) so the ARN syntax and the role-owns-account rule are enforced server-side; a malformed ARN, or an ARN whose account part is not the accountId being configured, is refused with ValidationException (400) and nothing is persisted. Recorded message example: "The roleArn parameter value is not valid. The IAM role must belong to the account specified in the accountId parameter value."
Emulator: HTTP 200 in both shapes of the violation — roleArn 'arn:aws:iam::not-an-account-id:role/sim2real-fis-malformed' with accountId '111122223333' is stored verbatim, and the free-form string 'not-a-valid-role-arn-value' is also accepted and returned as targetAccountConfiguration.roleArn.
Why: an unusable role reference is persisted, so the failure surfaces (if ever) only when the experiment tries to assume the role; clients cannot distinguish a configuration AWS accepted from one it would have rejected at create time.
Fix: CreateTargetAccountConfiguration's input validation (roleArn ARN parse plus account-ownership check against accountId).
Traces: fis.create-target-account-configuration-malformed-role-arn-rejected, fis.target-account-configuration-invalid-role-arn-rejected

Cause 3 of 8: the duplicate-configuration ConflictException message does not follow FIS's rule.
AWS: creating a second configuration for an accountId already configured on the same template is refused with ConflictException (Smithy: CreateTargetAccountConfiguration errors include ConflictException, errorKind client, httpError 409). The message is built from the request — it names the experiment template id and the target account id — in the form "TargetAccountConfiguration for <experimentTemplateId> and target account <accountId> already exists.", so that rule is the requirement.
Emulator: the code/status are right but the message is the emulator's own prose: 'Experiment template EXT000000000001 already has a target account configuration for account 111111111111' instead of 'TargetAccountConfiguration for EXT000000000001 and target account 111111111111 already exists.'
Why: clients and operators that key on FIS's documented message rule mis-classify the conflict.
Fix: the conflict-error construction in the CreateTargetAccountConfiguration handler.
Traces: fis.create-target-account-configuration-duplicate-conflict

Cause 4 of 8: an explicitly empty description is persisted as present-but-empty instead of being normalized to absent.
AWS: recorded behavior only (the model, TargetAccountConfiguration.description -> @length 0..512 @pattern ^[\s\S]*$, permits an empty string). On real FIS an explicitly empty-string description is normalized exactly like an omitted one: GetTargetAccountConfiguration comes back with the 'description' key ABSENT, while a non-empty description round-trips byte-exact.
Emulator: GetTargetAccountConfiguration returns {'roleArn': ..., 'accountId': ..., 'description': ''} — the empty string is stored and serialized as a present member, creating a third representation AWS does not have.
Why: a client that branches on presence of 'description' (or diffs a configuration it just wrote) sees a member AWS would have omitted, so round-trip/idempotency comparisons disagree with AWS.
Fix: CreateTargetAccountConfiguration's handler/state write (normalize an empty description to absent) or the TargetAccountConfiguration serializer.
Traces: fis.target-account-configuration-empty-vs-absent-role-arn-preserved

Cause 5 of 8: GetExperiment's per-action entries omit the `state` member.
AWS: each entry of Experiment.actions is an ExperimentAction which carries `state` -> ExperimentActionState {status, reason} plus startTime/endTime (Smithy: ExperimentAction members actionId, description, parameters, targets, startAfter, state, startTime, endTime). A client can therefore read experiment.actions[<name>].state.status to see whether an individual action is pending/initiating/running/completed/stopped/failed.
Emulator: GetExperiment returns the action map without a 'state' member at all — the check's read of experiment['actions']['fault']['state'] raises KeyError: 'state'.
Why: per-action progress is unobservable; any client that waits for a specific action to be running, or that reports which action failed, breaks with a missing-key error rather than reading a status.
Fix: the GetExperiment/Experiment serializer and the experiment state (populate ExperimentAction.state, and startTime/endTime, per action).
Traces: fis.fis-stop-condition-breach-halts-a-live-api-fault-and-the-fault-actually-stops

Cause 6 of 8: CloudWatch-alarm stop conditions are never evaluated, so the service never halts an experiment on breach.
AWS: a template whose stopConditions contains {source: 'aws:cloudwatch:alarm', value: <alarmArn>} makes FIS poll that alarm while the experiment runs; when the alarm goes to ALARM the SERVICE transitions the experiment to the terminal status 'stopped' (Smithy: ExperimentStatus includes stopped) with no StopExperiment call from the caller.
Emulator: the experiment reaches 'running', the alarm is driven to ALARM via cloudwatch:SetAlarmState, and the experiment never leaves 'running' — the check exhausts its observation window and is reported as a timeout. No stop-condition evaluation exists.
Why: the blast-radius guardrail that is the whole point of a stop condition is absent; a client that relies on the service aborting an experiment on an alarm breach hangs, and an emulator-based test of a safety control passes vacuously.
Fix: the experiment runtime/background progression (evaluate ExperimentStopCondition entries against CloudWatch alarm state and drive the experiment to 'stopped' with a stop-condition reason).
Traces: fis.experiment-halts-on-stop-condition-breach

Cause 7 of 8: execution-time role-permission failures are not modelled — an experiment whose role lacks the action's permission never reaches 'failed'.
AWS: an action such as aws:cloudwatch:assert-alarm-state is executed under the template's roleArn; when that role lacks the required permission (cloudwatch:DescribeAlarms) the experiment ends in the terminal status 'failed' (Smithy: ExperimentStatus includes failed) and ExperimentState.reason names an insufficient-privileges failure, while an otherwise identical template with a permissioned role reaches 'completed'.
Emulator: the under-privileged experiment never reaches any terminal status — polling GetExperiment status for the terminal set exhausts the window and the check is reported as a timeout. The emulator neither executes the action nor fails the experiment, so least-privilege enforcement is unobservable.
Why: a client cannot distinguish a run that genuinely succeeded from one AWS would have failed on permissions; failure-handling branches are never taken and IAM misconfiguration is invisible against the emulator.
Fix: the experiment runtime (execute/verify the action under the template role and set state.status='failed' with a reason when the call is denied).
Traces: fis.start-experiment-with-role-lacking-permissions-denied

Cause 8 of 8: a multi-account experiment with a non-wait action never reaches 'completed', so the whole multi-account flow cannot be observed.
AWS: an experiment started from a multi-account template (experimentOptions.accountTargeting='multi-account') whose single action is aws:fis:inject-api-throttle-error with duration PT1M progresses to 'completed' once the action's duration elapses; ListExperimentResolvedTargets then reports the resolved target and GetExperimentTargetAccountConfiguration reports the frozen configuration (Smithy: ExperimentStatus includes completed).
Emulator: GetExperiment status polling never returns a terminal status for such an experiment — the check exhausts its window and is reported as a timeout, so none of the downstream multi-account assertions are ever reached. NOTE for the fixer: this is the same missing time-driven action completion described as Cause 5 of the open issue 20261022T043627Z-fis-control-plane-9-fidelity-divergences-no-wait.md (which covers aws:fis:wait); it is recorded here because that issue does not carry this check, and it must also hold for non-wait actions with a duration parameter.
Why: every end-to-end client workflow that waits for a multi-account experiment to finish hangs, and the target-account wiring (resolved targets, frozen target-account configuration) can never be verified.
Fix: the experiment runtime/action scheduler (complete any action whose duration has elapsed, for all action types, and drive the experiment to 'completed' with an endTime).
Traces: fis.e2e-experiment-full-flow-with-target-accounts

## Evidence
Cause 1: create_target_account_configuration(experimentTemplateId=<multi-account tpl>, accountId='abcdefghijkl', roleArn='arn:aws:iam::123456789012:role/sim2real-abcdefghijkl') -> HTTP 200, body {'targetAccountConfiguration': {'roleArn': 'arn:aws:iam::123456789012:role/sim2real-abcdefghijkl', 'accountId': 'abcdefghijkl'}}. Same with accountId='1234567890123' (13 digits) -> HTTP 200 with description 'should be rejected' persisted. Expected: 400 ValidationException, nothing persisted (example message "The accountId parameter value is not valid. Specify a valid accountId.").
Cause 2: create_target_account_configuration(accountId='111122223333', roleArn='arn:aws:iam::not-an-account-id:role/sim2real-fis-malformed') -> HTTP 200 with that roleArn echoed back; create_target_account_configuration(accountId='111111111111', roleArn='not-a-valid-role-arn-value') -> HTTP 200 with roleArn 'not-a-valid-role-arn-value'. Expected: 400 ValidationException (example message "The roleArn parameter value is not valid. The IAM role must belong to the account specified in the accountId parameter value.").
Cause 3: duplicate create -> ConflictException with message 'Experiment template EXT000000000001 already has a target account configuration for account 111111111111'; expected the request-derived form 'TargetAccountConfiguration for EXT000000000001 and target account 111111111111 already exists.'
Cause 4: create with description='' then get_target_account_configuration -> {'roleArn': 'arn:aws:iam::688034766475:role/sim2real-target-role', 'accountId': '688034766475', 'description': ''}; expected the 'description' key to be absent (same representation as an omitted description).
Cause 5: get_experiment(id=<experiment>)['experiment']['actions']['fault']['state'] -> KeyError: 'state' (the action entry has no 'state' member). Expected ExperimentAction.state.status to be readable (e.g. 'running' then 'stopped').
Cause 6: wait-only template with stopConditions=[{'source':'aws:cloudwatch:alarm','value':<alarmArn>}], experiment observed 'running', alarm driven OK -> ALARM via set_alarm_state; experiment status never becomes terminal (check reported 'Check timed out after 60.0s'; its own poll budget for a terminal status was 180 s). Expected: status 'stopped' with no caller StopExperiment.
Cause 7: template action aws:cloudwatch:assert-alarm-state with a role WITHOUT cloudwatch:DescribeAlarms; polling get_experiment for a terminal status never terminates (check reported 'Check timed out after 60.0s'). Expected: status 'failed' with state.reason naming an insufficient-privileges failure, while the permissioned sibling template reaches 'completed'.
Cause 8: multi-account template (accountTargeting='multi-account', action aws:fis:inject-api-throttle-error duration PT1M) + create_target_account_configuration, then start_experiment; status never reaches a terminal value (check reported 'Check timed out after 60.0s'). Expected: 'completed', then list_experiment_resolved_targets returning exactly the one aws:iam:role target named 'myRole'.

## Steps to Reproduce
Boot the emulator and point a botocore 'fis' client (plus 'iam' and 'cloudwatch' clients) at it in region us-west-2, then:
1. Cause 1: create a wait template with experimentOptions={'accountTargeting':'multi-account'} and call create_target_account_configuration(experimentTemplateId=tpl, accountId='abcdefghijkl', roleArn='arn:aws:iam::123456789012:role/x') and again with accountId='1234567890123' — observe 200 instead of ValidationException, and that get_target_account_configuration then finds the configuration.
2. Cause 2: on the same template call create_target_account_configuration(accountId='111122223333', roleArn='arn:aws:iam::not-an-account-id:role/x') and create_target_account_configuration(accountId='111111111111', roleArn='not-a-valid-role-arn-value') — observe 200 in both cases.
3. Cause 3: create a configuration for accountId='111111111111', then repeat the identical call — compare the ConflictException message to 'TargetAccountConfiguration for <templateId> and target account <accountId> already exists.'
4. Cause 4: create a configuration with description='' and then get_target_account_configuration — observe 'description': '' present instead of absent.
5. Cause 5: start any experiment and read get_experiment(id=...)['experiment']['actions'][<actionName>] — observe no 'state' member.
6. Cause 6: put_metric_alarm, set_alarm_state(OK), create a template with stopConditions=[{'source':'aws:cloudwatch:alarm','value':<alarmArn>}] and actions={'wait1':{'actionId':'aws:fis:wait','parameters':{'duration':'PT10M'}}}, start_experiment, wait for 'running', set_alarm_state(ALARM), then poll get_experiment status — observe it stays 'running'.
7. Cause 7: create two roles (one with cloudwatch:DescribeAlarms, one without), create identical templates with actions={'assert1':{'actionId':'aws:cloudwatch:assert-alarm-state','parameters':{'alarmArns':<arn>,'alarmStates':'INSUFFICIENT_DATA'}}}, start both and poll get_experiment status — observe neither reaches 'failed'/'completed'.
8. Cause 8: create a multi-account template whose action is aws:fis:inject-api-throttle-error (duration PT1M) with an aws:iam:role target, add a target account configuration, start_experiment and poll get_experiment status — observe it never reaches 'completed'.
Equivalently, re-replay the behavioral checks listed in trace_ids.
