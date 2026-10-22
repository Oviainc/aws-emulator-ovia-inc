Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__firehose__s3.deploy-cloudwatch-3-metric-stream-cfn-unlisted-namespace-blocked-until-update cloudwatch__firehose__s3.deploy-cloudwatch-3-metric-stream-cfn-delivers-put-metric-to-s3 cloudwatch__firehose__s3.deep-cloudwatch-3-cwstreamfmt-otel-rejects-average-additional-statistic
Issue-Key: cloudwatch|GetMetricStream|ResourceNotFoundException
Issue-Key: cloudwatch__firehose__s3.deploy-cloudwatch-3-metric-stream-cfn-delivers-put-metric-to-s3|_outputs#17|KeyError|return {o["OutputKey"]: o["OutputValue"] for o in st.get("Outputs", [])}
Issue-Key: cloudwatch__firehose__s3.deep-cloudwatch-3-cwstreamfmt-otel-rejects-average-additional-statistic|run#9|AssertionError|assert got_err == expected, (
Issue-Commit: 17f16131651e3a76635c083c093a8581359ec045

# CloudWatch metric streams: a CFN-declared AWS::CloudWatch::MetricStream is never created and stack Outputs carry no OutputValue (3 root causes)

> _Note: this issue was identified against the code base at commit `17f16131651e3a76635c083c093a8581359ec045`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStream

## Description
Three net-new root causes behind the cloudwatch__firehose__s3 failures that are not OWNED by an open issue (the other seven checks in this group fail on S3 ListObjectsV2(encoding-type), owned by 20261022T154846Z-emr-glue-s3-cross-service-suite-cfn-declared).

Cause 1 of 3: a CFN-declared AWS::CloudWatch::MetricStream is not created in CloudWatch.
AWS: CloudFormation creates the metric stream through CloudWatch, so after CREATE_COMPLETE cloudwatch.GetMetricStream(Name=<the stack's stream>) returns 200 with the template's FirehoseArn, RoleArn, OutputFormat and IncludeFilters/ExcludeFilters, and a stack UPDATE that adds a namespace to IncludeFilters is visible there. Smithy: GetMetricStream models ResourceNotFoundException only for a stream that does not exist.
Emulator: the stack reports success but GetMetricStream answers 400 ResourceNotFoundException (example message: 'the metric stream sim2real-xsvc-msfstream-08196ac275ed does not exist') — no metric stream exists in CloudWatch at all.
Why: CFN-based provisioning of metric streams is a no-op: a client cannot read back, update or stop the stream it declared, and the namespace-filter gating that the template expresses never takes effect.
Fix: stack-resource provisioning for AWS::CloudWatch::MetricStream must materialise the stream through the CloudWatch service's state (CloudFormation create_stack/update_stack resource dispatch).
Traces: cloudwatch__firehose__s3.deploy-cloudwatch-3-metric-stream-cfn-unlisted-namespace-blocked-until-update.

Cause 2 of 3: a CloudFormation stack's Outputs are returned without OutputValue.
AWS: DescribeStacks on a CREATE_COMPLETE stack returns each Output with OutputKey AND OutputValue (the resolved Ref/GetAtt — here the metric stream name, the Firehose stream and the bucket).
Emulator: the Output entries have no OutputValue key (KeyError: 'OutputValue'), so nothing the template deployed can be addressed afterwards.
Why: every deploy-then-use workflow aborts at its first step; the client cannot discover the physical names of the resources it just deployed.
Fix: Output resolution/serialization in the CloudFormation service crate's stack handling (describe_stacks and the create-stack output resolution).
Traces: cloudwatch__firehose__s3.deploy-cloudwatch-3-metric-stream-cfn-delivers-put-metric-to-s3.

Cause 3 of 3 (LOW): the AdditionalStatistics refusal message is re-worded. AWS: recorded behavior only (measured us-west-2) — PutMetricStream with OutputFormat=opentelemetry1.0 and AdditionalStatistics=['Average'] is refused 400 InvalidParameterValue with the fixed, request-parameterized sentence 'Unsupported statistic <s> for selected OutputFormat'. Emulator: same 400 and same InvalidParameterValue code, but the message is 'The additional statistic Average is not supported for metric streams with the output format opentelemetry1.0'. Why: a client matching on CloudWatch's message text for this refusal sees a different sentence; the status and code are correct, so this is the lowest-value cause in the bundle and is recorded for completeness of the replay list.
Fix: the AdditionalStatistics/OutputFormat validation message in the CloudWatch PutMetricStream handler.
Traces: cloudwatch__firehose__s3.deep-cloudwatch-3-cwstreamfmt-otel-rejects-average-additional-statistic.

## Evidence
Cause 1 — expected 200 from cloudwatch.GetMetricStream on the stack's stream; actual botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the GetMetricStream operation: the metric stream sim2real-xsvc-msfstream-08196ac275ed does not exist.
Cause 2 — expected DescribeStacks Outputs entries with OutputValue; actual KeyError: 'OutputValue'.
Cause 3 — expected ('InvalidParameterValue', 'Unsupported statistic Average for selected OutputFormat'); actual ('InvalidParameterValue', 'The additional statistic Average is not supported for metric streams with the output format opentelemetry1.0').

## Steps to Reproduce
1. Boot the emulator and point botocore at it.
2. Deploy a template declaring an S3 bucket, a Firehose delivery stream and an AWS::CloudWatch::MetricStream (IncludeFilters on one namespace), with Outputs Ref-ing them; wait for CREATE_COMPLETE.
3. Read DescribeStacks Outputs -> expect OutputKey+OutputValue per entry; observe no OutputValue.
4. cloudwatch.get_metric_stream(Name=<the stack's metric stream>) -> expect 200 with the template's configuration; observe 400 ResourceNotFoundException.
5. cloudwatch.put_metric_stream(OutputFormat='opentelemetry1.0', AdditionalStatistics=[{'Statistic':'Average', ...}], ...) -> expect 400 InvalidParameterValue 'Unsupported statistic Average for selected OutputFormat'; observe the re-worded message.
6. Re-replay the check ids in trace_ids.
