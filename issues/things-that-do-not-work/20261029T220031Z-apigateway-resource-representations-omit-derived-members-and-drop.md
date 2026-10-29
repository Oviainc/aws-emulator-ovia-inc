Issue-Source: trace-sweep-divergence
Issue-Trace: apigatewayv2-domain-name-create-server-defaults
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigatewayv2-domain-name-create-server-defaults apigatewayv2-integration-response-errors apigatewayv2-integration-response-lifecycle apigatewayv2-model-lifecycle apigatewayv2-product-rest-endpoint-page-lifecycle apigatewayv2-quick-create-managed-resources apigatewayv2-route-integration-list
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigateway|CreateDomainName|2xx->2xx|$.domainNameArn
Issue-Key: apigateway|CreateDomainName|2xx->2xx|$.domainNameConfigurations[*].hostedZoneId
Issue-Key: apigateway|CreateDomainName|2xx->2xx|$.tags
Issue-Key: apigateway|GetDomainName|2xx->2xx|$.domainNameArn
Issue-Key: apigateway|GetDomainName|2xx->2xx|$.domainNameConfigurations[*].hostedZoneId
Issue-Key: apigateway|CreateIntegration|2xx->2xx|$.integrationResponseSelectionExpression
Issue-Key: apigateway|GetIntegrationResponse|2xx->2xx|$.responseTemplates.application/json
Issue-Key: apigateway|GetModel|2xx->2xx|$.description
Issue-Key: apigateway|CreateProductRestEndpointPage|2xx->2xx|$.displayContent.endpoint
Issue-Key: apigateway|CreateProductRestEndpointPage|2xx->2xx|$.lastModified
Issue-Key: apigateway|GetIntegrations|2xx->2xx|$.items[*].integrationId
Issue-Key: apigateway|GetIntegrations|2xx->2xx|$.items[*].integrationMethod
Issue-Key: apigateway|CreateRoute|2xx->2xx|$.apiGatewayManaged
Issue-Key: apigateway|CreateRoute|2xx->2xx|$.routeId
Issue-Key: apigateway|GetStages|2xx->2xx|$.items[*]
Issue-Commit: 89c27cbcf326b5c1a1abaeca904759cc614d9072

# apigateway resource representations omit derived members and drop updates

> _Note: this issue was identified against the code base at commit `89c27cbcf326b5c1a1abaeca904759cc614d9072`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomainName

## Description
Nine clusters where the response body is wrong independently of status: a derived member is missing, a server default is present that AWS omits (or absent where AWS emits it), an ARN is built with the wrong shape, or an accepted update is not reflected in the subsequent read. Each cause has its own fixture step.

Cause 1 of 9: CreateDomainName - ARN shape, missing hostedZoneId, phantom tags.
AWS: `domainNameArn` for a domain name has an EMPTY account-id field: `arn:aws:apigateway:<region>::/domainnames/<domainName>`; `domainNameConfigurations[].hostedZoneId` carries the fixed regional API Gateway hosted-zone id; and `tags` is OMITTED when no tags were supplied. Smithy: DomainNameConfiguration.HostedZoneId (jsonName `hostedZoneId`) - "The Amazon Route 53 Hosted Zone ID of the endpoint."
Emulator: the ARN interpolates the account id (`arn:aws:apigateway:<region>:<accountId>:/domainnames/<domainName>`), `hostedZoneId` is absent, and `tags` is present as `{}`.
Why: an ARN with an account id does not match an IAM resource pattern or an ACM/Route 53 cross-reference; a client cannot create the Route 53 alias record without the hosted-zone id; and a present-but-empty `tags` makes "has this resource been tagged" tests answer differently.
Fix: the domain-name response projection (ARN builder, regional hosted-zone id table, omit empty tags).
Traces: apigatewayv2-domain-name-create-server-defaults#task#2. Related open issue: `20261029T070719Z-apigatewayv2-3-net-new-root-causes-the-10.md` notes DomainNameConfiguration omits HostedZoneId but does not carry this fixture step's replay key.

Cause 2 of 9: GetDomainName - same ARN shape and missing hostedZoneId.
AWS: as cause 1.
Emulator: account id in the ARN, `hostedZoneId` absent.
Why/Fix: as cause 1, in the GetDomainName projection.
Traces: apigatewayv2-domain-name-create-server-defaults#task#3.

Cause 3 of 9: CreateIntegration omits integrationResponseSelectionExpression.
AWS: the integration body carries `integrationResponseSelectionExpression` with its server default `${integration.response.statuscode}` (fixed text, not derived from the request).
Emulator: the member is absent.
Why: a client that reads the selection expression to wire integration responses has nothing to read.
Fix: the CreateIntegration response projection - populate the default selection expression.
Traces: apigatewayv2-integration-response-errors#setup#2.

Cause 4 of 9: GetIntegrationResponse returns a stale responseTemplates value.
AWS: after UpdateIntegrationResponse changes `responseTemplates["application/json"]`, the read returns the NEW template.
Emulator: returns the template the resource was created with - the update was acknowledged but not persisted.
Why: a client's integration-response mapping silently never changes.
Fix: the UpdateIntegrationResponse state mutation (the responseTemplates map is not written back).
Traces: apigatewayv2-integration-response-lifecycle#task#7.

Cause 5 of 9: GetModel omits a description set by UpdateModel.
AWS: after the update the model body carries the new `description`.
Emulator: `description` is absent entirely.
Why: the client's model documentation is lost.
Fix: the UpdateModel state mutation for `description`.
Traces: apigatewayv2-model-lifecycle#task#7.

Cause 6 of 9: CreateProductRestEndpointPage derives the wrong endpoint URL.
AWS: `displayContent.endpoint` is the stage invoke URL only - `https://<apiId>.execute-api.<region>.amazonaws.com/<stageName>` - with no resource path appended.
Emulator: appends the resource path (`.../<stageName>/items`).
Why: the portal publishes an endpoint that is not the stage root a client is meant to call.
Fix: the restEndpointIdentifier -> endpoint URL derivation in the ProductRestEndpointPage projection.
Traces: apigatewayv2-product-rest-endpoint-page-lifecycle#task#6.

Cause 7 of 9: GetIntegrations omits integrationMethod on a quick-create managed integration.
AWS: the listed integration carries `integrationMethod: ANY` for the api-gateway-managed quick-create integration.
Emulator: `integrationMethod` is absent.
Why: a client inspecting the managed integration cannot see which HTTP method it forwards.
Fix: the quick-create integration materialization (set integrationMethod) and the GetIntegrations projection.
Traces: apigatewayv2-quick-create-managed-resources#task#2.

Cause 8 of 9: CreateRoute emits apiGatewayManaged on a user-created route.
AWS: `apiGatewayManaged` is OMITTED for a route the caller created; it is present (true) only for routes API Gateway itself manages.
Emulator: emits `apiGatewayManaged: false`.
Why: a client that uses the member's presence to tell managed from unmanaged routes classifies every route as managed-known.
Fix: the route response projection - omit apiGatewayManaged unless the route is managed.
Traces: apigatewayv2-quick-create-managed-resources#task#7.

Cause 9 of 9: GetStages emits apiGatewayManaged inside list items.
AWS: the listed stage carries `stageName`, `defaultRouteSettings`, `routeSettings`, `stageVariables`, `createdDate`, `lastUpdatedDate`, `tags` - and no `apiGatewayManaged`.
Emulator: adds `apiGatewayManaged: false` to the list element.
Why/Fix: as cause 8, in the stage list projection.
Traces: apigatewayv2-route-integration-list#task#7.

## Evidence
Replay verdict `mismatch` with matching statuses. Details: `$.domainNameArn: value_mismatch (expected 'arn:aws:apigateway:us-west-2::/domainnames/<name>', got 'arn:aws:apigateway:us-west-2:000000000000:/domainnames/<name>')`, `$.domainNameConfigurations[0].hostedZoneId: missing`, `$.tags: type_mismatch (expected None, got {})`; `$.integrationResponseSelectionExpression: missing (expected '${integration.response.statuscode}')`; `$.responseTemplates.application/json: value_mismatch (expected "$input.json('$.body')", got "$input.json('$')")`; `$.description: missing (expected 'an updated probe model')`; `$.displayContent.endpoint: value_mismatch` (emulator appends `/items`); `$.items[0].integrationMethod: missing (expected 'ANY')`; `$.apiGatewayManaged: unexpected_present (expected None, got False)`.

## Steps to Reproduce
Replay the corpus. Cause 1: POST /v2/domainnames (fixture apigatewayv2-domain-name-create-server-defaults step task#2) and compare `domainNameArn`, `domainNameConfigurations[0].hostedZoneId` and `tags`. Cause 4: create an integration response, PATCH its responseTemplates, then GET it (apigatewayv2-integration-response-lifecycle#task#7) - the old template comes back.
