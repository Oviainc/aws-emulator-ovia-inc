Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__s3.cfn-changeset-template-source-conflicts-rejected cloudformation__s3.cfn-changeset-template-url-allows-template-over-inline-limit cloudformation__s3.cfn-changeset-template-url-deleted-or-unknown-version-rejected cloudformation__s3.cfn-changeset-template-url-include-nested-stacks-child-url cloudformation__s3.cfn-changeset-template-url-missing-bucket-rejected cloudformation__s3.cfn-changeset-template-url-object-over-1mb-rejected cloudformation__s3.cfn-changeset-template-url-unsupported-location-rejected
Issue-Key: cloudformation__s3.cfn-changeset-template-source-conflicts-rejected|run#8|AssertionError|assert got == ("ValidationError", message), f"{sorted(source)!r}: expected {message!r}, got {got!r}"
Issue-Key: cloudformation__s3.cfn-changeset-template-url-allows-template-over-inline-limit|run#11|AssertionError|assert got == want, f"the inline template must be refused by length: expected {want!r}, got {got!r}"
Issue-Key: cloudformation__s3.cfn-changeset-template-url-deleted-or-unknown-version-rejected|run#10|AssertionError|assert got == ("ValidationError", message), f"{url!r}: expected {message!r}, got {got!r}"
Issue-Key: cloudformation__s3.cfn-changeset-template-url-include-nested-stacks-child-url|run#14|AssertionError|assert nested_id, f"IncludeNestedStacks must give the nested change its own ChangeSetId: {update['Changes']!r}"
Issue-Key: cloudformation__s3.cfn-changeset-template-url-missing-bucket-rejected|run#6|AssertionError|assert got == want, f"{url!r}: expected {want!r}, got {got!r}"
Issue-Key: cloudformation__s3.cfn-changeset-template-url-object-over-1mb-rejected|run#11|AssertionError|assert got == want, f"expected {want!r}, got {got!r}"
Issue-Key: cloudformation__s3.cfn-changeset-template-url-unsupported-location-rejected|run#7|AssertionError|assert got == ("ValidationError", message), f"{url!r}: expected {message!r}, got {got!r}"
Issue-Commit: 482573dd66a8fdd59e0387c95d89889e67d3b0e1

# CreateChangeSet template-source validation: 7 divergent refusals for TemplateBody/TemplateURL

> _Note: this issue was identified against the code base at commit `482573dd66a8fdd59e0387c95d89889e67d3b0e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateChangeSet

## Description
CreateChangeSet's template-source validation diverges from AWS in seven distinct ways. Seven net-new root causes are bundled here; a fixer must close all seven.

Cause 1 of 7: both TemplateBody and TemplateURL supplied.
AWS: refuses ValidationError with the fixed message 'You cannot specify both Template URL and Template Body.' (Smithy: CreateChangeSet models TemplateBody and TemplateURL as separate optional members, so the mutual-exclusion rule is prose/recorded behavior; the recorded response is ValidationError 400 with that exact text.)
Emulator: ValidationError with 'You must specify exactly one of TemplateBody, TemplateURL or UsePreviousTemplate' -- the message for the *neither-supplied* case, reused for the both-supplied case.
Why: a client that branches on the message (or a human reading it) is told it supplied nothing when it supplied two sources; the two mistakes become indistinguishable.
Fix: the CreateChangeSet handler's template-source validation, which must keep the both-supplied and none-supplied messages distinct.
Traces: cloudformation__s3.cfn-changeset-template-source-conflicts-rejected

Cause 2 of 7: inline TemplateBody over the 51200-byte limit does not echo the offending value.
AWS: ValidationError whose message is the standard Smithy constraint report and INCLUDES the rejected value, truncated: "1 validation error detected: Value '<first bytes of the template>...<N chars total>' at 'templateBody' failed to satisfy constraint: Member must have length less than or equal to 51200". The fixed text is '1 validation error detected: Value ', " at 'templateBody' failed to satisfy constraint: Member must have length less than or equal to 51200"; the variable part is the request's own templateBody prefix and its total length.
Emulator: omits the value entirely -- "1 validation error detected: Value at 'templateBody' failed to satisfy constraint: Member must have length less than or equal to 51200".
Why: tooling that parses the standard constraint-violation message to recover which value was rejected finds no value; the message is not the one AWS emits for this constraint class.
Fix: the shared constraint-violation message formatter used by the CreateChangeSet validation path, which must render the rejected value.
Traces: cloudformation__s3.cfn-changeset-template-url-allows-template-over-inline-limit

Cause 3 of 7: TemplateURL naming a deleted or unknown object versionId.
AWS: ValidationError whose message wraps the S3 refusal verbatim: 'S3 error: Access Denied\nFor more information check http://docs.aws.amazon.com/AmazonS3/latest/API/ErrorResponses.html' (CloudFormation reports a version it cannot read as Access Denied, never as a missing version).
Emulator: ValidationError with 'S3 object does not exist. Bucket: <bucket>, Key: <key>. Error: NoSuchVersion' -- its own sentence, naming a bucket/key/S3-code the AWS message never carries.
Why: a caller cannot tell a permissions problem from a version problem the way AWS presents it, and the message leaks a shape no AWS client parses.
Fix: the CreateChangeSet TemplateURL fetch path, which must translate the S3 outcome into CloudFormation's 'S3 error: <s3 message>' envelope.
Traces: cloudformation__s3.cfn-changeset-template-url-deleted-or-unknown-version-rejected

Cause 4 of 7: IncludeNestedStacks gives the nested change no ChangeSetId.
AWS: with IncludeNestedStacks=true, a Modify change on a child AWS::CloudFormation::Stack resource carries its own ChangeSetId naming the nested change set (that id is how a caller walks into the child's changes).
Emulator: the Changes entry for the child carries Type/ResourceChange with Action, LogicalResourceId, PhysicalResourceId, ResourceType, Replacement, Scope and Details, but no ChangeSetId at all.
Why: a client that recurses into nested change sets has no handle to recurse with, so nested change detail is unreachable.
Fix: the CreateChangeSet/DescribeChangeSet nested-stack expansion, which must mint and report a nested ChangeSetId.
Traces: cloudformation__s3.cfn-changeset-template-url-include-nested-stacks-child-url

Cause 5 of 7: TemplateURL in a bucket that does not exist.
AWS: ValidationError with 'S3 error: The specified bucket does not exist\nFor more information check http://docs.aws.amazon.com/AmazonS3/latest/API/ErrorResponses.html'.
Emulator: ValidationError with 'S3 object does not exist. Bucket: <bucket>, Key: <key>. Error: NoSuchBucket'.
Why: same as cause 3 -- the refusal is not in CloudFormation's wording, so message-matching clients and operators see an unfamiliar error.
Fix: same template-fetch error mapping, for the missing-bucket outcome.
Traces: cloudformation__s3.cfn-changeset-template-url-missing-bucket-rejected

Cause 6 of 7: a TemplateURL object larger than 1 MB is judged against the inline 51200 limit.
AWS: a template fetched from S3 may be up to 1000000 bytes; over that it is refused ValidationError 'Template may not exceed 1000000 bytes in size.' The 51200 limit applies only to inline TemplateBody.
Emulator: answers "1 validation error detected: Value at 'templateBody' failed to satisfy constraint: Member must have length less than or equal to 51200" -- it applies the inline limit to the fetched template.
Why: every template between 51200 and 1000000 bytes served from S3 is wrongly refused, and the refusal names a member the caller never sent.
Fix: the CreateChangeSet size validation, which must apply 51200 only to TemplateBody and 1000000 to a fetched TemplateURL.
Traces: cloudformation__s3.cfn-changeset-template-url-object-over-1mb-rejected

Cause 7 of 7: a TemplateURL that is not an S3 location.
AWS: ValidationError 'TemplateURL must be a supported URL.' for a non-S3 host (e.g. https://example.com/stack.json).
Emulator: ValidationError 'TemplateURL must reference a valid S3 object to which you have access.' -- the message AWS uses for an S3 URL it cannot read, not for an unsupported scheme/host.
Why: the caller is told it has an access problem on an S3 object when the URL is not an S3 URL at all.
Fix: the CreateChangeSet TemplateURL parser, which must separate 'unsupported URL' from 'S3 object not accessible'.
Traces: cloudformation__s3.cfn-changeset-template-url-unsupported-location-rejected

## Evidence
Replay details, verbatim:
1. ['TemplateBody', 'TemplateURL']: expected 'You cannot specify both Template URL and Template Body.', got ('ValidationError', 'You must specify exactly one of TemplateBody, TemplateURL or UsePreviousTemplate')
2. expected ('ValidationError', '1 validation error detected: Value \'{"AWSTemplateFormatVersion": "2010-09-09", ...<60189 chars total>\' at \'templateBody\' failed to satisfy constraint: Member must have length less than or equal to 51200'), got ('ValidationError', "1 validation error detected: Value at 'templateBody' failed to satisfy constraint: Member must have length less than or equal to 51200")
3. expected 'S3 error: Access Denied\nFor more information check http://docs.aws.amazon.com/AmazonS3/latest/API/ErrorResponses.html', got ('ValidationError', 'S3 object does not exist. Bucket: <bucket>, Key: templates/stack.json. Error: NoSuchVersion')
4. AssertionError: IncludeNestedStacks must give the nested change its own ChangeSetId: [{'Type': 'Resource', 'ResourceChange': {'Action': 'Modify', 'LogicalResourceId': 'Child', 'ResourceType': 'AWS::CloudFormation::Stack', ...}}]  (no ChangeSetId key)
5. expected ('ValidationError', 'S3 error: The specified bucket does not exist\nFor more information check http://docs.aws.amazon.com/AmazonS3/latest/API/ErrorResponses.html'), got ('ValidationError', 'S3 object does not exist. Bucket: <bucket>, Key: stack.json. Error: NoSuchBucket')
6. expected ('ValidationError', 'Template may not exceed 1000000 bytes in size.'), got ('ValidationError', "1 validation error detected: Value at 'templateBody' failed to satisfy constraint: Member must have length less than or equal to 51200")
7. 'https://example.com/stack.json': expected 'TemplateURL must be a supported URL.', got ('ValidationError', 'TemplateURL must reference a valid S3 object to which you have access.')

## Steps to Reproduce
Replay the behavioral suite and run the cloudformation__s3 checks listed in trace_ids. Each one calls CreateChangeSet against a stack whose template source is varied: (1) both TemplateBody and TemplateURL; (2) an inline TemplateBody of ~60 KB; (3) a TemplateURL carrying a versionId that was deleted; (4) a parent template whose child resource is an AWS::CloudFormation::Stack, with IncludeNestedStacks=true; (5) a TemplateURL in a bucket that was never created; (6) a TemplateURL object larger than 1 MB; (7) TemplateURL=https://example.com/stack.json. Compare the returned error code and message (and, for 4, the Changes[].ResourceChange payload) with the expectations above.
