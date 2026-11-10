Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2.delete-portal-product-access-denied-without-permission
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigatewayv2.delete-portal-product-access-denied-without-permission|_error_code#7|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Commit: 64b86d5e14a7a25edd6b553817d01f64764c1ea9

# apigatewayv2: DeletePortalProduct performs no authorization, so a principal carrying an explicit Deny gets 204 and the product is actually deleted

> _Note: this issue was identified against the code base at commit `64b86d5e14a7a25edd6b553817d01f64764c1ea9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeletePortalProduct

## Description
One net-new root cause in the apigatewayv2 control plane, grounded in a failing behavioral check of suite simparity-behavioral-aws-v1 that the replay report marked neither ALREADY FILED nor OWNED by an open issue.

Related open issues, for context, neither of which covers this cause nor carries this check's replay key: 20261025T030504Z-apigatewayv2-createportal-performs-no-authorization-and-createintegrationresponse-performs.md (the same missing-authorization defect, but for CreatePortal) and 20261025T163331Z-apigatewayv2-putportalproductsharingpolicy-performs-no-authorization-and-createroute-s.md (the same defect for PutPortalProductSharingPolicy). Neither names DeletePortalProduct, and this cause is a DESTRUCTIVE write on a different operation and a different handler file, so it is filed separately here. This is not a refusal regression: the call is not refused, it succeeds where AWS denies it, so no fixed issue is reopened.

=== Cause 1 of 1: DeletePortalProduct does not evaluate the caller's identity-based policy, so an explicitly denied principal is served 204 No Content and the portal product is destroyed ===
AWS: Smithy (services/service-com-amazonaws-apigatewayv2-apigatewayv2/smithy-model/com-amazonaws-apigatewayv2-apigatewayv2.json): DeletePortalProduct -> DELETE /v2/portalproducts/{PortalProductId}, success code 204 with an empty output (Unit); input member PortalProductId (httpLabel, required); modeled errors AccessDeniedException (httpError 403), BadRequestException (400), NotFoundException (404), TooManyRequestsException (429). A caller whose identity-based policy carries an explicit Deny on apigateway:DeletePortalProduct must be refused with error code AccessDeniedException / HTTP 403, and the product must remain readable afterwards through GetPortalProduct (GET /v2/portalproducts/{PortalProductId}, 200) with the same PortalProductId; an unrestricted principal must still succeed with 204, after which GetPortalProduct must answer NotFoundException / 404. The refusal message is AWS's documented authorization form, whose FIXED text is 'User: ' + ' is not authorized to perform: ' + ' on resource: ' + ' with an explicit deny in an identity-based policy'; its VARIABLE parts are taken from the request and from state - the caller's STS assumed-role ARN (arn:aws:sts::<account>:assumed-role/<role>/<session>), the action apigateway:DeletePortalProduct, and the addressed product resource (arn:aws:apigateway:<region>:<account>:/portalproducts/<PortalProductId>). That rule is the requirement; no single run's account id, role name, region or product id is part of it. The subsequent NotFoundException text for a deleted product is likewise a template - fixed 'Product not found in account ' + ' with id ' with the account and the requested PortalProductId filled in - but that leg was never reached in this replay, so it is recorded here only as the expected follow-on, not as an observed divergence.
Emulator: the restricted principal's DeletePortalProduct SUCCEEDS with HTTP 204 and no body (observed ResponseMetadata HTTPStatusCode 204, content-type application/json, no error), identical to the unrestricted path, and the product is actually removed. No IAM evaluation happens for this operation at all, so the AccessDeniedException branch the model declares is unreachable, and the check never got as far as asserting that the product is still gettable.
Why: a client cannot rely on the emulator to model least privilege for portal products, and here the over-permissive path is destructive. Least-privilege regression tests that assert a denied principal is refused pass against the emulator and fail on AWS; an agent or harness that probes its permissions by attempting the call concludes it is authorized when AWS would deny it; and because the delete is actually applied, state that AWS would have preserved is gone, so later GetPortalProduct/GetPortalProducts reads diverge for the rest of the session.
Fix: the authorization path of the DeletePortalProduct handler (services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/ops/delete_portal_product.rs) must evaluate the caller's identity-based policy for apigateway:DeletePortalProduct against the product's resource ARN and return AccessDeniedException / 403 with AWS's explicit-deny message WITHOUT mutating product state. The same gate belongs on the sibling product operations (CreatePortalProduct, GetPortalProduct, UpdatePortalProduct, GetPortalProducts), which share this missing-authorization shape.
Traces: apigatewayv2.delete-portal-product-access-denied-without-permission.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against this checkout: 90/218 passed. The check below carried NO ALREADY FILED and NO OWNED-by marker in the replay report.

Cause 1 - apigatewayv2.delete-portal-product-access-denied-without-permission [mismatch]:
  File "check_apigatewayv2_delete_portal_product_access_denied_without_permission.py", line 34, in run
    code, message = _error_code(lambda: restricted_apigwv2.delete_portal_product(PortalProductId=product_id))
  File "...", line 88, in _error_code
    raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
  AssertionError: expected a ClientError, but the call SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 204, 'HTTPHeaders': {'content-type': 'application/json', 'date': '...'}, 'RetryAttempts': 0}}

Expected: error code 'AccessDeniedException' with HTTP 403 and the message template 'User: arn:aws:sts::<account>:assumed-role/<role>/<session> is not authorized to perform: apigateway:DeletePortalProduct on resource: arn:aws:apigateway:<region>:<account>:/portalproducts/<PortalProductId> with an explicit deny in an identity-based policy' (fixed text as quoted; ARN, action resource and product id filled from the request/state), and the product still readable via GetPortalProduct with the same PortalProductId.
Actual: HTTP 204 No Content, no error, product deleted, for a session assuming a role whose inline policy carries {Effect: Deny, Action: ['apigateway:DeletePortalProduct'], Resource: '*'} on top of AdministratorAccess.
Not reached, hence not claimed as observed: the positive control (unrestricted DeletePortalProduct -> 204) and the follow-on GetPortalProduct -> NotFoundException 'Product not found in account <account> with id <PortalProductId>'.

## Steps to Reproduce
1. Boot the emulator from this checkout and point a botocore apigatewayv2 client at it.
2. CreatePortalProduct(DisplayName='<unique name>', Tags={...}) and keep the returned PortalProductId.
3. With IAM: CreateRole(<unique role>) trusting the account root, AttachRolePolicy(arn:aws:iam::aws:policy/AdministratorAccess), PutRolePolicy with an inline document {Version: '2012-10-17', Statement: [{Effect: 'Deny', Action: ['apigateway:DeletePortalProduct'], Resource: '*'}]}.
4. sts.AssumeRole(RoleArn=<that role>, RoleSessionName='sim2real-behav') and build a second apigatewayv2 client from the returned temporary credentials.
5. Call DeletePortalProduct(PortalProductId=<id>) with the RESTRICTED client. Expect a ClientError with Code 'AccessDeniedException', HTTP 403 and the explicit-deny message above; observe HTTP 204 with no error.
6. Call GetPortalProduct(PortalProductId=<id>) with the unrestricted client: on AWS it still returns the product (200); against the emulator the product has been deleted by the denied call.
7. Re-replay the check id in trace_ids to confirm the fix.
