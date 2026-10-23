Issue-Source: trace-sweep-divergence
Issue-Trace: lambda-core-create-network-connector-invalid-parameters
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lambda-core-create-network-connector-invalid-parameters lambda-core-list-network-connectors-ambient
Issue-Shape-Id: com.amazonaws.lambdacore#LambdaCoreApiService
Issue-Key: lambda|CreateNetworkConnector|4xx->4xx|$.message
Issue-Key: lambda|ListNetworkConnectors|2xx->2xx|$.NetworkConnectors[Arn="arn
Issue-Commit: 946f42f6a143eaa9f4f23faa899ea5f11b0d1603

# lambda network-connector wire fidelity: anchored regex echoed in validation message and LastModified serialized as epoch number

> _Note: this issue was identified against the code base at commit `946f42f6a143eaa9f4f23faa899ea5f11b0d1603`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateNetworkConnector

## Description
Two net-new wire divergences on the lambdacore network-connector surface, recorded under the `lambda` service token. Both are serialization defects: the emulator chooses the right status and error code but renders a value in a form AWS never emits.

Cause 1 of 2 — CreateNetworkConnector validation message echoes the anchored @pattern.
AWS: Smithy (com.amazonaws.lambdacore) CreateNetworkConnector.Name -> NetworkConnectorName, required, @length min 1 max 140, @pattern "^(arn:aws[a-zA-Z-]*:lambda:(eusc-)?[a-z]{2}((-gov)|(-iso([a-z]?)))?-[a-z]+-\\d{1}:\\d{12}:network-connector:[a-zA-Z0-9-_]+(:[1-9][0-9]*)?)|[a-zA-Z0-9_-]{1,64}$"; modeled error InvalidParameterValueException (errorKind client, httpError 400). A name that violates the pattern is refused with HTTP 400 / InvalidParameterValueException, and the message is request-derived: "1 validation error detected: Value '<supplied value>' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>" where <pattern> is the modeled pattern with the leading ^ and the trailing $ anchors stripped. The recorded response confirms that rule (anchorless pattern text).
Emulator: same 400 and same error code, but the pattern it quotes keeps the anchors — it prints the raw Smithy trait value "^(arn:...)|[a-zA-Z0-9_-]{1,64}$".
Why: clients and test suites that assert on the constraint reported for a rejected member, and tooling that reconstructs the service's published pattern from the error, read a pattern AWS never publishes; the anchored form is also not interchangeable when re-applied, since the alternation makes anchor placement semantically significant.
Fix: the shared validation-message formatter used by this operation's input validation — strip leading ^ / trailing $ when rendering a @pattern into a constraint message.
Traces: lambda-core-create-network-connector-invalid-parameters#task#1 (cluster also covers #task#2).

Cause 2 of 2 — ListNetworkConnectors renders LastModified as an epoch number instead of a date-time string.
AWS: Smithy ListNetworkConnectors output NetworkConnectors -> NetworkConnectorsList, whose summary carries LastModified -> CoreTimestamp with @timestampFormat "date-time". Under the service's REST-JSON protocol a date-time timestamp is serialized as an ISO-8601 string (for example "2026-04-04T12:00:00.000Z"); the fixture asserts the shape of that member rather than a fixed instant.
Emulator: emits a bare JSON number (example from the replay: 1792721676.8408382), i.e. epoch-seconds-with-fraction, for NetworkConnectors[].LastModified.
Why: a generated SDK deserializer bound to @timestampFormat date-time expects a JSON string for this member; a number is a protocol type violation that either fails deserialization or silently yields a wrong instant, so every consumer of ListNetworkConnectors sees a broken LastModified.
Fix: the ListNetworkConnectors response serializer (the network-connector summary projection) must honour the member's date-time timestamp format rather than emitting the internal epoch float.
Traces: lambda-core-list-network-connectors-ambient#task#9 (cluster also covers #task#10).

No open issue covers either cause for this service. The anchored-pattern fault is the same class already filed for other services (20261023T004131Z-bedrock-agentcore-control-validation-error-messages-quote-the-anchored-regex-pattern.md and 20261023T004257Z-networkmonitor-validation-error-messages-diverge-from-aws-anchored.md); those are different services and do not cover this one.

## Evidence
Cause 1 (fixture lambda-core-create-network-connector-invalid-parameters, task#1, status 400 expected and 400 actual, key $.message):
  expected: "1 validation error detected: Value 'bad name has spaces & symbols!!' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: (arn:aws[a-zA-Z-]*:lambda:(eusc-)?[a-z]{2}((-gov)|(-iso([a-z]?)))?-[a-z]+-\d{1}:\d{12}:network-connector:[a-zA-Z0-9-_]+(:[1-9][0-9]*)?)|[a-zA-Z0-9_-]{1,64}"
  actual:   same text but the pattern is wrapped as "^(arn:...)|[a-zA-Z0-9_-]{1,64}$"

Cause 2 (fixture lambda-core-list-network-connectors-ambient, task#9, status 200 expected and 200 actual):
  $.NetworkConnectors[Arn="...:network-connector:sim2real-lnc-..."].LastModified: expected a date-time-shaped value ('<shaped-not-asserted>'), actual 1792721676.8408382 (bare JSON number)

## Steps to Reproduce
1. Replay the wire corpus simparity-wire-aws-v1 against the emulator (replay_wire_corpus).
2. Cause 1: POST /2026-04-04/network-connectors with Name="bad name has spaces & symbols!!" and a valid Configuration (fixture lambda-core-create-network-connector-invalid-parameters, step task#1). Observe 400 InvalidParameterValueException whose $.message quotes the pattern with ^ and $ still attached.
3. Cause 2: create a network connector, then GET /2026-04-04/network-connectors (fixture lambda-core-list-network-connectors-ambient, step task#9) and inspect NetworkConnectors[].LastModified: it is a JSON number rather than an ISO-8601 date-time string.
