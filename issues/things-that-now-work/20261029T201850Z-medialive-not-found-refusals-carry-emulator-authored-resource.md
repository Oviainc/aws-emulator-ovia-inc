Issue-Source: trace-sweep-divergence
Issue-Trace: medialive-input-security-group-error-paths
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: medialive-input-security-group-error-paths medialive-sdi-source-error-paths medialive-multiplex-and-program-error-paths medialive-cloudwatch-alarm-template-group-lifecycle
Issue-Shape-Id: com.amazonaws.medialive#MediaLive
Issue-Key: medialive|DescribeInputSecurityGroup|4xx->4xx|$.message
Issue-Key: medialive|UpdateInputSecurityGroup|4xx->4xx|$.message
Issue-Key: medialive|DeleteInputSecurityGroup|4xx->4xx|$.message
Issue-Key: medialive|DescribeSdiSource|4xx->4xx|$.message
Issue-Key: medialive|UpdateSdiSource|4xx->4xx|$.message
Issue-Key: medialive|DeleteSdiSource|4xx->4xx|$.message
Issue-Key: medialive|DescribeMultiplexProgram|4xx->4xx|$.message
Issue-Key: medialive|GetCloudWatchAlarmTemplateGroup|4xx->4xx|$.message
Issue-Commit: 0d251cb15d5c6a3e8a766cb6c72620a30f43ef7f

# medialive not-found refusals carry emulator-authored resource labels, name the wrong resource, and add a message AWS omits

> _Note: this issue was identified against the code base at commit `0d251cb15d5c6a3e8a766cb6c72620a30f43ef7f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInputSecurityGroup

## Description
This issue bundles 8 causes. All of them are the same fault seen on eight refusal paths: the emulator's not-found refusals are worded by a generic, emulator-local resource-label helper rather than by MediaLive's own per-resource sentence, so the HTTP status and the error class are right while the `message` body member is wrong. The status (404) and the error type (NotFoundException) match on every one of the eight; only the message text (or its presence) diverges. Each cause below must be closed before submitting.

Cause 1 of 8: DescribeInputSecurityGroup, a security-group id the account does not hold.
AWS: Smithy: DescribeInputSecurityGroup -> GET /prod/inputSecurityGroups/{InputSecurityGroupId}, error NotFoundException (client, httpError 404) whose only member is `Message` (jsonName `message`, target __string). The recorded response is 404 NotFoundException with message 'InputSecurityGroup <id> not found'. Fixed text: the literal token `InputSecurityGroup `, then ` not found`; the only variable part is the requested InputSecurityGroupId, taken verbatim from the request path label. AWS uses the shape name `InputSecurityGroup` as the label here, not an English phrase.
Emulator: answers 404 NotFoundException but with message 'Input security group <id> not found' — the label is re-spelled as three lowercase English words.
Why: a client that matches the refusal text (retry classifiers, test assertions, log scrapers that key on the resource label) sees a sentence MediaLive never emits, so a legitimately-missing security group is not recognised as such. The error class is right, so only text-sensitive callers break — but the text is part of the contract.
Fix: the not-found refusal construction for input security groups in the medialive crate's InputSecurityGroup handlers / their shared not-found helper (services/service-com-amazonaws-medialive-medialive): the resource label must be the API's shape spelling, not a prose rendering.
Traces: medialive-input-security-group-error-paths#task#1.

Cause 2 of 8: UpdateInputSecurityGroup, same missing id.
AWS: recorded behavior: 404 NotFoundException, message 'InputSecurityGroup <id> not found' — identical template to Cause 1, same fixed text, same single variable (the path id).
Emulator: 404 NotFoundException with 'Input security group <id> not found'.
Why: same as Cause 1 — the update path's refusal is unrecognisable to a text-matching client.
Fix: same refusal helper, reached from the UpdateInputSecurityGroup handler.
Traces: medialive-input-security-group-error-paths#task#2.

Cause 3 of 8: DeleteInputSecurityGroup, same missing id.
AWS: recorded behavior: 404 NotFoundException, message 'InputSecurityGroup <id> not found'.
Emulator: 404 NotFoundException with 'Input security group <id> not found'.
Why: same as Cause 1, on the delete path.
Fix: same refusal helper, reached from the DeleteInputSecurityGroup handler.
Traces: medialive-input-security-group-error-paths#task#3.

Cause 4 of 8: DescribeSdiSource, an SdiSource id the account does not hold.
AWS: Smithy: DescribeSdiSource -> GET /prod/sdiSources/{SdiSourceId}, error NotFoundException (404) with member `message`. Recorded response: 404 NotFoundException, message 'SDI source <id> not found'. Fixed text: the literal token `SDI source ` and ` not found`; the only variable part is the requested SdiSourceId from the path label. Note this is the OPPOSITE convention to Cause 1: for this resource AWS words the label as the English phrase 'SDI source', not as the shape name `SdiSource`. The two conventions are per-resource and cannot be derived from the shape name, which is exactly what the emulator's single helper assumes.
Emulator: answers 404 NotFoundException but with 'SdiSource <id> not found' — the shape name is emitted verbatim.
Why: a text-matching client sees a label MediaLive never uses for this resource. Because the emulator gets Cause 1 and Cause 4 wrong in opposite directions, no single normalisation rule on the client side can recover either.
Fix: per-resource refusal wording for SdiSource in the medialive crate's SdiSource handlers / the shared not-found helper — the label must be a per-resource constant, not a transformation of the shape name.
Traces: medialive-sdi-source-error-paths#task#1.

Cause 5 of 8: UpdateSdiSource, same missing id.
AWS: recorded behavior: 404 NotFoundException, message 'SDI source <id> not found' — same template as Cause 4.
Emulator: 404 NotFoundException with 'SdiSource <id> not found'.
Why: same as Cause 4, on the update path.
Fix: same per-resource label, reached from the UpdateSdiSource handler.
Traces: medialive-sdi-source-error-paths#task#2.

Cause 6 of 8: DeleteSdiSource, same missing id.
AWS: recorded behavior: 404 NotFoundException, message 'SDI source <id> not found'.
Emulator: 404 NotFoundException with 'SdiSource <id> not found'.
Why: same as Cause 4, on the delete path.
Fix: same per-resource label, reached from the DeleteSdiSource handler.
Traces: medialive-sdi-source-error-paths#task#3.

Cause 7 of 8: DescribeMultiplexProgram names the PARENT resource instead of the program when the multiplex is absent.
AWS: Smithy: DescribeMultiplexProgram -> GET /prod/multiplexes/{MultiplexId}/programs/{ProgramName}, both labels required, error NotFoundException (404). Recorded response: 404 NotFoundException, message 'Multiplex program <multiplexId>/<programName> not found'. Fixed text: the literal token `Multiplex program `, the `/` joining the two identifiers, and ` not found`; the variable parts are the MultiplexId and the ProgramName, both taken from the request path labels. AWS reports the refusal at the granularity of the resource the request named — the program — even when it is the enclosing multiplex that does not exist.
Emulator: answers 404 NotFoundException but with 'Multiplex <multiplexId> not found' — it short-circuits on the parent lookup and reports the multiplex as the missing resource, dropping the ProgramName from the message entirely.
Why: a caller cannot tell from the emulator's refusal whether the multiplex or the program is missing, and the ProgramName it asked about never appears in the response. Code that parses the identifier out of the message to decide what to create next (create the multiplex vs. create the program) takes the wrong branch.
Fix: the DescribeMultiplexProgram handler's lookup/refusal path in the medialive crate: a failed parent-multiplex lookup must still be reported as a multiplex-program not-found naming `<multiplexId>/<programName>`, not re-raised as the parent's own not-found.
Traces: medialive-multiplex-and-program-error-paths#task#6.

Cause 8 of 8: GetCloudWatchAlarmTemplateGroup emits a `message` body member on a 404 where AWS sends none.
AWS: Smithy: GetCloudWatchAlarmTemplateGroup -> GET /prod/cloudwatch-alarm-template-groups/{Identifier}, error NotFoundException (404) whose `message` member is OPTIONAL (not @required). Recorded behavior only: for a template-group identifier that does not exist, AWS answers 404 with no `message` member in the body at all.
Emulator: answers 404 but adds an emulator-authored sentence, 'cloudwatch alarm template group <id> not found' — lowercase prose that is not AWS wording for any MediaLive resource, in a body position AWS leaves absent.
Why: a client that treats presence of `message` as meaningful (surfacing it to a user, or asserting the body shape of a refusal) sees a field AWS does not send, carrying text AWS does not write. The status and error class are correct, so this is purely a response-body shape and wording divergence.
Fix: the monitor / CloudWatch-alarm-template-group not-found refusal construction in the medialive crate: this path must emit the 404 NotFoundException without synthesising a message member.
Traces: medialive-cloudwatch-alarm-template-group-lifecycle#teardown#8.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator (emulator_up=true, bootstrap_signal=false, score 83.2%), 8 mismatch clusters, all verdict=mismatch with expected_status == actual_status:

1. medialive-input-security-group-error-paths#task#1 DescribeInputSecurityGroup, 404->404: $.message value_mismatch (expected 'InputSecurityGroup 888888888 not found', got 'Input security group 888888888 not found').
2. medialive-input-security-group-error-paths#task#2 UpdateInputSecurityGroup, 404->404: same expected/actual pair.
3. medialive-input-security-group-error-paths#task#3 DeleteInputSecurityGroup, 404->404: same expected/actual pair.
4. medialive-sdi-source-error-paths#task#1 DescribeSdiSource, 404->404: $.message value_mismatch (expected 'SDI source 777777777 not found', got 'SdiSource 777777777 not found').
5. medialive-sdi-source-error-paths#task#2 UpdateSdiSource, 404->404: same expected/actual pair.
6. medialive-sdi-source-error-paths#task#3 DeleteSdiSource, 404->404: same expected/actual pair.
7. medialive-multiplex-and-program-error-paths#task#6 DescribeMultiplexProgram, 404->404: $.message value_mismatch (expected 'Multiplex program 9999999/no-such-program not found', got 'Multiplex 9999999 not found') — the ProgramName is dropped and the parent is named instead.
8. medialive-cloudwatch-alarm-template-group-lifecycle#teardown#8 GetCloudWatchAlarmTemplateGroup, 404->404: $.message type_mismatch (expected None, got 'cloudwatch alarm template group 1000000 not found') — the member is absent in the recorded AWS body and present in the emulator's.

The ids 888888888, 777777777, 9999999/no-such-program and 1000000 are the values the fixtures requested; they fill the message template and are not themselves requirements. Smithy grounding: NotFoundException = client error, httpError 404, single optional member Message (jsonName 'message'); DescribeMultiplexProgram takes both MultiplexId and ProgramName as required httpLabels.

## Steps to Reproduce
1. Replay fixture medialive-input-security-group-error-paths against the emulator and inspect steps task#1/#2/#3: each calls Describe/Update/DeleteInputSecurityGroup with an input-security-group id that does not exist; compare the 404 body's $.message to 'InputSecurityGroup <id> not found'.
2. Replay medialive-sdi-source-error-paths steps task#1/#2/#3: each calls Describe/Update/DeleteSdiSource with a non-existent SdiSource id; compare $.message to 'SDI source <id> not found'.
3. Replay medialive-multiplex-and-program-error-paths step task#6: GET /prod/multiplexes/<missing-multiplex-id>/programs/<program-name>; the 404 message must name 'Multiplex program <multiplexId>/<programName>', not the multiplex alone.
4. Replay medialive-cloudwatch-alarm-template-group-lifecycle step teardown#8: GET /prod/cloudwatch-alarm-template-groups/<deleted-identifier>; the 404 body must carry no 'message' member.
5. In each case the status (404) and x-amzn-errortype (NotFoundException) already match; the assertion that fails is on $.message only.
