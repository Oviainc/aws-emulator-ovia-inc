Issue-Source: trace-sweep-divergence
Issue-Trace: backup-gateway-gateway-disassociate-not-found
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: backup-gateway-gateway-disassociate-not-found backup-gateway-gateway-negative-paths backup-gateway-hypervisor-connectivity-test backup-gateway-hypervisor-property-mappings backup-gateway-virtual-machine-inventory-and-sync
Issue-Shape-Id: com.amazonaws.backupgateway#BackupOnPremises_v20210101
Issue-Key: backup-gateway|DisassociateGatewayFromServer|4xx->4xx|$.ErrorCode
Issue-Key: backup-gateway|DisassociateGatewayFromServer|4xx->4xx|$.Message
Issue-Key: backup-gateway|DisassociateGatewayFromServer|4xx->4xx|status
Issue-Key: backup-gateway|AssociateGatewayToServer|4xx->4xx|$.Message
Issue-Key: backup-gateway|AssociateGatewayToServer|4xx->4xx|$.__type
Issue-Key: backup-gateway|TestHypervisorConfiguration|4xx->4xx|$.ErrorCode
Issue-Key: backup-gateway|TestHypervisorConfiguration|4xx->4xx|$.Message
Issue-Key: backup-gateway|TestHypervisorConfiguration|4xx->4xx|$.__type
Issue-Key: backup-gateway|TestHypervisorConfiguration|4xx->4xx|status
Issue-Key: backup-gateway|GetHypervisorPropertyMappings|4xx->4xx|$.ErrorCode
Issue-Key: backup-gateway|GetHypervisorPropertyMappings|4xx->4xx|$.Message
Issue-Key: backup-gateway|GetHypervisorPropertyMappings|4xx->4xx|status
Issue-Key: backup-gateway|PutHypervisorPropertyMappings|4xx->4xx|$.ErrorCode
Issue-Key: backup-gateway|GetVirtualMachine|4xx->4xx|$.ErrorCode
Issue-Key: backup-gateway|GetVirtualMachine|4xx->4xx|$.Message
Issue-Key: backup-gateway|GetVirtualMachine|4xx->4xx|status
Issue-Commit: 2216fbd2f00c4c2b6b2036585ad12df5e43cbdbf

# backup-gateway not-found and validation refusals answer 404 and emulator-authored ErrorCode/Message

> _Note: this issue was identified against the code base at commit `2216fbd2f00c4c2b6b2036585ad12df5e43cbdbf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DisassociateGatewayFromServer

## Description
SIX net-new root causes in this crate's refusal plane, all of them the same component: the single place that chooses a refusal's HTTP status, its ErrorCode and its sentence (crate::state::MissingResource -- status_override()/error_code()/message(), plus the NOT_FOUND / OUT_OF_DATE constants beside it) does not model the resource types and conditions AWS distinguishes, so handlers fall through to a generic arm. Every cause must be closed before this issue is submitted.

Shared ground truth: Backup Gateway is an awsJson1_0/rpcv2Cbor service (Smithy digest: sdkId 'Backup Gateway', protocols awsJson1_0 + rpcv2Cbor). ResourceNotFoundException carries @httpError 404 in the model, but the recorded wire answers every one of these client refusals with HTTP 400 -- recorded behavior, and the crate already acknowledges it for ONE arm (MissingResource::Hypervisor overrides to 400) while leaving the rest on the modeled 404. ResourceNotFoundException$ErrorCode is documented 'A description of which resource wasn't found' and ValidationException$ErrorCode 'A description of what caused the validation error'; AWS fills them with a per-resource/per-condition token and pairs each with a FIXED sentence that echoes no identifier. The emulator instead composes 'The resource '<arn>' was not found.' from the request ARN -- the ARN is a request value, never part of AWS's message template.

Cause 1 of 6 -- DisassociateGatewayFromServer, GatewayArn naming no gateway.
AWS: HTTP 400, ResourceNotFoundException, ErrorCode 'GatewayNotFound', Message exactly 'The specified gateway was not found.' (entirely fixed text; no variable part). Smithy: DisassociateGatewayFromServer errors = ConflictException, ResourceNotFoundException; ResourceNotFoundException$ErrorCode documented as above. Status 400 for this class: recorded behavior only.
Emulator: HTTP 404, ResourceNotFoundException with NO ErrorCode member and Message "The resource 'arn:aws:backup-gateway:<region>:<account>:gateway/<gateway-id>' was not found." (the ARN taken from the request).
Why: a client branching on the status class sees a 404 where the SDK and every other Backup Gateway refusal give 400, and a client branching on ErrorCode == 'GatewayNotFound' finds the member absent, so it cannot tell a missing gateway from any other not-found.
Fix: MissingResource needs a Gateway arm (code 'GatewayNotFound', fixed message, status 400) and this operation's refusal arm must select it instead of MissingResource::Other.
Traces: backup-gateway-gateway-disassociate-not-found#task#1.

Cause 2 of 6 -- AssociateGatewayToServer, GatewayArn naming no gateway: wrong error CLASS.
AWS: HTTP 400, ResourceNotFoundException (__type suffix ResourceNotFoundException), Message 'The specified gateway was not found.' -- same fixed sentence as cause 1.
Emulator: HTTP 400 (status correct here) but ValidationException, with the ARN-echoing message "The resource '<arn>' was not found.".
Why: an unresolvable gateway is a not-found condition, not an input-validation failure; a client that retries or re-creates on ResourceNotFoundException and surfaces ValidationException to the user takes the wrong branch. It also shows the crate answers ONE condition two ways across sibling operations (cause 1 answers 404 ResourceNotFound, this one 400 Validation).
Fix: AssociateGatewayToServer's gateway-resolution arm must refuse through the same MissingResource::Gateway choice as cause 1.
Traces: backup-gateway-gateway-negative-paths#task#4.

Cause 3 of 6 -- TestHypervisorConfiguration with an unresolvable GatewayArn: AWS validates rather than resolving.
AWS: HTTP 400, ValidationException, ErrorCode 'InvalidParameter', Message exactly 'The specified request contains invalid parameters.' (fixed text, no variable part). Smithy: TestHypervisorConfiguration errors = ConflictException, ResourceNotFoundException, plus the service-common ValidationException; ValidationException$ErrorCode documented 'A description of what caused the validation error'. Which of the two classes this path takes: recorded behavior only.
Emulator: HTTP 404, ResourceNotFoundException, no ErrorCode, Message "The resource '<gateway-arn>' was not found.".
Why: the caller is told the gateway is missing when AWS reports the request itself invalid, and gets 404 where AWS gives 400 -- so neither the status nor the error class a client switches on matches.
Fix: TestHypervisorConfiguration's handler must refuse an unresolvable GatewayArn with ValidationException/'InvalidParameter' at 400 rather than routing through MissingResource.
Traces: backup-gateway-hypervisor-connectivity-test#task#1.

Cause 4 of 6 -- GetHypervisorPropertyMappings on a hypervisor that carries no mappings: wrong status, wrong ErrorCode token, wrong sentence.
AWS: HTTP 400, ResourceNotFoundException, ErrorCode 'HypervisorPropertyMappingsNotFound', Message exactly 'The hypervisor property mappings is not found.' (fixed text).
Emulator: HTTP 404, ErrorCode 'MappingsNotFound', Message 'The mappings are not found.' -- both strings are this crate's own invention (state.rs constants MAPPINGS_NOT_FOUND / MAPPINGS_NOT_FOUND_MESSAGE), and MissingResource::PropertyMappings deliberately returns status_override() == None, keeping the modeled 404.
Why: the ErrorCode token and the message text are both requirements of this error class; a client matching 'HypervisorPropertyMappingsNotFound' or asserting the sentence never matches, and the status class is wrong on top.
Fix: MissingResource::PropertyMappings must carry ErrorCode 'HypervisorPropertyMappingsNotFound', the AWS sentence and status 400.
Traces: backup-gateway-hypervisor-property-mappings#task#3.

Cause 5 of 6 -- PutHypervisorPropertyMappings refused because the gateway is out of date: invented ErrorCode token.
AWS: HTTP 400 with the out-of-date refusal whose ErrorCode is, verbatim as recorded, ' OutdatedGateway' -- including the LEADING SPACE, which is part of AWS's fixed token and not a transcription slip. The sentence the emulator already sends matched and is not at issue. Recorded behavior only (AWS publishes no vocabulary for ValidationException$ErrorCode).
Emulator: ErrorCode 'GatewayOutOfDate' (state.rs constant GATEWAY_OUT_OF_DATE), status and message otherwise agreeing.
Why: the ErrorCode is the machine-readable half of this refusal; a client keyed on the recorded token never matches.
Fix: the GATEWAY_OUT_OF_DATE constant in crate::state must hold AWS's token (leading space included) so every operation carrying the gateway-out-of-date refusal emits it.
Traces: backup-gateway-hypervisor-property-mappings#task#4.

Cause 6 of 6 -- GetVirtualMachine, ResourceArn naming no virtual machine.
AWS: HTTP 400, ResourceNotFoundException, ErrorCode 'VirtualMachineNotFound', Message exactly 'The virtual machine is not found.' (fixed text, no identifier echoed). Smithy: GetVirtualMachine errors = ResourceNotFoundException; ErrorCode documented as above.
Emulator: HTTP 404, no ErrorCode, Message "The resource '<vm-arn>' was not found." -- src/ops/get_virtual_machine.rs explicitly selects MissingResource::Other, whose error_code() is None and whose message() is the ARN-echoing resource_not_found_message().
Why: same as cause 1 -- wrong status class and a missing discriminator, so a client cannot distinguish a missing machine and sees a message AWS never sends.
Fix: MissingResource needs a VirtualMachine arm (code 'VirtualMachineNotFound', fixed message, status 400) selected by this handler.
Traces: backup-gateway-virtual-machine-inventory-and-sync#task#4.

Not covered by existing open issues: 20261028T161930Z-backup-gateway-getgateway-error-response.md (the GetGateway path bypassing the MissingResource choice) and 20261029T000229Z-backup-gateway-tagresource-error-response.md (404-vs-400 on the tagging ARNs) are adjacent to the same component but name different operations and different fixture steps; neither carries the replay keys above. 20261022T030803Z-backup-gateway-negative-paths-succeed-or-return-the.md is resolved and does not cover these.

## Evidence
Replay of trace set simparity-wire-aws-v1, 6 mismatch clusters, all service=backup-gateway:

1) backup-gateway-gateway-disassociate-not-found#task#1 DisassociateGatewayFromServer -- status: expected 400, got 404; $.ErrorCode: missing (expected 'GatewayNotFound', got None); $.Message: expected 'The specified gateway was not found.', got "The resource 'arn:aws:backup-gateway:us-west-2:000000000000:gateway/gw-1111111111nonexistent' was not found."

2) backup-gateway-gateway-negative-paths#task#4 AssociateGatewayToServer -- $.__type: expected 'com.amazonaws.backuponpremises#ResourceNotFoundException', got 'ValidationException'; $.Message: expected 'The specified gateway was not found.', got "The resource 'arn:aws:backup-gateway:us-west-2:000000000000:gateway/gw-0000000000nonexistent' was not found." (status 400 matched)

3) backup-gateway-hypervisor-connectivity-test#task#1 TestHypervisorConfiguration -- status: expected 400, got 404; $.__type: expected 'com.amazonaws.backuponpremises#ValidationException', got 'ResourceNotFoundException'; $.ErrorCode: missing (expected 'InvalidParameter'); $.Message: expected 'The specified request contains invalid parameters.', got "The resource 'arn:aws:backup-gateway:us-west-2:000000000000:gateway/gw-3333333333nonexistent' was not found."

4) backup-gateway-hypervisor-property-mappings#task#3 GetHypervisorPropertyMappings -- status: expected 400, got 404; $.ErrorCode: expected 'HypervisorPropertyMappingsNotFound', got 'MappingsNotFound'; $.Message: expected 'The hypervisor property mappings is not found.', got 'The mappings are not found.'

5) backup-gateway-hypervisor-property-mappings#task#4 PutHypervisorPropertyMappings -- $.ErrorCode: expected ' OutdatedGateway' (leading space verbatim), got 'GatewayOutOfDate' (status 400 and Message matched)

6) backup-gateway-virtual-machine-inventory-and-sync#task#4 GetVirtualMachine -- status: expected 400, got 404; $.ErrorCode: missing (expected 'VirtualMachineNotFound', got None); $.Message: expected 'The virtual machine is not found.', got "The resource 'arn:aws:backup-gateway:us-west-2:000000000000:vm/vm-nonexistent00' was not found."

Source of the behavior in the checkout: services/service-com-amazonaws-backupgateway-backuponpremises-v20210101/src/state.rs -- MissingResource (enum at ~L2933, error_code() ~L2970, message() ~L2979, status_override() ~L2990: only the Hypervisor arm overrides to 400) and the constants MAPPINGS_NOT_FOUND ~L275, MAPPINGS_NOT_FOUND_MESSAGE ~L280, GATEWAY_OUT_OF_DATE ~L328; and src/ops/get_virtual_machine.rs ~L175 selecting MissingResource::Other.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus) and read the backup-gateway clusters.
3. Per cause, with a well-formed ARN that resolves to nothing:
   a. DisassociateGatewayFromServer{GatewayArn: arn:aws:backup-gateway:<region>:<account>:gateway/<unused-id>} -> expect 400 ResourceNotFoundException, ErrorCode 'GatewayNotFound', Message 'The specified gateway was not found.'; observe 404 with no ErrorCode and an ARN-echoing message. (fixture backup-gateway-gateway-disassociate-not-found step task#1)
   b. AssociateGatewayToServer with the same unused gateway ARN -> expect ResourceNotFoundException with that same sentence; observe ValidationException. (backup-gateway-gateway-negative-paths task#4)
   c. TestHypervisorConfiguration with an unused GatewayArn -> expect 400 ValidationException, ErrorCode 'InvalidParameter', Message 'The specified request contains invalid parameters.'; observe 404 ResourceNotFoundException. (backup-gateway-hypervisor-connectivity-test task#1)
   d. ImportHypervisorConfiguration, then GetHypervisorPropertyMappings on that hypervisor before any PutHypervisorPropertyMappings -> expect 400, ErrorCode 'HypervisorPropertyMappingsNotFound', Message 'The hypervisor property mappings is not found.'; observe 404 / 'MappingsNotFound' / 'The mappings are not found.'. (backup-gateway-hypervisor-property-mappings task#3)
   e. Drive PutHypervisorPropertyMappings into its gateway-out-of-date refusal -> expect ErrorCode ' OutdatedGateway'; observe 'GatewayOutOfDate'. (backup-gateway-hypervisor-property-mappings task#4)
   f. GetVirtualMachine with a vm/<id> ARN no sync ever recorded -> expect 400, ErrorCode 'VirtualMachineNotFound', Message 'The virtual machine is not found.'; observe 404 with no ErrorCode and an ARN-echoing message. (backup-gateway-virtual-machine-inventory-and-sync task#4)
