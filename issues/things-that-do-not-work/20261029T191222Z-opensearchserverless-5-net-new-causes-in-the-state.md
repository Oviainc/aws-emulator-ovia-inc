Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearchserverless.index-mapping-json-round-trip opensearchserverless.vpc-endpoint-deletion-transition-window opensearchserverless.vpc-endpoint-lifecycle-transitions opensearchserverless.deploy-collection-cfn-becomes-active opensearchserverless.deploy-vpc-endpoint-cfn-becomes-available opensearchserverless.collection-endpoint-requires-aoss-sigv4-service-name opensearchserverless.data-access-policy-read-permission-without-write-blocks-indexing opensearchserverless.deleting-network-policy-stops-the-live-collection-endpoint-serving opensearchserverless.server-side-network-policy-enforces-vpc-only-access
Issue-Shape-Id: com.amazonaws.opensearchserverless#OpenSearchServerless
Issue-Key: opensearchserverless.index-mapping-json-round-trip|run#23|AssertionError|assert after_update == updated_properties, (
Issue-Key: opensearchserverless.vpc-endpoint-deletion-transition-window|run#20|AssertionError|assert errors[0]["errorMessage"] == "The specified VpcEndpoint is not found.", (
Issue-Key: opensearchserverless.vpc-endpoint-lifecycle-transitions|run#30|AssertionError|assert gone["vpcEndpointErrorDetails"] == expected_detail, (
Issue-Key: opensearchserverless.deploy-collection-cfn-becomes-active|_deploy_cfn#6|AssertionError|assert status == "CREATE_COMPLETE", f"stack ended in {status}"
Issue-Key: opensearchserverless|BatchGetVpcEndpoint|ValidationException
Issue-Key: opensearchserverless.collection-endpoint-requires-aoss-sigv4-service-name|_wait_index_creatable#14|Timeout|time.sleep(5.0)
Issue-Key: opensearchserverless.data-access-policy-read-permission-without-write-blocks-indexing|_poll_until#18|Timeout|time.sleep(interval_s)
Issue-Key: opensearchserverless.deleting-network-policy-stops-the-live-collection-endpoint-serving|_wait_index_creatable#14|Timeout|time.sleep(5.0)
Issue-Key: opensearchserverless.server-side-network-policy-enforces-vpc-only-access|run#45|Timeout|time.sleep(10)
Issue-Commit: 92c3b140342b1bc1fab2a232b8cee3f25ea78ff5

# opensearchserverless: 5 net-new causes in the state/representation plane — UpdateIndex drops the existing mapping, BatchGetVpcEndpoint's NOT_FOUND message is misspelled, both CloudFormation AOSS resources are broken, and the collection data plane serves the AWS-JSON front door

> _Note: this issue was identified against the code base at commit `92c3b140342b1bc1fab2a232b8cee3f25ea78ff5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchGetVpcEndpoint

## Description
FIVE net-new root causes outside the refusal plane, all grounded in failing behavioral checks of the opensearchserverless service token. Each cause is independent and must be closed on its own.

=== Cause 1 of 5: UpdateIndex REPLACES the index mapping with the submitted schema instead of merging it ===
AWS: Smithy: UpdateIndex takes id + indexName + indexSchema (a `document` shape) and models only InternalServerException / ResourceNotFoundException / ValidationException — there is no 'replace' semantic in the model. Recorded behavior: submitting an indexSchema that adds one property leaves the previously created properties in place; a subsequent read of the index's mapping reports the union of the original properties and the added one.
Emulator: after UpdateIndex the mapping contains ONLY the submitted property — the four properties created with the index are gone.
Why: a caller that adds a field the documented way silently destroys the rest of its index schema, so every document field created earlier becomes unmapped; data written against the old mapping is no longer described by it.
Fix: the UpdateIndex handler's index-schema state write in the opensearchserverless crate — merge the submitted schema into the stored one rather than overwriting it. (Open issue 20261029T175047Z-opensearchserverless-updateindex-validation.md concerns a different aspect of the same handler — writing while the collection is not ACTIVE — and does not cover this.)
Traces: opensearchserverless.index-mapping-json-round-trip.

=== Cause 2 of 5: BatchGetVpcEndpoint's NOT_FOUND errorMessage is not AWS's text ===
AWS: Smithy: BatchGetVpcEndpoint returns vpcEndpointErrorDetails of VpcEndpointErrorDetail {id (pattern ^vpce-[0-9a-z]*$), errorCode, errorMessage}. Recorded behavior: for an id that no longer exists the entry is {id: <the requested vpce- id>, errorCode: 'NOT_FOUND', errorMessage: 'The specified VpcEndpoint is not found.'} — the message is entirely fixed text, spelled 'VpcEndpoint' as one word.
Emulator: errorMessage 'The specified Vpc Endpoint is not found.' (two words). id and errorCode are correct.
Why: the error detail is the only machine-readable signal in an otherwise 200 batch response, so a client matching AWS's fixed message never recognises the purged endpoint.
Fix: the BatchGetVpcEndpoint handler's error-detail serializer in this crate.
Traces: opensearchserverless.vpc-endpoint-deletion-transition-window, opensearchserverless.vpc-endpoint-lifecycle-transitions.

=== Cause 3 of 5: a CloudFormation stack containing AWS::OpenSearchServerless::Collection never reaches CREATE_COMPLETE ===
AWS: recorded behavior only (CloudFormation resource behavior is not in the opensearchserverless Smithy model). Recorded behavior: a stack whose only resource is AWS::OpenSearchServerless::Collection with Properties {Name, Type: 'SEARCH', EncryptionConfig: {AWSOwnedKey: true}} reaches CREATE_COMPLETE, its {"Ref": "Collection"} output is the collection id, and BatchGetCollection reports that collection ACTIVE with the requested name.
Emulator: the stack ends in DELETE_COMPLETE (created with OnFailure='DELETE', i.e. the resource create FAILED and the stack was rolled back and deleted), so no collection id is ever produced. Hypothesis: the AOSS Collection resource provider does not forward the template's inline EncryptionConfig to CreateCollection, so the create is refused by this crate's own missing-encryption validation.
Why: AOSS collections cannot be provisioned by CloudFormation at all — the deployment path most consumers use fails, and the failure is reported only as a rolled-back stack.
Fix: the CloudFormation resource provider for AWS::OpenSearchServerless::Collection (its mapping of template Properties — Name/Type/EncryptionConfig/Tags — onto CreateCollection, and its create-complete waiter).
Traces: opensearchserverless.deploy-collection-cfn-becomes-active.

=== Cause 4 of 5: AWS::OpenSearchServerless::VpcEndpoint's Ref resolves to the endpoint NAME, not its vpce- id ===
AWS: recorded behavior only (CloudFormation resource behavior is not modeled), cross-checked against Smithy: VpcEndpointId is constrained to pattern ^vpce-[0-9a-z]*$, so a name can never be a valid id. Recorded behavior: {"Ref": "<VpcEndpoint logical id>"} returns the endpoint's Id (a `vpce-...` value) which BatchGetVpcEndpoint accepts and reports ACTIVE.
Emulator: the stack reaches CREATE_COMPLETE but the Ref output is the endpoint's Name, so the follow-up call is refused: `ValidationException: 1 validation error detected: Value '<name>' at 'ids.1.member' failed to satisfy constraint: Member must satisfy regular expression pattern: vpce-[0-9a-z]*`.
Why: anything that consumes the stack's Ref/physical resource id — an output, another resource's property, a client reading the stack — gets a value the service's own API rejects, so a CloudFormation-provisioned VPC endpoint cannot be referenced.
Fix: the CloudFormation resource provider for AWS::OpenSearchServerless::VpcEndpoint — set the physical resource id (and Ref) to the created endpoint's `id`.
Traces: opensearchserverless.deploy-vpc-endpoint-cfn-becomes-available.

=== Cause 5 of 5: the published collectionEndpoint serves the AWS-JSON RPC front door, so no native data-plane behavior of a collection is observable ===
AWS: recorded behavior only (a collection's data plane is the OpenSearch REST API, not in the control-plane Smithy model). Recorded behavior: with an ACTIVE collection, a matching network policy, a data-access policy naming the signing principal and the IAM grant aoss:APIAccessAll, a SigV4-signed (service name `aoss`) request to `https://<collectionEndpoint>/...` — PUT /<index>, PUT /<index>/_doc/<id>, GET /<index>/_doc/<id>, POST /<index>/_search, GET /<index>/_mapping, none of which carry an x-amz-target header — is served by OpenSearch: accepted (200/201) when the policies allow it, 403 with body error.type 'forbidden' when the credential scope is wrong (e.g. signed `es` instead of `aoss`) or a permission was withdrawn, and 401 when no network policy admits the caller.
Emulator: the published endpoint answers HTTP 400 `{"__type":"SerializationException","message":"missing required header `x-amz-target`"}` to all of them (fixed text, nothing derived from the request) — the host reaches the AWS-JSON protocol dispatcher. Because these checks poll/retry while waiting for policy propagation, the suite reports them as 'Check timed out after 60.0s'.
Why: every data-plane guarantee of this service is unverifiable and, worse, indistinguishable: SigV4 scope validation (`aoss` vs `es`), per-permission data-access-policy enforcement (aoss:ReadDocument without aoss:WriteDocument), network-policy enforcement (VPC-only access) and the effect of deleting a network policy on a live endpoint all produce the same 400 whether they should allow or deny.
Fix: the opensearchserverless collection data plane plus the emulator's host/endpoint routing table — route the published collectionEndpoint host to an OpenSearch REST handler that authorises SigV4 `aoss` requests against the collection's network and data-access policies, instead of letting the host reach the AWS-JSON `x-amz-target` dispatcher. NOTE: the same wall is already filed under other service tokens (20261029T171944Z-bedrock-agent-the-opensearch-serverless-collection-endpoint-answers.md and 20261029T171944Z-bedrock-agent-lambda-opensearchserverles-....md); these four checks were not marked owned by either and are recorded here so they are re-replayed. If one of those fixes lands first this cause should close with it.
Traces: opensearchserverless.collection-endpoint-requires-aoss-sigv4-service-name, opensearchserverless.data-access-policy-read-permission-without-write-blocks-indexing, opensearchserverless.deleting-network-policy-stops-the-live-collection-endpoint-serving, opensearchserverless.server-side-network-policy-enforces-vpc-only-access.

## Evidence
Cause 1 — expected the mapping after UpdateIndex to be the union {'title': {'type':'text'}, 'views': {'type':'integer'}, 'location': {'type':'geo_point'}, 'tags': {'type':'keyword'}, 'rating': {'type':'float'}}; got only {'rating': {'type': 'float'}} (AssertionError: 'mapping after UpdateIndex mismatch').
Cause 2 — expected vpcEndpointErrorDetails entry errorMessage 'The specified VpcEndpoint is not found.'; got {'id': 'vpce-<id>', 'errorMessage': 'The specified Vpc Endpoint is not found.', 'errorCode': 'NOT_FOUND'} (status 200, id and errorCode correct).
Cause 3 — expected stack status CREATE_COMPLETE and BatchGetCollection reporting the collection ACTIVE; got 'AssertionError: stack ended in DELETE_COMPLETE' (OnFailure='DELETE').
Cause 4 — expected the stack's {'Ref': 'Endpoint'} output to be a 'vpce-...' id accepted by BatchGetVpcEndpoint; got the endpoint name, and BatchGetVpcEndpoint refused it: ValidationException "1 validation error detected: Value 'sim2real-cfnvpce-<suffix>' at 'ids.1.member' failed to satisfy constraint: Member must satisfy regular expression pattern: vpce-[0-9a-z]*" (the name itself is a per-run value; the defect is that a name is returned where an id is required).
Cause 5 — expected: aoss-signed PUT/GET/POST on the collection endpoint accepted 200/201, 403 with error.type 'forbidden' for an 'es'-scoped signature or a withdrawn permission, 401 with no matching network policy. actual: HTTP 400 `{"__type":"SerializationException","message":"missing required header `x-amz-target`"}` for all of them (the same body the sibling bedrock-agent checks surface directly as opensearchpy RequestError(400, ...)), reported by the suite as 'Error: Check timed out after 60.0s' because each check retries while waiting for policy propagation.

## Steps to Reproduce
Cause 1: create_collection + wait ACTIVE, create_index(id=<coll id>, indexName=<i>, indexSchema with properties title/views/location/tags), then update_index(id=<coll id>, indexName=<i>, indexSchema={'mappings': {'properties': {'rating': {'type': 'float'}}}}) and read the index's mapping back.
Cause 2: create_vpc_endpoint, wait ACTIVE, delete_vpc_endpoint, poll batch_get_vpc_endpoint(ids=[<vpce id>]) until vpcEndpointErrorDetails is populated and inspect errorMessage.
Cause 3: cloudformation.create_stack(TemplateBody={'Resources': {'Collection': {'Type': 'AWS::OpenSearchServerless::Collection', 'Properties': {'Name': <n>, 'Type': 'SEARCH', 'EncryptionConfig': {'AWSOwnedKey': True}}}}, 'Outputs': {'CollectionId': {'Value': {'Ref': 'Collection'}}}}, OnFailure='DELETE') and poll describe_stacks -> DELETE_COMPLETE; describe_stack_events shows the failed resource.
Cause 4: ec2.create_vpc + modify_vpc_attribute(EnableDnsSupport, EnableDnsHostnames) + create_subnet, then create_stack with a single AWS::OpenSearchServerless::VpcEndpoint {Name, VpcId, SubnetIds} and Output {'Ref': 'Endpoint'}; the stack reaches CREATE_COMPLETE, then batch_get_vpc_endpoint(ids=[<that output>]) -> ValidationException on the vpce- pattern.
Cause 5: create encryption + network security policies and a data access policy naming an assumed role that also holds the IAM permission aoss:APIAccessAll, create_collection, wait ACTIVE, read collectionEndpoint, then SigV4-sign (service 'aoss', with x-amz-content-sha256 on body-bearing requests) PUT /<index>, PUT /<index>/_doc/<id>, GET /<index>/_doc/<id>, POST /<index>/_search, GET /<index>/_mapping against that endpoint -> 400 SerializationException 'missing required header `x-amz-target`' for every verb, for every principal, with and without a matching network policy.
