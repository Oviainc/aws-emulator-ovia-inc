Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-data-automation-blueprint-error-paths
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-data-automation-blueprint-error-paths bedrock-data-automation-blueprint-lifecycle bedrock-data-automation-project-error-paths bedrock-data-automation-project-invalid-name-pattern-rejected
Issue-Shape-Id: com.amazonaws.bedrockdataautomation#AmazonBedrockKeystoneBuildTimeService
Issue-Key: bedrock-data-automation|CreateBlueprint|4xx->4xx|$.message
Issue-Key: bedrock-data-automation|GetBlueprint|4xx->4xx|$.message
Issue-Key: bedrock-data-automation|DeleteBlueprint|4xx->4xx|$.message
Issue-Key: bedrock-data-automation|CreateDataAutomationProject|4xx->4xx|$.message
Issue-Key: bedrock-data-automation|GetDataAutomationProject|4xx->4xx|$.message
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# Blueprint and project error messages diverge from AWS wording

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateBlueprint

## Description
Six client-error paths return the right HTTP status and error type but the emulator's own prose instead of the wording this service uses. Five of them are name/ARN-not-found and name-conflict refusals whose AWS wording is FIXED text with no request values interpolated, where the emulator echoes the requested name or ARN; the sixth leaks a @sensitive member's value into a ValidationException message AWS deliberately omits it from. Six net-new causes. (The open bedrock-data-automation-runtime error-response issues, e.g. 20261026T003553Z-bedrock-data-automation-runtime-tagresource-error-response, are for a different service and do not cover these.)

=== Cause 1 of 6: CreateBlueprint name-conflict message echoes the requested name ===
AWS: Smithy: CreateBlueprint errors include ConflictException (errorKind client, @httpError(409), single member message -> NonBlankString), doc "This exception is thrown when there is a conflict performing an operation". The recorded 409 message is exactly "Blueprint name already exists." -- entirely fixed text, with NO part taken from the request or from state.
Emulator: status 409 (correct) with message "a blueprint named <blueprintName> already exists in this account" -- different fixed text, and it interpolates the requested blueprintName, which Smithy marks @sensitive on BlueprintName.
Why: a client that branches on the conflict message (the only discriminator ConflictException carries, since the shape has no code member) does not match, so a create-or-adopt path falls through to its unexpected-error branch. Echoing the @sensitive blueprint name also puts it into error logs AWS keeps it out of.
Fix: the CreateBlueprint handler's conflict refusal wording -- emit the fixed "Blueprint name already exists." and interpolate nothing.
Traces: bedrock-data-automation-blueprint-error-paths#task#2.

=== Cause 2 of 6: GetBlueprint not-found message echoes the requested ARN ===
AWS: Smithy: GetBlueprint errors include ResourceNotFoundException (errorKind client, @httpError(404), single member message), doc "This exception is thrown when a resource referenced by the operation does not exist". The recorded 404 message is exactly "Blueprint Arn not found." -- fixed text with no request value in it.
Emulator: status 404 (correct) with message "blueprint <blueprintArn> does not exist" -- different fixed text plus the requested ARN interpolated.
Why: a client matching the service's not-found wording treats the response as an unknown failure; and the echoed ARN makes the message length a function of the request, which the service's fixed wording never is.
Fix: the GetBlueprint handler's not-found refusal wording.
Traces: bedrock-data-automation-blueprint-error-paths#task#4 (also bedrock-data-automation-blueprint-lifecycle#teardown#11, #teardown#13).

=== Cause 3 of 6: DeleteBlueprint not-found message echoes the requested ARN ===
AWS: Smithy: DeleteBlueprint errors include ResourceNotFoundException @httpError(404). The recorded 404 message is exactly "Blueprint Arn not found.".
Emulator: status 404 (correct) with message "blueprint <blueprintArn> does not exist".
Why: idempotent teardown code that recognises the service's not-found wording to swallow a double-delete does not recognise this one and escalates.
Fix: the DeleteBlueprint handler's not-found refusal wording -- single-home it with Cause 2 so the two cannot drift.
Traces: bedrock-data-automation-blueprint-error-paths#task#5 (also bedrock-data-automation-blueprint-lifecycle#teardown#14).

=== Cause 4 of 6: CreateDataAutomationProject name-conflict message echoes the requested name ===
AWS: Smithy: CreateDataAutomationProject errors include ConflictException @httpError(409). The recorded 409 message is exactly "Project name already exists." -- fixed text, nothing interpolated.
Emulator: status 409 (correct) with message "a data automation project named <projectName> already exists in this account" -- different fixed text, and it echoes projectName, which Smithy marks @sensitive on DataAutomationProjectName.
Why: same discrimination failure as Cause 1, plus a @sensitive project name pushed into client-visible error text.
Fix: the CreateDataAutomationProject handler's conflict refusal wording.
Traces: bedrock-data-automation-project-error-paths#task#2.

=== Cause 5 of 6: GetDataAutomationProject not-found message echoes the requested ARN ===
AWS: Smithy: GetDataAutomationProject errors include ResourceNotFoundException @httpError(404). The recorded 404 message is exactly "Project Arn not found.".
Emulator: status 404 (correct) with message "no data automation project exists with ARN <projectArn>".
Whip: n/a.
Why: a client that keys retire-vs-retry off the not-found wording does not match, so a project that is genuinely gone is treated as an unclassified error.
Fix: the GetDataAutomationProject handler's not-found refusal wording.
Traces: bedrock-data-automation-project-error-paths#task#4.

=== Cause 6 of 6: CreateDataAutomationProject's projectName ValidationException leaks the sensitive value and reports an anchored pattern ===
AWS: Smithy: CreateDataAutomationProjectRequest.projectName targets DataAutomationProjectName, @length(1..128), pattern ^[a-zA-Z0-9-_]+$, and is marked @sensitive; ValidationException is errorKind client @httpError(400) with members message and fieldList. The recorded 400 message is "1 validation error detected: Value at 'projectName' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9-_]+". Fixed text: "1 validation error detected: Value at '", "' failed to satisfy constraint: Member must satisfy regular expression pattern: ". Variable parts: the member path (projectName, from the request) and the pattern (from the model, reported WITHOUT the ^ and $ anchors). Crucially there is NO offending value between "Value" and "at" -- because the member is @sensitive, AWS suppresses it.
Emulator: status 400 (correct) with message "1 validation error detected: Value '<projectName>' at 'projectName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9-_]+$" -- it inserts the rejected @sensitive projectName and reports the anchored form of the pattern.
Why: the emulator writes a @sensitive value into an error body and therefore into every client log and trace that records it, which is exactly what the @sensitive trait exists to prevent; and a client (or a test) that compares the message against AWS's sensitive-suppressed, unanchored form never matches, so the validation contract cannot be asserted against the emulator.
Fix: the ValidationException message builder used by CreateDataAutomationProject's input validation -- suppress the offending value for @sensitive members and report the pattern unanchored.
Traces: bedrock-data-automation-project-invalid-name-pattern-rejected#task#1.

## Evidence
Cause 1 (bedrock-data-automation-blueprint-error-paths#task#2, CreateBlueprint, 409 vs 409): $.message expected 'Blueprint name already exists.', got 'a blueprint named <blueprintName> already exists in this account' (this run: 'sim2real-probe-bedrock-data-automation-bda-blueprint-err-f8cc9362').
Cause 2 (bedrock-data-automation-blueprint-error-paths#task#4, GetBlueprint, 404 vs 404): $.message expected 'Blueprint Arn not found.', got 'blueprint <arn> does not exist' (this run: arn:aws:bedrock:us-west-2:000000000000:blueprint/0c579b95-0000000000000000). 3 steps in the cluster.
Cause 3 (bedrock-data-automation-blueprint-error-paths#task#5, DeleteBlueprint, 404 vs 404): $.message expected 'Blueprint Arn not found.', got 'blueprint <arn> does not exist'. 2 steps in the cluster.
Cause 4 (bedrock-data-automation-project-error-paths#task#2, CreateDataAutomationProject, 409 vs 409): $.message expected 'Project name already exists.', got 'a data automation project named <projectName> already exists in this account'.
Cause 5 (bedrock-data-automation-project-error-paths#task#4, GetDataAutomationProject, 404 vs 404): $.message expected 'Project Arn not found.', got 'no data automation project exists with ARN <projectArn>'.
Cause 6 (bedrock-data-automation-project-invalid-name-pattern-rejected#task#1, CreateDataAutomationProject, 400 vs 400): $.message expected "1 validation error detected: Value at 'projectName' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9-_]+", got "1 validation error detected: Value '<projectName>' at 'projectName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9-_]+$".
Model citations: Smithy ConflictException errorKind client @httpError(409), member message -> NonBlankString; ResourceNotFoundException errorKind client @httpError(404), member message -> NonBlankString; ValidationException errorKind client @httpError(400), members message + fieldList; BlueprintName @length(1..128) pattern ^[a-zA-Z0-9-_]+$ @sensitive; DataAutomationProjectName @length(1..128) pattern ^[a-zA-Z0-9-_]+$ @sensitive; CreateBlueprint/CreateDataAutomationProject model ConflictException; GetBlueprint/DeleteBlueprint/GetDataAutomationProject model ResourceNotFoundException; protocol restJson1. (The generated names and ARNs quoted above are this run's values; the requirement is the fixed wording and, for Cause 6, the member path plus the unanchored pattern with the sensitive value suppressed.)

## Steps to Reproduce
1. Replay the wire corpus simparity-wire-aws-v1 against the emulator (replay_wire_corpus).
2. Cause 1: fixture bedrock-data-automation-blueprint-error-paths, step task#2 -- CreateBlueprint reusing an existing blueprint name; expect 409 with message exactly 'Blueprint name already exists.'.
3. Cause 2: same fixture, step task#4 -- GetBlueprint on a blueprint ARN that does not exist; expect 404 with message exactly 'Blueprint Arn not found.'.
4. Cause 3: same fixture, step task#5 -- DeleteBlueprint on that ARN; expect 404 with message exactly 'Blueprint Arn not found.'.
5. Cause 4: fixture bedrock-data-automation-project-error-paths, step task#2 -- CreateDataAutomationProject reusing an existing project name; expect 409 with message exactly 'Project name already exists.'.
6. Cause 5: same fixture, step task#4 -- GetDataAutomationProject on a project ARN that does not exist; expect 404 with message exactly 'Project Arn not found.'.
7. Cause 6: fixture bedrock-data-automation-project-invalid-name-pattern-rejected, step task#1 -- CreateDataAutomationProject with a projectName containing '/'; expect 400 whose message omits the offending value and reports the pattern unanchored.
