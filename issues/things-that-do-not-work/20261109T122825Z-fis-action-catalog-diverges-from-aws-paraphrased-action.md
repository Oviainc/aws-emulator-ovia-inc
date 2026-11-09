Issue-Source: trace-sweep-divergence
Issue-Trace: fis-catalog-list-actions
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: fis-catalog-list-actions fis-action-pagination-single-page-boundary
Issue-Shape-Id: com.amazonaws.fis#FaultInjectionSimulator
Issue-Key: fis|ListActions|2xx->2xx|$.actions[arn="arn:aws:fis:us-west-2:000000000000:action/aws:cloudwatch:assert-alarm-state"].description
Issue-Key: fis|ListActions|2xx->2xx|$.actions[arn="arn:aws:fis:us-west-2:000000000000:action/aws:directconnect:virtual-interface-disconnect"]
Issue-Key: fis|ListActions|2xx->2xx|$.actions[arn="arn:aws:fis:us-west-2:000000000000:action/aws:dsql:cluster-connection-failure"]
Issue-Key: fis|ListActions|2xx->2xx|$.actions[arn="arn:aws:fis:us-west-2:000000000000:action/aws:dynamodb:global-table-pause-replication"].description
Issue-Key: fis|ListActions|2xx->2xx|$.actions[arn="arn:aws:fis:us-west-2:000000000000:action/aws:ebs:pause-volume-io"].description
Issue-Key: fis|ListActions|2xx->2xx|$.actions[arn="arn:aws:fis:us-west-2:000000000000:action/aws:ebs:volume-io-latency"]
Issue-Key: fis|ListActions|2xx->2xx|$.actions[arn="arn:aws:fis:us-west-2:000000000000:action/aws:ec2:api-error"]
Issue-Key: fis|ListActions|2xx->2xx|$.actions[arn="arn:aws:fis:us-west-2:000000000000:action/aws:ec2:api-insufficient-instance-capacity-error"].description
Issue-Key: fis|ListActions|2xx->2xx|$.actions[arn="arn:aws:fis:us-west-2:000000000000:action/aws:ec2:asg-insufficient-instance-capacity-error"].description
Issue-Key: fis|ListActions|2xx->2xx|$.actions[arn="arn:aws:fis:us-west-2:000000000000:action/aws:ec2:send-spot-instance-interruptions"].description
Issue-Key: fis|ListActions|2xx->2xx|$.actions[arn="arn:aws:fis:us-west-2:000000000000:action/aws:ecs:drain-container-instances"].description
Issue-Key: fis|ListActions|2xx->2xx|$.actions[arn="arn:aws:fis:us-west-2:000000000000:action/aws:ecs:stop-task"].description
Issue-Key: fis|ListActions|2xx->2xx|$.actions[arn="arn:aws:fis:us-west-2:000000000000:action/aws:ecs:task-cpu-stress"].description
Issue-Key: fis|ListActions|2xx->2xx|$.actions[arn="arn:aws:fis:us-west-2:000000000000:action/aws:ecs:task-io-stress"].description
Issue-Key: fis|ListActions|2xx->2xx|$.actions[arn="arn:aws:fis:us-west-2:000000000000:action/aws:ecs:task-kill-process"].description
Issue-Key: fis|ListActions|2xx->2xx|$.actions[arn="arn:aws:fis:us-west-2:000000000000:action/aws:ecs:task-network-blackhole-port"].description
Issue-Key: fis|ListActions|2xx->2xx|$.actions[arn="arn:aws:fis:us-west-2:000000000000:action/aws:ecs:task-network-latency"].description
Issue-Key: fis|ListActions|2xx->2xx|$.actions[arn="arn:aws:fis:us-west-2:000000000000:action/aws:ecs:task-network-packet-loss"].description
Issue-Key: fis|ListActions|2xx->2xx|$.actions[arn="arn:aws:fis:us-west-2:000000000000:action/aws:eks:inject-kubernetes-custom-resource"].description
Issue-Key: fis|ListActions|2xx->2xx|$.actions[arn="arn:aws:fis:us-west-2:000000000000:action/aws:eks:pod-cpu-stress"].description
Issue-Key: fis|ListActions|2xx->2xx|$.actions[arn="arn:aws:fis:us-west-2:000000000000:action/aws:eks:pod-delete"].description
Issue-Key: fis|ListActions|2xx->2xx|$.actions[arn="arn:aws:fis:us-west-2:000000000000:action/aws:eks:pod-io-stress"].description
Issue-Key: fis|ListActions|2xx->2xx|$.actions[arn="arn:aws:fis:us-west-2:000000000000:action/aws:eks:pod-memory-stress"].description
Issue-Key: fis|ListActions|2xx->2xx|$.actions[arn="arn:aws:fis:us-west-2:000000000000:action/aws:eks:pod-network-blackhole-port"].description
Issue-Key: fis|ListActions|2xx->2xx|$.actions[arn="arn:aws:fis:us-west-2:000000000000:action/aws:eks:pod-network-latency"].description
Issue-Key: fis|ListActions|2xx->2xx|$.actions[arn="arn:aws:fis:us-west-2:000000000000:action/aws:eks:pod-network-packet-loss"].description
Issue-Key: fis|ListActions|2xx->2xx|$.actions[arn="arn:aws:fis:us-west-2:000000000000:action/aws:eks:terminate-nodegroup-instances"].description
Issue-Key: fis|ListActions|2xx->2xx|$.actions[arn="arn:aws:fis:us-west-2:000000000000:action/aws:elasticache:replicationgroup-interrupt-az-power"].description
Issue-Key: fis|ListActions|2xx->2xx|$.actions[arn="arn:aws:fis:us-west-2:000000000000:action/aws:fis:inject-api-internal-error"].description
Issue-Key: fis|ListActions|2xx->2xx|$.actions[arn="arn:aws:fis:us-west-2:000000000000:action/aws:fis:inject-api-throttle-error"].description
Issue-Key: fis|ListActions|2xx->2xx|$.actions[arn="arn:aws:fis:us-west-2:000000000000:action/aws:fis:inject-api-unavailable-error"].description
Issue-Key: fis|ListActions|2xx->2xx|$.actions[arn="arn:aws:fis:us-west-2:000000000000:action/aws:fis:wait"].description
Issue-Key: fis|ListActions|2xx->2xx|$.actions[arn="arn:aws:fis:us-west-2:000000000000:action/aws:kinesis:stream-expired-iterator-exception"]
Issue-Key: fis|ListActions|2xx->2xx|$.actions[arn="arn:aws:fis:us-west-2:000000000000:action/aws:kinesis:stream-provisioned-throughput-exception"]
Issue-Key: fis|ListActions|2xx->2xx|$.actions[arn="arn:aws:fis:us-west-2:000000000000:action/aws:lambda:invocation-add-delay"].description
Issue-Key: fis|ListActions|2xx->2xx|$.actions[arn="arn:aws:fis:us-west-2:000000000000:action/aws:lambda:invocation-error"].description
Issue-Key: fis|ListActions|2xx->2xx|$.actions[arn="arn:aws:fis:us-west-2:000000000000:action/aws:lambda:invocation-http-integration-response"].description
Issue-Key: fis|ListActions|2xx->2xx|$.actions[arn="arn:aws:fis:us-west-2:000000000000:action/aws:memorydb:multi-region-cluster-pause-replication"]
Issue-Key: fis|ListActions|2xx->2xx|$.actions[arn="arn:aws:fis:us-west-2:000000000000:action/aws:network:disrupt-connectivity"].description
Issue-Key: fis|ListActions|2xx->2xx|$.actions[arn="arn:aws:fis:us-west-2:000000000000:action/aws:network:disrupt-vpc-endpoint"]
Issue-Key: fis|ListActions|2xx->2xx|$.actions[arn="arn:aws:fis:us-west-2:000000000000:action/aws:network:route-table-disrupt-connectivity"]
Issue-Key: fis|ListActions|2xx->2xx|$.actions[arn="arn:aws:fis:us-west-2:000000000000:action/aws:network:route-table-disrupt-cross-region-connectivity"]
Issue-Key: fis|ListActions|2xx->2xx|$.actions[arn="arn:aws:fis:us-west-2:000000000000:action/aws:network:transit-gateway-disrupt-cross-region-connectivity"].description
Issue-Key: fis|ListActions|2xx->2xx|$.actions[arn="arn:aws:fis:us-west-2:000000000000:action/aws:rds:failover-db-cluster"].description
Issue-Key: fis|ListActions|2xx->2xx|$.actions[arn="arn:aws:fis:us-west-2:000000000000:action/aws:rds:reboot-db-instances"].description
Issue-Key: fis|ListActions|2xx->2xx|$.actions[arn="arn:aws:fis:us-west-2:000000000000:action/aws:s3:bucket-pause-replication"].description
Issue-Key: fis|ListActions|2xx->2xx|$.actions[arn="arn:aws:fis:us-west-2:000000000000:action/aws:ssm:send-command"].description
Issue-Commit: 3b4b4e2df28997ae7cbb85154b0c83cf06a43ef8

# FIS action catalog diverges from AWS: paraphrased action descriptions and missing actions in ListActions

> _Note: this issue was identified against the code base at commit `3b4b4e2df28997ae7cbb85154b0c83cf06a43ef8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListActions

## Description
The emulator serves its own hand-written seed table for the read-only FIS ACTION catalog that ListActions (and GetAction) report from. The response is HTTP 200 with a well-formed ActionSummaryList, but the catalog CONTENT is not AWS's: roughly forty entries carry a paraphrase instead of AWS's published description string, and about ten actions AWS publishes are absent from the table altogether. 2 causes.

Cause 1 of 2: every ListActions page reports paraphrased action descriptions instead of AWS's published text.
AWS: Smithy model services/service-com-amazonaws-fis-faultinjectionsimulator/smithy-model/com-amazonaws-fis-faultinjectionsimulator.json — ListActions GET /actions -> 200, output ListActionsResponse.actions: ActionSummaryList of ActionSummary{id, arn, description, targets, tags}; GetAction GET /actions/{id} -> 200 serves the same record (errors ValidationException 400, ResourceNotFoundException 404 for GetAction). The description text is catalog data the model does not carry, so this is recorded behavior only: each description is a FIXED AWS-published string keyed by the action id, and the only per-environment parts of an ActionSummary are the account id and the region embedded in its arn. The recording shows, for example, that the action id aws:cloudwatch:assert-alarm-state carries 'Asserts that the CloudWatch alarms are in the expected states.', aws:ebs:pause-volume-io carries 'Pauses IO for a set of EBS volumes' and aws:dynamodb:global-table-pause-replication carries 'Disrupts DynamoDB Global Table regional connectivity by pausing replication and blocking table operations in the affected region.'
Emulator: it returns its own re-worded summary for each of those ids — 'Verify that the specified alarms are in one of the specified alarm states.', 'Pause I/O operations on the specified EBS volumes.' and 'Pause replication of the specified DynamoDB global tables in this Region.' respectively — and the same paraphrasing affects roughly forty ids across the aws:ec2, aws:ecs, aws:eks, aws:elasticache, aws:fis, aws:lambda, aws:network, aws:rds, aws:s3 and aws:ssm families. Replay verdict: mismatch, expected_status 200, actual_status 200, value_mismatch at $.actions[arn=...].description.
Why: the action catalog is the discovery surface a client reads to pick, validate and present actions. A client that renders the catalog shows text no AWS document contains; a client, test, docs generator or golden/snapshot comparison that matches the description against AWS's published catalog finds no match and concludes the action is unrecognised. GetAction serves the same record, so the wrong text is returned by both paths.
Fix: the FIS action-catalog seed table the ListActions and GetAction handlers read from — the service's static catalog module (crate::state's ACTION_CATALOG, declared in services/service-com-amazonaws-fis-faultinjectionsimulator/src/state.rs and its catalog submodule), not the handler's serialization, which is already correct. Each action's description must be the AWS-published string for that action id.
Traces: fis-catalog-list-actions#task#1 (representative) and fis-action-pagination-single-page-boundary#task#2, which fails on the same record in the paginated path.

Cause 2 of 2: the action catalog is INCOMPLETE — actions AWS publishes are absent from ListActions entirely.
AWS: same model binding as Cause 1; membership of the catalog is likewise recorded behavior only. The recorded ListActions response enumerates every action AWS publishes for the region, including aws:directconnect:virtual-interface-disconnect ('Direct Connect Failover Test.', targets VirtualInterfaces/aws:directconnect:virtual-interface), aws:dsql:cluster-connection-failure ('Simulates connection failures to a DSQL cluster for the specified duration.', targets Clusters/aws:dsql:cluster), aws:ebs:volume-io-latency ('Sets latency to EBS volume I/O operations.'), aws:ec2:api-error, aws:kinesis:stream-expired-iterator-exception, aws:kinesis:stream-provisioned-throughput-exception, aws:memorydb:multi-region-cluster-pause-replication, aws:network:disrupt-vpc-endpoint, aws:network:route-table-disrupt-connectivity and aws:network:route-table-disrupt-cross-region-connectivity. Each entry's id, arn (region/account from the request environment), description, targets map and tags are the catalog's fixed data.
Emulator: it holds no record for those ids at all — the replay reports them as missing (expected present, got None) in the ListActions body — so the catalog it serves is about ten entries short of AWS's. Replay verdict: mismatch, expected_status 200, actual_status 200, missing at $.actions[arn="...action/aws:directconnect:virtual-interface-disconnect"] and the other ids named above.
Why: a client enumerates this catalog to decide which actions exist before building a template. An absent action makes a perfectly valid action id look unsupported: a chooser does not offer it, a validator rejects a CreateExperimentTemplate that names it, and GetAction on that id presumably answers ResourceNotFoundException (404) for an action AWS supports. The short table also changes the catalog's total size, so any consumer that pages the catalog or asserts its count sees a boundary AWS does not have.
Fix: the same static action-catalog table in crate::state — it must contain every action AWS publishes, each with its id, arn, description, targets map and tags.
Traces: fis-catalog-list-actions#task#1.

Relation to open issues: the open issue 20261106T163756Z-fis-built-in-catalog-data-diverges-from-aws.md describes the paraphrased-description fault and the incomplete TARGET-RESOURCE-TYPE catalog, and the replay reports the ListTargetResourceTypes cluster as owned by it — but it does NOT own the two ListActions clusters filed here (the replay reports owned_by empty for both), and it does not mention the MISSING action entries of Cause 2 at all. No other open FIS issue (20261024T000450Z, 20261029T183321Z, 20261104T032609Z, 20261107T200341Z, 20261108T211946Z) touches the read-only action catalog.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: 285 steps, 279 passed, 6 mismatched, 6 divergence clusters, every one service=fis; both clusters below have empty stub_path and empty owned_by.

Cause 1 — ListActions (GET /actions), fixture fis-catalog-list-actions step task#1; the same record also fails in fis-action-pagination-single-page-boundary step task#2.
  expected_status 200, actual_status 200, verdict mismatch
  $.actions[arn="arn:aws:fis:<region>:<account>:action/aws:cloudwatch:assert-alarm-state"].description
    expected: 'Asserts that the CloudWatch alarms are in the expected states.'
    actual:   'Verify that the specified alarms are in one of the specified alarm states.'
  $.actions[arn="...action/aws:ebs:pause-volume-io"].description
    expected: 'Pauses IO for a set of EBS volumes'
    actual:   'Pause I/O operations on the specified EBS volumes.'
  $.actions[arn="...action/aws:dynamodb:global-table-pause-replication"].description
    expected: 'Disrupts DynamoDB Global Table regional connectivity by pausing replication and blocking table operations in the affected region.'
    actual:   'Pause replication of the specified DynamoDB global tables in this Region.'
  plus the same value_mismatch on .description for ~40 further ids reported in the cluster key, across aws:ec2:* (api-insufficient-instance-capacity-error, asg-insufficient-instance-capacity-error, send-spot-instance-interruptions), aws:ecs:* (drain-container-instances, stop-task, task-cpu-stress, task-io-stress, task-kill-process, task-network-blackhole-port, task-network-latency, task-network-packet-loss), aws:eks:* (inject-kubernetes-custom-resource, pod-cpu-stress, pod-delete, pod-io-stress, pod-memory-stress, pod-network-blackhole-port, pod-network-latency, pod-network-packet-loss, terminate-nodegroup-instances), aws:elasticache:replicationgroup-interrupt-az-power, aws:fis:* (inject-api-internal-error, inject-api-throttle-error, inject-api-unavailable-error, wait), aws:lambda:* (invocation-add-delay, invocation-error, invocation-http-integration-response), aws:network:* (disrupt-connectivity, transit-gateway-disrupt-cross-region-connectivity), aws:rds:* (failover-db-cluster, reboot-db-instances), aws:s3:bucket-pause-replication and aws:ssm:send-command.
  (The account id and region in each arn are environment values used only to key the record; the description is the requirement.)

Cause 2 — same fixture/step. Entries reported missing (expected present in the body, got None):
  aws:directconnect:virtual-interface-disconnect — expected {id, arn, description 'Direct Connect Failover Test.', tags {}, targets {VirtualInterfaces: {resourceType 'aws:directconnect:virtual-interface'}}}
  aws:dsql:cluster-connection-failure — expected description 'Simulates connection failures to a DSQL cluster for the specified duration.', targets {Clusters: {resourceType 'aws:dsql:cluster'}}
  aws:ebs:volume-io-latency — expected description 'Sets latency to EBS volume I/O operations.'
  aws:ec2:api-error
  aws:kinesis:stream-expired-iterator-exception
  aws:kinesis:stream-provisioned-throughput-exception
  aws:memorydb:multi-region-cluster-pause-replication
  aws:network:disrupt-vpc-endpoint
  aws:network:route-table-disrupt-connectivity
  aws:network:route-table-disrupt-cross-region-connectivity

Model consulted (smithy_query, services/service-com-amazonaws-fis-faultinjectionsimulator/smithy-model/com-amazonaws-fis-faultinjectionsimulator.json):
  ListActions GET /actions -> 200, output.actions: ActionSummaryList; errors ValidationException(400)
  GetAction GET /actions/{id} -> 200 over the same record; errors ResourceNotFoundException(404), ValidationException(400)
The model carries neither the catalog's text nor its membership, so the recorded responses are the authority for both causes.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from this checkout.
2. Cause 1: GET /actions against the fis endpoint (ListActions; equivalently `aws fis list-actions`). Read the description of the entry whose arn ends in action/aws:cloudwatch:assert-alarm-state: AWS returns 'Asserts that the CloudWatch alarms are in the expected states.', the emulator returns 'Verify that the specified alarms are in one of the specified alarm states.'. Repeat for aws:ebs:pause-volume-io and aws:dynamodb:global-table-pause-replication. Then GET /actions/aws:cloudwatch:assert-alarm-state (GetAction) and observe the same wrong text on the single-record path.
3. Cause 2: in the same ListActions body, search for the ids aws:directconnect:virtual-interface-disconnect, aws:dsql:cluster-connection-failure, aws:ebs:volume-io-latency, aws:ec2:api-error, aws:kinesis:stream-expired-iterator-exception, aws:kinesis:stream-provisioned-throughput-exception, aws:memorydb:multi-region-cluster-pause-replication, aws:network:disrupt-vpc-endpoint, aws:network:route-table-disrupt-connectivity and aws:network:route-table-disrupt-cross-region-connectivity — none of them is present. Then GET /actions/aws:ec2:api-error and observe the emulator cannot resolve an action AWS publishes.
4. Re-replay fixture fis-catalog-list-actions step task#1 AND fixture fis-action-pagination-single-page-boundary step task#2 after the fix; both must pass.
