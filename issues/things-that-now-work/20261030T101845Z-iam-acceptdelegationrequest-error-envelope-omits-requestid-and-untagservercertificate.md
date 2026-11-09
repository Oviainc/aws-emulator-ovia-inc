Issue-Source: trace-sweep-divergence
Issue-Trace: iam-delegation-request-guard-clauses-and-create-attempt
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iam-delegation-request-guard-clauses-and-create-attempt iam-untag-server-certificate-lifecycle
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam|AcceptDelegationRequest|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|ListServerCertificateTags|2xx->2xx|$.ListServerCertificateTagsResponse.ListServerCertificateTagsResult.Tags.member
Issue-Commit: 58c0a8354906cc8e7217ba15b90fa6143710efff

# iam: AcceptDelegationRequest error envelope omits RequestId and UntagServerCertificate leaves the removed tag in place

> _Note: this issue was identified against the code base at commit `58c0a8354906cc8e7217ba15b90fa6143710efff`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AcceptDelegationRequest

## Description
Two net-new wire divergences on the iam service, both reproduced by replaying the simparity-wire-aws-v1 corpus against the emulator built from this checkout. Neither is carried by an open issue's replay key, so both must be closed before this issue is submitted.

Cause 1 of 2: AcceptDelegationRequest's NoSuchEntity error envelope has no RequestId element.
AWS: IAM is an awsQuery service, so every error body is <ErrorResponse><Error><Type/><Code/><Message/></Error><RequestId>...</RequestId></ErrorResponse> and the RequestId element is always present (its value is a per-request UUID, which is per-run and NOT part of the requirement — only the element's presence and position are). Smithy: AcceptDelegationRequest declares NoSuchEntityException with awsQueryError code NoSuchEntity / httpResponseCode 404 (input DelegationRequestId, required, @length 16..128, pattern ^[\w-]+$). The recorded response for an unknown DelegationRequestId is HTTP 404 with Error/Type Sender, Error/Code NoSuchEntity and the NoSuchEntity message, followed by RequestId.
Emulator: status 404, Error/Type, Error/Code and Error/Message all match, but the RequestId child is absent — replay detail `$.ErrorResponse.RequestId: missing (expected '<uuid>', got None)`.
Why: a client that correlates or logs by request id, or an SDK/middleware that reads ResponseMetadata.RequestId off a failed call, gets a null; wrappers that assert on the field raise instead of surfacing NoSuchEntity, so the real IAM error never reaches application code.
Fix: the awsQuery error serializer on the AcceptDelegationRequest path — it must emit <RequestId> after </Error> with the same per-request id the success path puts in ResponseMetadata/RequestId. NOTE: the open issue 20261029T184359Z-iam-error-responses-omit-the-responsemetadata-requestid-element describes this same serializer fault for 69 other operations, but its Issue-Key list does not include AcceptDelegationRequest and no open issue carries this fixture step's replay key, so this cause is filed and must be verified here.
Traces: iam-delegation-request-guard-clauses-and-create-attempt#task#3.

Cause 2 of 2: a tag removed by UntagServerCertificate is still returned by ListServerCertificateTags — the untag never mutates state.
AWS: Smithy: UntagServerCertificate(ServerCertificateName required, TagKeys required, tagKeyListType @length 0..50) removes exactly the named keys from the server certificate's tag set and returns an empty body (output Unit); ListServerCertificateTags returns the surviving Tags list (required, @length 0..50) for that certificate. Recorded behavior: after untagging one key of a two-key tag set, the subsequent ListServerCertificateTags returns only the key that was not named — one Tags member, the remaining Key/Value pair.
Emulator: the same ListServerCertificateTags answers 200 but still carries BOTH tags — replay detail `$.ListServerCertificateTagsResponse.ListServerCertificateTagsResult.Tags.member: type_mismatch (expected {'Value': 'probe', 'Key': 'Env'}, got [{'Key': 'Env', 'Value': 'probe'}, {'Key': 'Tier', 'Value': 'web'}])`. The removal requested for the untagged key was not applied, so the tag set is unchanged in both membership and cardinality (the recorded single-member serialization vs. the emulator's two-member list follows from the extra member, not from a separate shape rule).
Why: tag-driven clients (cost allocation, tag-based IAM conditions, cleanup/reconciliation loops) read a tag they explicitly deleted as still attached; a reconcile loop re-issues the untag forever and any tag-conditioned authorization keeps matching on a key that no longer exists in AWS.
Fix: the UntagServerCertificate handler and the server-certificate tag state it writes — the removal of the named TagKeys must be persisted so the next ListServerCertificateTags/GetServerCertificate read reflects it. The open behavioral issue 20261030T063350Z-iam-untagservercertificate-stub reports that this handler is still a scaffold returning 501; that issue carries only behavioral check ids and no wire replay key, so this fixture step must be re-replayed here to confirm the persisted-state effect.
Traces: iam-untag-server-certificate-lifecycle#task#5.

## Evidence
Cause 1 — fixture iam-delegation-request-guard-clauses-and-create-attempt step task#3, operation AcceptDelegationRequest.
Expected (recorded AWS): HTTP 404, body <ErrorResponse><Error><Type>Sender</Type><Code>NoSuchEntity</Code><Message>...</Message></Error><RequestId>{per-request uuid}</RequestId></ErrorResponse>. Fixed text: the Type/Code pair Sender/NoSuchEntity; variable part: the RequestId value (per-run) and the delegation request id interpolated into the message.
Actual (emulator): HTTP 404 with the identical Error block and no RequestId child. Replay detail: `$.ErrorResponse.RequestId: missing (expected 'e87f4144-ce69-4b12-b7d6-db2f701249ba', got None)` — the quoted uuid is that run's value, not a requirement.

Cause 2 — fixture iam-untag-server-certificate-lifecycle step task#5, operation ListServerCertificateTags (following an UntagServerCertificate of one key in the same fixture).
Expected (recorded AWS): HTTP 200 with Tags carrying only the key that was not untagged (one member: Key/Value, e.g. Env=probe in that recording).
Actual (emulator): HTTP 200 with Tags carrying both the surviving key and the key that was requested removed. Replay detail: `$.ListServerCertificateTagsResponse.ListServerCertificateTagsResult.Tags.member: type_mismatch (expected {'Value': 'probe', 'Key': 'Env'}, got [{'Key': 'Env', 'Value': 'probe'}, {'Key': 'Tier', 'Value': 'web'}])`. The tag names/values shown are that fixture's values; the requirement is that the untagged key is gone and the others remain.

## Steps to Reproduce
1. Clone this checkout and boot the emulator from it.
2. Cause 1: replay fixture iam-delegation-request-guard-clauses-and-create-attempt step task#3 — AcceptDelegationRequest with a DelegationRequestId that does not exist. Read the 404 body: <ErrorResponse> has <Error> (Sender/NoSuchEntity) but no <RequestId> sibling.
3. Cause 2: replay fixture iam-untag-server-certificate-lifecycle through step task#5 — upload a server certificate, tag it with two keys, call UntagServerCertificate naming one key, then call ListServerCertificateTags for that certificate. The response still lists the untagged key alongside the surviving one instead of only the survivor.
4. Both steps must pass on re-replay before this issue is closed.
