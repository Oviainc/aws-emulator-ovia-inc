Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__kinesis.aws-proxy-subtype-kinesis-putrecord-required-parameters-refused-else-lands-mapped-data-and-key
Issue-Key: apigatewayv2__kinesis.aws-proxy-subtype-kinesis-putrecord-required-parameters-refused-else-lands-mapped-data-and-key|_error#13|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Commit: 39b863d101414613449d16e44f0b23fd775a8d87

# apigatewayv2__kinesis: CreateIntegration accepts a Kinesis-PutRecord AWS_PROXY subtype with no required parameters mapped instead of refusing it with BadRequestException

> _Note: this issue was identified against the code base at commit `39b863d101414613449d16e44f0b23fd775a8d87`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateIntegration

## Description
Cause 1 of 1: an AWS_PROXY integration declaring IntegrationSubtype 'Kinesis-PutRecord' with an empty RequestParameters map is created (201) instead of being refused, so the subtype's required-parameter contract is not enforced.

AWS: Smithy (service-com-amazonaws-apigatewayv2-apigatewayv2): CreateIntegration — POST /v2/apis/{ApiId}/integrations, 201; members IntegrationType (required), IntegrationSubtype (StringWithLengthBetween1And128), RequestParameters : IntegrationParameters; modeled errors BadRequestException (400), ConflictException (409), NotFoundException (404), TooManyRequestsException (429). For a first-class AWS service integration subtype, API Gateway validates at create time that every parameter the subtype requires is present in the parameter mapping (or that passthrough is enabled) and otherwise answers 400 BadRequestException with the message "Operation: <IntegrationSubtype> requires enabling passthrough, or defining all of the following parameters in the parameter mapping: [<RequiredParam>, ...]" — the sentence is fixed text, while the subtype name and the bracketed list of missing required parameters come from the request and the subtype's definition (for Kinesis-PutRecord: StreamName, Data, PartitionKey).
Emulator: the call returns HTTP 201 with the integration created (IntegrationSubtype 'Kinesis-PutRecord', RequestParameters {}), raising no error.
Why: a misconfigured integration that AWS refuses at create time is accepted here and then fails only at invoke time (or silently drops the record), so a client loses the deploy-time guardrail and the deployment behaves differently in production.
Fix: the AWS_PROXY subtype parameter validation in the apigatewayv2 crate's CreateIntegration/UpdateIntegration handler — the per-subtype required-parameter table and the BadRequestException it must raise.
Traces: apigatewayv2__kinesis.aws-proxy-subtype-kinesis-putrecord-required-parameters-refused-else-lands-mapped-data-and-key.

## Evidence
Expected: CreateIntegration(IntegrationType='AWS_PROXY', IntegrationSubtype='Kinesis-PutRecord', PayloadFormatVersion='1.0', CredentialsArn=<role>, RequestParameters={}) -> 400 BadRequestException, message "Operation: Kinesis-PutRecord requires enabling passthrough, or defining all of the following parameters in the parameter mapping: [StreamName, Data, PartitionKey]". Actual: HTTP 201 and the integration created — "expected the call to be rejected, but it succeeded: {... 'IntegrationSubtype': 'Kinesis-PutRecord', 'IntegrationType': 'AWS_PROXY', 'RequestParameters': {}, ...}".

## Steps to Reproduce
1. Create a Kinesis stream and an IAM role granting kinesis:PutRecord.
2. CreateApi(ProtocolType='HTTP'), then CreateIntegration with IntegrationType='AWS_PROXY', IntegrationSubtype='Kinesis-PutRecord', PayloadFormatVersion='1.0', CredentialsArn=<role>, RequestParameters={} — it returns 201 instead of 400 BadRequestException.
3. Replay apigatewayv2__kinesis.aws-proxy-subtype-kinesis-putrecord-required-parameters-refused-else-lands-mapped-data-and-key.
