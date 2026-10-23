Issue-Source: trace-sweep-divergence
Issue-Trace: lambda-core-list-network-connectors-ambient
Issue-Step: task#12
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lambda-core-list-network-connectors-ambient lambda-core-network-connector-not-found-errors lambdacore-absent-connector-envelopes lambdacore-create-validation-rejections
Issue-Shape-Id: com.amazonaws.lambdacore#LambdaCoreApiService
Issue-Key: lambda-core|GetNetworkConnector|4xx->4xx|$.Message
Issue-Key: lambda-core|UpdateNetworkConnector|4xx->4xx|$.Message
Issue-Key: lambda-core|DeleteNetworkConnector|4xx->4xx|$.Message
Issue-Key: lambda-core|CreateNetworkConnector|4xx->4xx|$.message
Issue-Commit: 946f42f6a143eaa9f4f23faa899ea5f11b0d1603

# lambda-core rejection messages diverge from AWS: not-found text names the wrong identifier and the constraint echo keeps regex anchors

> _Note: this issue was identified against the code base at commit `946f42f6a143eaa9f4f23faa899ea5f11b0d1603`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetNetworkConnector

## Description
Four net-new divergences in which lambda-core returns the correct HTTP status and the correct error code but a message whose request-derived content differs from what AWS produces. The messages here are requirements, not examples: each is built from the request (the identifier that was asked for, or the member and constraint that failed), and that is the rule that diverges.

Cause 1 of 4 — GetNetworkConnector not-found message does not name the identifier that was requested.
AWS: Smithy (com.amazonaws.lambdacore) GetNetworkConnector.Identifier -> NetworkConnectorIdentifier ("a flexible identifier that accepts a network connector ID, name, or ARN"), modeled error ResourceNotFoundException (httpError 404, member Message). A lookup of a connector that does not exist is refused 404 with a message built from the request: "Network connector not found: <the identifier exactly as supplied>" — in the recording the ARN that the caller passed.
Emulator: answers 404 ResourceNotFoundException, but its Message names a different identifier than the request carried — it echoes an internal connector id (recorded example: 'The network connector nc-0000000000000001 does not exist in this account and Region' for a request made by ARN), so the message does not identify the resource the caller asked for at all.
Why: a caller that looks up a connector by name or ARN cannot correlate the refusal with its own request, and log-scraping/diagnostic tooling is handed an identifier the caller never used.
Fix: the GetNetworkConnector handler's not-found path must format ResourceNotFoundException.Message from the request's Identifier, using AWS's "Network connector not found: <identifier>" form.
Traces: lambda-core-list-network-connectors-ambient#task#12 (cluster also covers lambda-core-network-connector-not-found-errors#task#1 and lambdacore-absent-connector-envelopes#task#1).

Cause 2 of 4 — UpdateNetworkConnector not-found message uses non-AWS wording.
AWS: UpdateNetworkConnector (PUT /2026-04-04/network-connectors/{Identifier}) models ResourceNotFoundException (httpError 404). For an absent connector AWS refuses 404 with the same request-derived form, "Network connector not found: <identifier>".
Emulator: 404 ResourceNotFoundException with 'The network connector <identifier> does not exist in this account and Region'.
Why: the message template is service-owned behavior; clients and conformance suites that assert the documented refusal text diverge on an otherwise-correct refusal.
Fix: the UpdateNetworkConnector handler's not-found path / the shared not-found message helper for this service.
Traces: lambda-core-network-connector-not-found-errors#task#2 (cluster also covers lambdacore-absent-connector-envelopes#task#2).

Cause 3 of 4 — DeleteNetworkConnector not-found message uses the same non-AWS wording.
AWS: DeleteNetworkConnector (DELETE /2026-04-04/network-connectors/{Identifier}) models ResourceNotFoundException (httpError 404); refusal message "Network connector not found: <identifier>".
Emulator: 404 ResourceNotFoundException with 'The network connector <identifier> does not exist in this account and Region'.
Why: same as cause 2 — one shared formatter is wrong, so every identifier-addressed operation reports a refusal AWS never phrases that way.
Fix: the DeleteNetworkConnector handler's not-found path / the shared helper.
Traces: lambda-core-network-connector-not-found-errors#task#3 (cluster also covers lambdacore-absent-connector-envelopes#task#3).

Cause 4 of 4 — CreateNetworkConnector constraint message echoes the anchored @pattern.
AWS: Smithy CreateNetworkConnector.Name -> NetworkConnectorName, @pattern "^(arn:aws[a-zA-Z-]*:lambda:...:network-connector:[a-zA-Z0-9-_]+(:[1-9][0-9]*)?)|[a-zA-Z0-9_-]{1,64}$"; error InvalidParameterValueException (httpError 400). The rejection message is built from the request and the constraint, quoting the modeled pattern with the leading ^ and trailing $ removed.
Emulator: correct 400 and code, but prints the trait value verbatim with the anchors intact.
Why: tooling that reads the published pattern out of the validation error gets a pattern AWS does not publish, and for this alternation the anchor placement changes what the pattern means.
Fix: the service's validation-message formatter (pattern rendering), shared with CreateNetworkConnector's input validation.
Traces: lambdacore-create-validation-rejections#task#1 (cluster also covers #task#3).

Related-but-not-covering: 20261023T000909Z-notificationscontacts-error-message-fidelity-divergences-in-validation-and.md and 20261023T004257Z-networkmonitor-validation-error-messages-diverge-from-aws-anchored.md are the same fault classes on other services.

## Evidence
Cause 1 — lambda-core-list-network-connectors-ambient#task#12, GetNetworkConnector, expected 404 / actual 404:
  $.Message expected 'Network connector not found: arn:aws:lambda:us-west-2:000000000000:network-connector:sim2real-lnc-cc84868f'
  $.Message actual   'The network connector nc-0000000000000001 does not exist in this account and Region'

Cause 2 — lambda-core-network-connector-not-found-errors#task#2, UpdateNetworkConnector, expected 404 / actual 404:
  $.Message expected 'Network connector not found: sim2real-probe-lambda-core-missing-connector-4d303a72'
  $.Message actual   'The network connector sim2real-probe-lambda-core-missing-connector-4d303a72 does not exist in this account and Region'

Cause 3 — lambda-core-network-connector-not-found-errors#task#3, DeleteNetworkConnector, expected 404 / actual 404: identical expected/actual message divergence as cause 2.

Cause 4 — lambdacore-create-validation-rejections#task#1, CreateNetworkConnector, expected 400 / actual 400:
  $.message expected "1 validation error detected: Value 'sim2real probe bad!' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: (arn:aws[a-zA-Z-]*:lambda:...)|[a-zA-Z0-9_-]{1,64}"
  $.message actual   same text with the pattern rendered as "^(arn:aws[a-zA-Z-]*:lambda:...)|[a-zA-Z0-9_-]{1,64}$"

## Steps to Reproduce
1. Replay the wire corpus simparity-wire-aws-v1 (replay_wire_corpus).
2. Cause 1: create a connector, delete it, then GET /2026-04-04/network-connectors/{its ARN} (fixture lambda-core-list-network-connectors-ambient, step task#12) — the 404 Message names an internal nc-... id instead of the ARN requested.
3. Causes 2 and 3: PUT and DELETE /2026-04-04/network-connectors/sim2real-probe-lambda-core-missing-connector-4d303a72 (fixture lambda-core-network-connector-not-found-errors, steps task#2 and task#3) — 404 with the 'does not exist in this account and Region' wording.
4. Cause 4: POST /2026-04-04/network-connectors with Name='sim2real probe bad!' (fixture lambdacore-create-validation-rejections, step task#1) — 400 whose message quotes the anchored pattern.
