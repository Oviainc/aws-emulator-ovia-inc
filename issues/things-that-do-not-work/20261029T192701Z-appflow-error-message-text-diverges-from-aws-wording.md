Issue-Source: trace-sweep-divergence
Issue-Trace: appflow-connector-profile-error-paths
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appflow-connector-profile-error-paths appflow-create-flow-nonexistent-connector-profile-rejected
Issue-Shape-Id: com.amazonaws.appflow#SandstoneConfigurationServiceLambda
Issue-Key: appflow|CreateConnectorProfile|4xx->4xx|$.message
Issue-Key: appflow|DescribeFlow|4xx->4xx|$.message
Issue-Commit: 2216fbd2f00c4c2b6b2036585ad12df5e43cbdbf

# AppFlow error message text diverges from AWS wording

> _Note: this issue was identified against the code base at commit `2216fbd2f00c4c2b6b2036585ad12df5e43cbdbf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateConnectorProfile

## Description
Two AppFlow error paths return the correct HTTP status and error shape but a message whose text is not what AWS emits. The shared fault is message construction: the emulator writes its own prose instead of the AWS message template for that error class. 2 causes.

Cause 1 of 2: CreateConnectorProfile pattern-constraint ValidationException message renders the regex with anchors.
AWS: Smithy: CreateConnectorProfile.connectorProfileName is @required, @length min 0 max 256, @pattern "^[\\w/!@#+=.-]+$"; modeled error ValidationException (httpError 400, "The request has invalid or missing parameters."). A value that does not satisfy the pattern is rejected with the standard constraint-violation message, whose FIXED text is "<n> validation error detected: Value '<value>' at '<memberName>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>" and whose VARIABLE parts are the error count, the rejected value from the request, the member name, and the pattern. The pattern is rendered WITHOUT the leading ^ and trailing $ anchors, i.e. "[\\w/!@#+=.-]+" for this member (recorded response: 400, message "1 validation error detected: Value '<name>' at 'connectorProfileName' failed to satisfy constraint: Member must satisfy regular expression pattern: [\\w/!@#+=.-]+").
Emulator: same 400 and same template, but it interpolates the raw modeled pattern including the anchors: "... Member must satisfy regular expression pattern: ^[\\w/!@#+=.-]+$".
Why: a client that asserts on the documented constraint message, or that lifts the pattern out of the message to re-validate input client-side, gets a different (anchored) regex string than AWS ever sends; message equality checks and pattern round-trips fail even though the request was correctly rejected.
Fix: the appflow CreateConnectorProfile input-validation layer / the shared constraint-violation message formatter that renders an @pattern trait into the "Member must satisfy regular expression pattern" clause - it must emit the pattern with the ^ and $ anchors stripped.
Traces: appflow-connector-profile-error-paths#task#4.

Cause 2 of 2: DescribeFlow not-found message uses emulator prose instead of the AWS sentence.
AWS: Smithy: DescribeFlow has modeled errors InternalServerException (500) and ResourceNotFoundException (404, "The resource specified in the request (such as the source or destination connector profile) is not found."); ResourceNotFoundException carries one member, message. Recorded behavior only for the text: AWS answers 404 with the fixed text "There is no flow with the name " followed by the flowName taken verbatim from the request (recorded: "There is no flow with the name <flowName>"). The only variable part is the flow name.
Emulator: also 404 / ResourceNotFoundException, but message "No flow named <flowName> exists in this account" - entirely different fixed text, with the flow name interpolated mid-sentence.
Why: clients and tests that match the AWS not-found sentence (or that parse the flow name out of its documented position) do not recognise the emulator's error, so a legitimately-absent flow reads as an unclassified failure; the status and code being right makes the divergence silent.
Fix: the appflow DescribeFlow handler's lookup-miss branch where the ResourceNotFoundException message is built.
Traces: appflow-create-flow-nonexistent-connector-profile-rejected#task#8.

## Evidence
Cause 1 (appflow-connector-profile-error-paths#task#4, CreateConnectorProfile): expected status 400, actual 400; $.message expected "1 validation error detected: Value '<name>' at 'connectorProfileName' failed to satisfy constraint: Member must satisfy regular expression pattern: [\\w/!@#+=.-]+", actual "... Member must satisfy regular expression pattern: ^[\\w/!@#+=.-]+$" (anchors present).
Cause 2 (appflow-create-flow-nonexistent-connector-profile-rejected#task#8, DescribeFlow): expected status 404, actual 404; $.message expected "There is no flow with the name <flowName>", actual "No flow named <flowName> exists in this account".
Model basis: Smithy CreateConnectorProfile.connectorProfileName @pattern ^[\\w/!@#+=.-]+$ with ValidationException(400); DescribeFlow errors ResourceNotFoundException(404).

## Steps to Reproduce
1. Boot the emulator and replay the wire corpus (replay_wire_corpus).
2. Cause 1: POST /create-connector-profile with a connectorProfileName that violates the @pattern (e.g. one containing a space and '!'). Observe the 400 ValidationException message: the regular-expression pattern it quotes carries the ^ and $ anchors, where AWS quotes it unanchored. Fixture appflow-connector-profile-error-paths, step task#4.
3. Cause 2: POST /describe-flow with a flowName that does not exist in the account. Observe the 404 ResourceNotFoundException message "No flow named <name> exists in this account" instead of AWS's "There is no flow with the name <name>". Fixture appflow-create-flow-nonexistent-connector-profile-rejected, step task#8.
