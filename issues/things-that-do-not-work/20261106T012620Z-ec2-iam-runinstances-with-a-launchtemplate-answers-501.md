Issue-Severity: MEDIUM
Issue-Commit: ed195bdda9ffdbdf051f799fc5087e4de9084b05

# ec2__iam: RunInstances with a LaunchTemplate answers 501 NotImplemented, so an unresolvable iamInstanceProfile.name is never refused with AWS's InvalidParameterValue

> _Note: this issue was identified against the code base at commit `ed195bdda9ffdbdf051f799fc5087e4de9084b05`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunInstances

## Description
Cause 1 of 1: RunInstances is implemented for a plain launch, but the LaunchTemplate-driven path is still a scaffold stub, so the request is answered NotImplemented before any parameter is validated and the refusal AWS serves for an instance profile that does not exist can never be observed.

AWS: the on-disk Smithy model (services/service-com-amazonaws-ec2-amazonec2) declares RunInstances with the member LaunchTemplate -> LaunchTemplateSpecification and IamInstanceProfile -> IamInstanceProfileSpecification (ec2QueryName IamInstanceProfile) and models NO operation errors — EC2's ec2Query refusals are not in the model — so the error code and message are recorded behavior only. When the effective launch specification (whether supplied directly or resolved from the launch template) names an IAM instance profile by Name that does not resolve in the account, AWS refuses the call with HTTP 400 and error code InvalidParameterValue and the message, as a template: "Value (<profile-name>) for parameter iamInstanceProfile.name is invalid. Invalid IAM Instance Profile name". Fixed text, exactly as AWS words it: "Value (", ") for parameter iamInstanceProfile.name is invalid. Invalid IAM Instance Profile name"; the single variable part is the profile NAME taken verbatim from the request (here, from the launch template version's IamInstanceProfile.Name). No instance and no reservation is created.
Emulator: the call is refused with code NotImplemented and the message "operation RunInstances with a LaunchTemplate is scaffolded but not implemented in this emulator" — an emulator-authored refusal that names no parameter, no value and no AWS error class, and that fires for every LaunchTemplate launch regardless of whether the request is valid.
Why: a NotImplemented is not an AWS error class. Client code that distinguishes a bad-input refusal (fix the profile name, do not retry) from a service fault retries or aborts on the wrong branch; negative tests asserting the InvalidParameterValue contract cannot run; and every downstream scenario that launches from a launch template — Auto Scaling groups, instance-profile resolution through a template, SSM/SSH bring-up — is blocked behind the same stub rather than exercising validation.
Fix: the RunInstances handler and its validation in the ec2 service crate — the LaunchTemplate path must resolve the named template version into an effective launch specification and then run the same parameter validation as a direct launch, refusing an unresolvable IamInstanceProfile.Name with InvalidParameterValue and AWS's iamInstanceProfile.name message before any instance is minted.
Nearest open issue, named so it is not mistaken for coverage of this replay key: 20261022T235108Z-ec2-sql-ha-application-status-check-fast-launch.md records that several EC2 paths including RunInstances-with-LaunchTemplate answer 501 NotImplemented, but it does not carry this check id nor the InvalidParameterValue iamInstanceProfile.name refusal as a graded requirement; close the two together.
Traces: ec2__iam.asgssh-run-instances-with-unresolvable-profile-rejected.

## Evidence
Suite simparity-behavioral-aws-v1 replayed against this checkout: 451/793 passed; service heading `### ec2__iam` 1/7. The check is marked neither ALREADY FILED nor OWNED in the report.

[mismatch] ec2__iam.asgssh-run-instances-with-unresolvable-profile-rejected:
  AssertionError: launching with the missing profile 'sim2real-asgssh-noprofile-1eda003d0ea2' must be refused with the exact error AWS documents for an unresolvable profile name; got ('NotImplemented', 'operation RunInstances with a LaunchTemplate is scaffolded but not implemented in this emulator'), wanted ('InvalidParameterValue', 'Value (sim2real-asgssh-noprofile-1eda003d0ea2) for parameter iamInstanceProfile.name is invalid. Invalid IAM Instance Profile name')

Expected: HTTP 400, error code InvalidParameterValue, message "Value (<profile-name>) for parameter iamInstanceProfile.name is invalid. Invalid IAM Instance Profile name" with <profile-name> echoed from the request. Actual: error code NotImplemented with emulator-authored text naming the operation and the emulator. The profile name shown is this run's generated value filling the template, not part of the requirement.

## Steps to Reproduce
1. Boot the emulator from this checkout; point ec2 and iam clients at it.
2. CreateLaunchTemplate with a LaunchTemplateData whose IamInstanceProfile is {"Name": "<a profile name that was never created>"} (plus an ImageId and InstanceType).
3. Call RunInstances(MinCount=1, MaxCount=1, LaunchTemplate={LaunchTemplateName: <that template>, Version: '1'}).
4. Observed: a refusal with code NotImplemented and message 'operation RunInstances with a LaunchTemplate is scaffolded but not implemented in this emulator'. Expected: HTTP 400 InvalidParameterValue with 'Value (<profile-name>) for parameter iamInstanceProfile.name is invalid. Invalid IAM Instance Profile name', and no instance or reservation created.
5. Control: create the instance profile, then repeat step 3 — the launch must succeed and the instance must carry the resolved profile ARN.
