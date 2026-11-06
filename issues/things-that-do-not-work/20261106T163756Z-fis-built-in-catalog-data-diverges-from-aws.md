Issue-Source: trace-sweep-divergence
Issue-Trace: fis-action-pagination-single-page-boundary
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: fis-action-pagination-single-page-boundary fis-catalog-list-actions fis-catalog-list-target-resource-types
Issue-Shape-Id: com.amazonaws.fis#FaultInjectionSimulator
Issue-Key: fis|ListActions|2xx->2xx|$.actions[arn="arn
Issue-Key: fis|ListTargetResourceTypes|2xx->2xx|$.targetResourceTypes[*]
Issue-Commit: 181e33813931a7f10d4bb8f5600a81e754f55b27

# FIS built-in catalog data diverges from AWS: wrong action/target-resource-type descriptions and missing target resource types

> _Note: this issue was identified against the code base at commit `181e33813931a7f10d4bb8f5600a81e754f55b27`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListActions

## Description
The emulator serves its own hand-written seed data for the two FIS read-only catalogs (the action catalog behind ListActions/GetAction and the target-resource-type catalog behind ListTargetResourceTypes/GetTargetResourceType). Both bodies return HTTP 200 with a well-formed shape, but the catalog CONTENT is not AWS's: descriptions are paraphrased rather than the strings AWS publishes, and the target-resource-type catalog is short several entries, which also shifts every later index of the list. 2 causes.

Cause 1 of 4 — numbering note: this bundle has exactly 2 causes, numbered 1 and 2 below; close both before submitting.

Cause 1 of 2: ListActions returns paraphrased action descriptions.
AWS: Smithy model services/service-com-amazonaws-fis-faultinjectionsimulator — ListActions GET /actions -> 200, output ListActionsResponse.actions: ActionSummaryList; each ActionSummary carries id/arn/description/targets/tags, and GetAction GET /actions/{id} returns the same description for the same action id. The description text itself is catalog data, not modeled, so this is recorded behavior only: the recording shows that for the action whose arn ends in action/aws:cloudwatch:assert-alarm-state AWS returns the description 'Asserts that the CloudWatch alarms are in the expected states.' Every entry of this catalog is a fixed AWS-published string keyed by the action id; the only per-environment variable parts of an ActionSummary are the account id and region embedded in its arn.
Emulator: for that same action it returns 'Verify that the specified alarms are in one of the specified alarm states.' — a paraphrase of the same concept with entirely different wording. Replay verdict: mismatch, expected_status 200, actual_status 200, $.actions[arn="arn:aws:fis:<region>:<account>:action/aws:cloudwatch:assert-alarm-state"].description value_mismatch.
Why: the action catalog is the discovery surface clients use to pick and present actions. A client that renders the catalog shows text no AWS document contains; a client (or test, or snapshot/golden comparison, or docs generator) that matches the description against AWS's published catalog finds no match and concludes the action is unavailable or unrecognised. Because GetAction serves the same record, the wrong text is returned by both paths.
Fix: the FIS action-catalog seed data that ListActions and GetAction read from — i.e. the service's static catalog/state module, not the handler's serialization, which is already correct. Each action's description must be the AWS-published string for that action id.
Traces: fis-action-pagination-single-page-boundary#task#2 (representative, the cluster's replay key); also fis-catalog-list-actions#task#1, which fails on the same record.

Cause 2 of 2: ListTargetResourceTypes returns an incomplete catalog with paraphrased descriptions, so the list diverges by both content and length.
AWS: Smithy model — ListTargetResourceTypes GET /targetResourceTypes -> 200, output ListTargetResourceTypesResponse.targetResourceTypes: TargetResourceTypeSummaryList of TargetResourceTypeSummary{resourceType, description}; GetTargetResourceType GET /targetResourceTypes/{resourceType} -> 200 or ResourceNotFoundException (404) for an unknown type. The membership and description text are catalog data, so recorded behavior only: the recording enumerates AWS's supported target resource types, including aws:kinesis:stream ('AWS Kinesis Data Stream target for the FIS actions.'), aws:memorydb:multi-region-cluster ('A MemoryDB multi-region cluster.') and aws:s3:bucket ('An Amazon S3 bucket'), and gives aws:ec2:vpc-endpoint the description 'A Virtual Private Cloud (VPC) endpoint.', aws:eks:pod 'A Kubernetes pod.', and aws:elasticache:replicationgroup 'An ElastiCache Replication Group.' Each description is fixed AWS text keyed by resourceType; nothing in a TargetResourceTypeSummary is per-request or per-account.
Emulator: it serves a shorter catalog with different text — aws:ec2:vpc-endpoint as 'An Amazon EC2 VPC Endpoint.', aws:eks:pod as 'An Amazon EKS pod.', aws:elasticache:replicationgroup as 'An Amazon ElastiCache replication group.' — and omits entries altogether (aws:kinesis:stream, aws:memorydb:multi-region-cluster and aws:s3:bucket are absent), so from the first omission onward every position of the list holds the wrong resource type and the list ends short (the replay reports position 23 as expected-present/got-nothing). Replay verdict: mismatch, expected_status 200, actual_status 200, with missing/extra reported at $.targetResourceTypes[10], [15], [16], [18], [19], [20], [21], [22], [23].
Why: this catalog is what tells a client which resource types FIS can target. A client that enumerates it to validate or to populate a chooser before calling CreateExperimentTemplate will reject a perfectly valid S3-bucket, Kinesis-stream or MemoryDB-multi-region-cluster target, and the shifted indices break any consumer that reads the list positionally or diffs it against AWS's. The paraphrased descriptions break catalog comparison the same way Cause 1 does, and GetTargetResourceType inherits both faults — an omitted type presumably answers ResourceNotFoundException for a type AWS supports.
Fix: the FIS target-resource-type catalog seed data read by ListTargetResourceTypes and GetTargetResourceType — the static catalog/state module. The catalog must contain every resource type AWS supports, in AWS's order, each with AWS's published description string.
Traces: fis-catalog-list-target-resource-types#task#1.

Neither cause is covered by an existing open FIS issue: the open FIS issues concern TargetAccountConfiguration validation, CloudFormation AWS::FIS::ExperimentTemplate provisioning, the inject-api privileges gate, and tag-identified target resolution in ListExperimentResolvedTargets (20261022T063355Z-..., 20261022T093447Z-..., 20261024T000450Z-..., 20261029T183321Z-..., 20261104T032609Z-...). None of them touches the read-only action or target-resource-type catalog.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: 279 steps, 276 passed, 3 mismatched, 2 divergence clusters, both service=fis, both with empty stub_path and empty owned_by.

Cause 1 — ListActions (GET /actions), fixture fis-action-pagination-single-page-boundary step task#2; same record also fails in fis-catalog-list-actions#task#1.
  expected_status 200, actual_status 200, verdict mismatch
  $.actions[arn="arn:aws:fis:us-west-2:000000000000:action/aws:cloudwatch:assert-alarm-state"].description: value_mismatch
    expected: 'Asserts that the CloudWatch alarms are in the expected states.'
    actual:   'Verify that the specified alarms are in one of the specified alarm states.'
  (The account id and region in the arn are environment values used only to key the record; the description is the requirement.)

Cause 2 — ListTargetResourceTypes (GET /targetResourceTypes), fixture fis-catalog-list-target-resource-types step task#1.
  expected_status 200, actual_status 200, verdict mismatch
  $.targetResourceTypes[10]: expected {resourceType 'aws:ec2:vpc-endpoint', description 'A Virtual Private Cloud (VPC) endpoint.'}, got description 'An Amazon EC2 VPC Endpoint.'
  $.targetResourceTypes[15]: expected {resourceType 'aws:eks:pod', description 'A Kubernetes pod.'}, got description 'An Amazon EKS pod.'
  $.targetResourceTypes[16]: expected {resourceType 'aws:elasticache:replicationgroup', description 'An ElastiCache Replication Group.'}, got description 'An Amazon ElastiCache replication group.'
  $.targetResourceTypes[18]: expected {resourceType 'aws:kinesis:stream', description 'AWS Kinesis Data Stream target for the FIS actions.'}, got {resourceType 'aws:lambda:function'} — aws:kinesis:stream absent, list shifted
  $.targetResourceTypes[19]: expected {resourceType 'aws:lambda:function', description 'A Lambda Function.'}, got {resourceType 'aws:rds:cluster'}
  $.targetResourceTypes[20]: expected {resourceType 'aws:memorydb:multi-region-cluster', description 'A MemoryDB multi-region cluster.'}, got {resourceType 'aws:rds:db'}
  $.targetResourceTypes[23]: expected {resourceType 'aws:s3:bucket', description 'An Amazon S3 bucket'}, got nothing — emulator list ends short
  (plus the matching 'extra' reports for the same positions at [21] and [22])

Model consulted (smithy_query, services/service-com-amazonaws-fis-faultinjectionsimulator/smithy-model/com-amazonaws-fis-faultinjectionsimulator.json):
  ListActions GET /actions -> 200, output.actions: ActionSummaryList; errors: ValidationException(400)
  ListTargetResourceTypes GET /targetResourceTypes -> 200, output.targetResourceTypes: TargetResourceTypeSummaryList; errors: ValidationException(400)
  GetAction GET /actions/{id} and GetTargetResourceType GET /targetResourceTypes/{resourceType} serve the same catalog records; errors: ResourceNotFoundException(404), ValidationException(400)
The model does not carry the catalog's text or membership, so the recorded responses are the authority for both.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from this checkout.
2. Cause 1: GET /actions on the fis endpoint (ListActions; equivalently `aws fis list-actions`). Find the entry whose arn ends in `action/aws:cloudwatch:assert-alarm-state` and read its `description`. Expect 'Asserts that the CloudWatch alarms are in the expected states.'; the emulator returns 'Verify that the specified alarms are in one of the specified alarm states.'. Also GET /actions/aws:cloudwatch:assert-alarm-state (GetAction) and observe the same wrong text. Replay fixture fis-action-pagination-single-page-boundary step task#2 (and fis-catalog-list-actions step task#1).
3. Cause 2: GET /targetResourceTypes (ListTargetResourceTypes; equivalently `aws fis list-target-resource-types`). Search the returned list for resourceType `aws:kinesis:stream`, `aws:memorydb:multi-region-cluster` and `aws:s3:bucket` — all three are absent. Compare the descriptions of `aws:ec2:vpc-endpoint`, `aws:eks:pod` and `aws:elasticache:replicationgroup` against AWS's published strings — all three are paraphrased. Then GET /targetResourceTypes/aws:s3:bucket (GetTargetResourceType) and observe it cannot resolve a type AWS supports. Replay fixture fis-catalog-list-target-resource-types step task#1.
4. Re-replay both fixtures named above after the fix; both must pass.
