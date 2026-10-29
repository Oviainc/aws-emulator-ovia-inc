Issue-Source: trace-sweep-divergence
Issue-Trace: opensearchserverless-collection-error-paths
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: opensearchserverless-collection-error-paths opensearchserverless-security-config-errors opensearchserverless-security-config-iamfederation-lifecycle opensearchserverless-security-config-saml-lifecycle opensearchserverless-collection-lifecycle opensearchserverless-vpc-endpoint-errors
Issue-Shape-Id: com.amazonaws.opensearchserverless#OpenSearchServerless
Issue-Key: aoss|DeleteCollectionGroup|4xx->4xx|$.message
Issue-Key: aoss|GetSecurityConfig|4xx->4xx|$.message
Issue-Key: aoss|UpdateSecurityConfig|4xx->4xx|$.message
Issue-Key: aoss|DeleteSecurityConfig|4xx->4xx|$.message
Issue-Key: aoss|CreateSecurityConfig|4xx->4xx|$.message
Issue-Key: aoss|UpdateCollection|4xx->4xx|$.message
Issue-Key: aoss|DeleteVpcEndpoint|4xx->4xx|$.message
Issue-Key: aoss|UpdateVpcEndpoint|4xx->4xx|$.__type
Issue-Key: aoss|UpdateVpcEndpoint|4xx->4xx|$.message
Issue-Key: aoss|BatchGetVpcEndpoint|2xx->2xx|$.vpcEndpointErrorDetails[*].errorMessage
Issue-Commit: a04a180e27a1be47457c995aec938403d1945997

# aoss not-found and conflict refusals are emulator-authored prose, and a missing VPC endpoint is misclassified (9 causes)

> _Note: this issue was identified against the code base at commit `a04a180e27a1be47457c995aec938403d1945997`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteCollectionGroup

## Description
NINE net-new root causes in the aoss (OpenSearch Serverless) refusal plane, all from replaying the wire corpus simparity-wire-aws-v1. In every one of them the HTTP status the emulator answers is already right (400 for a client error); what diverges is the error CODE and/or the error MESSAGE text, and an error message is a requirement, not decoration. Each cause is independent and must be closed on its own.

=== Cause 1 of 9: DeleteCollectionGroup's not-found message is reworded and drops the quoting ===
AWS: Smithy: DeleteCollectionGroup takes id (CollectionGroupId, @length 3..40, @pattern ^[a-z0-9]{3,40}$) and models ResourceNotFoundException. Recorded behavior: for an id no collection group carries, the refusal is HTTP 400 with message `CollectionGroup with ID '<id>' cannot be found.` — fixed text (`CollectionGroup with ID `, the SINGLE-QUOTED id, ` cannot be found.` with the trailing period), where <id> is the request's id.
Emulator: `Collection group with ID <id> is not found` — different capitalisation of the resource kind, no quotes around the id, no trailing period, and `is not found` instead of `cannot be found.`.
Why: the error code is the generic not-found, so the message is the only discriminator a client or a human operator has; a client matching AWS's text does not recognise the refusal.
Fix: the DeleteCollectionGroup handler's not-found refusal message in the opensearchserverless crate.
Traces: opensearchserverless-collection-error-paths#task#6.

=== Cause 2 of 9: GetSecurityConfig's not-found message is reworded ===
AWS: Smithy: GetSecurityConfig takes id (SecurityConfigId, @length 1..100) and models ResourceNotFoundException. Recorded behavior: HTTP 400 with message `Security config with id: <id> does not exist` — fixed text with a COLON after `id` and the tail `does not exist`; <id> is the request's id (of the form <type>/<account>/<name>).
Emulator: `Security config with id <id> is not found` — no colon, and `is not found` for `does not exist`.
Why: a client keying on AWS's message text mis-classifies a genuine not-found.
Fix: the GetSecurityConfig handler's not-found message. NOTE: open behavioral issue 20261029T191222Z-opensearchserverless-10-net-new-causes-in-the-refusal.md (Cause 3) describes the same wording from the behavioral suite; this replay step was NOT marked owned by it, so it is recorded here to be re-replayed, and this cause may close with that one if its fix lands first.
Traces: opensearchserverless-security-config-errors#task#3 (same cluster: opensearchserverless-security-config-iamfederation-lifecycle#teardown#7, opensearchserverless-security-config-saml-lifecycle#teardown#6).

=== Cause 3 of 9: UpdateSecurityConfig's not-found message names the id, where AWS names the policy name and type ===
AWS: Smithy: UpdateSecurityConfig takes id + configVersion and models ResourceNotFoundException. Recorded behavior: HTTP 400 with message `Policy with name <name> and type <type> is not found` — fixed text whose variables are the NAME and the TYPE parsed out of the request's id (e.g. id `saml/<account>/<name>` yields name <name>, type `saml`). AWS words this operation's refusal differently from GetSecurityConfig's (Cause 2), so the two are not interchangeable.
Emulator: `Security config with id <id> is not found` — the same sentence it uses for the read path, rendering the whole id.
Why: both the fixed text and the rendered variable differ, so a client that matches AWS's refusal on update sees an unrecognised message.
Fix: the UpdateSecurityConfig handler's not-found refusal message.
Traces: opensearchserverless-security-config-errors#task#4.

=== Cause 4 of 9: DeleteSecurityConfig's not-found message likewise names the id instead of name+type ===
AWS: Smithy: DeleteSecurityConfig takes id and models ResourceNotFoundException. Recorded behavior: HTTP 400 with `Policy with name <name> and type <type> is not found`, the same form as the update path — name and type parsed from the request's id.
Emulator: `Security config with id <id> is not found`.
Why: same as Cause 3 — the only discriminator between a missing config and any other 400 is text the emulator does not reproduce.
Fix: the DeleteSecurityConfig handler's not-found refusal message.
Traces: opensearchserverless-security-config-errors#task#6.

=== Cause 5 of 9: CreateSecurityConfig's already-exists message names the wrong resource kind ===
AWS: Smithy: CreateSecurityConfig takes type + name (ConfigName, @length 3..32, @pattern ^[a-z][a-z0-9-]+$) and models ConflictException. Recorded behavior: re-creating an existing configuration is refused HTTP 400, code ConflictException, with message `Policy with name <name> and type <type> already exists` — fixed text, <name> from the request's name and <type> from the request's type. AWS calls a security configuration a `Policy` in this sentence.
Emulator: `Security config with name <name> and type <type> already exists` — correct code and status, wrong fixed text.
Why: a caller that distinguishes a duplicate-name conflict from another conflict by message text cannot.
Fix: the CreateSecurityConfig handler's duplicate-name refusal message.
Traces: opensearchserverless-security-config-errors#task#2.

=== Cause 6 of 9: UpdateCollection's wrong-status refusal is reworded and omits that UPDATE_FAILED is updatable ===
AWS: Smithy: UpdateCollection takes id (CollectionId) and models ValidationException. Recorded behavior: updating a collection that is not in an updatable state is refused HTTP 400 with `Collection Id <id> cannot be updated because it is not in ACTIVE or UPDATE_FAILED Status` — entirely fixed text apart from <id>, the request's collection id; the sentence names BOTH updatable statuses, ACTIVE and UPDATE_FAILED, with `Status` capitalised.
Emulator: `Collection with ID <id> is in status <status> and cannot be updated; it must be ACTIVE` — different fixed text, and it states ACTIVE as the only updatable status, which also describes a narrower precondition than AWS's.
Why: the message is the only signal that the caller should wait and retry; a client matching AWS's wording, or reading it to learn that an UPDATE_FAILED collection can be updated, is misinformed.
Fix: the UpdateCollection handler's collection-status precondition and its refusal message.
Traces: opensearchserverless-collection-lifecycle#task#4.

=== Cause 7 of 9: DeleteVpcEndpoint's not-found message is generic and drops the endpoint id ===
AWS: Smithy: DeleteVpcEndpoint takes id (VpcEndpointId, @pattern ^vpce-[0-9a-z]*$) and models ResourceNotFoundException. Recorded behavior: HTTP 400 with `VpcEndpoint with ID '<id>' cannot be found.` — fixed text with the SINGLE-QUOTED request id, `VpcEndpoint` spelled as one word, and a trailing period.
Emulator: `The specified Vpc Endpoint is not found.` — different fixed text, `Vpc Endpoint` as two words, and the id the caller asked about is not rendered at all.
Why: the id is the only way a caller batching deletes can tell WHICH endpoint was missing; a generic sentence makes the refusal unattributable, and the text does not match AWS's.
Fix: the DeleteVpcEndpoint handler's not-found refusal message.
Traces: opensearchserverless-vpc-endpoint-errors#task#3.

=== Cause 8 of 9: UpdateVpcEndpoint classifies a missing endpoint as ValidationException where AWS uses ConflictException ===
AWS: Smithy: UpdateVpcEndpoint takes id (^vpce-[0-9a-z]*$) and models exactly ConflictException, InternalServerException and ValidationException — notably NO ResourceNotFoundException, which is why AWS reports a missing endpoint as a conflict. Recorded behavior: HTTP 400 with __type `com.amazon.aoss#ConflictException` and message `The VpcEndpoint with Id '<id>' is not exist.` — AWS's own fixed text (ungrammatical, `Id` mixed-case, single-quoted id, trailing period) with <id> the request's endpoint id.
Emulator: HTTP 400 with __type `ValidationException` and message `The specified Vpc Endpoint is not found.`.
Why: the error CODE drives a client's branch — a ValidationException means 'the request is malformed, never retry it', a ConflictException means 'the resource state conflicts'. The emulator sends a caller down the wrong branch, and the message is wrong too.
Fix: the UpdateVpcEndpoint handler's missing-endpoint classification (raise ConflictException with that message instead of ValidationException).
Traces: opensearchserverless-vpc-endpoint-errors#task#2.

=== Cause 9 of 9: BatchGetVpcEndpoint's NOT_FOUND errorMessage spells VpcEndpoint as two words ===
AWS: Smithy: BatchGetVpcEndpoint answers 200 with vpcEndpointErrorDetails of VpcEndpointErrorDetail {id, errorCode, errorMessage}. Recorded behavior: for an id that does not exist the entry carries errorMessage `The specified VpcEndpoint is not found.` — entirely fixed text, `VpcEndpoint` as ONE word.
Emulator: `The specified Vpc Endpoint is not found.` (two words). The id and errorCode are correct.
Why: the per-id error detail is the only machine-readable signal in an otherwise 200 batch response, so a client matching AWS's fixed message never recognises the missing endpoint.
Fix: the BatchGetVpcEndpoint handler's error-detail serializer. NOTE: open behavioral issue 20261029T191222Z-opensearchserverless-5-net-new-causes-in-the-state.md (Cause 2) reports the same spelling from the behavioral suite; this replay step was NOT marked owned by it, so it is recorded here to be re-replayed and may close together with it.
Traces: opensearchserverless-vpc-endpoint-errors#task#4.

## Evidence
All nine clusters come from the simparity-wire-aws-v1 replay (status 400 expected and 400 served in every one; only $.__type and/or $.message diverge).
Cause 1 — opensearchserverless-collection-error-paths#task#6 DeleteCollectionGroup: $.message expected "CollectionGroup with ID '<id>' cannot be found."; got 'Collection group with ID <id> is not found' (the 40-character id in the recording is the request's own value, not part of the template).
Cause 2 — opensearchserverless-security-config-errors#task#3 GetSecurityConfig: expected 'Security config with id: <id> does not exist'; got 'Security config with id <id> is not found'. Cluster also covers opensearchserverless-security-config-iamfederation-lifecycle#teardown#7 and opensearchserverless-security-config-saml-lifecycle#teardown#6.
Cause 3 — opensearchserverless-security-config-errors#task#4 UpdateSecurityConfig: expected 'Policy with name <name> and type saml is not found'; got 'Security config with id saml/<account>/<name> is not found'.
Cause 4 — opensearchserverless-security-config-errors#task#6 DeleteSecurityConfig: expected 'Policy with name <name> and type saml is not found'; got 'Security config with id saml/<account>/<name> is not found'.
Cause 5 — opensearchserverless-security-config-errors#task#2 CreateSecurityConfig: expected 'Policy with name <name> and type saml already exists'; got 'Security config with name <name> and type saml already exists'.
Cause 6 — opensearchserverless-collection-lifecycle#task#4 UpdateCollection: expected 'Collection Id <id> cannot be updated because it is not in ACTIVE or UPDATE_FAILED Status'; got 'Collection with ID <id> is in status CREATING and cannot be updated; it must be ACTIVE'.
Cause 7 — opensearchserverless-vpc-endpoint-errors#task#3 DeleteVpcEndpoint: expected "VpcEndpoint with ID '<vpce-id>' cannot be found."; got 'The specified Vpc Endpoint is not found.'.
Cause 8 — opensearchserverless-vpc-endpoint-errors#task#2 UpdateVpcEndpoint: expected $.__type 'com.amazon.aoss#ConflictException' and $.message "The VpcEndpoint with Id '<vpce-id>' is not exist."; got $.__type 'ValidationException' and $.message 'The specified Vpc Endpoint is not found.' (the namespace prefix itself is normalised by the replay harness — the diverging part is the code name ConflictException vs ValidationException).
Cause 9 — opensearchserverless-vpc-endpoint-errors#task#4 BatchGetVpcEndpoint (200/200): $.vpcEndpointErrorDetails[0].errorMessage expected 'The specified VpcEndpoint is not found.'; got 'The specified Vpc Endpoint is not found.'.

## Steps to Reproduce
Boot the emulator from this checkout and replay the aoss wire fixtures; each cause is reachable directly with an SDK:
Cause 1: delete_collection_group(id='<40 chars of [a-z0-9] never created>') -> inspect message.
Cause 2: get_security_config(id='saml/<account>/<never-created-name>').
Cause 3: update_security_config(id='saml/<account>/<never-created-name>', configVersion='<any token matching the PolicyVersion pattern>').
Cause 4: delete_security_config(id='saml/<account>/<never-created-name>').
Cause 5: create_security_config(type='saml', name=<n>, samlOptions={...}) twice.
Cause 6: create_collection(name=<n>, encryptionConfig/policy present) and immediately, while it is still CREATING, update_collection(id=<id>, description='x').
Cause 7: delete_vpc_endpoint(id='vpce-00000000000000000').
Cause 8: update_vpc_endpoint(id='vpce-00000000000000000', addSubnetIds=[...]) -> inspect the error code and message.
Cause 9: batch_get_vpc_endpoint(ids=['vpce-00000000000000000']) -> inspect vpcEndpointErrorDetails[0].errorMessage.
