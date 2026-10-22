Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3outposts.create-endpoint-customer-owned-ip-pool-without-access-type-rejected s3outposts.create-endpoint-invalid-outpost-id-format-validation-error s3outposts.create-endpoint-invalid-security-group-id-format-validation-error s3outposts.create-endpoint-invalid-subnet-id-format-validation-error s3outposts.create-endpoint-nonexistent-outpost-returns-not-found s3outposts.delete-endpoint-invalid-endpoint-id-format-validation-error s3outposts.delete-endpoint-nonexistent-endpoint-returns-not-found s3outposts.list-shared-endpoints-invalid-outpost-id-format-validation-error
Issue-Shape-Id: com.amazonaws.s3outposts#S3Outposts
Issue-Key: s3outposts.create-endpoint-customer-owned-ip-pool-without-access-type-rejected|run#8|AssertionError|assert baseline_message == OUTPOST_NOT_FOUND, f"message={baseline_message!r}"
Issue-Key: s3outposts.create-endpoint-invalid-outpost-id-format-validation-error|run#10|AssertionError|assert control_message == ACCESS_DENIED, f"message={control_message!r}"
Issue-Key: s3outposts.create-endpoint-invalid-security-group-id-format-validation-error|run#10|AssertionError|assert control_message == ACCESS_DENIED, f"message={control_message!r}"
Issue-Key: s3outposts.create-endpoint-invalid-subnet-id-format-validation-error|run#10|AssertionError|assert control_message == OUTPOST_NOT_FOUND, f"message={control_message!r}"
Issue-Key: s3outposts.create-endpoint-nonexistent-outpost-returns-not-found|run#11|AssertionError|assert message == OUTPOST_NOT_FOUND, f"message={message!r}"
Issue-Key: s3outposts.delete-endpoint-invalid-endpoint-id-format-validation-error|run#9|AssertionError|assert control_message == ACCESS_DENIED, f"message={control_message!r}"
Issue-Key: s3outposts.delete-endpoint-nonexistent-endpoint-returns-not-found|run#6|AssertionError|assert message == OUTPOST_NOT_FOUND, f"message={message!r}"
Issue-Key: s3outposts.list-shared-endpoints-invalid-outpost-id-format-validation-error|run#8|AssertionError|assert control_message == ACCESS_DENIED, f"message={control_message!r}"
Issue-Commit: 4cbced68d8f61275ceb5b427ff8478e9aa7bafe5

# s3outposts ResourceNotFoundException Message is synthesized from the request ids instead of the service's two fixed not-found texts (2 causes)

> _Note: this issue was identified against the code base at commit `4cbced68d8f61275ceb5b427ff8478e9aa7bafe5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEndpoint, DeleteEndpoint, ListSharedEndpoints

## Description
S3 on Outposts answers an unresolvable Outpost/endpoint reference with ResourceNotFoundException (HTTP 404) whose Message is one of TWO FIXED service strings, selected by the FORM of the OutpostId. The emulator instead builds the Message by formatting the request's own identifiers into a template, so both strings are wrong on every not-found path and the two cases are no longer distinguishable. HTTP status (404) and error code (ResourceNotFoundException) are correct in all eight failing checks — the divergence is the Message rule, and the rule is known because the recorded behavior shows the text is a CONSTANT per OutpostId form, never derived from the request, consistently across 8 independent checks and 3 operations.

No prior s3outposts issue exists (list_existing_issues query='s3outposts' returns none); nothing in this group is marked ALREADY FILED or OWNED.

=== Cause 1 of 2: op-/Outpost-owned-id form must answer Message "Access Denied" ===
AWS: Smithy (services/service-com-amazonaws-s3outposts-s3outposts/smithy-model/com-amazonaws-s3outposts-s3outposts.json): ResourceNotFoundException {Message: ErrorMessage}, @error client, @httpError 404; OutpostId @pattern ^(op-[a-f0-9]{17}|\d{12}|ec2)$. The model does not state the message text, so this is RECORDED BEHAVIOR ONLY: for an OutpostId in the op-[a-f0-9]{17} form that does not resolve to an Outpost with S3 capacity for the caller, the recordings show ResourceNotFoundException with Message exactly "Access Denied" — the service masks the existence of such an Outpost rather than reporting which id failed. The same masked text is returned for an endpoint lookup under such an Outpost (DeleteEndpoint with a well-formed, never-issued EndpointId) and for ListSharedEndpoints. Hypothesis: the op- form names a physical Outpost the caller may not be entitled to see at all, so existence is withheld.
Emulator: returns ResourceNotFoundException/404 with a request-derived Message: 'Outpost op-00000000000000000 not found' (CreateEndpoint, ListSharedEndpoints) and 'Endpoint A1B2C3D4E5F6G7H8I9J not found on Outpost op-00000000000000000' (DeleteEndpoint).
Why: a client cannot tell an authorization-masked lookup from a plain miss, and the emulator discloses the queried id where AWS deliberately does not — any caller or test asserting on the service's masking contract (or scrubbing logs that must not echo Outpost ids) behaves differently against the emulator.
Fix: the not-found message construction shared by the CreateEndpoint / DeleteEndpoint / ListSharedEndpoints handlers — the `outpost_not_found` and `endpoint_not_found` helpers in the s3outposts service state module, which both format the request's ids into the text.
Traces: s3outposts.create-endpoint-invalid-outpost-id-format-validation-error, s3outposts.create-endpoint-invalid-security-group-id-format-validation-error, s3outposts.delete-endpoint-invalid-endpoint-id-format-validation-error, s3outposts.list-shared-endpoints-invalid-outpost-id-format-validation-error. In each of these the ValidationException half of the check PASSES (pattern rejection and its "1 validation error detected: ..." text are correct); only the positive-control not-found Message diverges.

=== Cause 2 of 2: 12-digit shared-account id form must answer the fixed outpost-not-found text ===
AWS: Same shapes as above; recorded behavior only. For an OutpostId in the \d{12} (shared/account-style) branch of the pattern that does not resolve, the recordings show ResourceNotFoundException/404 with Message exactly "The specified outpost does not exist, or does not have S3 capacity provisioned on it." This holds for CreateEndpoint regardless of which other members are supplied (with or without CustomerOwnedIpv4Pool, with an unknown-but-well-formed SubnetId/SecurityGroupId), and it also governs DeleteEndpoint: a well-formed but never-issued EndpointId under an unresolvable 12-digit OutpostId reports the OUTPOST-level text, not an endpoint-level one, i.e. the Outpost lookup is resolved first and its failure text wins.
Emulator: returns 404/ResourceNotFoundException with 'Outpost 999999999999 not found' / 'Outpost 111122223333 not found', and for DeleteEndpoint 'Endpoint B1C2D3E4F5G6H7I8J9K not found on Outpost 111122223333' — an endpoint-level message where AWS reports the Outpost-level one, showing the emulator checks the (outpost,endpoint) pair as one key instead of resolving the Outpost first.
Why: callers that surface the service text to operators get a message that never occurs in AWS, and the endpoint-level phrasing misattributes the failure to the endpoint when the Outpost itself is what is missing or has no S3 capacity provisioned — the actionable remediation is different.
Fix: same component as Cause 1, plus the Outpost-before-endpoint resolution order in the DeleteEndpoint handler (it currently looks up the (outpost_id, endpoint_id) tuple directly and never distinguishes an unknown Outpost from an unknown endpoint).
Traces: s3outposts.create-endpoint-nonexistent-outpost-returns-not-found, s3outposts.create-endpoint-customer-owned-ip-pool-without-access-type-rejected, s3outposts.create-endpoint-invalid-subnet-id-format-validation-error, s3outposts.delete-endpoint-nonexistent-endpoint-returns-not-found.

## Evidence
Suite: 5/13 passed (38.5%), 8 mismatched, all in s3outposts. Status and error code matched in every case; only ResourceNotFoundException.Message diverged.

Cause 1 (expected Message "Access Denied"):
- create-endpoint-invalid-outpost-id-format-validation-error: CreateEndpoint(OutpostId='op-00000000000000000', SubnetId='subnet-00000000', SecurityGroupId='sg-00000000') -> expected ResourceNotFoundException/'Access Denied'; got ResourceNotFoundException/'Outpost op-00000000000000000 not found' (AssertionError: message='Outpost op-00000000000000000 not found').
- create-endpoint-invalid-security-group-id-format-validation-error: same control call -> got 'Outpost op-00000000000000000 not found'.
- list-shared-endpoints-invalid-outpost-id-format-validation-error: ListSharedEndpoints(OutpostId='op-00000000000000000') -> got 'Outpost op-00000000000000000 not found'.
- delete-endpoint-invalid-endpoint-id-format-validation-error: DeleteEndpoint(EndpointId='A1B2C3D4E5F6G7H8I9J', OutpostId='op-00000000000000000') -> got 'Endpoint A1B2C3D4E5F6G7H8I9J not found on Outpost op-00000000000000000'.

Cause 2 (expected Message "The specified outpost does not exist, or does not have S3 capacity provisioned on it."):
- create-endpoint-nonexistent-outpost-returns-not-found: CreateEndpoint(OutpostId='111122223333', SubnetId='subnet-aaaaaaaa', SecurityGroupId='sg-aaaaaaaa') -> got 'Outpost 111122223333 not found'.
- create-endpoint-customer-owned-ip-pool-without-access-type-rejected: CreateEndpoint(OutpostId='999999999999', SubnetId='subnet-00000000', SecurityGroupId='sg-00000000') -> got 'Outpost 999999999999 not found'.
- create-endpoint-invalid-subnet-id-format-validation-error: control CreateEndpoint(OutpostId='999999999999', SubnetId='subnet-00000000', SecurityGroupId='sg-00000000') -> got 'Outpost 999999999999 not found'.
- delete-endpoint-nonexistent-endpoint-returns-not-found: DeleteEndpoint(EndpointId='B1C2D3E4F5G6H7I8J9K', OutpostId='111122223333') -> expected the Outpost-level fixed text; got 'Endpoint B1C2D3E4F5G6H7I8J9K not found on Outpost 111122223333'.

Source of the synthesized text (file/line citations):
- services/service-com-amazonaws-s3outposts-s3outposts/src/ops/create_endpoint.rs:198-203 — `if !store.outposts.contains_key(&input.outpost_id) { ... crate::state::outpost_not_found(&input.outpost_id) ... }`.
- services/service-com-amazonaws-s3outposts-s3outposts/src/ops/delete_endpoint.rs:180-190 — resolves the (outpost_id, endpoint_id) tuple as a single key and returns `crate::state::endpoint_not_found(&input.outpost_id, &input.endpoint_id)`, so an unknown Outpost is reported as an unknown endpoint.
- The message templates themselves live in the `outpost_not_found` / `endpoint_not_found` helpers of services/service-com-amazonaws-s3outposts-s3outposts/src/state.rs.

Model evidence: smithy_query(query='shape', shape='ResourceNotFoundException') -> {errorKind: client, httpError: 404, members: {Message: ErrorMessage}}; smithy_query(query='operation', operation='CreateEndpoint,DeleteEndpoint,ListSharedEndpoints') -> OutpostId pattern ^(op-[a-f0-9]{17}|\d{12}|ec2)$, EndpointId pattern ^[a-zA-Z0-9]{19}$. The model constrains the ids but states no message text, hence 'recorded behavior only' for the two fixed strings.

## Steps to Reproduce
Boot the emulator and point a botocore s3outposts client at it.

Cause 1 — op- form must be masked as "Access Denied":
1. s3o.create_endpoint(OutpostId='op-'+'0'*17, SubnetId='subnet-'+'0'*8, SecurityGroupId='sg-'+'0'*8)
   Expect ClientError Code='ResourceNotFoundException' (HTTP 404), Message='Access Denied'. Observed Message='Outpost op-00000000000000000 not found'.
2. s3o.list_shared_endpoints(OutpostId='op-'+'0'*17) — same expectation, same divergence.
3. s3o.delete_endpoint(EndpointId='A1B2C3D4E5F6G7H8I9J', OutpostId='op-'+'0'*17)
   Expect Message='Access Denied'. Observed 'Endpoint A1B2C3D4E5F6G7H8I9J not found on Outpost op-00000000000000000'.

Cause 2 — 12-digit form must return the fixed Outpost text:
4. s3o.create_endpoint(OutpostId='111122223333', SubnetId='subnet-'+'a'*8, SecurityGroupId='sg-'+'a'*8)
   Expect Code='ResourceNotFoundException' (404), Message='The specified outpost does not exist, or does not have S3 capacity provisioned on it.' Observed 'Outpost 111122223333 not found'.
5. Repeat 4 with CustomerOwnedIpv4Pool='ipv4pool-coip-'+'0'*17 and AccessType unset — the response must be byte-identical to step 4 (no pairing validation); observed the same wrong Message.
6. s3o.delete_endpoint(EndpointId='B1C2D3E4F5G6H7I8J9K', OutpostId='111122223333')
   Expect the Outpost-level fixed text from step 4. Observed 'Endpoint B1C2D3E4F5G6H7I8J9K not found on Outpost 111122223333'.

Control that already passes and must keep passing: a malformed id (OutpostId='not-a-valid-outpost-id', SubnetId='bad-subnet-id', SecurityGroupId='bad-sg-id', EndpointId='short-id') still returns ValidationException/400 with "1 validation error detected: Value '<v>' at '<member>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>".

Re-replay: the 8 check ids in trace_ids.
