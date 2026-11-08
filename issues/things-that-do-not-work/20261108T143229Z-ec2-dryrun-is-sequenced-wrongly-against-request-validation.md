Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-trajwire-deb-deb-instance-profile-named-by-role
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-trajwire-deb-deb-instance-profile-named-by-role ec2-trajwire-deb-deb-instance-type-casing-and-absent-ami ec2-trajwire-deb-deb-key-pair-named-before-created ec2-trajwire-deb-deb-security-group-name-passed-with-subnet ec2-trajwire-lia-dry-run-permission-probe
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|RunInstances|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|DescribeInstances|4xx->4xx|$.Response.Errors.Error.Message
Issue-Related: 20261103T071133Z-ec2-dryrun-is-never-honoured-dry-run-probes.md
Issue-Related: 20261103T071133Z-ec2-invalid-enum-and-attribute-values-are-refused.md
Issue-Related: 20261103T071133Z-ec2-pagination-and-filter-validation-refusals-use-emulator.md
Issue-Commit: 4466ef5882b584b00b87b338b6098c1e65748c9e

# EC2 DryRun is sequenced wrongly against request validation

> _Note: this issue was identified against the code base at commit `4466ef5882b584b00b87b338b6098c1e65748c9e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunInstances

## Description
This issue bundles 2 causes. Both are the same fault: the emulator runs its DryRun short-circuit at a fixed point in the request pipeline instead of at the point AWS evaluates it for that operation, so a dry-run request gets the wrong one of the two possible refusals. It is NOT covered by the open issues 20261029T175114Z-ec2-dryrun-contract-authorized-dryrun-requests-answer-notimplemented.md or 20261103T071133Z-ec2-dryrun-is-never-honoured-dry-run-probes.md (those cover DryRun answering NotImplemented/501 or never refusing); the replay reports both clusters below as unowned.

Cause 1 of 2: RunInstances with DryRun=true and an invalid iamInstanceProfile.name (fixture ec2-trajwire-deb-deb-instance-profile-named-by-role, step task#6).
AWS: recorded behavior only - Smithy (services/service-com-amazonaws-ec2-amazonec2) models no errors for RunInstances (errors: []), so the contract is the recording's. AWS validates the launch parameters BEFORE honouring DryRun and answers HTTP 400 with Code 'InvalidParameterValue' and Message "Value (<iamInstanceProfile.name value from the request>) for parameter iamInstanceProfile.name is invalid. Invalid IAM Instance Profile name". Fixed text: "Value (" ... ") for parameter iamInstanceProfile.name is invalid. Invalid IAM Instance Profile name"; the only variable part is the profile name copied from the request (the recording's 'sim2real-probe-deb-ip-1a34b480-PowerUserRole' is one run's value, not part of the template).
Emulator: answers HTTP 412 with Code 'DryRunOperation' and Message 'Request would have succeeded, but DryRun flag is set.' - it reports that the request WOULD have succeeded.
Why: a client using DryRun as a pre-flight check is told its launch request is valid when AWS would reject it, so the real launch fails later with an error the pre-flight promised would not occur. Validation-feedback loops (console wizards, Terraform plan-style flows) silently accept bad instance-profile names.
Fix: the RunInstances handler's request-validation ordering - parameter validation for iamInstanceProfile must run before the DryRun short-circuit.
Traces: ec2-trajwire-deb-deb-instance-profile-named-by-role#task#6 (cluster also covers ec2-trajwire-deb-deb-instance-type-casing-and-absent-ami#task#4, ec2-trajwire-deb-deb-key-pair-named-before-created#task#3, ec2-trajwire-deb-deb-security-group-name-passed-with-subnet#task#4 and #task#5).

Cause 2 of 2: DescribeInstances with DryRun=true and an unrecognised filter name (fixture ec2-trajwire-lia-dry-run-permission-probe, step task#3).
AWS: recorded behavior only (DescribeInstances models no errors either). AWS honours DryRun for DescribeInstances BEFORE validating the filter set and answers HTTP 412 with Code 'DryRunOperation' and the fixed Message 'Request would have succeeded, but DryRun flag is set.' (no variable parts).
Emulator: answers HTTP 400 with Code 'InvalidParameterValue' and Message "The filter '<filter name>' is invalid" - it validates filters first.
Why: the inverse failure of cause 1. A caller probing its own permissions with DryRun gets a 400 parameter error instead of the 412 that signals 'authorized', so permission-probe logic reads the call as malformed and cannot distinguish an authorization answer from a bad request.
Fix: the DescribeInstances handler's request-validation ordering - the DryRun short-circuit must precede filter-name validation.
Traces: ec2-trajwire-lia-dry-run-permission-probe#task#3.

## Evidence
Cause 1 - fixture ec2-trajwire-deb-deb-instance-profile-named-by-role step task#6, RunInstances: expected status 400, actual 412; $.Response.Errors.Error.Code expected 'InvalidParameterValue', got 'DryRunOperation'; $.Response.Errors.Error.Message expected 'Value (<profile-name>) for parameter iamInstanceProfile.name is invalid. Invalid IAM Instance Profile name', got 'Request would have succeeded, but DryRun flag is set.' (cluster affected_count 5).
Cause 2 - fixture ec2-trajwire-lia-dry-run-permission-probe step task#3, DescribeInstances: expected status 412, actual 400; Code expected 'DryRunOperation', got 'InvalidParameterValue'; Message expected 'Request would have succeeded, but DryRun flag is set.', got "The filter 'bogus-filter' is invalid".

## Steps to Reproduce
1. Replay the wire corpus against the emulator (replay_wire_corpus).
2. Cause 1: replay fixture ec2-trajwire-deb-deb-instance-profile-named-by-role step task#6 - RunInstances with DryRun=true and IamInstanceProfile.Name set to a name that is not an instance profile. Expect 400 InvalidParameterValue naming parameter iamInstanceProfile.name; the emulator answers 412 DryRunOperation.
3. Cause 2: replay fixture ec2-trajwire-lia-dry-run-permission-probe step task#3 - DescribeInstances with DryRun=true and Filter.1.Name set to an unknown filter name. Expect 412 DryRunOperation; the emulator answers 400 InvalidParameterValue about the filter.
