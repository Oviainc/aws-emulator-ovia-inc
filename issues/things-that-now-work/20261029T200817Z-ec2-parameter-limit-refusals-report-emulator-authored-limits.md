Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-page-describe-instances-tag-filtered-two-pages
Issue-Step: task#15
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-page-describe-instances-tag-filtered-two-pages ec2-page-describe-volumes-tag-filtered-two-pages ec2-page-describe-launch-template-versions-two-pages ec2-run-instances-validation-and-diagnostic-interrupt ec2-vpc-attributes-and-account-settings ec2-instance-attribute-reset-roundtrip ec2-capacity-reservation-error-paths ec2-capacity-reservation-interruptible-allocation-error ec2-allocate-hosts-invalid-instance-type-error ec2-allocate-hosts-invalid-zone-error
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeInstances|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|RunInstances|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|ModifyVpcAttribute|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|ResetInstanceAttribute|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|AllocateHosts|4xx->4xx|$.Response.Errors.Error.Code
Issue-Related: 20261029T200817Z-ec2-id-validation-verdict-is-inverted-malformed-ids.md
Issue-Related: 20261029T200817Z-ec2-not-found-and-malformed-refusals-emit-generic.md
Issue-Commit: c42b5bef1562db44010b2579a8522ff39a3564f6

# EC2 parameter-limit refusals report emulator-authored limits instead of AWS's parameter messages

> _Note: this issue was identified against the code base at commit `c42b5bef1562db44010b2579a8522ff39a3564f6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstances

## Description
Eight net-new causes, one defect: when a request violates a numeric bound or an attribute-selection rule, the emulator refuses with its OWN description of the limit instead of AWS's parameter message - and in one case with the wrong error code as well. The per-operation bounds AWS states in these messages differ from the emulator's assumed bounds, so the text is not a paraphrase but a different contract. ec2 models no error shapes in Smithy; each expectation is recorded behavior only, from the recorded 400.

Cause 1 of 8 - DescribeInstances with MaxResults below the minimum (ec2-page-describe-instances-tag-filtered-two-pages, task#15). AWS: 'Value ( <n> ) for parameter maxResults is invalid. Expecting a value greater than 5.' - fixed text with the request's value inside spaced parentheses, and AWS's real floor for this operation is 5, not 1. Emulator: "Invalid value '<n>' for maxResults. It must be at least 1". Fix: DescribeInstances' pagination validation (bound and message).

Cause 2 of 8 - DescribeVolumes with the same violation (ec2-page-describe-volumes-tag-filtered-two-pages#task#11): AWS uses the identical 'Expecting a value greater than 5.' template; the emulator uses its 'at least 1' prose.

Cause 3 of 8 - DescribeLaunchTemplateVersions with MaxResults out of range (ec2-page-describe-launch-template-versions-two-pages#task#7). AWS: 'Maximum results allowed are between 1 and 200' - fixed text, no value interpolated. Emulator: "Invalid value '<n>' for maxResults. MaxResults must be between 1 and 200".

Cause 4 of 8 - RunInstances with MaxCount < MinCount (ec2-run-instances-validation-and-diagnostic-interrupt#task#5). AWS: 'Maximum instance count must not be smaller than minimum instance count' - fixed text only. Emulator: "Invalid value '<n>' for maxCount. MaxCount must be greater than or equal to MinCount".

Cause 5 of 8 - ModifyVpcAttribute with two attributes in one request (ec2-vpc-attributes-and-account-settings#task#14). AWS: 'Fields for multiple attribute types specified: <attr1>, <attr2>' - fixed prefix plus the member names from the request. Emulator: 'You cannot modify <attr1> and <attr2> in the same request. Use separate requests for each attribute.'

Cause 6 of 8 - ResetInstanceAttribute for a non-resettable attribute (ec2-instance-attribute-reset-roundtrip#task#6). AWS: 'Value (<attribute>) for parameter attribute is invalid. Unknown attribute.' Emulator: 'Value (<attribute>) for parameter attribute is invalid. You can only reset the following attributes: kernel | ramdisk | sourceDestCheck' - it keeps AWS's prefix but replaces the trailing sentence with its own enumeration.

Cause 7 of 8 - CreateInterruptibleCapacityReservationAllocation with an out-of-range instance count (ec2-capacity-reservation-error-paths#task#5). AWS states the absolute allowed range: "The instance count isn't valid. Enter a value between 1 and 1000 instances, and try again". Emulator reports availability in the target reservation instead: "Invalid value '<n>' for instanceCount: Capacity Reservation '<cr-id>' has <m> instances available for allocation".

Cause 8 of 8 - AllocateHosts with an unknown instance type (ec2-allocate-hosts-invalid-instance-type-error#task#1). Code AND message diverge: AWS answers `InvalidHostConfiguration`, "The instance type '<type>' is not supported."; the emulator answers `InvalidParameterValue`, "Invalid value '<type>' for instanceType. EC2 offers no such instance type". Smithy: AllocateHosts models InstanceType as a plain String (no enum), so the supported-type check and its error class live entirely in the handler.

Why: a client that reads the limit out of the message (or branches on the code, cause 8) programs the wrong bound; pagination callers in particular retry with a value the emulator accepts but AWS rejects, so the emulated contract is looser than the real one.

Fix: the parameter-validation step of each named operation under services/service-com-amazonaws-ec2-amazonec2/src/ops/ - both the bound and the emitted message, plus the error code for cause 8.

Traces: representative step plus the seven fixture#step bindings in trace_ids.

## Evidence
Replay detail, verbatim (all 400 expected / 400 returned):
1. DescribeInstances: expected 'Value ( 0 ) for parameter maxResults is invalid. Expecting a value greater than 5.'; actual "Invalid value '0' for maxResults. It must be at least 1" (cluster also covers task#16).
2. DescribeVolumes: same expected template; actual "Invalid value '0' for maxResults. It must be at least 1".
3. DescribeLaunchTemplateVersions: expected 'Maximum results allowed are between 1 and 200'; actual "Invalid value '0' for maxResults. MaxResults must be between 1 and 200" (cluster covers task#7..task#9).
4. RunInstances: expected 'Maximum instance count must not be smaller than minimum instance count'; actual "Invalid value '1' for maxCount. MaxCount must be greater than or equal to MinCount".
5. ModifyVpcAttribute: expected 'Fields for multiple attribute types specified: enableDnsSupport, enableDnsHostnames'; actual 'You cannot modify enableDnsSupport and enableDnsHostnames in the same request. Use separate requests for each attribute.'
6. ResetInstanceAttribute: expected 'Value (instanceType) for parameter attribute is invalid. Unknown attribute.'; actual 'Value (instanceType) for parameter attribute is invalid. You can only reset the following attributes: kernel | ramdisk | sourceDestCheck'.
7. CreateInterruptibleCapacityReservationAllocation: expected "The instance count isn't valid. Enter a value between 1 and 1000 instances, and try again"; actual "Invalid value '1000000' for instanceCount: Capacity Reservation 'cr-00000000000000022' has 1 instances available for allocation" (cluster also covers ec2-capacity-reservation-interruptible-allocation-error#task#2).
8. AllocateHosts: expected code 'InvalidHostConfiguration' / "The instance type 'sim2real-bogus.type' is not supported."; actual 'InvalidParameterValue' / "Invalid value 'sim2real-bogus.type' for instanceType. EC2 offers no such instance type" (cluster also covers ec2-allocate-hosts-invalid-zone-error#task#1).
All eight clusters: verdict=mismatch, stub_path empty, owned_by empty.

## Steps to Reproduce
1. Clone, boot the emulator built from the checkout.
2. replay_wire_corpus on simparity-wire-aws-v1.
3. For ec2-page-describe-instances-tag-filtered-two-pages step task#15 and each fixture#step in trace_ids, compare the emulator's 400 code/message with the recorded one.
