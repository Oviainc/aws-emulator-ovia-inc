Issue-Source: trace-sweep-divergence
Issue-Trace: cloudformation-cfns3cnt-bucket-side-reverse-lookup
Issue-Step: task#9
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudformation-cfns3cnt-bucket-side-reverse-lookup
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: s3vectors|ListTagsForResource|2xx->2xx|$.tags.aws:cloudformation:logical-id
Issue-Key: s3vectors|ListTagsForResource|2xx->2xx|$.tags.aws:cloudformation:stack-id
Issue-Key: s3vectors|ListTagsForResource|2xx->2xx|$.tags.aws:cloudformation:stack-name
Issue-Commit: 63c64ce081b9c54ca8821134bfa1b94c44bc83e7

# Stack-created vector bucket tag map lacks the three aws:cloudformation reserved keys

> _Note: this issue was identified against the code base at commit `63c64ce081b9c54ca8821134bfa1b94c44bc83e7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTagsForResource

## Description
One net-new root cause of service `s3vectors`. The replay reports this cluster with stub_path='' and owned_by=[], so no open issue carries its replay key. Open issue 20261103T171935Z-s3vectors-s3-vectors-listtagsforresource-omits-the-aws-cloudformation-tags.md describes the same shape on the same fixture step but does NOT own this key (its recorded Issue-Key is 's3vectors|ListTagsForResource|2xx->2xx|$.tags.aws', while the cluster reported now keys on the per-tag paths $.tags.aws:cloudformation:stack-id / stack-name / logical-id), so the divergence is still live and must be closed here; closing both together is correct.

Cause 1 of 1: a vector bucket provisioned by a CloudFormation stack carries none of the three reserved stack-ownership tags, so its tag map cannot be used to find the owning stack.
AWS: recorded behavior only for the tag provenance — the S3 Vectors Smithy model only states the container (Smithy: ListTagsForResource GET /tags/{resourceArn}, output member `tags` target TagsMap required; TagsMap key TagKey @length 1..128 @pattern ^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$, value TagValue @length 0..256; errors NotFoundException 404, ValidationException 400, AccessDeniedException 403) and says nothing about reserved keys. AWS CloudFormation stamps every resource it provisions — here an AWS::S3Vectors::VectorBucket — with three reserved tags IN ADDITION to any the template declared: aws:cloudformation:stack-id = the owning stack's ARN, aws:cloudformation:stack-name = the stack's name, aws:cloudformation:logical-id = the resource's logical id in the template. The owning service's tag-read API returns them: S3 Vectors ListTagsForResource answers 200 with all three present in `tags`. The three KEYS are the fixed requirement; their values are taken from the owning stack (ARN, name) and from the template (the resource's logical id).
Emulator: ListTagsForResource answers 200 but `tags` contains none of the three aws:cloudformation:* entries — all three are absent from the map.
Why: those three tags ARE the resource-to-stack reverse lookup. A client holding only a vector bucket ARN cannot discover the stack that owns it, cannot distinguish a stack-managed bucket from a hand-made one, and every stack-scoped tag report or cost/ownership attribution over vector buckets comes back with no stack attribution.
Fix: the AWS::S3Vectors::VectorBucket provisioning path in the CloudFormation resource provider must merge the three reserved tags into the vector-bucket tag set that the S3 Vectors ListTagsForResource handler reads out of state, alongside the template's tags.
Traces: cloudformation-cfns3cnt-bucket-side-reverse-lookup#task#9.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout. Divergence cluster: divergence_key 'service=s3vectors|ListTagsForResource|status=200->200|keys=$.tags.aws:cloudformation:logical-id,$.tags.aws:cloudformation:stack-id,$.tags.aws:cloudformation:stack-name', verdict mismatch, affected_count 1, stub_path='', owned_by=[]. Fixture cloudformation-cfns3cnt-bucket-side-reverse-lookup, step task#9, operation ListTagsForResource. Status 200 expected and 200 actual, so only the body diverges:
- $.tags.aws:cloudformation:stack-id: missing — expected the owning stack's ARN, got None.
- $.tags.aws:cloudformation:stack-name: missing — expected the owning stack's name, got None.
- $.tags.aws:cloudformation:logical-id: missing — expected the resource's logical id in the template ('Vec' in this recording), got None.
The stack ARN, its uuid, the generated stack name and the probe name suffixes in the recording are per-run values filling those slots; the requirement is the presence of the three reserved keys with values derived from the owning stack and the template.

## Steps to Reproduce
1. Clone this checkout, build the emulator and replay the simparity-wire-aws-v1 wire corpus against it.
2. Replay fixture cloudformation-cfns3cnt-bucket-side-reverse-lookup step task#9: deploy a CloudFormation stack declaring an AWS::S3Vectors::VectorBucket (logical id Vec), wait for CREATE_COMPLETE, then call S3 Vectors ListTagsForResource with the created vector bucket's ARN.
3. Inspect the returned `tags` map: it must contain aws:cloudformation:stack-id (the stack ARN), aws:cloudformation:stack-name (the stack name) and aws:cloudformation:logical-id (the template logical id) in addition to any template tags. Observe that none of the three are present.
