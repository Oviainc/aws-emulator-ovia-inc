Issue-Source: trace-sweep-divergence
Issue-Trace: servicediscovery-namespace-operations-not-found
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: servicediscovery-namespace-operations-not-found servicediscovery-operation-polling-and-filtered-listing servicediscovery-dns-namespace-request-validation-errors servicediscovery-http-namespace-service-instance-discovery-lifecycle servicediscovery-operation-tracking-and-filtered-listings servicediscovery-resource-tagging-lifecycle-and-limits servicediscovery-tagging-operations-resource-not-found
Issue-Shape-Id: com.amazonaws.servicediscovery#Route53AutoNaming_v20170314
Issue-Key: servicediscovery|GetNamespace|4xx->4xx|status
Issue-Key: servicediscovery|UpdatePrivateDnsNamespace|4xx->4xx|$.Message
Issue-Key: servicediscovery|UpdatePrivateDnsNamespace|4xx->4xx|status
Issue-Key: servicediscovery|DeleteNamespace|4xx->4xx|$.Message
Issue-Key: servicediscovery|DeleteNamespace|4xx->4xx|status
Issue-Key: servicediscovery|UpdateHttpNamespace|4xx->4xx|$.Message
Issue-Key: servicediscovery|UpdateHttpNamespace|4xx->4xx|status
Issue-Key: servicediscovery|UpdatePublicDnsNamespace|4xx->4xx|$.Message
Issue-Key: servicediscovery|UpdatePublicDnsNamespace|4xx->4xx|status
Issue-Key: servicediscovery|CreateService|4xx->4xx|$.Message
Issue-Key: servicediscovery|CreateService|4xx->4xx|status
Issue-Key: servicediscovery|GetOperation|4xx->4xx|status
Issue-Key: servicediscovery|ListTagsForResource|4xx->4xx|status
Issue-Key: servicediscovery|ListTagsForResource|4xx->4xx|$.Message
Issue-Key: servicediscovery|ListTagsForResource|4xx->4xx|$.__type
Issue-Commit: 355c5d16add239c66c8799ffa26377be788e0f43

# Cloud Map namespace, operation and tagging errors answer HTTP 404/409 instead of the 400 AWS serves

> _Note: this issue was identified against the code base at commit `355c5d16add239c66c8799ffa26377be788e0f43`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetNamespace

## Description
TEN independent, net-new wire divergences in the servicediscovery (Cloud Map) emulator, all reached through namespace-, operation- and tag-scoped calls, and all sharing one fault family: the HTTP status (and in two cases the error identity and message) the emulator serves for Cloud Map's client-error classes. Each of the ten causes below must be closed before this issue is submitted. Model citations use services/service-com-amazonaws-servicediscovery-route53autonaming-v20170314/smithy-model/com-amazonaws-servicediscovery-route53autonaming-v20170314.json.

Shared AWS fact for causes 1-10: the Smithy model carries @httpError 404 on NamespaceNotFound / OperationNotFound / ResourceNotFoundException and 409 on ResourceInUse, but EVERY recorded AWS response in this wire corpus for those classes is HTTP 400 with the error code carried in the awsJson1_1 __type field. The wire recording is the ground truth for what Cloud Map serves: this service answers its client errors with status 400 and distinguishes them only by __type. A status code is a requirement, so serving 404/409 is a defect even when the code and message match.

Cause 1 of 10: GetNamespace for an id that does not exist answers 404 instead of 400.
AWS: Smithy GetNamespace{Id: Arn (required, len 0..255)} -> Namespace; errors InvalidInput (400) and NamespaceNotFound (modelled 404). Recorded: HTTP 400, __type NamespaceNotFound, empty Message.
Emulator: HTTP 404 with __type NamespaceNotFound (code and empty body matched; only the status diverges).
Why: a client that branches on the HTTP status (or an SDK retry/classification layer keyed on 4xx sub-codes) sees a transport-level 404 where AWS sends 400; negative tests asserting the recorded status fail.
Fix: the error-to-status mapping in the service's awsJson1_1 error serializer (the shared not-found error construction), not the handler's logic.
Traces: servicediscovery-namespace-operations-not-found#task#2; also servicediscovery-operation-polling-and-filtered-listing#teardown#11 and #teardown#12.

Cause 2 of 10: UpdatePrivateDnsNamespace on a missing namespace answers 404 and omits the Message AWS fills with the requested namespace id.
AWS: Smithy UpdatePrivateDnsNamespace{Id: Arn (required), UpdaterRequestId (idempotency token), Namespace: PrivateDnsNamespaceChange (required)} -> OperationId; errors DuplicateRequest, InvalidInput, NamespaceNotFound, ResourceInUse. Recorded: HTTP 400, __type NamespaceNotFound, and the Message IS the namespace Id taken from the request - no fixed text at all, the whole body is the request's Id value.
Emulator: HTTP 404 and NO Message member in the body.
Why: a caller that reads Message to learn which namespace was rejected gets nothing, and the status is wrong too.
Fix: the NamespaceNotFound construction in UpdatePrivateDnsNamespace's handler plus the shared status mapping.
Traces: servicediscovery-dns-namespace-request-validation-errors#task#8; also servicediscovery-namespace-operations-not-found#task#5.

Cause 3 of 10: DeleteNamespace on a namespace that still contains services answers 409 with emulator prose instead of 400 with AWS's fixed ResourceInUse text.
AWS: Smithy DeleteNamespace{Id (required)} -> OperationId; errors DuplicateRequest, InvalidInput, NamespaceNotFound, ResourceInUse. Recorded: HTTP 400, __type ResourceInUse, Message exactly 'Namespace has associated services; delete the services before deleting the namespace' - the whole message is FIXED text, with no variable part (no id, no count).
Emulator: HTTP 409 with Message "The namespace <namespaceId> can't be deleted because it still contains <n> service(s)." - emulator-authored prose whose variable parts are the namespace id and the service count.
Why: clients and tests that assert the documented conflict text, or that treat 400 vs 409 differently, both break. The open behavioral issue 20261023T124702Z-servicediscovery-cloud-map-servicediscovery-8-fidelity-divergences-missing-dnsconfig.md names this message under its Cause 8 but records the status as 409-matching; the wire recording shows the status itself is wrong, and this is a separate replay key.
Fix: the ResourceInUse construction in DeleteNamespace's handler and the shared status mapping.
Traces: servicediscovery-http-namespace-service-instance-discovery-lifecycle#task#10.

Cause 4 of 10: UpdateHttpNamespace on a missing namespace answers 404 and omits the Message AWS fills with the requested namespace id.
AWS: Smithy UpdateHttpNamespace{Id (required), UpdaterRequestId, Namespace: HttpNamespaceChange (required)} -> OperationId; errors as for the other namespace updates. Recorded: HTTP 400, __type NamespaceNotFound, Message = the namespace Id from the request (no fixed text).
Emulator: HTTP 404, no Message member.
Why: same as cause 2 - the only place AWS reports the rejected id is empty, and the status diverges.
Fix: UpdateHttpNamespace's NamespaceNotFound construction plus the shared status mapping.
Traces: servicediscovery-namespace-operations-not-found#task#3.

Cause 5 of 10: UpdatePublicDnsNamespace on a missing namespace answers 404 and omits the id Message.
AWS: Smithy UpdatePublicDnsNamespace{Id (required), UpdaterRequestId, Namespace: PublicDnsNamespaceChange (required)} -> OperationId. Recorded: HTTP 400, __type NamespaceNotFound, Message = the namespace Id from the request.
Emulator: HTTP 404, no Message member.
Why: as cause 2/4.
Fix: UpdatePublicDnsNamespace's NamespaceNotFound construction plus the shared status mapping.
Traces: servicediscovery-namespace-operations-not-found#task#4.

Cause 6 of 10: CreateService against a NamespaceId that does not exist answers 404 and omits the id Message.
AWS: Smithy CreateService{Name (required, ServiceName pattern), NamespaceId: Arn, CreatorRequestId, Description, DnsConfig, HealthCheckConfig, HealthCheckCustomConfig, Tags, Type} -> Service; errors InvalidInput 400 / NamespaceNotFound / ResourceLimitExceeded 400 / ServiceAlreadyExists 400 / TooManyTagsException 400. Recorded: HTTP 400, __type NamespaceNotFound, Message = the NamespaceId value from the request.
Emulator: HTTP 404, no Message member.
Why: a create that fails because the namespace is gone reports neither the AWS status nor the namespace it could not find.
Fix: CreateService's namespace-resolution failure path plus the shared status mapping.
Traces: servicediscovery-namespace-operations-not-found#task#6.

Cause 7 of 10: DeleteNamespace on a missing namespace answers 404 and omits the id Message.
AWS: as cause 3. Recorded for the not-found case: HTTP 400, __type NamespaceNotFound, Message = the namespace Id from the request.
Emulator: HTTP 404, no Message member.
Why: as cause 2.
Fix: DeleteNamespace's NamespaceNotFound construction plus the shared status mapping.
Traces: servicediscovery-namespace-operations-not-found#task#7.

Cause 8 of 10: GetOperation for an unknown OperationId answers 404 instead of 400.
AWS: Smithy GetOperation{OperationId: OperationId (required, len 0..255), OwnerAccount: AWSAccountId} -> Operation; errors InvalidInput (400) and OperationNotFound (modelled 404). Recorded: HTTP 400 with __type OperationNotFound.
Emulator: HTTP 404 (code matched).
Why: pollers that classify a 404 as 'operation vanished' versus a 400 client error take the wrong branch mid-poll.
Fix: the shared error-status mapping for OperationNotFound.
Traces: servicediscovery-operation-polling-and-filtered-listing#task#7; also servicediscovery-operation-tracking-and-filtered-listings#task#5.

Cause 9 of 10: ListTagsForResource for a resource that no longer exists answers 404 instead of 400.
AWS: Smithy ListTagsForResource{ResourceARN: AmazonResourceName (required, len 1..1011)} -> Tags; errors InvalidInput (400) and ResourceNotFoundException (modelled 404). Recorded: HTTP 400 with __type ResourceNotFoundException.
Emulator: HTTP 404 (code and message matched).
Why: tag-reading code that distinguishes 400 from 404 diverges.
Fix: the shared error-status mapping for ResourceNotFoundException.
Traces: servicediscovery-resource-tagging-lifecycle-and-limits#task#16.

Cause 10 of 10: the tagging operations skip AWS's ARN resource-format validation, so a malformed resource identifier is reported as ResourceNotFoundException (404) instead of InvalidInput (400).
AWS: ListTagsForResource / TagResource / UntagResource take ResourceARN and validate the ARN's resource segment BEFORE any lookup. Recorded: for an ARN whose resource part is 'namespace/ns-doesnotexist000' (a well-formed prefix with an identifier that is not a valid Cloud Map id), AWS answers HTTP 400, __type InvalidInput, Message "Invalid resource format in ARN. Resource must start with 'service/' or 'namespace/' followed by a valid identifier. Example: 'namespace/ns-a2b3c4d5e6f7g2h3' or 'service/srv-b2c3d4e5f6g7h2i3'" - the entire message is fixed text (the two example ids are part of the literal template, not values from the request).
Emulator: HTTP 404, __type ResourceNotFoundException, Message "The operation can't be completed because the resource <arn> was not found." - it goes straight to the store lookup and reports a miss.
Why: a client that passes a syntactically invalid ARN is told the resource is absent rather than that its input is malformed, so the real bug (bad ARN construction) is masked; the status, the error code and the message all diverge, which means error-handling code keyed on InvalidInput never fires.
Fix: shared ARN parsing/validation in the tagging handlers (the resource-ARN parser used by ListTagsForResource/TagResource/UntagResource) must reject a resource segment that is not 'service/<valid srv id>' or 'namespace/<valid ns id>' with InvalidInput before lookup.
Traces: servicediscovery-tagging-operations-resource-not-found#task#2; also servicediscovery-tagging-operations-resource-not-found#task#3 (TagResource through the same parser).

## Evidence
Replay of trace set simparity-wire-aws-v1 against this checkout: 16 fixtures replayed, 168 steps, 89 passed, 79 mismatched, score 53.0%, emulator_up=true (/__admin/health 200). Every cause below is a reported mismatch cluster; expected values are the recorded AWS responses.

Cause 1 - servicediscovery-namespace-operations-not-found#task#2, GetNamespace: 'status: expected 400, got 404' (cluster also covers servicediscovery-operation-polling-and-filtered-listing#teardown#11, #teardown#12).
Cause 2 - servicediscovery-dns-namespace-request-validation-errors#task#8, UpdatePrivateDnsNamespace: 'status: expected 400, got 404' and '$.Message: missing (expected the namespace id, got None)' (recorded id value in the fixture: ns-aaaaaaaaaaaaaaaa).
Cause 3 - servicediscovery-http-namespace-service-instance-discovery-lifecycle#task#10, DeleteNamespace: 'status: expected 400, got 409'; '$.Message: value_mismatch (expected \'Namespace has associated services; delete the services before deleting the namespace\', got "The namespace ns-pa8aaut13u20kze9 can't be deleted because it still contains 1 service(s).")'.
Cause 4 - servicediscovery-namespace-operations-not-found#task#3, UpdateHttpNamespace: 'status: expected 400, got 404'; '$.Message: missing (expected the requested namespace id, got None)'.
Cause 5 - servicediscovery-namespace-operations-not-found#task#4, UpdatePublicDnsNamespace: same two deltas.
Cause 6 - servicediscovery-namespace-operations-not-found#task#6, CreateService: 'status: expected 400, got 404'; '$.Message: missing (expected the requested namespace id, got None)'.
Cause 7 - servicediscovery-namespace-operations-not-found#task#7, DeleteNamespace: same two deltas.
Cause 8 - servicediscovery-operation-polling-and-filtered-listing#task#7, GetOperation: 'status: expected 400, got 404'.
Cause 9 - servicediscovery-resource-tagging-lifecycle-and-limits#task#16, ListTagsForResource: 'status: expected 400, got 404'.
Cause 10 - servicediscovery-tagging-operations-resource-not-found#task#2, ListTagsForResource: 'status: expected 400, got 404'; '$.__type: value_mismatch (expected \'InvalidInput\', got \'ResourceNotFoundException\')'; '$.Message: value_mismatch (expected "Invalid resource format in ARN. Resource must start with \'service/\' or \'namespace/\' followed by a valid identifier. Example: \'namespace/ns-a2b3c4d5e6f7g2h3\' or \'service/srv-b2c3d4e5f6g7h2i3\'", got "The operation can't be completed because the resource arn:aws:servicediscovery:<region>:<account>:namespace/ns-doesnotexist000 was not found.")'.

## Steps to Reproduce
Boot the emulator from this checkout and replay the fixtures/steps named in Issue-Trace/Issue-Step and Issue-Trace-Ids, or drive the endpoint with botocore (sd = boto3.client('servicediscovery', endpoint_url=<emulator>)) and inspect e.response['ResponseMetadata']['HTTPStatusCode'] plus e.response['Error']:
1. Cause 1: sd.get_namespace(Id='ns-doesnotexist000') -> emulator 404 NamespaceNotFound; AWS 400 NamespaceNotFound with an empty message.
2. Causes 2/4/5: sd.update_private_dns_namespace(Id='ns-doesnotexist000', Namespace={'Description':'x'}), sd.update_http_namespace(...), sd.update_public_dns_namespace(...) -> emulator 404 with no Message; AWS 400 whose Message is the requested id.
3. Cause 3: create an HTTP namespace, poll its operation to SUCCESS, create one service in it, then sd.delete_namespace(Id=ns) -> emulator 409 with its own prose; AWS 400 ResourceInUse 'Namespace has associated services; delete the services before deleting the namespace'.
4. Causes 6/7: sd.create_service(Name='<valid>', NamespaceId='ns-doesnotexist000') and sd.delete_namespace(Id='ns-doesnotexist000') -> emulator 404 without Message; AWS 400 with the namespace id as the Message.
5. Cause 8: sd.get_operation(OperationId='<unknown>') -> emulator 404; AWS 400 OperationNotFound.
6. Cause 9: tag a namespace, delete it, then sd.list_tags_for_resource(ResourceARN=<its arn>) -> emulator 404; AWS 400.
7. Cause 10: sd.list_tags_for_resource(ResourceARN='arn:aws:servicediscovery:<region>:<account>:namespace/ns-doesnotexist000') -> emulator 404 ResourceNotFoundException; AWS 400 InvalidInput with the fixed 'Invalid resource format in ARN...' text. Repeat with sd.tag_resource(ResourceARN=<same>, Tags=[{'Key':'k','Value':'v'}]).
