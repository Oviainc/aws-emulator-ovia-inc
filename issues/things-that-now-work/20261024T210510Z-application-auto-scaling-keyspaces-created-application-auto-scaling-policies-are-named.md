Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: application-autoscaling.keyspaces-table-auto-scaling-specification-registers-the-targets-and-policies
Issue-Shape-Id: com.amazonaws.applicationautoscaling#AnyScaleFrontendService
Issue-Key: application-autoscaling.keyspaces-table-auto-scaling-specification-registers-the-targets-and-policies|run#21|AssertionError|assert (p['PolicyName'], p['PolicyType']) == (
Issue-Commit: 5184dca560a9e5da85ae3e6d8907d5afc24dbecc

# Keyspaces-created Application Auto Scaling policies are named with the DynamoDB-style '<MetricType>:<resourceId>' convention instead of '<keyspace>-<table>-scaling-policy'

> _Note: this issue was identified against the code base at commit `5184dca560a9e5da85ae3e6d8907d5afc24dbecc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeScalingPolicies

## Description
One cause.

Cause 1 of 1: the target-tracking scaling policies that Amazon Keyspaces registers with Application Auto Scaling as a side effect of CreateTable(autoScalingSpecification=...) come back from DescribeScalingPolicies under the wrong PolicyName. Failing check: application-auto-scaling.keyspaces-table-auto-scaling-specification-registers-the-targets-and-policies.

AWS: recorded behavior only — the Smithy model constrains ScalingPolicy.PolicyName only as a required string (@length 1..256, pattern ^\p{Print}+$) and states no naming convention (Smithy: ScalingPolicy.PolicyName -> com.amazonaws.applicationautoscaling#PolicyName; the model is silent on who mints the name). What the recording shows is that when Keyspaces auto-registers a table's read and write dimensions, each resulting policy is a TargetTrackingScaling policy whose PolicyName is derived from the Keyspaces resource it was created for: '<keyspaceName>-<tableName>-scaling-policy', the SAME name for both the cassandra:table:ReadCapacityUnits and the cassandra:table:WriteCapacityUnits dimension (the dimension, not the name, distinguishes them). The check's first two assertions already pass against the emulator, so the registration side of the integration is right: both cassandra targets exist with the requested MinCapacity/MaxCapacity under the AWSServiceRoleForApplicationAutoScaling_CassandraTable service-linked role, and exactly one policy exists per dimension. Only the minted name diverges.

Emulator: the policies are returned with the DynamoDB-style name the emulator uses for its own target-tracking policies — the predefined metric type joined to the resource id, e.g. PolicyName='CassandraReadCapacityUtilization:keyspace/<keyspace>/table/sim2real_t' for the read dimension. PolicyType is correct ('TargetTrackingScaling'). The replay assertion reads: AssertionError: the cassandra:table:ReadCapacityUnits policy is TargetTrackingScaling named 'CassandraReadCapacityUtilization:keyspace/<keyspace>/table/sim2real_t'.

Why: PolicyName is the policy's identity in this API, not decoration. A client that wants to inspect, re-put or delete the policy Keyspaces created addresses it by name (PutScalingPolicy/DeleteScalingPolicy/DescribeScalingPolicies PolicyNames=[...]), and infrastructure code and tests commonly compute that name from the keyspace and table rather than discovering it. Against the emulator every such lookup misses, DeleteScalingPolicy on the AWS name raises ObjectNotFoundException, and a PutScalingPolicy using the AWS name silently creates a THIRD policy on the same dimension instead of updating the existing one. The minted PolicyARN and the TargetTracking-* CloudWatch alarm names are derived from the same string, so they are wrong in the same way.

Fix: the Keyspaces->Application Auto Scaling auto-registration path — whatever mints the scalable targets and policies for CreateTable's autoScalingSpecification must name the policy after the registering resource ('<keyspace>-<table>-scaling-policy') rather than falling through to the generic '<PredefinedMetricType>:<ResourceId>' naming used for a client-issued PutScalingPolicy. The generic default is correct for a policy a client puts without a PolicyName; it must not be applied to the Keyspaces-minted ones. Note this is NOT covered by any open Application Auto Scaling issue: 20261022T182203Z-application-autoscaling-application-auto-scaling-minted-cloudwatch-alarms-scalable-target.md covers alarm minting and target representation for client-issued policies, and 20261024T115439Z-application-auto-scaling-has-no-enforcement-or-scheduling.md covers enforcement and scheduling, neither the name of a Keyspaces-minted policy.

Traces: application-auto-scaling.keyspaces-table-auto-scaling-specification-registers-the-targets-and-policies (assertion at check line 89, on the (PolicyName, PolicyType) pair for the read dimension).

## Evidence
Check: application-auto-scaling.keyspaces-table-auto-scaling-specification-registers-the-targets-and-policies [mismatch]

Setup (abridged): CreateKeyspace, then
  keyspaces.create_table(keyspaceName=<keyspace>, tableName='sim2real_t',
      capacitySpecification={'throughputMode':'PROVISIONED','readCapacityUnits':2,'writeCapacityUnits':2},
      autoScalingSpecification={
        'readCapacityAutoScaling':  {minimumUnits:2, maximumUnits:6, targetValue:50.0, disableScaleIn:True},
        'writeCapacityAutoScaling': {minimumUnits:2, maximumUnits:5, targetValue:70.0, disableScaleIn:False}})
then poll to ACTIVE and read
  application-autoscaling.describe_scaling_policies(ServiceNamespace='cassandra', ResourceId='keyspace/<keyspace>/table/sim2real_t')

Passed against the emulator (so these are NOT the divergence):
  describe_scalable_targets -> [('cassandra:table:ReadCapacityUnits', 2, 6, 'AWSServiceRoleForApplicationAutoScaling_CassandraTable'),
                                ('cassandra:table:WriteCapacityUnits', 2, 5, 'AWSServiceRoleForApplicationAutoScaling_CassandraTable')]
  describe_scaling_policies -> exactly one policy per dimension

Expected (AWS, recorded): for BOTH dimensions
  (PolicyName, PolicyType) == ('<keyspace>-sim2real_t-scaling-policy', 'TargetTrackingScaling')

Actual (emulator): for cassandra:table:ReadCapacityUnits
  (PolicyName, PolicyType) == ('CassandraReadCapacityUtilization:keyspace/<keyspace>/table/sim2real_t', 'TargetTrackingScaling')
  i.e. PolicyType correct, PolicyName minted as '<PredefinedMetricType>:<ResourceId>'.
  (the write dimension is reached only after the read assertion and is expected to diverge identically, as 'CassandraWriteCapacityUtilization:keyspace/<keyspace>/table/sim2real_t')

Replay output:
  File "check_..._keyspaces_table_auto_scaling_specification_registers_the_targets_and_policies.py", line 89, in run
    assert (p['PolicyName'], p['PolicyType']) == (f'{keyspace}-{table}-scaling-policy', 'TargetTrackingScaling')
  AssertionError: the cassandra:table:ReadCapacityUnits policy is TargetTrackingScaling named 'CassandraReadCapacityUtilization:keyspace/<keyspace>/table/sim2real_t'

No HTTP status or error code diverges here; both calls return 200. The divergence is in the value of the required ScalingPolicy.PolicyName member.

## Steps to Reproduce
1. ks = client('keyspaces'); aas = client('application-autoscaling')
2. ks.create_keyspace(keyspaceName=K) and poll get_keyspace until it is readable.
3. ks.create_table(keyspaceName=K, tableName='sim2real_t',
     schemaDefinition={'allColumns':[{'name':'pk','type':'text'}], 'partitionKeys':[{'name':'pk'}]},
     capacitySpecification={'throughputMode':'PROVISIONED','readCapacityUnits':2,'writeCapacityUnits':2},
     autoScalingSpecification={
       'readCapacityAutoScaling':  {'autoScalingDisabled':False,'minimumUnits':2,'maximumUnits':6,
          'scalingPolicy':{'targetTrackingScalingPolicyConfiguration':{'targetValue':50.0,'disableScaleIn':True}}},
       'writeCapacityAutoScaling': {'autoScalingDisabled':False,'minimumUnits':2,'maximumUnits':5,
          'scalingPolicy':{'targetTrackingScalingPolicyConfiguration':{'targetValue':70.0,'disableScaleIn':False}}}})
4. Poll ks.get_table until status == 'ACTIVE'.
5. aas.describe_scaling_policies(ServiceNamespace='cassandra', ResourceId=f'keyspace/{K}/table/sim2real_t')
6. Read PolicyName on each returned policy.
   AWS: both are f'{K}-sim2real_t-scaling-policy'.
   Emulator: 'CassandraReadCapacityUtilization:keyspace/{K}/table/sim2real_t' and the Write counterpart.
7. Secondary confirmation that the name is load-bearing: aas.delete_scaling_policy(ServiceNamespace='cassandra', ResourceId=f'keyspace/{K}/table/sim2real_t', ScalableDimension='cassandra:table:ReadCapacityUnits', PolicyName=f'{K}-sim2real_t-scaling-policy') succeeds on AWS and raises ObjectNotFoundException against the emulator.
