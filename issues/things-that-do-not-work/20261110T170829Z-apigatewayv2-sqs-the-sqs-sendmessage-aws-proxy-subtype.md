Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__sqs.aws-proxy-subtype-sqs-sendmessage-delivers-to-the-queue
Issue-Key: apigatewayv2__sqs.aws-proxy-subtype-sqs-sendmessage-delivers-to-the-queue|run#28|AssertionError|assert response_message_id == delivered[0]["MessageId"], (
Issue-Commit: 6cc05b66b5dc4f3acc804c4a889717eb31d245db

# apigatewayv2__sqs: the SQS-SendMessage AWS_PROXY subtype answers a JSON body instead of passing SQS's SendMessageResponse XML through to the client

> _Note: this issue was identified against the code base at commit `6cc05b66b5dc4f3acc804c4a889717eb31d245db`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateIntegration

## Description
Cause 1 of 1: a route with IntegrationSubtype='SQS-SendMessage' enqueues the mapped body correctly but returns a JSON object instead of the SQS response the client must receive verbatim, so the client cannot read the MessageId of the message it just sent.

AWS: Smithy (service-com-amazonaws-apigatewayv2-apigatewayv2) CreateIntegration.IntegrationType -> AWS_PROXY with IntegrationSubtype (StringWithLengthBetween1And128) and RequestParameters -> IntegrationParameters (here MessageBody='$request.body.msg'). For a first-class service subtype the target service's own response is passed through to the client verbatim; for SQS-SendMessage that is SQS's query-protocol XML envelope SendMessageResponse/SendMessageResult/MessageId (namespaced), from which the client parses the MessageId — and that MessageId is the one a consumer then reads off the queue (recorded behavior for the pass-through shape; the gateway's own refusals, by contrast, use its JSON {"message": ...} envelope).
Emulator: the response body is JSON — '{"MD5OfMessageBody":"<md5>","MessageId":"<uuid>"}' — so XML parsing of SendMessageResponse yields no MessageId (None) even though the message itself was enqueued with that id.
Why: a client of the integration cannot correlate its send with the queued message: parsing the documented XML envelope returns nothing, and any code written against AWS's pass-through response (or an SDK-style parser) breaks on a body of a different media type and shape.
Fix: the response serialization of the AWS_PROXY service-subtype dispatch in the apigatewayv2 execute-api data-plane handler — return the target service's own wire response (SQS query-protocol XML for SendMessage, with its content type) rather than re-encoding it as JSON.
Traces: apigatewayv2__sqs.aws-proxy-subtype-sqs-sendmessage-delivers-to-the-queue.

## Evidence
Expected: the 200 response body is SQS's query-protocol XML (SendMessageResponse/SendMessageResult/MessageId) and the MessageId parsed from it equals the MessageId the consumer reads off the queue. Actual: AssertionError "the MessageId the client received must be the MessageId the consumer reads off the queue: response None vs queue '<uuid>' (response body was '{\"MD5OfMessageBody\":\"<md5>\",\"MessageId\":\"<uuid>\"}')" — the body is JSON, so the XML MessageId element is absent.

## Steps to Reproduce
1. Create an SQS queue and an IAM role API Gateway can assume for sqs:SendMessage.
2. CreateApi(ProtocolType='HTTP'); CreateIntegration(IntegrationType='AWS_PROXY', IntegrationSubtype='SQS-SendMessage', CredentialsArn=<role>, RequestParameters={'QueueUrl': <url>, 'MessageBody': '$request.body.msg'}, PayloadFormatVersion='1.0'); CreateRoute('POST /send') and deploy.
3. POST {"msg": "<marker>"} to the route and inspect the 200 response body — it is JSON, not SQS's SendMessageResponse XML.
4. ReceiveMessage from the queue and compare its MessageId with the one parsed from the XML envelope (none is parseable).
5. Replay apigatewayv2__sqs.aws-proxy-subtype-sqs-sendmessage-delivers-to-the-queue.
