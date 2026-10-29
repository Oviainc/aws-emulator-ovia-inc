Issue-Source: trace-sweep-divergence
Issue-Trace: globalaccelerator-accelerator-lifecycle
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: globalaccelerator-accelerator-lifecycle globalaccelerator-endpoint-group-lifecycle globalaccelerator-listener-lifecycle
Issue-Shape-Id: com.amazonaws.globalaccelerator#GlobalAccelerator_V20180706
Issue-Key: globalaccelerator|CreateAccelerator|4xx->4xx|$.Message
Issue-Key: globalaccelerator|CreateEndpointGroup|4xx->4xx|$.Message
Issue-Key: globalaccelerator|UpdateEndpointGroup|4xx->4xx|$.Message
Issue-Key: globalaccelerator|DeleteListener|4xx->4xx|$.Message
Issue-Key: globalaccelerator|CreateListener|4xx->4xx|$.Message
Issue-Commit: e47c691b8b899576dd7de95cf7f79b31f1a62071

# globalaccelerator error messages diverge from AWS wording across five validation and conflict paths

> _Note: this issue was identified against the code base at commit `e47c691b8b899576dd7de95cf7f79b31f1a62071`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAccelerator

## Description
Five net-new wire divergences in the globalaccelerator emulator share one root cause: the HTTP status and the error code are right on every one of these paths, but the `Message` member of the error body is emulator-authored prose instead of the message AWS Global Accelerator returns for that error class. Global Accelerator is an awsJson1_1 service whose errors carry a human-readable `Message`; SDK users, CLI output and test assertions surface that string, so a wrong message is a wire defect even when the code matches. The five causes are independent message templates and must all be closed before this issue is submitted.

Cause 1 of 4+1 (Cause 1 of 5): CreateAccelerator — accelerator name longer than the service limit.
AWS: Smithy models CreateAcceleratorRequest.Name as GenericString with @length min 0 max 255 and the operation throws InvalidArgumentException (httpError 400); the model does not state the runtime name limit, so the message is recorded behavior only. The recording shows HTTP 400 with Message exactly 'Accelerator Name should be between 1 - 64 characters in size.' — the whole string is fixed text, with the bounds 1 and 64 being the service's name-length limits and no part taken from the request.
Emulator: returns HTTP 400 with Message 'Name is too long: a name can have a maximum of 64 characters, must contain only alphanumeric characters, periods (.), or hyphens (-), and must not begin or end with a hyphen or period' — a different, longer template (it reads like another service's name-validation text) that also asserts a character-class rule AWS does not mention in this error.
Why: a client asserting on, logging or pattern-matching the accelerator-name validation message gets text that never appears in AWS, and the extra character-class claim misleads a caller into thinking a different rule was violated.
Fix: CreateAccelerator's input validation — the name-length check must emit the service's own fixed message rather than a generic/borrowed name validator's text.
Traces: globalaccelerator-accelerator-lifecycle#task#6 (also task#7).

Cause 2 of 5: CreateEndpointGroup — endpoint group already exists for the listener/region pair.
AWS: Smithy models EndpointGroupAlreadyExistsException on CreateEndpointGroup (errorKind client, httpError 400); the message itself is recorded behavior only. The recording shows HTTP 400 with Message exactly 'Endpoint group already exists.' — entirely fixed text, with no ARN, listener id or region interpolated.
Emulator: returns HTTP 400 with a constructed sentence of the form 'endpoint group <endpointGroupArn> already covers Region <region> for listener <listenerArn>' — it interpolates the existing endpoint-group ARN, the region and the listener ARN into a message AWS never parameterizes.
Why: the message is both differently worded and environment-dependent (it embeds generated ARNs), so a caller comparing it against AWS's constant string fails, and a caller that stores or displays it leaks per-run identifiers where AWS shows a stable sentence.
Fix: CreateEndpointGroup's conflict path / endpoint-group state check — raise EndpointGroupAlreadyExistsException with the fixed message and no interpolation.
Traces: globalaccelerator-endpoint-group-lifecycle#task#7.

Cause 3 of 5: UpdateEndpointGroup — traffic percentage above the modeled maximum.
AWS: Smithy models UpdateEndpointGroupRequest.TrafficDialPercentage as a float with @range min 0 max 100, and the operation throws InvalidArgumentException (httpError 400). The recording shows HTTP 400 with the standard AWS constraint-violation template: "1 validation error detected: Value '<value>' at '<memberPath>' failed to satisfy constraint: Member must have value less than or equal to 100". The fixed text is '1 validation error detected: Value ', " at '", "' failed to satisfy constraint: Member must have value less than or equal to "; the variable parts are the rejected value as the service renders it (a float, rendered with its decimal part — the recording shows '150.0'), the member path the service reports for this input member ('trafficRetentionPercentage' in the recording) and the limit 100 from the member's @range max.
Emulator: emits the same template but with two wrong variable parts — the member path is reported as 'trafficDialPercentage' and the value is rendered as the integer '150' rather than as a float.
Why: a client that parses the standard validation template to find which member it must correct is pointed at a member name the service does not use in this error, and value-rendering differs so an exact-match assertion on the validation message fails.
Fix: UpdateEndpointGroup's input validation / constraint-error formatter — the member path reported for this input member and the float rendering of the rejected value must match the service's.
Traces: globalaccelerator-endpoint-group-lifecycle#task#8 (also task#9).

Cause 4 of 5: DeleteListener — listener still has an endpoint group attached.
AWS: Smithy models AssociatedEndpointGroupFoundException on DeleteListener (errorKind client, httpError 400); the message is recorded behavior only. The recording shows HTTP 400 with Message of the form 'Cannot delete listener <listenerArn> because an associated endpoint group was found.' — fixed text 'Cannot delete listener ' + ' because an associated endpoint group was found.', with the single variable part being the ListenerArn from the request.
Emulator: returns HTTP 400 with 'Listener <listenerArn> has an endpoint group associated with it; delete its endpoint groups before deleting it.' — a wholly different sentence (different clause order, added remediation advice) around the same ARN.
Why: a client asserting on or matching the associated-endpoint-group message sees text that does not exist in AWS, so delete-ordering error handling keyed on that message does not fire.
Fix: DeleteListener's handler/error path — AssociatedEndpointGroupFoundException must be raised with the service's template, interpolating only the request's ListenerArn.
Traces: globalaccelerator-endpoint-group-lifecycle#task#10.

Cause 5 of 5: CreateListener — unrecognized protocol value.
AWS: Smithy models CreateListenerRequest.Protocol as the required enum Protocol (TCP, UDP) and the operation throws InvalidArgumentException (httpError 400). The recording shows HTTP 400 with Message exactly 'Protocol must be either TCP or UDP' — a service-specific, entirely fixed sentence; AWS does not use the generic constraint template for this member.
Emulator: returns HTTP 400 with the generic enum-constraint template instead: "1 validation error detected: Value '<value>' at 'protocol' failed to satisfy constraint: Member must satisfy enum value set: [TCP, UDP]".
Why: the caller gets a differently shaped message for an invalid protocol, so clients matching AWS's short protocol sentence (and tests pinned to it) fail, even though the status and error code are right.
Fix: CreateListener's input validation — the Protocol enum check must emit the service's dedicated fixed message rather than falling through to the generic enum-constraint formatter.
Traces: globalaccelerator-listener-lifecycle#task#8.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, bootstrap_signal false, 5 divergence clusters, all service=globalaccelerator, all verdict=mismatch, all expected_status 400 == actual_status 400, all diverging on key $.Message):

1) globalaccelerator-accelerator-lifecycle#task#6 CreateAccelerator — expected Message 'Accelerator Name should be between 1 - 64 characters in size.'; actual 'Name is too long: a name can have a maximum of 64 characters, must contain only alphanumeric characters, periods (.), or hyphens (-), and must not begin or end with a hyphen or period'. affected_count 2 (task#6, task#7).

2) globalaccelerator-endpoint-group-lifecycle#task#7 CreateEndpointGroup — expected Message 'Endpoint group already exists.'; actual 'endpoint group <endpointGroupArn> already covers Region <region> for listener <listenerArn>' (the run showed arn:aws:globalaccelerator::000000000000:accelerator/<id>/listener/<id>/endpoint-group/<id>, us-west-2 — per-run values, not part of the requirement).

3) globalaccelerator-endpoint-group-lifecycle#task#8 UpdateEndpointGroup — expected "1 validation error detected: Value '150.0' at 'trafficRetentionPercentage' failed to satisfy constraint: Member must have value less than or equal to 100"; actual "1 validation error detected: Value '150' at 'trafficDialPercentage' failed to satisfy constraint: Member must have value less than or equal to 100". affected_count 2 (task#8, task#9).

4) globalaccelerator-endpoint-group-lifecycle#task#10 DeleteListener — expected 'Cannot delete listener <listenerArn> because an associated endpoint group was found.'; actual 'Listener <listenerArn> has an endpoint group associated with it; delete its endpoint groups before deleting it.'

5) globalaccelerator-listener-lifecycle#task#8 CreateListener — expected 'Protocol must be either TCP or UDP'; actual "1 validation error detected: Value 'UDPX' at 'protocol' failed to satisfy constraint: Member must satisfy enum value set: [TCP, UDP]".

Model grounding (services/service-com-amazonaws-globalaccelerator-globalaccelerator-v20180706/smithy-model/com-amazonaws-globalaccelerator-globalaccelerator-v20180706.json): CreateAccelerator.Name GenericString @length 0..255, errors include InvalidArgumentException(400); CreateListener.Protocol required enum Protocol[TCP,UDP], errors include InvalidArgumentException(400); CreateEndpointGroup errors include EndpointGroupAlreadyExistsException(400); UpdateEndpointGroup.TrafficDialPercentage float @range 0..100, errors include InvalidArgumentException(400); DeleteListener errors include AssociatedEndpointGroupFoundException(400). No other open issue covers these (no open globalaccelerator issues exist).

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay trace set simparity-wire-aws-v1.
3. Cause 1: replay globalaccelerator-accelerator-lifecycle step task#6 — CreateAccelerator with a Name longer than 64 characters; compare the 400 body's Message to 'Accelerator Name should be between 1 - 64 characters in size.'
4. Cause 2: replay globalaccelerator-endpoint-group-lifecycle step task#7 — CreateEndpointGroup for a listener/region pair that already has an endpoint group; compare Message to 'Endpoint group already exists.'
5. Cause 3: replay globalaccelerator-endpoint-group-lifecycle step task#8 — UpdateEndpointGroup with a traffic percentage above 100; compare the validation message's member path and float value rendering.
6. Cause 4: replay globalaccelerator-endpoint-group-lifecycle step task#10 — DeleteListener on a listener that still owns an endpoint group; compare Message to 'Cannot delete listener <listenerArn> because an associated endpoint group was found.'
7. Cause 5: replay globalaccelerator-listener-lifecycle step task#8 — CreateListener with Protocol outside [TCP, UDP]; compare Message to 'Protocol must be either TCP or UDP'.
