Issue-Source: trace-sweep-divergence
Issue-Trace: iot-delete-v2-logging-level-invalid-target-type
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iot-delete-v2-logging-level-invalid-target-type iot-domain-configuration-error-paths iot-scheduled-audit-invalid-daymonth-error iot-vpce-traj-fix-endpoint-via-policy-version iot-vpce-traj-policy-document-refusals iot-command-execution-errors iot-ota-update-lifecycle iot-topic-rule-create-get-delete iot-topic-rule-disabled-flag iot-topic-rule-get-nonexistent-error iot-topic-rule-lifecycle iot-topic-rule-list-and-describe-roundtrip iot-topic-rule-multi-action-with-error-action
Issue-Shape-Id: com.amazonaws.iot#AWSIotService
Issue-Key: iot|DeleteV2LoggingLevel|4xx->4xx|$.message
Issue-Key: iot|DeleteV2LoggingLevel|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: iot|CreateDomainConfiguration|4xx->4xx|$.message
Issue-Key: iot|CreateDomainConfiguration|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: iot|CreateScheduledAudit|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: iot|SetDefaultPolicyVersion|4xx->4xx|$.message
Issue-Key: iot|SetDefaultPolicyVersion|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: iot|CreatePolicy|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: iot|DeleteCommandExecution|4xx->4xx|$.message
Issue-Key: iot|CreateOTAUpdate|4xx->4xx|$.message
Issue-Key: iot|GetTopicRule|4xx->4xx|$.message
Issue-Related: 20261107T022027Z-iot-already-exists-conflicts-9-causes-reworded-conflictexception.md
Issue-Commit: ff097a89ab5b098f807ae300b709617042ce117b

# iot refusal envelopes: 8 causes — wrong x-amzn-errortype class and reworded validation messages

> _Note: this issue was identified against the code base at commit `ff097a89ab5b098f807ae300b709617042ce117b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteV2LoggingLevel

## Description
The emulator routes IoT request-validation refusals through a generic Smithy constraint validator that emits x-amzn-errortype 'ValidationException' and its own message layout, while the legacy iot exception family AWS uses for these operations is InvalidRequestException with the IoT doc-namespace suffix on the errortype header. The messages also diverge systematically: the emulator quotes the rejected VALUE where AWS omits it, anchors the reported regex with ^...$ where AWS reports the bare pattern, and lists enum members in its own order rather than AWS's. 8 net-new causes, all in service iot.

Cause 1 of 8: DeleteV2LoggingLevel with an out-of-range targetType reports the enum value set in the wrong order and the wrong errortype.
AWS: recorded behavior: 400 whose message is AWS's standard constraint sentence — '1 validation error detected: Value ' + "'" + <rejected value> + "'" + " at 'targetType' failed to satisfy constraint: Member must satisfy enum value set: [" + <the enum members in the model's declaration order> + ']'. For LogTargetType that order is DEFAULT, SOURCE_IP, DEVICE_DEFENDER, PRINCIPAL_ID, THING_GROUP, CLIENT_ID. The errortype header is not asserted by this fixture but the class is the legacy iot one.
Emulator: same sentence but the member list is reordered to [DEFAULT, THING_GROUP, CLIENT_ID, SOURCE_IP, PRINCIPAL_ID] — five members instead of six, DEVICE_DEFENDER dropped — and x-amzn-errortype is 'ValidationException'.
Why: the enumerated member list is the actionable part of this message: a caller reading it is told DEVICE_DEFENDER is not a legal targetType when it is, and the ordering difference breaks any exact-match assertion.
Fix: the shared constraint-validation message builder must render enum members in the model's declaration order and must not drop members; the errortype serializer must emit the operation's modelled error class.
Traces: iot-delete-v2-logging-level-invalid-target-type#task#1.

Cause 2 of 8: CreateDomainConfiguration pattern refusal quotes the rejected value and the anchored regex, and uses the wrong error class.
AWS: Smithy: domainConfigurationName carries @pattern [\w.-]+. Recorded behavior: 400 with x-amzn-errortype 'InvalidRequestException:http://iot.amazonaws.com/doc/2015-10-02/' and message "1 validation error detected: Value at 'domainConfigurationName' failed to satisfy constraint: Member must satisfy regular expression pattern: [\w.-]+" — AWS omits the rejected value entirely for this member and reports the pattern WITHOUT ^ and $ anchors.
Emulator: 400 with x-amzn-errortype 'ValidationException' and message "1 validation error detected: Value 'IoT:sim2real-reserved' at 'domainConfigurationName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[\w.-]+$" — it interpolates the caller's value and the anchored pattern.
Why: wrong errortype means the SDK raises the wrong exception class, so a catch on InvalidRequestException does not fire; and echoing the caller's value into the message is an information-disclosure difference as well as a text mismatch.
Fix: the CreateDomainConfiguration validation path and the shared pattern-refusal message builder (omit the value, strip the anchors) plus the errortype serializer.
Traces: iot-domain-configuration-error-paths#task#4.

Cause 3 of 8: CreateScheduledAudit refusal carries the wrong x-amzn-errortype.
AWS: recorded behavior: 400 with x-amzn-errortype 'InvalidRequestException:http://iot.amazonaws.com/doc/2015-10-02/'. The message matched.
Emulator: the same 400 and the same message but x-amzn-errortype 'ValidationException'.
Why: this cause isolates the header defect from the message defect — the body is already right, so the only thing breaking a client is the exception class the SDK resolves from the header.
Fix: the errortype serializer for the legacy iot exception family — emit the modelled shape name plus the IoT doc namespace suffix.
Traces: iot-scheduled-audit-invalid-daymonth-error#task#1.

Cause 4 of 8: SetDefaultPolicyVersion pattern refusal quotes the value and the anchored regex.
AWS: Smithy: SetDefaultPolicyVersion.policyVersionId is an httpLabel with @pattern ^[0-9]+$ in the model; the recorded refusal reports the pattern as the UNANCHORED '[0-9]+' and omits the rejected value: "1 validation error detected: Value at 'policyVersionId' failed to satisfy constraint: Member must satisfy regular expression pattern: [0-9]+".
Emulator: "1 validation error detected: Value 'v2' at 'policyVersionId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9]+$", with x-amzn-errortype 'ValidationException'.
Why: same two message defects (value echoed, pattern anchored) on a second operation, confirming the builder and not one handler is at fault.
Fix: the shared pattern-refusal message builder.
Traces: iot-vpce-traj-fix-endpoint-via-policy-version#task#8.

Cause 5 of 8: CreatePolicy missing-required refusal interpolates the literal 'null' as the value.
AWS: Smithy: CreatePolicy.policyDocument is @required. Recorded behavior: 400 with message "1 validation error detected: Value at 'policyDocument' failed to satisfy constraint: Member must not be null" — no value is rendered between 'Value' and 'at'.
Emulator: "1 validation error detected: Value null at 'policyDocument' failed to satisfy constraint: Member must not be null" — it renders the word 'null' as if it were the caller's value — with x-amzn-errortype 'ValidationException'.
Why: the required-member refusal is the most commonly asserted validation message in client test suites; the stray 'null' token makes every exact match fail.
Fix: the shared required-member refusal message builder — render no value segment when the member is absent.
Traces: iot-vpce-traj-policy-document-refusals#task#7 (also iot-vpce-traj-policy-document-refusals#task#8).

Cause 6 of 8: DeleteCommandExecution rejects a malformed targetArn with the emulator's own prose.
AWS: Smithy: DeleteCommandExecution.targetArn is @required, httpQuery 'targetArn', @length(0..2048), and the operation's modelled client errors are ValidationException(400) and ConflictException(409). Recorded behavior: 400 whose message is the fixed sentence 'Invalid targetArn' with NO variable part — the offending ARN is not echoed.
Emulator: 400 'Invalid target arn not-an-arn not supported as a target' — a different sentence that also interpolates the caller's string.
Why: AWS's message for this refusal is a constant; the emulator's is neither matchable nor minimal.
Fix: the DeleteCommandExecution handler's targetArn validation.
Traces: iot-command-execution-errors#task#1.

Cause 7 of 8: CreateOTAUpdate rejects a malformed target ARN with the emulator's own prose.
AWS: Smithy: CreateOTAUpdate.targets is @required with @length(min 1); modelled client errors include InvalidRequestException(400). Recorded behavior: 400 whose message is the fixed sentence 'Target ARN not in proper syntax.' (terminating period, no variable part).
Emulator: 400 '<the caller's string> is not an IoT thing or thing group ARN'.
Why: same pattern — a constant AWS sentence replaced by an interpolated emulator sentence.
Fix: the CreateOTAUpdate handler's target validation.
Traces: iot-ota-update-lifecycle#task#2.

Cause 8 of 8: GetTopicRule's authorization refusal uses an IAM-style message instead of AWS's topic-rule wording.
AWS: Smithy: GetTopicRule errors include UnauthorizedException (httpError 401). Recorded behavior: 401 whose message is "Access to topic rule '" + <ruleName> + "' was denied" — fixed text 'Access to topic rule ', the single quotes, and ' was denied'; the only variable part is the rule name from the URI label.
Emulator: 401 'Not authorized to perform: iot:GetTopicRule on resource: arn:aws:iot:<region>:<account>:rule/<ruleName>' — an IAM-style sentence naming the action and the full ARN.
Why: the status is right but the body is a different contract; a client branching on AWS's denial text, or displaying it, gets a sentence AWS never emits, and the emulator leaks the resolved ARN.
Fix: the GetTopicRule handler's unauthorized path (its UnauthorizedException message construction), not the shared IAM denial formatter.
Traces: iot-topic-rule-create-get-delete#teardown#4 (also iot-topic-rule-disabled-flag#teardown#4, iot-topic-rule-get-nonexistent-error#task#1, iot-topic-rule-lifecycle#teardown#5, iot-topic-rule-list-and-describe-roundtrip#teardown#5, iot-topic-rule-multi-action-with-error-action#teardown#4).

Related open issue that does NOT cover these: '20261106T201842Z-iot-refusal-fidelity-10-net-new-causes-createpolicy.md' is a behavioral finding and carries no replay key for any of the eight fixture steps above.

## Evidence
Replay of trace set simparity-wire-aws-v1. Expected = recorded AWS response, Actual = emulator.

1. DeleteV2LoggingLevel, iot-delete-v2-logging-level-invalid-target-type#task#1 — status 400 both; $.message expected "1 validation error detected: Value 'NOT_A_REAL_TYPE' at 'targetType' failed to satisfy constraint: Member must satisfy enum value set: [DEFAULT, SOURCE_IP, DEVICE_DEFENDER, PRINCIPAL_ID, THING_GROUP, CLIENT_ID]", got "... enum value set: [DEFAULT, THING_GROUP, CLIENT_ID, SOURCE_IP, PRINCIPAL_ID]"; $headers.x-amzn-errortype got 'ValidationException' where the fixture asserts the shaped legacy class.
2. CreateDomainConfiguration, iot-domain-configuration-error-paths#task#4 — $.message expected "1 validation error detected: Value at 'domainConfigurationName' failed to satisfy constraint: Member must satisfy regular expression pattern: [\\w.-]+", got "1 validation error detected: Value 'IoT:sim2real-reserved' at 'domainConfigurationName' ... pattern: ^[\\w.-]+$"; errortype expected 'InvalidRequestException:http://iot.amazonaws.com/doc/2015-10-02/', got 'ValidationException'.
3. CreateScheduledAudit, iot-scheduled-audit-invalid-daymonth-error#task#1 — message matched; errortype expected 'InvalidRequestException:http://iot.amazonaws.com/doc/2015-10-02/', got 'ValidationException'.
4. SetDefaultPolicyVersion, iot-vpce-traj-fix-endpoint-via-policy-version#task#8 — $.message expected "... Value at 'policyVersionId' ... pattern: [0-9]+", got "... Value 'v2' at 'policyVersionId' ... pattern: ^[0-9]+$"; errortype got 'ValidationException'.
5. CreatePolicy, iot-vpce-traj-policy-document-refusals#task#7 — $.message expected "1 validation error detected: Value at 'policyDocument' failed to satisfy constraint: Member must not be null", got "1 validation error detected: Value null at 'policyDocument' ..."; errortype got 'ValidationException'. Cluster of 2.
6. DeleteCommandExecution, iot-command-execution-errors#task#1 — $.message expected 'Invalid targetArn', got 'Invalid target arn not-an-arn not supported as a target'.
7. CreateOTAUpdate, iot-ota-update-lifecycle#task#2 — $.message expected 'Target ARN not in proper syntax.', got 'not-a-valid-thing-arn-but-long-enough-to-pass-length-check is not an IoT thing or thing group ARN'.
8. GetTopicRule, iot-topic-rule-create-get-delete#teardown#4 — status 401 both; $.message expected "Access to topic rule 'sim2real_probe_iot_topicrule_2ae9385a' was denied", got 'Not authorized to perform: iot:GetTopicRule on resource: arn:aws:iot:us-west-2:000000000000:rule/sim2real_probe_iot_topicrule_2ae9385a'. Cluster of 6.

(Rejected values, rule names and ARNs above are this recording's values; the requirements are the fixed texts, the member/pattern rendering rules and the errortype class described per cause.)

## Steps to Reproduce
1. git clone the repo and boot the emulator from this checkout.
2. Replay the simparity-wire-aws-v1 wire corpus.
3. Observe the eight clusters listed under Evidence, starting with fixture iot-delete-v2-logging-level-invalid-target-type step task#1.
4. Equivalently by hand: DELETE /v2LoggingLevel?targetType=NOT_A_REAL_TYPE&targetName=x; POST /domainConfigurations/IoT:reserved; POST /audit/scheduledaudits/<n> with an out-of-range dayOfMonth; PATCH /policies/<p>/version/v2; POST /policies/<p> with no policyDocument; DELETE /command-executions/<e>?targetArn=not-an-arn; POST /otaUpdates/<o> with a non-ARN target; GET /rules/<r> with a principal lacking iot:GetTopicRule. Compare each response's x-amzn-errortype header and message against the AWS form given per cause.
