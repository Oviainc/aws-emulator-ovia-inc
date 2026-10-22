Issue-Source: trace-sweep-divergence
Issue-Trace: backup-gateway-gateway-negative-paths
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: backup-gateway-gateway-negative-paths backup-gateway-import-hypervisor-password-validation backup-gateway-resource-tagging
Issue-Shape-Id: com.amazonaws.backupgateway#BackupOnPremises_v20210101
Issue-Key: backup-gateway|CreateGateway|4xx->2xx|$.ErrorCode
Issue-Key: backup-gateway|CreateGateway|4xx->2xx|$.Message
Issue-Key: backup-gateway|CreateGateway|4xx->2xx|$.__type
Issue-Key: backup-gateway|CreateGateway|4xx->2xx|status
Issue-Key: backup-gateway|ListGateways|2xx->2xx|$.Gateways[*]
Issue-Key: backup-gateway|ImportHypervisorConfiguration|4xx->4xx|$.message
Issue-Key: backup-gateway|TagResource|4xx->4xx|$.ErrorCode
Issue-Key: backup-gateway|TagResource|4xx->4xx|$.Message
Issue-Key: backup-gateway|TagResource|4xx->4xx|status
Issue-Commit: 5598afbcdd0af564c379fb305d1563cf7235aea7

# backup-gateway negative paths succeed or return the wrong modeled error

> _Note: this issue was identified against the code base at commit `5598afbcdd0af564c379fb305d1563cf7235aea7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateGateway

## Description
Four net-new wire divergences in the backup-gateway (BackupOnPremises) emulator share one fault family: operations that AWS rejects on a negative path are accepted, or are rejected with an error that does not match the modeled/recorded error on the wire. All four causes must be closed before this issue is submitted.

Cause 1 of 4: CreateGateway with an activation key that was never issued (fixture backup-gateway-gateway-negative-paths, step task#2).
AWS: CreateGateway models ValidationException as a client error with httpError 400 (Smithy: CreateGateway errors = InternalServerException 500, ThrottlingException 429, ValidationException 400; ValidationException has members ErrorCode and Message, ErrorCode documented as "A description of what caused the validation error"). The recorded response for an unknown activation key is HTTP 400 with __type 'com.amazonaws.backuponpremises#ValidationException', ErrorCode 'ActivationKeyNotFound' and message 'The specified activation key was not found.' (message quoted as an example; the requirements are the 400 status, the __type and the ErrorCode).
Emulator: returns HTTP 200 with a CreateGatewayOutput and no __type/ErrorCode/Message at all, i.e. it registers a gateway for an activation key that does not exist.
Why: a client that provisions a gateway from an activation key gets a success and a GatewayArn for a key the service never activated, so error handling for the single most common CreateGateway failure never runs and the client proceeds against a gateway that does not exist in AWS.
Fix: CreateGateway's handler/validation must look the ActivationKey up in service state and raise ValidationException (400, ErrorCode ActivationKeyNotFound) when it is unknown, instead of unconditionally creating state.

Cause 2 of 4: ListGateways reports the gateway that the rejected CreateGateway should never have created (fixture backup-gateway-gateway-negative-paths, step task#3).
AWS: after the CreateGateway call of cause 1 is rejected, no gateway exists, and the recorded ListGateways response carries an empty Gateways list (Smithy: ListGateways output Gateways + NextToken, paginated on Gateways).
Emulator: ListGateways returns one extra Gateways entry (a GatewayArn/GatewayDisplayName/GatewayType BACKUP_VM/LastSeenTime record) created by the call that should have failed.
Why: the failed-create path leaves durable state, so a client's list/describe view disagrees with AWS and any subsequent delete/associate step operates on a phantom gateway. Hypothesis: this is the read-side symptom of cause 1 (state written before validation), so fixing CreateGateway's ordering is expected to clear it, but it must be re-replayed to confirm.
Fix: CreateGateway's state mutation must happen only after validation succeeds; ListGateways' handler should keep returning exactly the gateways in state.

Cause 3 of 4: ImportHypervisorConfiguration's validation message echoes the sensitive Password value (fixture backup-gateway-import-hypervisor-password-validation, step task#1).
AWS: Smithy marks ImportHypervisorConfiguration.Password as @sensitive with @pattern "^[ -~]+$" and @length 1..100; the recorded 400 ValidationException message is "1 validation error detected: Value at 'password' failed to satisfy constraint: Member must satisfy regular expression pattern: [ -~]+" - it names the member and the constraint but omits the offending value, which is the documented handling of a sensitive member. The rule here is: a constraint-violation message for a @sensitive member must not include the member's value.
Emulator: same 400 status and ValidationException, but the message is "1 validation error detected: Value 'bad\tpassword' at 'password' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[ -~]+$" - the rejected password is reproduced verbatim in the error body (and the pattern is rendered with its ^...$ anchors, which the recording shows unanchored; the anchor rendering alone is an example-level difference, the value echo is the defect).
Why: a client or log pipeline that records AWS error bodies ends up storing the hypervisor password that was just rejected, which AWS never emits; code that redacts nothing because AWS redacts for it now leaks a credential.
Fix: the constraint-validation formatter for this operation must suppress the value fragment when the member carries @sensitive, and should render the pattern as the recording does.

Cause 4 of 4: TagResource on a non-existent hypervisor ARN returns the wrong error and status (fixture backup-gateway-resource-tagging, step task#6).
AWS: for a well-formed hypervisor ARN that does not exist, the recorded TagResource response is HTTP 400 with ErrorCode 'HypervisorNotFound' and message 'The hypervisor is not found.' (recorded behavior: although Smithy binds both ResourceNotFoundException at 404 and ValidationException at 400 to TagResource, the recording shows AWS answering the missing-hypervisor case with the 400/HypervisorNotFound form).
Emulator: returns HTTP 404 with no ErrorCode member and a generic message of the form "The resource '<arn>' was not found."
Why: the status class and the error code are both wrong, so a client that retries or branches on ErrorCode == HypervisorNotFound (or that treats 4xx-but-not-404 differently) takes the wrong branch; the missing ErrorCode member also breaks any consumer reading it.
Fix: TagResource's handler must map an unknown hypervisor ARN to the 400 ValidationException-shaped error carrying ErrorCode HypervisorNotFound rather than the generic 404 ResourceNotFoundException path.

No existing open issue covers backup-gateway (a listing filtered on 'backup-gateway' returned none), and bootstrap-backupgateway is already resolved, so these are fidelity defects on a stood-up service.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up; /__admin/health 200, data listener accepting; state_reset_failures 0):

Cause 1 - backup-gateway-gateway-negative-paths#task#2, CreateGateway: expected status 400, got 200; $.__type missing (expected 'com.amazonaws.backuponpremises#ValidationException'); $.ErrorCode missing (expected 'ActivationKeyNotFound'); $.Message missing (expected 'The specified activation key was not found.').

Cause 2 - backup-gateway-gateway-negative-paths#task#3, ListGateways: status 200 both, but $.Gateways[0] extra - expected None, got {'GatewayArn': 'arn:aws:backup-gateway:us-west-2:000000000000:gateway/bgw-0000000000000000', 'GatewayDisplayName': 'sim2real-probe-backup-gateway-gw-10319bc8', 'GatewayType': 'BACKUP_VM', 'LastSeenTime': 1792638397.05} (ids/names/timestamps are environment-specific; the requirement is that the list is empty).

Cause 3 - backup-gateway-import-hypervisor-password-validation#task#1, ImportHypervisorConfiguration: status 400 both; $.message value_mismatch - expected "1 validation error detected: Value at 'password' failed to satisfy constraint: Member must satisfy regular expression pattern: [ -~]+", got "1 validation error detected: Value 'bad\tpassword' at 'password' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[ -~]+$". Smithy: ImportHypervisorConfigurationInput.Password is @sensitive, pattern ^[ -~]+$.

Cause 4 - backup-gateway-resource-tagging#task#6, TagResource: expected status 400, got 404; $.ErrorCode missing (expected 'HypervisorNotFound'); $.Message expected 'The hypervisor is not found.', got "The resource 'arn:aws:backup-gateway:us-west-2:000000000000:hypervisor/hv-2222222222nonexistent' was not found.".

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from the checkout.
2. Replay trace set simparity-wire-aws-v1 and inspect the backup-gateway fixtures.
3. Cause 1+2: replay fixture backup-gateway-gateway-negative-paths - step task#2 calls CreateGateway with an activation key that was never issued (expect 400 ValidationException / ErrorCode ActivationKeyNotFound, observe 200 + CreateGatewayOutput), then step task#3 calls ListGateways (expect an empty Gateways list, observe the gateway created by the step that should have failed).
4. Cause 3: replay fixture backup-gateway-import-hypervisor-password-validation step task#1 - ImportHypervisorConfiguration with a Password containing a tab (outside ^[ -~]+$); expect a 400 ValidationException whose message names 'password' and the pattern but does NOT contain the password value, observe the value echoed in the message.
5. Cause 4: replay fixture backup-gateway-resource-tagging step task#6 - TagResource against a hypervisor ARN that does not exist; expect HTTP 400 with ErrorCode 'HypervisorNotFound', observe HTTP 404 with no ErrorCode.
