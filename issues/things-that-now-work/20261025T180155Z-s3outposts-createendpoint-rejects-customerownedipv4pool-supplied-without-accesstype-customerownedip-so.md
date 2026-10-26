Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3outposts.create-endpoint-customer-owned-ip-pool-without-access-type-rejected
Issue-Shape-Id: com.amazonaws.s3outposts#S3Outposts
Issue-Key: s3outposts.create-endpoint-customer-owned-ip-pool-without-access-type-rejected|run#10|AssertionError|assert subject_code == baseline_code, (
Issue-Commit: 285e13d027e3ead88e88da0752407bcae844042d

# CreateEndpoint rejects CustomerOwnedIpv4Pool supplied without AccessType=CustomerOwnedIp, so the request never reaches the Outpost-existence check

> _Note: this issue was identified against the code base at commit `285e13d027e3ead88e88da0752407bcae844042d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEndpoint

## Description
REOPENS 20261024T183631Z-s3outposts-createendpoint-validation.md: its fix left the call refused.

Cause 1 of 1: CreateEndpoint enforces a CustomerOwnedIpv4Pool/AccessType pairing rule in the pool-present direction that the service does not enforce, turning a request the service only fails on Outpost lookup into a 400 ValidationException.

AWS: Smithy (services/service-com-amazonaws-s3outposts-s3outposts/smithy-model/com-amazonaws-s3outposts-s3outposts.json) models CreateEndpointRequest with required OutpostId/SubnetId/SecurityGroupId and two optional members, AccessType (EndpointAccessType enum) and CustomerOwnedIpv4Pool (pattern ^ipv4pool-coip-([0-9a-f]{17})$). The model expresses NO cross-member constraint between them; AccessType's prose states only the one direction — "If you choose CustomerOwnedIp, you must also provide the customer-owned IP address pool (CoIP pool)" — and notes "Private is the default access type value". It says nothing that makes a pool illegal on a Private/absent AccessType request, and recorded behavior confirms the reverse pairing is NOT validation-rejected: a well-formed CustomerOwnedIpv4Pool with AccessType unset reaches exactly the same Outpost-existence check a request with neither member reaches, and fails identically with ResourceNotFoundException (HTTP 404, modeled on CreateEndpoint) whose Message is the service's fixed not-found text "The specified outpost does not exist, or does not have S3 capacity provisioned on it." — that string is wholly fixed text with no request- or state-derived parts.

Emulator: the handler applies the pool rule in BOTH directions before any lookup. services/service-com-amazonaws-s3outposts-s3outposts/src/ops/create_endpoint.rs:169-188 defaults the absent AccessType to Private and then calls crate::state::check_coip_pool(&access_type, input.customer_owned_ipv4_pool.as_deref()), returning CreateEndpointErrorModeled::ValidationException on the Private+pool combination. Because that runs ahead of the Outpost check at create_endpoint.rs:198-203, the replay sees error code "ValidationException" (HTTP 400) where the baseline request (same OutpostId, no pool) correctly yields "ResourceNotFoundException" (HTTP 404) with the fixed not-found Message. The two requests therefore diverge in status, error code and message purely on the presence of the pool member.

Why: a client that sets CustomerOwnedIpv4Pool and leaves AccessType to its documented Private default gets a 400 ValidationException from the emulator for a request AWS admits — against AWS the call proceeds to the Outpost (succeeding on a provisioned Outpost, 404 on an unknown one), so the endpoint can never be created against the emulator. Error-handling code is misled twice over: it sees a validation refusal where AWS reports a missing Outpost, so retry/provision logic keyed on ResourceNotFoundException never fires, and the pairing is advertised as a hard input constraint the real API does not impose.

Fix: the cross-member validation in CreateEndpoint's handler (services/service-com-amazonaws-s3outposts-s3outposts/src/ops/create_endpoint.rs:169-188 and the shared check_coip_pool helper in src/state.rs). Only the modeled/documented direction (AccessType=CustomerOwnedIp with no pool) is a ValidationException; a pool supplied with AccessType absent or Private must fall through to the Outpost-existence check and the normal create path, and the stored EndpointRecord (src/state.rs:201-227) must then reflect whatever the request said rather than being rejected. Note the prior fix for 20261024T183631Z-s3outposts-createendpoint-validation.md introduced this refusal; the persistence concern that issue raised must be resolved without refusing the request.

Traces: s3outposts.create-endpoint-customer-owned-ip-pool-without-access-type-rejected — baseline CreateEndpoint(OutpostId=<nonexistent 12-digit id>, SubnetId, SecurityGroupId) gives ResourceNotFoundException; the same call plus a well-formed CustomerOwnedIpv4Pool gives ValidationException.

## Evidence
Check s3outposts.create-endpoint-customer-owned-ip-pool-without-access-type-rejected (mismatch):
  AssertionError: expected CustomerOwnedIpv4Pool without AccessType=CustomerOwnedIp to reach the same Outpost lookup as the baseline request ('ResourceNotFoundException'), got 'ValidationException' -- a ValidationException here would mean the pairing is enforced

Expected (AWS): CreateEndpoint with OutpostId=<unknown Outpost>, SubnetId, SecurityGroupId and CustomerOwnedIpv4Pool=ipv4pool-coip-<17 hex>, AccessType omitted -> HTTP 404, Error.Code "ResourceNotFoundException", Error.Message exactly "The specified outpost does not exist, or does not have S3 capacity provisioned on it." (entirely fixed text) — identical to the baseline request that omits the pool.
Actual (emulator): HTTP 400, Error.Code "ValidationException" from the pre-lookup pairing check.

Baseline leg of the same check passes, so the divergence is attributable solely to the presence of CustomerOwnedIpv4Pool.

Smithy: CreateEndpoint errors = AccessDeniedException(403), ConflictException(409), InternalServerException(500), OutpostOfflineException(400), ResourceNotFoundException(404), ThrottlingException(429), ValidationException(400). CreateEndpointRequest declares no cross-member constraint tying CustomerOwnedIpv4Pool to AccessType; AccessType doc states the requirement only in the CustomerOwnedIp direction.

Code: src/ops/create_endpoint.rs:180-188 (default-to-Private then check_coip_pool -> ValidationException) runs before src/ops/create_endpoint.rs:198-203 (Outpost existence -> ResourceNotFoundException).

## Steps to Reproduce
1. Start the emulator with no Outposts provisioned (leave S3_OUTPOSTS_PROVISIONED_OUTPOSTS unset) so every Outpost id is unknown.
2. POST /S3Outposts/CreateEndpoint with {"OutpostId":"999999999999","SubnetId":"subnet-00000000","SecurityGroupId":"sg-00000000"} — observe HTTP 404 ResourceNotFoundException with Message "The specified outpost does not exist, or does not have S3 capacity provisioned on it." (correct baseline).
3. POST the same body with "CustomerOwnedIpv4Pool":"ipv4pool-coip-00000000000000000" added and AccessType still omitted.
4. Expected: the identical HTTP 404 ResourceNotFoundException and the same fixed Message — the pool does not change which check the request reaches.
5. Observed: HTTP 400 ValidationException from the AccessType/pool pairing check, before the Outpost is ever looked up.
6. Repeat step 3 with AccessType="Private" explicitly: same incorrect ValidationException.
7. GET /S3Outposts/ListEndpoints?maxResults=100 — no endpoint was created by either request, which is correct and unaffected.
