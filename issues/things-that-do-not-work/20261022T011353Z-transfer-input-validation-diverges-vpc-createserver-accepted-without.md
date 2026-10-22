Issue-Source: trace-sweep-divergence
Issue-Trace: transfer-server-delete-error-paths
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: transfer-server-delete-error-paths transfer-server-error-paths transfer-as2-certificate-and-profile-lifecycle
Issue-Shape-Id: com.amazonaws.transfer#TransferService
Issue-Key: transfer|CreateServer|4xx->2xx|$.Message
Issue-Key: transfer|CreateServer|4xx->2xx|$.__type
Issue-Key: transfer|CreateServer|4xx->2xx|status
Issue-Key: transfer|CreateProfile|4xx->4xx|$.message
Issue-Commit: 872e0ddf6cb614931de58689afe8393c02abd052

# Transfer input validation diverges: VPC CreateServer accepted without EndpointDetails, enum rejection renders the value

> _Note: this issue was identified against the code base at commit `872e0ddf6cb614931de58689afe8393c02abd052`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateServer

## Description
Transfer's request validation diverges from AWS in two places: a cross-member requirement is not enforced at all on CreateServer, and the enum-rejection message is built with a rendering AWS does not use. 2 divergence clusters. No open transfer issue covers either; the message-rendering defect is the same class as the already-open controlcatalog issue 'Validation error messages inject a rejected-value rendering AWS does not emit' (20261021T234107Z), which does not cover transfer.

Cause 1 of 2 — CreateServer with EndpointType VPC and no EndpointDetails is accepted.
AWS: Smithy: CreateServer input has EndpointType (enum, PUBLIC|VPC|VPC_ENDPOINT) and EndpointDetails, with InvalidRequestException (@error client, @httpError 400, required member Message) modeled on the operation and documented as 'thrown when the client submits a malformed request'; EndpointDetails is documented as the VPC endpoint settings required to host the endpoint in a VPC. Recorded behavior: requesting EndpointType VPC without EndpointDetails is refused with HTTP 400 and __type InvalidRequestException (recorded Message 'EndpointDetails required for VPC' — an example of the text; the requirement is the 400 and the error code, and that the refusal names the missing cross-member requirement).
Emulator: answers HTTP 200 and creates the server; no __type and no Message in the body.
Why: the emulator hands back a ServerId for a configuration AWS never creates, so negative tests pass against the emulator and the same template fails in production; worse, the fixture's subsequent steps operate on a server that cannot exist, so an entire class of VPC misconfiguration goes undetected.
Fix: the transfer CreateServer handler's input validation (cross-member check EndpointType==VPC implies EndpointDetails present) raising InvalidRequestException before the state write.
Traces: transfer-server-delete-error-paths#task#5, also transfer-server-error-paths#task#5.

Cause 2 of 2 — the enum-constraint validation message renders the rejected value and so does not follow AWS's construction.
AWS: Smithy: CreateProfile input ProfileType is required and targets enum ProfileType with values [LOCAL, PARTNER]. Recorded behavior: an unmodelled profileType is refused with HTTP 400 and a validation message of the form '1 validation error detected: Value at 'profileType' failed to satisfy constraint: Member must satisfy enum value set: [...]' — i.e. the message is built from the request by naming the member and its constraint and does NOT quote the rejected value back to the caller.
Emulator: same 400, but the message is '1 validation error detected: Value 'BOGUS' at 'profileType' failed to satisfy constraint: Member must satisfy enum value set: [LOCAL, PARTNER]' — it injects the rejected value after 'Value'. (The ordering of the enum members inside the bracketed set differs too, but no documented or model-stated rule fixes that order, so the order is not treated as a requirement here.)
Why: the rejected value is caller-controlled input echoed into an error message, which both differs from AWS's message construction and reflects unvalidated input back to clients and logs; clients asserting on AWS's message form mis-handle the refusal.
Fix: the shared validation-message builder used by the transfer handlers (the member/constraint renderer) must omit the rejected value for enum constraints.
Traces: transfer-as2-certificate-and-profile-lifecycle#task#12.

## Evidence
Cause 1: transfer-server-delete-error-paths#task#5 CreateServer — status expected 400, got 200; $.__type expected 'InvalidRequestException', got None; $.Message expected 'EndpointDetails required for VPC', got None (2 steps in cluster, also transfer-server-error-paths#task#5).
Cause 2: transfer-as2-certificate-and-profile-lifecycle#task#12 CreateProfile — status 400 on both sides; $.message expected "1 validation error detected: Value at 'profileType' failed to satisfy constraint: Member must satisfy enum value set: [PARTNER, LOCAL]", got "1 validation error detected: Value 'BOGUS' at 'profileType' failed to satisfy constraint: Member must satisfy enum value set: [LOCAL, PARTNER]" — the divergence relied on is the injected rejected value, not the bracket ordering.

## Steps to Reproduce
1. Call CreateServer with EndpointType='VPC' and no EndpointDetails -> emulator answers 200 with a ServerId; AWS recordings show 400 with __type InvalidRequestException (Cause 1).
2. Call CreateProfile with As2Id set and ProfileType='BOGUS' -> emulator answers 400 with a message that quotes 'BOGUS' back; AWS's recorded message names only the member and the constraint (Cause 2).
Replay: fixtures transfer-server-delete-error-paths (and transfer-server-error-paths) and transfer-as2-certificate-and-profile-lifecycle.
