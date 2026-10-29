Issue-Source: trace-sweep-divergence
Issue-Trace: controltower-baseline-catalog-discovery
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: controltower-baseline-catalog-discovery controltower-tag-resource-error-paths
Issue-Shape-Id: com.amazonaws.controltower#AWSControlTowerApis
Issue-Key: controltower|ListTagsForResource|4xx->4xx|$.message
Issue-Key: controltower|ListTagsForResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: controltower|ListTagsForResource|4xx->4xx|status
Issue-Key: controltower|TagResource|4xx->4xx|$.message
Issue-Key: controltower|TagResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: controltower|TagResource|4xx->4xx|status
Issue-Key: controltower|UntagResource|4xx->4xx|$.message
Issue-Key: controltower|UntagResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: controltower|UntagResource|4xx->4xx|status
Issue-Commit: 2f933b71caa2a46f81d276f12cbb60da2946d084

# Control Tower tag-plane ARN validation is skipped: malformed and cross-account resourceArn answered 404 ResourceNotFoundException instead of 400 BadRequestException

> _Note: this issue was identified against the code base at commit `2f933b71caa2a46f81d276f12cbb60da2946d084`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
All three tag-plane operations treat an unacceptable resourceArn as a lookup miss: they go straight to the tag/resource store and report that nothing matches, instead of refusing the ARN at the front door. AWS validates the ARN first and answers 400 with x-amzn-errortype BadRequestException; the emulator answers 404 ResourceNotFoundException with its own prose. The error class, status and message are all wrong, and the precedence (validate the ARN before resolving it) is the root cause shared by the three causes. A previously resolved issue (20261022T032312Z-controltower-input-validation-rejections-answer-validationexception-instead-of) covered the BadRequestException front-door envelope for other operations; it does not cover the tag plane, which still never reaches that path.

Cause 1 of 3 — ListTagsForResource on an ARN AWS rejects as invalid (fixture controltower-baseline-catalog-discovery, step task#3).
AWS: Smithy: ListTagsForResource.resourceArn @httpLabel, required, Arn @length 20..2048 @pattern ^arn:aws[0-9a-zA-Z_\-:\/]+$; modelled errors ValidationException (400) and ResourceNotFoundException (404). The recorded response shows the front door refusing the ARN first: HTTP 400, x-amzn-errortype: BadRequestException, message 'Invalid input resource arn' — entirely fixed text, with no request- or state-derived values interpolated.
Emulator: HTTP 404, x-amzn-errortype: ResourceNotFoundException, message 'no resource matches resourceArn <resourceArn>' (emulator-authored prose that echoes the request ARN).
Why: a client cannot distinguish a bad request from an absent resource. A 404 with ResourceNotFoundException is a retry-or-create signal in most SDK error handling, and a caller that passes an unusable ARN (a baseline ARN, which is not a taggable Control Tower resource) is told to create something rather than to fix its input.
Fix: resourceArn validation in the ListTagsForResource handler, ahead of the tag-store lookup, emitting the BadRequestException front-door envelope.
Traces: controltower-baseline-catalog-discovery#task#3 (the cluster also covers controltower-tag-resource-error-paths#task#2).

Cause 2 of 3 — TagResource on a cross-account resourceArn (fixture controltower-tag-resource-error-paths, step task#1).
AWS: Smithy: TagResource.resourceArn @httpLabel required, tags required; http 204 on success; modelled errors ValidationException (400), ResourceNotFoundException (404). Recorded behavior: an ARN whose account segment is not the caller's account is refused before resolution — HTTP 400, x-amzn-errortype: BadRequestException, message 'Invalid input resource arn: ARN must be in the same account'. The whole message is fixed text; the rule it states (the ARN's account segment must equal the calling account) is the requirement, and the offending ARN itself is NOT part of the message.
Emulator: HTTP 404, x-amzn-errortype: ResourceNotFoundException, message 'no resource matches resourceArn <resourceArn>'.
Why: cross-account tagging attempts look like missing resources, so a client retries or tries to create the resource instead of correcting the account in the ARN; the same-account rule is unenforced in the emulator's tag store.
Fix: the TagResource handler's resourceArn validation (shape check plus same-account check) ahead of resource resolution.
Traces: controltower-tag-resource-error-paths#task#1.

Cause 3 of 3 — UntagResource on a cross-account resourceArn (fixture controltower-tag-resource-error-paths, step task#3).
AWS: Smithy: UntagResource.resourceArn @httpLabel required, tagKeys @httpQuery required; http 204; modelled errors ValidationException (400), ResourceNotFoundException (404). Recorded behavior is the sibling of cause 2 — HTTP 400, x-amzn-errortype: BadRequestException, message 'Invalid input resource arn: ARN must be in the same account' (all fixed text).
Emulator: HTTP 404, x-amzn-errortype: ResourceNotFoundException, message 'no resource matches resourceArn <resourceArn>'.
Why: same as cause 2 — an input defect is reported as an absent resource, and the untag path accepts cross-account ARNs as far as the store lookup.
Fix: the UntagResource handler's resourceArn validation, shared with TagResource/ListTagsForResource so all three refuse identically.
Traces: controltower-tag-resource-error-paths#task#3.

## Evidence
Replay of trace set simparity-wire-aws-v1 (controltower, 3 clusters):

1) ListTagsForResource, controltower-baseline-catalog-discovery#task#3 (cluster also affects controltower-tag-resource-error-paths#task#2): status expected 400, got 404; $.message expected 'Invalid input resource arn', got 'no resource matches resourceArn arn:aws:controltower:us-west-2::baseline/17BSJV3IGJ2QSGA2'; $headers.x-amzn-errortype expected 'BadRequestException', got 'ResourceNotFoundException'.

2) TagResource, controltower-tag-resource-error-paths#task#1: status expected 400, got 404; $.message expected 'Invalid input resource arn: ARN must be in the same account', got 'no resource matches resourceArn arn:aws:controltower:us-west-2:111111111111:landingzone/NONEXISTENT9999999999'; $headers.x-amzn-errortype expected 'BadRequestException', got 'ResourceNotFoundException'.

3) UntagResource, controltower-tag-resource-error-paths#task#3: status expected 400, got 404; $.message expected 'Invalid input resource arn: ARN must be in the same account', got 'no resource matches resourceArn arn:aws:controltower:us-west-2:111111111111:landingzone/NONEXISTENT9999999999'; $headers.x-amzn-errortype expected 'BadRequestException', got 'ResourceNotFoundException'.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Fixture controltower-baseline-catalog-discovery, step task#3: GET /tags/{resourceArn} with a baseline ARN; observe 404 ResourceNotFoundException instead of 400 BadRequestException 'Invalid input resource arn'.
3. Fixture controltower-tag-resource-error-paths, step task#1: POST /tags/{resourceArn} with an ARN whose account segment is not the caller's account; observe 404 ResourceNotFoundException instead of 400 BadRequestException 'Invalid input resource arn: ARN must be in the same account'.
4. Same fixture, step task#3: DELETE /tags/{resourceArn}?tagKeys=... with the same cross-account ARN; observe the identical wrong 404.
