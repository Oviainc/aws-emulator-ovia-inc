Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue__kinesis-analytics-v2.dropping-the-catalog-database-strands-the-notebook-until-it-is-recreated glue__kinesis-analytics-v2.studio-notebook-naming-a-catalog-database-that-does-not-exist-rejected
Issue-Key: glue__kinesis-analytics-v2|CreateApplication|AmbiguousService
Issue-Commit: 9979b14c0e509985792db6dac0171a1eff5d6a10

# Cross-service Glue + Kinesis Analytics V2 Studio-notebook checks never run: router refuses every 'kinesisanalytics'-signed request with AmbiguousService

> _Note: this issue was identified against the code base at commit `9979b14c0e509985792db6dac0171a1eff5d6a10`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApplication

## Description
Cause 1 of 1: the cross-service (Glue + Kinesis Analytics V2) Studio-notebook behavioral checks die on their very first call, kinesisanalyticsv2:CreateApplication, because the emulator's request router resolves the target service by sigv4 signing name / endpoint prefix alone. Two emulated services claim the name 'kinesisanalytics' (the 2015-08-14 v1 API and the 2018-05-23 v2 API), so the router refuses the request instead of dispatching it. Nothing after that first call is exercised, so the referential behavior these checks exist to verify (Glue DeleteDatabase/GetDatabase against a notebook's catalog, and UpdateApplication re-resolving a dangling DatabaseARN) is never reached. Failing check ids: glue__kinesis-analytics-v2.dropping-the-catalog-database-strands-the-notebook-until-it-is-recreated, glue__kinesis-analytics-v2.studio-notebook-naming-a-catalog-database-that-does-not-exist-rejected.

AWS: Smithy services/service-com-amazonaws-kinesisanalyticsv2-kinesisanalytics-20180523/smithy-model/com-amazonaws-kinesisanalyticsv2-kinesisanalytics-20180523.json declares service KinesisAnalytics_20180523, version 2018-05-23, protocol awsJson1_1, awsService.endpointPrefix 'kinesisanalytics', arnNamespace 'kinesisanalytics', sigv4Name 'kinesisanalytics'. The v1 model (service-com-amazonaws-kinesisanalytics-kinesisanalytics-20150814, KinesisAnalytics_20150814) declares the IDENTICAL endpointPrefix and sigv4Name. Real AWS serves both API versions behind that one endpoint and signing name and disambiguates them by the awsJson1_1 X-Amz-Target service-shape prefix (KinesisAnalytics_20180523.<Operation> vs KinesisAnalytics_20150814.<Operation>), so a shared signing name is normal and is not an error condition. CreateApplication is modeled to answer HTTP 200 with a required ApplicationDetail, or one of the modeled client errors (CodeValidationException, InvalidArgumentException, InvalidRequestException, LimitExceededException, ResourceInUseException, TooManyTagsException, UnsupportedOperationException at 400; ConcurrentModificationException at 409). 'AmbiguousService' is not a modeled error of this service and does not exist in AWS.

Emulator: every request signed for 'kinesisanalytics' is rejected with error code AmbiguousService; the recorded message (example only) is "2 emulated services all claim requests for 'kinesisanalytics': kinesisanalytics, kinesisanalytics; the request is rejected rather than served by an arbitrary one". botocore retried to its ceiling ('reached max retries: 3'), i.e. the response is classified as retryable/server-side rather than as a modeled client error.

Why: a client cannot create or operate a Kinesis Analytics V2 Studio notebook against this emulator at all, so no workflow that combines Glue Data Catalog state with a Studio notebook can run. The caller sees a non-AWS error code that the SDK retries and then surfaces as an unknown fault, instead of an ApplicationDetail or a modeled 400/409. In particular the catalog referential rules these checks assert — DeleteDatabase succeeding while a notebook names the database, the notebook continuing to describe the now-dangling DatabaseARN, UpdateApplication being refused with InvalidArgumentException until the database is re-created, and the application version / Zeppelin log level staying put across the refused update — remain entirely unverified.

Fix: the shared request router / service-resolution layer. For awsJson1_1 services it must dispatch on the X-Amz-Target service-shape prefix (KinesisAnalytics_20180523 vs KinesisAnalytics_20150814) rather than on the sigv4 signing name or endpoint prefix, and must never answer AmbiguousService for a signing name AWS legitimately shares across API versions. Note: open issue 20261022T071206Z-kinesis-analytics-v2-is-entirely-unreachable-router-refuses.md reports the same root cause but carries only the three single-service kinesis-analytics-v2 replay keys; the two cross-service glue__kinesis-analytics-v2 keys below are not covered by it, so they are filed here.

Traces: glue__kinesis-analytics-v2.dropping-the-catalog-database-strands-the-notebook-until-it-is-recreated, glue__kinesis-analytics-v2.studio-notebook-naming-a-catalog-database-that-does-not-exist-rejected.

## Evidence
Expected (AWS, per Smithy KinesisAnalytics_20180523, awsJson1_1): CreateApplication returns HTTP 200 with ApplicationDetail, or a modeled client error (InvalidArgumentException / InvalidRequestException / ResourceInUseException / LimitExceededException / CodeValidationException / TooManyTagsException / UnsupportedOperationException at 400, ConcurrentModificationException at 409). Dispatch is by X-Amz-Target prefix, so sharing the 'kinesisanalytics' signing name with the 2015-08-14 API is expected and harmless.

Actual (emulator replay, both cross-service checks, identical): botocore.exceptions.ClientError: An error occurred (AmbiguousService) when calling the CreateApplication operation (reached max retries: 3): 2 emulated services all claim requests for 'kinesisanalytics': kinesisanalytics, kinesisanalytics; the request is rejected rather than served by an arbitrary one.

Per-service pass rate in the suite report: glue__kinesis-analytics-v2 0/2 (0%), both verdict 'error'. Neither check reached any Glue call, so no Glue-side assertion was evaluated.

## Steps to Reproduce
1. Boot the emulator from mainline.
2. Create a Studio execution role via IAM and a Glue database via glue.create_database(DatabaseInput={'Name': <db>}).
3. Point a botocore client at the emulator: kda = boto3.client('kinesisanalyticsv2', endpoint_url=<emulator>).
4. Call kda.create_application(ApplicationName=<app>, RuntimeEnvironment='ZEPPELIN-FLINK-3_0', ApplicationMode='INTERACTIVE', ServiceExecutionRole=<role arn>, ApplicationConfiguration={'ZeppelinApplicationConfiguration': {'CatalogConfiguration': {'GlueDataCatalogConfiguration': {'DatabaseARN': <db arn>}}}}).
5. Observe error code AmbiguousService naming a 'kinesisanalytics' collision, retried to the botocore ceiling, instead of an ApplicationDetail or a modeled 400/409 error. Any other kinesisanalyticsv2 operation (describe_application, update_application) reproduces it identically.
6. Replay glue__kinesis-analytics-v2.dropping-the-catalog-database-strands-the-notebook-until-it-is-recreated and glue__kinesis-analytics-v2.studio-notebook-naming-a-catalog-database-that-does-not-exist-rejected to confirm the fix; both must then proceed past CreateApplication and exercise the Glue catalog assertions.
