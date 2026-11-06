Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearch.overly-permissive-scheduled-action-invalid-type-rejected opensearch.vpc-endpoint-access-lifecycle
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: opensearch.overly-permissive-scheduled-action-invalid-type-rejected|run#11|AssertionError|assert control_message == expected, f"expected message {expected!r}, got {control_message!r}"
Issue-Key: opensearch.vpc-endpoint-access-lifecycle|run#22|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Commit: 92e1207f9957e93edc6c957f01f4316de742e856

# opensearch: two not-found refusals answer emulator-authored messages — UpdateScheduledAction (unknown ActionID) and AuthorizeVpcEndpointAccess (unresolvable domain) (2 causes)

> _Note: this issue was identified against the code base at commit `92e1207f9957e93edc6c957f01f4316de742e856`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateScheduledAction

## Description
2 causes. Both are ResourceNotFoundException refusals whose error CODE is right but whose MESSAGE is emulator-authored: the handlers reuse this crate's generic resource-lookup messages (and interpolate request values into them) instead of the fixed sentences AWS words for these two refusal classes. An HTTP status, an error code and an error message are all requirements, so each of these is a defect on its own.

Cause 1 of 2 — UpdateScheduledAction refuses an unknown ActionID with its own sentence (failing check: opensearch.overly-permissive-scheduled-action-invalid-type-rejected).
AWS: Smithy com.amazonaws.opensearch#UpdateScheduledAction, PUT /2021-01-01/opensearch/domain/{DomainName}/scheduledAction/update; DomainName @required @httpLabel @length(3,28) @pattern ^[a-z][a-z0-9\-]+$, ActionID @required, ActionType @required enum [JVM_HEAP_SIZE_TUNING, JVM_YOUNG_GEN_TUNING, SERVICE_SOFTWARE_UPDATE], ScheduleAt @required enum [NOW, OFF_PEAK_WINDOW, TIMESTAMP]; modelled errors BaseException(400), ConflictException(409), LimitExceededException(409), ResourceNotFoundException(409), SlotNotAvailableException(409), ValidationException(400), InternalException(500). The model does not state message text, so the message requirement is recorded behavior: for a request whose ActionType IS in the enum but whose ActionID names no scheduled action on an existing domain, AWS answers ResourceNotFoundException with the message "ReleaseVersion does not applies for this domain or release does not exist". That whole sentence is FIXED text (AWS's own wording for this class, grammatical oddity included); it names NO value from the request or state — there is no variable part to substitute, and in particular neither the ActionID nor the DomainName appears in it.
Emulator: it answers ResourceNotFoundException (code correct) with "Scheduled action <ActionID> doesn't exist for domain <DomainName>." — a sentence this service authored, built from the request's ActionID and DomainName. In the replayed check the body read "Scheduled action sim2real-action-de39db40d7cc doesn't exist for domain sim2real-osd-a105f7c588bb."; those two tokens are just that run's generated ids and are not part of any template — the defect is that request values are interpolated at all, into a message AWS words differently.
Why: a client that branches on the refusal text (the common way to tell "this release/action id is not applicable to this domain" from the other 409/404 classes this operation can return) never matches, so its handling branch never fires; and the deliberate non-disclosure in AWS's wording is lost, because the emulator confirms back the exact action id and domain name it failed to resolve.
Fix: the UpdateScheduledAction handler's scheduled-action lookup refusal path — it must raise the operation-specific not-found refusal with that fixed sentence and no interpolated request value, rather than this crate's generic per-resource lookup message.

Cause 2 of 2 — AuthorizeVpcEndpointAccess refuses an unresolvable domain through the generic domain-lookup message (failing check: opensearch.vpc-endpoint-access-lifecycle).
AWS: Smithy com.amazonaws.opensearch#AuthorizeVpcEndpointAccess, POST /2021-01-01/opensearch/domain/{DomainName}/authorizeVpcEndpointAccess; DomainName @required @httpLabel @length(3,28) @pattern ^[a-z][a-z0-9\-]+$, Service enum [application.opensearchservice.amazonaws.com], Account @pattern ^[0-9]+$; modelled errors BaseException(400), DisabledOperationException(409), LimitExceededException(409), ResourceNotFoundException(409), ValidationException(400), InternalException(500). The model does not state message text, so the message requirement is recorded behavior: for a DomainName that does not resolve to a VPC-endpoint-capable domain the caller may act on, AWS answers ResourceNotFoundException with "Either the domain doesn't exist, it doesn't support creation of VPC endpoints, or you don't have permission to create a VPC endpoint for the domain." The whole sentence is FIXED text — the VPC-endpoint-access paths deliberately collapse absence, unsupported-domain and not-authorized into one sentence and name NO request value, so there is no variable part.
Emulator: it answers ResourceNotFoundException (code correct) with the generic domain-lookup message "Domain not found: <DomainName>" — fixed prefix "Domain not found: " authored by this crate plus the request's DomainName (the replayed check saw 'Domain not found: sim2real-behav-os-missing-32'; that token is only the generated domain name of that run).
Why: a client that matches on the refusal text to distinguish "the domain is gone" from "this domain does not support VPC endpoints" or "you are not authorized" gets a sentence AWS never sends, so that branch never fires; and the intentional ambiguity of AWS's wording is lost because the emulator confirms the exact domain name it could not find.
Fix: the AuthorizeVpcEndpointAccess handler's domain-resolution refusal path — it must raise the VPC-endpoint-access-specific not-found/not-permitted refusal with that fixed sentence instead of reusing this crate's shared domain-not-found constructor. Related but NOT covering this cause: open issue 20261030T001859Z-es-revokevpcendpointaccess-not-found-refusal-replaces-aws-s.md describes the same wording defect for RevokeVpcEndpointAccess on the `es` surface; it carries neither this operation nor this check's replay key, so fixing it alone leaves AuthorizeVpcEndpointAccess diverging.

Traces: cause 1 — opensearch.overly-permissive-scheduled-action-invalid-type-rejected; the emulator wording is produced by crate::state::scheduled_action_not_found, called at services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/update_scheduled_action.rs:216-220. Cause 2 — opensearch.vpc-endpoint-access-lifecycle; the emulator wording is produced by crate::state::domain_not_found, called at services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/authorize_vpc_endpoint_access.rs:177-181.

## Evidence
Cause 1 (check opensearch.overly-permissive-scheduled-action-invalid-type-rejected): after CreateDomain and a ValidationException-returning UpdateScheduledAction with ActionType='NOT_A_REAL_ACTION_TYPE' (that half passes), the same call with the VALID ActionType='SERVICE_SOFTWARE_UPDATE' and the same nonexistent ActionID is expected to be ResourceNotFoundException with message "ReleaseVersion does not applies for this domain or release does not exist" (entirely fixed text).
Actual (emulator): ResourceNotFoundException with "Scheduled action sim2real-action-de39db40d7cc doesn't exist for domain sim2real-osd-a105f7c588bb." — fixed emulator-authored frame "Scheduled action <ActionID> doesn't exist for domain <DomainName>." filled with the request's ActionID and DomainName.
Assertion: AssertionError: expected message 'ReleaseVersion does not applies for this domain or release does not exist', got "Scheduled action sim2real-action-de39db40d7cc doesn't exist for domain sim2real-osd-a105f7c588bb."

Cause 2 (check opensearch.vpc-endpoint-access-lifecycle): the happy path (AuthorizeVpcEndpointAccess for service principal application.opensearchservice.amazonaws.com on a VPC domain, PrincipalType 'AWS Service', ListVpcEndpointAccess round-trip, RevokeVpcEndpointAccess removal) all passed; the final negative step diverges. Expected: ResourceNotFoundException with message "Either the domain doesn't exist, it doesn't support creation of VPC endpoints, or you don't have permission to create a VPC endpoint for the domain." (entirely fixed text, naming no request value).
Actual (emulator): ResourceNotFoundException with 'Domain not found: sim2real-behav-os-missing-32' — fixed prefix "Domain not found: " plus the request's DomainName.
Assertion: AssertionError: expected message "Either the domain doesn't exist, it doesn't support creation of VPC endpoints, or you don't have permission to create a VPC endpoint for the domain.", got 'Domain not found: sim2real-behav-os-missing-32'

Model cited for both: errors/inputs as listed in the description; the Smithy model states no message text, so both message requirements are recorded behavior. Error codes matched in both cases — only the message text diverges.

## Steps to Reproduce
Cause 1:
1. CreateDomain a minimal active domain (t3.small.search, 1 node, gp3 10 GiB) and wait for Created=true/Processing=false.
2. Call UpdateScheduledAction with DomainName=<that domain>, ActionID=<an id no scheduled action uses>, ActionType='SERVICE_SOFTWARE_UPDATE', ScheduleAt='NOW'.
3. Observe ResourceNotFoundException with message "Scheduled action <ActionID> doesn't exist for domain <DomainName>."
4. Expected: ResourceNotFoundException with the fixed message "ReleaseVersion does not applies for this domain or release does not exist", containing no request value.
5. Re-replay check opensearch.overly-permissive-scheduled-action-invalid-type-rejected to confirm.

Cause 2:
1. Create a VPC, subnet and security group, create the es.amazonaws.com service-linked role, and CreateDomain a VPC-attached domain; wait for it to become active.
2. Call AuthorizeVpcEndpointAccess with DomainName=<a well-formed name no domain exists under> and Service='application.opensearchservice.amazonaws.com'.
3. Observe ResourceNotFoundException with message 'Domain not found: <DomainName>'.
4. Expected: ResourceNotFoundException with the fixed sentence "Either the domain doesn't exist, it doesn't support creation of VPC endpoints, or you don't have permission to create a VPC endpoint for the domain.", which must not name the requested domain.
5. Re-replay check opensearch.vpc-endpoint-access-lifecycle to confirm.
