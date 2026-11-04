Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53.list-hosted-zones-by-vpc-invalid-pagination-token-rejected
Issue-Shape-Id: com.amazonaws.route53#AWSDnsV20130401
Issue-Key: route53.list-hosted-zones-by-vpc-invalid-pagination-token-rejected|run#14|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Commit: 46b3983886cf69b5bbc8712ce2669b136a5035ce

# Route 53 ListHostedZonesByVPC InvalidPaginationToken Message is emulator-authored ('Invalid pagination token: <token>') instead of AWS's 'The nextToken provided is invalid.'

> _Note: this issue was identified against the code base at commit `46b3983886cf69b5bbc8712ce2669b136a5035ce`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListHostedZonesByVPC

## Description
Cause 1 of 1: ListHostedZonesByVPC refuses a NextToken it never minted with the right status and the right error code, but with a sentence the emulator composed instead of the one Route 53 words for that error class.

AWS: Smithy: ListHostedZonesByVPC (GET /2013-04-01/hostedzonesbyvpc) binds InvalidPaginationToken with httpError 400; NextToken targets PaginationToken @length 0..1024, so token validity is service-side. Smithy doc for InvalidPaginationToken: "The value that you specified to get the second or subsequent page of results is invalid." Recorded behavior: for a corrupted/unminted NextToken, Route 53 answers HTTP 400, Error.Code 'InvalidPaginationToken', and Error.Message exactly 'The nextToken provided is invalid.' — the whole message is FIXED text; it has NO variable parts, and in particular it does NOT echo the submitted token.

Emulator: same HTTP 400 and same code 'InvalidPaginationToken', but Error.Message is 'Invalid pagination token: <token>' — emulator-authored wording whose tail is the token value copied verbatim from the request (the recorded run shows 'Invalid pagination token: corrupted-not-a-real-token-xyz', where 'corrupted-not-a-real-token-xyz' is the request's NextToken and not part of any template).

Why: the message is the only prose the service gives for this refusal class. A client (or test) that matches Route 53's exact sentence 'The nextToken provided is invalid.' never matches, and a client that logs or surfaces the refusal shows text AWS never emits. It additionally reflects an attacker/caller-supplied token back into the error body, which AWS deliberately does not do for this error.

Fix: the refusal's message construction, not the pagination logic — `crate::state::invalid_pagination_token()` as called from the token-validation arm of ListHostedZonesByVPC's handler; the token must not be interpolated into the message, and the fixed AWS sentence must be rendered instead. Detection of the bad token (status 400 + code InvalidPaginationToken) is already correct, and the surrounding assertions pass: the zone stays visible through an un-tokened ListHostedZonesByVPC before and after the bad-token call.

Note: this is NOT covered by the open issue 20261029T220729Z-route53-route-53-refusal-messages-are-emulator-authored-instead, which carries CreateHostedZone/AssociateVPCWithHostedZone/CreateTrafficPolicy message causes only and names neither this operation nor InvalidPaginationToken.

Traces: route53.list-hosted-zones-by-vpc-invalid-pagination-token-rejected.

## Evidence
Behavioral check route53.list-hosted-zones-by-vpc-invalid-pagination-token-rejected, mismatch:

  AssertionError: expected message 'The nextToken provided is invalid.', got 'Invalid pagination token: corrupted-not-a-real-token-xyz'

The check first asserts the error code and that assertion passes (code == 'InvalidPaginationToken', HTTP 400), then fails on the message comparison. Expected: fixed text 'The nextToken provided is invalid.' with no variable parts. Actual: 'Invalid pagination token: ' followed by the NextToken value from the request.

Source of the diverging text: services/service-com-amazonaws-route53-awsdnsv20130401/src/ops/list_hosted_zones_by_vpc.rs lines 230-239 return ListHostedZonesByVPCErrorModeled::InvalidPaginationToken(invalid_pagination_token(token)) — the helper is crate::state::invalid_pagination_token (imported at line 102) and it embeds the submitted token in the message.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Create a VPC via ec2 CreateVpc (or reuse one) and create a PRIVATE hosted zone associated with it via route53 CreateHostedZone (HostedZoneConfig.PrivateZone=true, VPC={VPCId, VPCRegion}); wait for the change to be INSYNC.
3. Confirm the zone is listed by route53 ListHostedZonesByVPC(VPCId=<vpc>, VPCRegion=<region>) with no NextToken.
4. Call route53 ListHostedZonesByVPC(VPCId=<vpc>, VPCRegion=<region>, NextToken='corrupted-not-a-real-token-xyz').
5. Observe HTTP 400 with Error.Code 'InvalidPaginationToken' (correct) and Error.Message 'Invalid pagination token: corrupted-not-a-real-token-xyz'; AWS returns Error.Message 'The nextToken provided is invalid.' with no echo of the token.
