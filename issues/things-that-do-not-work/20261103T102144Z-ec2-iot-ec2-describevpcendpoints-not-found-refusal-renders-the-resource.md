Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__iot.iot-dangling-sourcevpce-reference-accepted-over-permissive
Issue-Key: ec2__iot.iot-dangling-sourcevpce-reference-accepted-over-permissive|run#10|AssertionError|assert message == not_found, f"DescribeVpcEndpoints for {_DANGLING!r} said {message!r}"
Issue-Commit: eea73b787e75fa69d7e1f25d97c4dc355e67b336

# EC2 DescribeVpcEndpoints not-found refusal renders the resource noun as 'VPC endpoint ID' instead of AWS's 'Vpc Endpoint Id'

> _Note: this issue was identified against the code base at commit `eea73b787e75fa69d7e1f25d97c4dc355e67b336`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeVpcEndpoints

## Description
One root cause, in the EC2 id-lookup refusal text for DescribeVpcEndpoints. (The failing check is listed under the suite heading `ec2__iot`, but the diverging call is EC2's DescribeVpcEndpoints — EC2 is bootstrapped and answering, it answers with the wrong sentence. The check never reached its IoT half: it failed on the EC2 precondition that establishes the endpoint id is dangling.)

Cause 1 of 1: the not-found sentence for a well-formed but nonexistent VPC endpoint id.

AWS: recorded behavior only — the EC2 Smithy model declares no errors for DescribeVpcEndpoints (the generated `DescribeVpcEndpointsErrorModeled` enum is uninhabited; EC2 uses the ec2 query protocol and carries its refusals in `$.Response.Errors.Error`), so the contract here is the recorded refusal. For a `VpcEndpointIds` entry that is syntactically a VPC endpoint id (`vpce-` + 17 hex chars) but names no endpoint of the account, AWS answers HTTP 400 with Code `InvalidVpcEndpointId.NotFound` and Message `The Vpc Endpoint Id '<id>' does not exist`. Fixed text: `The Vpc Endpoint Id '` ... `' does not exist` (that exact title-cased noun, single quotes, no trailing period); the only variable part is the id, taken verbatim from the request's `VpcEndpointIds` member. The same AWS sentence is already recorded, for a different input, in the open issue 20261103T071133Z-ec2-confuses-malformed-and-not-found-ids-and.md (its cause 7 quotes AWS as "The Vpc Endpoint Id 'vpce-123' does not exist"); that issue describes the Malformed/NotFound class inversion for this operation and does not cover this residual wording divergence, so it is named here rather than relied on.

Emulator: HTTP 400 with the right code, `InvalidVpcEndpointId.NotFound`, but the Message is `The VPC endpoint ID '<id>' does not exist` — the resource noun is rendered `VPC endpoint ID` (upper-case acronym, upper-case ID) where AWS renders `Vpc Endpoint Id`. Everything else about the sentence (quoting, no trailing period, the id substituted from the request) already matches.

Why: the refusal message is part of the contract. A client, an operator runbook or a test that distinguishes a dangling endpoint reference by the sentence AWS words — and anything that string-matches the refusal to decide whether to re-resolve an id or to treat the reference as permanently gone — sees text no AWS region produces. Here it also blocks everything behind the probe: the check establishes that an id provably does not resolve before using it as an `aws:SourceVpce` condition value, so an unrelated IoT policy assertion is never reached.

Fix: the not-found refusal text of the operation's handler. `services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_vpc_endpoints.rs` builds it at lines 268-272 by calling `state::id_not_found(state::INVALID_VPC_ENDPOINT_ID_NOT_FOUND, "VPC endpoint ID", unknown)`; the noun passed at that call site (and, if the shared `crate::state::id_not_found` template is to stay generic, the template's rendering) must produce AWS's wording for this resource family. The code class and status are already right and must not change.

Traces: ec2__iot.iot-dangling-sourcevpce-reference-accepted-over-permissive.

## Evidence
Behavioral check `ec2__iot.iot-dangling-sourcevpce-reference-accepted-over-permissive` [mismatch].

Request: `ec2.describe_vpc_endpoints(VpcEndpointIds=['vpce-00000000000000000'])` (a syntactically valid id that names no endpoint).

Expected (AWS): HTTP 400, Error.Code `InvalidVpcEndpointId.NotFound`, Error.Message `The Vpc Endpoint Id 'vpce-00000000000000000' does not exist`.
Actual (emulator): HTTP 400, Error.Code `InvalidVpcEndpointId.NotFound` (the code assertion passed), Error.Message `The VPC endpoint ID 'vpce-00000000000000000' does not exist`.

Replay assertion that failed: `AssertionError: DescribeVpcEndpoints for 'vpce-00000000000000000' said "The VPC endpoint ID 'vpce-00000000000000000' does not exist"` (check source line 61, comparing against `f"The Vpc Endpoint Id '{_DANGLING}' does not exist"`).

Source of the emulator sentence: services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_vpc_endpoints.rs:268-272 — `state::id_not_found(state::INVALID_VPC_ENDPOINT_ID_NOT_FOUND, "VPC endpoint ID", unknown)`. Only the noun diverges; the id is substituted from the request, as AWS does.

## Steps to Reproduce
1. Replay behavioral check `ec2__iot.iot-dangling-sourcevpce-reference-accepted-over-permissive`.
2. Or directly: call `ec2:DescribeVpcEndpoints` with `VpcEndpointIds=['vpce-00000000000000000']` (any well-formed `vpce-` + 17 hex-character id that no endpoint in the store carries).
3. Observe the 400 refusal: the code is `InvalidVpcEndpointId.NotFound` as AWS returns, but the message reads `The VPC endpoint ID '<id>' does not exist` instead of AWS's `The Vpc Endpoint Id '<id>' does not exist`.
4. Repeat with any other absent-but-well-formed endpoint id to confirm the divergence is in the fixed noun, not in the substituted id.
