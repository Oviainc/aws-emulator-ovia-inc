Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: config.discovered-resource-counts-and-config-history
Issue-Shape-Id: com.amazonaws.configservice#StarlingDoveService
Issue-Key: config.discovered-resource-counts-and-config-history|run#14|AssertionError|assert message == (
Issue-Commit: 07e872c7593558b582731e73c173338530c40226

# config: GetResourceConfigHistory's over-length resourceId ValidationException clips the echoed value and appends an emulator-authored "... [N bytes]" marker instead of echoing the submitted value in full

> _Note: this issue was identified against the code base at commit `07e872c7593558b582731e73c173338530c40226`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResourceConfigHistory

## Description
ONE net-new behavioral root cause on this service's boundary-validation rendering path, grounded in one failing behavioral check. 1 CAUSE must be closed before submitting.

Related open issue, which does NOT cover this check: 20261030T093351Z-config-6-net-new-behavioral-root-causes-putconfigrule.md names the same clipping marker in its Cause 4, but only for TagResource's tags.1.member.value leg and only under check ids config.config-rule-custom-policy-details-validated / config.overly-permissive-aggregator-invalid-account-id / config.tag-key-and-value-contract-limits-rejected / config.tag-resource-lifecycle. It carries no replay key for config.discovered-resource-counts-and-config-history, so that check is not gated by it and the GetResourceConfigHistory member path is not named anywhere in it. If the shared renderer is the single fix point, closing it here is a renderer change that also satisfies that issue's Cause 4; the requirement below is stated for this operation's own member path.

--- Cause 1 of 1: the ValidationException message for a resourceId longer than the modeled maximum does not reproduce AWS's constraint-violation sentence, because the rejected value is truncated and annotated (check config.discovered-resource-counts-and-config-history).
AWS: GetResourceConfigHistoryRequest$resourceId is required and targets com.amazonaws.configservice#ResourceId with @length min 1, max 768 (Smithy: GetResourceConfigHistory input member resourceId -> ResourceId @length 1..768); the operation models ValidationException (errorKind client, httpError 400, defaulted) among its errors, documented as "The requested operation is not valid. You will see this exception if ... the input value fails the validation." A request whose resourceId is one character past that maximum is refused 400 ValidationException with AWS's standard single-violation constraint sentence. Its template: the fixed text "1 validation error detected: Value '", then the submitted value ECHOED IN FULL however long it is, then the fixed text "' at '", then the member path "resourceId", then the fixed text "' failed to satisfy constraint: Member must have length less than or equal to ", then the member's modeled maximum (768). The only variable parts are the value (taken verbatim from the request), the member path (taken from the rejected member) and the limit (taken from the model's @length max); everything else is fixed text. The status and code are also requirements: HTTP 400, code ValidationException.
Emulator: answers the right status and the right code (the check's `assert code == "ValidationException"` passes) and gets the fixed head and tail of the sentence right, but it does not echo the value: it emits a prefix of the value, then the literal marker "... [<byte length of the whole value> bytes]", i.e. "1 validation error detected: Value 'xxxxxxxx…xxx... [769 bytes]' at 'resourceId' failed to satisfy constraint: Member must have length less than or equal to 768" for a 769-character resourceId. The "... [N bytes]" annotation is emulator-authored; it appears nowhere in AWS's rendering. The same marker is observable for TagResource ("... [257 bytes]" at 'tags.1.member.value'), so the clipping lives in the shared renderer, not in this operation.
Why: this operation collapses two different invalid-input classes onto two different refusals — ResourceNotDiscoveredException for a well-formed key the recorder never captured, ValidationException for an input that violates the modeled constraint — and within the ValidationException class the message is the ONLY thing that tells a client which member was rejected and what value was rejected. A client (or an operator reading a log) that extracts the rejected value out of the message gets a truncated prefix plus a byte count rather than the value it sent, so value-keyed handling, request replay and any parity assertion on AWS's sentence fail. The marker also misreports the rendering as if AWS summarised long values, which it does not.
Fix: the shared ValidationException constraint-violation renderer used by this service's boundary validation (the length-constraint leg that formats the "Value '<value>' at '<path>' failed to satisfy constraint: ..." sentence) — stop clipping the echoed value and stop appending the " ... [N bytes]" annotation, for any value length. Nothing in GetResourceConfigHistory's own handler (services/service-com-amazonaws-configservice-starlingdoveservice/src/ops/get_resource_config_history.rs) builds this message: the handler's only refusal wordings are InvalidLimitException, InvalidTimeRangeException, InvalidNextTokenException and not_discovered_message(), so the length violation is produced by the boundary validator before handle() runs.
Traces (failing check ids, as examples): config.discovered-resource-counts-and-config-history (cause 1).

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator built from mainline: 117/176 passed (66.5%), bootstrap_signal=false, so the emulator was serving and this call was actually answered. Service token `config` resolves to the bootstrapped configservice emulator (services/service-com-amazonaws-configservice-starlingdoveservice).

Check config.discovered-resource-counts-and-config-history [mismatch]. Its earlier legs PASSED, which localises the divergence: GetDiscoveredResourceCounts returned a well-formed summary with totalDiscoveredResources and a list resourceCounts, the per-type query (resourceTypes=["AWS::S3::Bucket"]) was likewise well-formed, and the never-discovered leg answered the right code AND the right message (ResourceNotDiscoveredException, "Resource <id> of resourceType:AWS::S3::Bucket is unknown or has not been discovered"). The failure is on the final leg only:

  File "check_config_discovered_resource_counts_and_config_history.py", line 76, in run
    assert message == (
  AssertionError: 1 validation error detected: Value 'xxxxxxxx…xxxxxx... [769 bytes]' at 'resourceId' failed to satisfy constraint: Member must have length less than or equal to 768

  Request: GetResourceConfigHistory resourceType='AWS::S3::Bucket', resourceId='x' * 769 (one over the modeled 768 maximum)
  status/code: 400 ValidationException on both sides (that assertion passed)
  expected message: "1 validation error detected: Value '" + the full 769-character value + "' at 'resourceId' failed to satisfy constraint: Member must have length less than or equal to 768"
  got message:      "1 validation error detected: Value '" + a clipped prefix of that value + "... [769 bytes]" + "' at 'resourceId' failed to satisfy constraint: Member must have length less than or equal to 768"

The sole difference is the echoed value: head and tail of the sentence, the member path 'resourceId' and the limit 768 all match. Corroboration that this is the shared renderer and not this operation: the same emulator-authored marker appears for TagResource's 257-character tag value ("Value 'vbbbb…bbb... [257 bytes]' at 'tags.1.member.value' failed to satisfy constraint: Member must have length less than or equal to 256"), where N again equals the full length of the submitted value.

Model grounding (services/service-com-amazonaws-configservice-starlingdoveservice/smithy-model/com-amazonaws-configservice-starlingdoveservice.json): com.amazonaws.configservice#ResourceId is a string with @length {min: 1, max: 768}; GetResourceConfigHistoryRequest$resourceId is required and targets it; com.amazonaws.configservice#ValidationException is errorKind client with httpError 400 (defaulted). Handler reviewed: services/service-com-amazonaws-configservice-starlingdoveservice/src/ops/get_resource_config_history.rs lines 205-297 and 369-374 (its own refusal wordings), which do not include any length-constraint message.

Per-run values are not part of the requirement: the resource id generated by ctx.unique_name for the never-discovered leg, request ids and timestamps are not asserted here. The requirements are the 400 status, the ValidationException code, and the message template above with its value / member path / limit filled from the request and the model.

## Steps to Reproduce
1. git_clone the repo and build/boot the emulator from mainline.
2. Replay the behavioral suite simparity-behavioral-aws-v1 (bootstrap_signal must be false, i.e. the emulator is serving).
3. Run check config.discovered-resource-counts-and-config-history. Alternatively, by hand against the running emulator: call GetResourceConfigHistory with resourceType='AWS::S3::Bucket' and resourceId set to 769 'x' characters (one past ResourceId's modeled @length max of 768).
4. Observe the refusal is 400 ValidationException (correct) but its message clips the echoed value and appends the emulator-authored marker "... [769 bytes]".
5. Required: the message must be exactly "1 validation error detected: Value '<the full 769-character resourceId from the request>' at 'resourceId' failed to satisfy constraint: Member must have length less than or equal to 768" — the value echoed in full, with no clipping and no " ... [N bytes]" annotation.
6. Positive control to keep green: in the same check, GetResourceConfigHistory for a well-formed but never-recorded resourceId must still answer 400 ResourceNotDiscoveredException with "Resource <resourceId> of resourceType:<resourceType> is unknown or has not been discovered", and GetDiscoveredResourceCounts (with and without resourceTypes) must still answer totalDiscoveredResources plus a resourceCounts list.
7. Re-replay the check id in trace_ids to confirm the fix: config.discovered-resource-counts-and-config-history.
