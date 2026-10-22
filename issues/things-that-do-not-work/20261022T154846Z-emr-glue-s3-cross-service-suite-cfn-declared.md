Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr__glue__s3.cfn-deploy-emr-cluster-glue-catalog-config-positive emr__glue__s3.cfn-deploy-emr-cluster-s3-logging-stack-positive emr__glue__s3.cfn-deploy-glue-crawler-s3-target-stack-positive emr__glue__s3.cfn-deploy-glue-crawler-invalid-s3-path-negative emr__glue__s3.cfn-deploy-glue-job-s3-script-location-positive emr__glue__s3.cfn-deploy-glue-job-role-missing-s3-access-negative emr__glue__s3.cfn-deploy-glue-database-table-s3-stack-positive emr__glue__s3.glue-etl-job-reads-s3-csv-source-positive emr__glue__s3.glue-etl-job-writes-parquet-to-s3-positive emr__glue__s3.glue-job-bookmark-persists-processed-s3-keys-positive emr__glue__s3.glue-job-missing-s3-getobject-source-rejected emr__glue__s3.glue-job-role-cross-account-trust-mismatch-rejected emr__glue__s3.glue-job-start-with-role-missing-glue-trust-rejected emr__glue__s3.iam-role-trust-glue-service-assumed-successfully-positive emr__glue__s3.s3-batch-operation-role-missing-s3-trust-rejected emr__glue__s3.eventbridge-glue-job-state-change-triggers-rule-positive emr__glue__s3.eventbridge-rule-missing-target-permission-negative emr__glue__s3.cli-glue-job-create-start-monitor-s3-output-workflow emr__glue__s3.glue-connection-uses-role-with-glue-trust-positive emr__glue__s3.cfn-deploy-emr-step-s3-bootstrap-positive
Issue-Key: emr__glue__s3|DescribeCluster|InvalidRequestException
Issue-Key: emr__glue__s3|PutObject|NoSuchBucket
Issue-Key: s3|HeadBucket|400
Issue-Key: emr__glue__s3|CreateJob|NotImplemented
Issue-Key: emr__glue__s3.cli-glue-job-create-start-monitor-s3-output-workflow|_run_shell#13|AssertionError|raise AssertionError(
Issue-Key: emr__glue__s3|CreateConnection|NotImplemented
Issue-Key: s3|ListObjectsV2|NotImplemented
Issue-Commit: db1fbaa8559703996072fdf323a0d84f8f863f33

# EMR/Glue/S3 cross-service suite: CFN-declared EMR clusters and S3 buckets are never created in their services, and Glue CreateJob/CreateConnection plus S3 ListObjectsV2(encoding-type) answer 501 (5 causes)

> _Note: this issue was identified against the code base at commit `db1fbaa8559703996072fdf323a0d84f8f863f33`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateJob

## Description
Five root causes behind the emr__glue__s3 failures that are not already filed. Causes 1-2 are the CloudFormation provisioning gap seen from the consuming service's side; causes 3-5 are scaffold stubs the replay report did not mark. Note for triage: the open issue 20261022T131025Z-glue-createjob-is-a-501-stub-gettable-ignores already names Glue CreateJob as a 501 stub for the glue suite; cause 3 is recorded here because these emr__glue__s3 checks are not on that issue's replay list.

Cause 1 of 5: a CFN-declared AWS::EMR::Cluster is not created in EMR — its physical id is a stack-derived name that DescribeCluster refuses.
AWS: CloudFormation creates the cluster through EMR, so Ref of an AWS::EMR::Cluster resource is a j-<13 upper-alnum> job-flow id and emr.DescribeCluster(ClusterId=that id) returns 200 with the cluster (including the Configurations the template declared). Smithy: DescribeCluster models InvalidRequestException (400) only for an id that is not valid or does not exist.
Emulator: the stack reports success but the cluster id handed back is the stack-derived name (e.g. 'sim2real-xsvc-emrglue-<hex>'), and DescribeCluster on it answers 400 InvalidRequestException "Cluster id '<name>' is not valid or does not exist." — no cluster exists in EMR at all.
Why: every deploy-then-inspect workflow breaks: a client cannot read back the hive-site/spark-hive-site Configurations it deployed, cannot poll the cluster, and cannot terminate it.
Fix: stack-resource provisioning in services/service-com-amazonaws-cloudformation-cloudformation/src/ops/create_stack.rs — AWS::EMR::Cluster must be materialised through the EMR service's state so its real j- id is the physical id.
Traces: emr__glue__s3.cfn-deploy-emr-cluster-glue-catalog-config-positive, emr__glue__s3.cfn-deploy-emr-cluster-s3-logging-stack-positive.

Cause 2 of 5: a CFN-declared AWS::S3::Bucket is not created in S3, and the name it reports is not a legal bucket name.
AWS: CloudFormation creates the bucket in S3; Ref of an AWS::S3::Bucket with no explicit BucketName is a generated DNS-compliant lowercase name ('<stack>-<logicalid>-<lowercase suffix>'), the bucket exists (HeadBucket 200) and PutObject into it succeeds.
Emulator: PutObject into the stack's bucket answers 404 NoSuchBucket, and for one stack HeadBucket answers HTTP 400 Bad Request — the reported name is not even a valid bucket name (it carries the logical id's upper-case characters), so S3 rejects the request before the lookup.
Why: no CFN-deployed data path works: scripts, ETL sources and crawler targets cannot be staged into the bucket the same template declared, so every deploy-then-use workflow aborts on its first S3 write.
Fix: same provisioning seam as cause 1 — AWS::S3::Bucket must be created through the S3 service's state, and the generated physical name must satisfy S3's bucket-name rules (lowercase, DNS-compliant).
Traces: emr__glue__s3.cfn-deploy-glue-crawler-s3-target-stack-positive, emr__glue__s3.cfn-deploy-glue-crawler-invalid-s3-path-negative, emr__glue__s3.cfn-deploy-glue-job-s3-script-location-positive, emr__glue__s3.cfn-deploy-glue-job-role-missing-s3-access-negative, emr__glue__s3.cfn-deploy-glue-database-table-s3-stack-positive (HeadBucket 400).

Cause 3 of 5: Glue CreateJob answers 501 NotImplemented.
AWS: glue CreateJob returns 200 with {Name} for a valid Command/Role, and refuses duplicates with AlreadyExistsException and bad input with InvalidInputException (400). Recorded behavior in these checks: CreateJob succeeds and StartJobRun then drives the job to a terminal state.
Emulator: 501, surfacing as botocore ClientError 'NotImplemented' ("operation CreateJob is scaffolded but not implemented in this emulator"), including through the AWS CLI path ("aws: [ERROR]: An error occurred (NotImplemented) ...").
Why: the entire Glue ETL surface is unreachable — job creation, job runs, bookmarks, role-trust refusals and the EventBridge job-state-change events that depend on a run all abort on the first call.
Fix: the scaffold stub services/service-com-amazonaws-glue-awsglue/src/ops/create_job.rs.
Traces: emr__glue__s3.glue-etl-job-reads-s3-csv-source-positive, emr__glue__s3.glue-etl-job-writes-parquet-to-s3-positive, emr__glue__s3.glue-job-bookmark-persists-processed-s3-keys-positive, emr__glue__s3.glue-job-missing-s3-getobject-source-rejected, emr__glue__s3.glue-job-role-cross-account-trust-mismatch-rejected, emr__glue__s3.glue-job-start-with-role-missing-glue-trust-rejected, emr__glue__s3.iam-role-trust-glue-service-assumed-successfully-positive, emr__glue__s3.s3-batch-operation-role-missing-s3-trust-rejected, emr__glue__s3.eventbridge-glue-job-state-change-triggers-rule-positive, emr__glue__s3.eventbridge-rule-missing-target-permission-negative, emr__glue__s3.cli-glue-job-create-start-monitor-s3-output-workflow.

Cause 4 of 5: Glue CreateConnection answers 501 NotImplemented.
AWS: glue CreateConnection returns 200 (empty body) for a valid ConnectionInput, and refuses with AlreadyExistsException / InvalidInputException (400).
Emulator: 501 'NotImplemented'.
Why: a client cannot register a Glue connection, so every workflow that binds a job or crawler to a connection (and the role-trust assertion on it) is unreachable.
Fix: the scaffold stub services/service-com-amazonaws-glue-awsglue/src/ops/create_connection.rs.
Traces: emr__glue__s3.glue-connection-uses-role-with-glue-trust-positive.

Cause 5 of 5: S3 ListObjectsV2 answers 501 when the request carries encoding-type.
AWS: ListObjectsV2 accepts EncodingType=url and returns 200 with Contents whose Key values are URL-encoded; the parameter is optional and never changes the operation's availability.
Emulator: "operation ListObjectsV2 with encoding-type is scaffolded but not implemented in this emulator" — a 501 for the same operation that works without the parameter, i.e. a partially implemented handler.
Why: every paginated listing issued by boto3's paginator or the AWS CLI (both of which send encoding-type=url by default) fails, so object enumeration is unusable from ordinary clients even though plain ListObjectsV2 works.
Fix: the encoding-type branch of services/service-com-amazonaws-s3-amazons3/src/ops/list_objects_v2.rs.
Traces: emr__glue__s3.cfn-deploy-emr-step-s3-bootstrap-positive.

## Evidence
Cause 1 — expected 200 from emr.DescribeCluster on the stack's cluster; actual botocore.errorfactory.InvalidRequestException: An error occurred (InvalidRequestException) when calling the DescribeCluster operation: Cluster id 'sim2real-xsvc-emrglue-37e1144fba39' is not valid or does not exist. (also 'sim2real-xsvc-emrlog-d109eb43834c').
Cause 2 — expected 200 from PutObject/HeadBucket on the stack's bucket; actual botocore.errorfactory.NoSuchBucket: An error occurred (NoSuchBucket) when calling the PutObject operation: Unknown (4 checks), and botocore.exceptions.ClientError: An error occurred (400) when calling the HeadBucket operation: Bad Request (1 check).
Cause 3 — expected 200 {Name}; actual An error occurred (NotImplemented) when calling the CreateJob operation: operation CreateJob is scaffolded but not implemented in this emulator (11 checks, one of them via the CLI).
Cause 4 — expected 200; actual An error occurred (NotImplemented) when calling the CreateConnection operation: operation CreateConnection is scaffolded but not implemented in this emulator.
Cause 5 — expected 200 with URL-encoded keys; actual An error occurred (NotImplemented) when calling the ListObjectsV2 operation: operation ListObjectsV2 with encoding-type is scaffolded but not implemented in this emulator.

## Steps to Reproduce
1. Boot the emulator and point botocore at it.
2. cfn = client('cloudformation'); deploy a stack with an AWS::S3::Bucket and an AWS::EMR::Cluster; wait for CREATE_COMPLETE.
3. s3.put_object into the stack's bucket -> expect 200; observe 404 NoSuchBucket. s3.head_bucket on it -> observe 400 Bad Request.
4. emr.describe_cluster(ClusterId=<stack's cluster id>) -> expect 200; observe 400 InvalidRequestException 'not valid or does not exist'.
5. glue.create_job(Name='j', Role=<role>, Command={'Name':'glueetl','ScriptLocation':'s3://b/s.py'}) -> expect 200; observe 501 NotImplemented. Same for glue.create_connection.
6. s3.get_paginator('list_objects_v2').paginate(Bucket=b) (sends encoding-type=url) -> expect 200 pages; observe 501 NotImplemented.
7. Re-replay the check ids in trace_ids.
