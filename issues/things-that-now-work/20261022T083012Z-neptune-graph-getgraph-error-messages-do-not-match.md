Issue-Source: trace-sweep-divergence
Issue-Trace: neptune-graph-get-graph-invalid-identifier-pattern-rejected
Issue-Step: task#1
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: neptune-graph-get-graph-invalid-identifier-pattern-rejected
Issue-Shape-Id: com.amazonaws.neptunegraph#AmazonNeptuneGraph
Issue-Key: neptune-graph|GetGraph|4xx->4xx|$.message
Issue-Commit: ed70707e5c9f5d41c1939f41ff8fd3bb3f1df054

# neptune-graph GetGraph error messages do not match AWS wording

> _Note: this issue was identified against the code base at commit `ed70707e5c9f5d41c1939f41ff8fd3bb3f1df054`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetGraph

## Description
GetGraph returns the correct HTTP status and error shape in both of the failure paths the corpus exercises, but it composes the `message` member of both modeled errors with its own wording instead of the formats AWS builds from the request. This issue carries 2 causes; both must be closed.

Cause 1 of 2: ValidationException message for a graphIdentifier that fails its pattern (fixture neptune-graph-get-graph-invalid-identifier-pattern-rejected, step task#1).
AWS: GetGraph binds `graphIdentifier` as an @httpLabel with @required and @pattern (Smithy: GetGraph.graphIdentifier -> GraphIdentifier, pattern "^g-[a-z0-9]{10}$"; modeled error ValidationException, httpError 400, message @required, doc "A resource could not be validated."). The recorded response is 400 ValidationException with a message assembled from the request — the count of violations, the rejected value, the member path and the constraint — and the constraint is rendered as the BARE pattern with the anchors stripped: "1 validation error detected: Value 'not-a-valid-id' at 'graphIdentifier' failed to satisfy constraint: Member must satisfy regular expression pattern: g-[a-z0-9]{10}". The rule is therefore known: the pattern is echoed without its leading ^ and trailing $.
Emulator: answers 400 with the correct shape but echoes the model's pattern literally, anchors included: "... Member must satisfy regular expression pattern: ^g-[a-z0-9]{10}$".
Why: clients and tests that surface or match on the constraint text (and anything that lifts the pattern out of the message to re-validate input client-side) see a regex the service never emits; the anchors make a literal comparison against the AWS-rendered message fail for every pattern-constrained member, not just this one.
Fix: the shared constraint-violation message builder used by the neptune-graph input validation for GetGraph — strip the anchors when rendering a @pattern into the "Member must satisfy regular expression pattern:" clause, rather than hard-coding the message in the handler.
Traces: neptune-graph-get-graph-invalid-identifier-pattern-rejected#task#1.

Cause 2 of 2: ResourceNotFoundException message for a well-formed but absent graph id (same fixture, step task#2).
AWS: Smithy: GetGraph error ResourceNotFoundException, httpError 404, message @required, doc "A specified resource could not be located." The model does not fix the text, so recorded behavior only: the recording shows the message built from the caller's account and the requested identifier, in the form "No graph found for customer [<accountId>] with graph id [<graphIdentifier>]" — recorded example "No graph found for customer [000000000000] with graph id [g-0000000000]".
Emulator: answers 404 with the correct shape but a message of its own: "graph g-0000000000 not found" — the account is absent and the bracketed template is not used.
Why: a client that reports or logs the service's not-found text loses the account scoping AWS includes, so a lookup failure caused by calling with the wrong account/partition is indistinguishable from a genuinely deleted graph.
Fix: the not-found path of the GetGraph handler (and the shared graph-lookup error construction it calls), which should format the message from the request's graphIdentifier and the resolved caller account rather than from a literal.
Traces: neptune-graph-get-graph-invalid-identifier-pattern-rejected#task#2.

Note: hosts, ports, generated ids and the specific account used are environment detail; the requirement in each cause is the message FORM, built from the request, not the recorded literal.

## Evidence
Replay of trace set simparity-wire-aws-v1 (2 divergence clusters, both neptune-graph GetGraph, both status-matched and message-mismatched):

Cause 1 — fixture neptune-graph-get-graph-invalid-identifier-pattern-rejected, step task#1, verdict mismatch, expected_status 400, actual_status 400, divergence_key service=neptune-graph|GetGraph|status=400->400|keys=$.message:
  expected $.message = "1 validation error detected: Value 'not-a-valid-id' at 'graphIdentifier' failed to satisfy constraint: Member must satisfy regular expression pattern: g-[a-z0-9]{10}"
  actual   $.message = "1 validation error detected: Value 'not-a-valid-id' at 'graphIdentifier' failed to satisfy constraint: Member must satisfy regular expression pattern: ^g-[a-z0-9]{10}$"
  (differs only by the anchors ^ ... $ around the echoed pattern)

Cause 2 — same fixture, step task#2, verdict mismatch, expected_status 404, actual_status 404, divergence_key service=neptune-graph|GetGraph|status=404->404|keys=$.message:
  expected $.message = "No graph found for customer [000000000000] with graph id [g-0000000000]"
  actual   $.message = "graph g-0000000000 not found"

Model evidence (services/service-com-amazonaws-neptunegraph-amazonneptunegraph/smithy-model/com-amazonaws-neptunegraph-amazonneptunegraph.json): GetGraph GET /graphs/{graphIdentifier}; graphIdentifier -> GraphIdentifier with pattern ^g-[a-z0-9]{10}$; errors ValidationException(400), ResourceNotFoundException(404), ThrottlingException(429), InternalServerException(500); both error shapes have a @required `message`.

Status codes and error shapes are correct in both cases — only the message composition diverges. Replay summary: steps_total 6, steps_passed 0, steps_mismatched 6, steps_diverged 2, state_reset_failures 0, bootstrap_signal false.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay fixture neptune-graph-get-graph-invalid-identifier-pattern-rejected from trace set simparity-wire-aws-v1.
3. Step task#1 — GET /graphs/not-a-valid-id (GetGraph with a graphIdentifier that violates ^g-[a-z0-9]{10}$): the emulator returns 400 ValidationException, but $.message echoes the pattern with its anchors ("^g-[a-z0-9]{10}$") where AWS echoes it bare ("g-[a-z0-9]{10}").
4. Step task#2 — GET /graphs/g-0000000000 (GetGraph for a well-formed id that does not exist): the emulator returns 404 ResourceNotFoundException, but $.message is "graph g-0000000000 not found" instead of AWS's account-scoped form "No graph found for customer [<accountId>] with graph id [<graphIdentifier>]".
5. Both steps re-run after the fix must match on $.message as well as status.
