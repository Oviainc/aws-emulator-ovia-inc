Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__mediapackage.deploy-channel-and-origin-endpoint-round-trip-and-delete cloudformation__mediapackage.deploy-origin-endpoint-invalid-channel-id-rejected-and-rolled-back
Issue-Key: cloudformation__mediapackage.deploy-channel-and-origin-endpoint-round-trip-and-delete|stack_outputs#9|KeyError|return {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Key: cloudformation__mediapackage.deploy-origin-endpoint-invalid-channel-id-rejected-and-rolled-back|run#8|AssertionError|assert status == "ROLLBACK_COMPLETE", (
Issue-Commit: d92c3cc324fbe4ad920dfad809c989914fc83363

# cloudformation__mediapackage: 2 net-new causes — stack Outputs for AWS::MediaPackage::* resources carry no OutputValue, and an OriginEndpoint naming a missing channel reaches CREATE_COMPLETE instead of rolling back

> _Note: this issue was identified against the code base at commit `d92c3cc324fbe4ad920dfad809c989914fc83363`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStacks

## Description
Behavioral replay of the CloudFormation + MediaPackage deploy checks (suite heading `### cloudformation__mediapackage`; the defects below are in the CloudFormation stack engine and its AWS::MediaPackage::* resource handlers, so the issue is filed under `cloudformation`) shows TWO independent net-new root causes. A fixer MUST close both causes and re-replay both checks listed in trace_ids before submitting.

Related but NOT covering: 20261109T141301Z-cloudformation-s3tables-cloudformation-stack-outputs-for-aws-s3tables-resources-are.md, 20261109T143402Z-cloudformation-mq-stack-outputs-for-aws-amazonmq-configuration.md, 20261109T154607Z-cloudformation-signer-2-net-new-causes-signer-stack.md and 20261109T144630Z-cloudformation-elasticbeanstalk-stack-outputs-omit-outputvalue-a-stack.md report the same OutputValue-omission shape for OTHER resource namespaces (S3Tables, AmazonMQ, Signer/Lambda, Elastic Beanstalk) and own only those replay keys; none of them names MediaPackage resources or either check id here. 20261109T142012Z-cloudformation-mediapackage-vod-aws-mediapackage-packaginggroup-and-aws.md concerns the unregistered AWS::MediaPackage::PackagingGroup/PackagingConfiguration types of MediaPackage VOD — a different resource set; Channel and OriginEndpoint ARE recognized here (the stack reaches CREATE_COMPLETE), so it does not cover this.

Cause 1 of 2: DescribeStacks returns Outputs entries for AWS::MediaPackage::Channel / AWS::MediaPackage::OriginEndpoint with an OutputKey but no OutputValue, so Ref and Fn::GetAtt on those resources resolve to nothing.
  AWS: Smithy com.amazonaws.cloudformation#Output has members OutputKey ("The key associated with the output") and OutputValue ("The value associated with the output"); DescribeStacks returns Stack.Outputs as a list of those structures. Per the AWS::MediaPackage::Channel and AWS::MediaPackage::OriginEndpoint template references, Ref returns the resource Id and Fn::GetAtt supports Arn (both types) and Url (OriginEndpoint). A CREATE_COMPLETE stack that declares Outputs built from {"Ref": "Channel"}, {"Fn::GetAtt": ["Channel", "Arn"]}, {"Ref": "Endpoint"}, {"Fn::GetAtt": ["Endpoint", "Arn"]} and {"Fn::GetAtt": ["Endpoint", "Url"]} therefore has, for EVERY declared output, both OutputKey and the resolved OutputValue — the channel/endpoint Id, the ARN MediaPackage's own DescribeChannel/DescribeOriginEndpoint reports, and the endpoint playback Url.
  Emulator: the stack reaches CREATE_COMPLETE, DescribeStacks reports Outputs rows, but the rows carry no OutputValue at all: reading `o["OutputValue"]` over `Stacks[0]["Outputs"]` raises KeyError: 'OutputValue'. The outputs are emitted as key-only entries, i.e. intrinsic resolution against the MediaPackage resources produced no value.
  Why: a client cannot obtain any identifier of what the stack deployed. Every downstream consumer of a deploy — a nested stack importing an export, a pipeline reading DescribeStacks outputs to find the channel ARN or the endpoint playback URL, cross-stack Fn::ImportValue — gets an output row with no value while the stack claims success, so the deploy silently produces nothing usable instead of failing loudly.
  Fix: the CloudFormation stack engine's Outputs resolution (intrinsic Ref/Fn::GetAtt evaluation against provisioned resource attributes) and the AWS::MediaPackage::Channel / ::OriginEndpoint resource handlers that must publish the Ref identity and the Arn/Url attributes, plus the DescribeStacks response serializer which should never emit an Output without OutputValue.
  Traces: cloudformation__mediapackage.deploy-channel-and-origin-endpoint-round-trip-and-delete (fails at stack_outputs() with KeyError: 'OutputValue' immediately after the stack reached CREATE_COMPLETE).

Cause 2 of 2: an AWS::MediaPackage::OriginEndpoint whose ChannelId names no existing channel is provisioned successfully — the stack ends CREATE_COMPLETE instead of failing the resource and rolling back.
  AWS: the OriginEndpoint handler calls MediaPackage CreateOriginEndpoint, which refuses an unknown ChannelId (NotFoundException, "channel with id=<channelId> not found" per the service's own messages). CloudFormation therefore records the resource as CREATE_FAILED and rolls the stack back to ROLLBACK_COMPLETE, and the DescribeStackEvents ResourceStatusReason for the logical resource is CloudFormation's handler-wrapped NotFound: fixed text `Resource handler returned message: "Resource of type 'AWS::MediaPackage::Channel' with identifier '<channel-id>' was not found." (RequestToken: <request token>, HandlerErrorCode: NotFound)` — the fixed parts are that wrapper, the type name `AWS::MediaPackage::Channel`, the words `with identifier`/`was not found.` and `HandlerErrorCode: NotFound`; the variable parts are the ChannelId taken from the template and the per-run RequestToken. No origin endpoint exists afterwards (DescribeOriginEndpoint answers NotFoundException, "originEndpoint with id=<id> not found").
  Emulator: the bad stack ends CREATE_COMPLETE (replay: "stack 'sim2real-mpepbad-...' ended 'CREATE_COMPLETE', expected ROLLBACK_COMPLETE"). The dangling ChannelId is never validated, so the resource is stub/unvalidated-provisioned, no CREATE_FAILED event with a NotFound handler reason is recorded, and no rollback happens.
  Why: a client gets a green deploy for a template that AWS rejects. The stack reports resources that cannot work (an endpoint attached to no channel), the failure surfaces only later at playback time, and CI that gates on stack status passes a template that can never deploy on real AWS. It also means CFN never surfaces referential errors for MediaPackage at all.
  Fix: the AWS::MediaPackage::OriginEndpoint resource handler — it must perform the real MediaPackage CreateOriginEndpoint (and propagate its NotFoundException) rather than synthesizing success — plus the stack engine's failure/rollback path that converts a handler error into a CREATE_FAILED event with the `Resource handler returned message: "..." (RequestToken: ..., HandlerErrorCode: NotFound)` reason and drives the stack to ROLLBACK_COMPLETE.
  Traces: cloudformation__mediapackage.deploy-origin-endpoint-invalid-channel-id-rejected-and-rolled-back.

## Evidence
Suite: simparity-behavioral-aws-v1, 52/60 passed; per-service line `cloudformation__mediapackage 0/2 (0%) 2 failed`. Neither failing check was marked ALREADY FILED or OWNED by.

Cause 1 — [error] cloudformation__mediapackage.deploy-channel-and-origin-endpoint-round-trip-and-delete:
  File "check_...round_trip_and_delete.py", line 203, in run -> outputs = stack_outputs(cfn, stack_id)
  File "...", line 111, in stack_outputs -> return {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
  KeyError: 'OutputValue'
  The template declares Resources Channel (AWS::MediaPackage::Channel, Id+Description) and Endpoint (AWS::MediaPackage::OriginEndpoint, Id, ChannelId={"Ref":"Channel"}, ManifestName "index", HlsPackage.PlaylistType "EVENT", Whitelist ["192.0.2.0/24"]) and Outputs ChannelId={"Ref":"Channel"}, ChannelArn={"Fn::GetAtt":["Channel","Arn"]}, EndpointId={"Ref":"Endpoint"}, EndpointArn={"Fn::GetAtt":["Endpoint","Arn"]}, EndpointUrl={"Fn::GetAtt":["Endpoint","Url"]}. The stack reached CREATE_COMPLETE (the preceding assertion passed), so the engine considers the resources provisioned; the Outputs rows nonetheless have no OutputValue member. Expected: every row carries OutputValue, and ChannelId/EndpointId equal the declared Ids while ChannelArn/EndpointArn/EndpointUrl equal what MediaPackage's DescribeChannel/DescribeOriginEndpoint report.

Cause 2 — [mismatch] cloudformation__mediapackage.deploy-origin-endpoint-invalid-channel-id-rejected-and-rolled-back:
  File "check_...invalid_channel_id_rejected_and_rolled_back.py", line 233, in run -> assert status == "ROLLBACK_COMPLETE"
  AssertionError: stack 'sim2real-mpepbad-786d1887cca7' ended 'CREATE_COMPLETE', expected ROLLBACK_COMPLETE
  The template contains ONLY an AWS::MediaPackage::OriginEndpoint whose ChannelId is a channel id that was never created. Expected terminal status ROLLBACK_COMPLETE, with the Endpoint resource's CREATE_FAILED ResourceStatusReason parsing (per the check's _HANDLER_REASON regex `^Resource handler returned message: "(?P<message>.*)" \(RequestToken: [0-9a-f-]+, HandlerErrorCode: (?P<code>[A-Za-z]+)\)$`) to message "Resource of type 'AWS::MediaPackage::Channel' with identifier '<missing-channel-id>' was not found." and code "NotFound", and MediaPackage DescribeOriginEndpoint then answering NotFoundException "originEndpoint with id=<endpoint-id> not found". The quoted stack name/ids are one run's generated values and are not part of the requirement.

Model evidence: smithy_query(query="shape", shape="Output,Stack", model_path="services/service-com-amazonaws-cloudformation-cloudformation/smithy-model/com-amazonaws-cloudformation-cloudformation.json") — Output.OutputKey/OutputValue as documented above, Stack.Outputs a list of Output. The resource-type attribute sets (Ref, Arn, Url) and the handler-error wrapper text are AWS-documented behavior / recorded behavior only; the per-run RequestToken is never part of the requirement.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the simparity-behavioral-aws-v1 suite; both cloudformation__mediapackage checks fail as above.
2. Cause 1, directly with boto3 against the emulator (region us-west-2):
   a. cfn.create_stack(StackName="s1", TemplateBody=<the template above: Channel + OriginEndpoint, Outputs ChannelId/ChannelArn/EndpointId/EndpointArn/EndpointUrl>).
   b. Poll cfn.describe_stacks(StackName=<StackId>) until CREATE_COMPLETE, then read Stacks[0]["Outputs"].
   c. Observed: rows without OutputValue (KeyError on o["OutputValue"]). Expected: all five keys present with values equal to the declared Ids and to MediaPackage's DescribeChannel/DescribeOriginEndpoint Arn and Url.
3. Cause 2:
   a. cfn.create_stack(StackName="s2", TemplateBody=<only an AWS::MediaPackage::OriginEndpoint with ChannelId set to a channel id that does not exist>).
   b. Poll describe_stacks until terminal. Observed CREATE_COMPLETE; expected ROLLBACK_COMPLETE.
   c. cfn.describe_stack_events(StackName=<StackId>) and look for the Endpoint resource's CREATE_FAILED row: expected ResourceStatusReason `Resource handler returned message: "Resource of type 'AWS::MediaPackage::Channel' with identifier '<channel-id>' was not found." (RequestToken: <uuid>, HandlerErrorCode: NotFound)`; none is recorded.
   d. mp.describe_origin_endpoint(Id=<endpoint id>) must raise NotFoundException "originEndpoint with id=<endpoint-id> not found".
4. Pass criterion: every declared Output comes back with a resolved OutputValue, and a template whose OriginEndpoint ChannelId names no channel fails that resource with the NotFound handler reason and leaves the stack ROLLBACK_COMPLETE with nothing created in MediaPackage.
