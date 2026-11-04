Issue-Source: trace-sweep-divergence
Issue-Trace: fis-target-account-config-duplicate-and-not-found-errors
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: fis-target-account-config-duplicate-and-not-found-errors fis-target-account-configuration-duplicate-and-notfound-errors fis-single-account-template-rejects-target-config
Issue-Shape-Id: com.amazonaws.fis#FaultInjectionSimulator
Issue-Key: fis|GetTargetAccountConfiguration|4xx->4xx|$.message
Issue-Key: fis|UpdateTargetAccountConfiguration|4xx->4xx|$.message
Issue-Key: fis|CreateTargetAccountConfiguration|4xx->4xx|$.message
Issue-Commit: 611eef386f1100e1b77f5a1515bab52b7a7a6aeb

# FIS target account configuration errors name the wrong resource and use non-AWS conflict wording

> _Note: this issue was identified against the code base at commit `611eef386f1100e1b77f5a1515bab52b7a7a6aeb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTargetAccountConfiguration, UpdateTargetAccountConfiguration, CreateTargetAccountConfiguration

## Description
The target-account-configuration sub-resource of an experiment template reports its failures with its own wording rather than FIS's: not-found is attributed to the child configuration where FIS attributes it to the parent template, and the duplicate and single-account refusals use message templates FIS does not emit. The statuses are correct throughout; the message text is the defect. 4 causes.

Cause 1 of 4 (GetTargetAccountConfiguration not-found names the configuration, not the template).
AWS: Smithy: GetTargetAccountConfiguration GET /experimentTemplates/{experimentTemplateId}/targetAccountConfigurations/{accountId}, errors ResourceNotFoundException (404) / ValidationException (400), message member ExceptionMessage. Recorded: 404 with message 'ExperimentTemplate not found.' — entirely fixed text naming the PARENT experiment template, with no id or account interpolated, because the parent is resolved before the child.
Emulator: 404 with 'TargetAccountConfiguration not found.' (recorded: expected 'ExperimentTemplate not found.', got 'TargetAccountConfiguration not found.').
Why: the client is told a configuration is missing from a template that exists, when FIS is reporting that the template itself does not resolve — the wrong remediation (create the configuration vs. fix the template id). Message-matching clients also diverge.
Fix: the GetTargetAccountConfiguration handler must resolve the parent experiment template first and emit FIS's template-level not-found message.
Traces: fis-target-account-config-duplicate-and-not-found-errors#task#5.

Cause 2 of 4 (UpdateTargetAccountConfiguration not-found names the configuration, not the template).
AWS: Smithy: UpdateTargetAccountConfiguration PATCH /experimentTemplates/{experimentTemplateId}/targetAccountConfigurations/{accountId}, errors ResourceNotFoundException (404) / ValidationException (400). Recorded: 404 with message 'ExperimentTemplate not found.' — fixed text, parent-level.
Emulator: 404 with 'TargetAccountConfiguration not found.' (recorded: expected 'ExperimentTemplate not found.', got 'TargetAccountConfiguration not found.').
Why: as cause 1, on the update path.
Fix: parent-first resolution and the template-level not-found message in the UpdateTargetAccountConfiguration handler.
Traces: fis-target-account-config-duplicate-and-not-found-errors#task#6.

Cause 3 of 4 (CreateTargetAccountConfiguration duplicate-conflict message).
AWS: Smithy: CreateTargetAccountConfiguration models ConflictException (409); experimentTemplateId and accountId are @required httpLabels (accountId -> TargetAccountId @length 12..48). Recorded: creating a configuration for an (template, account) pair that already has one yields 409 with message 'TargetAccountConfiguration for <experimentTemplateId> and target account <accountId> already exists.' — fixed text 'TargetAccountConfiguration for ' + the template id from the request path + ' and target account ' + the account id from the request path + ' already exists.'
Emulator: 409 with 'Experiment template <id> already has a target account configuration for account <accountId>' (recorded: expected 'TargetAccountConfiguration for EXT000000000001 and target account 000000000000 already exists.', got 'Experiment template EXT000000000001 already has a target account configuration for account 000000000000'; the template id and account are request/state values in both — the template is the divergence).
Why: the conflict is detected correctly but reported with wording FIS never emits, so clients that match on the message to distinguish 'already exists' from other 409s, or that assert the service's text, diverge.
Fix: the conflict message emitted by the CreateTargetAccountConfiguration handler's duplicate check.
Traces: fis-target-account-config-duplicate-and-not-found-errors#task#8 (also fis-target-account-configuration-duplicate-and-notfound-errors#task#5).

Cause 4 of 4 (CreateTargetAccountConfiguration single-account refusal message).
AWS: recorded behavior only for the wording (the model exposes only ValidationException/400 as the class): a template whose experimentOptions.accountTargeting is not multi-account refuses the call with 400 and the message 'ExperimentTemplate <experimentTemplateId> in AWS account <accountId> is not configured as multi account. Account targeting value is: <accountTargeting>.' — fixed text with three variable parts taken from the request and the template's state: the template id, the account that owns the template, and the template's current accountTargeting value.
Emulator: 400 with 'a target account configuration requires experimentOptions.accountTargeting of the experiment template to be multi-account.' (recorded: expected the AWS template rendered for the fixture's template/account, got that generic sentence) — a static sentence that names neither the template, nor the account, nor the current accountTargeting value.
Why: a client or operator reading the error cannot tell WHICH template or account was rejected or what its accountTargeting currently is, which is exactly the diagnostic payload AWS puts in the message; message-matching clients also diverge.
Fix: the accountTargeting gate in the CreateTargetAccountConfiguration handler must render FIS's message, interpolating the template id, owning account and the template's accountTargeting value.
Traces: fis-single-account-template-rejects-target-config#task#3.

## Evidence
Cause 1 — fis-target-account-config-duplicate-and-not-found-errors#task#5, GetTargetAccountConfiguration 404/404: $.message expected 'ExperimentTemplate not found.', got 'TargetAccountConfiguration not found.'
Cause 2 — fis-target-account-config-duplicate-and-not-found-errors#task#6, UpdateTargetAccountConfiguration 404/404: $.message expected 'ExperimentTemplate not found.', got 'TargetAccountConfiguration not found.'
Cause 3 — fis-target-account-config-duplicate-and-not-found-errors#task#8, CreateTargetAccountConfiguration 409/409: $.message expected 'TargetAccountConfiguration for EXT000000000001 and target account 000000000000 already exists.', got 'Experiment template EXT000000000001 already has a target account configuration for account 000000000000' (2 steps affected, incl. fis-target-account-configuration-duplicate-and-notfound-errors#task#5).
Cause 4 — fis-single-account-template-rejects-target-config#task#3, CreateTargetAccountConfiguration 400/400: $.message expected 'ExperimentTemplate EXT000000000001 in AWS account 000000000000 is not configured as multi account. Account targeting value is: single-account.', got 'a target account configuration requires experimentOptions.accountTargeting of the experiment template to be multi-account.'
Smithy cited: GetTargetAccountConfiguration / UpdateTargetAccountConfiguration errors ResourceNotFoundException(404)+ValidationException(400); CreateTargetAccountConfiguration errors ConflictException(409)/ResourceNotFoundException(404)/ServiceQuotaExceededException(402)/ValidationException(400), experimentTemplateId @required @httpLabel, accountId -> TargetAccountId @required @httpLabel @length 12..48; ResourceNotFoundException/ConflictException/ValidationException each carry message : ExceptionMessage (@length 0..1024).

## Steps to Reproduce
1. Replay the wire corpus against the emulator (replay_wire_corpus).
2. Cause 1: GET /experimentTemplates/{templateId}/targetAccountConfigurations/{accountId} where the template does not resolve, and read $.message (fis-target-account-config-duplicate-and-not-found-errors#task#5).
3. Cause 2: PATCH the same path and read $.message (fis-target-account-config-duplicate-and-not-found-errors#task#6).
4. Cause 3: POST /experimentTemplates/{templateId}/targetAccountConfigurations/{accountId} twice for a multi-account template and read the 409 $.message (fis-target-account-config-duplicate-and-not-found-errors#task#8).
5. Cause 4: create a template with experimentOptions.accountTargeting = single-account, then POST a target account configuration for it and read the 400 $.message (fis-single-account-template-rejects-target-config#task#3).
