Issue-Source: trace-sweep-divergence
Issue-Trace: compute-optimizer-ec2-instance-recommendations-and-metrics
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: compute-optimizer-ec2-instance-recommendations-and-metrics compute-optimizer-asg-and-ecs-recommendations compute-optimizer-ebs-idle-license-recommendations compute-optimizer-rds-recommendations-and-projected-metrics
Issue-Shape-Id: com.amazonaws.computeoptimizer#ComputeOptimizerService
Issue-Key: compute-optimizer|GetEC2InstanceRecommendations|2xx->2xx|$.errors[*]
Issue-Key: compute-optimizer|GetAutoScalingGroupRecommendations|2xx->2xx|$.errors[*]
Issue-Key: compute-optimizer|GetECSServiceRecommendations|2xx->2xx|$.errors[*]
Issue-Key: compute-optimizer|GetIdleRecommendations|2xx->2xx|$.errors[*]
Issue-Key: compute-optimizer|GetLicenseRecommendations|2xx->2xx|$.errors[*]
Issue-Key: compute-optimizer|GetRDSDatabaseRecommendations|2xx->2xx|$.errors[*]
Issue-Key: compute-optimizer|GetECSServiceRecommendationProjectedMetrics|2xx->4xx|$.recommendedOptionProjectedMetrics
Issue-Key: compute-optimizer|GetECSServiceRecommendationProjectedMetrics|2xx->4xx|status
Issue-Key: compute-optimizer|GetEC2RecommendationProjectedMetrics|2xx->4xx|$.recommendedOptionProjectedMetrics
Issue-Key: compute-optimizer|GetEC2RecommendationProjectedMetrics|2xx->4xx|status
Issue-Key: compute-optimizer|GetRDSDatabaseRecommendationProjectedMetrics|2xx->4xx|$.recommendedOptionProjectedMetrics
Issue-Key: compute-optimizer|GetRDSDatabaseRecommendationProjectedMetrics|2xx->4xx|status
Issue-Commit: 2f933b71caa2a46f81d276f12cbb60da2946d084

# Resources with no recommendation are reported as absent instead of as an empty recommendation result

> _Note: this issue was identified against the code base at commit `2f933b71caa2a46f81d276f12cbb60da2946d084`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetEC2InstanceRecommendations

## Description
NINE causes, one shared fault. Every Compute Optimizer recommendation reader in this crate treats "this resource has no recommendation for me to return" as "this resource does not exist". Compute Optimizer only produces a recommendation after it has accumulated enough CloudWatch metric history for a resource, so "enrolled, nothing analyzed yet" is the normal steady state of a fresh account and is the state every one of these fixtures exercises. AWS answers that state with HTTP 200 and an EMPTY result: the recommendation list is empty, the `errors` list is empty/absent, and the projected-metrics list is empty. The emulator instead manufactures an absence signal — either a synthetic GetRecommendationError/IdleRecommendationError entry with code `ResourceNotFound`, or an HTTP 404 ResourceNotFoundException. Causes 1-6 are the fabricated-error form (six readers); causes 7-9 are the 404 form (three projected-metrics readers). They are one defect expressed in nine handlers: the "no row in the findings catalog" branch emits a not-found signal rather than falling through to an empty result.

=== Cause 1 of 9: GetEC2InstanceRecommendations fabricates a ResourceNotFound error entry ===
AWS: Smithy: GetEC2InstanceRecommendations input instanceArns -> InstanceArns; output {nextToken, instanceRecommendations: InstanceRecommendations, errors: GetRecommendationErrors}; modeled errors include InvalidParameterValueException (client, 400) and ResourceNotFoundException (client, 404). GetRecommendationError is documented "Describes an error experienced when getting recommendations. For example, an error is returned if you request recommendations for an unsupported Auto Scaling group, or if you request recommendations for an instance of an unsupported instance family" — i.e. the list carries UNSUPPORTED-resource errors, not "not analyzed yet". Recorded behavior: a request naming an EC2 instance ARN for which the account holds no recommendation is answered HTTP 200 with `instanceRecommendations` empty and NO `errors` entry at all (`errors` empty/absent).
Emulator: HTTP 200 (status matches) but the body carries `errors[0] = {identifier: <the instance ARN from the request>, code: "ResourceNotFound", message: "Compute Optimizer has no Amazon EC2 instance recommendation for <instance ARN>."}`. The `code` value `ResourceNotFound` and that whole message sentence are emulator-authored; the ARN is interpolated from the request.
Why: a client that treats a non-empty `errors` list as a partial failure (logging it, alerting on it, failing a batch, retrying the ARN) reports a hard error for every resource that AWS simply has not analyzed yet — which on a freshly enrolled account is every resource. The two states are not distinguishable against the emulator.
Fix: the GetEC2InstanceRecommendations handler in the compute-optimizer crate — the branch that handles a requested ARN with no catalog row must leave `errors` empty instead of pushing a GetRecommendationError.
Traces: compute-optimizer-ec2-instance-recommendations-and-metrics#task#4

=== Cause 2 of 9: GetAutoScalingGroupRecommendations fabricates a ResourceNotFound error entry ===
AWS: Smithy: GetAutoScalingGroupRecommendations input autoScalingGroupArns -> AutoScalingGroupArns; output {nextToken, autoScalingGroupRecommendations, errors: GetRecommendationErrors}. Recorded behavior: an Auto Scaling group ARN with no recommendation yields HTTP 200, empty recommendation list, and no `errors` entry.
Emulator: HTTP 200 with `errors[0] = {identifier: <the Auto Scaling group ARN from the request>, code: "ResourceNotFound", message: "Compute Optimizer has no Auto Scaling group recommendation for <ASG ARN>."}`. Fixed text is the emulator's own sentence; the only request-derived part is the ARN.
Why: same as Cause 1 — an unanalyzed Auto Scaling group is surfaced to the caller as a per-resource failure.
Fix: the GetAutoScalingGroupRecommendations handler in the compute-optimizer crate, same no-catalog-row branch.
Traces: compute-optimizer-asg-and-ecs-recommendations#task#4

=== Cause 3 of 9: GetECSServiceRecommendations fabricates a ResourceNotFound error entry ===
AWS: Smithy: GetECSServiceRecommendations input serviceArns -> ServiceArns; output {nextToken, ecsServiceRecommendations, errors: GetRecommendationErrors}. Recorded behavior: an ECS service ARN with no recommendation yields HTTP 200, empty recommendation list, no `errors` entry.
Emulator: HTTP 200 with `errors[0] = {identifier: <the ECS service ARN from the request>, code: "ResourceNotFound", message: "No Amazon ECS service with the Amazon Resource Name (ARN) <service ARN> exists."}`. Note this message additionally asserts the service does not EXIST, a claim the emulator cannot make from "no recommendation row".
Why: a caller is told its live ECS service does not exist; orchestration that deletes or re-creates the service on that signal acts on a false premise.
Fix: the GetECSServiceRecommendations handler in the compute-optimizer crate, same no-catalog-row branch.
Traces: compute-optimizer-asg-and-ecs-recommendations#task#5

=== Cause 4 of 9: GetIdleRecommendations fabricates a ResourceNotFound error entry ===
AWS: Smithy: GetIdleRecommendations input resourceArns -> ResourceArns; output {nextToken, idleRecommendations, errors: IdleRecommendationErrors}; IdleRecommendationError is documented "Returns of list of resources that doesn't have idle recommendations" and carries {identifier, code, message, resourceType}. Recorded behavior: an EBS volume ARN with no idle recommendation yields HTTP 200, empty `idleRecommendations`, and NO `errors` entry.
Emulator: HTTP 200 with `errors[0] = {identifier: <the volume ARN from the request>, code: "ResourceNotFound", message: "Compute Optimizer has no idle resource recommendation for the resource this identifier names.", resourceType: "EBSVolume"}`.
Why: same as Cause 1 — the idle-resource report cannot be read as "nothing idle"; it reads as "every resource failed".
Fix: the GetIdleRecommendations handler in the compute-optimizer crate, same no-catalog-row branch.
Traces: compute-optimizer-ebs-idle-license-recommendations#task#6

=== Cause 5 of 9: GetLicenseRecommendations fabricates a ResourceNotFound error entry ===
AWS: Smithy: GetLicenseRecommendations input resourceArns -> ResourceArns; output {nextToken, licenseRecommendations, errors: GetRecommendationErrors}. Recorded behavior: an EC2 instance ARN with no license recommendation yields HTTP 200, empty `licenseRecommendations`, no `errors` entry.
Emulator: HTTP 200 with `errors[0] = {identifier: <the instance ARN from the request>, code: "ResourceNotFound", message: "Compute Optimizer has no license recommendation for the resource this identifier names."}`.
Why: same as Cause 1.
Fix: the GetLicenseRecommendations handler in the compute-optimizer crate, same no-catalog-row branch.
Traces: compute-optimizer-ebs-idle-license-recommendations#task#7

=== Cause 6 of 9: GetRDSDatabaseRecommendations fabricates a ResourceNotFound error entry ===
AWS: Smithy: GetRDSDatabaseRecommendations input resourceArns -> ResourceArns; output {nextToken, rdsDBRecommendations, errors: GetRecommendationErrors}. Recorded behavior: an RDS DB instance ARN with no recommendation yields HTTP 200, empty `rdsDBRecommendations`, no `errors` entry.
Emulator: HTTP 200 with `errors[0] = {identifier: <the DB ARN from the request>, code: "ResourceNotFound", message: "Compute Optimizer has no Amazon Aurora or RDS database recommendation for the resource this identifier names."}`.
Why: same as Cause 1.
Fix: the GetRDSDatabaseRecommendations handler in the compute-optimizer crate, same no-catalog-row branch.
Traces: compute-optimizer-rds-recommendations-and-projected-metrics#task#4

=== Cause 7 of 9: GetEC2RecommendationProjectedMetrics answers 404 instead of 200 with an empty list ===
AWS: Smithy: GetEC2RecommendationProjectedMetrics input {instanceArn (required), stat (required, MetricStatistic), period (required), startTime (required), endTime (required), recommendationPreferences}; output {recommendedOptionProjectedMetrics: RecommendedOptionProjectedMetrics} — a LIST with no required members, so an empty list is a representable and expected answer; ResourceNotFoundException (client, 404) is modeled but is not what the recording shows for this state. Recorded behavior: for an instance ARN that has no recommendation, the call is answered HTTP 200 with `recommendedOptionProjectedMetrics: []`.
Emulator: HTTP 404 and no `recommendedOptionProjectedMetrics` member at all (the member is absent where AWS sends `[]`).
Why: the status class itself diverges — an SDK raises ResourceNotFoundException where AWS returns a successful empty response, so a dashboard that charts projected metrics for each analyzed instance aborts instead of rendering an empty series, and a caller cannot tell "no projection yet" from "wrong ARN".
Fix: the GetEC2RecommendationProjectedMetrics handler in the compute-optimizer crate — the lookup miss must return a 200 response carrying an empty `recommendedOptionProjectedMetrics` list rather than raising ResourceNotFoundException.
Traces: compute-optimizer-ec2-instance-recommendations-and-metrics#task#6

=== Cause 8 of 9: GetECSServiceRecommendationProjectedMetrics answers 404 instead of 200 with an empty list ===
AWS: Smithy: GetECSServiceRecommendationProjectedMetrics input {serviceArn (required), stat (required), period (required), startTime (required), endTime (required)}; output {recommendedOptionProjectedMetrics: ECSServiceRecommendedOptionProjectedMetrics}. Recorded behavior: an ECS service ARN with no recommendation is answered HTTP 200 with `recommendedOptionProjectedMetrics: []`.
Emulator: HTTP 404, `recommendedOptionProjectedMetrics` absent.
Why: same as Cause 7.
Fix: the GetECSServiceRecommendationProjectedMetrics handler in the compute-optimizer crate, same lookup-miss branch.
Traces: compute-optimizer-asg-and-ecs-recommendations#task#6

=== Cause 9 of 9: GetRDSDatabaseRecommendationProjectedMetrics answers 404 instead of 200 with an empty list ===
AWS: Smithy: GetRDSDatabaseRecommendationProjectedMetrics input {resourceArn (required), stat (required), period (required), startTime (required), endTime (required), recommendationPreferences}; output {recommendedOptionProjectedMetrics: RDSDatabaseRecommendedOptionProjectedMetrics}. Recorded behavior: an RDS resource ARN with no recommendation is answered HTTP 200 with `recommendedOptionProjectedMetrics: []`.
Emulator: HTTP 404, `recommendedOptionProjectedMetrics` absent.
Why: same as Cause 7.
Fix: the GetRDSDatabaseRecommendationProjectedMetrics handler in the compute-optimizer crate, same lookup-miss branch. Note the already-FIXED issue 20261026T232352Z-compute-optimizer-getrdsdatabaserecommendationprojectedmetrics-wrong-behavior.md added an analysis-due gate to this reader; the gate must not turn into a 404.
Traces: compute-optimizer-rds-recommendations-and-projected-metrics#task#5

Relation to open issues: 20261024T082711Z-compute-optimizer-getebsvolumerecommendations-wrong-behavior.md and 20261024T082711Z-compute-optimizer-getlambdafunctionrecommendations-wrong-behavior.md concern which inventory a reader analyses, and 20261023T062035Z-compute-optimizer-wrong-rejection-class-for-out-of.md concerns maxResults/effectiveIncludeList — none of them covers the absence signal these nine readers emit for a resource with no recommendation.

## Evidence
All nine clusters replayed against a running emulator (emulator_up=true, 53/70 steps passed), so the service answers; these are fidelity divergences, not a bootstrap gap.

Fabricated-error form (status matched at 200; the body carries an extra errors[0]):
  Cause 1 GetEC2InstanceRecommendations [compute-optimizer-ec2-instance-recommendations-and-metrics#task#4]
    expected $.errors[0]: None (no entry)
    actual   $.errors[0]: {'identifier': 'arn:aws:ec2:us-east-1:000000000000:instance/i-0fedcba9876543210', 'code': 'ResourceNotFound', 'message': 'Compute Optimizer has no Amazon EC2 instance recommendation for arn:aws:ec2:us-east-1:000000000000:instance/i-0fedcba9876543210.'}
  Cause 2 GetAutoScalingGroupRecommendations [compute-optimizer-asg-and-ecs-recommendations#task#4]
    expected $.errors[0]: None
    actual   {'identifier': 'arn:aws:autoscaling:us-east-1:000000000000:autoScalingGroup:00000000-0000-0000-0000-000000000000:autoScalingGroupName/co-probe-asg', 'code': 'ResourceNotFound', 'message': 'Compute Optimizer has no Auto Scaling group recommendation for <that ARN>.'}
  Cause 3 GetECSServiceRecommendations [compute-optimizer-asg-and-ecs-recommendations#task#5]
    expected $.errors[0]: None
    actual   {'identifier': 'arn:aws:ecs:us-east-1:000000000000:service/co-probe-cluster/co-probe-service', 'code': 'ResourceNotFound', 'message': 'No Amazon ECS service with the Amazon Resource Name (ARN) <that ARN> exists.'}
  Cause 4 GetIdleRecommendations [compute-optimizer-ebs-idle-license-recommendations#task#6]
    expected $.errors[0]: None
    actual   {'identifier': 'arn:aws:ec2:us-east-1:000000000000:volume/vol-0123456789abcdef0', 'code': 'ResourceNotFound', 'message': 'Compute Optimizer has no idle resource recommendation for the resource this identifier names.', 'resourceType': 'EBSVolume'}
  Cause 5 GetLicenseRecommendations [compute-optimizer-ebs-idle-license-recommendations#task#7]
    expected $.errors[0]: None
    actual   {'identifier': 'arn:aws:ec2:us-east-1:000000000000:instance/i-0123456789abcdef0', 'code': 'ResourceNotFound', 'message': 'Compute Optimizer has no license recommendation for the resource this identifier names.'}
  Cause 6 GetRDSDatabaseRecommendations [compute-optimizer-rds-recommendations-and-projected-metrics#task#4]
    expected $.errors[0]: None
    actual   {'identifier': 'arn:aws:rds:us-east-1:000000000000:db:co-probe-db', 'code': 'ResourceNotFound', 'message': 'Compute Optimizer has no Amazon Aurora or RDS database recommendation for the resource this identifier names.'}
  (the account id 000000000000, the region token and the probe resource names/ids are per-environment values filling the emulator's template; the defect is that an entry is emitted at all, and the emulator-authored code/message template it is built from)

404-instead-of-empty form:
  Cause 7 GetEC2RecommendationProjectedMetrics [compute-optimizer-ec2-instance-recommendations-and-metrics#task#6]
    status: expected 200, got 404; $.recommendedOptionProjectedMetrics: missing (expected [], got None)
  Cause 8 GetECSServiceRecommendationProjectedMetrics [compute-optimizer-asg-and-ecs-recommendations#task#6]
    status: expected 200, got 404; $.recommendedOptionProjectedMetrics: missing (expected [], got None)
  Cause 9 GetRDSDatabaseRecommendationProjectedMetrics [compute-optimizer-rds-recommendations-and-projected-metrics#task#5]
    status: expected 200, got 404; $.recommendedOptionProjectedMetrics: missing (expected [], got None)

Model evidence (smithy_query on services/service-com-amazonaws-computeoptimizer-computeoptimizerservice/smithy-model/com-amazonaws-computeoptimizer-computeoptimizerservice.json): each Get*Recommendations output declares `errors` as an OPTIONAL list of GetRecommendationError/IdleRecommendationError, documented for UNSUPPORTED resources ("an error is returned if you request recommendations for an unsupported Auto Scaling group, or ... an instance of an unsupported instance family"); each Get*ProjectedMetrics output declares only `recommendedOptionProjectedMetrics`, an optional list, so `[]` is a well-formed success body. No modeled trait makes "no recommendation yet" a 404 or an errors entry.

## Steps to Reproduce
Boot the emulator and opt the account in (UpdateEnrollmentStatus status=Active, confirm with GetEnrollmentStatus).

Causes 1-6 — for each reader, pass one ARN of the matching resource type for which no recommendation has been produced:
  GetEC2InstanceRecommendations(instanceArns=[<ec2 instance arn>])
  GetAutoScalingGroupRecommendations(autoScalingGroupArns=[<asg arn>])
  GetECSServiceRecommendations(serviceArns=[<ecs service arn>])
  GetIdleRecommendations(resourceArns=[<ebs volume arn>])
  GetLicenseRecommendations(resourceArns=[<ec2 instance arn>])
  GetRDSDatabaseRecommendations(resourceArns=[<rds db arn>])
Each returns HTTP 200. Read the `errors` member: the emulator carries one entry with code 'ResourceNotFound' and its own message naming the ARN; AWS returns the recommendation list empty and `errors` empty/absent.

Causes 7-9 — call each projected-metrics reader for an ARN with no recommendation, with stat/period/startTime/endTime set:
  GetEC2RecommendationProjectedMetrics(instanceArn=..., stat='Average', period=3600, startTime=..., endTime=...)
  GetECSServiceRecommendationProjectedMetrics(serviceArn=..., stat='Average', period=3600, startTime=..., endTime=...)
  GetRDSDatabaseRecommendationProjectedMetrics(resourceArn=..., stat='Average', period=3600, startTime=..., endTime=...)
Each answers HTTP 404 ResourceNotFoundException; AWS answers HTTP 200 with `recommendedOptionProjectedMetrics: []`.

Replay: re-run fixture compute-optimizer-ec2-instance-recommendations-and-metrics step task#4 (representative) plus the eight bindings in trace_ids.
