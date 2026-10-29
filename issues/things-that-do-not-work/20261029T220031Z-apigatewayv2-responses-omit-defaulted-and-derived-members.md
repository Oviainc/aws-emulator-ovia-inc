Issue-Source: trace-sweep-divergence
Issue-Trace: apigatewayv2-create-routing-rule-missing-condition-rejected
Issue-Step: task#11
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigatewayv2-create-routing-rule-missing-condition-rejected apigatewayv2-put-routing-rule-nonexistent-not-found apigatewayv2-routing-rule-missing-condition-rejected apigatewayv2-routing-rule-priority-numeric-precision apigatewayv2-domain-routing-rule-bogus-child-and-bounds apigatewayv2-websocket-deployment-requires-route-targets
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigatewayv2|CreateRoutingRule|2xx->2xx|$.actions[*].invokeApi.stripBasePath
Issue-Key: apigatewayv2|GetRoutingRule|2xx->2xx|$.actions[*].invokeApi.stripBasePath
Issue-Key: apigatewayv2|PutRoutingRule|2xx->2xx|$.actions[*].invokeApi.stripBasePath
Issue-Key: apigatewayv2|GetDomainName|2xx->2xx|$.domainNameArn
Issue-Key: apigatewayv2|GetDomainName|2xx->2xx|$.domainNameConfigurations[*].hostedZoneId
Issue-Key: apigatewayv2|CreateIntegration|2xx->2xx|$.integrationId
Issue-Key: apigatewayv2|CreateIntegration|2xx->2xx|$.integrationResponseSelectionExpression
Issue-Commit: 89c27cbcf326b5c1a1abaeca904759cc614d9072

# apigatewayv2 responses omit defaulted and derived members

> _Note: this issue was identified against the code base at commit `89c27cbcf326b5c1a1abaeca904759cc614d9072`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRoutingRule

## Description
Five clusters where the status is right but the body omits a member AWS always emits: a boolean server default on a routing rule's invokeApi action, the domain name's hosted-zone id and ARN shape, and an integration's default response selection expression. Each cause has its own fixture step.

Cause 1 of 5: CreateRoutingRule omits actions[].invokeApi.stripBasePath.
AWS: the invokeApi action always carries `stripBasePath` with its server default `false` when the caller did not set it.
Emulator: the member is absent from the action.
Why: a client that reads actions[].invokeApi.stripBasePath to decide whether the base path is forwarded gets a null instead of the effective value, and a read-modify-write of the rule drops the setting.
Fix: the routing-rule action response projection - populate the stripBasePath default.
Traces: apigatewayv2-create-routing-rule-missing-condition-rejected#task#11.

Cause 2 of 5: GetRoutingRule omits actions[].invokeApi.stripBasePath.
AWS: `stripBasePath: false` present.
Emulator: absent.
Why/Fix: as cause 1, in the GetRoutingRule projection.
Traces: apigatewayv2-create-routing-rule-missing-condition-rejected#task#12.

Cause 3 of 5: PutRoutingRule omits actions[].invokeApi.stripBasePath.
AWS: `stripBasePath: false` present.
Emulator: absent.
Why/Fix: as cause 1, in the PutRoutingRule projection.
Traces: apigatewayv2-put-routing-rule-nonexistent-not-found#task#13.

Cause 4 of 5: GetDomainName - wrong ARN shape and missing hostedZoneId.
AWS: `domainNameArn` has an EMPTY account-id field (`arn:aws:apigateway:<region>::/domainnames/<domainName>`) and `domainNameConfigurations[].hostedZoneId` carries the fixed regional API Gateway hosted-zone id. Smithy: DomainNameConfiguration.HostedZoneId (jsonName `hostedZoneId`) - "The Amazon Route 53 Hosted Zone ID of the endpoint."
Emulator: the ARN interpolates the account id and `hostedZoneId` is absent.
Why: an ARN carrying an account id does not match the IAM/ACM resource pattern a client builds, and without the hosted-zone id a client cannot create the Route 53 alias record for the custom domain.
Fix: the domain-name response projection (ARN builder and the regional hosted-zone id table). Related open issue: `20261029T070719Z-apigatewayv2-3-net-new-root-causes-the-10.md` notes DomainNameConfiguration omits HostedZoneId but does not carry this fixture step's replay key.
Traces: apigatewayv2-domain-routing-rule-bogus-child-and-bounds#task#3.

Cause 5 of 5: CreateIntegration omits integrationResponseSelectionExpression.
AWS: the integration body carries `integrationResponseSelectionExpression` with its server default `${integration.response.statuscode}` - fixed text, not derived from the request.
Emulator: the member is absent.
Why: a client that reads the selection expression to wire integration responses has nothing to read.
Fix: the CreateIntegration response projection - populate the default selection expression.
Traces: apigatewayv2-websocket-deployment-requires-route-targets#task#2.

## Evidence
Replay verdict `mismatch` with matching statuses. Details: `$.actions[0].invokeApi.stripBasePath: missing (expected False, got None)` on the three routing-rule clusters; `$.domainNameArn: value_mismatch (expected 'arn:aws:apigateway:us-west-2::/domainnames/<name>', got 'arn:aws:apigateway:us-west-2:000000000000:/domainnames/<name>')` plus `$.domainNameConfigurations[0].hostedZoneId: missing`; `$.integrationResponseSelectionExpression: missing (expected '${integration.response.statuscode}', got None)`.

## Steps to Reproduce
Replay the corpus. Cause 1: create a domain name and POST /v2/domainnames/{domainName}/routingrules with an invokeApi action and no stripBasePath (fixture apigatewayv2-create-routing-rule-missing-condition-rejected step task#11) - the 201 body's action has no stripBasePath. Cause 4: GET /v2/domainnames/{domainName} (apigatewayv2-domain-routing-rule-bogus-child-and-bounds#task#3) and compare domainNameArn and hostedZoneId.
