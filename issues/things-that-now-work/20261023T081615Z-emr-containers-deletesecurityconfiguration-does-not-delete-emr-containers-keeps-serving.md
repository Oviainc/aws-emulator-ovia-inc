Issue-Source: trace-sweep-divergence
Issue-Trace: emr-containers-security-configuration-lifecycle
Issue-Step: teardown#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: emr-containers-security-configuration-lifecycle
Issue-Shape-Id: com.amazonaws.emrcontainers#AwsChicagoWebService
Issue-Key: emr-containers|DescribeSecurityConfiguration|4xx->2xx|$
Issue-Key: emr-containers|DescribeSecurityConfiguration|4xx->2xx|$.message
Issue-Key: emr-containers|DescribeSecurityConfiguration|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: emr-containers|DescribeSecurityConfiguration|4xx->2xx|status
Issue-Commit: 0dfa9e823e78890cf2fe273b1e49d27d43c189eb

# DeleteSecurityConfiguration does not delete: emr-containers keeps serving the deleted configuration

> _Note: this issue was identified against the code base at commit `0dfa9e823e78890cf2fe273b1e49d27d43c189eb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeSecurityConfiguration

## Description
One cause: the emr-containers emulator's delete path for a security configuration does not remove it from state (or DescribeSecurityConfiguration does not consult the deleted flag), so a configuration remains readable indefinitely after a successful delete.

Cause 1 of 1 - DescribeSecurityConfiguration still answers 200 after DeleteSecurityConfiguration (fixture emr-containers-security-configuration-lifecycle, step teardown#7).
AWS: Smithy (services/service-com-amazonaws-emrcontainers-awschicagowebservice/smithy-model/com-amazonaws-emrcontainers-awschicagowebservice.json): DeleteSecurityConfiguration is DELETE /securityconfigurations/{id} returning the deleted `id`; DescribeSecurityConfiguration is GET /securityconfigurations/{id} and models ResourceNotFoundException and ValidationException, both at httpError 400. The recording shows the post-delete read settling on a terminal HTTP 400 carrying an x-amzn-errortype error header and a message naming the configuration id (recorded message example: "Security Configuration 00000000083xm71kdia1bx1sv doesn't exist."). The requirement is the status (400) and that an x-amzn-errortype error code is returned for a deleted id; the exact message text is an example only - but a 2xx body is not a permissible answer for a deleted resource under any reading of the model, since the resource no longer exists.
Emulator: answers HTTP 200 with a full SecurityConfiguration body (id, name, arn, createdAt, securityConfigurationData, tags) and no x-amzn-errortype and no message. The fixture polls the read for 30 attempts and the status never leaves 200, so this is not an eventual-consistency window - the delete simply never takes effect.
Why: a client cannot tell whether its delete worked. Teardown and idempotent re-provision logic that deletes a configuration and then waits for the read to 404/400 never terminates (here, 30 polls), so cleanup loops hang; a caller that lists or reads configurations after deleting sees phantom resources, and a re-create under the same name can collide with state that should be gone. It also makes the delete operation untestable: the only observable confirmation of a delete is the subsequent read.
Fix: DeleteSecurityConfiguration's handler and the security-configuration state store in the emr-containers service crate - have delete actually remove (or tombstone) the entry, and have DescribeSecurityConfiguration's lookup miss it afterwards and raise the modeled 400 ResourceNotFoundException with the x-amzn-errortype header set.
Traces: emr-containers-security-configuration-lifecycle#teardown#7.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator_up true; /__admin/health answered 200 and the data listener accepts; state_reset_failures 0, so this is not cross-fixture state bleed).

emr-containers-security-configuration-lifecycle#teardown#7, DescribeSecurityConfiguration, expected status 400 -> actual 200:
  status: expected 400, got 200
  $.message: missing (expected "Security Configuration 00000000083xm71kdia1bx1sv doesn't exist.", got None)
  $headers.x-amzn-errortype: missing (expected '<shaped-not-asserted>' i.e. an error code header, got None)
  $: poll: status 200 not in [400] after 30 attempt(s) - got {'securityConfiguration': {'id': '00000000083xm71kdia1bx1sv', 'name': 'sim2real-probe-emr-containers-secconfig-6fd42819', 'arn': 'arn:aws:emr-containers:us-west-2:000000000000:/securityconfigurations/00000000083xm71kdia1bx1sv', 'createdAt': 1792743174.676, 'securityConfigurationData': {...}, 'tags': {}}}
  divergence_key: service=emr-containers|DescribeSecurityConfiguration|status=400->200|keys=$,$.message,$headers.x-amzn-errortype,status

The same fixture's earlier steps created the configuration and deleted it, and the delete step itself did not diverge - the emulator reported the delete as successful while leaving the resource readable.

Model evidence: DeleteSecurityConfiguration = DELETE /securityconfigurations/{id} -> {id}; DescribeSecurityConfiguration = GET /securityconfigurations/{id}, errors {InternalServerException 500, ResourceNotFoundException 400, ValidationException 400}.

Not covered by the open issue 20261022T001417Z-emr-containers-error-paths-missing-input-validation-on.md: that issue is about identifier validation on the emr-containers-error-paths fixture and carries no replay key for this lifecycle step.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from that checkout.
2. Replay trace set simparity-wire-aws-v1, fixture emr-containers-security-configuration-lifecycle.
3. Steps task#1..task#4 create a security configuration (CreateSecurityConfiguration) and read it back successfully.
4. The teardown calls DELETE /securityconfigurations/{id} (DeleteSecurityConfiguration), which returns successfully.
5. Step teardown#7 then polls GET /securityconfigurations/{id} (DescribeSecurityConfiguration) expecting HTTP 400 with an x-amzn-errortype error code; observe HTTP 200 with the full configuration body for all 30 poll attempts.
6. Re-replay emr-containers-security-configuration-lifecycle#teardown#7 to confirm the fix; the post-delete read must reach 400.
