Issue-Source: trace-sweep-divergence
Issue-Trace: s3outposts-create-endpoint-unknown-outpost
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3outposts-create-endpoint-unknown-outpost s3outposts-delete-endpoint-not-found s3outposts-list-shared-endpoints-for-outpost
Issue-Shape-Id: com.amazonaws.s3outposts#S3Outposts
Issue-Key: s3-outposts|CreateEndpoint|4xx->4xx|$.Message
Issue-Key: s3-outposts|DeleteEndpoint|4xx->4xx|$.Message
Issue-Key: s3-outposts|ListSharedEndpoints|4xx->4xx|$.Message
Issue-Commit: f973f78f74c99aad4e85f2736f1fde89ed7327e3

# Unknown op- Outpost masked as Access Denied in ResourceNotFoundException

> _Note: this issue was identified against the code base at commit `f973f78f74c99aad4e85f2736f1fde89ed7327e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEndpoint

## Description
Every operation that resolves a reference under an OutpostId answers an unresolvable `op-...` Outpost with the WRONG fixed ResourceNotFoundException$Message. The emulator selects the message by the FORM of the OutpostId: `services/service-com-amazonaws-s3outposts-s3outposts/src/state.rs` `not_found_message()` returns the outpost-not-found text ONLY for the 12-digit account-id branch of OutpostId's pattern and returns the masked string "Access Denied" for the `op-[a-f0-9]{17}` and `ec2` branches. The recorded corpus shows AWS doing the opposite for the `op-` branch: the outpost-not-found text is what a caller gets. This is one defect (an inverted branch in one shared helper) observed through three operations, so it is three causes to close.

Cause 1 of 3: CreateEndpoint with an OutpostId that names no Outpost the caller can see.
AWS: Smithy: CreateEndpoint errors include ResourceNotFoundException (errorKind client, httpError 404, member Message: ErrorMessage; doc "The requested resource was not found."). The model carries no message text, so the text is recorded behavior only: the recording shows HTTP 404 ResourceNotFoundException whose Message is the service constant "The specified outpost does not exist, or does not have S3 capacity provisioned on it." The constant is per-condition, never built from the request — the same text appears for three different operations and three different request shapes in this corpus, and the emulator's own state.rs documents both texts as service constants.
Emulator: HTTP 404 ResourceNotFoundException (status and error shape are correct) with $.Message = "Access Denied".
Why: a client gets an authorization failure where AWS reported a missing/unprovisioned Outpost. Code that branches on, logs, or surfaces the message tells an operator to fix credentials or a resource policy instead of provisioning S3 capacity on the Outpost or correcting the OutpostId; a retry-on-auth-refresh path also retries a request that can never succeed.
Fix: the not-found message selection shared by these handlers — `not_found_message()` / `not_found()` in the service's state module — must return the outpost-not-found constant for the `op-`/`ec2` branches as the recording shows, not the masked string.
Traces: s3outposts-create-endpoint-unknown-outpost#task#1.

Cause 2 of 3: DeleteEndpoint naming an endpoint on an Outpost that does not resolve.
AWS: Smithy: DeleteEndpoint errors include ResourceNotFoundException (client, httpError 404). Recorded behavior only for the text: HTTP 404 with the same service constant "The specified outpost does not exist, or does not have S3 capacity provisioned on it."
Emulator: HTTP 404 ResourceNotFoundException with $.Message = "Access Denied".
Why: same as Cause 1 — a delete against a nonexistent Outpost is reported as an access problem, so idempotent teardown logic that treats access-denied as fatal (and not-found as already-deleted) aborts instead of completing.
Fix: same shared not-found message selection; DeleteEndpoint resolves its Outpost through it.
Traces: s3outposts-delete-endpoint-not-found#task#1.

Cause 3 of 3: ListSharedEndpoints for an OutpostId that does not resolve.
AWS: Smithy: ListSharedEndpoints errors include ResourceNotFoundException (client, httpError 404); input OutpostId is required with pattern ^(op-[a-f0-9]{17}|\d{12}|ec2)$. Recorded behavior only for the text: HTTP 404 with the same constant.
Emulator: HTTP 404 ResourceNotFoundException with $.Message = "Access Denied".
Why: same as above — a discovery/listing loop over Outpost ids cannot tell "this Outpost is not visible to me / not provisioned" from "my credentials are wrong", so it stops enumerating rather than skipping the id.
Fix: same shared not-found message selection; the listing handler resolves the Outpost before paging endpoints.
Traces: s3outposts-list-shared-endpoints-for-outpost#task#2.

No existing open issue covers s3-outposts (list_existing_issues returned none for 'outpost'/'s3outposts').

## Evidence
Cause 1 — fixture s3outposts-create-endpoint-unknown-outpost step task#1, CreateEndpoint: expected status 404, actual 404; $.Message value_mismatch — expected 'The specified outpost does not exist, or does not have S3 capacity provisioned on it.', got 'Access Denied'.
Cause 2 — fixture s3outposts-delete-endpoint-not-found step task#1, DeleteEndpoint: expected 404, actual 404; $.Message value_mismatch — expected 'The specified outpost does not exist, or does not have S3 capacity provisioned on it.', got 'Access Denied'.
Cause 3 — fixture s3outposts-list-shared-endpoints-for-outpost step task#2, ListSharedEndpoints: expected 404, actual 404; $.Message value_mismatch — expected 'The specified outpost does not exist, or does not have S3 capacity provisioned on it.', got 'Access Denied'.
The status (404) and the ResourceNotFoundException shape match in all three; only the fixed Message constant diverges, and it diverges the same way each time. Source of the inverted rule: services/service-com-amazonaws-s3outposts-s3outposts/src/state.rs, MASKED_NOT_FOUND_MESSAGE / OUTPOST_NOT_FOUND_MESSAGE / not_found_message().

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Replay fixture s3outposts-create-endpoint-unknown-outpost step task#1 (POST /S3Outposts/CreateEndpoint with an OutpostId in the op-[a-f0-9]{17} form that the store holds no record for) and read $.Message of the 404 body: it is 'Access Denied' instead of the outpost-not-found constant.
3. Replay s3outposts-delete-endpoint-not-found step task#1 (DELETE /S3Outposts/DeleteEndpoint?outpostId=<unknown op- id>&endpointId=...) — same 404 body with 'Access Denied'.
4. Replay s3outposts-list-shared-endpoints-for-outpost step task#2 (GET /S3Outposts/ListSharedEndpoints?outpostId=<unknown op- id>) — same 404 body with 'Access Denied'.
5. All three must carry Message 'The specified outpost does not exist, or does not have S3 capacity provisioned on it.' with status 404.
