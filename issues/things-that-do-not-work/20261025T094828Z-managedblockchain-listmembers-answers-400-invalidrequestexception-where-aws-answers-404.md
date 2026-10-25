Issue-Source: trace-sweep-divergence
Issue-Trace: managedblockchain-list-members-invalid-status-filter-rejected
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-ga-prod-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: managedblockchain-list-members-invalid-status-filter-rejected
Issue-Shape-Id: com.amazonaws.managedblockchain#TaigaWebService
Issue-Key: managedblockchain|ListMembers|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: managedblockchain|ListMembers|4xx->4xx|status
Issue-Commit: 8748ae5f8ce40fdce02dd30b608cc6e842806ad3

# ListMembers answers 400 InvalidRequestException where AWS answers 404 for an unresolvable network scope

> _Note: this issue was identified against the code base at commit `8748ae5f8ce40fdce02dd30b608cc6e842806ad3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListMembers

## Description
One root cause in the ListMembers error path: the operation reports a request-validation failure (HTTP 400 InvalidRequestException) for a request whose recorded AWS answer is a 404 not-found, so resource-existence failures and request-shape failures are indistinguishable on this operation.

Cause 1 of 1: the unresolvable `/networks/{NetworkId}` scope is refused as a 400 request-validation error instead of the recorded 404.
  AWS: recorded behavior only for the status. Smithy: ListMembers is GET /networks/{NetworkId}/members with NetworkId an @required @httpLabel (ResourceIdString, length 1-32) and Status an @httpQuery "status" targeting enum MemberStatus (CREATING | AVAILABLE | CREATE_FAILED | UPDATING | DELETING | DELETED | INACCESSIBLE_ENCRYPTION_KEY); its declared error set is AccessDeniedException(403), InternalServiceErrorException(500), InvalidRequestException(400), ThrottlingException(429) and declares NO 404. The recording nonetheless shows AWS answering this request with HTTP 404, and the recorded response's x-amzn-errortype is not the modeled InvalidRequestException (the fixture asserts the 404 status with the error-type shaped rather than pinned to a literal). The service's own 404 client error shape is com.amazonaws.managedblockchain#ResourceNotFoundException (httpError 404, members Message/ResourceName), which the already-open behavioral issue 20261025T091959Z-managedblockchain-listmembers-misclassifies-an-unresolvable-networkid-as-invalidrequestexception-400 records AWS returning for exactly this scope (its checks managedblockchain.list-members-nonexistent-network-not-found / .list-members-invalid-status-filter-rejected). Hypothesis: AWS resolves the network named by the path label BEFORE validating the query-string filters, so a request against a network that does not resolve is reported as not-found and never reaches filter validation.
  Emulator: the replayed step returns HTTP 400 with x-amzn-errortype: InvalidRequestException. The handler folds the unresolved-network case into its own InvalidRequestException on the rationale that the operation models no 404 (the direction the now-fixed issue 20261024T161931Z-managedblockchain-listmembers-error-response asked for), which the recorded wire behavior contradicts.
  Why: a client branching on the not-found condition — a provisioning poller that treats a 404/ResourceNotFoundException as "network not visible yet" and retries, or an idempotent teardown that swallows the 404 — instead receives a 400 request-validation error, which SDK error classification treats as a permanent caller bug and will not retry or swallow. Because the same 400 InvalidRequestException is also the code for a genuinely malformed request, a caller cannot tell "your request was wrong" from "that network does not exist" on this operation.
  Fix: the ListMembers handler's network-resolution branch and the operation's error channel — answer an unresolvable network scope with the service's 404 not-found shape rather than remapping it onto InvalidRequestException, and keep filter validation ordered after (not before) scope resolution. The error enum generated from the model's declared error list has to be able to carry that shape, so the operation's scaffolded error channel is part of the same change. The sibling scope-by-network listings share the same remap rationale, so check them while fixing this.
  Traces: managedblockchain-list-members-invalid-status-filter-rejected#task#1 (status: expected 404, got 400; $headers.x-amzn-errortype got InvalidRequestException).

The open behavioral issue named above diagnoses the same handler from the behavioral corpus but carries behavioral check keys only, not this wire fixture/step binding, so this wire divergence is not replayed by it.

## Evidence
Replay step managedblockchain-list-members-invalid-status-filter-rejected#task#1, verdict mismatch, divergence_key service=managedblockchain|ListMembers|status=404->400|keys=$headers.x-amzn-errortype,status.
Expected (recorded AWS): HTTP 404, x-amzn-errortype not asserted to a literal but not InvalidRequestException; the service's 404 client error is com.amazonaws.managedblockchain#ResourceNotFoundException.
Actual (emulator): HTTP 404 -> 400; $headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'InvalidRequestException').
Model: smithy_query(operation=ListMembers) -> GET /networks/{NetworkId}/members, errors AccessDeniedException(403), InternalServiceErrorException(500), InvalidRequestException(400), ThrottlingException(429), no 404 declared; input Status is @httpQuery "status" targeting enum MemberStatus. smithy_query(shape=ResourceNotFoundException) -> client error, httpError 404, members Message/ResourceName. smithy_query(shape=MemberStatus) -> CREATING, AVAILABLE, CREATE_FAILED, UPDATING, DELETING, DELETED, INACCESSIBLE_ENCRYPTION_KEY.
Related issues: open 20261025T091959Z-managedblockchain-listmembers-misclassifies-an-unresolvable-networkid-as-invalidrequestexception-400 (same root cause, behavioral keys only — no wire replay key); fixed 20261024T161931Z-managedblockchain-listmembers-error-response (argued the opposite direction from the model alone and produced the current 400).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-ga-prod-v1).
2. Replay fixture managedblockchain-list-members-invalid-status-filter-rejected, step task#1: a ListMembers GET against /networks/{NetworkId}/members for a well-formed NetworkId that resolves to no network, carrying a status query filter.
3. Observe HTTP 400 with x-amzn-errortype InvalidRequestException.
4. Expected: HTTP 404 with the service's not-found error type (ResourceNotFoundException), i.e. the unresolvable path scope reported as not-found before any query-filter validation.
5. Equivalently with a client: point a botocore managedblockchain client (region us-east-1) at the emulator and call list_members(NetworkId='n-' + '0'*30, Status='AVAILABLE'); the response is 400 InvalidRequestException instead of 404.
