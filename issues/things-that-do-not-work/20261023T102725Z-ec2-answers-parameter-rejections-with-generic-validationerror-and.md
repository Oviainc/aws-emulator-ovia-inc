Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-account-and-instance-type-metadata
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-account-and-instance-type-metadata ec2-capacity-manager-data-export-setup ec2-capacity-reservation-family-error-semantics ec2-client-vpn-endpoint-invalid-cidr-size ec2-instance-export-task-error ec2-instance-reboot-and-status-report ec2-on-demand-capacity-reservation-visibility ec2-page-describe-security-groups-vpc-scoped-trailing-empty-page ec2-page-describe-subnets-vpc-scoped-two-pages
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeInstanceTypes|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|DescribeInstanceTypes|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|DescribeInstanceTypes|4xx->4xx|$.Response.RequestID
Issue-Key: ec2|DescribeInstanceTypes|4xx->4xx|$headers.content-type
Issue-Key: ec2|CreateCapacityManagerDataExport|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|CreateCapacityManagerDataExport|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|CreateCapacityManagerDataExport|4xx->4xx|$.Response.RequestID
Issue-Key: ec2|CreateCapacityManagerDataExport|4xx->4xx|$headers.content-type
Issue-Key: ec2|CreateCapacityReservation|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|CreateCapacityReservation|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|CreateCapacityReservation|4xx->4xx|$.Response.RequestID
Issue-Key: ec2|CreateCapacityReservation|4xx->4xx|$headers.content-type
Issue-Key: ec2|CreateClientVpnEndpoint|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|CreateClientVpnEndpoint|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|CreateClientVpnEndpoint|4xx->4xx|$.Response.RequestID
Issue-Key: ec2|CreateClientVpnEndpoint|4xx->4xx|$headers.content-type
Issue-Key: ec2|CreateInstanceExportTask|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|CreateInstanceExportTask|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|CreateInstanceExportTask|4xx->4xx|$.Response.RequestID
Issue-Key: ec2|CreateInstanceExportTask|4xx->4xx|$headers.content-type
Issue-Key: ec2|ReportInstanceStatus|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|ReportInstanceStatus|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|ReportInstanceStatus|4xx->4xx|$.Response.RequestID
Issue-Key: ec2|ReportInstanceStatus|4xx->4xx|$headers.content-type
Issue-Key: ec2|DescribeCapacityReservationBillingRequests|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|DescribeCapacityReservationBillingRequests|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|DescribeCapacityReservationBillingRequests|4xx->4xx|$.Response.RequestID
Issue-Key: ec2|DescribeCapacityReservationBillingRequests|4xx->4xx|$headers.content-type
Issue-Key: ec2|DescribeSecurityGroups|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|DescribeSecurityGroups|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|DescribeSecurityGroups|4xx->4xx|$.Response.RequestID
Issue-Key: ec2|DescribeSecurityGroups|4xx->4xx|$headers.content-type
Issue-Key: ec2|DescribeSubnets|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|DescribeSubnets|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|DescribeSubnets|4xx->4xx|$.Response.RequestID
Issue-Key: ec2|DescribeSubnets|4xx->4xx|$headers.content-type
Issue-Commit: 91a1d12adf13e91eb189076c1b2883c7fc3c7ce7

# EC2 answers parameter rejections with generic ValidationError and MalformedInput codes

> _Note: this issue was identified against the code base at commit `91a1d12adf13e91eb189076c1b2883c7fc3c7ce7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstanceTypes

## Description
Nine operations reject a bad parameter with the right HTTP 400 but a generic model-constraint error Code ('ValidationError' or 'MalformedInput') instead of the EC2 code AWS uses. All nine must be closed before submitting. Background: EC2 models no error shapes (Smithy com-amazonaws-ec2-amazonec2: every operation queried reports errors: []), so the ec2Query Code is the entire contract a client can branch on, and 'ValidationError'/'MalformedInput' are not part of EC2's vocabulary - no client error handler matches them. The recorded messages below are examples; the requirement is the Code. Where the emulator's message is generated from the request (it names the member and the constraint it violated) that generated form is itself wrong for EC2, which reports the offending value in its own prose.

Cause 1 of 9: DescribeInstanceTypes with an unknown instance type.
AWS: Code InvalidInstanceType (message example 'The following supplied instance types do not exist: [t9.enormous]').
Emulator: Code ValidationError with a '1 validation error detected: ... Member must satisfy enum value set: [a1.medium, ...]' message enumerating the whole InstanceType enum.
Why: a client that catalogues capability by probing instance types cannot distinguish 'no such type' from a malformed request.
Fix: instance-type validation in the DescribeInstanceTypes handler, ahead of the generic enum check.
Traces: ec2-account-and-instance-type-metadata#task#7.

Cause 2 of 9: CreateCapacityManagerDataExport with an unsupported OutputFormat.
AWS: Code InvalidParameterValue (message example "The value 'xml' for OutputFormat isn't valid. Valid values: CSV or Parquet.").
Emulator: Code ValidationError on 'outputFormat'.
Fix: OutputFormat validation in the CreateCapacityManagerDataExport handler.
Traces: ec2-capacity-manager-data-export-setup#task#1.

Cause 3 of 9: CreateCapacityReservation with an unknown InstancePlatform.
AWS: Code InvalidParameterValue (message example 'Invalid instance platform: Not A Real Platform , accountId: 000000000000').
Emulator: Code ValidationError on 'instancePlatform'.
Fix: InstancePlatform validation in the CreateCapacityReservation handler.
Traces: ec2-capacity-reservation-family-error-semantics#task#1.

Cause 4 of 9: CreateClientVpnEndpoint with AuthenticationOptions omitted.
AWS: Code MissingParameter (message example 'The request must include the AuthenticationOptions parameter. Add the required parameter and retry the request.').
Emulator: Code MalformedInput with "Value null at 'authenticationOptions' failed to satisfy constraint: Member must not be null".
Why: a client cannot tell 'you forgot a parameter' from 'your request body is corrupt' - the first is fixable by the caller, the second is not.
Fix: required-member validation in the CreateClientVpnEndpoint handler.
Traces: ec2-client-vpn-endpoint-invalid-cidr-size#task#2.

Cause 5 of 9: CreateInstanceExportTask with a nonexistent instance id.
AWS: Code InvalidParameter (message example 'The instance ID provided (i-0000000000deadbee) does not exist. Please make sure the ID is correct.').
Emulator: Code ValidationError - and it rejects a DIFFERENT member, complaining that 'vmdk' at 'exportToS3Task.diskImageFormat' is not in the enum [VMDK, RAW, VHD]; the enum check is case-sensitive where AWS accepts the lowercase form, so the instance id is never reached.
Fix: case handling on DiskImageFormat plus instance-existence validation ordering in the CreateInstanceExportTask handler.
Traces: ec2-instance-export-task-error#task#1.

Cause 6 of 9: ReportInstanceStatus with an unknown reason code.
AWS: Code InvalidParameterValue (message example 'invalid reason code not-a-real-reason-code').
Emulator: Code ValidationError on 'reasonCodes.1.member'.
Fix: reason-code validation in the ReportInstanceStatus handler.
Traces: ec2-instance-reboot-and-status-report#task#8.

Cause 7 of 9: DescribeCapacityReservationBillingRequests with an unknown Role.
AWS: Code InvalidParameterValue (message example 'invalid-role-value is not a valid value for RoleType. Specify either odcr-owner or unused-reservation-billing-owner.').
Emulator: Code ValidationError on 'role'.
Fix: Role validation in the DescribeCapacityReservationBillingRequests handler.
Traces: ec2-on-demand-capacity-reservation-visibility#task#7.

Cause 8 of 9: DescribeSecurityGroups with MaxResults above the ceiling.
AWS: Code InvalidParameterValue (message example 'Value ( 1001 ) for parameter maxResults is invalid. Expecting a value smaller than 1000.').
Emulator: Code ValidationError with 'Member must have value less than or equal to 1000'.
Fix: MaxResults range validation in the DescribeSecurityGroups handler.
Traces: ec2-page-describe-security-groups-vpc-scoped-trailing-empty-page#task#11.

Cause 9 of 9: DescribeSubnets with MaxResults above the ceiling.
AWS: Code InvalidParameterValue (message example 'Value ( 1001 ) for parameter MaxResults is invalid. Expecting a value smaller than or equal to 1000.').
Emulator: Code ValidationError, as cause 8.
Fix: MaxResults range validation in the DescribeSubnets handler.
Traces: ec2-page-describe-subnets-vpc-scoped-two-pages#task#12.

## Evidence
All nine clusters are status 400 -> 400 with '$.Response.Errors.Error.Code: value_mismatch'. Cause 1 expected 'InvalidInstanceType', got 'ValidationError'. Cause 2 expected 'InvalidParameterValue', got 'ValidationError'. Cause 3 expected 'InvalidParameterValue', got 'ValidationError'. Cause 4 expected 'MissingParameter', got 'MalformedInput'. Cause 5 expected 'InvalidParameter', got 'ValidationError' (and the message rejects exportToS3Task.diskImageFormat='vmdk' rather than the instance id). Cause 6 expected 'InvalidParameterValue', got 'ValidationError'. Cause 7 expected 'InvalidParameterValue', got 'ValidationError'. Causes 8 and 9 expected 'InvalidParameterValue', got 'ValidationError'. Each cluster also shows the missing Response.RequestID and the text/xml content-type, filed separately.

## Steps to Reproduce
Replay the fixture steps named under Traces. Minimal repros: (1) ec2:DescribeInstanceTypes InstanceTypes=['t9.enormous']. (2) ec2:CreateCapacityManagerDataExport OutputFormat='xml'. (3) ec2:CreateCapacityReservation InstancePlatform='Not A Real Platform'. (4) ec2:CreateClientVpnEndpoint with AuthenticationOptions omitted. (5) ec2:CreateInstanceExportTask InstanceId='i-0000000000deadbee' with ExportToS3Task.DiskImageFormat='vmdk'. (6) ec2:ReportInstanceStatus ReasonCodes=['not-a-real-reason-code']. (7) ec2:DescribeCapacityReservationBillingRequests Role='invalid-role-value'. (8) ec2:DescribeSecurityGroups MaxResults=1001. (9) ec2:DescribeSubnets MaxResults=1001.
