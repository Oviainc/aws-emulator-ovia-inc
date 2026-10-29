Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway.resource-policy-denies-unlisted-path-at-stage apigateway.deep-apigw-integration-usage-plan-quota-meters-each-key-separately apigateway.test-invoke-method-captures-latency-and-log-server-side apigateway.deploy-rest-api-stage-cfn-invoke-behaves apigateway.get-model-template-and-models apigateway.get-api-keys-pagination-order apigateway.get-deployments-ordering apigateway.op-contract-delete-stage apigateway.delete-nonexistent-integration-returns-notfound apigateway.integration-delete-cascade apigateway.delete-method-removes-integration apigateway.get-integration-response-not-found-vs-badrequest apigateway.put-integration-without-method-not-found
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway.resource-policy-denies-unlisted-path-at-stage|run#27|AssertionError|assert "because no resource-based policy allows the execute-api:Invoke action" in denial, (
Issue-Key: apigateway.deep-apigw-integration-usage-plan-quota-meters-each-key-separately|assert_quota_rejection#21|AssertionError|assert rejection is not None, (
Issue-Key: apigateway.test-invoke-method-captures-latency-and-log-server-side|run#20|AssertionError|assert marker in log, (
Issue-Key: api-gateway|TestInvokeMethod|NotFoundException
Issue-Key: apigateway.get-model-template-and-models|run#15|AssertionError|assert "Empty" in names and "Error" in names, (
Issue-Key: apigateway.get-api-keys-pagination-order|run#11|AssertionError|assert order_a not in (sorted(names), sorted(names, reverse=True)), (
Issue-Key: apigateway.get-deployments-ordering|run#11|AssertionError|assert returned_order == expected_order, (
Issue-Key: apigateway.op-contract-delete-stage|run#22|AssertionError|assert not any(
Issue-Key: api-gateway|DeleteIntegration|NotFoundException
Issue-Key: apigateway.delete-method-removes-integration|run#9|AssertionError|assert message == "Invalid Integration identifier specified", f"unexpected message: {message!r}"
Issue-Key: apigateway.get-integration-response-not-found-vs-badrequest|run#17|AssertionError|assert msg_b == "Invalid Integration identifier specified", f"unexpected message B: {msg_b!r}"
Issue-Key: apigateway.put-integration-without-method-not-found|run#15|AssertionError|assert absent_code_message == (
Issue-Commit: c8d80ea15f1454d07f388eb2b932b3a5430349f4

# API Gateway control-plane state and representation: 10 root causes (DeleteIntegration unusable, integration-identifier errors, collection ordering, built-in models, quota metering, TestInvokeMethod log)

> _Note: this issue was identified against the code base at commit `c8d80ea15f1454d07f388eb2b932b3a5430349f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteIntegration

## Description
This issue bundles 10 NET-NEW root causes in API Gateway control-plane state keeping and response representation. A fixer must close all 10 before submitting.

Cause 1 of 10: a resource-policy denial does not say the resource policy is what refused the call.
AWS: when a stage invocation is denied by the API's resource policy, the 403 message names the resource policy as the reason (the check asserts AWS's exact resource-policy wording).
Emulator: emits the identity-based wording 'User: anonymous is not authorized to perform: execute-api:Invoke on resource: arn:aws:execute-api:us-west-2:000000000000:<apiId>/<stage>/GET/blocked' — fixed text 'User: ... is not authorized to perform: execute-api:Invoke on resource: ', with principal and ARN taken from the request.
Why: a client cannot tell a resource-policy denial from an identity-policy denial, which is the whole point of the two messages.
Fix: the authorization-denial message builder in the invoke pipeline.
Traces: apigateway.resource-policy-denies-unlisted-path-at-stage

Cause 2 of 10: usage-plan DAY quota is not metered or enforced at the edge.
AWS: once a usage plan's DAY quota is consumed, further invocations with that API key are refused with HTTP 429; each key is metered separately.
Emulator: key a was admitted 7 times and never refused with a DAY quota of 2; status sequence [200, 200, 200, 200, 200, 200, 200].
Why: quota-dependent behaviour (back-pressure, billing guards) never triggers.
Fix: usage-plan metering/enforcement state consulted by the invoke pipeline.
Traces: apigateway.deep-apigw-integration-usage-plan-quota-meters-each-key-separately

Cause 3 of 10: the TestInvokeMethod execution log is a static stub that does not reflect the configured integration.
AWS: the returned log records what actually happened for that request, so it varies with the method's configured integration and templates.
Emulator: returns a fixed line sequence ('Starting execution for request', 'HTTP Method', 'Method request headers: []', 'Integration type: MOCK', 'Successfully completed execution', ...) that never mentions the request's own response-template marker.
Why: callers that parse the execution log to debug an integration get information unrelated to their configuration.
Fix: TestInvokeMethod's log generation (derive it from the executed integration, not a template).
Traces: apigateway.test-invoke-method-captures-latency-and-log-server-side

Cause 4 of 10: a REST API provisioned by CloudFormation is not addressable through the API Gateway control plane.
AWS: after a stack creates an AWS::ApiGateway::RestApi/Stage, GetStage/TestInvokeMethod on the physical id succeed.
Emulator: TestInvokeMethod refuses with NotFoundException 'Invalid API identifier specified 000000000000:ddee2a8de8' — fixed text 'Invalid API identifier specified ', with the account id and api id from the request. The open issue 20261023T125454Z-apigateway-cloudformation-provisioned-api-gateway-resources-are-unaddressable-through.md describes the same shape but does not carry this check.
Why: nothing deployed through CloudFormation can be driven through the API Gateway API.
Fix: the id/registry linkage between the CloudFormation resource provisioner and apigateway state.
Traces: apigateway.deploy-rest-api-stage-cfn-invoke-behaves

Cause 5 of 10: a new REST API is not seeded with the built-in Empty and Error models.
AWS: every RestApi is created with the built-in models named 'Empty' and 'Error', and GetModels lists them alongside user models.
Emulator: GetModels returns only the user-created model; got {'Sim2RealWidget'}.
Why: PutMethodResponse/PutIntegrationResponse references to 'Empty'/'Error' — the documented default models — cannot resolve.
Fix: CreateRestApi state initialization (seed built-in models).
Traces: apigateway.get-model-template-and-models

Cause 6 of 10: GetApiKeys returns keys alphabetized by name instead of in the service's stable order.
AWS: GetApiKeys returns a stable server-defined order that is not a sort by name, and it is the same across pages.
Emulator: returns ['...-zulu', '...-yankee', '...-xray', '...-whiskey', '...-victor'] — a name sort.
Why: paginating clients that rely on a stable non-alphabetical order see a different sequence and can skip or repeat keys.
Fix: GetApiKeys listing/ordering in the api-key state.
Traces: apigateway.get-api-keys-pagination-order

Cause 7 of 10: GetDeployments is not id-ascending.
AWS: GetDeployments returns deployments in ascending id order, independent of insertion order.
Emulator: returned ['6ofuwnbaaa', '5mw7f4baaa', '4kdlzkcaaa'] (insertion order) where ascending id order was expected.
Why: clients that take the first/last element to find the oldest or newest deployment pick the wrong one.
Fix: GetDeployments listing/ordering in the deployment state.
Traces: apigateway.get-deployments-ordering

Cause 8 of 10: DeleteStage does not cascade the usage plan's apiStage reference.
AWS: deleting a stage removes the stale apiStage entry from any usage plan that referenced it.
Emulator: the usage plan still carries the apiStage reference to the deleted stage.
Why: a usage plan keeps metering a stage that no longer exists, and GetUsagePlan returns a dangling reference.
Fix: DeleteStage's cascade over usage-plan state.
Traces: apigateway.op-contract-delete-stage

Cause 9 of 10: DeleteIntegration refuses an integration that exists.
AWS: DeleteIntegration on a method that has an integration returns 204 and removes it (Smithy: DeleteIntegration DELETE /restapis/{restApiId}/resources/{resourceId}/methods/{httpMethod}/integration, code 204; NotFoundException only when there is no integration).
Emulator: raises NotFoundException 'Invalid Integration identifier specified' for an integration that was just created with PutIntegration, so the call cannot be made at all and the cascade cannot be observed.
Why: integrations cannot be deleted, and any teardown or reconfiguration flow fails.
Fix: integration lookup/keying in DeleteIntegration's handler and the integration state it reads.
Traces: apigateway.delete-nonexistent-integration-returns-notfound, apigateway.integration-delete-cascade

Cause 10 of 10: integration lookups report the Method identifier instead of the Integration identifier.
AWS: GetIntegration / GetIntegrationResponse / PutIntegration on a method with no integration refuse with NotFoundException 'Invalid Integration identifier specified' — fixed text, no variable part. 'Invalid Method identifier specified' is AWS's message for a missing METHOD, a different condition.
Emulator: returns 'Invalid Method identifier specified' for the missing-integration condition.
Why: a client cannot distinguish 'this method does not exist' from 'this method has no integration', which drives different remediation.
Fix: the not-found message selection in the integration handlers.
Traces: apigateway.delete-method-removes-integration, apigateway.get-integration-response-not-found-vs-badrequest, apigateway.put-integration-without-method-not-found

## Evidence
Representative replay output (verbatim):
- 'AssertionError: the denial must say the resource policy is what failed to allow the call, got 'User: anonymous is not authorized to perform: execute-api:Invoke on resource: arn:aws:execute-api:us-west-2:000000000000:8sh3tqaaaa/suemdfx0i4i/GET/blocked'' (Cause 1)
- 'AssertionError: key a was admitted 7 times and never refused; the DAY quota of 2 was not enforced; sequence [200, 200, 200, 200, 200, 200, 200]' (Cause 2)
- 'AssertionError: execution log does not mention our per-request response-template marker; a static stub log would not vary with the actual configured integration.' (Cause 3)
- 'botocore.errorfactory.NotFoundException: An error occurred (NotFoundException) when calling the TestInvokeMethod operation: Invalid API identifier specified 000000000000:ddee2a8de8' (Cause 4)
- 'AssertionError: expected built-in models 'Empty' and 'Error' to still be present in GetModels, got {'Sim2RealWidget'}' (Cause 5)
- 'AssertionError: GetApiKeys appears to simply alphabetize by name rather than preserve a stable server-defined order' (Cause 6)
- 'AssertionError: GetDeployments not id-ascending: got ['6ofuwnbaaa', '5mw7f4baaa', '4kdlzkcaaa'], expected ['4kdlzkcaaa', '5mw7f4baaa', '6ofuwnbaaa']' (Cause 7)
- 'AssertionError: expected the usage plan's stale apiStage reference to be cascaded away' (Cause 8)
- 'botocore.errorfactory.NotFoundException: An error occurred (NotFoundException) when calling the DeleteIntegration operation: Invalid Integration identifier specified' (Cause 9)
- 'AssertionError: unexpected message B: 'Invalid Method identifier specified'' where 'Invalid Integration identifier specified' is required (Cause 10)

## Steps to Reproduce
1. Create a REST API with a resource, a GET method and a MOCK integration.
2. Call DeleteIntegration on that method — it refuses with NotFoundException 'Invalid Integration identifier specified' (Cause 9). Delete the method instead and then GetIntegration/GetIntegrationResponse — the message is 'Invalid Method identifier specified' (Cause 10).
3. Call GetModels on a freshly created API (Cause 5), GetApiKeys after creating several named keys (Cause 6), GetDeployments after three deployments (Cause 7).
4. Create a usage plan with an apiStage, delete the stage, then GetUsagePlan (Cause 8). Attach a DAY quota of 2 and invoke repeatedly with the key (Cause 2).
5. Deploy an AWS::ApiGateway::RestApi+Stage via CloudFormation and call TestInvokeMethod on the physical id (Cause 4).
6. Attach a resource policy denying one path and invoke it (Cause 1); call TestInvokeMethod and read 'log' (Cause 3).
