Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudfront__dynamodb__lambda__s3.e2e-spa-distribution-serves-shell-and-api-writes-dynamodb apigateway__cloudfront__dynamodb__lambda__s3.e2e-spa-api-without-lambda-permission-rejected
Issue-Key: apigateway__cloudfront__dynamodb__lambda__s3.e2e-spa-distribution-serves-shell-and-api-writes-dynamodb|run#38|PublishedUrlRejected|edge = ctx.endpoint_url(dist["DomainName"])
Issue-Key: apigateway__cloudfront__dynamodb__lambda__s3.e2e-spa-api-without-lambda-permission-rejected|run#32|PublishedUrlRejected|edge = ctx.endpoint_url(dist["DomainName"])
Issue-Commit: 5469355dc4ba582ea958345f64905a5aea7e37f0

# apigateway__cloudfront__dynamodb__lambda__s3: a distribution's DomainName is published as <id>.cloudfront.net, an address the emulator does not serve, so no SPA/API edge request reaches a verdict

> _Note: this issue was identified against the code base at commit `5469355dc4ba582ea958345f64905a5aea7e37f0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDistribution

## Description
One cause.

Cause 1 of 1: CloudFront publishes an edge hostname that is not one of the emulator's own hosts, so every request through the distribution is unroutable.

AWS: CreateDistribution/GetDistribution return Distribution.DomainName, and that name is the address a client actually fetches from — GET https://<DomainName>/<path> reaches the distribution and is served from the configured origin (Smithy: cloudfront CreateDistribution output Distribution member DomainName; the viewer request is the distribution's data plane). The contract is that the published name resolves to the service answering the request.

Emulator: DomainName is minted in AWS's public shape (recorded here as a '<id>.cloudfront.net' host) while the emulator answers only on its own host/port. The harness rejects the published endpoint with 'endpoint_url was given https://<id>.cloudfront.net ... its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener'. The host is the fixed part of the defect; the distribution id and the port differ per run.

Why: a client that does exactly what AWS documents — take DomainName and fetch it — never reaches the emulator. Both the positive SPA path (shell served from S3, /api write landing in DynamoDB) and the negative path (API Gateway integration without lambda:InvokeFunction permission must answer 500/403 at the edge) are unverifiable, so the whole CloudFront-fronted estate is unusable even though the control plane reports CREATE/Deployed.

Fix: the CloudFront distribution state/serializer must publish a DomainName the emulator serves (its own host, or a per-distribution listener), and the CloudFront data plane must route viewer requests for that name to the configured origin.

Traces: apigateway__cloudfront__dynamodb__lambda__s3.e2e-spa-distribution-serves-shell-and-api-writes-dynamodb, apigateway__cloudfront__dynamodb__lambda__s3.e2e-spa-api-without-lambda-permission-rejected. Related open issues name the same symptom under other service headings (20261109T152222Z-apigateway-cloudfront-s3-a-distribution-s-domainname-is.md, 20261109T164957Z-cloudfront-kms-s3-a-distribution-s-domainname-is.md); neither carries these two checks' replay keys.

## Evidence
Expected: GET https://<Distribution.DomainName>/ serves the S3-origin SPA shell; POST https://<DomainName>/api writes the item to DynamoDB; the same path without lambda:InvokeFunction is refused at the edge.
Actual: simparity.published.PublishedUrlRejected: endpoint_url was given https://du75el7ewhhv91.cloudfront.net (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:35529/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener.
Raised from context.py endpoint_route -> _published_target, before any edge request could be made.

## Steps to Reproduce
1. Create the SPA estate: S3 origin bucket + API Gateway REST API + Lambda + DynamoDB table.
2. cloudfront.create_distribution(...) with the S3 origin and the API Gateway custom origin.
3. Read Distribution.DomainName from the response (or GetDistribution).
4. Attempt any HTTP GET/POST against https://<DomainName>/ -> the published host is not an address the emulator answers on.
