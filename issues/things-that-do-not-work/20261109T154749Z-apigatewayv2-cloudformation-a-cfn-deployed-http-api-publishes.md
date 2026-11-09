Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__cloudformation.deploy-apigatewayv2-cfn-disable-execute-api-endpoint-blocks-default-domain apigatewayv2__cloudformation.deploy-apigatewayv2-cfn-inline-body-openapi-proxy-serves-its-routes
Issue-Key: apigatewayv2__cloudformation.deploy-apigatewayv2-cfn-disable-execute-api-endpoint-blocks-default-domain|run#12|PublishedUrlRejected|enabled_endpoint = ctx.stage_url(enabled["ApiEndpoint"], "$default")
Issue-Key: apigatewayv2__cloudformation.deploy-apigatewayv2-cfn-inline-body-openapi-proxy-serves-its-routes|run#11|PublishedUrlRejected|api_endpoint = ctx.stage_url(got_api["ApiEndpoint"], "$default")
Issue-Commit: 39b863d101414613449d16e44f0b23fd775a8d87

# apigatewayv2__cloudformation: a CFN-deployed HTTP API publishes an AWS execute-api ApiEndpoint the emulator does not serve, so neither the inline-OpenAPI routes nor DisableExecuteApiEndpoint can be exercised

> _Note: this issue was identified against the code base at commit `39b863d101414613449d16e44f0b23fd775a8d87`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApi

## Description
Cause 1 of 1: an API created by a CloudFormation stack publishes its invoke address as an AWS public execute-api hostname instead of an address the emulator answers on, so no request reaches the stack's API.

AWS: Smithy (service-com-amazonaws-apigatewayv2-apigatewayv2): CreateApi/GetApi output member ApiEndpoint (jsonName apiEndpoint) is documented as "The URI of the API, of the form {api-id}.execute-api.{region}.amazonaws.com"; the same value is what AWS::ApiGatewayV2::Api exposes via Fn::GetAtt ApiEndpoint. On AWS that URI is served by the API, so a caller takes it and reaches the route. With DisableExecuteApiEndpoint=true AWS answers requests to that default host 403 Forbidden with body {"message":"Forbidden"}; with an inline OpenAPI Body the declared paths serve their integrations.
Emulator: the stack's api answers GetApi/Fn::GetAtt with "https://<api-id>.execute-api.<region>.amazonaws.com", a host the emulator does not listen on. Replay refuses to leave the emulator with simparity.published.PublishedUrlRejected ("its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener"), so no HTTP request is made and both the DisableExecuteApiEndpoint gate and the inline-Body route serving reach no verdict.
Why: a CFN/CDK estate deployed against the emulator cannot be driven end to end — the only address the stack publishes for its API points outside the emulator.
Fix: the ApiEndpoint construction in the apigatewayv2 crate's CreateApi/GetApi/ImportApi handlers (which the cloudformation AWS::ApiGatewayV2::Api handler surfaces as Fn::GetAtt ApiEndpoint) and the execute-api front-door listener that must serve the published address.
Related but not covering: 20261109T120740Z-iot-the-published-data-plane-address-is-a.md names the same published value under service iot.
Traces: apigatewayv2__cloudformation.deploy-apigatewayv2-cfn-disable-execute-api-endpoint-blocks-default-domain, apigatewayv2__cloudformation.deploy-apigatewayv2-cfn-inline-body-openapi-proxy-serves-its-routes.

## Evidence
Expected: after CreateStack reaches CREATE_COMPLETE, Fn::GetAtt/GetApi ApiEndpoint is reachable by the caller — the inline-Body route answers its integration, and with DisableExecuteApiEndpoint=true the default host answers 403 {"message":"Forbidden"}. Actual: simparity.published.PublishedUrlRejected at context.py:572 — "endpoint_url was given https://<api-id>.execute-api.us-west-2.amazonaws.com (published by the emulator or built by the check): its host is not one of the emulator's own hosts" (recordings show api ids e77k44w5b0 and mp67t4dbd4); no request is issued.

## Steps to Reproduce
1. CreateStack with an AWS::ApiGatewayV2::Api (inline OpenAPI Body or Name/ProtocolType plus routes and an AutoDeploy stage) and an Output of Fn::GetAtt Api.ApiEndpoint.
2. Read the output: it is https://<api-id>.execute-api.<region>.amazonaws.com, not the emulator's host:port; a request to it never reaches the emulator.
3. Replay apigatewayv2__cloudformation.deploy-apigatewayv2-cfn-inline-body-openapi-proxy-serves-its-routes and apigatewayv2__cloudformation.deploy-apigatewayv2-cfn-disable-execute-api-endpoint-blocks-default-domain.
