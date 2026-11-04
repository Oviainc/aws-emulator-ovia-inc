Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway.apigw-iam-auth-unknown-access-key-rejected-at-stage apigateway.apigw-iam-auth-wrong-secret-signature-rejected-at-stage apigateway.create-domain-name-access-association-invalid-source-rejected apigateway.delete-vpc-link-in-use-fails apigateway.apigw-apikeyrequired-mismatched-key-rejected-before-proxy-integration apigateway.explicit-verb-method-takes-precedence-over-any-and-get-method-post-stays-not-found
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway.apigw-iam-auth-unknown-access-key-rejected-at-stage|run#13|AssertionError|assert status == 403, (
Issue-Key: apigateway.apigw-iam-auth-wrong-secret-signature-rejected-at-stage|run#13|AssertionError|assert status == 403, (
Issue-Key: apigateway.create-domain-name-access-association-invalid-source-rejected|_error#13|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: apigateway.delete-vpc-link-in-use-fails|run#23|AssertionError|assert message == (
Issue-Key: apigateway.apigw-apikeyrequired-mismatched-key-rejected-before-proxy-integration|_poll_get#44|Timeout|time.sleep(3.0)
Issue-Key: apigateway.explicit-verb-method-takes-precedence-over-any-and-get-method-post-stays-not-found|_served#19|Timeout|time.sleep(3)
Issue-Commit: 024100b485faef6e8a7d69224b42c4d112173d44

# apigateway: 4 net-new root causes — execute-api never verifies the SigV4 credential or signature, CreateDomainNameAccessAssociation accepts a malformed VPC endpoint id, DeleteVpcLink's in-use refusal is reworded, and two stage data-plane checks never answer

> _Note: this issue was identified against the code base at commit `024100b485faef6e8a7d69224b42c4d112173d44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Four net-new root causes among the failing checks under the `### apigateway` heading that are neither ALREADY FILED nor OWNED by an open issue. Open issues that look adjacent but do not carry these replay keys: 20261029T193654Z-apigateway-api-gateway-execute-api-data-plane-10-root.md, 20261029T193654Z-apigateway-api-gateway-refusal-contracts-part-1-10-root.md, 20261103T035351Z-apigateway-api-gateway-control-plane-8-net-new-refusal.md, 20261028T131753Z-apigateway-3-net-new-root-causes-a-vpclink.md.

=== Cause 1 of 4: the execute-api stage accepts an unknown access key and a forged signature ===
AWS: a deployed stage whose method is `authorizationType=AWS_IAM` resolves the access key id in the SigV4 `Authorization` header and verifies the signature against that key's secret. An access key id that names no credential, and a correct access key id signed with the wrong secret, are both refused HTTP 403 before any integration runs; the integration response is never served. recorded behavior only — the on-disk Smithy model services/service-com-amazonaws-apigateway-backplanecontrolservice/smithy-model/com-amazonaws-apigateway-backplanecontrolservice.json models only the control plane and does not describe the execute-api data plane.
Emulator: both requests are served HTTP 200 with the MOCK integration body `{"served":"iam-protected"}`. The signature is parsed for identity and then neither the key nor the signature is checked.
Why: any caller who can reach the endpoint, with a made-up access key id or with a leaked key id and no secret, invokes every AWS_IAM-protected method. SigV4 at the edge stops being an authentication boundary.
Fix: the execute-api data-plane SigV4 verifier in the apigateway service crate — resolve the credential named by the access key id, refuse an unknown one, recompute the signature over the canonical request with that credential's secret and refuse a mismatch, all before route dispatch.
Traces: apigateway.apigw-iam-auth-unknown-access-key-rejected-at-stage, apigateway.apigw-iam-auth-wrong-secret-signature-rejected-at-stage.

=== Cause 2 of 4: CreateDomainNameAccessAssociation accepts a malformed accessAssociationSource ===
AWS: with `accessAssociationSourceType=VPCE`, `accessAssociationSource` must be a VPC endpoint id. A value that is not one is refused with `BadRequestException` (Smithy: CreateDomainNameAccessAssociation declares BadRequestException httpError 400 alongside ConflictException/LimitExceededException/TooManyRequestsException/UnauthorizedException; `accessAssociationSource` and `accessAssociationSourceType` are both required) and nothing is created.
Emulator: the call succeeds with HTTP 201 and mints an association, echoing `accessAssociationSource: "not-a-valid-vpce-id"` and a `domainNameAccessAssociationArn`.
Why: a client that relies on the create call to reject a malformed endpoint id persists a dangling association that can never be resolved, and its negative-path error handling for BadRequestException is never exercised.
Fix: input validation in the CreateDomainNameAccessAssociation handler — validate the VPCE source shape (and that the endpoint exists) and refuse with BadRequestException 400 before any state is written.
Traces: apigateway.create-domain-name-access-association-invalid-source-rejected.

=== Cause 3 of 4: DeleteVpcLink's in-use refusal carries an emulator-authored sentence ===
AWS: DeleteVpcLink on an AVAILABLE VPC link that an integration references is refused `BadRequestException` (Smithy: DeleteVpcLink declares BadRequestException httpError 400) with the message `Cannot delete vpc link. Vpc link '<vpcLinkId>', is referenced in [<restApiId>:<httpMethod>:<resourceId>] in format of [RESTApiId:Method:Resource].` The fixed text is `Cannot delete vpc link. Vpc link '`, `', is referenced in [` and `] in format of [RESTApiId:Method:Resource].`; the variable parts are the vpcLinkId from the request and the referencing integration's restApiId, HTTP method and resourceId from the emulator's state.
Emulator: the message is `Cannot delete VPC link '<vpcLinkId>', is referenced in integration: GET//` — a different sentence, differently capitalised, and the reference is reported as `<METHOD>//` rather than as the `[RESTApiId:Method:Resource]` triple, so the referencing resource id and api id are not reported at all.
Why: the message is the only place AWS tells the caller WHICH integration is holding the link, so an operator (or automation parsing the reference triple) cannot locate the blocker and the rewritten text breaks any message-matching assertion.
Fix: the DeleteVpcLink handler's in-use refusal message construction — emit AWS's sentence and fill the reference with the triple `[restApiId:httpMethod:resourceId]` taken from the integration that references the link.
Traces: apigateway.delete-vpc-link-in-use-fails.

=== Cause 4 of 4: two execute-api stage scenarios never produce a response ===
AWS: both scenarios complete in seconds against AWS. (a) A method with `apiKeyRequired=true` behind a usage plan refuses a request carrying a key that is not associated with that plan with HTTP 403 `{"message":"Forbidden"}` BEFORE the AWS_PROXY integration is invoked, so the backend never sees the request. (b) On a resource that defines both GET and ANY, the stage serves GET from the GET method and POST from ANY, while GetMethod for POST is NotFoundException; after GET is deleted and the API redeployed, ANY serves GET.
Emulator: neither check reaches any verdict — both end `Check timed out after 60.0s`, i.e. the deployed stage never produces an HTTP response for these request shapes, so no status, body or integration-invocation evidence exists at all.
Why: a client gets a hang instead of a response. There is no status code to branch on and no timeout error to retry; the API-key pre-integration gate and the explicit-verb-over-ANY routing precedence are both entirely unobservable.
Fix: the execute-api data-plane request pipeline in the apigateway service crate — the stage invoke path must always produce a response (api-key/usage-plan gate before integration dispatch; explicit-verb-vs-ANY route selection) rather than blocking.
Traces: apigateway.apigw-apikeyrequired-mismatched-key-rejected-before-proxy-integration, apigateway.explicit-verb-method-takes-precedence-over-any-and-get-method-post-stays-not-found.

## Evidence
Cause 1 — apigateway.apigw-iam-auth-unknown-access-key-rejected-at-stage: `AssertionError: a request signed by an unknown key must be 403; got (200, '{"served":"iam-protected"}')`. apigateway.apigw-iam-auth-wrong-secret-signature-rejected-at-stage: `AssertionError: a request signed with the wrong secret must be 403; got (200, '{"served":"iam-protected"}')`.

Cause 2 — apigateway.create-domain-name-access-association-invalid-source-rejected: `AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 201, ...}, 'domainNameAccessAssociationArn': 'arn:aws:apigateway:us-west-2::/domainnameaccessassociations/<id>', 'domainNameArn': 'arn:aws:apigateway:us-west-2::/domainnames/<domain>+<id>', 'accessAssociationSourceType': 'VPCE', 'accessAssociationSource': 'not-a-valid-vpce-id', 'tags': {}}`. Expected: BadRequestException (400) and no association created.

Cause 3 — apigateway.delete-vpc-link-in-use-fails, at `assert message == (f"Cannot delete vpc link. Vpc link '{vpc_link_id}', is referenced in [{api_id}:GET:{root_id}] in format of [RESTApiId:Method:Resource].")`; the message the emulator returned was `Cannot delete VPC link '<vpcLinkId>', is referenced in integration: GET//`.

Cause 4 — apigateway.apigw-apikeyrequired-mismatched-key-rejected-before-proxy-integration: `Error: Check timed out after 60.0s`. apigateway.explicit-verb-method-takes-precedence-over-any-and-get-method-post-stays-not-found: `Error: Check timed out after 60.0s`.

## Steps to Reproduce
1. Build and boot the emulator from the checkout and replay the behavioral suite.
2. Re-replay the six check ids in trace_ids (all under the `### apigateway` heading).
3. Cause 1 manual repro: CreateRestApi, PutMethod GET authorizationType=AWS_IAM on the root resource with a MOCK integration returning {"served":"iam-protected"}, CreateDeployment stageName=<stage>; sign a GET to the stage URL with (a) a randomly generated access key id that was never issued and (b) a valid access key id plus a wrong secret. Observed 200 in both cases; expected 403.
4. Cause 2 manual repro: CreateDomainName for a PRIVATE domain, then CreateDomainNameAccessAssociation with domainNameArn=<that domain's arn>, accessAssociationSourceType=VPCE, accessAssociationSource="not-a-valid-vpce-id". Observed HTTP 201; expected 400 BadRequestException with nothing created.
5. Cause 3 manual repro: CreateVpcLink over an NLB and wait for AVAILABLE; CreateRestApi and PutIntegration type=HTTP connectionType=VPC_LINK connectionId=<vpcLinkId> on the root GET; DeleteVpcLink. Compare the BadRequestException message against AWS's `Cannot delete vpc link. Vpc link '<id>', is referenced in [<restApiId>:GET:<resourceId>] in format of [RESTApiId:Method:Resource].`
6. Cause 4 manual repro: deploy a stage with an apiKeyRequired AWS_PROXY method behind a usage plan and invoke it with a key not in that plan; and deploy a resource carrying both GET and ANY and invoke GET and POST. Both invocations do not return within 60 s.
