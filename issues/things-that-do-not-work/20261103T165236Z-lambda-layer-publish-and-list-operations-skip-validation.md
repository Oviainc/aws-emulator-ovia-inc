Issue-Source: trace-sweep-divergence
Issue-Trace: lambda-lls-s3-source-publish-mistakes-then-valid-publish
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lambda-lls-s3-source-publish-mistakes-then-valid-publish lambda-x2x-xlsx-layer-attach-mistakes-and-size-ceiling lambda-lls-version-number-mistakes-then-list-first lambda-lls-list-filter-and-paging-parameter-mistakes
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda|PublishLayerVersion|4xx->2xx|$.Type
Issue-Key: lambda|PublishLayerVersion|4xx->2xx|$.message
Issue-Key: lambda|PublishLayerVersion|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: lambda|PublishLayerVersion|4xx->2xx|status
Issue-Key: lambda|PublishLayerVersion|4xx->4xx|$.message
Issue-Key: lambda|ListLayerVersions|2xx->2xx|$.LayerVersions[*]
Issue-Key: lambda|PublishLayerVersion|2xx->2xx|$.LayerVersionArn
Issue-Key: lambda|PublishLayerVersion|2xx->2xx|$.Version
Issue-Key: lambda|GetLayerVersion|4xx->4xx|$.Message
Issue-Key: lambda|GetLayerVersion|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: lambda|GetLayerVersion|4xx->4xx|$.message
Issue-Key: lambda|GetLayerVersion|4xx->4xx|status
Issue-Key: lambda|ListLayers|4xx->4xx|$.message
Issue-Key: lambda|ListLayerVersions|4xx->4xx|$.message
Issue-Key: lambda|ListLayers|4xx->2xx|$.Type
Issue-Key: lambda|ListLayers|4xx->2xx|$.message
Issue-Key: lambda|ListLayers|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: lambda|ListLayers|4xx->2xx|status
Issue-Related: 20261103T165236Z-lambda-layer-version-payloads-omit-content-metadata-and.md
Issue-Commit: dd64cbf0b038e7955d699468def5671676941ec2

# Lambda layer publish and list operations skip validation

> _Note: this issue was identified against the code base at commit `dd64cbf0b038e7955d699468def5671676941ec2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PublishLayerVersion

## Description
PublishLayerVersion publishes layer versions AWS refuses, which inflates the layer's version counter and its version listing; the layer read and list operations then refuse bad input with the wrong status or the wrong enumeration. 9 causes, all in the PublishLayerVersion / GetLayerVersion / ListLayers / ListLayerVersions validation stage.

Cause 1 of 9 - PublishLayerVersion publishes a version for an S3 key that does not exist.
AWS: Smithy: PublishLayerVersion input Content -> LayerVersionContentInput; the operation declares InvalidParameterValueException (400). AWS fetches the named object and surfaces S3's failure: HTTP 400, Type 'User', message 'Error occurred while GetObject. S3 Error Code: NoSuchKey. S3 Error Message: The specified key does not exist.' (fixed text; the S3 error code and message are the ones S3 returned).
Emulator: answers HTTP 201 and publishes a layer version whose content is empty.
Why: a client that mistypes the S3 key gets a published, attachable layer version containing nothing; the function it attaches it to then fails at runtime with no deploy-time signal. Every subsequent version number for that layer is also shifted (causes 3 and 4).
Fix: the PublishLayerVersion handler's S3 content resolution, before the version is allocated.
Traces: lambda-lls-s3-source-publish-mistakes-then-valid-publish#task#4 (4 steps affected).

Cause 2 of 9 - the both-sources refusal uses emulator wording.
AWS: same InvalidParameterValueException (400); supplying both an inline ZipFile and the S3 locator is refused with the message 'Please do not provide other FunctionCode parameters when providing a ZipFile.' (fixed text, no variable parts).
Emulator: refuses with the correct 400 but 'Content must name either an Amazon S3 location or an inline ZipFile, not both'.
Fix: the PublishLayerVersion content validation's message.
Traces: lambda-lls-s3-source-publish-mistakes-then-valid-publish#task#7.

Cause 3 of 9 - ListLayerVersions lists versions that should never have been published.
AWS: after the refused publishes, the layer has no versions, so ListLayerVersions returns an empty LayerVersions list.
Emulator: returns three entries (versions 3, 2 and 1) created by the publishes of cause 1.
Why: a client enumerating a layer's versions to pick the newest picks a version that does not exist on AWS and whose content is empty.
Fix: cause 1's validation; the listing itself is correct once no phantom versions are written.
Traces: lambda-lls-s3-source-publish-mistakes-then-valid-publish#task#9.

Cause 4 of 9 - the first legitimate publish is numbered 4 instead of 1.
AWS: Smithy: PublishLayerVersion output Version (LayerVersionNumber) and LayerVersionArn. With no prior successful publish, the first version is 1 and LayerVersionArn ends in ':1'.
Emulator: returns Version=4 and an ARN ending ':4', because the three refused publishes consumed numbers 1-3. The same response also carries cause 1's empty content metadata.
Why: a client that pins a layer by version number - the documented way to attach a layer reproducibly - pins a number that means a different version on AWS.
Fix: as cause 1 - the version counter must only advance on an accepted publish.
Traces: lambda-lls-s3-source-publish-mistakes-then-valid-publish#task#10.

Cause 5 of 9 - GetLayerVersion omits the message for a non-numeric version number.
AWS: Smithy: GetLayerVersion input VersionNumber is a required long bound to an httpLabel. A non-numeric label is a deserialization failure: HTTP 400 with the message "'<value>' can not be converted to Long" - fixed text with the supplied label value quoted.
Emulator: answers the correct 400 with x-amzn-errortype ValidationException but no $.Message at all.
Why: a client that surfaces or matches the message has nothing to read, so the operator is told only 'bad request' without being told which path segment is wrong.
Fix: the GetLayerVersion path-label deserialization error message.
Traces: lambda-lls-version-number-mistakes-then-list-first#task#2 (2 steps affected).

Cause 6 of 9 - a version number below 1 is answered 404 instead of 400.
AWS: GetLayerVersion declares both InvalidParameterValueException (400) and ResourceNotFoundException (404); a version number less than 1 is an input error, refused with HTTP 400 and the message 'Layer Version Cannot be less than 1' (fixed text, no variable parts).
Emulator: answers HTTP 404 with x-amzn-errortype ResourceNotFoundException and no message, i.e. it looks the out-of-range number up as if it could exist.
Why: a client distinguishing 'fix your input' from 'this version was deleted / not yet published' takes the wrong branch and may retry forever.
Fix: the GetLayerVersion handler's VersionNumber range validation, ahead of the lookup.
Traces: lambda-lls-version-number-mistakes-then-list-first#task#4.

Cause 7 of 9 - the ListLayers CompatibleRuntime enum refusal advertises a different value set.
AWS: Smithy: ListLayers input CompatibleRuntime targets the Runtime enum. An unknown value is refused with HTTP 400 and "1 validation error detected: Value '<value>' at 'compatibleRuntime' failed to satisfy constraint: Member must satisfy enum value set: [<set>]" - fixed text whose variable parts are the offending value, the member name and the service's own enum value set. AWS's set additionally contains the values its live model carries and that this emulator's does not (among them ruby2.6, nasa, custom, byol, go1.9, nodejs8.x, nodejs8.9, python3.4, python2.7-greengrass and the three greengrass runtime ARNs), and AWS does not emit them in the model's declaration order.
Emulator: refuses with the correct 400 and the correct sentence frame, but the bracketed set is this emulator's own, shorter list in declaration order, so it both omits values AWS accepts and advertises a different membership.
Why: a client that parses the advertised set to offer the operator a valid runtime - or that validates locally against it before retrying - rejects runtimes AWS accepts.
Fix: the Runtime enum value set used by the ListLayers validation's message.
Traces: lambda-lls-list-filter-and-paging-parameter-mistakes#task#1.

Cause 8 of 9 - ListLayerVersions advertises the same wrong value set.
AWS: as cause 7, with the member name 'compatibleRuntime' on ListLayerVersions.
Emulator: as cause 7.
Fix: as cause 7, shared by both list validations.
Traces: lambda-lls-list-filter-and-paging-parameter-mistakes#task#5.

Cause 9 of 9 - an invalid pagination Marker is accepted instead of refused.
AWS: Smithy: ListLayers is @paginated with inputToken Marker and declares InvalidParameterValueException (400). A Marker that is not one the service issued is refused with HTTP 400, Type 'User' and the message 'Invalid pagination key.' (fixed text, no variable parts).
Emulator: answers HTTP 200 with a page of results, silently ignoring the token.
Why: a client that resumes paging with a corrupted or stale token restarts from the beginning and double-counts layers instead of being told the token is unusable.
Fix: the ListLayers handler's Marker validation.
Traces: lambda-lls-list-filter-and-paging-parameter-mistakes#task#4.

## Evidence
Cause 1: lambda-lls-s3-source-publish-mistakes-then-valid-publish#task#4 - expected 400 + $.Type='User' + $.message='Error occurred while GetObject. S3 Error Code: NoSuchKey. S3 Error Message: The specified key does not exist.'; got 201 (4 steps affected).
Cause 2: #task#7 - 400 vs 400, $.message expected 'Please do not provide other FunctionCode parameters when providing a ZipFile.', got 'Content must name either an Amazon S3 location or an inline ZipFile, not both'.
Cause 3: #task#9 - $.LayerVersions[0..2] expected absent, got versions 3, 2 and 1.
Cause 4: #task#10 - $.Version expected 1, got 4; $.LayerVersionArn expected '...:<layer>:1', got '...:<layer>:4'.
Cause 5: lambda-lls-version-number-mistakes-then-list-first#task#2 - 400 vs 400, $.Message expected "'latest' can not be converted to Long", got absent; x-amzn-errortype came back ValidationException against a shape assertion (2 steps affected).
Cause 6: #task#4 - expected 400 + $.message='Layer Version Cannot be less than 1'; got 404 ResourceNotFoundException with no message.
Causes 7-8: lambda-lls-list-filter-and-paging-parameter-mistakes#task#1 (ListLayers) and #task#5 (ListLayerVersions) - 400 vs 400, the bracketed enum value set in $.message differs in membership and order from AWS's.
Cause 9: #task#4 - expected 400 + $.Type='User' + $.message='Invalid pagination key.'; got 200.

## Steps to Reproduce
1. Replay lambda-lls-s3-source-publish-mistakes-then-valid-publish end to end: task#4 (publish from a missing S3 key), task#7 (ZipFile plus S3 locator), task#9 (ListLayerVersions) and task#10 (the first valid publish) - check the version number it is assigned.
2. Replay lambda-lls-version-number-mistakes-then-list-first task#2 (GetLayerVersion with a non-numeric version) and task#4 (version 0 or negative).
3. Replay lambda-lls-list-filter-and-paging-parameter-mistakes task#1 and task#5 (bad CompatibleRuntime) and task#4 (a bogus Marker).
