Issue-Source: trace-sweep-divergence
Issue-Trace: batch-batch-diag-job-definition-log-configuration-absent-vs-override
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: batch-batch-diag-job-definition-log-configuration-absent-vs-override batch-job-definition-preserves-untagged-image
Issue-Shape-Id: com.amazonaws.batch#AWSBatchV20160810
Issue-Key: batch|DescribeJobDefinitions|2xx->2xx|$.jobDefinitions[*]
Issue-Key: batch|DescribeJobDefinitions|2xx->2xx|$.jobDefinitions[*].containerProperties.command
Issue-Key: batch|DescribeJobDefinitions|2xx->2xx|$.jobDefinitions[*].containerProperties.environment
Issue-Key: batch|DescribeJobDefinitions|2xx->2xx|$.jobDefinitions[*].containerProperties.fargatePlatformConfiguration
Issue-Key: batch|DescribeJobDefinitions|2xx->2xx|$.jobDefinitions[*].containerProperties.mountPoints
Issue-Key: batch|DescribeJobDefinitions|2xx->2xx|$.jobDefinitions[*].containerProperties.networkConfiguration.interfaceConfigurations
Issue-Key: batch|DescribeJobDefinitions|2xx->2xx|$.jobDefinitions[*].containerProperties.secrets
Issue-Key: batch|DescribeJobDefinitions|2xx->2xx|$.jobDefinitions[*].containerProperties.ulimits
Issue-Key: batch|DescribeJobDefinitions|2xx->2xx|$.jobDefinitions[*].containerProperties.volumes
Issue-Key: batch|DescribeJobDefinitions|2xx->2xx|$.jobDefinitions[*].parameters
Issue-Commit: a4ef059b31cd49fc5529baf331c68065b4514ad5

# batch DescribeJobDefinitions omits empty collections and applied defaults from a job definition (2 causes)

> _Note: this issue was identified against the code base at commit `a4ef059b31cd49fc5529baf331c68065b4514ad5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeJobDefinitions

## Description
Two net-new root causes in how a registered job definition is projected on read. Both must be closed before submitting.

Cause 1 of 2 - DescribeJobDefinitions drops every collection member that is empty instead of reporting it as an empty list/map.
AWS: Smithy (com.amazonaws.batch) DescribeJobDefinitions, POST /v1/describejobdefinitions, success 200, output `jobDefinitions` (JobDefinitionList of JobDefinition). JobDefinition carries `parameters` (ParametersMap) and ContainerProperties carries `command`, `volumes`, `environment`, `mountPoints`, `ulimits`, `resourceRequirements` and `secrets` (all list/map members). The recording shows AWS materializing these on a read of a container job definition even when nothing was supplied for them: `parameters` = {}, and `volumes`/`environment`/`mountPoints`/`ulimits`/`resourceRequirements`/`secrets` = [] (recorded behavior; the model does not mark them @default).
Emulator: those members are absent from the described job definition entirely (None). The members the caller did set (jobDefinitionName, jobDefinitionArn, revision, status, type, containerProperties.image/vcpus/memory/command, tags, containerOrchestrationType) are present and correct; only the empty ones vanish.
Why: a client that reads a job definition back to diff it against desired state (an IaC provider refresh, a drift detector, a deploy gate) sees a member go from [] to absent, which its diff reports as a change, so it re-applies the job definition on every run. Code that indexes containerProperties['environment'] without a guard raises a KeyError against the emulator where it works against AWS.
Fix: the JobDefinition/ContainerProperties projection used by DescribeJobDefinitions in the batch crate - serialize empty collections as [] / {} rather than skipping them.
Traces: batch-batch-diag-job-definition-log-configuration-absent-vs-override#task#5 (cluster also covers #task#6 of the same fixture and batch-job-definition-preserves-untagged-image#task#4)

Cause 2 of 2 - a job definition's applied defaults are not reported on read: `fargatePlatformConfiguration` and the nested `networkConfiguration.interfaceConfigurations` are missing.
AWS: ContainerProperties.fargatePlatformConfiguration (FargatePlatformConfiguration) - the model documents that "if one isn't specified, the LATEST platform version is used by default" - and ContainerProperties.networkConfiguration. The recording shows a read of a registered definition reporting `fargatePlatformConfiguration` = {"platformVersion": "LATEST"}, i.e. AWS applies the default at register time and reports it, plus `networkConfiguration.interfaceConfigurations` = [] alongside the empty collections of cause 1.
Emulator: `fargatePlatformConfiguration` and `networkConfiguration.interfaceConfigurations` are absent (None) on the described revision.
Why: the platform version is what decides how the job will actually run. A caller that reads it back to learn or pin that version gets nothing, so it either keeps re-sending the field or concludes the definition is incomplete; the absent nested list breaks the same diff/index paths as cause 1.
Fix: the defaulting applied when RegisterJobDefinition stores the definition, plus the containerProperties projection in DescribeJobDefinitions, in the batch crate. Note: open issue 20261102T090835Z-batch-ecr-lambda-logs-batch-job-definition-validation-and-defaults-diverge-and.md describes the same fargatePlatformConfiguration default from the behavioral suite, but it carries no wire replay key for this fixture step (and does not mention networkConfiguration.interfaceConfigurations), so this step is filed here with its own key.
Traces: batch-job-definition-preserves-untagged-image#teardown#6

## Evidence
Both from the simparity-wire-aws-v1 replay against this checkout. Expected = recorded AWS response; actual = emulator. Names, ARNs and the account id below are this run's values filling the templates, not requirements.

Cause 1 - batch-batch-diag-job-definition-log-configuration-absent-vs-override#task#5, DescribeJobDefinitions, status 200 on both sides (cluster of 3 steps: #task#5, #task#6, batch-job-definition-preserves-untagged-image#task#4):
  $.jobDefinitions[0]: expected {... 'parameters': {}, 'containerProperties': {'image': 'public.ecr.aws/amazonlinux/amazonlinux', 'vcpus': 1, 'memory': 512, 'command': ['/bin/true'], 'volumes': [], 'environment': [], 'mountPoints': [], 'ulimits': [], 'resourceRequirements': [], 'secrets': []}, 'tags': {}, 'containerOrchestrationType': 'ECS'}
  got      {... 'containerProperties': {'image': 'public.ecr.aws/amazonlinux/amazonlinux', 'vcpus': 1, 'memory': 512, 'command': ['/bin/true']}, 'tags': {}, 'containerOrchestrationType': 'ECS'} - i.e. parameters and the six empty containerProperties collections are absent.

Cause 2 - batch-job-definition-preserves-untagged-image#teardown#6, DescribeJobDefinitions, status 200 on both sides:
  $.jobDefinitions[0].parameters: missing (expected {}, got None)
  $.jobDefinitions[0].containerProperties.command / .volumes / .environment / .mountPoints / .ulimits / .secrets: missing (expected [], got None)
  $.jobDefinitions[0].containerProperties.networkConfiguration.interfaceConfigurations: missing (expected [], got None)
  $.jobDefinitions[0].containerProperties.fargatePlatformConfiguration: missing (expected {'platformVersion': 'LATEST'}, got None)

## Steps to Reproduce
Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1), or by hand:

Cause 1: aws batch register-job-definition --job-definition-name jd --type container --container-properties '{"image":"public.ecr.aws/amazonlinux/amazonlinux","vcpus":1,"memory":512,"command":["/bin/true"]}' then aws batch describe-job-definitions --job-definitions jd -> the described revision has no `parameters` and no `volumes`/`environment`/`mountPoints`/`ulimits`/`resourceRequirements`/`secrets`; AWS reports them as {} and [].

Cause 2: register a job definition that leaves containerProperties.fargatePlatformConfiguration and networkConfiguration unset, then describe it -> no `fargatePlatformConfiguration` and no `networkConfiguration.interfaceConfigurations`; AWS reports {"platformVersion":"LATEST"} and [].

Then re-replay: batch-batch-diag-job-definition-log-configuration-absent-vs-override#task#5 and batch-job-definition-preserves-untagged-image#teardown#6.
