Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: config.configuration-recorder-lifecycle config.delete-service-linked-recorder-conflict-while-active-blocked config.delete-service-linked-recorder-while-recording-conflict-blocked config.delete-config-rule-with-attached-remediation-rejected config.e2e-external-evaluation-persists-when-delete-evaluation-results-denied
Issue-Shape-Id: com.amazonaws.configservice#StarlingDoveService
Issue-Key: config.configuration-recorder-lifecycle|run#9|AssertionError|assert message == "The recording group provided is not valid", message
Issue-Key: config.delete-service-linked-recorder-conflict-while-active-blocked|run#8|AssertionError|assert message == expected, (
Issue-Key: config.delete-service-linked-recorder-while-recording-conflict-blocked|run#10|AssertionError|assert message == expected, (
Issue-Key: config.delete-config-rule-with-attached-remediation-rejected|run#33|AssertionError|assert message == (
Issue-Key: config.e2e-external-evaluation-persists-when-delete-evaluation-results-denied|_error#14|AssertionError|raise AssertionError("expected the call to be rejected, but it succeeded")
Issue-Commit: 70ed2d2e7473e9995477fa4e995826b18e438b3d

# config: 5 net-new refusal divergences on the recorder, config-rule and evaluation-results planes (InvalidRecordingGroupException/UnmodifiableEntityException/ResourceInUseException wording, DeleteEvaluationResults not refused)

> _Note: this issue was identified against the code base at commit `70ed2d2e7473e9995477fa4e995826b18e438b3d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutConfigurationRecorder

## Description
Five distinct net-new root causes on AWS Config's refusal paths: four operations answer the modeled error class but with emulator-authored prose instead of AWS's message text, and one operation does not refuse at all. FIVE CAUSES must be closed.

Note on neighbouring issues: the fixed issue 20261030T003924Z-config-recorder-tagging-and-retention-refusals-answer-wrong.md covered DescribeDeliveryChannelStatus, AssociateResourceTypes, DeleteServiceLinkedConfigurationRecorder, StartConfigRulesEvaluation, GetResourceConfigHistory, ListTagsForResource, DeleteRetentionConfiguration, StartResourceEvaluation, TagResource and PutThirdPartyServiceLinkedConfigurationRecorder — none of the five operations below. The open issue 20261030T093351Z-config-6-net-new-behavioral-root-causes-putconfigrule.md covers PutConfigRule/aggregator/tag ValidationException renderings and does not cover these operations either.

Cause 1 of 5 — PutConfigurationRecorder's invalid-recordingGroup refusal message is emulator prose.
AWS: PutConfigurationRecorder models InvalidRecordingGroupException (client, httpError 400, defaulted) (Smithy: PutConfigurationRecorder.errors; InvalidRecordingGroupException doc: "Setting the allSupported field of RecordingGroup to true, but providing a non-empty list for the resourceTypes field of RecordingGroup"). The recording shows 400 with the error code InvalidRecordingGroupException and the FIXED, request-independent sentence "The recording group provided is not valid" — no member names and no echoed values.
Emulator: the status and code are right, but the message is "RecordingGroup.allSupported is true, so RecordingGroup.resourceTypes must be empty" — the emulator's own explanatory prose naming model member paths.
Why: a client that branches on, logs or asserts AWS's sentence (the only payload this error carries besides the code) does not match, and the response leaks internal member-path wording AWS never sends.
Fix: PutConfigurationRecorder's input validation / error construction — emit AWS's fixed sentence for every InvalidRecordingGroupException case instead of a per-rule description.
Traces: config.configuration-recorder-lifecycle.

Cause 2 of 5 — StopConfigurationRecorder (and, on the same code path, StartConfigurationRecorder) on an active SERVICE-LINKED recorder refuses with the wrong message.
AWS: StopConfigurationRecorder models UnmodifiableEntityException (client, 400) (Smithy: StopConfigurationRecorder.errors; StartConfigurationRecorder.errors models it too; UnmodifiableEntityException doc: "The requested operation is not valid" for service-linked configuration recorders). The recording shows 400 UnmodifiableEntityException whose message is the template "<OperationName> operation does not apply to a service-linked configuration recorder <recorderName> because it's always recording": the fixed text is " operation does not apply to a service-linked configuration recorder " + " because it's always recording", the variable parts are the operation name (StopConfigurationRecorder / StartConfigurationRecorder) and the recorder's name taken from the request/state (e.g. the AWSConfigurationRecorderForObservabilityadmin recorder created for ServicePrincipal observabilityadmin.amazonaws.com — that literal name is a per-run/per-principal value, not part of the template).
Emulator: right code, message "This operation does not affect service-linked configuration recorders. Service-linked configuration recorders are always recording. To stop recording, you must delete the service-linked configuration recorder. Use the DeleteServiceLinkedConfigurationRecorder operation to delete a service-linked configuration recorder." — it names neither the operation nor the recorder, and is a four-sentence paraphrase AWS does not send. (The replay aborted on the Stop assertion, so the identically-worded Start refusal in the same check was not reached; it is the same message template with the operation name substituted and belongs to this cause.)
Why: the message is the only place AWS identifies WHICH recorder and WHICH operation was refused; a caller asserting or parsing it (as the behavioral check does) fails, and automation keyed on the "always recording" sentence takes the wrong branch.
Fix: the service-linked-recorder guard shared by StopConfigurationRecorder/StartConfigurationRecorder — build the UnmodifiableEntityException message from the operation name and the recorder name.
Traces: config.delete-service-linked-recorder-conflict-while-active-blocked.

Cause 3 of 5 — DeleteConfigurationRecorder on a service-linked recorder refuses with the wrong message.
AWS: DeleteConfigurationRecorder models UnmodifiableEntityException (client, 400) (Smithy: DeleteConfigurationRecorder.errors). The recording shows 400 UnmodifiableEntityException with the template "DeleteConfigurationRecorder operation does not apply to a service-linked configuration recorder <recorderName>. The request was rejected because service-linked configuration recorders are protected AWS resources. Please use DeleteServiceLinkedConfigurationRecorder API to attempt the deletion." — fixed text apart from the recorder name taken from the request.
Emulator: right code, message "You cannot use this operation to delete a service-linked configuration recorder. Use the DeleteServiceLinkedConfigurationRecorder operation to delete a service-linked configuration recorder." — the recorder is not named and the "protected AWS resources" sentence is absent.
Why: same as cause 2 — the refused recorder is unidentifiable from the response, and message-keyed client handling diverges.
Fix: DeleteConfigurationRecorder's service-linked guard / error construction.
Traces: config.delete-service-linked-recorder-while-recording-conflict-blocked.

Cause 4 of 5 — DeleteConfigRule's in-use refusal (an attached remediation configuration) is reworded.
AWS: DeleteConfigRule models ResourceInUseException (client, 400) (Smithy: DeleteConfigRule.errors; ResourceInUseException doc covers the DeleteConfigRule cases). The recording shows 400 ResourceInUseException with the template "The rule <configRuleName> currently has a RemediationConfiguration. Please delete the RemediationConfiguration and try again." — fixed text around one variable, the rule name from the request, unquoted.
Emulator: right code, message "A remediation action is associated with the Config rule '<configRuleName>' and Config cannot delete this rule. Delete the remediation action associated with the rule before deleting the rule and try your request again later." — different wording, the rule name quoted, and it says "remediation action" where AWS names the RemediationConfiguration resource.
Why: callers that detect this specific in-use reason by message (ResourceInUseException is also raised while a rule is already deleting and for evaluation results) cannot distinguish the cases, and assertions on AWS's sentence fail.
Fix: DeleteConfigRule's handler pre-delete check / error construction.
Traces: config.delete-config-rule-with-attached-remediation-rejected.

Cause 5 of 5 — DeleteEvaluationResults is ACCEPTED for a service-linked (conformance-pack deployed) Config rule, where AWS denies it.
AWS: recorded behavior only — the Config Smithy model declares no AccessDeniedException shape and DeleteEvaluationResults models only NoSuchConfigRuleException and ResourceInUseException (Smithy: DeleteEvaluationResults.errors). The recording shows that for a rule deployed by a conformance pack (a rule an AWS service owns), DeleteEvaluationResults is refused with the error code AccessDeniedException whose message starts with the fixed text "An AWS service owns " and ends with the fixed text ". You do not have permissions to take action on this rule."; the owner named between them is an AWS-side identifier (a per-environment value, not part of the template).
Emulator: the call SUCCEEDS (2xx, no error), so a caller can clear the evaluation results of a rule it does not own.
Why: the ownership gate is invisible — a client's permission-probe succeeds where AWS denies it, and worse, the stored external evaluation can be destroyed by a call AWS never performs, so state diverges after the call too.
Fix: DeleteEvaluationResults' handler — gate on the rule's owner (service-linked / conformance-pack-created rules) and refuse with AccessDeniedException and AWS's sentence before touching evaluation state.
Traces: config.e2e-external-evaluation-persists-when-delete-evaluation-results-denied.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator built from this checkout (115/177 passed; the five checks below failed and are not marked ALREADY FILED or OWNED).

Cause 1 — config.configuration-recorder-lifecycle [mismatch]:
  PutConfigurationRecorder with recordingGroup {"allSupported": true, "resourceTypes": ["AWS::EC2::Instance"]}
  code: InvalidRecordingGroupException (matches)
  message: expected 'The recording group provided is not valid'
           got      'RecordingGroup.allSupported is true, so RecordingGroup.resourceTypes must be empty'
  (assertion: check_config_configuration_recorder_lifecycle.py line 376/6042 `assert message == "The recording group provided is not valid"`)

Cause 2 — config.delete-service-linked-recorder-conflict-while-active-blocked [mismatch]:
  StopConfigurationRecorder on an active service-linked recorder (ServicePrincipal observabilityadmin.amazonaws.com)
  code: UnmodifiableEntityException (matches)
  message: expected "StopConfigurationRecorder operation does not apply to a service-linked configuration recorder <recorderName> because it's always recording"
           got      'This operation does not affect service-linked configuration recorders. Service-linked configuration recorders are always recording. To stop recording, you must delete the service-linked configuration recorder. Use the DeleteServiceLinkedConfigurationRecorder operation to delete a service-linked configuration recorder.'
  (<recorderName> in the recorded run was AWSConfigurationRecorderForObservabilityadmin — a value from state, not part of the requirement)

Cause 3 — config.delete-service-linked-recorder-while-recording-conflict-blocked [mismatch]:
  DeleteConfigurationRecorder on a recording service-linked recorder
  code: UnmodifiableEntityException (matches)
  message: expected 'DeleteConfigurationRecorder operation does not apply to a service-linked configuration recorder <recorderName>. The request was rejected because service-linked configuration recorders are protected AWS resources. Please use DeleteServiceLinkedConfigurationRecorder API to attempt the deletion.'
           got      'You cannot use this operation to delete a service-linked configuration recorder. Use the DeleteServiceLinkedConfigurationRecorder operation to delete a service-linked configuration recorder.'

Cause 4 — config.delete-config-rule-with-attached-remediation-rejected [mismatch]:
  DeleteConfigRule while a RemediationConfiguration is attached
  code: ResourceInUseException (matches)
  message: expected 'The rule <configRuleName> currently has a RemediationConfiguration. Please delete the RemediationConfiguration and try again.'
           got      "A remediation action is associated with the Config rule '<configRuleName>' and Config cannot delete this rule. Delete the remediation action associated with the rule before deleting the rule and try your request again later."
  (the recorded run's rule name sim2real-inuse-rule-bd54c889eacd is a per-run value)

Cause 5 — config.e2e-external-evaluation-persists-when-delete-evaluation-results-denied [mismatch]:
  DeleteEvaluationResults(ConfigRuleName=<conformance-pack process-check rule>)
  expected: 400 AccessDeniedException, message startswith 'An AWS service owns ' and endswith '. You do not have permissions to take action on this rule.'
  got:      the call SUCCEEDED (harness: "expected the call to be rejected, but it succeeded", check line 53/101)

Model grounding (services/service-com-amazonaws-configservice-starlingdoveservice/smithy-model/com-amazonaws-configservice-starlingdoveservice.json): PutConfigurationRecorder models InvalidRecordingGroupException (400); StopConfigurationRecorder, StartConfigurationRecorder and DeleteConfigurationRecorder each model UnmodifiableEntityException (400); DeleteConfigRule models ResourceInUseException (400); DeleteEvaluationResults models only NoSuchConfigRuleException and ResourceInUseException (the service declares no AccessDeniedException shape, so cause 5's code/message is grounded in the recording).

## Steps to Reproduce
1. git clone the repo; run the behavioral suite (simparity-behavioral-aws-v1) against the emulator built from this checkout.
2. Cause 1: replay config.configuration-recorder-lifecycle — PutConfigurationRecorder with recordingGroup allSupported=true AND a non-empty resourceTypes list; compare the InvalidRecordingGroupException message to 'The recording group provided is not valid'.
3. Cause 2: replay config.delete-service-linked-recorder-conflict-while-active-blocked — PutServiceLinkedConfigurationRecorder(ServicePrincipal='observabilityadmin.amazonaws.com'), then StopConfigurationRecorder on the returned recorder name; compare the UnmodifiableEntityException message to the '<Operation> operation does not apply to a service-linked configuration recorder <name> because it's always recording' template (and the same for StartConfigurationRecorder).
4. Cause 3: replay config.delete-service-linked-recorder-while-recording-conflict-blocked — DeleteConfigurationRecorder on that same service-linked recorder; compare the message to AWS's 'protected AWS resources ... Please use DeleteServiceLinkedConfigurationRecorder API' template.
5. Cause 4: replay config.delete-config-rule-with-attached-remediation-rejected — PutConfigRule, PutRemediationConfigurations for it, then DeleteConfigRule; compare the ResourceInUseException message to 'The rule <name> currently has a RemediationConfiguration. Please delete the RemediationConfiguration and try again.'
6. Cause 5: replay config.e2e-external-evaluation-persists-when-delete-evaluation-results-denied — deploy a process-check rule through a conformance pack, PutExternalEvaluation for it, then DeleteEvaluationResults on that rule; it must be refused with AccessDeniedException ('An AWS service owns ... . You do not have permissions to take action on this rule.') and the injected evaluation must survive.
7. Re-replay all five check ids listed in trace_ids to confirm the fix.
