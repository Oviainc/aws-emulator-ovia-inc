Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__ssm.vscan-instance-information-filter-rejects-a-malformed-instance-id ec2__ssm.w22n-tag-filter-mixed-with-a-platform-filter-rejected-on-both-node-apis
Issue-Key: ec2__ssm.vscan-instance-information-filter-rejects-a-malformed-instance-id|run#8|AssertionError|assert msg == expected, f"expected the exact InvalidInstanceId message {expected!r}, got {msg!r}"
Issue-Key: ec2__ssm.w22n-tag-filter-mixed-with-a-platform-filter-rejected-on-both-node-apis|run#12|AssertionError|assert code == "ValidationException", (
Issue-Commit: 3a80585caf3e9f62e646ed534a139af0bb6604b7

# SSM managed-node filter validation: malformed instance id message does not state the pattern constraint, and a tag: filter combined with PlatformTypes returns InvalidFilterKey instead of ValidationException

> _Note: this issue was identified against the code base at commit `3a80585caf3e9f62e646ed534a139af0bb6604b7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstanceInformation

## Description
Two net-new root causes in the SSM emulator's managed-node filter validation.

Cause 1 of 2: the malformed-instance-id refusal does not report the constraint that was violated.
AWS: the message is a constraint violation built from the request - it names the member and the regular expression the value failed: "Value at 'instanceId' failed to satisfy constraint: Member must satisfy regular expression pattern: (^i-(\\w{8}|\\w{17})$)|(^mi-\\w{17}$)". That pattern is the modelled rule for this identifier space (Smithy, ssm: the managed-node id pattern admits i- with 8 or 17 characters and mi- with 17), and DescribeInstanceInformation declares InvalidInstanceId as a client error (400). Because the message is derived from the member name and its modelled pattern, that derivation is the requirement.
Emulator: returns the right error code but a hand-written prose message instead: "Value 'not-an-instance-id' for filter key 'InstanceIds' is not a managed node ID: a node ID is 'i-' and 8 or 17 characters, or 'mi-' and 17 characters" - it names the filter key rather than the 'instanceId' member and never states the pattern.
Why: AWS's constraint-violation messages are machine-parseable and uniform across operations - clients and SDK-level validators extract the member path and the pattern from them to report which input to fix. The emulator's bespoke sentence carries neither, so that extraction yields nothing, and the message does not match the modelled constraint it is supposed to be reporting.
Fix: the input/filter validation for DescribeInstanceInformation in services/service-com-amazonaws-ssm-amazonssm/src/ops/describe_instance_information.rs - produce the constraint-violation message from the member name and its modelled pattern rather than a literal sentence.
Traces: ec2__ssm.vscan-instance-information-filter-rejects-a-malformed-instance-id.

Cause 2 of 2: a tag: filter combined with a non-tag filter is refused with the wrong error code on both managed-node APIs.
AWS: recorded behavior only for the code. A "tag:<key>" filter cannot be combined with a non-tag filter on either managed-node API: tag: + PlatformTypes=Windows is refused with ValidationException by DescribeInstanceInformation AND by DescribeInstanceProperties, while two tag: filters together, and InstanceIds + PlatformTypes together, are both ACCEPTED - so the rule is about which filter kinds may be ANDed, not a blanket refusal. (Smithy lists InvalidFilterKey, InvalidInstanceId, InvalidInstanceInformationFilterValue and InvalidNextToken for this operation; the combination rule is enforced as a request validation, so ValidationException is recorded behavior rather than a modelled per-operation error.)
Emulator: refuses the pair with error code InvalidFilterKey. InvalidFilterKey means "that filter key is not recognised" - but both keys here ARE recognised; it is their COMBINATION that is invalid.
Why: wrong error code on a rejection. A client that retries or falls back on InvalidFilterKey (by dropping the unknown key) will drop a key that was perfectly valid and silently widen or narrow its node query; a client that handles ValidationException (fix the request shape) never triggers. Fleet-scoped queries - the ones that must choose between scoping by tag OR narrowing by platform - get the wrong guidance, and an emulator that simply ANDs every filter it is given would accept the refused pair outright.
Fix: the filter-combination validation shared by DescribeInstanceInformation and DescribeInstanceProperties in services/service-com-amazonaws-ssm-amazonssm/src/ops/ - classify a disallowed COMBINATION as ValidationException and keep InvalidFilterKey for genuinely unknown keys, while continuing to accept tag:+tag: and InstanceIds+PlatformTypes.
Traces: ec2__ssm.w22n-tag-filter-mixed-with-a-platform-filter-rejected-on-both-node-apis.

## Evidence
Cause 1 - AssertionError: expected the exact InvalidInstanceId message "Value at 'instanceId' failed to satisfy constraint: Member must satisfy regular expression pattern: (^i-(\\w{8}|\\w{17})$)|(^mi-\\w{17}$)", got "Value 'not-an-instance-id' for filter key 'InstanceIds' is not a managed node ID: a node ID is 'i-' and 8 or 17 characters, or 'mi-' and 17 characters". Error code InvalidInstanceId matched.
Cause 2 - AssertionError: DescribeInstanceInformation must refuse tag + PlatformTypes with ValidationException, got 'InvalidFilterKey'. Expected error code: ValidationException. Observed error code: InvalidFilterKey. The check asserts the same refusal on DescribeInstanceProperties and asserts that tag:+tag: and InstanceIds+PlatformTypes are ACCEPTED, so the finding is the combination rule, not a blanket rejection.
The literal bad value 'not-an-instance-id' and the tag key used are inputs chosen by the check, not environment-specific values.

## Steps to Reproduce
1. git_clone(), then run_behavioral_suite().
2. Cause 1: ssm.DescribeInstanceInformation(Filters=[{Key:'InstanceIds', Values:['not-an-instance-id']}]) -> expect InvalidInstanceId whose message names the 'instanceId' member and its regular-expression constraint; observe the bespoke prose message.
3. Cause 2: ssm.DescribeInstanceInformation(Filters=[{Key:'tag:<any>', Values:['v']}, {Key:'PlatformTypes', Values:['Windows']}]) -> expect ValidationException; observe InvalidFilterKey. Repeat against ssm.DescribeInstanceProperties. Then confirm two tag: filters together, and InstanceIds + PlatformTypes together, are still accepted.
