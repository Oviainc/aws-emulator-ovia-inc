Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: firehose.data-format-conversion-configuration-echoed-then-disable-preserves-schema-members firehose.extended-s3-destination-configuration-defaults firehose.processing-configuration-parameter-normalization-and-update-replaces-block firehose.tag-delivery-stream-disallowed-key-character-rejected firehose.tag-reserved-aws-prefix-rejected firehose.untag-delivery-stream-invalid-tag-key firehose.start-encryption-invalid-kms-key firehose.update-destination-immutable-delivery-stream-type firehose.update-destination-type-change-requires-full-config firehose.deploy-cfn-firehose-stream-becomes-active firehose.deploy-cfn-stream-tags-and-encryption-applied firehose.create-kinesis-source-type-without-source-config firehose.delete-stream-while-update-in-progress-resource-in-use firehose.e2e-update-destination-stale-version-rejected firehose.start-encryption-invalid-key-type-combo firehose.start-encryption-nonexistent-stream-not-found firehose.start-encryption-while-stream-creating-rejected firehose.start-encryption-while-stream-deleting-rejected firehose.stop-encryption-nonexistent-stream-not-found firehose.stop-encryption-while-update-in-progress-resource-in-use firehose.update-destination-change-type-rejected firehose.update-destination-invalid-destination-id firehose.update-destination-rejected-while-stream-creating firehose.update-destination-stale-version-conflict firehose.update-destination-stale-version-id-rejected firehose.update-destination-unknown-destination-id
Issue-Shape-Id: com.amazonaws.firehose#Firehose_20150804
Issue-Key: firehose.data-format-conversion-configuration-echoed-then-disable-preserves-schema-members|run#30|KeyError|assert conversion2["SchemaConfiguration"]["DatabaseName"] == database, (
Issue-Key: firehose.extended-s3-destination-configuration-defaults|run#14|KeyError|assert ext["CloudWatchLoggingOptions"]["Enabled"] is False, (
Issue-Key: firehose.processing-configuration-parameter-normalization-and-update-replaces-block|run#13|AssertionError|assert names == ["LambdaArn", "NumberOfRetries", "RoleArn", "BufferSizeInMBs", "BufferIntervalInSeconds"], (
Issue-Key: firehose.tag-delivery-stream-disallowed-key-character-rejected|_error#5|AssertionError|raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: firehose.tag-reserved-aws-prefix-rejected|_err#3|AssertionError|raise AssertionError(f"expected a ClientError, the call succeeded with {result!r}")
Issue-Key: firehose.untag-delivery-stream-invalid-tag-key|run#13|AssertionError|raise AssertionError("expected rejection of a reserved 'aws:' tag key")
Issue-Key: firehose.start-encryption-invalid-kms-key|run#9|AssertionError|raise AssertionError("expected InvalidKMSResourceException for an unusable KMS key")
Issue-Key: firehose.update-destination-immutable-delivery-stream-type|run#13|AssertionError|assert err.get("Message") == expected_msg, (
Issue-Key: firehose.update-destination-type-change-requires-full-config|run#16|AssertionError|assert err.get("Message") == expected_msg, (
Issue-Key: firehose.deploy-cfn-firehose-stream-becomes-active|_deploy_cfn#4|KeyError|outputs[o["OutputKey"]] = o["OutputValue"]
Issue-Key: firehose|ListTagsForDeliveryStream|ResourceNotFoundException
Issue-Key: firehose.create-kinesis-source-type-without-source-config|run#6|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: firehose.delete-stream-while-update-in-progress-resource-in-use|run#12|AssertionError|assert update_message == expected, f"expected {expected!r}, got {update_message!r}"
Issue-Key: firehose.e2e-update-destination-stale-version-rejected|run#23|AssertionError|assert msg == expected_msg, f"expected message {expected_msg!r}, got {msg!r}"
Issue-Key: firehose.start-encryption-invalid-key-type-combo|run#6|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: firehose.start-encryption-nonexistent-stream-not-found|run#6|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: firehose.start-encryption-while-stream-creating-rejected|run#16|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: firehose.start-encryption-while-stream-deleting-rejected|run#12|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: firehose.stop-encryption-nonexistent-stream-not-found|run#6|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: firehose.stop-encryption-while-update-in-progress-resource-in-use|run#11|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: firehose.update-destination-change-type-rejected|run#19|AssertionError|assert message == expected_msg, f"expected message {expected_msg!r}, got {message!r}"
Issue-Key: firehose|UpdateDestination|InvalidArgumentException
Issue-Key: firehose.update-destination-rejected-while-stream-creating|run#14|AssertionError|assert msg == expected, f"expected message {expected!r}, got {msg!r}"
Issue-Key: firehose.update-destination-stale-version-conflict|run#13|AssertionError|assert err.get("Message") == expected_msg, (
Issue-Key: firehose.update-destination-stale-version-id-rejected|run#16|AssertionError|assert err.get("Message") == expected_msg, (
Issue-Key: firehose.update-destination-unknown-destination-id|run#14|AssertionError|assert err.get("Message") == expected_msg, (
Issue-Commit: 17f16131651e3a76635c083c093a8581359ec045

# Firehose control plane: configuration echo drops members, tag-key and KMS validation absent, destination-type-change misclassified, CFN deploys are inert, and rejection messages are re-worded (9 root causes)

> _Note: this issue was identified against the code base at commit `17f16131651e3a76635c083c093a8581359ec045`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDeliveryStream

## Description
Nine net-new root causes behind the `firehose.*` behavioral failures that are not marked ALREADY FILED or OWNED by an open issue. Causes 7-8 are the CloudFormation provisioning gap seen from Firehose's side; the open issue 20261022T064119Z-firehose-cloudformation-deployed-firehose-delivery-streams-are-never-provisioned covers the same family for other check ids but these two check ids are not on its replay list. Cause 9 is the message-template family; the open issue 20261022T064119Z-firehose-createdeliverystream-putrecordbatch-are-overly-permissive-and-misclassify owns the CreateDeliveryStream/PutRecordBatch permissiveness checks and none of the checks listed here.

Cause 1 of 9: an UpdateDestination that only sets DataFormatConversionConfiguration.Enabled=false WIPES the stored conversion block instead of preserving it.
AWS: Smithy DataFormatConversionConfiguration.Enabled is documented "Defaults to true. Set it to false if you want to disable format conversion while preserving the configuration details", so after such an update DescribeDeliveryStream still returns DataFormatConversionConfiguration with Enabled=false plus the previously stored SchemaConfiguration (DatabaseName/TableName/RoleARN/Region/VersionId), InputFormatConfiguration and OutputFormatConfiguration (ParquetSerDe).
Emulator: the second DescribeDeliveryStream returns a DataFormatConversionConfiguration that has no SchemaConfiguration key at all (KeyError: 'SchemaConfiguration'), i.e. the sub-structures are dropped when conversion is disabled.
Why: a client that disables conversion temporarily loses its Glue schema binding and cannot re-enable it without re-sending the whole block; it also cannot read back what schema a stream is bound to.
Fix: the UpdateDestination merge of ExtendedS3DestinationUpdate into stored destination state, and the ExtendedS3DestinationDescription serializer, in the Firehose service crate (services/service-com-amazonaws-firehose-firehose-20150804/src/ops/update_destination.rs and the describe path).
Traces: firehose.data-format-conversion-configuration-echoed-then-disable-preserves-schema-members.

Cause 2 of 9: ExtendedS3DestinationDescription omits the defaulted CloudWatchLoggingOptions block.
AWS: recorded behavior only — after CreateDeliveryStream with an ExtendedS3DestinationConfiguration that does not mention CloudWatchLoggingOptions, DescribeDeliveryStream returns ExtendedS3DestinationDescription.CloudWatchLoggingOptions with Enabled=false. Smithy models CloudWatchLoggingOptions as a member of ExtendedS3DestinationDescription.
Emulator: the member is absent from the description (KeyError: 'CloudWatchLoggingOptions').
Why: a client reading back the effective destination configuration cannot tell whether CloudWatch error logging is on, and code that dereferences the documented default block raises instead of reading False.
Fix: destination-description defaulting/serialization for the extended S3 destination in the Firehose service crate.
Traces: firehose.extended-s3-destination-configuration-defaults.

Cause 3 of 9: processor parameters are echoed in submission order instead of the service's canonical parameter order.
AWS: recorded behavior only — Firehose normalizes ProcessingConfiguration.Processors[].Parameters into its own fixed order (LambdaArn, NumberOfRetries, RoleArn, BufferSizeInMBs, BufferIntervalInSeconds), which follows the Smithy ProcessorParameterName enum order (LambdaArn, NumberOfRetries, MetadataExtractionQuery, JsonParsingEngine, RoleArn, BufferSizeInMBs, BufferIntervalInSeconds, ...).
Emulator: the list comes back in the order the caller sent it — observed ['BufferIntervalInSeconds', 'BufferSizeInMBs', 'LambdaArn'].
Why: clients and IaC tools that diff the described configuration against their declared configuration see a permanent spurious drift and keep re-applying updates.
Fix: ProcessingConfiguration normalization on the create/update path and the Describe serializer in the Firehose service crate.
Traces: firehose.processing-configuration-parameter-normalization-and-update-replaces-block.

Cause 4 of 9: tag-key validation is not enforced by TagDeliveryStream or UntagDeliveryStream.
AWS: Smithy Tag.Key (and TagKeyList's TagKey, used by UntagDeliveryStream.TagKeys) is length 1-128 with pattern ^(?!aws:)[\p{L}\p{Z}\p{N}_.:\/=+\-@%]*$, so a key beginning with the reserved prefix 'aws:' or containing a character outside that class is refused with HTTP 400 (TagDeliveryStream/UntagDeliveryStream model InvalidArgumentException; AWS answers a 400 ValidationException-class refusal for the pattern violation).
Emulator: all three calls return HTTP 200 with an empty body — the reserved prefix and the disallowed character are accepted and the tag is stored.
Why: a client can create 'aws:'-namespaced tags that real AWS reserves, and tag-validation error paths (the usual place a caller sanitizes user input) never fire, so tagging code that works against the emulator fails against AWS.
Fix: input validation for the tag-key shape in the TagDeliveryStream and UntagDeliveryStream handlers of the Firehose service crate.
Traces: firehose.tag-delivery-stream-disallowed-key-character-rejected, firehose.tag-reserved-aws-prefix-rejected, firehose.untag-delivery-stream-invalid-tag-key.

Cause 5 of 9: StartDeliveryStreamEncryption accepts an unusable KMS key instead of refusing it.
AWS: Smithy StartDeliveryStreamEncryption models InvalidKMSResourceException (client, HTTP 400) and AWS raises it when the supplied KeyARN cannot be used (nonexistent/unusable key).
Emulator: the call is not refused with InvalidKMSResourceException (assertion 'expected InvalidKMSResourceException for an unusable KMS key'), so encryption is reported as enabled against a key that does not exist.
Why: a client never learns that its CMK is wrong; the stream looks encrypted with a key that could never wrap data, and the modeled error path is untestable.
Fix: KMS key resolution/validation in the StartDeliveryStreamEncryption handler of the Firehose service crate.
Traces: firehose.start-encryption-invalid-kms-key.

Cause 6 of 9: UpdateDestination refuses a destination-TYPE change for the wrong reason.
AWS: recorded behavior only — changing a stream's destination type to or from HttpEndpoint is refused outright, 400 with the message 'Changing the destination type to or from HttpEndpoint is not supported at this time.'; the refusal does not depend on which sub-members the update carries.
Emulator: the call is refused with a different RULE — it complains that 'HttpEndpointDestinationUpdate.S3Update must be specified when the destination type changes', i.e. it treats the type change as permissible once enough members are supplied rather than as unsupported.
Why: a client that adds S3Update (exactly what the emulator's message instructs) is accepted by the emulator and rejected by AWS, so the emulator teaches a migration path that does not exist.
Fix: the destination-type-change guard in the UpdateDestination handler of the Firehose service crate.
Traces: firehose.update-destination-immutable-delivery-stream-type, firehose.update-destination-type-change-requires-full-config.

Cause 7 of 9: a CloudFormation stack's Outputs are returned without OutputValue.
AWS: DescribeStacks on a CREATE_COMPLETE stack returns each Output with both OutputKey and OutputValue (the resolved Ref/GetAtt, e.g. the delivery stream name or ARN).
Emulator: the Output entries have no OutputValue key at all (KeyError: 'OutputValue'), so nothing deployed by a template can be addressed afterwards.
Why: every deploy-then-use workflow breaks at its first step — the client cannot discover the physical name/ARN of the resource it just deployed.
Fix: Output resolution/serialization in the CloudFormation service crate's stack handling (services/service-com-amazonaws-cloudformation-cloudformation/src/ops/describe_stacks.rs and the create-stack output resolution).
Traces: firehose.deploy-cfn-firehose-stream-becomes-active.

Cause 8 of 9: a CFN-declared AWS::KinesisFirehose::DeliveryStream is not created in Firehose, so Firehose APIs do not see it.
AWS: CloudFormation creates the stream through Firehose; after CREATE_COMPLETE, ListTagsForDeliveryStream / DescribeDeliveryStream on the stack's stream return 200 with the template's tags and encryption configuration.
Emulator: ListTagsForDeliveryStream answers 400 ResourceNotFoundException (example message: 'Firehose under account 000000000000 not found.') — no stream exists in the Firehose service at all.
Why: tags, encryption and every other property declared in a template are unverifiable and unusable; CFN-based provisioning of Firehose is a no-op from the consuming service's side.
Fix: stack-resource provisioning for AWS::KinesisFirehose::DeliveryStream must materialise the stream through the Firehose service's state (CloudFormation create_stack resource dispatch).
Traces: firehose.deploy-cfn-stream-tags-and-encryption-applied.

Cause 9 of 9: control-plane rejection MESSAGES are re-synthesized instead of using Firehose's recorded templates, while status and error code match.
AWS: recorded behavior only — these refusals use fixed, request-parameterized templates, repeated identically across several recordings: 'Firehose under accountId <account> not found'; 'Cannot enable encryption on firehose: <name> when it is in <STATUS> status'; 'Cannot disable encryption on firehose: <name> when encryption is in <STATUS> status'; 'Cannot update firehose: <name> when it is in <STATUS> status'; 'Cannot update firehose: <name> since the current version id: <X> and specified version id: <Y> do not match'; 'Destination Id <id> not found'; 'KeyARN has to be specified when KeyType is CUSTOMER_MANAGED_CMK'; 'KinesisSourceStreamConfig is mandatory for KinesisStreamAsSource stream type.'; 'Changing the destination type to/from Elasticsearch/Amazonopensearchservice/AmazonOpenSearchServerless from/to other destinations is not supported at this time.'
Emulator: the same conditions are detected and answered with the same HTTP status and error code, but with freshly worded prose (examples: 'Firehose stream <name> not found'; 'Firehose stream <name> is CREATING and server-side encryption cannot be changed until it is ACTIVE'; 'Cannot update Firehose stream <name>: the current version id is 2 and the specified version id is 1'; 'destination id <id> is not a destination of Firehose stream <name>'; 'KeyARN is required when KeyType is CUSTOMER_MANAGED_CMK').
Why: clients and test suites that match on Firehose's message text (and operators reading logs) see a different sentence for every one of these rejections; the account-scoped not-found wording in particular carries different information (account vs stream name).
Fix: centralise the Firehose error-message templates next to the error constructors in the Firehose service crate so each rejection renders AWS's template from the request parameters.
Traces: the 15 checks listed in trace_ids below whose only divergence is message text.

## Evidence
Cause 1 — expected DescribeDeliveryStream[...]['DataFormatConversionConfiguration']['SchemaConfiguration']['DatabaseName'] == the Glue database after update_destination(ExtendedS3DestinationUpdate={'DataFormatConversionConfiguration': {'Enabled': False}}); actual KeyError: 'SchemaConfiguration'.
Cause 2 — expected ExtendedS3DestinationDescription['CloudWatchLoggingOptions']['Enabled'] is False; actual KeyError: 'CloudWatchLoggingOptions'.
Cause 3 — expected parameter order (LambdaArn, NumberOfRetries, RoleArn, BufferSizeInMBs, BufferIntervalInSeconds); actual ['BufferIntervalInSeconds', 'BufferSizeInMBs', 'LambdaArn'].
Cause 4 — expected 400 rejection; actual HTTP 200 {'ResponseMetadata': {'HTTPStatusCode': 200, ... 'content-length': '2'}} for a tag key with a disallowed character, for a reserved 'aws:' key on TagDeliveryStream, and for a reserved 'aws:' key on UntagDeliveryStream.
Cause 5 — expected 400 InvalidKMSResourceException; actual no error ('expected InvalidKMSResourceException for an unusable KMS key').
Cause 6 — expected 400 'Changing the destination type to or from HttpEndpoint is not supported at this time.'; actual 400 'HttpEndpointDestinationUpdate.S3Update must be specified when the destination type changes, because no parameter is merged in that case' (2 checks).
Cause 7 — expected Outputs entries with OutputValue; actual KeyError: 'OutputValue'.
Cause 8 — expected 200 from ListTagsForDeliveryStream on the stack's stream; actual botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the ListTagsForDeliveryStream operation: Firehose under account 000000000000 not found.
Cause 9 — same code, different message, e.g. expected 'Firehose under accountId 000000000000 not found', got 'Firehose stream sim2real-fh-missing-38a9539b4941 not found'; expected 'Cannot update firehose: <name> since the current version id: 2 and specified version id: 1 do not match', got 'Cannot update Firehose stream <name>: the current version id is 2 and the specified version id is 1'; expected enum list [ZIP, ZSTD, HADOOP_SNAPPY, Snappy, GZIP, UNCOMPRESSED] in the compressionFormat validation sentence, got [UNCOMPRESSED, GZIP, ZIP, Snappy, HADOOP_SNAPPY].

## Steps to Reproduce
1. Boot the emulator and point botocore at it; fh = client('firehose').
2. Cause 1: create a stream with ExtendedS3DestinationConfiguration.DataFormatConversionConfiguration (Enabled=true, SchemaConfiguration -> a Glue table, OpenXJsonSerDe in, ParquetSerDe out); describe it; then update_destination with ExtendedS3DestinationUpdate={'DataFormatConversionConfiguration': {'Enabled': False}} and describe again -> expect Enabled False with SchemaConfiguration/OutputFormatConfiguration preserved; observe the sub-structures gone.
3. Cause 2: create a stream with an ExtendedS3DestinationConfiguration that omits CloudWatchLoggingOptions; describe -> expect CloudWatchLoggingOptions.Enabled False; observe the member missing.
4. Cause 3: create/update a stream with a Lambda processor whose Parameters are supplied out of order; describe -> expect the canonical order; observe submission order.
5. Cause 4: fh.tag_delivery_stream(DeliveryStreamName=s, Tags=[{'Key':'aws:reserved','Value':'v'}]) and with a key containing a character outside the modeled class, and fh.untag_delivery_stream(TagKeys=['aws:reserved']) -> expect 400; observe 200.
6. Cause 5: fh.start_delivery_stream_encryption with KeyType=CUSTOMER_MANAGED_CMK and a KeyARN for a nonexistent key -> expect 400 InvalidKMSResourceException; observe acceptance.
7. Cause 6: update_destination on an ExtendedS3 stream supplying HttpEndpointDestinationUpdate -> expect 400 'Changing the destination type to or from HttpEndpoint is not supported at this time.'; observe the S3Update complaint.
8. Causes 7-8: deploy a template with an AWS::KinesisFirehose::DeliveryStream plus Outputs; after CREATE_COMPLETE read DescribeStacks Outputs (observe no OutputValue) and call list_tags_for_delivery_stream on the stream (observe ResourceNotFoundException).
9. Cause 9: trigger each rejection in trace_ids and compare the message text with the recorded templates quoted above.
10. Re-replay the check ids in trace_ids.
