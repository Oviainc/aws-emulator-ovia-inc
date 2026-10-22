Issue-Source: trace-sweep-divergence
Issue-Trace: managedblockchain-create-member-invitation-not-found
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: managedblockchain-create-member-invitation-not-found managedblockchain-get-node-not-found managedblockchain-list-members-invalid-status-filter-rejected
Issue-Shape-Id: com.amazonaws.managedblockchain#TaigaWebService
Issue-Key: managedblockchain|GetNetwork|4xx->4xx|$.Message
Issue-Key: managedblockchain|GetNetwork|4xx->4xx|$.ResourceName
Issue-Key: managedblockchain|GetNode|4xx->4xx|$.Message
Issue-Key: managedblockchain|GetNode|4xx->4xx|$.ResourceName
Issue-Key: managedblockchain|ListMembers|4xx->4xx|$.Message
Issue-Key: managedblockchain|ListMembers|4xx->4xx|$headers.x-amzn-errortype
Issue-Commit: 6e0c43ba1fe296efb543002735fa691273898c5f

# managedblockchain error responses do not match modeled exception shapes

> _Note: this issue was identified against the code base at commit `6e0c43ba1fe296efb543002735fa691273898c5f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetNetwork

## Description
Three causes, all in the managedblockchain error path: the emulator returns the right HTTP status for these failures but serializes the error payload with members AWS does not send, with a value that violates the modeled shape, with the wrong error code, and with no Message at all. No other open issue covers managedblockchain (list_existing_issues 'managedblockchain' is empty).

Cause 1 of 3 — GetNetwork on an unknown network populates ResourceNotFoundException.ResourceName with a bare resource id (fixture managedblockchain-create-member-invitation-not-found, step task#2).
AWS: Smithy: GetNetwork -> ResourceNotFoundException (errorKind client, httpError 404); ResourceNotFoundException members are Message (String) and ResourceName (ArnString, @length 1..1011, @pattern ^arn:.+:.+:.+:.+:.+$). The recorded 404 response carries only Message (ResourceName absent) and the message names the missing resource type and id, e.g. 'Network n-0000... not found.'
Emulator: returns 404 ResourceNotFoundException but adds ResourceName set to the bare network id (e.g. 'n-0000...'), which is not an ARN and cannot satisfy the ArnString pattern, and words Message as a network-does-not-exist-on-this-account sentence (recorded text is only an example; the requirement is that Message names the resource that was not found).
Why: a client that reads ResourceName expects an ARN it can parse/compare (region, account, resource id). Receiving a bare id makes ARN parsing fail or produces a wrong-shaped ARN, and SDK/model-driven validators can reject the field outright; AWS simply omits it here, so the member should not be emitted at all unless an ARN is available.
Fix: the GetNetwork handler's not-found error construction / the ResourceNotFoundException serializer for this service — stop populating ResourceName with a non-ARN id (omit it, as the recording does) and name the requested resource in Message.
Traces: managedblockchain-create-member-invitation-not-found#task#2 (expected 404 / got 404; $.ResourceName type_mismatch expected None got 'n-0000...'; $.Message value_mismatch).

Cause 2 of 3 — GetNode on an unknown node reports the NETWORK as the missing resource and again emits a non-ARN ResourceName (fixture managedblockchain-get-node-not-found, step task#1).
AWS: Smithy: GetNode (GET /networks/{NetworkId}/nodes/{NodeId}) -> ResourceNotFoundException (client, 404), same shape as above. The recording shows AWS attributing the failure to the resource actually requested — the node — e.g. 'Node nd-0000... not found.' — and omitting ResourceName. The attribution rule is request-derived: the exception names the resource the operation was asked for.
Emulator: returns 404 but the payload describes the NETWORK as nonexistent and sets ResourceName to the network id instead of leaving it unset; the requested NodeId appears nowhere in the response.
Why: callers (and operators reading logs) cannot tell which resource is missing. Retry/branching logic that distinguishes 'network gone' from 'node gone' takes the wrong branch, and the non-ARN ResourceName breaks ARN parsing exactly as in Cause 1.
Fix: the GetNode handler's lookup/validation order and its not-found error construction — resolve the node and report the requested node as the missing resource, and drop the non-ARN ResourceName from the serialized exception.
Traces: managedblockchain-get-node-not-found#task#1 (expected 404 / got 404; $.ResourceName type_mismatch expected None got 'n-0000...'; $.Message value_mismatch, expected a node-not-found message, got a network-does-not-exist message).

Cause 3 of 3 — ListMembers with an invalid status filter returns an unmodeled error code 'ValidationException' and no Message (fixture managedblockchain-list-members-invalid-status-filter-rejected, step task#2).
AWS: Smithy: ListMembers.Status targets the MemberStatus enum (CREATING, AVAILABLE, CREATE_FAILED, UPDATING, DELETING, DELETED, INACCESSIBLE_ENCRYPTION_KEY) bound as httpQuery 'status'; the operation's only modeled client 400 is InvalidRequestException (errorKind client, httpError 400, member Message). The recorded response is 400 with a Message built from the request naming the rejected value, e.g. 'Status NOT_A_REAL_STATUS is not valid.'
Emulator: answers 400 but with x-amzn-errortype 'ValidationException' — an error shape that is not in this service's model at all — and with no Message member in the body.
Why: clients dispatch on the error code: SDK error unions for managedblockchain have no ValidationException variant, so the typed catch for InvalidRequestException never fires and the failure surfaces as an unknown/unmodeled error. With Message also absent, the caller cannot even report which query parameter was rejected.
Fix: ListMembers input validation — reject an out-of-enum status with InvalidRequestException (400) and a Message built from the request that names the member and the invalid value.
Traces: managedblockchain-list-members-invalid-status-filter-rejected#task#2 (expected 400 / got 400; $headers.x-amzn-errortype shape_mismatch got 'ValidationException'; $.Message missing).

## Evidence
Replay of trace set simparity-wire-aws-v1 (3 fixtures, 5 steps, 0 passed, state_reset_failures=0, bootstrap_signal=false):

1) managedblockchain-create-member-invitation-not-found#task#2 — GetNetwork, verdict mismatch, expected_status 404, actual_status 404.
   detail: $.Message: value_mismatch (expected 'Network n-000000000000000000000000000000 not found.', got 'network n-000000000000000000000000000000 does not exist on this account'); $.ResourceName: type_mismatch (expected None, got 'n-000000000000000000000000000000').
   Model: ResourceNotFoundException.ResourceName is ArnString, @pattern ^arn:.+:.+:.+:.+:.+$ — the emitted value cannot match.

2) managedblockchain-get-node-not-found#task#1 — GetNode, verdict mismatch, expected_status 404, actual_status 404.
   detail: $.Message: value_mismatch (expected 'Node nd-00000000000000000000000000000 not found.', got 'network n-000000000000000000000000000000 does not exist on this account'); $.ResourceName: type_mismatch (expected None, got 'n-000000000000000000000000000000').
   Expected: the exception names the requested node; emulator names the network and emits a non-ARN ResourceName.

3) managedblockchain-list-members-invalid-status-filter-rejected#task#2 — ListMembers, verdict mismatch, expected_status 400, actual_status 400.
   detail: $.Message: missing (expected 'Status NOT_A_REAL_STATUS is not valid.', got None); $headers.x-amzn-errortype: shape_mismatch (expected a modeled error shape, got 'ValidationException').
   Model: ListMembers errors = AccessDeniedException(403), InternalServiceErrorException(500), InvalidRequestException(400), ThrottlingException(429) — ValidationException is not among them.

Recorded message texts above are examples; the requirements asserted are the status, the error code, the presence/absence of the ResourceName member and its ARN shape, and that Message is present and derived from the request.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus) — all 5 steps of the 3 managedblockchain fixtures diverge.
3. Cause 1: replay fixture managedblockchain-create-member-invitation-not-found step task#2 — GET /networks/n-000000000000000000000000000000 for a network that does not exist. Observe 404 ResourceNotFoundException whose body includes ResourceName='n-0000...' (a bare id, not an ARN); the recording's body has no ResourceName.
4. Cause 2: replay fixture managedblockchain-get-node-not-found step task#1 — GET /networks/{NetworkId}/nodes/nd-00000000000000000000000000000 for a node that does not exist. Observe the 404 body describes the network as nonexistent and sets ResourceName to the network id, never mentioning the requested NodeId.
5. Cause 3: replay fixture managedblockchain-list-members-invalid-status-filter-rejected step task#2 — GET /networks/{NetworkId}/members?status=NOT_A_REAL_STATUS. Observe HTTP 400 with header x-amzn-errortype: ValidationException and a body with no Message member; expected InvalidRequestException with a Message naming the invalid status value.
