Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr.unsupported-fleet-instance-type-rejected
Issue-Shape-Id: com.amazonaws.emr#ElasticMapReduce
Issue-Key: emr.unsupported-fleet-instance-type-rejected|_rejection#7|AssertionError|raise AssertionError(
Issue-Commit: 20004884b35d4e30d0a51c1edcbd16198ad55508

# EMR RunJobFlow accepts an instance fleet naming an unsupported instance type, which AWS refuses with ValidationException "Instance type '<type>' is not supported."

> _Note: this issue was identified against the code base at commit `20004884b35d4e30d0a51c1edcbd16198ad55508`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunJobFlow

## Description
THIS ISSUE BUNDLES 1 CAUSE.

Cause 1 of 1: RunJobFlow does not validate the instance types named in an instance fleet's InstanceTypeConfigs, and in particular does not validate the PRIMARY (MASTER) fleet's types, so a fleet naming an instance type that does not exist is accepted and a cluster id is returned.

  AWS: 400 ValidationException with the message "Instance type 'm5.3xlarge' is not supported." EMR validates the instance type of EVERY instance fleet in the request (not only the CORE one) against the types the requested release label offers, and refuses the whole RunJobFlow call. The message's fixed text is "Instance type '" + "' is not supported."; the only variable part is the instance type value, taken verbatim from the offending Instances.InstanceFleets[].InstanceTypeConfigs[].InstanceType member of the request (here the nonexistent m5.3xlarge). Smithy: RunJobFlow's modelled error list carries only InternalServerError (httpError 500), so the 400 ValidationException envelope and this sentence are recorded behavior only; the members involved are RunJobFlow.Instances -> JobFlowInstancesConfig.InstanceFleets -> InstanceFleetConfig.InstanceTypeConfigs -> InstanceTypeConfig.InstanceType (XmlStringMaxLen256, required) and RunJobFlow.ReleaseLabel (XmlStringMaxLen256), which together decide whether the type is offered.

  Emulator: HTTP 200 with a JobFlowId (the replay observed cluster j-0000000000002) for exactly the same request shape, after it had already accepted the unmodified control request. No refusal is made on any fleet's InstanceTypeConfigs, so the cluster is created holding an instance type no EMR release offers.

  Why: a client that mistypes an instance type, or uses one its release label does not offer, is handed a cluster id and believes the cluster is provisioning. On AWS the call fails synchronously and the client can correct the request; against the emulator the fault is silent — capacity for that fleet can never materialise, and code that branches on a ValidationException from RunJobFlow (retry with a supported type, surface the typo to the user) is never exercised. Validation gates that depend on this refusal are also untestable.

  Fix: RunJobFlow's request validation — the instance-fleet branch of the handler (services/service-com-amazonaws-emr-elasticmapreduce/src/ops/run_job_flow.rs), which must check the instance type of every InstanceTypeConfigs entry of every fleet, including the MASTER/primary fleet, against the set of types the requested ReleaseLabel supports, and refuse through the service's 400 ValidationException channel with AWS's sentence before any cluster is stored.

  Traces: emr.unsupported-fleet-instance-type-rejected.

Relation to already-open issues: 20261102T094623Z-emr-runjobflow-accepts-five-cluster-shapes-aws-refuses.md and 20261105T093917Z-emr-runjobflow-accepts-eight-more-cluster-shapes-aws.md each bundle other RunJobFlow shapes AWS refuses (fleet capacity, markets, placement groups, nested classifications, multi-primary release gates, the five-instance-type-per-fleet limit) and own those checks' replay keys; none of them covers instance-type SUPPORT, and this check's replay key is carried by neither.

## Evidence
Replay of simparity-behavioral-aws-v1, heading '### emr', check [mismatch] emr.unsupported-fleet-instance-type-rejected:

  File "check_emr_unsupported_fleet_instance_type_rejected.py", line 31, in run
    code, message = _rejection(ctx, bad)
  File "check_emr_unsupported_fleet_instance_type_rejected.py", line 409, in _rejection
    raise AssertionError(
  AssertionError: RunJobFlow accepted the request (cluster j-0000000000002); real AWS rejects it

The check first sends an unmodified request (emr-5.36.1, a primary and a CORE instance fleet of one on-demand m5.2xlarge unit each) and asserts it is accepted and not terminal — that positive control passed, so the service role, instance profile and subnet are known good. It then re-sends the identical request with ONE field mutated: the primary fleet's InstanceTypeConfigs names 'm5.3xlarge' (an instance type that does not exist). Expected: ('ValidationException', "Instance type 'm5.3xlarge' is not supported."). Actual: HTTP 200 and a JobFlowId. (The cluster id in the quoted assertion is whatever the emulator generated for that run; it is not part of the requirement.)

## Steps to Reproduce
1. Clone the repo and let the behavioral suite build and boot the emulator.
2. Create an EMR service role and an EC2 instance profile, and a subnet in a VPC.
3. Call RunJobFlow with ReleaseLabel=emr-5.36.1 and Instances.InstanceFleets = [ {Name: 'MasterInstanceFleet', InstanceFleetType: 'MASTER', TargetOnDemandCapacity: 1, TargetSpotCapacity: 0, InstanceTypeConfigs: [{InstanceType: 'm5.2xlarge'}]}, {Name: 'CoreInstanceFleet', InstanceFleetType: 'CORE', ... 'm5.2xlarge'} ] — confirm it is accepted (the positive control).
4. Re-send the same request with the MASTER fleet's InstanceTypeConfigs changed to [{InstanceType: 'm5.3xlarge'}].
5. Expected: HTTP 400 ValidationException, message "Instance type 'm5.3xlarge' is not supported." Observed: HTTP 200 with a JobFlowId.
6. Re-replay emr.unsupported-fleet-instance-type-rejected to confirm the fix.
