Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafkaconnect.list-connectors-malformed-next-token-rejected kafkaconnect.create-custom-plugin-duplicate-name-rejected kafkaconnect.create-custom-plugin-malformed-s3-location-rejected kafkaconnect.custom-plugin-delete-removes-resource kafkaconnect.custom-plugin-lifecycle-creating-to-active kafkaconnect.describe-custom-plugin-not-found-rejected kafkaconnect.list-custom-plugins-name-prefix-filter kafkaconnect.list-tags-for-resource-reflects-creation-time-tags
Issue-Shape-Id: com.amazonaws.kafkaconnect#KafkaConnect
Issue-Key: kafkaconnect.list-connectors-malformed-next-token-rejected|run#7|AssertionError|assert message == "Invalid parameter nextToken.", message
Issue-Key: kafkaconnect|PutObject|NotImplemented
Issue-Commit: d6cf4ead3dce27ec3b8bcbc3d4a389da0fa571ea

# kafkaconnect: invalid-nextToken BadRequestException message diverges, and 7 custom-plugin/tagging checks are blocked by the S3 PutObject stub (2 causes)

> _Note: this issue was identified against the code base at commit `d6cf4ead3dce27ec3b8bcbc3d4a389da0fa571ea`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListConnectors

## Description
Two distinct root causes found in the kafkaconnect behavioral sweep. Both belong to the kafkaconnect check family (check ids lead with `kafkaconnect`), so they are bundled here; a fixer must close BOTH before submitting.

=== Cause 1 of 2: ListConnectors rejects an unissued nextToken with the wrong message ===
AWS: Smithy (services/service-com-amazonaws-kafkaconnect-kafkaconnect/smithy-model/com-amazonaws-kafkaconnect-kafkaconnect.json) models ListConnectors with `nextToken` (__string, httpQuery) and the error `BadRequestException` (httpError 400, doc: "HTTP Status Code 400: Bad request due to incorrect input. Correct your request and then retry it."). The model does not state the message text; recorded behavior shows kafkaconnect answers 400 / `BadRequestException` with a request-derived validation message that names the offending member: `Invalid parameter nextToken.` The same `Invalid parameter <member>[: <reason>]` rule appears in the other recorded kafkaconnect rejections in this suite (`Invalid parameter bucketArn: Invalid ARN.`, `Invalid parameter customPluginArn: The requested resource doesn\u2019t exist.`), so the rule — the message names the rejected member — is the requirement, not the exact recorded sentence.
Emulator: status and error code are correct (400 / BadRequestException), but the message is a fixed generic sentence that names no member: `The nextToken is not one this service issued. Correct your request and then retry it.` (example value).
Why: a client or SDK-based tool that keys its diagnostics or its retry/reporting logic on kafkaconnect's `Invalid parameter <member>` validation text cannot tell which parameter the service rejected; error-text-driven assertions and operator triage see a message shape no kafkaconnect region produces.
Fix: the ListConnectors handler's pagination-token rejection path — the message string constructed when token decoding fails — and, since the same rule governs every kafkaconnect parameter rejection, the shared validation-message helper the other kafkaconnect ops should use.
Traces: kafkaconnect.list-connectors-malformed-next-token-rejected (verdict: mismatch; assertion `assert message == "Invalid parameter nextToken."`). Citation for the current behavior: services/service-com-amazonaws-kafkaconnect-kafkaconnect/src/ops/list_connectors.rs lines 185-194.

=== Cause 2 of 2: every custom-plugin / tagging check is blocked by the S3 PutObject dependency answering NotImplemented ===
AWS: these checks provision a kafkaconnect prerequisite the real service requires — a plugin ZIP uploaded to S3 (`s3.put_object`) whose `s3Location.bucketArn`/`fileKey` is then passed to `CreateCustomPlugin`. On AWS, `PutObject` returns 200 and the plugin then reaches ACTIVE, so the kafkaconnect assertions (ConflictException on duplicate plugin name, BadRequestException on a malformed bucketArn, NotFoundException after delete, namePrefix filtering, ListTagsForResource reflecting creation-time tags) are reached at all.
Emulator: the S3 dependency answers the scaffold stub for PutObject — `An error occurred (NotImplemented) when calling the PutObject operation: operation PutObject is scaffolded but not implemented in this emulator` — so all 7 checks error out in setup and NONE of the kafkaconnect custom-plugin or tagging behavior is exercised. Note this is a cross-service dependency: s3 is not yet stood up (open issue "Bootstrap the Amazon S3 emulator", 20261021T221645Z-bootstrap-service-com-amazonaws-s3-amazons3.md, plus 20261022T064119Z-s3-listobjectsv2-stub.md). Neither issue names PutObject or this kafkaconnect check family, and the replay report did not mark these checks ALREADY FILED, so they are recorded here; the same pattern is already recorded for other dependents (20261022/mwaa, b2bi, transcribe issues).
Why: the whole kafkaconnect custom-plugin surface and its tagging behavior are unverified — a client doing the normal kafkaconnect workflow (upload plugin to S3, CreateCustomPlugin, poll to ACTIVE, Describe/List/Delete) cannot complete step one, and any fidelity defect behind that wall is invisible.
Fix: the S3 PutObject handler (object write path and its state), which the kafkaconnect checks consume as a prerequisite; once it answers, re-replay the 7 checks to surface any genuine kafkaconnect custom-plugin/tag divergence underneath.
Traces: kafkaconnect.create-custom-plugin-duplicate-name-rejected, kafkaconnect.create-custom-plugin-malformed-s3-location-rejected, kafkaconnect.custom-plugin-delete-removes-resource, kafkaconnect.custom-plugin-lifecycle-creating-to-active, kafkaconnect.describe-custom-plugin-not-found-rejected, kafkaconnect.list-custom-plugins-name-prefix-filter, kafkaconnect.list-tags-for-resource-reflects-creation-time-tags.

## Evidence
Cause 1 (kafkaconnect.list-connectors-malformed-next-token-rejected, mismatch):
  Request: ListConnectors(maxResults=10, nextToken="<a token this service never issued>")
  Expected (AWS): HTTP 400, Error.Code=BadRequestException, Error.Message names the rejected member — recorded example: "Invalid parameter nextToken."
  Actual (emulator): HTTP 400, Error.Code=BadRequestException (correct), Error.Message="The nextToken is not one this service issued. Correct your request and then retry it." — names no member.
  Harness output: AssertionError: The nextToken is not one this service issued. Correct your request and then retry it. (raised by `assert message == "Invalid parameter nextToken."`)

Cause 2 (7 checks, errored in setup):
  Expected (AWS): s3.put_object(Bucket=<fixture bucket>, Key="plugin.zip", Body=<minimal ZIP>) -> HTTP 200, then CreateCustomPlugin/DescribeCustomPlugin/ListCustomPlugins/ListTagsForResource assertions run.
  Actual (emulator): botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the PutObject operation: operation PutObject is scaffolded but not implemented in this emulator — raised identically in all 7 checks before any kafkaconnect assertion is reached.

## Steps to Reproduce
Cause 1:
1. Boot the emulator and point a botocore kafkaconnect client at it.
2. kc.list_connectors(maxResults=10, nextToken="sim2real-not-a-real-pagination-token==")
3. Observe HTTP 400 / BadRequestException with message "The nextToken is not one this service issued. Correct your request and then retry it."; AWS's message names the rejected member (recorded: "Invalid parameter nextToken.").

Cause 2:
1. Boot the emulator; create an S3 bucket via the s3 client.
2. s3.put_object(Bucket=<bucket>, Key="plugin.zip", Body=<minimal zip bytes>)
3. Observe NotImplemented (scaffolded stub) instead of 200; the kafkaconnect custom-plugin checks therefore never issue CreateCustomPlugin.
