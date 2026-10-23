Issue-Source: trace-sweep-divergence
Issue-Trace: kafkaconnect-custom-plugin-invalid-locations
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kafkaconnect-custom-plugin-invalid-locations
Issue-Shape-Id: com.amazonaws.kafkaconnect#KafkaConnect
Issue-Key: kafkaconnect|CreateCustomPlugin|4xx->2xx|$.message
Issue-Key: kafkaconnect|CreateCustomPlugin|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: kafkaconnect|CreateCustomPlugin|4xx->2xx|status
Issue-Key: kafkaconnect|ListCustomPlugins|2xx->2xx|$.customPlugins[*]
Issue-Commit: 0b023ce22a329ff2896e758c2b0769a212a04448

# kafkaconnect CreateCustomPlugin accepts an unreachable s3Location instead of rejecting it with 400 BadRequestException

> _Note: this issue was identified against the code base at commit `0b023ce22a329ff2896e758c2b0769a212a04448`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCustomPlugin

## Description
Two causes, both rooted in CreateCustomPlugin performing no reachability/validation check on the supplied custom-plugin location before it creates the resource.

Cause 1 of 2: CreateCustomPlugin returns 200 and creates a plugin for a location the caller cannot read.
AWS: POST /v1/custom-plugins takes a required `location` (CustomPluginLocation) and models BadRequestException with httpError 400 among its errors (Smithy: CreateCustomPlugin.location required, CustomPluginLocation; errors BadRequestException=400, ConflictException=409, NotFoundException=404). The recording shows AWS validating the referenced object before creating anything: when the s3Location points at a bucket/key that does not exist or is not accessible, the call fails with HTTP 400, an `x-amzn-errortype` header naming the modeled error, and a `message` body member (recorded example: 'Invalid parameter s3Location: Could not access the specified custom plugin file. Make sure the bucket is in the same region, the file exists and you have access to it.'). The 400 status, the presence of the error-type header and the presence of a `message` member are the requirements; the exact sentence is an example.
Emulator: returns HTTP 200 with a normal CreateCustomPlugin response body (customPluginArn/customPluginState=CREATING), no `message` member and no `x-amzn-errortype` header.
Why: a client that relies on AWS surfacing a bad plugin artifact at create time sees success, and only discovers the broken plugin much later (or never, since the emulator also never moves the plugin out of CREATING). Error-handling paths for a mistyped bucket or key are never exercised, and tests asserting a 400 pass against AWS and fail against the emulator.
Fix: CreateCustomPlugin's validation in the kafkaconnect operation handler — check the referenced location (bucket exists/accessible, file key present) before mutating state, and on failure serialize BadRequestException with HTTP 400, the `x-amzn-errortype` header and a `message` member naming the offending parameter.
Traces: kafkaconnect-custom-plugin-invalid-locations#task#2.

Cause 2 of 2: ListCustomPlugins reports the plugin that should never have been created.
AWS: GET /v1/custom-plugins returns `customPlugins` (list of CustomPluginSummary) (Smithy: ListCustomPlugins -> ListCustomPluginsResponse.customPlugins). After the rejected create, the recording shows no summary for that name.
Emulator: returns 200 with one extra entry for the rejected plugin name, carrying a customPluginArn, customPluginState=CREATING and a latestRevision whose location echoes the unreachable s3Location.
Why: the phantom resource is visible to every subsequent list/describe call and to name-uniqueness checks, so a client retrying the create with a corrected location can collide with a resource AWS never created.
Fix: same handler/state as cause 1 — rejecting the create must leave the custom-plugin store untouched, so no summary is ever listed.
Traces: kafkaconnect-custom-plugin-invalid-locations#task#4.

## Evidence
Cause 1 — kafkaconnect-custom-plugin-invalid-locations#task#2, CreateCustomPlugin: expected status 400, got 200; $.message missing (expected 'Invalid parameter s3Location: Could not access the specified custom plugin file. Make sure the bucket is in the same region, the file exists and you have access to it.', got None); $headers.x-amzn-errortype missing (expected a shaped error type, got None).

Cause 2 — kafkaconnect-custom-plugin-invalid-locations#task#4, ListCustomPlugins: status 200 both sides, but $.customPlugins[0] is extra — expected None, got {'customPluginArn': 'arn:aws:kafkaconnect:...:custom-plugin/sim2real-probe-kafkaconnect-plugin-missing-.../...', 'customPluginState': 'CREATING', 'latestRevision': {'contentType': 'ZIP', 'location': {'s3Location': {'bucketArn': 'arn:aws:s3:::sim2real-probe-kafkaconnect-nobucket-...', 'fileKey': 'plugin.zip'}}, 'revision': 1}, ...}. (ARNs, ids and timestamps in the recording are environment-specific examples.)

## Steps to Reproduce
1. Boot the emulator and replay fixture kafkaconnect-custom-plugin-invalid-locations.
2. Step task#2: POST /v1/custom-plugins with contentType=ZIP, a name, and location.s3Location pointing at a bucket ARN/fileKey that does not exist. Observe 200 with a CreateCustomPlugin success body instead of 400 BadRequestException with an x-amzn-errortype header and a message member.
3. Step task#4: GET /v1/custom-plugins and observe the rejected plugin listed in customPlugins instead of an empty list.
