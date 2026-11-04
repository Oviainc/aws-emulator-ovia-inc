Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: config.e2e-remediation-auto-execution-consequence config.remediation-exceptions-lifecycle
Issue-Shape-Id: com.amazonaws.configservice#StarlingDoveService
Issue-Key: config.e2e-remediation-auto-execution-consequence|_poll_until#22|TimeoutError|raise TimeoutError("the polled condition: the run budget ended first; not graded")
Issue-Key: config.remediation-exceptions-lifecycle|_wait_discovered#11|TimeoutError|raise TimeoutError(f"Config discovering {resource_id}: the run budget ended first; not graded")
Issue-Commit: 44f88a38fb8560ba6fbbf2b389df854ff00d57e4

# config: a running recorder never discovers an EC2 instance, so ListDiscoveredResources stays empty forever and every remediation flow that starts from Config's inventory cannot begin

> _Note: this issue was identified against the code base at commit `44f88a38fb8560ba6fbbf2b389df854ff00d57e4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListDiscoveredResources

## Description
ONE CAUSE (1 of 1). Service token taken from the suite heading `### config` (the configservice crate is bootstrapped: services/service-com-amazonaws-configservice-starlingdoveservice, and it answers 111/161 checks in this run).

Cause 1 of 1: with a customer-managed configuration recorder in state RECORDING (allSupported recording group) and a delivery channel in place, Config's resource inventory never acquires an EC2 instance that exists in the emulated EC2 service, so ListDiscoveredResources(resourceType='AWS::EC2::Instance', resourceIds=[<instance id>]) answers an empty resourceIdentifiers list indefinitely. Failing checks: config.e2e-remediation-auto-execution-consequence, config.remediation-exceptions-lifecycle.

AWS: ListDiscoveredResources is modelled (Smithy, configservice/StarlingDoveService) with required input member resourceType (enum ResourceType), optional resourceIds/resourceName/limit (range 0-100)/includeDeletedResources/nextToken, output resourceIdentifiers (ResourceIdentifierList) + nextToken, errors InvalidLimitException / InvalidNextTokenException / NoAvailableConfigurationRecorderException / ValidationException (all client, HTTP 400). Its documentation states: "The results consist of resources that Config has discovered, including those that Config is not currently recording. You can narrow the results to include only resources that have specific resource IDs or a resource name." So on AWS, once a recorder whose recordingGroup covers AWS::EC2::Instance is recording, an instance that was launched (and is running, then stopped) is discovered asynchronously and ListDiscoveredResources returns one ResourceIdentifier for it (resourceType AWS::EC2::Instance, the instance's resourceId, resourceName when available) within a couple of minutes; recorded behaviour of these two checks is exactly that — they poll this call for up to 180 s as the precondition for the remediation surface, because StartRemediationExecution/PutRemediationConfigurations refuse a resource key Config has not discovered.

Emulator: the poll never succeeds. Both checks create the recorder and delivery channel, launch an EC2 instance through the emulated EC2 service, wait for it to reach running (and in one case stopped — so EC2 answers correctly), and then loop on ListDiscoveredResources for that exact resourceType/resourceId; the call keeps returning resourceIdentifiers = [] (no error — HTTP 200 with an empty list) until the poll is abandoned. Replay output:
  [error] config.e2e-remediation-auto-execution-consequence — check_config_e2e_remediation_auto_execution_consequence.py line 77 `_poll_until(lambda: config.list_discovered_resources(resourceType="AWS::EC2::Instance", resourceIds=[instance_id])["resourceIdentifiers"], lambda ids: bool(ids), timeout_s=180)` -> TimeoutError.
  [error] config.remediation-exceptions-lifecycle — check_config_remediation_exceptions_lifecycle.py line 36 `_wait_discovered(config, "AWS::EC2::Instance", instance_id, timeout_s=_budget_s(ctx, 180.0))`, whose body is the same ListDiscoveredResources poll (line 183) -> TimeoutError: "Config discovering i-00000000000000001: the run budget ended first; not graded".
Honest caveat about the verdict text: both polls were cut off by the harness run budget rather than by their own 180 s deadline, so the harness labels them "not graded"; what IS grounded is the observed emulator behaviour during the poll — every ListDiscoveredResources call for a live instance under a recording recorder returned an empty list, and no later assertion of either check was reached. Related open issues describe the same recording gap on a different surface and do NOT carry these two checks' replay keys: 20261025T144945Z-config-the-configuration-recorder-never-records-resources-from.md (SQS queues, read through GetResourceConfigHistory/BatchGetResourceConfig/SelectResourceConfig) and 20261029T184030Z-config-sqs-a-running-allsupported-recorder-still-produces.md. Neither names ListDiscoveredResources or AWS::EC2::Instance, and neither of these checks is marked ALREADY FILED or OWNED in the replay report.

Why: a client that uses Config the way AWS documents it gets a permanently empty account. DescribeConfigurationRecorderStatus reports recording=true while the inventory stays empty, so inventory and drift tooling cannot distinguish "nothing exists" from "nothing is recorded", and every downstream Config surface that is gated on discovery is unreachable: PutRemediationConfigurations / StartRemediationExecution for a real resource key, remediation exceptions keyed on a discovered resource, and any change-triggered rule evaluation for an EC2 instance. A compliance workflow therefore hangs on its first wait instead of progressing, and an automated guardrail silently never fires.

Fix: the configuration-recorder recording loop in the configservice crate — while a customer-managed recorder is RECORDING and its recordingGroup covers the type, resources created/changed/deleted in the other emulated services (here AWS::EC2::Instance from the EC2 service) must be captured into the recorder's inventory, which today only grows from an explicit PutResourceConfig write, and ListDiscoveredResources' handler must project that inventory (filtering by resourceType, resourceIds/resourceName and includeDeletedResources) instead of only PutResourceConfig-written entries.

Traces: config.e2e-remediation-auto-execution-consequence (poll at line 77), config.remediation-exceptions-lifecycle (_wait_discovered at line 36).

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator built from this checkout (emulator serving; bootstrap_signal false — 115/177 checks passed, config 111/161).

Expected (AWS / Smithy): HTTP 200 from ListDiscoveredResources with resourceIdentifiers containing one entry for the launched instance — {resourceType: 'AWS::EC2::Instance', resourceId: <the instance id from the request>, resourceName: <name when available>} — within a couple of minutes of the instance existing under a RECORDING recorder whose recordingGroup covers the type. Documentation: "The results consist of resources that Config has discovered, including those that Config is not currently recording."

Actual (emulator): HTTP 200 with resourceIdentifiers = [] on every call, indefinitely, for an instance the emulated EC2 service reports as running and then stopped.

  [error] config.e2e-remediation-auto-execution-consequence
    File "check_config_e2e_remediation_auto_execution_consequence.py", line 77, in run -> _poll_until(...)
    File "check_config_e2e_remediation_auto_execution_consequence.py", line 215, in _poll_until
    TimeoutError: the polled condition: the run budget ended first; not graded
    (the polled condition at line 77 is config.list_discovered_resources(resourceType="AWS::EC2::Instance", resourceIds=[instance_id])["resourceIdentifiers"] being non-empty)

  [error] config.remediation-exceptions-lifecycle
    File "check_config_remediation_exceptions_lifecycle.py", line 36, in run -> _wait_discovered(config, resource_type, instance_id, timeout_s=_budget_s(ctx, 180.0), interval_s=1.0, ctx=ctx)
    File "check_config_remediation_exceptions_lifecycle.py", line 183, in _wait_discovered
    TimeoutError: Config discovering i-00000000000000001: the run budget ended first; not graded
    (_wait_discovered, line 158 of the same file: "Poll Config's resource inventory until the recorder has discovered resource_id" — it polls the same ListDiscoveredResources call)

Neither check is marked ALREADY FILED (no unimplemented scaffold stub was hit) and neither is OWNED by an open issue in this replay. The instance id in the message (i-00000000000000001) is a per-run emulator-generated value, not part of any requirement; the requirement is that the discovered-resource list contains the instance id from the request.

## Steps to Reproduce
1. git_clone the repo, build and boot the emulator (run_behavioral_suite does this).
2. Re-replay the two check ids in trace_ids.
3. Or directly: iam:CreateRole trusting config.amazonaws.com with AWS_ConfigRole attached; s3:CreateBucket for delivery; config:PutConfigurationRecorder with ConfigurationRecorder={'name':'default','roleARN':<role arn>,'recordingGroup':{'allSupported':True}}; config:PutDeliveryChannel with that bucket; config:StartConfigurationRecorder; confirm config:DescribeConfigurationRecorderStatus reports recording=true.
4. ec2:RunInstances (any AMI/instance type, MinCount=MaxCount=1) and wait for the instance_running waiter, then ec2:StopInstances and wait for instance_stopped — EC2 answers normally.
5. Poll config:ListDiscoveredResources(resourceType='AWS::EC2::Instance', resourceIds=[<instance id>]): on AWS a ResourceIdentifier for that instance appears within ~1-2 minutes; the emulator returns resourceIdentifiers=[] forever.
6. Consequence to observe after step 5 succeeds on AWS but never on the emulator: config:PutConfigRule (REQUIRED_TAGS scoped to AWS::EC2::Instance) + config:PutRemediationConfigurations + config:StartRemediationExecution for that resource key, and config:PutRemediationExceptions / DescribeRemediationExceptions / DeleteRemediationExceptions keyed on the same discovered instance — all of which the two checks are blocked from reaching.
