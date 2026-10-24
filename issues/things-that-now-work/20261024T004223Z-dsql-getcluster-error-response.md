Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: a1b03c4f6e81626c353b3ff836c908b12c596c5c

# Boundary constraint refusals return a ValidationException body with only `message`, omitting the modeled required `reason`

_Filed: 2026-10-24T00:42:23Z_
_Operation: GetCluster | Class: error-response_

> _Note: this issue was identified against the code base at commit `a1b03c4f6e81626c353b3ff836c908b12c596c5c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every request refused by the framework's model-constraint walk (a malformed `identifier`, a `maxResults` outside its range, a `clientToken` outside its pattern, an absent required member) is answered through this service's `override_refusal` seam, which can only produce a `Refused { code, message }` pair.

Expected: HTTP 400 with a modeled `ValidationException` body carrying `message` AND the @required `reason` (a `ValidationExceptionReason`: cannotParse | fieldValidationFailed | deletionProtectionEnabled | other | unknownOperation), optionally `fieldList` naming the offending member — the same body shape the handler-side refusals of this service already produce (`state.rs::invalid_resource_arn`, `delete_cluster.rs`'s deletion-protection refusal, `put_cluster_policy.rs`'s parse refusal all fill `reason`). A required member of a modeled error shape cannot be absent in a real AWS response.

Actual: `src/service.rs::refused_for` returns `Refused { code: "ValidationException", message: <AWS envelope> }`, and `Refused` (vendor/smithy-server-runtime/src/pre_dispatch.rs line 164) carries only `code` and `message` — so the rendered body is `{"message": "1 validation error detected: ..."}` with no `reason`. This is service-wide: it is the answer for a malformed `identifier` on GetCluster/GetStream/UpdateCluster/DeleteCluster/Get|Put|DeleteClusterPolicy/GetVpcEndpointServiceName, for `maxResults` outside range on ListClusters/ListStreams, and for pattern/length violations on CreateCluster/CreateStream/TagResource. A client that branches on `reason` sees it absent for the most common failure mode of the whole service. Status (400) and wire code are correct; only the body shape diverges.

## Evidence

Spec: `ValidationException` declares `message` and `reason` as @required (`fieldList` optional) and is a service-common error for every operation (smithy_query service_info common_errors: AccessDenied / InternalServer / Throttling / Validation). Implementation: `src/service.rs` lines 83-150 (`refused_for` / `validation_envelope` / `clause_for` build only a code and a message), with `Refused` defined as exactly two String fields in `vendor/smithy-server-runtime/src/pre_dispatch.rs` lines 161-169. Contrast the handler-side modeled refusals that DO fill `reason`: `src/state.rs::invalid_resource_arn` lines 970-983, `src/ops/delete_cluster.rs` lines 212-223, `src/ops/put_cluster_policy.rs` lines 228-236. Tests: `tests/all/error_contract.rs::a_constraint_violation_names_the_member_and_withholds_the_value` (lines 154-288) asserts status 400, errortype ValidationException and the message envelope only — no assertion on `reason`; same for `tests/all/vpc_endpoint_service_name.rs::a_malformed_identifier_is_refused_as_invalid` and `policy_document_class.rs::a_malformed_identifier_is_refused_by_get_cluster_policy`. Related already-filed issues cover the handler-side synthetic refusals for ListClusters/ListStreams/TagResource only; the framework refusal path is a different mechanism and is unfiled.

## Steps to Reproduce

1. GET /cluster/not-a-valid-identifier!! (signed as dsql).
2. Observe 400, x-amzn-errortype ValidationException, body {"message": "1 validation error detected: Value at 'identifier' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-z0-9]{26}"}.
3. Observe the body has no `reason` member, although ValidationException models it @required.
4. Repeat with GET /cluster?max-results=0 and POST /cluster {"deletionProtectionEnabled": false, "clientToken": "has a space"} — same shape, same missing member.
