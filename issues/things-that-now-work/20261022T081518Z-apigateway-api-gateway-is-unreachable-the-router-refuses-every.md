Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__lambda__logs__resourcegroupstaggingapi__sqs.api-key-discovered-tagged-and-removed-from-index-on-delete apigateway__lambda__logs__resourcegroupstaggingapi__sqs.tag-resources-nonexistent-api-key-arn-rejected-per-resource
Issue-Key: apigateway__lambda__logs__resourcegroupstaggingapi__sqs|CreateApiKey|AmbiguousService
Issue-Commit: 5ef273b5ec187d9b55db2ccfdad00816d8c5c39a

# API Gateway is unreachable: the router refuses every apigateway-signed request with AmbiguousService because two emulated services claim the 'apigateway' signing name

> _Note: this issue was identified against the code base at commit `5ef273b5ec187d9b55db2ccfdad00816d8c5c39a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApiKey

## Description
Cause 1 of 1: every request addressed to API Gateway is refused by the emulator's service router before it reaches a handler.

AWS: CreateApiKey is a real, bound API Gateway operation — Smithy: CreateApiKey, POST /apikeys, http code 201, output shape ApiKey (id, name, enabled, createdDate, tags, ...), modeled errors BadRequestException 400 / ConflictException 409 / LimitExceededException 429 / NotFoundException 404 / TooManyRequestsException 429 / UnauthorizedException 401. AmbiguousService is not an AWS error code at all, and no API Gateway operation can answer it. GetApiKey (GET /apikeys/{apiKey}, 200) and DeleteApiKey (DELETE /apikeys/{apiKey}, 202) are likewise reachable.
Emulator: the call fails with an emulator-generated 5xx-class refusal, retried to exhaustion by botocore: "An error occurred (AmbiguousService) when calling the CreateApiKey operation (reached max retries: 3): 2 emulated services all claim requests for 'apigateway': apigateway, apigateway; the request is rejected rather than served by an arbitrary one". The router resolves a request to a service by its sigv4 signing name, and two bootstrapped services sign as 'apigateway' (the API Gateway control plane and ApiGatewayV2), so the resolution is ambiguous and the request is dropped. The two distinct claimants are reported under the same display token, so the message cannot even tell them apart.
Why: API Gateway is completely unusable through the emulator — not one operation of the service can be exercised, and the failure surfaces as a retried server error rather than any modeled client error, so callers burn their full retry budget on every call. Any scenario that touches API Gateway (here, API-key creation feeding tagging-API discovery) cannot run at all.
Fix: the request router's service-resolution step — a signing name shared by several modeled services must be disambiguated (by endpoint prefix / host, X-Amz-Target or the request's method+URI against each candidate's bound operations) instead of being treated as a collision and refused. This is the same router defect class already filed for other signing-name collisions — see open issues "Kinesis Analytics V2 is entirely unreachable: router refuses every request with AmbiguousService because kinesisanalytics (v1) and kinesisanalyticsv2 ..." and "Chime SDK Media Pipelines is entirely unreachable: the router resolves by sigv4 signing name ..." — but neither of those covers the 'apigateway' claimants, so this is a net-new instance.
Traces: the two checks below both die on their first apigateway call (CreateApiKey) and never reach their subject assertions.

## Evidence
Expected: CreateApiKey returns HTTP 201 with an ApiKey envelope (id/name/enabled/tags), per Smithy CreateApiKey -> ApiKey, http {method: POST, uri: /apikeys, code: 201}.
Actual: botocore.exceptions.ClientError: An error occurred (AmbiguousService) when calling the CreateApiKey operation (reached max retries: 3): 2 emulated services all claim requests for 'apigateway': apigateway, apigateway; the request is rejected rather than served by an arbitrary one.
The retry exhaustion shows the refusal is returned with a retryable 5xx-class status; AWS has no AmbiguousService error code in any API Gateway operation's error list.

## Steps to Reproduce
1. Boot the emulator and point a botocore 'apigateway' client at it.
2. Call create_api_key(name='x', enabled=True, tags={'Run':'r'}).
3. Observe the AmbiguousService refusal after three retries instead of a 201 ApiKey envelope. Any other apigateway operation (get_api_key, delete_api_key) fails the same way, because the refusal happens in the router before operation dispatch.
