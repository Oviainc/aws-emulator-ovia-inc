Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.create-managed-prefix-list ec2.modify-vpc-endpoint-payer-responsibility-invalid-value-rejected ec2.op-contract-delete-ipam-pool
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.create-managed-prefix-list|run#20|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: ec2.modify-vpc-endpoint-payer-responsibility-invalid-value-rejected|run#21|AssertionError|assert code == "InvalidParameter", f"expected InvalidParameter, got {code!r}"
Issue-Key: ec2.op-contract-delete-ipam-pool|run#22|AssertionError|assert code == "InvalidIpamPoolId.NotFound", f"expected InvalidIpamPoolId.NotFound, got {code}"
Issue-Commit: 6bf1871c82f616f42fdbabf9cffce393eeefa7cd

# EC2 refusal fidelity: 3 net-new divergences in error code and message on implemented reject paths (CreateManagedPrefixList, ModifyVpcEndpointPayerResponsibility enum validation, DeleteIpamPool)

> _Note: this issue was identified against the code base at commit `6bf1871c82f616f42fdbabf9cffce393eeefa7cd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateManagedPrefixList

## Description
Three distinct net-new root causes in how the EC2 crate words and classifies refusals on paths that DO answer (they are not 501 stubs). Each cause below must be closed before this issue is submitted. None of them is covered by the similar open issues 20261029T175114Z-ec2-refusal-messages-are-emulator-authored-prose-instead.md or 20261029T175114Z-ec2-client-error-classification-wrong-error-codes-for.md: neither carries these checks' replay keys, so they do not cover these three paths.

Cause 1 of 3: CreateManagedPrefixList refuses an over-length PrefixListName with the right code but emulator-authored prose instead of AWS's message.
AWS: Smithy (services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json) models CreateManagedPrefixList.PrefixListName as a required String and documents the name as "up to 255 characters in length"; the recorded refusal for a longer name is HTTP 400, ec2Query error code InvalidParameterValue, message 'The prefix list name is too long. A prefix list name can be up to (255) characters in length.' Every word of that message is fixed text; the only variable part is the limit, which is the modeled/documented 255 and is rendered in parentheses.
Emulator: answers code InvalidParameterValue (correct) with the message 'prefixListName accepts at most 255 characters' — the emulator's own validator wording, naming the wire member in lowerCamelCase and omitting AWS's sentence form.
Why: a client that matches, logs or surfaces the refusal text (CLI output, CDK/CFN diagnostics, test assertions on the documented message) sees a message AWS never emits, so message-driven handling and user-facing diagnostics diverge even though the code matches.
Fix: the message template in the length validation for PrefixListName in CreateManagedPrefixList's handler/validation.
Traces: ec2.create-managed-prefix-list.

Cause 2 of 3: an out-of-enum PayerResponsibility is refused by the generic modeled-constraint layer as ValidationError instead of EC2's InvalidParameter.
AWS: Smithy models ModifyVpcEndpointPayerResponsibility.PayerResponsibility as required enum PayerResponsibilityType (values vpc-endpoint-account, resource-gateway-account, vpc-endpoint-service-account) and Scope as required enum PayerResponsibilityScope (vpc-endpoint-charges, resource-gateway-charges). The recorded refusal of a value outside that enum ('requester-pays') is the ec2Query error code InvalidParameter with message 'Invalid payer responsibility. Valid values are: vpc-endpoint-account, vpc-endpoint-service-account' — fixed text plus the list of values AWS accepts for the endpoint scope.
Emulator: answers error code ValidationError (the shared modeled-constraint/ValidationException channel), so the refusal is not an EC2 ec2Query error at all and carries none of AWS's wording.
Why: EC2 clients branch on ec2Query codes; 'ValidationError' is not an EC2 error code, so SDK/CLI error handling, retry classification and user diagnostics all miss the parameter error they are written for.
Fix: the EC2 enum/constraint rejection mapping for this operation's input (it must emit the service's InvalidParameter ec2Query error, with AWS's message, instead of the generic ValidationException), in ModifyVpcEndpointPayerResponsibility's validation path.
Traces: ec2.modify-vpc-endpoint-payer-responsibility-invalid-value-rejected.

Cause 3 of 3: DeleteIpamPool on an id that has finished deleting answers IncorrectState instead of the not-found code.
AWS: Smithy models DeleteIpamPool.IpamPoolId as required (DeleteIpamPool models no errors, so recorded behavior only): a second DeleteIpamPool on a pool that has reached delete-complete is refused with ec2Query code InvalidIpamPoolId.NotFound and message "The ipam-pool ID '<ipam-pool-id>' does not exist" — fixed text with the pool id taken verbatim from the request.
Emulator: answers code IncorrectState, i.e. it still finds the deleted pool in state and classifies the call as a state conflict rather than reporting the id as absent.
Why: a client cleaning up (or retrying a delete after a timeout) treats NotFound as success-by-idempotence and IncorrectState as a transient conflict to retry, so teardown loops spin or fail instead of completing.
Fix: the terminal-state/absence handling in DeleteIpamPool's handler (a pool in delete-complete must resolve as absent for mutations) and its not-found refusal message.
Traces: ec2.op-contract-delete-ipam-pool.

## Evidence
Cause 1 — ec2.create-managed-prefix-list [mismatch]: `AssertionError: expected message 'The prefix list name is too long. A prefix list name can be up to (255) characters in length.', got 'prefixListName accepts at most 255 characters'`. Code InvalidParameterValue matched; only the message diverged. Fixed text: the whole sentence pair; variable part: the limit 255 (the modeled/documented maximum).

Cause 2 — ec2.modify-vpc-endpoint-payer-responsibility-invalid-value-rejected [mismatch]: `AssertionError: expected InvalidParameter, got 'ValidationError'`. Request: ModifyVpcEndpointPayerResponsibility(ServiceId=<service>, VpcEndpointId=<endpoint>, PayerResponsibility='requester-pays', Scope='vpc-endpoint-charges') against a real available interface endpoint. Expected: code InvalidParameter, message 'Invalid payer responsibility. Valid values are: vpc-endpoint-account, vpc-endpoint-service-account' (all fixed text; the value list is AWS's, not the request's). Actual: code ValidationError.

Cause 3 — ec2.op-contract-delete-ipam-pool [mismatch]: `AssertionError: expected InvalidIpamPoolId.NotFound, got IncorrectState`. Sequence: CreateIpam -> CreateIpamPool -> poll create-complete -> DeleteIpamPool (succeeds, pool polls to delete-complete/absent) -> DeleteIpamPool again. Expected: code InvalidIpamPoolId.NotFound, message "The ipam-pool ID '<pool-id>' does not exist" (fixed text; <pool-id> is the id from the request). Actual: code IncorrectState.

## Steps to Reproduce
1. Boot the emulator and point an EC2 client at it with credentials it accepts.
2. Cause 1: call CreateManagedPrefixList(PrefixListName='x'*256, AddressFamily='IPv4', MaxEntries=1). Observe code InvalidParameterValue with message 'prefixListName accepts at most 255 characters' instead of 'The prefix list name is too long. A prefix list name can be up to (255) characters in length.'
3. Cause 2: create a VPC, subnet, internal NLB, a VPC endpoint service configuration and an interface VPC endpoint on it; then call ModifyVpcEndpointPayerResponsibility(ServiceId=..., VpcEndpointId=..., PayerResponsibility='requester-pays', Scope='vpc-endpoint-charges'). Observe code ValidationError instead of InvalidParameter with 'Invalid payer responsibility. Valid values are: vpc-endpoint-account, vpc-endpoint-service-account'.
4. Cause 3: CreateIpam, CreateIpamPool in its private default scope, wait for create-complete, DeleteIpamPool, wait until the pool is delete-complete/absent, then call DeleteIpamPool on the same id. Observe IncorrectState instead of InvalidIpamPoolId.NotFound / "The ipam-pool ID '<pool-id>' does not exist".
5. Re-replay the three check ids in trace_ids to confirm.
