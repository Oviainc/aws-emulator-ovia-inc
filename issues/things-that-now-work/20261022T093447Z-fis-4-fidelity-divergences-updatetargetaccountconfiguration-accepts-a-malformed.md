Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: fis.update-target-account-configuration-malformed-role-arn-rejected fis.update-target-account-configuration-nonexistent-not-found fis.update-target-account-configuration-nonexistent-rejected fis.get-experiment-target-account-configuration-not-found-rejected fis.get-experiment-target-account-configuration-cross-experiment-not-found fis.list-target-account-configurations-invalid-token-rejected
Issue-Shape-Id: com.amazonaws.fis#FaultInjectionSimulator
Issue-Key: fis.update-target-account-configuration-malformed-role-arn-rejected|_error#7|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: fis.update-target-account-configuration-nonexistent-not-found|run#8|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: fis.update-target-account-configuration-nonexistent-rejected|run#13|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: fis.get-experiment-target-account-configuration-not-found-rejected|run#16|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: fis.get-experiment-target-account-configuration-cross-experiment-not-found|run#21|AssertionError|assert message == expected, f"expected {expected!r}, got {message!r}"
Issue-Key: fis.list-target-account-configurations-invalid-token-rejected|run#16|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Commit: 0b8884c4ab978d23b4fe746f795a1277a2a36df3

# FIS: 4 fidelity divergences — UpdateTargetAccountConfiguration accepts a malformed roleArn, and the not-found / invalid-nextToken error messages do not follow FIS's rules

> _Note: this issue was identified against the code base at commit `0b8884c4ab978d23b4fe746f795a1277a2a36df3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateTargetAccountConfiguration

## Description
Four distinct net-new root causes in the emulated FIS target-account-configuration control plane, each grounded in a failing behavioral check of the replay suite. A fixer must close ALL FOUR causes before submitting. Smithy citations are from services/service-com-amazonaws-fis-faultinjectionsimulator/smithy-model/com-amazonaws-fis-faultinjectionsimulator.json. The open issue 20261022T063355Z-fis-8-fidelity-divergences-targetaccountconfiguration-accepts-invalid-accountid.md covers the CREATE path of this resource and the conflict message; none of the four causes below is carried by it (its replay keys name only Create*/experiment-runtime checks), and the overlap is called out explicitly on Cause 1.

Cause 1 of 4: UpdateTargetAccountConfiguration does not validate roleArn — a value that is not an ARN at all is accepted with HTTP 200 and overwrites the stored configuration.
AWS: roleArn is modelled only loosely (Smithy: UpdateTargetAccountConfiguration.roleArn -> RoleArn, @length 20..2048, @pattern ^[\S]+$), so the ARN syntax rule is enforced server-side: a value that satisfies the shape bound but is not a well-formed IAM role ARN is refused with ValidationException (Smithy: UpdateTargetAccountConfiguration errors are ResourceNotFoundException, errorKind client, httpError 404 and ValidationException, errorKind client, httpError 400) and the previously stored roleArn is left untouched. The test value 'this-is-not-a-valid-iam-role-arn-000000' is 39 characters with no whitespace, so it passes @length/@pattern and the rejection can only come from the handler's own ARN validation. Recorded message example: 'Value for roleArn is not a valid ARN.' — the requirement is the 400/ValidationException classification plus the rule that the malformed value never persists.
Emulator: the call answers HTTP 200 with body {'targetAccountConfiguration': {'roleArn': 'this-is-not-a-valid-iam-role-arn-000000', 'accountId': '111122223333'}}, i.e. the garbage value is accepted and replaces a previously valid roleArn.
Why: a client's negative path is never exercised, and a good configuration is silently destroyed by a typo'd update — the failure surfaces (if ever) only when FIS tries to assume the role at experiment time, instead of at the mutating call AWS rejects.
Fix: UpdateTargetAccountConfiguration's input validation (parse roleArn as an IAM role ARN, and apply the same account-ownership rule as the create path, before the state write).
NOTE for the fixer: Cause 2 of the open issue 20261022T063355Z-... describes the same missing ARN validation on CreateTargetAccountConfiguration only; this cause is the UPDATE handler, which that issue carries no check for. Both handlers must enforce the rule.
Traces: fis.update-target-account-configuration-malformed-role-arn-rejected

Cause 2 of 4: the template-scoped TargetAccountConfiguration not-found message is the emulator's own prose instead of FIS's fixed message.
AWS: Get/UpdateTargetAccountConfiguration for an accountId that is not configured on the given experiment template is refused with 404 ResourceNotFoundException (Smithy: both operations list ResourceNotFoundException, errorKind client, httpError 404) and the message is the constant 'TargetAccountConfiguration not found.' — recorded behavior, and the rule is a constant rather than a request-derived string because the identical text is recorded for three different template/account pairs across three independent checks (never-configured account, deleted account, post-delete mutation).
Emulator: the status and code are correct, but the message is 'No target account configuration found for account 111122223333 in experiment template EXT000000000001' (and the same form with account 555555555555 / template EXT000000000001).
Why: clients, SDK wrappers and operators that key on FIS's message to distinguish 'this account was never configured' from other 404s mis-classify the error against the emulator.
Fix: the not-found error construction in the GetTargetAccountConfiguration and UpdateTargetAccountConfiguration handlers (emit FIS's constant message).
Traces: fis.update-target-account-configuration-nonexistent-not-found, fis.update-target-account-configuration-nonexistent-rejected

Cause 3 of 4: GetExperimentTargetAccountConfiguration's not-found message does not follow FIS's request-derived rule.
AWS: a lookup for an accountId that has no frozen configuration on the given experiment is refused with 404 ResourceNotFoundException (Smithy: GetExperimentTargetAccountConfiguration -> GET /experiments/{experimentId}/targetAccountConfigurations/{accountId}, errors ResourceNotFoundException 404, ValidationException 400) and the message is BUILT FROM THE REQUEST, naming the experiment id and then the target account id: 'TargetAccountConfiguration for experiment <experimentId> and target account <accountId> not found'. That rule is the requirement; it is confirmed by two recordings with different experiment/account pairs (EXP000000000001/442519065733 and EXP000000000002/420784474840).
Emulator: the status and code are correct, but the message reverses the rule and uses its own wording: 'No target account configuration found for account 442519065733 in experiment EXP000000000001'.
Why: a client that parses or matches FIS's documented message form to report which experiment/account pair was missing gets no match against the emulator.
Fix: the not-found error construction in the GetExperimentTargetAccountConfiguration handler (format the message from experimentId and accountId in FIS's order/wording).
Traces: fis.get-experiment-target-account-configuration-not-found-rejected, fis.get-experiment-target-account-configuration-cross-experiment-not-found

Cause 4 of 4: ListTargetAccountConfigurations rejects a corrupted nextToken with the wrong message.
AWS: an invalid/corrupted pagination token is refused with 400 ValidationException (Smithy: ListTargetAccountConfigurations.nextToken -> NextToken, @httpQuery nextToken, @length 1..1024, @pattern ^[\S]+$; errors include ValidationException, errorKind client, httpError 400; paginated inputToken/outputToken nextToken) and the message is the constant 'The provided nextToken is invalid.' (recorded behavior; the text does not vary with the token or the template).
Emulator: the status and code are correct, but the message is the emulator's internal explanation: 'nextToken is not a token returned by ListTargetAccountConfigurations'.
Why: paginating clients that surface or match the service's invalid-token message report something AWS never says; the generic form is also what distinguishes an invalid token from other validation failures.
Fix: the nextToken validation error in the ListTargetAccountConfigurations handler.
Traces: fis.list-target-account-configurations-invalid-token-rejected

## Evidence
Cause 1: create_target_account_configuration(experimentTemplateId=<multi-account tpl>, accountId='111122223333', roleArn='arn:aws:iam::111122223333:role/sim2real-fis-target-...') then update_target_account_configuration(experimentTemplateId=<tpl>, accountId='111122223333', roleArn='this-is-not-a-valid-iam-role-arn-000000') -> HTTP 200, body {'targetAccountConfiguration': {'roleArn': 'this-is-not-a-valid-iam-role-arn-000000', 'accountId': '111122223333'}} (AssertionError: expected the call to be rejected, but it SUCCEEDED and returned ...). Expected: 400 ValidationException, and get_target_account_configuration still showing the pre-update roleArn.
Cause 2: update_target_account_configuration(experimentTemplateId='EXT000000000001', accountId='111122223333'|'555555555555', roleArn=<valid arn>) for a never-configured account -> ResourceNotFoundException with message 'No target account configuration found for account 111122223333 in experiment template EXT000000000001'; expected 'TargetAccountConfiguration not found.' (AssertionError: expected message 'TargetAccountConfiguration not found.', got 'No target account configuration found for account ... in experiment template EXT000000000001'). The same constant is expected from get_target_account_configuration on a deleted/never-configured account.
Cause 3: get_experiment_target_account_configuration(experimentId='EXP000000000001', accountId='442519065733') -> ResourceNotFoundException with message 'No target account configuration found for account 442519065733 in experiment EXP000000000001'; expected 'TargetAccountConfiguration for experiment EXP000000000001 and target account 442519065733 not found'. Cross-experiment variant: experimentId='EXP000000000002', accountId='420784474840' -> same divergence.
Cause 4: list_target_account_configurations(experimentTemplateId=<tpl>, nextToken=<corrupted token>) -> ValidationException with message 'nextToken is not a token returned by ListTargetAccountConfigurations'; expected 'The provided nextToken is invalid.'

## Steps to Reproduce
Boot the emulator and point botocore 'fis' and 'iam' clients at it in us-west-2, then:
1. Create an IAM role trusting fis.amazonaws.com and create a wait template with experimentOptions={'accountTargeting': 'multi-account'} (actions {waitAction: {actionId: aws:fis:wait, parameters: {duration: PT1M}}}, targets {}, stopConditions [{source: none}]).
2. Cause 1: create_target_account_configuration(accountId='111122223333', roleArn='arn:aws:iam::111122223333:role/x'), then update_target_account_configuration(accountId='111122223333', roleArn='this-is-not-a-valid-iam-role-arn-000000') — observe HTTP 200 and that get_target_account_configuration now returns the malformed value.
3. Cause 2: update_target_account_configuration(accountId='555555555555', roleArn='arn:aws:iam::555555555555:role/x') for an account never configured on the template, and get_target_account_configuration for the same account — compare both messages to 'TargetAccountConfiguration not found.'
4. Cause 3: start an experiment from a multi-account template and call get_experiment_target_account_configuration(experimentId=<that experiment>, accountId=<an account with no configuration on it>) — compare the message to 'TargetAccountConfiguration for experiment <experimentId> and target account <accountId> not found'.
5. Cause 4: call list_target_account_configurations(experimentTemplateId=<tpl>, nextToken='not-a-real-token') — compare the message to 'The provided nextToken is invalid.'
Equivalently, re-replay the behavioral checks listed in trace_ids.
