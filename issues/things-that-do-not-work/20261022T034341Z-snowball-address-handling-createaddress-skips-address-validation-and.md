Issue-Source: trace-sweep-divergence
Issue-Trace: snowball-address-create-describe-round-trip
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: snowball-address-create-describe-round-trip snowball-create-address-invalid-rejected
Issue-Shape-Id: com.amazonaws.snowball#AWSIESnowballJobManagementService
Issue-Key: snowball|DescribeAddress|2xx->2xx|$.Address.Type
Issue-Key: snowball|CreateAddress|4xx->2xx|$.Message
Issue-Key: snowball|CreateAddress|4xx->2xx|$.__type
Issue-Key: snowball|CreateAddress|4xx->2xx|status
Issue-Key: snowball|CreateAddress|4xx->2xx|$.message
Issue-Commit: 37e4b54b7f2f231778aa0090f22742466dbbad1b

# Snowball address handling: CreateAddress skips address validation and DescribeAddress drops Address.Type

> _Note: this issue was identified against the code base at commit `37e4b54b7f2f231778aa0090f22742466dbbad1b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAddress

## Description
Three distinct causes in the snowball emulator's address surface, all replayed from the wire corpus. The common fault: the Address structure is neither validated on CreateAddress nor fully persisted/echoed on DescribeAddress — CreateAddress behaves as an unconditional 200 'accept everything' and the stored Address loses a modelled member.

Cause 1 of 3 — DescribeAddress omits Address.Type (fixture snowball-address-create-describe-round-trip, step task#2; also affects snowball-create-address-invalid-rejected#task#2).
AWS: Smithy: DescribeAddress output member Address targets com.amazonaws.snowball#Address, whose member Type targets the enum AddressType with values CUST_PICKUP | AWS_SHIP ('Differentiates between delivery address and pickup address in the customer account. Provided at job creation.'). The recording shows a CreateAddress whose Address carried Type, and the subsequent DescribeAddress returning 200 with $.Address.Type = 'AWS_SHIP' — i.e. the member supplied at creation is round-tripped back.
Emulator: returns 200 but $.Address.Type is absent from the response body entirely (expected 'AWS_SHIP', got None); every other Address member matched.
Why: a client that reads Address.Type to distinguish a pickup address from a delivery address sees it as unset for every address it created, so pickup/ship logic silently takes the wrong branch and round-trip comparisons of a created address against the described one never match.
Fix: the DescribeAddress handler's state/serializer path for snowball — the Address record must store the Type member written by CreateAddress and the serializer must emit it when present.

Cause 2 of 3 — CreateAddress accepts an incomplete address instead of raising InvalidAddressException (fixture snowball-create-address-invalid-rejected, step task#3).
AWS: Smithy: CreateAddress declares errors InvalidAddressException and UnsupportedAddressException, both client errors with httpError 400. InvalidAddressException is documented 'The address provided was invalid. Check the address with your region's carrier, and try again.', and the Address shape documents 'Addresses are validated at the time of creation ... if the address is invalid or unsupported, then an exception is thrown.' The recording for an Address missing its first street line is HTTP 400 with $.__type = 'InvalidAddressException' (example message: 'You must specify Address 1 in the address.', built from the missing member, so the requirement is that the error names the missing address line).
Emulator: HTTP 200 with a normal CreateAddressResult; no $.__type, no Message.
Why: the client's address is never rejected, so it receives an AddressId for an undeliverable address and the failure surfaces only much later at job creation/shipping time instead of at the call that AWS fails fast on.
Fix: CreateAddress input validation for snowball — validate the Address members required for delivery and return InvalidAddressException (HTTP 400) when one is missing.

Cause 3 of 3 — CreateAddress accepts an address in an unserviceable country instead of raising a 400 (fixture snowball-create-address-invalid-rejected, step task#4).
AWS: recorded behavior only for the error shape — the model binds only InvalidAddressException/UnsupportedAddressException (both httpError 400) to CreateAddress, and the Address doc states an unsupported address throws. The recording for an address whose Country is outside the region's serviceable area is HTTP 400 with $.__type = 'ValidationException' and a lowercase $.message member (example message: 'Delivery of the device from this region to this country is not available.'). Hypothesis: the service front-end rejects the region/country pair before the Snowball-specific validators run, which is why the recorded code is ValidationException rather than UnsupportedAddressException and why the member is 'message' rather than 'Message'.
Emulator: HTTP 200 with a normal CreateAddressResult; no $.__type, no message.
Why: a client gets a successful AddressId for a destination the service cannot ship to, so it cannot distinguish 'address outside serviceable area' from a usable address and will fail opaquely downstream.
Fix: CreateAddress input validation for snowball — reject an unserviceable region/country pair with HTTP 400 and __type 'ValidationException' carrying a lowercase 'message' member, matching the recorded wire shape.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health 200, no state_reset_failures):

Cause 1 — snowball-address-create-describe-round-trip#task#2, DescribeAddress, verdict=mismatch, expected 200 / actual 200: "$.Address.Type: missing (expected 'AWS_SHIP', got None)". Cluster also covers snowball-create-address-invalid-rejected#task#2.

Cause 2 — snowball-create-address-invalid-rejected#task#3, CreateAddress, verdict=mismatch: "status: expected 400, got 200; $.__type: missing (expected 'InvalidAddressException', got None); $.Message: missing (expected 'You must specify Address 1 in the address.', got None)".

Cause 3 — snowball-create-address-invalid-rejected#task#4, CreateAddress, verdict=mismatch: "status: expected 400, got 200; $.__type: missing (expected 'ValidationException', got None); $.message: missing (expected 'Delivery of the device from this region to this country is not available.', got None)".

Model evidence: Smithy CreateAddress -> errors InvalidAddressException (client, 400), UnsupportedAddressException (client, 400); Address.Type -> AddressType enum [CUST_PICKUP, AWS_SHIP]; Address doc: 'Addresses are validated at the time of creation ... if the address is invalid or unsupported, then an exception is thrown.' Error messages above are quoted as examples; the required parts are the status codes and the __type codes.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the wire corpus (trace set simparity-wire-aws-v1).
3. Cause 1: fixture snowball-address-create-describe-round-trip, step task#2 — CreateAddress with an Address carrying Type=AWS_SHIP, then DescribeAddress with the returned AddressId; observe the 200 response body has no Address.Type.
4. Cause 2: fixture snowball-create-address-invalid-rejected, step task#3 — CreateAddress with an Address missing its first street line; observe 200 + AddressId instead of 400 / __type=InvalidAddressException.
5. Cause 3: fixture snowball-create-address-invalid-rejected, step task#4 — CreateAddress with an Address whose Country is outside the region's serviceable area; observe 200 + AddressId instead of 400 / __type=ValidationException with a 'message' member.
