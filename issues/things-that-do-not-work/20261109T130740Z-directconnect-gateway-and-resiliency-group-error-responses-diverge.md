Issue-Source: trace-sweep-divergence
Issue-Trace: direct-connect-validation-and-errors
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: direct-connect-validation-and-errors
Issue-Shape-Id: com.amazonaws.directconnect#OvertureService
Issue-Key: directconnect|DescribeDirectConnectGateways|2xx->4xx|$.directConnectGateways
Issue-Key: directconnect|DescribeDirectConnectGateways|2xx->4xx|status
Issue-Key: directconnect|CreateDirectConnectGateway|4xx->4xx|$.message
Issue-Key: directconnect|CreateResiliencyGroup|4xx->4xx|$.message
Issue-Commit: eaa123f65a60b02b9ee1461dec055a780335fce9

# directconnect: gateway and resiliency-group error responses diverge from AWS status and message wording

> _Note: this issue was identified against the code base at commit `eaa123f65a60b02b9ee1461dec055a780335fce9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDirectConnectGateways

## Description
The emulator's directconnect validation/error path diverges from AWS three ways: one read that AWS answers successfully is rejected with a 400, and two 400 bodies are worded differently from AWS. This bundle has 3 causes; ALL THREE must be fixed before it is closed.

Cause 1 of 3: DescribeDirectConnectGateways answers 400 where AWS answers 200 with an empty list (fixture direct-connect-validation-and-errors, step task#3).
AWS: Smithy: DescribeDirectConnectGateways input members directConnectGatewayId, maxResults, nextToken are all OPTIONAL and the operation models only DirectConnectClientException (400) and DirectConnectServerException (500); a describe filtered by a gateway id that matches nothing is not an error. The recording shows AWS returning HTTP 200 with directConnectGateways present and EMPTY ([]) for this request.
Emulator: returns HTTP 400 and no directConnectGateways member at all.
Why: 'describe to see whether it exists' is the standard existence probe. A client that reads an empty list as 'absent' instead sees a thrown DirectConnectClientException, so create-if-missing and delete-then-verify flows abort rather than converge, and a non-fatal empty result is surfaced as a client fault.
Fix: DescribeDirectConnectGateways' input validation / filter handling - a well-formed request whose directConnectGatewayId selects nothing must fall through to the normal 200 path and serialize an empty directConnectGateways list (never omit the member), not raise a client exception. Hypothesis: the handler treats an unmatched gateway-id filter as a validation failure instead of as an empty selection.
Traces: direct-connect-validation-and-errors#task#3.

Cause 2 of 3: CreateDirectConnectGateway's invalid-Amazon-side-ASN message is re-worded (fixture direct-connect-validation-and-errors, step task#1).
AWS: Smithy: CreateDirectConnectGateway.amazonSideAsn targets LongAsn (no @range in the model) and the operation's client fault is DirectConnectClientException, httpError 400 - so the message text is recorded behavior only. The recording shows status 400 with the message '<asn> is not a valid value for Amazon Side ASN'. Fixed text: ' is not a valid value for Amazon Side ASN'; the single variable part is the rejected amazonSideAsn value copied from the request (the recorded '12345' is that run's input, not part of the template).
Emulator: same 400 status, but the message is 'amazonSideAsn <value> is not valid; the ASN must be in the private range of 64,512 to 65,534 or 4,200,000,000 to 4,294,967,294' - a different template that prefixes the member name and appends a range explanation AWS does not send.
Why: error messages are the only discriminator DirectConnectClientException offers, since every client fault shares one shape and one 400 status. Clients and test suites that match on AWS's phrasing to distinguish a bad ASN from any other client fault fail to classify the error.
Fix: the amazonSideAsn validation in CreateDirectConnectGateway's handler - emit AWS's message template with the request's ASN value interpolated.
Traces: direct-connect-validation-and-errors#task#1.

Cause 3 of 3: CreateResiliencyGroup's enum constraint message lists the value set in the wrong order (fixture direct-connect-validation-and-errors, step task#8).
AWS: Smithy: CreateResiliencyGroup.intendedResiliencyModel is required and targets the ResiliencyModel enum; DirectConnectClientException is the 400 client fault. The recording shows the standard framework constraint message "1 validation error detected: Value '<supplied>' at 'intendedResiliencyModel' failed to satisfy constraint: Member must satisfy enum value set: [basic-resiliency, high-resiliency, maximum-resiliency]". Fixed text: the '1 validation error detected: Value ... at ... failed to satisfy constraint: Member must satisfy enum value set: [...]' frame AND the ORDER of the enum value set, which AWS renders ascending (basic-resiliency, high-resiliency, maximum-resiliency) rather than in model declaration order; the variable parts are the rejected value and the member name, both taken from the request.
Emulator: same 400 status and frame, but renders the set as [maximum-resiliency, high-resiliency, basic-resiliency] - the model's declaration order, reversed relative to AWS.
Why: this message is a documented, byte-stable contract that clients and golden-file tests assert on verbatim; a differently ordered value set makes an otherwise correct rejection compare unequal, and makes the emulator's constraint text unusable as a reference for the allowed values.
Fix: the enum-constraint message formatter used by CreateResiliencyGroup's input validation - sort the enum value set the way AWS renders it instead of emitting model order.
Traces: direct-connect-validation-and-errors#task#8.

Unrelated open issues on this service that do NOT cover these: the DescribeLocations availableProviders issue and the CreateDirectConnectGatewayAssociation stub issue.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout.

Cause 1 - direct-connect-validation-and-errors#task#3, DescribeDirectConnectGateways:
  status: expected 200, got 400
  $.directConnectGateways: missing (expected [], got None)

Cause 2 - direct-connect-validation-and-errors#task#1, CreateDirectConnectGateway, expected 400 / actual 400:
  $.message: value_mismatch
    expected: '12345 is not a valid value for Amazon Side ASN'
    actual:   'amazonSideAsn 12345 is not valid; the ASN must be in the private range of 64,512 to 65,534 or 4,200,000,000 to 4,294,967,294'
  (12345 is the request's amazonSideAsn; the fixed text is '<asn> is not a valid value for Amazon Side ASN')

Cause 3 - direct-connect-validation-and-errors#task#8, CreateResiliencyGroup, expected 400 / actual 400:
  $.message: value_mismatch
    expected: "1 validation error detected: Value 'superfast-resiliency' at 'intendedResiliencyModel' failed to satisfy constraint: Member must satisfy enum value set: [basic-resiliency, high-resiliency, maximum-resiliency]"
    actual:   "1 validation error detected: Value 'superfast-resiliency' at 'intendedResiliencyModel' failed to satisfy constraint: Member must satisfy enum value set: [maximum-resiliency, high-resiliency, basic-resiliency]"
  (only the value-set ORDER differs; 'superfast-resiliency' is the request's value)

## Steps to Reproduce
1. Replay fixture direct-connect-validation-and-errors against the emulator.
2. Step task#3: DescribeDirectConnectGateways as recorded (a describe whose gateway-id filter selects nothing) - the emulator answers 400 with no directConnectGateways member instead of 200 with directConnectGateways: [].
3. Step task#1: CreateDirectConnectGateway with amazonSideAsn outside the valid ASN set (e.g. 12345) - status is 400 as expected but $.message uses the emulator's own wording instead of '<asn> is not a valid value for Amazon Side ASN'.
4. Step task#8: CreateResiliencyGroup with intendedResiliencyModel set to a value outside the ResiliencyModel enum - status 400, but $.message lists the enum value set in declaration order [maximum-resiliency, high-resiliency, basic-resiliency] instead of AWS's [basic-resiliency, high-resiliency, maximum-resiliency].
