Issue-Source: trace-sweep-divergence
Issue-Trace: apigatewayv2-portal-with-included-portal-product
Issue-Step: setup#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigatewayv2-portal-with-included-portal-product apigatewayv2-portal-with-included-product
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigateway|CreatePortal|2xx->2xx|$.endpointConfiguration.portalDefaultDomainName
Issue-Related: 20261029T220031Z-apigateway-portal-lifecycle-and-representation-preview-hostedzoneid-publishstatus.md
Issue-Commit: e4d3227510b7f117c2f27de6806d553a13c34f2d

# apigateway CreatePortal returns an invented portalDomainHostedZoneId instead of the regional constant

> _Note: this issue was identified against the code base at commit `e4d3227510b7f117c2f27de6806d553a13c34f2d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePortal

## Description
CreatePortal's endpoint-configuration projection mints the developer portal's Route 53 hosted-zone identifier instead of serving the fixed per-region value API Gateway publishes for portal default domains. One cause.

Cause 1 of 1: CreatePortal answers 201 with the wrong endpointConfiguration.portalDomainHostedZoneId.
AWS: Smithy (com.amazonaws.apigatewayv2#EndpointConfigurationResponse): member PortalDomainHostedZoneId (jsonName portalDomainHostedZoneId) is @required, @length min 1 max 64, documented as "The portal domain hosted zone identifier"; it is bound into CreatePortal's output (POST /v2/portals, modeled code 201) via EndpointConfiguration. The model does not state the value, so recorded behavior only: the recording shows a single fixed identifier for the region the portal is created in (us-west-2 records Z091913938PZVWK6ZMXI7). This is an AWS per-region constant, like the regional hosted-zone ids API Gateway returns for custom domain names - it is a property of the region, not of the portal, so it is identical for every portal created in that region and across runs.
Emulator: the same step returns a per-emulator value (recorded as Z47A2501D8FE6F) that is not the region's portal hosted-zone id, for every portal, in both CreatePortal response bodies of this cluster.
Why: the only use of portalDomainHostedZoneId is to build a Route 53 ALIAS/CNAME record (or a CloudFormation AliasTarget) that points a customer domain at the portal's default domain. A client that reads this member writes an alias record against a hosted zone that does not exist, so the DNS change is rejected or silently points nowhere; the client has no other source for the value and cannot work around it.
Fix: the CreatePortal handler's endpoint-configuration projection - resolve portalDomainHostedZoneId from a region -> portal-hosted-zone-id table keyed by the request's region, the same way the custom-domain-name projection resolves domainNameConfigurations[].hostedZoneId, rather than deriving or generating it per instance.
Traces: apigatewayv2-portal-with-included-portal-product#setup#2 (cluster also covers apigatewayv2-portal-with-included-product#setup#2).

Explicitly NOT part of this issue: the same two clusters also report $.endpointConfiguration.portalDefaultDomainName differing (AWS p-<portalId>.apigw-portal.<region>.on.aws vs the emulator's loopback host:port plus portal path). That member is the address the portal is actually served on, and an emulator must announce an address it is reachable on, so it is not filed here - consistent with the note in the open issue 20261029T220031Z-apigateway-portal-lifecycle-and-representation-preview-hostedzoneid-publishstatus.md.

Related: open issue 20261029T220031Z-apigateway-portal-lifecycle-and-representation-preview-hostedzoneid-publishstatus.md describes the same hosted-zone-id fault for other portal fixtures/operations (its causes 4, 6, 7, 8, 9) but does not carry this cluster's replay key, so this step is not gated by it.

## Evidence
Replay verdict `mismatch`, fixture apigatewayv2-portal-with-included-portal-product step setup#2, CreatePortal, expected status 201 / actual 201. Detail: `$.endpointConfiguration.portalDomainHostedZoneId: value_mismatch (expected 'Z091913938PZVWK6ZMXI7', got 'Z47A2501D8FE6F')`. The expected token is the us-west-2 portal hosted-zone constant recorded from AWS; the actual is an emulator-local value. The same cluster covers apigatewayv2-portal-with-included-product#setup#2 with the identical actual value, so the emulator's value does not vary per portal either - it is simply not the region's identifier. (The cluster's second reported key, `$.endpointConfiguration.portalDefaultDomainName: value_mismatch (expected 'p-{{bundling_portal_id}}.apigw-portal.us-west-2.on.aws', got '127.0.0.1:35149/_aws/portal/z1hvlts8lq')`, is the emulator's own reachable address and is deliberately excluded.)

## Steps to Reproduce
Replay the simparity-wire-aws-v1 corpus against the emulator. Fixture apigatewayv2-portal-with-included-portal-product, step setup#2: POST /v2/portals with authorization, endpointConfiguration, portalContent and an includedPortalProductArns entry. The response is 201 and its endpointConfiguration.portalDomainHostedZoneId is the emulator's generated id (Z47A2501D8FE6F) rather than the region's portal hosted-zone id recorded by AWS for us-west-2 (Z091913938PZVWK6ZMXI7). The same assertion fails on apigatewayv2-portal-with-included-product#setup#2.
