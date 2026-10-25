Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elasticbeanstalk.deploy-environment-via-cloudformation-serves-traffic
Issue-Shape-Id: com.amazonaws.elasticbeanstalk#AWSElasticBeanstalkService
Issue-Key: elasticbeanstalk.deploy-environment-via-cloudformation-serves-traffic|_wait_until#16|Timeout|time.sleep(interval_s)
Issue-Commit: f74e73101c4cd83227b9737045a1bdde9130cb35

# Elastic Beanstalk enhanced health reports no instances: DescribeInstancesHealth always returns an empty InstanceHealthList and DescribeEnvironmentHealth's InstancesHealth summary is all zeros, even though the environment's own resource inventory holds EC2 instances

> _Note: this issue was identified against the code base at commit `f74e73101c4cd83227b9737045a1bdde9130cb35`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstancesHealth

## Description
One net-new root cause.

Cause 1 of 1: a live Elastic Beanstalk environment with enhanced health reporting enabled (`aws:elasticbeanstalk:healthreporting:system` / `SystemType=enhanced`) reports NO per-instance health at all, so no caller can ever observe an instance of the environment becoming healthy. Failing check: elasticbeanstalk.deploy-environment-via-cloudformation-serves-traffic.

AWS: `DescribeInstancesHealth` "Retrieves detailed information about the health of instances in your Elastic Beanstalk environments. This operation requires enhanced health reporting." Its output member `InstanceHealthList` (Smithy: `InstanceHealthList` of `SingleInstanceHealth`) is documented as "Detailed health information about each instance", where each `SingleInstanceHealth` carries `InstanceId` (the ID of the Amazon EC2 instance), `HealthStatus`, `Color`, `Causes`, `LaunchedAt`, `AvailabilityZone`, `InstanceType`, `ApplicationMetrics`, `System` and `Deployment`. Correspondingly `DescribeEnvironmentHealth`'s `InstancesHealth` member (Smithy: `InstanceHealthSummary`) buckets the environment's instances by status, where `Ok` is documented as "Green. An instance is passing health checks and the health agent is not reporting any problems." For an environment that has reached `Status=Ready` / `HealthStatus=Ok` and whose resources include running EC2 instances, AWS returns one `SingleInstanceHealth` entry per instance with `HealthStatus` in the green set (`Ok`, or `Info` while an operation is in progress) and a correspondingly non-zero `InstanceHealthSummary.Ok`/`Info` count. The general requirement: the per-instance health list and the summary buckets must agree with the set of instances the same environment reports, and a Ready/Green environment must present at least one instance in a green status.

Emulator: `DescribeInstancesHealth` unconditionally answers `InstanceHealthList: []` for ANY found environment, whatever its status and whatever `AttributeNames` was asked for (services/service-com-amazonaws-elasticbeanstalk-awselasticbeanstalkservice/src/ops/describe_instances_health.rs — the handler builds `instance_health_list: Some(Vec::new())` and the module doc states "this emulator runs neither, so a found environment reports an empty instance list"). `DescribeEnvironmentHealth` likewise fills `InstancesHealth` with every bucket set to 0 (`no_data/unknown/pending/ok/info/warning/degraded/severe` all `Some(0)`). This contradicts the SAME environment record: `EnvironmentResources::provision` populates `instances: Vec<String>` with generated `i-...` EC2 instance ids (and `DescribeEnvironmentResources` reports them), so the emulator simultaneously claims the environment is made of N instances and that enhanced health knows of zero instances. Because of that, any client polling for "at least one instance in Ok/Info" never converges: the replayed check deploys an `AWS::ElasticBeanstalk::Environment` (SingleInstance, enhanced health) through CloudFormation, observes `DescribeEnvironmentHealth` and then polls `DescribeInstancesHealth` for up to 300s for one green instance, and the check is killed by the 60s harness budget because that condition can never become true.

Why: the normal post-deploy readiness gate for Elastic Beanstalk — "the environment is Ready AND at least one instance reports Ok/Info" — is unsatisfiable against this emulator. Every CLI/SDK/CI flow that waits on instance health (eb health, deployment smoke tests, CloudFormation-driven deploy pipelines, autoscaling/health dashboards) hangs until its own timeout instead of succeeding, and anything that counts instances by health bucket reads 0 instances for a running environment. A client cannot distinguish "environment has no instances" from "environment is healthy", so health-driven rollback/alarm logic silently takes the unhealthy branch.

Fix: the `DescribeInstancesHealth` handler (services/service-com-amazonaws-elasticbeanstalk-awselasticbeanstalkservice/src/ops/describe_instances_health.rs) must derive one `SingleInstanceHealth` per instance id held on the environment record's resource inventory and select its members per `AttributeNames`, and `DescribeEnvironmentHealth`'s `InstancesHealth` summary (src/ops/describe_environment_health.rs, `instance_health_summary`) must be computed from that same per-instance set so the two agree; the instance set and its per-instance status belong on the shared environment state (src/state.rs, `EnvironmentResources` / the status-transition machinery) rather than being hardcoded empty in each reporting handler, so a Ready/Green environment presents its instances as green and a Launching/Updating one presents them as Pending.

Traces: elasticbeanstalk.deploy-environment-via-cloudformation-serves-traffic.

Note: the open issues 20261024T015947Z-elasticbeanstalk-elastic-beanstalk-application-version-environment-state-machines-never.md and 20261023T205650Z-elasticbeanstalk-elastic-beanstalk-control-plane-guards-are-not-enforced.md cover version/environment state advancement and control-plane guards; neither covers enhanced-health instance reporting, which is this cause.

## Evidence
Check: elasticbeanstalk.deploy-environment-via-cloudformation-serves-traffic — verdict [mismatch], "Error: Check timed out after 60.0s".

Expected (AWS): after the environment reaches Status=Ready / HealthStatus=Ok, DescribeInstancesHealth(EnvironmentName=..., AttributeNames=["All"]) returns InstanceHealthList with >= 1 SingleInstanceHealth entry whose HealthStatus is "Ok" (or "Info"), carrying that instance's InstanceId, Color, LaunchedAt, InstanceType and AvailabilityZone; DescribeEnvironmentHealth(AttributeNames=["All"]).InstancesHealth reports a matching non-zero Ok (or Info) count.

Actual (emulator): DescribeInstancesHealth returns `{"InstanceHealthList": [], "RefreshedAt": <now>}` for every call on an existing environment — the handler is written as `instance_health_list: Some(Vec::new())` with no dependence on the environment at all. DescribeEnvironmentHealth returns InstancesHealth with NoData=Unknown=Pending=Ok=Info=Warning=Degraded=Severe=0. The same environment record's inventory, built by EnvironmentResources::provision in src/state.rs, contains generated EC2 instance ids (`i-...`) that DescribeEnvironmentResources does report — so the two views of the same environment contradict each other.

Consequence in the replay: the check's poll `len(InstanceHealthList) >= 1 and any(HealthStatus in ("Ok","Info"))` can never become true, so the check never finishes and is cut off at the harness's 60s budget instead of completing. Status codes are 200 on both health calls — the divergence is in the response bodies, not in a refusal.

## Steps to Reproduce
1. Boot the emulator.
2. Create an Elastic Beanstalk application and a SingleInstance environment with enhanced health, e.g. via CloudFormation (AWS::ElasticBeanstalk::Application + AWS::IAM::Role + AWS::IAM::InstanceProfile + AWS::ElasticBeanstalk::Environment with OptionSettings `aws:elasticbeanstalk:environment/EnvironmentType=SingleInstance`, `aws:elasticbeanstalk:healthreporting:system/SystemType=enhanced`, `aws:autoscaling:launchconfiguration/IamInstanceProfile=<ref>`), or directly with `elasticbeanstalk create-environment` using the same option settings.
3. Poll `elasticbeanstalk describe-environment-health --environment-name <env> --attribute-names All` until Status=Ready.
4. Call `elasticbeanstalk describe-instances-health --environment-name <env> --attribute-names All`.
   Observed: `InstanceHealthList` is `[]` forever.
   Expected: at least one entry with `HealthStatus` of `Ok`/`Info` and that instance's `InstanceId`.
5. Call `elasticbeanstalk describe-environment-resources --environment-name <env>` and note it DOES list `i-...` instance ids for the same environment, and that `describe-environment-health --attribute-names All` reports `InstancesHealth` with all buckets 0 — the two views disagree.
