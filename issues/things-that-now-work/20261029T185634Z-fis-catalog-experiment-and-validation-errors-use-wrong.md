Issue-Source: trace-sweep-divergence
Issue-Trace: fis-action-and-resource-type-catalog
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: fis-action-and-resource-type-catalog fis-get-target-resource-type-validation-error fis-get-action-not-found-error fis-experiment-error-paths fis-experiment-not-found-errors fis-experiment-target-account-configuration-not-found fis-experiment-template-error-paths fis-experiment-template-errors fis-list-actions-max-results-validation-error fis-list-target-resource-types-max-results-validation-error
Issue-Shape-Id: com.amazonaws.fis#FaultInjectionSimulator
Issue-Key: fis|GetTargetResourceType|4xx->4xx|$.__type
Issue-Key: fis|GetTargetResourceType|4xx->4xx|$.message
Issue-Key: fis|GetTargetResourceType|4xx->4xx|status
Issue-Key: fis|GetAction|4xx->4xx|$.message
Issue-Key: fis|ListExperimentResolvedTargets|4xx->4xx|$.message
Issue-Key: fis|GetExperimentTargetAccountConfiguration|4xx->4xx|$.message
Issue-Key: fis|ListExperimentTargetAccountConfigurations|4xx->4xx|$.message
Issue-Key: fis|CreateExperimentTemplate|4xx->4xx|$.__type
Issue-Key: fis|CreateExperimentTemplate|4xx->4xx|$.message
Issue-Key: fis|CreateExperimentTemplate|4xx->4xx|status
Issue-Key: fis|ListActions|4xx->4xx|$.message
Issue-Key: fis|ListTargetResourceTypes|4xx->4xx|$.message
Issue-Commit: 611eef386f1100e1b77f5a1515bab52b7a7a6aeb

# FIS catalog, experiment and validation errors use wrong status class and non-AWS message wording

> _Note: this issue was identified against the code base at commit `611eef386f1100e1b77f5a1515bab52b7a7a6aeb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTargetResourceType, GetAction, ListExperimentResolvedTargets, GetExperimentTargetAccountConfiguration, ListExperimentTargetAccountConfigurations, CreateExperimentTemplate, ListActions, ListTargetResourceTypes

## Description
Across the catalog, experiment and template surfaces the emulator chooses a different error class than FIS for the same input, and where the class is right it emits a generic constraint-report message instead of FIS's own wording. Two systematic habits produce all ten causes: it maps 'identifier does not resolve' to whichever of 404/400 its handler happens to use rather than to the class FIS uses for that operation, and it renders validation failures with a generic '1 validation error detected: ...' formatter instead of FIS's per-operation message. 10 causes.

Cause 1 of 10 (GetTargetResourceType unknown resource type: 400 ValidationException, not 404).
AWS: Smithy: GetTargetResourceType models BOTH ResourceNotFoundException (404) and ValidationException (400); for an unknown resourceType the recorded answer is 400 ValidationException with message 'TargetResourceType with ID <resourceType> not found.' — fixed text 'TargetResourceType with ID ' + the resourceType from the request + ' not found.' (note: FIS treats an unknown resource type as a validation failure, not a missing resource).
Emulator: 404 ResourceNotFoundException with 'No target resource type found with id <resourceType>' (recorded: status expected 400, got 404; $.__type expected 'ValidationException', got 'ResourceNotFoundException').
Why: a client branching on error type retries or reports a missing resource where AWS is telling it the input is invalid; status-class-based handling (400 = do not retry, fix the request) is inverted.
Fix: the GetTargetResourceType handler's unknown-type path must raise ValidationException/400 with FIS's message template.
Traces: fis-action-and-resource-type-catalog#task#6.

Cause 2 of 10 (GetTargetResourceType blank resourceType: same ValidationException message, not a generic constraint report).
AWS: Smithy: resourceType targets TargetResourceTypeId (@length 0..128, @pattern ^[\S]+$). Recorded: a whitespace-only resourceType yields 400 with the SAME message template as cause 1, 'TargetResourceType with ID <resourceType> not found.', with the request's (blank) value interpolated — FIS does not report the pattern constraint.
Emulator: 400 with "1 validation error detected: Value '<v>' at 'resourceType' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[\S]+$" (recorded: expected 'TargetResourceType with ID     not found.', got that generic text).
Why: the status is right but the message is from a different dialect; a client that parses the service's message, or asserts it, diverges, and an unresolvable id is reported as a syntax violation.
Fix: the GetTargetResourceType handler's validation must use the operation's not-found-style message rather than the generic constraint formatter.
Traces: fis-get-target-resource-type-validation-error#task#1.

Cause 3 of 10 (GetAction unknown action id message).
AWS: Smithy: GetAction errors ResourceNotFoundException (404) / ValidationException (400). Recorded: 404 with message 'Invalid actionId.' — entirely fixed text; it does NOT echo the requested id.
Emulator: 404 with 'No action found with id <id>' (recorded: expected 'Invalid actionId.', got 'No action found with id aws:not-a-real-service:fake-action'; the id is the request value, the template is the divergence).
Why: a client matching the service's message never matches, and the emulator leaks the requested id into a message AWS keeps generic.
Fix: the not-found message in the GetAction handler's catalog lookup.
Traces: fis-action-and-resource-type-catalog#task#5 (also fis-get-action-not-found-error#task#1).

Cause 4 of 10 (ListExperimentResolvedTargets unknown experiment message).
AWS: Smithy: ListExperimentResolvedTargets errors ResourceNotFoundException (404) / ValidationException (400). Recorded: 404 with message 'Experiment not found' — fixed text, no experiment id echoed, NO trailing period.
Emulator: 404 with 'Experiment with ID <experimentId> not found.' (recorded: expected 'Experiment not found', got 'Experiment with ID EXPT00000000000000 not found.').
Why: message-matching clients diverge; the emulator again echoes an identifier AWS omits.
Fix: the not-found message in the ListExperimentResolvedTargets handler's experiment lookup.
Traces: fis-experiment-error-paths#task#6 (also fis-experiment-not-found-errors#task#4).

Cause 5 of 10 (GetExperimentTargetAccountConfiguration reports the configuration, not the experiment, as missing).
AWS: Smithy: GetExperimentTargetAccountConfiguration GET /experiments/{experimentId}/targetAccountConfigurations/{accountId}, errors ResourceNotFoundException (404) / ValidationException (400). Recorded: when the experimentId does not resolve, 404 with message 'Experiment not found.' — fixed text (with trailing period), naming the EXPERIMENT, because the parent is checked before the child.
Emulator: 404 with 'TargetAccountConfiguration for experiment <experimentId> and target account <accountId> not found' (recorded: expected 'Experiment not found.', got that per-request string).
Why: the client is told the target-account configuration is missing from an experiment that exists, when in fact the experiment itself does not exist — the wrong remediation. Message-matching also fails.
Fix: the GetExperimentTargetAccountConfiguration handler must resolve and report the parent experiment first, with FIS's 'Experiment not found.' message.
Traces: fis-experiment-target-account-configuration-not-found#task#1.

Cause 6 of 10 (ListExperimentTargetAccountConfigurations unknown experiment message).
AWS: recorded: 404 with message 'Experiment not found.' — fixed text with trailing period, no id echoed.
Emulator: 404 with 'Experiment with ID <experimentId> not found.' (recorded: expected 'Experiment not found.', got 'Experiment with ID EXP99999999999999x not found.').
Why: as cause 4 — the class and status are right, the wording is not, and an id AWS omits is leaked.
Fix: the not-found message in the ListExperimentTargetAccountConfigurations handler's experiment lookup.
Traces: fis-experiment-target-account-configuration-not-found#task#2.

Cause 7 of 10 (CreateExperimentTemplate with an unpublished actionId: 404 ResourceNotFoundException, not 400 ValidationException).
AWS: Smithy: CreateExperimentTemplate models ConflictException (409), ResourceNotFoundException (404), ServiceQuotaExceededException (402) and ValidationException (400); actions is @required (CreateExperimentTemplateActionInputMap). Recorded: an actionId that FIS does not publish yields 404 ResourceNotFoundException with message "Invalid actionId in action '<actionKey>'." — fixed text "Invalid actionId in action '" + the key of the offending entry in the request's actions map + "'." (the action KEY, not the actionId value).
Emulator: 400 ValidationException with 'actions.<key>.actionId must be an action FIS publishes, got "<actionId>"' (recorded: status expected 404, got 400; $.__type expected 'ResourceNotFoundException', got 'ValidationException').
Why: a client branching on error type or status class treats an unresolvable action reference as a malformed request instead of a missing resource — opposite of cause 1's inversion, so neither mapping can be fixed by one global rule.
Fix: the CreateExperimentTemplate handler's action-reference resolution must raise ResourceNotFoundException/404 with FIS's message, keyed by the action map key.
Traces: fis-experiment-template-error-paths#task#3.

Cause 8 of 10 (CreateExperimentTemplate validation precedence: the missing required member must be reported first).
AWS: Smithy: CreateExperimentTemplate input has targets (CreateExperimentTemplateTargetInputMap) alongside required clientToken/description/stopConditions/actions/roleArn. Recorded: when the request both omits targets and names an unpublished actionId, FIS reports 400 with message 'targets is required' — fixed text naming the absent member; the structural requirement is reported BEFORE any action-reference resolution.
Emulator: 400 but with the action-reference complaint 'actions.<key>.actionId must be an action FIS publishes, got "<actionId>"' (recorded: expected 'targets is required', got that string).
Why: the client is handed the second-order problem and, after fixing the action id, hits an entirely different error — two round trips where AWS needs one, and a message-matching client never sees the member-missing error.
Fix: validation ordering in the CreateExperimentTemplate handler — structural/required-member checks before reference resolution — and the 'targets is required' message.
Traces: fis-experiment-template-errors#task#1.

Cause 9 of 10 (ListActions maxResults out of range message).
AWS: Smithy: ListActionsRequest.maxResults targets ListActionsMaxResults with @range min 1 max 100, httpQuery maxResults; the only modeled error is ValidationException (400). Recorded: 400 with message 'maxResults must be between 1 and 100' — fixed text carrying the modeled bounds; it does NOT echo the supplied value.
Emulator: 400 with "1 validation error detected: Value '<v>' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100" (recorded: expected 'maxResults must be between 1 and 100', got that generic text with the request's value).
Why: the generic formatter reports only the bound that was violated and in a different dialect, so a client asserting or parsing FIS's message diverges.
Fix: maxResults range validation in the ListActions handler must emit FIS's message instead of the generic constraint formatter.
Traces: fis-list-actions-max-results-validation-error#task#1.

Cause 10 of 10 (ListTargetResourceTypes maxResults out of range message).
AWS: Smithy: ListTargetResourceTypesRequest.maxResults targets ListTargetResourceTypesMaxResults with @range min 1 max 100; only ValidationException (400) is modeled. Recorded: 400 with message 'maxResults must be between 1 and 100' (fixed text, value not echoed).
Emulator: the same generic '1 validation error detected: ...' text (recorded: expected 'maxResults must be between 1 and 100', got the generic string).
Why: as cause 9, on the target-resource-type listing.
Fix: maxResults range validation in the ListTargetResourceTypes handler must emit FIS's message.
Traces: fis-list-target-resource-types-max-results-validation-error#task#1.

## Evidence
Cause 1 — fis-action-and-resource-type-catalog#task#6, GetTargetResourceType: status expected 400, got 404; $.__type expected 'ValidationException', got 'ResourceNotFoundException'; $.message expected 'TargetResourceType with ID aws:not:a-real-type not found.', got 'No target resource type found with id aws:not:a-real-type'.
Cause 2 — fis-get-target-resource-type-validation-error#task#1, 400/400: $.message expected 'TargetResourceType with ID     not found.', got "1 validation error detected: Value '   ' at 'resourceType' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[\S]+$".
Cause 3 — fis-action-and-resource-type-catalog#task#5, GetAction 404/404: $.message expected 'Invalid actionId.', got 'No action found with id aws:not-a-real-service:fake-action' (2 steps affected).
Cause 4 — fis-experiment-error-paths#task#6, ListExperimentResolvedTargets 404/404: $.message expected 'Experiment not found', got 'Experiment with ID EXPT00000000000000 not found.' (2 steps affected).
Cause 5 — fis-experiment-target-account-configuration-not-found#task#1, GetExperimentTargetAccountConfiguration 404/404: $.message expected 'Experiment not found.', got 'TargetAccountConfiguration for experiment EXP99999999999999x and target account 123456789012 not found'.
Cause 6 — fis-experiment-target-account-configuration-not-found#task#2, ListExperimentTargetAccountConfigurations 404/404: $.message expected 'Experiment not found.', got 'Experiment with ID EXP99999999999999x not found.'
Cause 7 — fis-experiment-template-error-paths#task#3, CreateExperimentTemplate: status expected 404, got 400; $.__type expected 'ResourceNotFoundException', got 'ValidationException'; $.message expected "Invalid actionId in action 'badAction'.", got 'actions.badAction.actionId must be an action FIS publishes, got "aws:ec2:not-a-real-action"'.
Cause 8 — fis-experiment-template-errors#task#1, CreateExperimentTemplate 400/400: $.message expected 'targets is required', got 'actions.badAction.actionId must be an action FIS publishes, got "aws:does-not-exist:no-such-action"'.
Cause 9 — fis-list-actions-max-results-validation-error#task#1, ListActions 400/400: $.message expected 'maxResults must be between 1 and 100', got "1 validation error detected: Value '101' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100".
Cause 10 — fis-list-target-resource-types-max-results-validation-error#task#1, ListTargetResourceTypes 400/400: same expected/actual pair.
Smithy cited: GetTargetResourceType errors ResourceNotFoundException(404)+ValidationException(400), resourceType -> TargetResourceTypeId @length 0..128 @pattern ^[\S]+$; GetAction errors ResourceNotFoundException(404)+ValidationException(400); ListExperimentResolvedTargets / GetExperimentTargetAccountConfiguration errors ResourceNotFoundException(404)+ValidationException(400); CreateExperimentTemplate errors ConflictException(409)/ResourceNotFoundException(404)/ServiceQuotaExceededException(402)/ValidationException(400), actions @required, targets optional in the model; ListActionsMaxResults and ListTargetResourceTypesMaxResults @range 1..100 with only ValidationException(400) modeled.

## Steps to Reproduce
1. Replay the wire corpus against the emulator (replay_wire_corpus).
2. Cause 1: GET /targetResourceTypes/<unknown type> — expect 400 ValidationException, observe 404 ResourceNotFoundException (fis-action-and-resource-type-catalog#task#6).
3. Cause 2: GET /targetResourceTypes/<whitespace-only type> and read $.message (fis-get-target-resource-type-validation-error#task#1).
4. Cause 3: GET /actions/<unknown action id> and read $.message (fis-action-and-resource-type-catalog#task#5).
5. Causes 4-6: GET /experiments/<unknown id>/resolvedTargets, GET /experiments/<unknown id>/targetAccountConfigurations/<account>, GET /experiments/<unknown id>/targetAccountConfigurations and read each $.message (fis-experiment-error-paths#task#6, fis-experiment-target-account-configuration-not-found#task#1 and #task#2).
6. Cause 7: POST /experimentTemplates with an actions entry whose actionId FIS does not publish — expect 404 ResourceNotFoundException, observe 400 ValidationException (fis-experiment-template-error-paths#task#3).
7. Cause 8: POST /experimentTemplates omitting targets AND naming an unpublished actionId — expect 'targets is required' first (fis-experiment-template-errors#task#1).
8. Causes 9-10: GET /actions?maxResults=101 and GET /targetResourceTypes?maxResults=101 and read $.message (fis-list-actions-max-results-validation-error#task#1, fis-list-target-resource-types-max-results-validation-error#task#1).
