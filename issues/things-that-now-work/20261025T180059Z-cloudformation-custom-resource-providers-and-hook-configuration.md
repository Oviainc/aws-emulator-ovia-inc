Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__sns__sqs.xsvc-cloudformation-3-sns-custom-resource-request-roundtrip-and-getatt-returns-response-data
Issue-Key: cloudformation__sns__sqs.xsvc-cloudformation-3-sns-custom-resource-request-roundtrip-and-getatt-returns-response-data|pull#14|Timeout|resp = self.sqs.receive_message(QueueUrl=self.queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Commit: 8fdd90c15790d38aa3c5d0088f99b03c79823121

# cloudformation: a template's custom resource is now invoked — the provider named in its `ServiceToken` is sent the request and answers at the `ResponseURL`

> _This report carries the ONE replay key that now passes. The parent report's other six
> keys do not pass and are re-filed under
> `issues/things-that-do-not-work/20261105T040000Z-cloudformation-6-of-the-7-custom-resource-and-hook-keys-remain-open.md`
> so they are not recorded as working; everything that fix established about them is there._

## Operation
CreateStack, DescribeStacks

## Description

A resource whose type is `Custom::<Name>` or `AWS::CloudFormation::CustomResource` is
provisioned by the PROVIDER its own `ServiceToken` property names. On every stack operation
that acts on such a resource, CloudFormation sends that provider a request — `RequestType`
(`Create`|`Update`|`Delete`), `ServiceToken`, `ResponseURL`, `StackId`, `RequestId`,
`ResourceType`, `LogicalResourceId`, `ResourceProperties` with every scalar as text, plus the
`PhysicalResourceId` on an update or a delete and the `OldResourceProperties` on an update —
and holds the resource, and the operation behind it, in its `*_IN_PROGRESS` status until the
provider answers at that request's pre-signed `ResponseURL`. The answer is what the resource
IS: its `PhysicalResourceId` is what `Ref` resolves to, its `Data` is what `Fn::GetAtt`
resolves against, and a `FAILED` answer fails the resource with the `Reason` it gave.

That is what the emulator now does. An SNS `ServiceToken` is published to
(`Publish`, so every subscriber of the topic reads the request as its own notification) and a
Lambda one is invoked with the request as its event; the `ResponseURL` is a URL this service
publishes and SERVES (`src/service/custom_resource_plane.rs`), followed unsigned as a
pre-signed URL is, and the answer it receives releases the resource's wait
(`src/state/stabilising.rs`) through `src/state/provisioning/custom_resource.rs`.

## Evidence
`cloudformation__sns__sqs.xsvc-cloudformation-3-sns-custom-resource-request-roundtrip-and-getatt-returns-response-data`
replays green: the subscribed queue receives the `Create` request, its `ServiceToken`,
`ResponseURL` and stringified `ResourceProperties` are the ones AWS sends, the provider's
`PUT` to the `ResponseURL` is accepted, and the stack then reports `CREATE_COMPLETE` with
`Fn::GetAtt` resolving to the answered `Data`.

Pinned by `services/service-com-amazonaws-cloudformation-cloudformation/tests/all/custom_resource_providers.rs`
(the request is sent and the resource held; the answer's id and `Data` become what `Ref` and
`Fn::GetAtt` resolve to; a `FAILED` answer fails it; an `Update` answered with a new
`PhysicalResourceId` is a replacement whose old resource is cleaned up; a stack deletion's
`Delete`; and the response plane's own refusals).

## Steps to Reproduce
Create an SNS topic, subscribe an SQS queue to it, `CreateStack` with
`{"Resources":{"Thing":{"Type":"Custom::Probe","Properties":{"ServiceToken":"<topic ARN>","Marker":"v1"}}}}`,
then `ReceiveMessage` on the queue: the provider request arrives, and PUTting
`{"Status":"SUCCESS","PhysicalResourceId":"…","Data":{…}}` to its `ResponseURL` settles the
stack with that id and that data.
