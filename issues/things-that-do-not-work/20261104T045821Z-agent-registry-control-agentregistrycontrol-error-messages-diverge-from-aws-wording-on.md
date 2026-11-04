Issue-Source: trace-sweep-divergence
Issue-Trace: agent-registry-control-registry-record-lifecycle
Issue-Step: task#9
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: agent-registry-control-registry-record-lifecycle agent-registry-control-registry-validation-and-conflict-errors
Issue-Shape-Id: com.amazonaws.agentregistrycontrol#AgentRegistryControl
Issue-Key: agent-registry-control|GetRegistryRecord|4xx->4xx|$.message
Issue-Key: agent-registry-control|GetRegistry|4xx->4xx|$.message
Issue-Key: agent-registry-control|CreateRegistryRecord|4xx->4xx|$.message
Issue-Key: agent-registry-control|CreateRegistry|4xx->4xx|$.message
Issue-Commit: 024100b485faef6e8a7d69224b42c4d112173d44

# AgentRegistryControl error messages diverge from AWS wording on registry and record errors

> _Note: this issue was identified against the code base at commit `024100b485faef6e8a7d69224b42c4d112173d44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRegistryRecord

## Description
Four AgentRegistryControl error paths return the correct HTTP status and the correct modeled error shape, but the `message` member carries emulator-invented prose instead of the message AWS words for that error class. The statuses and error codes match; only the message text diverges, and the message is part of the contract. This bundle has 4 causes and all 4 must be closed.

Cause 1 of 4 - GetRegistryRecord, record not found (fixture agent-registry-control-registry-record-lifecycle, step task#9).
AWS: Smithy: GetRegistryRecord error ResourceNotFoundException (httpError 404, member `message` -> NonBlankString). Recorded behavior: 404 with message template "Registry record with ID <recordId> not found." Fixed text: "Registry record with ID " ... " not found." (leading capital, trailing period); the only variable part is the `recordId` label value taken from the request URI.
Emulator: returns 404 with message "record <recordId> was not found in registry <registryId>" - lowercase, different phrasing, no trailing period, and it interpolates the registry id, which AWS's message does not mention.
Why: clients and tests that match on the not-found message (log scrapers, error-mapping layers, assertions on the human-readable reason) see text no real AWS response ever produces, and the message leaks the registry identifier.
Fix: the not-found error construction in the GetRegistryRecord handler of the agentregistrycontrol service crate's ops module.
Traces: agent-registry-control-registry-record-lifecycle#task#9 (cluster also covers agent-registry-control-registry-validation-and-conflict-errors#task#9).

Cause 2 of 4 - GetRegistry, registry not found (fixture agent-registry-control-registry-validation-and-conflict-errors, step task#8).
AWS: Smithy: GetRegistry error ResourceNotFoundException (httpError 404). Recorded behavior: 404 with message template "Registry with ARN <registryId> not found." Fixed text: "Registry with ARN " ... " not found."; the variable part is the registry identifier exactly as supplied in the `registryId` URI label (AWS echoes the supplied identifier, whether bare id or ARN).
Emulator: returns 404 with message "registry <registryId> was not found" - lowercase, "was not found" instead of "not found.", no "with ARN" phrase, no trailing period.
Why: same as Cause 1 - the error message a client surfaces or matches on does not resemble AWS's.
Fix: the not-found error construction in the GetRegistry handler of the agentregistrycontrol service crate's ops module.
Traces: agent-registry-control-registry-validation-and-conflict-errors#task#8.

Cause 3 of 4 - CreateRegistryRecord, duplicate record name (fixture agent-registry-control-registry-validation-and-conflict-errors, step task#6).
AWS: Smithy: CreateRegistryRecord error ConflictException (httpError 409, member `message`). Recorded behavior: 409 with message template "A record with name '<name>' already exists in registry <registryId>". Fixed text: "A record with name '" ... "' already exists in registry " ...; variable parts are the requested record `name` (single-quoted) and the registry id from the request.
Emulator: returns 409 with message "a record named <name> already exists in registry <registryId>" - lowercase "a", "named" instead of "with name", and the name is not quoted.
Why: a client that parses the conflicting name out of the quoted segment of the message gets nothing, and the surfaced text differs from AWS.
Fix: the duplicate-name conflict error construction in the CreateRegistryRecord handler of the agentregistrycontrol service crate's ops module.
Traces: agent-registry-control-registry-validation-and-conflict-errors#task#6.

Cause 4 of 4 - CreateRegistry, name fails the pattern constraint (fixture agent-registry-control-registry-validation-and-conflict-errors, step task#5).
AWS: Smithy: CreateRegistry.name -> RegistryName @pattern ^[a-zA-Z0-9][a-zA-Z0-9_\-\.\/]*$, @length 1..64; error ValidationException (httpError 400, reason enum default FieldValidationFailed). Recorded behavior: 400 with message template "1 validation error detected: Value at '<member>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>" where <pattern> is rendered WITHOUT the ^ and $ anchors, i.e. [a-zA-Z0-9][a-zA-Z0-9_\-\.\/]*, and the offending value is NOT echoed. Fixed text is everything but the member name and the pattern.
Emulator: returns 400 with message "1 validation error detected: Value '<the rejected value>' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9][a-zA-Z0-9_\-\.\/]*$" - it inserts "Value '<value>' at" where AWS writes "Value at", and it prints the anchored regex source instead of the unanchored pattern AWS reports.
Why: the message echoes caller input AWS deliberately omits here, and the quoted pattern differs from AWS's, so client-side validation mirrors and golden-file tests built on AWS messages diverge.
Fix: the constraint-violation message formatter used by CreateRegistry input validation in the agentregistrycontrol service crate (strip the regex anchors and drop the value echo for this message form).
Traces: agent-registry-control-registry-validation-and-conflict-errors#task#5.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator, 4 mismatch clusters, status and error shape correct in every one, `$.message` wrong in every one:

1) GetRegistryRecord 404 -> 404, $.message: expected 'Registry record with ID 76db21555d61 not found.', got 'record 76db21555d61 was not found in registry c0f25330f988' (agent-registry-control-registry-record-lifecycle#task#9; same cluster: agent-registry-control-registry-validation-and-conflict-errors#task#9).
2) GetRegistry 404 -> 404, $.message: expected 'Registry with ARN zzzzzzzzzzzz not found.', got 'registry zzzzzzzzzzzz was not found' (agent-registry-control-registry-validation-and-conflict-errors#task#8).
3) CreateRegistryRecord 409 -> 409, $.message: expected "A record with name 'sim2real-probe-arc-rec-6dd98020' already exists in registry 0f61cd4fdf9b", got 'a record named sim2real-probe-arc-rec-6dd98020 already exists in registry 0f61cd4fdf9b' (agent-registry-control-registry-validation-and-conflict-errors#task#6).
4) CreateRegistry 400 -> 400, $.message: expected "1 validation error detected: Value at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9][a-zA-Z0-9_\-\.\/]*", got "1 validation error detected: Value 'sim2real probe arc bad name!' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9][a-zA-Z0-9_\-\.\/]*$" (agent-registry-control-registry-validation-and-conflict-errors#task#5).

The ids, names and rejected values above are this run's values filling the templates; the requirement is the fixed text plus the request/state-derived substitutions described per cause.

A related open issue, 20261104T021341Z-agent-registry-discoverable-record-reads-resourcenotfoundexception-message-wording.md, reports the same class of wording defect on the sibling agent-registry service; it does not cover these agent-registry-control operations.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1: create a registry and a record, delete the record, then GET /registries/{registryId}/records/{recordId} for the deleted record - compare $.message against 'Registry record with ID <recordId> not found.' (agent-registry-control-registry-record-lifecycle, task#9).
3. Cause 2: GET /registries/zzzzzzzzzzzz for an identifier that does not exist - compare $.message against 'Registry with ARN <registryId> not found.' (agent-registry-control-registry-validation-and-conflict-errors, task#8).
4. Cause 3: CreateRegistryRecord twice with the same name in one registry - compare the 409 $.message against "A record with name '<name>' already exists in registry <registryId>" (same fixture, task#6).
5. Cause 4: CreateRegistry with name 'sim2real probe arc bad name!' - compare the 400 $.message against "1 validation error detected: Value at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9][a-zA-Z0-9_\-\.\/]*" (same fixture, task#5).
