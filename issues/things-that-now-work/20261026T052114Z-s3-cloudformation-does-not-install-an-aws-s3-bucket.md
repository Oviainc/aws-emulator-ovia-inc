Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3.s3-analytics-xacct-e2e-cross-account-exports-lifecycle
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3.s3-analytics-xacct-e2e-cross-account-exports-lifecycle|phase#5|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Commit: cb933d25eac9e1275ee3ecb224ec471b338d603c

# CloudFormation does not install an AWS::S3::Bucket AnalyticsConfigurations property: after CREATE_COMPLETE ListBucketAnalyticsConfigurations reports an empty list

> _Note: this issue was identified against the code base at commit `cb933d25eac9e1275ee3ecb224ec471b338d603c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListBucketAnalyticsConfigurations

## Description
One net-new root cause in the S3 surface, grounded in one failing behavioral check.

Cause 1 of 1: an AWS::S3::Bucket that declares AnalyticsConfigurations is provisioned WITHOUT them. The stack reaches CREATE_COMPLETE and publishes the bucket name, but the S3 API reports the bucket as carrying no storage-class-analysis configuration at all, so the whole IaC -> API translation this scenario exists for is unobservable.

AWS: Smithy s3#ListBucketAnalyticsConfigurations is GET /{Bucket}?analytics&x-id=ListBucketAnalyticsConfigurations (HTTP 200, output xmlName ListBucketAnalyticsConfigurationResult) with members IsTruncated, ContinuationToken, NextContinuationToken and AnalyticsConfigurationList (xmlName AnalyticsConfiguration, flattened; up to 100 per page). Each member is s3#AnalyticsConfiguration { Id (required), Filter, StorageClassAnalysis (required) }; StorageClassAnalysis.DataExport is s3#StorageClassAnalysisDataExport { OutputSchemaVersion (required, enum V_1), Destination (required) }, and Destination is s3#AnalyticsExportDestination { S3BucketDestination (required) } = s3#AnalyticsS3BucketDestination { Format (required), BucketAccountId, Bucket (required — documented as 'The Amazon Resource Name (ARN) of the bucket to which data is exported'), Prefix }. CloudFormation is given the FLAT form on AWS::S3::Bucket: AnalyticsConfiguration[].StorageClassAnalysis.DataExport.Destination = { BucketArn, BucketAccountId, Format }, and the S3 API must report the same export one level deeper as Destination.S3BucketDestination with BucketArn renamed Bucket, with Ref AWS::AccountId resolved to the deploying account, and with an empty StorageClassAnalysis surfacing as { Id, StorageClassAnalysis: {} } (no DataExport member). On real AWS, a bucket created by that template lists exactly the declared Ids. The per-run values here (bucket name, the destination bucket ARN, the account ids) come from the request and the account; only the SHAPE and the set of declared Ids are the requirement.

Emulator: the stack reaches CREATE_COMPLETE and the AnalyticsBucketName output resolves, but ListBucketAnalyticsConfigurations on that bucket answers HTTP 200 with no AnalyticsConfigurationList entries. Replay detail: `AssertionError: [phase 1/4 deploy] the deployed bucket must carry exactly the three declared arms ['CrossAccountExport', 'NoExport', 'OwnerAccountExport']; the API reports []`. This is not an S3 data-plane stub: the S3 crate's PutBucketAnalyticsConfiguration, GetBucketAnalyticsConfiguration, DeleteBucketAnalyticsConfiguration and ListBucketAnalyticsConfigurations handlers are all implemented (their handler modules document the store they read and write, and the list call here returned a well-formed empty 200 rather than a refusal). The declared configurations are therefore dropped during stack provisioning and never written into the S3 analytics store.

Why: a client that manages storage-class analysis through IaC — which is the only way the upstream scenario configures it — gets a stack that reports success and a bucket with no analytics exports. Every downstream read fails silently: list-bucket-analytics-configurations returns nothing to filter, the JMESPath the upstream solution runs over StorageClassAnalysis.DataExport.Destination.S3BucketDestination.BucketAccountId selects nothing, and the cross-account export cannot be told from an owner-account export or from an analysis with no export. Because CREATE_COMPLETE is still reported, there is no failure signal at deploy time either, so the gap is only discovered by the consumer that cannot find the configuration.

Fix: the CloudFormation resource provisioner for AWS::S3::Bucket — translate the template's AnalyticsConfigurations list into the S3 analytics configuration store (one stored configuration per declared Id, mapping the flat DataExport.Destination { BucketArn, BucketAccountId, Format } onto Destination.S3BucketDestination { Bucket, BucketAccountId, Format }, resolving Ref AWS::AccountId, and storing an empty StorageClassAnalysis with no DataExport), so that S3's already-implemented Get/List/Delete analytics handlers report it, and so that deleting the three configurations returns the bucket to omitting AnalyticsConfigurationList. No change is needed in the S3 analytics handlers themselves.

Related (does NOT cover this): 20261024T033759Z-dynamodb-lambda-rekognition-s3-cloudformation-does-not-install-an-aws-s3-bucket.md reports the same pattern for the bucket's NotificationConfiguration sub-resource (no LambdaFunctionConfigurations after deploy); it names neither the AnalyticsConfigurations property nor this check id. 20261025T160428Z-s3-multipart-analytics-3-net-new-root-causes.md Cause 2 covers ListBucketAnalyticsConfigurations refusing an ABSENT bucket with NotImplemented instead of NoSuchBucket — a different leg, and in this replay the operation answered 200, not a refusal.

Traces: s3.s3-analytics-xacct-e2e-cross-account-exports-lifecycle (fails in its first phase, 'deploy').

## Evidence
Failing check: s3.s3-analytics-xacct-e2e-cross-account-exports-lifecycle (verdict: mismatch), phase 1/4 'deploy'.

Actual: `AssertionError: [phase 1/4 deploy] the deployed bucket must carry exactly the three declared arms ['CrossAccountExport', 'NoExport', 'OwnerAccountExport']; the API reports []` — ListBucketAnalyticsConfigurations answered HTTP 200 with no AnalyticsConfigurationList member/entries for the bucket the stack had just created (the stack itself reached CREATE_COMPLETE and published its AnalyticsBucketName output, so the check got past both of those assertions).

Expected (real AWS, same template): ListBucketAnalyticsConfigurations lists exactly the three declared configurations, with
- Id 'CrossAccountExport' -> StorageClassAnalysis = { DataExport: { OutputSchemaVersion: 'V_1', Destination: { S3BucketDestination: { Format: 'CSV', BucketAccountId: <the foreign account id given as a template parameter>, Bucket: <the destination bucket ARN given as a template parameter> } } } } — i.e. the template's flat { BucketArn, BucketAccountId, Format } reported nested with BucketArn renamed Bucket;
- Id 'OwnerAccountExport' -> the same shape with BucketAccountId equal to the deploying account (Ref AWS::AccountId resolved, as reported by sts:GetCallerIdentity);
- Id 'NoExport' -> { Id: 'NoExport', StorageClassAnalysis: {} } (an empty StorageClassAnalysis surfaces with no DataExport member).
And, in the check's later phases, after DeleteBucketAnalyticsConfiguration for all three ids the body must be exactly { IsTruncated: False } — AnalyticsConfigurationList OMITTED, not returned empty.

Smithy cited: s3#ListBucketAnalyticsConfigurations — GET /{Bucket}?analytics&x-id=ListBucketAnalyticsConfigurations, code 200, required httpLabel Bucket, httpQuery continuation-token, httpHeader x-amz-expected-bucket-owner; output ListBucketAnalyticsConfigurationsOutput (xmlName ListBucketAnalyticsConfigurationResult) { IsTruncated, ContinuationToken, NextContinuationToken, AnalyticsConfigurationList }; errors: []. s3#AnalyticsConfiguration { Id required, Filter, StorageClassAnalysis required }; s3#StorageClassAnalysisDataExport { OutputSchemaVersion required (V_1), Destination required }; s3#AnalyticsExportDestination { S3BucketDestination required }; s3#AnalyticsS3BucketDestination { Format required, BucketAccountId, Bucket required (the destination bucket ARN), Prefix }. The CloudFormation-side flat Destination shape and the CREATE_COMPLETE-then-listed behavior are recorded behavior from the check's own oracle (verified against real AWS and frozen), not modeled by the S3 Smithy model.

Emulator-side grounding that this is a provisioning gap and not an S3 stub: services/service-com-amazonaws-s3-amazons3/src/ops/put_bucket_analytics_configuration.rs and .../list_bucket_analytics_configurations.rs are both marked IMPLEMENTED (put records a configuration per id on an existing bucket; list reports them in ascending id order with paging, and answers NoSuchBucket/404 for a bucket the emulator does not hold), and the replayed list call returned a 200 empty listing rather than a refusal.

## Steps to Reproduce
1. Boot the emulator and re-replay the behavioral check s3.s3-analytics-xacct-e2e-cross-account-exports-lifecycle (the only id in trace_ids).

Manual repro:
2. CreateStack with a template declaring one AWS::S3::Bucket whose AnalyticsConfigurations property carries three entries:
   - Id 'CrossAccountExport' with StorageClassAnalysis.DataExport = { OutputSchemaVersion: 'V_1', Destination: { BucketArn: <some destination bucket ARN>, BucketAccountId: <a 12-digit foreign account id>, Format: 'CSV' } };
   - Id 'OwnerAccountExport' with the same shape but BucketAccountId: !Ref AWS::AccountId;
   - Id 'NoExport' with an empty StorageClassAnalysis.
   Export the bucket name as a stack Output.
3. Wait for CREATE_COMPLETE, read the bucket name from describe-stacks Outputs.
4. Call `aws s3api list-bucket-analytics-configurations --bucket <that bucket>`. Expected: three AnalyticsConfiguration entries with the ids above, the cross-account arm reporting Destination.S3BucketDestination { Format: 'CSV', BucketAccountId: <the foreign account>, Bucket: <the destination ARN> }, the owner arm reporting the deploying account from sts:GetCallerIdentity, and 'NoExport' reporting StorageClassAnalysis: {}. Actual: an empty listing.
5. Control, to show the S3 side works: PutBucketAnalyticsConfiguration the same three configurations directly on a hand-made bucket and list them back — they are reported, which isolates the loss to the AWS::S3::Bucket stack provisioner.
6. Then DeleteBucketAnalyticsConfiguration for each id and list again: the body must be exactly { IsTruncated: false } with AnalyticsConfigurationList omitted.
