Issue-Source: trace-sweep-divergence
Issue-Trace: states-creation-input-validation-errors
Issue-Step: task#9
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: states-creation-input-validation-errors stepfunctions-state-machine-crud-and-tagging states-state-machine-lifecycle-crud stepfunctions-alias-invalid-routing-weights sfn-validation-reason-labeled-arn-and-completed-map-run
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: states|CreateStateMachine|4xx->4xx|$.message
Issue-Key: states|UpdateStateMachine|4xx->4xx|$.message
Issue-Related: 20261029T171911Z-states-step-functions-client-refusals-answer-the-modelled-httperror.md
Issue-Commit: a9006feb831fdd789c02a15722f93d2f2e52852a

# Step Functions refusal messages are emulator-authored instead of AWS's wording on four 400-class validation paths (4 causes)

> _Note: this issue was identified against the code base at commit `a9006feb831fdd789c02a15722f93d2f2e52852a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStateMachine

## Description
This issue bundles FOUR distinct net-new wire causes that share one fault: the refusal reaches the client with the right HTTP status (400) and the right error code, but the message is hand-written by the emulator rather than AWS's wording for that error class. An error message is a requirement, not a cosmetic detail: it is the only part of these refusals that says WHICH rule was broken, because the error code is shared across many rules. All four must be closed before submitting; re-replay every fixture#step listed in trace_ids.

=== Cause 1 of 4: CreateStateMachine's InvalidDefinition message for unparsable JSON drops the parser's source location and inserts emulator text ===
AWS: Smithy models CreateStateMachine input {name (required), definition (required, @length 1..1048576), roleArn (required), ...} and the error InvalidDefinition (400). Recorded behavior defines the message template: `Invalid State Machine Definition: '<CODE>: <parser message>\n at [Source: (String)"<definition>"; line: <L>, column: <C>] at [Source: (String)"<definition>"; line: <L>, column: <C>]'`. Fixed text: `Invalid State Machine Definition: '`, the code (here `INVALID_JSON_DESCRIPTION`), the `\n at [Source: (String)"`...`"; line: `...`, column: `...`]` location clause — which appears TWICE — and the closing quote. Variable parts: the code, the underlying JSON parser message, the submitted definition string echoed back, and the line/column where parsing failed.
Emulator: it emits `Invalid State Machine Definition: '<CODE>: the definition is not valid JSON: <parser message>'` — it injects its own `the definition is not valid JSON: ` clause and omits both `at [Source: ...]` location clauses entirely.
Why: the source-location clause is how a caller (and every CI log a human reads) locates the syntax error in a large definition; dropping it makes an invalid 1 MB ASL document unlocatable, and the injected clause breaks any assertion on AWS's wording.
Fix: the definition-validation error message construction in CreateStateMachine/UpdateStateMachine's validation path — render the code, the parser message and the doubled source-location clause, and drop the emulator's own prose.
Traces: states-creation-input-validation-errors#task#9 (also stepfunctions-state-machine-crud-and-tagging#task#12).

=== Cause 2 of 4: UpdateStateMachine's "nothing to update" refusal names only two of the four updatable members ===
AWS: Smithy models UpdateStateMachine input {stateMachineArn (required), definition, roleArn, loggingConfiguration, tracingConfiguration, publish, versionDescription, encryptionConfiguration} and the error MissingRequiredParameter (400). Recorded message, fixed text with NO variable part: `Either the definition, the role ARN, the LoggingConfiguration, or the TracingConfiguration must be specified`.
Emulator: `either definition or roleArn must be given to update a state machine` — emulator-authored, and it enumerates only two of the four members AWS accepts as a change, which also implies the emulator's own precondition is narrower than AWS's.
Why: a caller that updates ONLY loggingConfiguration or ONLY tracingConfiguration is told it must send a definition or roleArn; worse, if the handler's check matches its message, such an update is refused outright even though AWS accepts it.
Fix: UpdateStateMachine's validation — accept loggingConfiguration and tracingConfiguration (and the encryption configuration) as a change, and emit AWS's fixed sentence when none is present.
Traces: states-state-machine-lifecycle-crud#task#8 (also stepfunctions-state-machine-crud-and-tagging#task#13).

=== Cause 3 of 4: CreateStateMachineAlias's routing-weight refusal is reworded and loses the received-weights list ===
AWS: Smithy models CreateStateMachineAlias input {name (required), routingConfiguration (required, @length 1..2)} and ValidationException (400). Recorded message template: `Sum of routing configuration weights must equal 100. Received: [<weights>]` — fixed text up to and including `Received: [`, with the variable part being the weight values from the request's routingConfiguration, in request order.
Emulator: `the weights of a routing configuration must add up to 100, got <sum>` — emulator-authored wording, and it reports the computed SUM where AWS lists the individual received weights.
Why: for a two-entry routing configuration the list is what tells the caller which entry is wrong; a single sum does not. Any client or test matching AWS's sentence fails.
Fix: CreateStateMachineAlias/UpdateStateMachineAlias validation message construction — render AWS's sentence with the bracketed list of received weights.
Traces: stepfunctions-alias-invalid-routing-weights#task#5.

=== Cause 4 of 4: UpdateMapRun's completed-Map-Run refusal omits the Map Run ARN ===
AWS: Smithy models UpdateMapRun input {mapRunArn (required), maxConcurrency, toleratedFailurePercentage, toleratedFailureCount} with ValidationException (400). Recorded message template: `Cannot update completed MapRun <mapRunArn>` — fixed text `Cannot update completed MapRun ` plus the Map Run ARN taken from the request.
Emulator: `Cannot update a completed Map Run` — reworded and carrying no ARN at all.
Why: a batch tool updating many Map Runs concurrently cannot tell from the refusal WHICH Map Run had already completed, so it cannot skip it and retry the rest.
Fix: UpdateMapRun's handler/validation message construction — interpolate the requested mapRunArn into AWS's fixed text.
Traces: sfn-validation-reason-labeled-arn-and-completed-map-run#task#8.

## Evidence
Replay of trace set simparity-wire-aws-v1; all four lines verdict=mismatch, service=states, expected status == actual status (400), message only.

Cause 1 — states-creation-input-validation-errors#task#9, CreateStateMachine: `$.message: value_mismatch (expected 'Invalid State Machine Definition: \'INVALID_JSON_DESCRIPTION: Unexpected character (\'n\' (code 110)): was expecting double-quote to start field name\n at [Source: (String)"{not valid asl"; line: 1, column: 2] at [Source: (String)"{not valid asl"; line: 1, column: 2]\'', got "Invalid State Machine Definition: 'INVALID_JSON_DESCRIPTION: the definition is not valid JSON: Unexpected character ('n' (code 110)): was expecting double-quote to start field name'")`. The definition text `{not valid asl` and the line/column 1/2 are this fixture's input, not part of the template.
Cause 2 — states-state-machine-lifecycle-crud#task#8, UpdateStateMachine: `$.message: value_mismatch (expected 'Either the definition, the role ARN, the LoggingConfiguration, or the TracingConfiguration must be specified', got 'either definition or roleArn must be given to update a state machine')`.
Cause 3 — stepfunctions-alias-invalid-routing-weights#task#5, CreateStateMachineAlias: `$.message: value_mismatch (expected 'Sum of routing configuration weights must equal 100. Received: [42]', got 'the weights of a routing configuration must add up to 100, got 42')`. The value 42 is this fixture's request weight.
Cause 4 — sfn-validation-reason-labeled-arn-and-completed-map-run#task#8, UpdateMapRun: `$.message: value_mismatch (expected 'Cannot update completed MapRun <mapRunArn>', got 'Cannot update a completed Map Run')`. The ARN is this run's generated value.

## Steps to Reproduce
Cause 1: sfn:CreateStateMachine(name=<n>, roleArn=<role>, definition='{not valid asl'). Expect 400/InvalidDefinition with `Invalid State Machine Definition: 'INVALID_JSON_DESCRIPTION: <parser message>\n at [Source: (String)"<definition>"; line: L, column: C] at [Source: (String)"<definition>"; line: L, column: C]'`; observed the emulator's reworded message with no source location.
Cause 2: sfn:UpdateStateMachine(stateMachineArn=<sm>) with no definition, roleArn, loggingConfiguration or tracingConfiguration. Expect 400 with "Either the definition, the role ARN, the LoggingConfiguration, or the TracingConfiguration must be specified". Positive control: sfn:UpdateStateMachine with ONLY loggingConfiguration must succeed.
Cause 3: sfn:CreateStateMachineAlias(name=<a>, routingConfiguration=[{stateMachineVersionArn:<v1>, weight:42}]). Expect 400 with "Sum of routing configuration weights must equal 100. Received: [42]".
Cause 4: 1) run an execution containing a DISTRIBUTED Map to completion; 2) sfn:ListMapRuns to get its mapRunArn; 3) sfn:UpdateMapRun(mapRunArn=<it>, maxConcurrency=1). Expect 400 with "Cannot update completed MapRun <mapRunArn>".
Replay: the fixture#step tokens in Issue-Trace/Issue-Step and trace_ids.
