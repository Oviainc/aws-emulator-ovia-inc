Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agentcore-control.create-capacity-provider-out-of-range-value-rejected
Issue-Shape-Id: com.amazonaws.bedrockagentcorecontrol#AmazonBedrockAgentCoreControl
Issue-Key: bedrock-agentcore-control.create-capacity-provider-out-of-range-value-rejected|run#11|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Commit: c2d690adb5c99cf0fe6b339d9b4e7b6ccf196d09

# CreateCapacityProvider's out-of-range @range refusal uses the counted "1 validation error detected" form instead of AWS's JSON-pointer "must be between 60 and 1209600, inclusive" sentence

> _Note: this issue was identified against the code base at commit `c2d690adb5c99cf0fe6b339d9b4e7b6ccf196d09`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCapacityProvider

## Description
The emulator refuses an out-of-range idleInstanceTimeout with the right HTTP status and error code but with a message built from a different template than the one AWS uses on this input path. The error message is part of the operation's contract. 1 cause must be fixed before this issue is closed.

Cause 1 of 1: the @range violation message for CreateCapacityProvider's computeConfiguration members is rendered in the counted/dot-path/echoed-value form instead of AWS's JSON-pointer inclusive-range form.
AWS: Smithy InstanceLifecycleConfiguration.idleInstanceTimeout targets Integer with @range {min: 60, max: 1209600} (doc: "The number of seconds an instance can remain idle before it is stopped ... The default is 900 seconds"), reached from CreateCapacityProvider via computeConfiguration -> ec2Configuration -> lifecycleConfiguration; CreateCapacityProvider's modeled errors include ValidationException (httpError 400). Recorded behavior: a CreateCapacityProvider whose computeConfiguration.ec2Configuration.lifecycleConfiguration.idleInstanceTimeout exceeds the modeled maximum is refused 400 ValidationException with the message "validation error detected. Value at '<json-pointer to the offending member>' failed to satisfy constraint: Member must be between <range min> and <range max>, inclusive". The fixed text is "validation error detected. Value at '", "' failed to satisfy constraint: Member must be between ", " and ", ", inclusive"; the variable parts are the offending member's path rendered as a JSON pointer (leading '/', '/'-separated member names, no list indices or ".member" infixes) and the two @range bounds taken from the model. AWS does not echo the submitted value and does not prefix a violation count. The request leaves no capacity provider behind, and the same request with an in-range value is accepted.
Emulator: answers 400 ValidationException (status and code correct) with "1 validation error detected: Value '<submitted value>' at '<dot-separated member path>' failed to satisfy constraint: Member must have value less than or equal to <range max>" — i.e. a counted prefix AWS does not use here, a dot-separated path instead of a JSON pointer, an echo of the submitted value AWS omits, and a one-sided "must have value less than or equal to" phrase instead of the inclusive-range phrase that names both bounds.
Why: a client that matches or parses the refusal text to find the offending member (the JSON pointer is what AWS gives it) and to learn the permitted window gets a sentence that exists nowhere in AWS: the pointer it looks for is absent, the lower bound 60 is never reported, and assertions on the documented wording fail against the emulator while passing against AWS. Nothing in the response otherwise tells the caller which of the nested computeConfiguration members was rejected.
Fix: the shared constraint-violation message formatter used on the CreateCapacityProvider input validation path — it must emit the pointer-path, no-count, no-echoed-value, "between <min> and <max>, inclusive" template for @range violations on this operation's input members — not the operation's state handling.
Related open issues (neither covers this template): 20261029T212239Z-bedrock-agentcore-control-refusal-messages-diverge-from-aws.md cause 4 concerns how a Double is rendered *inside* the counted "1 validation error detected: ..." form on the gateway rate-limit paths, which is a different message template from the one required here; 20261029T212239Z-bedrock-agentcore-control-createcapacityprovider-stub.md covers the unimplemented handler that the in-range positive control of this same check will meet after the refusal text is fixed.
Traces: bedrock-agentcore-control.create-capacity-provider-out-of-range-value-rejected.

## Evidence
Check bedrock-agentcore-control.create-capacity-provider-out-of-range-value-rejected (verdict: mismatch). The error code matched (ValidationException); only the message diverged:
  expected: "validation error detected. Value at '/computeConfiguration/ec2Configuration/lifecycleConfiguration/idleInstanceTimeout' failed to satisfy constraint: Member must be between 60 and 1209600, inclusive"
  got:      "1 validation error detected: Value '1209601' at 'computeConfiguration.ec2Configuration.lifecycleConfiguration.idleInstanceTimeout' failed to satisfy constraint: Member must have value less than or equal to 1209600"
Fixed text of the requirement: "validation error detected. Value at '" + <pointer> + "' failed to satisfy constraint: Member must be between " + <min> + " and " + <max> + ", inclusive". Variable parts: the JSON-pointer member path (from the request body's shape) and the bounds 60 / 1209600 (from Smithy InstanceLifecycleConfiguration.idleInstanceTimeout @range). The value 1209601 is this run's submitted input and is NOT part of AWS's message at all.
Smithy: InstanceLifecycleConfiguration.idleInstanceTimeout -> Integer, @range min 60 max 1209600; CreateCapacityProvider errors include ValidationException (400), http PUT /capacity-providers, success code 202.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite.
2. Call CreateCapacityProvider with a valid name, permissionsConfiguration.capacityProviderOperatorRoleArn, and computeConfiguration.ec2Configuration carrying launchTemplateSource, vpcConfiguration and lifecycleConfiguration.idleInstanceTimeout set above the modeled maximum (e.g. 1209601).
3. Read $.message of the 400 ValidationException: the emulator returns the counted/dot-path/echoed-value sentence instead of AWS's "validation error detected. Value at '/computeConfiguration/ec2Configuration/lifecycleConfiguration/idleInstanceTimeout' failed to satisfy constraint: Member must be between 60 and 1209600, inclusive" (check bedrock-agentcore-control.create-capacity-provider-out-of-range-value-rejected).
