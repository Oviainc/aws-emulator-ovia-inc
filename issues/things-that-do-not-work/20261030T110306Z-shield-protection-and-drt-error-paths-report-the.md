Issue-Source: trace-sweep-divergence
Issue-Trace: shield-drt-access-lifecycle
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: shield-drt-access-lifecycle shield-protection-error-paths
Issue-Shape-Id: com.amazonaws.shield#AWSShield_20160616
Issue-Key: shield|AssociateDRTRole|4xx->4xx|$.__type
Issue-Key: shield|AssociateDRTRole|4xx->4xx|$.message
Issue-Key: shield|CreateProtection|4xx->4xx|$.__type
Issue-Key: shield|CreateProtection|4xx->4xx|$.message
Issue-Key: shield|DescribeProtection|4xx->4xx|$.message
Issue-Key: shield|DeleteProtection|4xx->4xx|$.message
Issue-Commit: 14fcb532a6243119e1963a7bcc0017c1001c0e15

# shield protection and DRT error paths report the wrong exception and message

> _Note: this issue was identified against the code base at commit `14fcb532a6243119e1963a7bcc0017c1001c0e15`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateProtection

## Description
Four shield failure paths answer HTTP 400 but with the wrong exception type or the wrong message text, because the handlers run their Shield-Advanced-subscription gate in the wrong position relative to the other preconditions and template request values into messages AWS keeps constant. 4 causes.

Cause 1 of 4: AssociateDRTRole on an account whose support plan does not permit SRT access is reported as a missing subscription instead of an invalid operation. (fixture shield-drt-access-lifecycle, step task#2)
AWS: Smithy AssociateDRTRole models both InvalidOperationException (HTTP 400) and ResourceNotFoundException (HTTP 400); the operation doc states "To use the services of the SRT and make an AssociateDRTRole request, you must be subscribed to the Business Support plan or the Enterprise Support plan." Recorded response: HTTP 400, __type InvalidOperationException, message "Operation is invalid for your AWS Support plan." — that message is entirely fixed text for this error class, with no part taken from the request or from state. The support-plan precondition is therefore evaluated before (or in place of) any subscription lookup.
Emulator: HTTP 400 with __type ResourceNotFoundException and message "The subscription does not exist." — the handler's first precondition is the subscription-state gate, so the support-plan refusal is never reachable.
Why: a client branches on the error code: InvalidOperationException means "upgrade the support plan, the call will never work as-is", while ResourceNotFoundException means "subscribe to Shield Advanced and retry". The emulator sends every caller down the subscribe-and-retry path, and SDK/`botocore` exception classes (`Shield.Client.exceptions.InvalidOperationException`) never match.
Fix: the AssociateDRTRole handler's precondition ordering and error construction — the support-plan gate must be evaluated and must emit InvalidOperationException with Shield's fixed wording, rather than falling through to the subscription gate.
Traces: shield-drt-access-lifecycle#task#2.

Cause 2 of 4: CreateProtection against a resource ARN that does not resolve is reported as a missing subscription instead of an invalid resource. (fixture shield-protection-error-paths, step task#1)
AWS: Smithy CreateProtection models InvalidResourceException (HTTP 400, "Exception that indicates that the resource is invalid. You might not have access to the resource, or the resource might not exist.") alongside ResourceNotFoundException. Recorded response: HTTP 400, __type InvalidResourceException, message of the form "Failed to validate EIP resource: <ResourceArn> in region: <region>, error: The allocation ID '<allocation-id>' does not exist (Service: AmazonEC2; Status Code: 400; Error Code: InvalidAllocationID.NotFound; Request ID: <request-id>; Proxy: null)". Fixed text: "Failed to validate EIP resource: ", " in region: ", ", error: The allocation ID '", "' does not exist (Service: AmazonEC2; Status Code: 400; Error Code: InvalidAllocationID.NotFound; Request ID: ", "; Proxy: null)". Variable parts, taken from the request: the ResourceArn verbatim, the region parsed out of that ARN, and the allocation id (the ARN's resource segment). The embedded Request ID is a per-run value and is not part of the requirement. So AWS validates the target resource before it consults the subscription.
Emulator: HTTP 400 with __type ResourceNotFoundException and message "The subscription does not exist." — the subscription gate runs first, so a malformed/unresolvable ResourceArn is never diagnosed.
Why: a client protecting a resource that does not exist is told the wrong thing to fix — it subscribes (or reports a subscription problem) when the actual defect is its ARN, and code catching InvalidResourceException to fall back to a different resource never fires.
Fix: the CreateProtection handler's precondition ordering plus its InvalidResourceException construction — resource resolution is judged before the subscription gate, and its failure is rendered in Shield's "Failed to validate <type> resource: ..." form.
Traces: shield-protection-error-paths#task#1.

Cause 3 of 4: DescribeProtection on an unsubscribed account reports a per-resource "no protection" message instead of the account-level subscription message. (fixture shield-protection-error-paths, step task#2)
AWS: Smithy DescribeProtection models only InvalidParameterException and ResourceNotFoundException (both HTTP 400). Recorded response: HTTP 400, __type ResourceNotFoundException, message "The subscription does not exist." — wholly fixed text, nothing from the request. The account-level subscription gate is evaluated before any protection lookup, so the identifier in the request never reaches the message.
Emulator: same status and code, but message "The resource <ResourceArn> has no protection." — the protection lookup ran first and interpolated the requested ARN.
Why: the message is the only thing separating several same-code Shield failures, so a client cannot distinguish "this account has no Shield Advanced subscription" from "subscribed, but this resource is unprotected"; it skips the subscribe step and retries forever.
Fix: the DescribeProtection handler's gate ordering — the subscription check precedes the protection/resource lookup, and emits the constant subscription message. Related open issue 20261029T200731Z-shield-validation-rejections-carry-non-aws-error-messages.md mentions this leg in prose under its cause 2, but it is keyed to behavioral checks and carries no fixture step for it, so it does not cover this divergence.
Traces: shield-protection-error-paths#task#2.

Cause 4 of 4: DeleteProtection's not-found message interpolates the requested protection id, where AWS's text is constant. (fixture shield-protection-error-paths, step task#3)
AWS: Smithy DeleteProtection takes a required ProtectionId and models ResourceNotFoundException (HTTP 400). Recorded response: HTTP 400, __type ResourceNotFoundException, message "The referenced protection does not exist." — the entire message is fixed text for this error class; no id, name or other request/state value appears in it.
Emulator: same status and code with message "The protection <ProtectionId> does not exist.", built by formatting the request's ProtectionId into a template of its own.
Why: message-based classification and assertions against Shield's documented wording fail, and the emulator echoes a caller-supplied identifier in an error AWS deliberately keeps value-free.
Fix: the DeleteProtection handler's not-found error construction (the shared not-found message constants for protections).
Traces: shield-protection-error-paths#task#3.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator (emulator_up=true, bootstrap_signal=false), service shield:

1. shield-drt-access-lifecycle#task#2 AssociateDRTRole — expected 400 / actual 400; $.__type: expected 'InvalidOperationException', got 'ResourceNotFoundException'; $.message: expected 'Operation is invalid for your AWS Support plan.', got 'The subscription does not exist.'.

2. shield-protection-error-paths#task#1 CreateProtection — expected 400 / actual 400; $.__type: expected 'InvalidResourceException', got 'ResourceNotFoundException'; $.message: expected "Failed to validate EIP resource: arn:aws:ec2:us-east-1:<account>:eip-allocation/<allocation-id> in region: us-east-1, error: The allocation ID '<allocation-id>' does not exist (Service: AmazonEC2; Status Code: 400; Error Code: InvalidAllocationID.NotFound; Request ID: <per-run request id>; Proxy: null)", got 'The subscription does not exist.'.

3. shield-protection-error-paths#task#2 DescribeProtection — expected 400 / actual 400; $.message: expected 'The subscription does not exist.', got 'The resource <ResourceArn> has no protection.'.

4. shield-protection-error-paths#task#3 DeleteProtection — expected 400 / actual 400; $.message: expected 'The referenced protection does not exist.', got 'The protection <ProtectionId> does not exist.'.

Source in the checkout: services/service-com-amazonaws-shield-awsshield-20160616/src/ops/associate_drt_role.rs lines 182-196 make the subscription-state gate the handler's first precondition and return ResourceNotFoundException(SUBSCRIPTION_NOT_FOUND_MESSAGE), with no support-plan gate anywhere in the handler and InvalidOperationException left unconstructed. services/service-com-amazonaws-shield-awsshield-20160616/src/ops/describe_protection.rs lines 186-201 build the per-resource/per-id not-found messages ahead of any account-level subscription check. The subscription message and resource-type constants live in services/service-com-amazonaws-shield-awsshield-20160616/src/state.rs (SUBSCRIPTION_NOT_FOUND_MESSAGE, SUBSCRIPTION_RESOURCE_TYPE).

Smithy (services/service-com-amazonaws-shield-awsshield-20160616/smithy-model/com-amazonaws-shield-awsshield-20160616.json): AssociateDRTRole errors = AccessDeniedForDependencyException, InternalErrorException, InvalidOperationException, InvalidParameterException, OptimisticLockException, ResourceNotFoundException; CreateProtection errors include InvalidResourceException and ResourceNotFoundException; DescribeProtection errors = InternalErrorException, InvalidParameterException, ResourceNotFoundException; DeleteProtection errors = InternalErrorException, OptimisticLockException, ResourceNotFoundException. All client errors are HTTP 400.

## Steps to Reproduce
Boot the emulator and point a shield client at it, with the account holding no active Shield Advanced subscription (the state the fixtures replay against).

1. associate_drt_role(RoleArn='arn:aws:iam::<account>:role/AWSSRTAccessRole') -> emulator: 400 ResourceNotFoundException 'The subscription does not exist.'; AWS: 400 InvalidOperationException 'Operation is invalid for your AWS Support plan.'. (shield-drt-access-lifecycle#task#2)
2. create_protection(Name='<valid name>', ResourceArn='arn:aws:ec2:<region>:<account>:eip-allocation/<nonexistent allocation id>') -> emulator: 400 ResourceNotFoundException 'The subscription does not exist.'; AWS: 400 InvalidResourceException "Failed to validate EIP resource: <arn> in region: <region>, error: The allocation ID '<allocation id>' does not exist (Service: AmazonEC2; Status Code: 400; Error Code: InvalidAllocationID.NotFound; Request ID: <per-run>; Proxy: null)". (shield-protection-error-paths#task#1)
3. describe_protection(ResourceArn='<that same arn>') -> emulator: 400 ResourceNotFoundException 'The resource <arn> has no protection.'; AWS: 400 ResourceNotFoundException 'The subscription does not exist.'. (shield-protection-error-paths#task#2)
4. delete_protection(ProtectionId='<any 36-char protection id>') -> emulator: 400 ResourceNotFoundException 'The protection <id> does not exist.'; AWS: 400 ResourceNotFoundException 'The referenced protection does not exist.'. (shield-protection-error-paths#task#3)

Re-replay shield-drt-access-lifecycle#task#2 and shield-protection-error-paths#task#1, #task#2 and #task#3 to confirm all four causes are closed.
