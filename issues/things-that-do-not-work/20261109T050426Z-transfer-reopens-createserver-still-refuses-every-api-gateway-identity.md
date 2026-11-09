Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: transfer.test-identity-provider-invalid-request-error transfer.test-identity-provider-invokes-configured-endpoint transfer.test-identity-provider-service-managed-rejected
Issue-Shape-Id: com.amazonaws.transfer#TransferService
Issue-Key: transfer|CreateServer|InvalidRequestException
Issue-Commit: 21f99eeabe4f0ab4efdf351ee564b8815702ec39

# REOPENS: CreateServer still refuses every API_GATEWAY identity provider — the execute-api resource it evaluates omits the method/path segments, so an invocation role scoped the way AWS documents never verifies

> _Note: this issue was identified against the code base at commit `21f99eeabe4f0ab4efdf351ee564b8815702ec39`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateServer

## Description
REOPENS 20261024T042010Z-transfer-fidelity-gaps-api-gateway-createserver-always-refused.md: its fix left the call refused.

ONE root cause, grounded in 3 failing behavioral checks of this suite.

Cause 1 of 1: CreateServer(IdentityProviderType='API_GATEWAY') is refused with InvalidRequestException because the execute-api request resource the invocation-role verification evaluates is built with only two segments after the API id.

AWS: Smithy (com.amazonaws.transfer#TransferService) CreateServer.IdentityProviderType -> IdentityProviderType enum, which includes API_GATEWAY; IdentityProviderDetails carries Url and InvocationRole; output ServerId (@required, len 19, ^s-([0-9a-f]{17})$); modeled errors AccessDeniedException 403, InternalServiceError 503, InvalidRequestException 400, ResourceExistsException 409, ResourceNotFoundException 404, ServiceUnavailableException 503, ThrottlingException 429. Recorded behavior: CreateServer with IdentityProviderType='API_GATEWAY' and IdentityProviderDetails={'Url': <a deployed REST API stage invoke URL>, 'InvocationRole': <a role trusted by transfer.amazonaws.com whose inline policy Allows execute-api:Invoke on that API>} succeeds with HTTP 200 and a ServerId; DescribeServer then reports IdentityProviderType='API_GATEWAY' and TestIdentityProvider against it answers 200. AWS's execute-api resource ARN — the resource an Invoke is authorized against — has the documented form arn:aws:execute-api:<region>:<account>:<api-id>/<stage>/<HTTP-METHOD>/<resource-path>, so the canonical whole-API grant a caller writes is arn:aws:execute-api:<region>:<account>:<api-id>/*/*/* (stage, method and path all wildcarded). A role carrying that grant MUST verify.

Emulator: every such CreateServer is refused HTTP 400 InvalidRequestException with the message 'Unable to verify access to API <url>' (fixed text 'Unable to verify access to API ' followed by the Url member taken verbatim from the request; the host, port, api id and stage in it are per-run values and are not part of the requirement). The previous fix taught services/service-com-amazonaws-transfer-transferservice/src/state/authorization.rs::execute_api_arn to recognise the path addressing this emulator publishes (<endpoint>/_aws/execute-api/<api-id>/<stage>), but the ARN it then builds, api_arn, is arn:aws:execute-api:<region>:<account>:<api-id>/<stage>/* — it carries a literal '*' in place of BOTH the method and the resource-path segments, i.e. only two segments after the api id instead of AWS's three. invocation_role_can_invoke passes that string to iam_policy::identity_decision as the REQUEST resource, where the role's documented grant pattern <api-id>/*/*/* needs three segments to match; it cannot, so the decision is NotStated, no AWS-managed policy is attached to make it undecidable, the gate answers false and src/ops/create_server.rs raises the refusal. Hypothesis: the same shortfall makes any stage/method-scoped grant unmatchable too, since a request resource may not itself be a pattern.

Why: no API_GATEWAY-backed (custom identity provider) server can be created against this emulator at all, so every custom-IdP flow fails at its very first call with a 400 that names the caller's own valid API URL as unverifiable — and the emulator refuses a URL its own API Gateway emulation minted and serves. All three TestIdentityProvider checks below die in their setup CreateServer, so the operation they exist to exercise (including the SERVICE_MANAGED InvalidRequestException '<server-id> not configured for external auth' vs ResourceNotFoundException 'Unknown server' distinction, and the API_GATEWAY positive control that must return StatusCode 200 with the endpoint's response body) is never reached and remains unverified.

Fix: the API-Gateway-URL -> execute-api ARN derivation in the transfer crate's authorization gate (services/service-com-amazonaws-transfer-transferservice/src/state/authorization.rs — execute_api_arn/api_arn, consumed by invocation_role_can_invoke and src/ops/create_server.rs) must produce a resource with AWS's full segment structure (api-id/stage/method/resource-path) so that a role allowed execute-api:Invoke on <api-id>/*/*/*, on <api-id>/<stage>/*/* or on the whole API verifies, and CreateServer then returns 200 with a ServerId.

Traces: transfer.test-identity-provider-invalid-request-error, transfer.test-identity-provider-invokes-configured-endpoint, transfer.test-identity-provider-service-managed-rejected (all three fail on the setup CreateServer, not on TestIdentityProvider).

## Evidence
Expected: transfer.create_server(Protocols=['SFTP'], IdentityProviderType='API_GATEWAY', Domain='S3', EndpointType='PUBLIC', IdentityProviderDetails={'Url': <stage invoke URL this emulator published>, 'InvocationRole': <role with inline Allow execute-api:Invoke on arn:aws:execute-api:<region>:<account>:<api-id>/*/*/*>}) -> HTTP 200 {'ServerId': 's-...'}.

Actual (all three checks, identical apart from the per-run port): botocore.errorfactory.InvalidRequestException: An error occurred (InvalidRequestException) when calling the CreateServer operation: Unable to verify access to API https://127.0.0.1:35819/_aws/execute-api/eocd2yo7iw/test (also observed with ports 41705 and 42425). Status 400, code InvalidRequestException, fixed message text 'Unable to verify access to API ' + the request's Url member.

Code evidence: authorization.rs::api_arn formats the resource as format!("{api_id}/{stage}/*") (line ~379) and its unit test an_invoke_url_resolves_to_the_apis_execute_api_arn asserts exactly that two-segment form for both addressings; invocation_role_can_invoke (line ~233) hands that string to iam_policy::Request::new(EXECUTE_API_INVOKE, ...) as the request resource and refuses on Decision::NotStated. The checks grant execute-api:Invoke on arn:aws:execute-api:<region>:<account>:<rest_api_id>/*/*/*.

Prior filing: 20261024T042010Z-transfer-fidelity-gaps-api-gateway-createserver-always-refused.md (status fixed) carried this as its Cause 1 with Issue-Key transfer|CreateServer|InvalidRequestException and trace ids transfer.test-identity-provider-invalid-request-error and transfer.test-identity-provider-service-managed-rejected; both are refused again, and transfer.test-identity-provider-invokes-configured-endpoint now fails the same way.

## Steps to Reproduce
1. Boot the emulator and point botocore at it (region us-west-2).
2. apigateway.create_rest_api; create_resource(pathPart='{proxy+}'); put_method(httpMethod='ANY', authorizationType='AWS_IAM'); put_integration (MOCK) with a canned IdP response; create_deployment(stageName='test'); build the stage invoke URL the emulator publishes (https://<endpoint>/_aws/execute-api/<api-id>/test).
3. iam.create_role trusted by transfer.amazonaws.com, then iam.put_role_policy with {'Effect':'Allow','Action':'execute-api:Invoke','Resource':'arn:aws:execute-api:<region>:<account>:<api-id>/*/*/*'}.
4. transfer.create_server(Protocols=['SFTP'], IdentityProviderType='API_GATEWAY', Domain='S3', EndpointType='PUBLIC', IdentityProviderDetails={'Url': <url from step 2>, 'InvocationRole': <role arn from step 3>}).
5. Observe HTTP 400 InvalidRequestException 'Unable to verify access to API <url>' instead of HTTP 200 with a ServerId. Every TestIdentityProvider check that needs an API_GATEWAY server dies here.
