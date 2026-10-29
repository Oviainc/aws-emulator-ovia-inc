Issue-Source: trace-sweep-divergence
Issue-Trace: apigateway-documentation-version-invalid-restapi
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigateway-documentation-version-invalid-restapi apigateway-error-paths-invalid-references apigateway-operations-on-deleted-rest-api apigateway-integration-and-responses-workflow apigateway-tag-vpc-link-opportunistic apigateway-vpc-link-error-paths apigateway-vpc-link-negative-paths
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway|GetDocumentationVersion|4xx->4xx|$.message
Issue-Key: apigateway|DeleteDocumentationVersion|4xx->4xx|$.message
Issue-Key: apigateway|GetExport|4xx->4xx|$.message
Issue-Key: apigateway|PutMethod|4xx->4xx|$.message
Issue-Key: apigateway|PutIntegration|4xx->4xx|$.message
Issue-Key: apigateway|GetTags|4xx->4xx|$.message
Issue-Key: apigateway|UntagResource|4xx->4xx|$.message
Issue-Key: apigateway|GetVpcLink|4xx->4xx|$.message
Issue-Key: apigateway|UpdateVpcLink|4xx->4xx|$.message
Issue-Related: 20261029T200808Z-apigateway-api-gateway-patch-path-refusals-omit-the-op.md
Issue-Commit: e22fc32f4dd53ee37fe3b4cc7fa000d8c822ec8a

# API Gateway lookup refusals name the wrong resource (10 causes)

> _Note: this issue was identified against the code base at commit `e22fc32f4dd53ee37fe3b4cc7fa000d8c822ec8a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDocumentationVersion

## Description
When a request names a parent resource that does not exist, API Gateway reports the refusal in terms of the LEAF resource the operation addresses, and it never echoes the caller's account id. The emulator reports the parent it failed to resolve (prefixed with the account id) or mis-capitalises the resource name. Status and error code match in all ten clusters; only the message diverges - and the message is a requirement.

Cause 1 of 10 - GetDocumentationVersion with an unknown restApiId. AWS: 404 NotFoundException `Invalid Documentation version specified` (fixed text, no interpolation). Emulator: `Invalid API identifier specified 000000000000:nonexistent1` - it names the parent and leaks `<accountId>:<apiId>`. Why: a client that branches on the refusal text cannot tell which resource was missing, and the account id has no counterpart in AWS's message. Fix: the documentation-version handler's not-found path. Trace: apigateway-documentation-version-invalid-restapi#task#2.

Cause 2 of 10 - UpdateDocumentationVersion, same. Trace: apigateway-documentation-version-invalid-restapi#task#4.

Cause 3 of 10 - DeleteDocumentationVersion, same. Trace: apigateway-documentation-version-invalid-restapi#task#5.

Cause 4 of 10 - GetExport with an unknown restApiId. AWS: 404 NotFoundException `Invalid stage identifier specified` (the export is addressed by stage). Emulator: `Invalid API identifier specified <acct>:<apiId>`. Fix: GetExport's lookup order/message. Trace: apigateway-error-paths-invalid-references#task#2 (2 steps).

Cause 5 of 10 - PutMethod with an unknown restApiId. AWS: 404 NotFoundException `Invalid Resource identifier specified`. Emulator: the API-identifier message. Fix: PutMethod's lookup order/message. Trace: apigateway-error-paths-invalid-references#task#3.

Cause 6 of 10 - PutIntegration of type HTTP with no integrationHttpMethod. AWS: 400 BadRequestException `Enumeration value for HttpMethod must be non-empty` (fixed text). Emulator: `integrationHttpMethod is required for an integration of type HTTP`. Fix: PutIntegration's validation message. Trace: apigateway-integration-and-responses-workflow#task#17.

Causes 7-10 share one fault: the VPC-link not-found message is capitalised `Invalid Vpc Link identifier specified` by AWS (fixed text, exactly that casing and spacing), while the emulator writes `Invalid VPC link identifier specified`. Fix: the single vpc-link not-found message used by these handlers.
  Cause 7 of 10 - GetTags for an unknown VPC-link ARN. Trace: apigateway-tag-vpc-link-opportunistic#task#1.
  Cause 8 of 10 - UntagResource for an unknown VPC-link ARN. Trace: apigateway-tag-vpc-link-opportunistic#task#3.
  Cause 9 of 10 - GetVpcLink. Trace: apigateway-vpc-link-error-paths#task#2 (2 steps).
  Cause 10 of 10 - UpdateVpcLink. Trace: apigateway-vpc-link-error-paths#task#3 (2 steps).

The recorded account id and identifiers are that run's values, not part of any template. No open issue owns these clusters; 'API Gateway refusal contracts, part 1/2/3' cover other operations.

## Evidence
Causes 1-3: 404 both; $.message expected 'Invalid Documentation version specified', got 'Invalid API identifier specified 000000000000:nonexistent1'.
Cause 4: expected 'Invalid stage identifier specified', got 'Invalid API identifier specified 000000000000:nonexist01'.
Cause 5: expected 'Invalid Resource identifier specified', got the same API-identifier message.
Cause 6: 400 both; expected 'Enumeration value for HttpMethod must be non-empty', got 'integrationHttpMethod is required for an integration of type HTTP'.
Causes 7-10: 404 both; expected 'Invalid Vpc Link identifier specified', got 'Invalid VPC link identifier specified'.

## Steps to Reproduce
1. Replay the corpus (replay_wire_corpus).
2. Causes 1-5: call GetDocumentationVersion / UpdateDocumentationVersion / DeleteDocumentationVersion / GetExport / PutMethod with a restApiId that does not exist and compare $.message.
3. Cause 6: PUT an integration with type=HTTP and no integrationHttpMethod.
4. Causes 7-10: call GetTags/UntagResource/GetVpcLink/UpdateVpcLink against an unknown VPC link id.
