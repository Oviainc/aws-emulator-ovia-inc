Issue-Source: trace-sweep-divergence
Issue-Trace: autoscaling-create-launch-configuration-unsupported
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: autoscaling-create-launch-configuration-unsupported autoscaling-launch-config-and-account-level-policy-reads autoscaling-launch-configuration-create-unsupported-in-modern-account autoscaling-launch-configuration-invalid-image-id-error autoscaling-pagination-invalid-next-token-errors
Issue-Shape-Id: com.amazonaws.autoscaling#AutoScaling_2011_01_01
Issue-Key: autoscaling|CreateLaunchConfiguration|4xx->2xx|$.ErrorResponse
Issue-Key: autoscaling|CreateLaunchConfiguration|4xx->2xx|status
Issue-Key: autoscaling|DescribeLaunchConfigurations|2xx->2xx|$.DescribeLaunchConfigurationsResponse.DescribeLaunchConfigurationsResult.LaunchConfigurations
Issue-Key: autoscaling|DeleteLaunchConfiguration|4xx->2xx|$.ErrorResponse
Issue-Key: autoscaling|DeleteLaunchConfiguration|4xx->2xx|status
Issue-Key: autoscaling|DescribeLaunchConfigurations|2xx->4xx|$.DescribeLaunchConfigurationsResponse
Issue-Key: autoscaling|DescribeLaunchConfigurations|2xx->4xx|status
Issue-Commit: 6f223cb07d8f19a9e48b2b4f7aac2665ac919e98

# autoscaling launch configuration surface diverges: creation is accepted where AWS refuses it

> _Note: this issue was identified against the code base at commit `6f223cb07d8f19a9e48b2b4f7aac2665ac919e98`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateLaunchConfiguration

## Description
The emulator still implements launch configurations as a creatable resource, while the recorded service refuses to create them at all; the three follow-on divergences are the downstream shape of that same fault plus one unrelated-looking rejection on the same operation family. Four causes, all autoscaling, all to be closed together.

Cause 1 of 4: CreateLaunchConfiguration must be refused. AWS: recorded behavior only — Smithy binds only AlreadyExistsFault, LimitExceededFault and ResourceContentionFault to CreateLaunchConfiguration and does NOT model UnsupportedOperation, so the requirement comes from the recording: HTTP 400 with ErrorResponse/Error Type=Sender, Code=UnsupportedOperation (recorded message, as an example: 'The Launch Configuration creation operation is not available in your account. Use launch templates to create configuration templates for your Auto Scaling groups.'). Emulator: HTTP 200 and the launch configuration is created and persisted. Why: a client probing whether launch configurations are still usable is told yes, so it builds a group around a resource type the real service cannot create — the failure surfaces much later and in a different operation. Fix: the CreateLaunchConfiguration handler must reject unconditionally with 400/UnsupportedOperation (Sender) rather than writing state. Traces: autoscaling-create-launch-configuration-unsupported#task#1 (also autoscaling-launch-config-and-account-level-policy-reads#task#1, autoscaling-launch-configuration-create-unsupported-in-modern-account#task#1, autoscaling-launch-configuration-invalid-image-id-error#task#1).

Cause 2 of 4: DescribeLaunchConfigurations then reports the phantom resource. AWS: 200 with an empty LaunchConfigurations list, because nothing could ever be created (Smithy: DescribeLaunchConfigurations output LaunchConfigurations is required, so the element is present but empty). Emulator: 200 with one member describing the configuration it wrongly accepted (ImageId/InstanceType/InstanceMonitoring/ARN echoed back). Why: inventory and cleanup logic sees a resource that does not exist in AWS. Fix: falls out of Cause 1 — with creation refused, the state holds no launch configurations; verify the empty-list serialization is an empty element, not a missing one. Traces: autoscaling-launch-config-and-account-level-policy-reads#task#2 (also autoscaling-launch-configuration-invalid-image-id-error#task#2).

Cause 3 of 4: DeleteLaunchConfiguration succeeds where AWS reports not-found. AWS: 400 with Error Type=Sender, Code=ValidationError for a name that cannot exist (recorded message, as an example: 'Launch configuration name not found - Launch configuration <name> not found'). Emulator: HTTP 200 with no ErrorResponse, because it deletes the phantom configuration from Cause 1. Why: a client's delete-then-assert-absent sequence passes against the emulator and fails against AWS. Fix: same root as Cause 1; the DeleteLaunchConfiguration handler must keep answering 400/ValidationError for an unknown name. Traces: autoscaling-launch-config-and-account-level-policy-reads#task#3.

Cause 4 of 4: DescribeLaunchConfigurations rejects a request AWS answers. AWS: 200 with an empty LaunchConfigurations list for this step (the fixture asserts DescribeLaunchConfigurationsResult.LaunchConfigurations = empty). Smithy: InvalidNextToken (code InvalidNextToken, 400) is bound to the operation, but the recording shows this particular request being answered 200. Emulator: HTTP 400 with no DescribeLaunchConfigurationsResponse. Hypothesis: the pagination fixture reuses the token that DescribeAutoScalingGroups legitimately rejects, and the emulator applies the same NextToken rejection to DescribeLaunchConfigurations while the recorded service tolerates it and returns the first (empty) page. Why: a paginating client is hard-failed on a list call that AWS answers, so enumeration of launch configurations aborts. Fix: the DescribeLaunchConfigurations handler's NextToken validation — match the recorded tolerance for this operation instead of rejecting. Traces: autoscaling-pagination-invalid-next-token-errors#task#3.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from mainline.

Cause 1 — CreateLaunchConfiguration, autoscaling-create-launch-configuration-unsupported#task#1: expected_status 400, actual_status 200; detail "$.ErrorResponse: missing (expected {'Error': {'Type': 'Sender', 'Code': 'UnsupportedOperation', 'Message': 'The Launch Configuration creation operation is not available in your account. Use launch templates to create configuration templates for your Auto Scaling groups.'}}, got None)". 4 fixtures affected.
Cause 2 — DescribeLaunchConfigurations, autoscaling-launch-config-and-account-level-policy-reads#task#2: status 200 both sides; detail "$.DescribeLaunchConfigurationsResponse.DescribeLaunchConfigurationsResult.LaunchConfigurations: type_mismatch (expected '', got {'member': {'LaunchConfigurationName': 'sim2real-probe-autoscaling-launchconfig-...', 'ImageId': 'ami-0123456789abcdef0', 'InstanceType': 't3.micro', ...}})". 2 fixtures affected.
Cause 3 — DeleteLaunchConfiguration, autoscaling-launch-config-and-account-level-policy-reads#task#3: expected_status 400, actual_status 200; detail "$.ErrorResponse: missing (expected {'Error': {'Type': 'Sender', 'Code': 'ValidationError', 'Message': 'Launch configuration name not found - Launch configuration ... not found'}}, got None)".
Cause 4 — DescribeLaunchConfigurations, autoscaling-pagination-invalid-next-token-errors#task#3: expected_status 200, actual_status 400; detail "$.DescribeLaunchConfigurationsResponse: missing (expected {'DescribeLaunchConfigurationsResult': {'LaunchConfigurations': ''}}, got None)".

Model evidence (smithy_query on services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/smithy-model/com-amazonaws-autoscaling-autoscaling-2011-01-01.json): CreateLaunchConfiguration errors = [AlreadyExists 400, LimitExceeded 400, ResourceContention 500] — UnsupportedOperation is not modeled, hence "recorded behavior only" for Cause 1. DescribeLaunchConfigurations output member LaunchConfigurations is required (present-but-empty, never absent) and the operation is paginated on NextToken/MaxRecords with InvalidNextToken bound.

## Steps to Reproduce
1. Boot the emulator from mainline.
2. Replay autoscaling-create-launch-configuration-unsupported step task#1 (Action=CreateLaunchConfiguration with a name, ImageId and InstanceType): observe HTTP 200 instead of HTTP 400 / Code=UnsupportedOperation.
3. Replay autoscaling-launch-config-and-account-level-policy-reads step task#2 (DescribeLaunchConfigurations): observe the phantom configuration in the list where AWS returns an empty list; then step task#3 (DeleteLaunchConfiguration on that name): observe HTTP 200 instead of 400 / Code=ValidationError.
4. Replay autoscaling-pagination-invalid-next-token-errors step task#3 (DescribeLaunchConfigurations): observe HTTP 400 instead of HTTP 200 with an empty LaunchConfigurations element.
5. After fixing, re-replay all four bindings above.
