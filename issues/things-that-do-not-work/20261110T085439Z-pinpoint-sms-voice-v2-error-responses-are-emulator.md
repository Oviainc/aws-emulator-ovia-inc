Issue-Source: trace-sweep-divergence
Issue-Trace: pinpoint-sms-voice-v2-configuration-set-client-token-idempotency
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: pinpoint-sms-voice-v2-configuration-set-client-token-idempotency pinpoint-sms-voice-v2-configuration-set-lifecycle pinpoint-sms-voice-v2-configuration-set-lifecycle-and-conflict pinpoint-sms-voice-v2-opt-out-list-and-number-lifecycle pinpoint-sms-voice-v2-opt-out-list-lifecycle
Issue-Shape-Id: com.amazonaws.pinpointsmsvoicev2#PinpointSMSVoiceV2
Issue-Key: pinpoint-sms-voice-v2|CreateConfigurationSet|4xx->4xx|$.Message
Issue-Key: pinpoint-sms-voice-v2|CreateConfigurationSet|4xx->4xx|$.__type
Issue-Key: pinpoint-sms-voice-v2|DeleteConfigurationSet|4xx->4xx|$.Message
Issue-Key: pinpoint-sms-voice-v2|CreateConfigurationSet|4xx->4xx|$.Fields
Issue-Key: pinpoint-sms-voice-v2|CreateConfigurationSet|4xx->4xx|$.Reason
Issue-Key: pinpoint-sms-voice-v2|PutOptedOutNumber|4xx->4xx|$.Fields
Issue-Key: pinpoint-sms-voice-v2|PutOptedOutNumber|4xx->4xx|$.Message
Issue-Key: pinpoint-sms-voice-v2|PutOptedOutNumber|4xx->4xx|$.Reason
Issue-Commit: a11b28e572acb01424c781dc62b11ca84bce3a9b

# pinpoint-sms-voice-v2 error responses are emulator-authored, not the service's exception type and Message template

> _Note: this issue was identified against the code base at commit `a11b28e572acb01424c781dc62b11ca84bce3a9b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateConfigurationSet

## Description
This bundle carries 5 causes. Every one of them is an error response whose HTTP status is right and whose body is not what AWS End User Messaging SMS and Voice (sms-voice, awsJson1_0) serves: one returns the wrong exception shape entirely, three carry emulator prose where the service renders a fixed template, and two are refused by the framework's synthetic error shape instead of the service's modeled ValidationException. A fixer must close all 5 before submitting.

Cause 1 of 5 — ClientToken replayed with a DIFFERENT request is a ConflictException, not a ValidationException (CreateConfigurationSet).
AWS: Smithy (services/service-com-amazonaws-pinpointsmsvoicev2-pinpointsmsvoicev2/smithy-model/...json) models CreateConfigurationSet.ClientToken as @idempotencyToken (len 1..64, pattern ^[!-~]+$) and declares both ConflictException and ValidationException for the operation, each HTTP 400. The recorded response for a token reused with a different ConfigurationSetName is error ConflictException (recorded __type 'com.amazonaws.smsvoice.exceptions#ConflictException'), HTTP 400, Message 'Token used for different request' — that whole message is fixed text with no variable part. Hypothesis: the service classifies a token/request mismatch as a conflict on the token resource rather than a field validation failure.
Emulator: HTTP 400 with __type 'ValidationException' and Message 'ClientToken was already used for a different CreateConfigurationSet request', plus Reason 'INVALID_PARAMETER' and a Fields entry for ClientToken that AWS does not send.
Why: an SDK caller branches on the error code. Code that catches ConflictException around an idempotent create (retry with a fresh token, surface a conflict to the user) never fires, and the ValidationException it does see is classified as a non-retryable bad request, so a correct client takes the wrong branch on a case AWS defines.
Fix: the ClientToken-mismatch arm of CreateConfigurationSet's handler must return the modeled ConflictException with the service's message, not ValidationException.
Traces: pinpoint-sms-voice-v2-configuration-set-client-token-idempotency#task#3 (handler: services/service-com-amazonaws-pinpointsmsvoicev2-pinpointsmsvoicev2/src/ops/create_configuration_set.rs:193-210).

Cause 2 of 5 — duplicate-name ConflictException Message is prose, not the 'Conflict Occurred - ...' template (CreateConfigurationSet).
AWS: Smithy ConflictException carries Message, Reason (ConflictExceptionReason, which includes RESOURCE_ALREADY_EXISTS), ResourceType and ResourceId; errorKind client, HTTP 400. The recording shows Message rendered from those members: the fixed text 'Conflict Occurred - Reason="<Reason>" ResourceType="<ResourceType>" ResourceId="<ResourceId>"', where the literal prefix and the three quoted clause names are fixed and the three values come from the error itself — Reason RESOURCE_ALREADY_EXISTS for a duplicate create, ResourceType 'configuration-set', ResourceId the ConfigurationSetName from the request (one recording's value was 'sim2real-probe-pinpointsmsvoicev2-cfgset-e519626a'; that name is a per-run value, not part of the requirement).
Emulator: code ConflictException and HTTP 400 are right; Message is 'configuration set <ConfigurationSetName> already exists'.
Why: a client that logs, asserts on or surfaces Message — or parses the Reason clause out of it — sees text AWS never emits, while the structured members are correct, so the divergence is invisible in the handler and only shows up at the client.
Fix: this operation's ConflictException construction (and the shared error-rendering helper it should use, so every ConflictException in the service renders one template) must build Message from Reason/ResourceType/ResourceId.
Related: the open behavioral issue 20261109T164730Z-pinpoint-sms-voice-v2-createconfigurationset-s-duplicate-name.md describes this same root cause but carries no wire replay key (the replay reported owned_by empty for this cluster), so this cause is filed here with the fixture binding that exercises it.
Traces: pinpoint-sms-voice-v2-configuration-set-lifecycle#task#2 (also pinpoint-sms-voice-v2-configuration-set-lifecycle-and-conflict#task#2); handler create_configuration_set.rs:217-227.

Cause 3 of 5 — ResourceNotFoundException Message is prose, not the 'Resource Not Found - ...' template (DeleteConfigurationSet).
AWS: Smithy ResourceNotFoundException carries Message, ResourceType and ResourceId; errorKind client, HTTP 400; DeleteConfigurationSet declares it. The recording renders Message as the fixed text 'Resource Not Found - ResourceType="<ResourceType>" ResourceId="<ResourceId>"' with ResourceType 'configuration-set' and ResourceId the requested name taken from the request. This is the same rendering convention as Cause 2, so it is the service's convention and not one recording's text.
Emulator: code ResourceNotFoundException and HTTP 400 are right; Message is 'configuration set <name> not found'.
Why: same as Cause 2 — the message a client reads or matches on is text AWS does not produce.
Fix: the ResourceNotFoundException construction in DeleteConfigurationSet's handler, via the same shared renderer.
Traces: pinpoint-sms-voice-v2-configuration-set-lifecycle#task#4 (also pinpoint-sms-voice-v2-configuration-set-lifecycle-and-conflict#task#9); handler services/service-com-amazonaws-pinpointsmsvoicev2-pinpointsmsvoicev2/src/ops/delete_configuration_set.rs:178-187.

Cause 4 of 5 — a modeled-constraint violation is refused with the framework's synthetic ValidationException (lowercase 'message', no Reason, no Fields) instead of the service's modeled one (CreateConfigurationSet.ConfigurationSetName).
AWS: Smithy CreateConfigurationSetRequest.ConfigurationSetName is @required, len 1..64, pattern ^[A-Za-z0-9_-]+$, and the operation declares ValidationException (HTTP 400) whose members are Message, Reason (ValidationExceptionReason, which includes FIELD_VALIDATION_FAILED) and Fields (list of ValidationExceptionField{Name, Message}, both @required). The recording shows, for a name that violates the pattern: HTTP 400, error ValidationException, Reason 'FIELD_VALIDATION_FAILED', Message "The following parameter is null, empty or invalid: ['configurationSetName']" and Fields [{Name: 'configurationSetName', Message: 'Member must satisfy regular expression pattern: [A-Za-z0-9_-]+'}]. Fixed text: 'The following parameter is null, empty or invalid: ' followed by the bracketed, single-quoted list of offending member names, and 'Member must satisfy regular expression pattern: ' followed by the member's modeled pattern with the ^ and $ anchors stripped. Variable parts: the member's wire name (lowerCamel, here configurationSetName) and its pattern — both taken from the request's offending member, not from any one recording.
Emulator: HTTP 400, but the body has none of those keys — $.Message, $.Reason and $.Fields are all absent. The boundary validator refuses the request with the synthetic smithy.framework#ValidationException (services/service-com-amazonaws-pinpointsmsvoicev2-pinpointsmsvoicev2/src/scaffolded.rs:117-178), whose only member is serialized as lowercase 'message'; on awsJson1_0 the service's modeled member names are Message/Reason/Fields, so a client reading the error gets no message at all and no field list.
Why: every AWS SDK surfaces Message for a 400, and callers that inspect Reason or Fields to point a user at the bad input get nothing. The failure is also indistinguishable from any other constraint violation, so a caller cannot tell which member was wrong.
Fix: the service's boundary/constraint-validation error path must render this service's modeled ValidationException — Message from the offending member list, Reason FIELD_VALIDATION_FAILED, and one Fields entry per violated member — instead of the framework's synthetic single-member shape.
Traces: pinpoint-sms-voice-v2-configuration-set-lifecycle-and-conflict#task#3.

Cause 5 of 5 — the same synthetic-ValidationException refusal for PutOptedOutNumber.OptedOutNumber.
AWS: Smithy PutOptedOutNumberRequest.OptedOutNumber is @required, target PhoneNumber, len 1..20, pattern ^\+?[1-9][0-9]{1,18}$; the operation declares ValidationException (HTTP 400). The recording shows HTTP 400, Reason 'FIELD_VALIDATION_FAILED', Message "The following parameter is null, empty or invalid: ['optedOutNumber']" and Fields [{Name: 'optedOutNumber', Message: 'Member must satisfy regular expression pattern: \+?[1-9][0-9]{1,18}'}] — same template as Cause 4, with this member's wire name and anchor-stripped pattern.
Emulator: HTTP 400 with $.Message, $.Reason and $.Fields all absent, from the same synthetic shape. (PutOptedOutNumber's handler itself is still a scaffold stub — src/ops/put_opted_out_number.rs:176 — but this step never reaches it: the refusal happens at the boundary, so this is the validation renderer's defect and is not covered by that operation's stub issue.)
Why: as Cause 4 — a bad phone number is rejected with an empty-bodied 400 instead of a message naming optedOutNumber and the pattern it must satisfy.
Fix: same shared validation renderer as Cause 4; both causes close together once constraint violations are rendered as the modeled ValidationException.
Traces: pinpoint-sms-voice-v2-opt-out-list-and-number-lifecycle#task#4 (also pinpoint-sms-voice-v2-opt-out-list-lifecycle#task#3).

## Evidence
Cause 1 — fixture pinpoint-sms-voice-v2-configuration-set-client-token-idempotency step task#3, CreateConfigurationSet. Expected (recorded AWS): HTTP 400, $.__type 'com.amazonaws.smsvoice.exceptions#ConflictException', $.Message 'Token used for different request'. Actual (emulator): HTTP 400, $.__type 'ValidationException', $.Message 'ClientToken was already used for a different CreateConfigurationSet request'.

Cause 2 — fixture pinpoint-sms-voice-v2-configuration-set-lifecycle step task#2, CreateConfigurationSet. Expected: HTTP 400, $.Message 'Conflict Occurred - Reason="RESOURCE_ALREADY_EXISTS" ResourceType="configuration-set" ResourceId="<ConfigurationSetName>"'. Actual: HTTP 400, $.Message 'configuration set <ConfigurationSetName> already exists'. (Recorded run used the generated name sim2real-probe-pinpointsmsvoicev2-cfgset-e519626a; the name is a per-run value, the surrounding text is the requirement.) Cluster also covers pinpoint-sms-voice-v2-configuration-set-lifecycle-and-conflict#task#2.

Cause 3 — fixture pinpoint-sms-voice-v2-configuration-set-lifecycle step task#4, DeleteConfigurationSet. Expected: HTTP 400, $.Message 'Resource Not Found - ResourceType="configuration-set" ResourceId="<name>"'. Actual: HTTP 400, $.Message 'configuration set <name> not found'. Cluster also covers pinpoint-sms-voice-v2-configuration-set-lifecycle-and-conflict#task#9.

Cause 4 — fixture pinpoint-sms-voice-v2-configuration-set-lifecycle-and-conflict step task#3, CreateConfigurationSet. Expected: HTTP 400 with $.Reason 'FIELD_VALIDATION_FAILED', $.Message "The following parameter is null, empty or invalid: ['configurationSetName']", $.Fields [{'Name': 'configurationSetName', 'Message': 'Member must satisfy regular expression pattern: [A-Za-z0-9_-]+'}]. Actual: HTTP 400 with $.Reason, $.Message and $.Fields all missing (None).

Cause 5 — fixture pinpoint-sms-voice-v2-opt-out-list-and-number-lifecycle step task#4, PutOptedOutNumber. Expected: HTTP 400 with $.Reason 'FIELD_VALIDATION_FAILED', $.Message "The following parameter is null, empty or invalid: ['optedOutNumber']", $.Fields [{'Name': 'optedOutNumber', 'Message': 'Member must satisfy regular expression pattern: \\+?[1-9][0-9]{1,18}'}]. Actual: HTTP 400 with all three missing (None). Cluster also covers pinpoint-sms-voice-v2-opt-out-list-lifecycle#task#3.

Code: src/ops/create_configuration_set.rs:196-210 (Cause 1), :217-227 (Cause 2); src/ops/delete_configuration_set.rs:178-187 (Cause 3); src/scaffolded.rs:117-178 and :261-279 — the synthetic ValidationException with a single lowercase 'message' member (Causes 4 and 5). All paths under services/service-com-amazonaws-pinpointsmsvoicev2-pinpointsmsvoicev2/.

## Steps to Reproduce
Boot the emulator and point an sms-voice (pinpoint-sms-voice-v2) client at it.

Cause 1: create_configuration_set(ConfigurationSetName='setA', ClientToken='tok-1') -> 200. Then create_configuration_set(ConfigurationSetName='setB', ClientToken='tok-1'). Observe __type ValidationException instead of ConflictException and the emulator-authored message instead of 'Token used for different request'.

Cause 2: create_configuration_set(ConfigurationSetName='dupe') twice. Read the second error's Message: prose instead of 'Conflict Occurred - Reason="RESOURCE_ALREADY_EXISTS" ResourceType="configuration-set" ResourceId="dupe"'.

Cause 3: delete_configuration_set(ConfigurationSetName='never-created'). Read Message: prose instead of 'Resource Not Found - ResourceType="configuration-set" ResourceId="never-created"'.

Cause 4: POST awsJson1_0 CreateConfigurationSet with ConfigurationSetName containing a character outside [A-Za-z0-9_-] (e.g. 'bad name!'). Observe a 400 whose JSON body has no Message, Reason or Fields key.

Cause 5: POST awsJson1_0 PutOptedOutNumber with OptOutListName of an existing list and OptedOutNumber='not-a-number'. Observe the same empty-bodied 400.

Re-replay: pinpoint-sms-voice-v2-configuration-set-client-token-idempotency#task#3, pinpoint-sms-voice-v2-configuration-set-lifecycle#task#2, pinpoint-sms-voice-v2-configuration-set-lifecycle#task#4, pinpoint-sms-voice-v2-configuration-set-lifecycle-and-conflict#task#3, pinpoint-sms-voice-v2-opt-out-list-and-number-lifecycle#task#4.
