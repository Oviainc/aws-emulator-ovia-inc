Issue-Source: trace-sweep-divergence
Issue-Trace: rds-global-cluster-lifecycle
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rds-global-cluster-lifecycle rds-event-subscription-duplicate-singular-exist rds-delete-groups-in-use-by-instance rds-option-group-lifecycle-batch-08
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|DescribeGlobalClusters|2xx->4xx|$.DescribeGlobalClustersResponse
Issue-Key: rds|DescribeGlobalClusters|2xx->4xx|status
Issue-Key: rds|DescribeEventSubscriptions|2xx->4xx|$.DescribeEventSubscriptionsResponse
Issue-Key: rds|DescribeEventSubscriptions|2xx->4xx|status
Issue-Key: rds|DeleteOptionGroup|2xx->4xx|status
Issue-Commit: 41eca1b5094053f7eb75d5bb9c5bde3b8db45df6

# RDS persisted resources are reported not found by later describe and delete calls

> _Note: this issue was identified against the code base at commit `41eca1b5094053f7eb75d5bb9c5bde3b8db45df6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeGlobalClusters

## Description
Resources the emulator has just created and acknowledged with a 200 are not visible to the next operation in the same fixture: the follow-up read or delete answers 404 with a not-found envelope. Three clusters, three resource kinds (global cluster, event subscription, option group). The shared fault is that the create path's state write is not visible to the lookup the later operation performs.

Cause 1 of 3: DescribeGlobalClusters cannot find a global cluster created in the preceding step.
AWS: Smithy: DescribeGlobalClusters -> GlobalClustersMessage {Marker, GlobalClusters}; the only modeled error is GlobalClusterNotFoundFault (errorKind client, httpError 404, awsQueryError code GlobalClusterNotFoundFault). AWS returns 200 with the GlobalClusterMember for a cluster that exists; 404 is reserved for an identifier that does not.
Emulator: returns HTTP 404 and an ErrorResponse where the recording has HTTP 200 and DescribeGlobalClustersResponse.DescribeGlobalClustersResult.GlobalClusters with the member (GlobalClusterIdentifier, GlobalClusterArn, Engine aurora-mysql, Status available, Endpoint, StorageEncrypted, DeletionProtection, EngineLifecycleSupport).
Why: a client that creates a global cluster and then polls it to 'available' never sees it; the poll fails and any wait-for-ready loop aborts with GlobalClusterNotFoundFault on a cluster it owns.
Fix: the global-cluster state the DescribeGlobalClusters handler looks up (the create path's write must be visible to it).
Traces: rds-global-cluster-lifecycle#task#2.

Cause 2 of 3: DescribeEventSubscriptions cannot find an event subscription created in the same fixture.
AWS: Smithy: DescribeEventSubscriptions -> EventSubscriptionsMessage {Marker, EventSubscriptionsList}; the only modeled error is SubscriptionNotFoundFault (client, httpError 404, awsQueryError code SubscriptionNotFound). AWS returns 200 with the EventSubscription for a subscription that exists.
Emulator: returns 404 where the recording has 200 and DescribeEventSubscriptionsResponse.DescribeEventSubscriptionsResult.EventSubscriptionsList.EventSubscription {EventSubscriptionArn, CustSubscriptionId, SnsTopicArn, SourceType db-instance, Enabled, CustomerAwsId, Status active}.
Why: a client that creates an event subscription and reads it back to confirm Status 'active' gets a not-found error, so the subscription appears never to have been made.
Fix: the event-subscription state the DescribeEventSubscriptions handler looks up.
Traces: rds-event-subscription-duplicate-singular-exist#task#4.

Cause 3 of 3: DeleteOptionGroup returns 404 for an option group that exists.
AWS: Smithy: DeleteOptionGroup -> empty output, errors InvalidOptionGroupStateFault (400) and OptionGroupNotFoundFault (client, httpError 404, awsQueryError code OptionGroupNotFoundFault). AWS returns 200 when the named group exists and is not in use.
Emulator: returns 404 where the recording has 200.
Why: a teardown or idempotent cleanup path that deletes an option group it created fails with OptionGroupNotFoundFault, so callers cannot reliably release the resource and a retry loop keeps failing.
Fix: the option-group state the DeleteOptionGroup handler looks up.
Traces: rds-delete-groups-in-use-by-instance#teardown#14 (representative) - the cluster also covers rds-option-group-lifecycle-batch-08#teardown#7, whose fixture has no blocked setup step, so this cause reproduces independently of any unimplemented prerequisite operation.

## Evidence
Cause 1: status expected 200, got 404; $.DescribeGlobalClustersResponse: missing (expected DescribeGlobalClustersResult.GlobalClusters.GlobalClusterMember {Engine: 'aurora-mysql', Status: 'available', ...}, got None). Cause 2: status expected 200, got 404; $.DescribeEventSubscriptionsResponse: missing (expected EventSubscriptionsList.EventSubscription {SourceType: 'db-instance', Status: 'active', ...}, got None). Cause 3: divergence_key service=rds|DeleteOptionGroup|status=200->404|keys=status, detail 'status: expected 200, got 404', affected_count 2.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Replay fixture rds-global-cluster-lifecycle through step task#2: CreateGlobalCluster succeeds, then Action=DescribeGlobalClusters for the same GlobalClusterIdentifier returns 404 instead of 200 with the member.
3. Replay rds-event-subscription-duplicate-singular-exist through task#4: the subscription is created, then DescribeEventSubscriptions for it returns 404.
4. Replay rds-delete-groups-in-use-by-instance#teardown#14 and rds-option-group-lifecycle-batch-08#teardown#7: DeleteOptionGroup on an existing group returns 404 instead of 200.
