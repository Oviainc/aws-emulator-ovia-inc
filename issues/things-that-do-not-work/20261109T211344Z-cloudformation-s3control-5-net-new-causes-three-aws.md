Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__s3control.deploy-s3-access-grants-instance-location-grant-lifecycle cloudformation__s3control.deploy-s3-multi-region-access-point-and-policy-lifecycle cloudformation__s3control.deploy-s3-storage-lens-and-group-lifecycle cloudformation__s3control.deploy-s3-access-point-lifecycle cloudformation__s3control.deploy-s3-multi-region-access-point-missing-bucket-rejected
Issue-Key: cloudformation__s3control|CreateStack|ValidationError
Issue-Key: cloudformation__s3control.deploy-s3-access-point-lifecycle|stack_outputs#11|KeyError|return {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Key: cloudformation__s3control.deploy-s3-multi-region-access-point-missing-bucket-rejected|run#10|AssertionError|assert status == "ROLLBACK_COMPLETE", f"rejected stack ended {status!r}"
Issue-Commit: 5469355dc4ba582ea958345f64905a5aea7e37f0

# cloudformation__s3control: 5 net-new causes — three AWS::S3::* S3-Control resource families unregistered, stack Outputs rows carry no OutputValue, and an MRAP over a non-existent bucket reaches CREATE_COMPLETE

> _Note: this issue was identified against the code base at commit `5469355dc4ba582ea958345f64905a5aea7e37f0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Five distinct root causes in the CloudFormation-to-S3-Control deploy path. All five must be closed before submitting.

Cause 1 of 5: AWS::S3::AccessGrantsInstance, AWS::S3::AccessGrantsLocation and AWS::S3::AccessGrant are not registered resource types.
AWS: CloudFormation supports these three types; CreateStack on a template declaring an S3 Access Grants instance, a location and a grant is accepted (200 with a StackId) and provisions them in S3 Control (modelled target surface, e.g. CreateAccessGrantsInstance POST /v20180820/accessgrantsinstance -> 200 with AccessGrantsInstanceId/AccessGrantsInstanceArn/CreatedAt). ValidationError "Template format error: Unrecognized resource types: [...]" is reserved for genuinely unsupported types; its fixed text is "Template format error: Unrecognized resource types: " and the bracketed list is taken from the request's template (registry support itself is recorded behavior only — not modelled in Smithy).
Emulator: CreateStack answers HTTP 400 ValidationError "Template format error: Unrecognized resource types: [AWS::S3::AccessGrantsInstance, AWS::S3::AccessGrantsLocation, AWS::S3::AccessGrant]".
Why: an Access Grants stack can never be deployed, and the user is told a correct template is malformed.
Fix: the CloudFormation resource-type registry plus provisioning handlers for the three Access Grants types (create/update/delete against S3 Control, Ref and GetAtt resolution).
Traces: cloudformation__s3control.deploy-s3-access-grants-instance-location-grant-lifecycle.

Cause 2 of 5: AWS::S3::MultiRegionAccessPointPolicy is not a registered resource type.
AWS: CloudFormation supports AWS::S3::MultiRegionAccessPointPolicy (properties MrapName, Policy) alongside AWS::S3::MultiRegionAccessPoint; a template declaring both is accepted and the policy is established at S3 Control (readable via GetMultiRegionAccessPointPolicy -> Policy.Established). Recorded behavior only for the registry entry.
Emulator: CreateStack answers HTTP 400 ValidationError "Template format error: Unrecognized resource types: [AWS::S3::MultiRegionAccessPointPolicy]" — note AWS::S3::MultiRegionAccessPoint itself is accepted, so only the policy type is missing.
Why: no stack can attach a policy to an MRAP, so the whole MRAP-plus-policy deployment pattern is unusable even though the MRAP type works.
Fix: register AWS::S3::MultiRegionAccessPointPolicy and give it a handler that puts the policy document at S3 Control and resolves Ref to the MRAP name.
Traces: cloudformation__s3control.deploy-s3-multi-region-access-point-and-policy-lifecycle.

Cause 3 of 5: AWS::S3::StorageLensGroup is not a registered resource type.
AWS: CloudFormation supports AWS::S3::StorageLensGroup; the template is accepted and the group is created in S3 Control (modelled: CreateStorageLensGroup POST /v20180820/storagelensgroup -> 204). Recorded behavior only for the registry entry.
Emulator: CreateStack answers HTTP 400 ValidationError "Template format error: Unrecognized resource types: [AWS::S3::StorageLensGroup]".
Why: a Storage Lens configuration-plus-group stack cannot be deployed at all.
Fix: register AWS::S3::StorageLensGroup and back it with a handler calling the S3 Control Storage Lens Group surface.
Traces: cloudformation__s3control.deploy-s3-storage-lens-and-group-lifecycle.

Cause 4 of 5: DescribeStacks returns Outputs rows with an OutputKey but no OutputValue.
AWS: each member of DescribeStacks Stacks[].Outputs carries OutputKey and OutputValue; a template Output whose Value is a Ref or Fn::GetAtt is resolved before the stack reaches CREATE_COMPLETE, so a CREATE_COMPLETE stack never exposes a valueless Output. For AWS::S3::AccessPoint, Ref resolves to the access point name and GetAtt Arn/Alias/NetworkOrigin resolve to the same values S3 Control's own GetAccessPoint returns (modelled: GetAccessPoint output members Name, AccessPointArn, Alias, NetworkOrigin).
Emulator: the stack reaches CREATE_COMPLETE, but reading its Outputs raises KeyError: 'OutputValue' — the rows are emitted with the key only, so Ref/GetAtt on the access point are never resolved into the output.
Why: every client that reads a stack output (CLI `describe-stacks --query Outputs`, CDK, any pipeline wiring one stack's output into the next) gets a row it cannot use; cross-stack and intra-stack value passing silently breaks on a stack reported as successfully created.
Fix: the CreateStack/DescribeStacks output-resolution and serialization path — resolve each Output's Value (Ref, Fn::GetAtt, Fn::Sub) during provisioning and always serialize OutputValue; and the AWS::S3::AccessPoint handler's Ref/GetAtt attribute resolution.
Traces: cloudformation__s3control.deploy-s3-access-point-lifecycle. The same no-OutputValue symptom is filed for other services (20261109T154607Z-cloudformation-signer-..., 20261109T172356Z-autoscaling-cloudformation-..., 20261109T183444Z-cloudformation-synthetics-...); none covers cloudformation__s3control.

Cause 5 of 5: an AWS::S3::MultiRegionAccessPoint whose Regions entry names a bucket that does not exist reaches CREATE_COMPLETE instead of failing the resource and rolling the stack back.
AWS: the MRAP resource handler surfaces S3 Control's refusal for a Regions entry naming an absent bucket; the resource goes CREATE_FAILED and the stack settles ROLLBACK_COMPLETE, with the failed resource's ResourceStatusReason carrying the handler reason whose error code is "InvalidRequest" and whose message is "Invalid request provided: The specified bucket does not exist." (fixed text — it names no bucket, so there is no variable part). After rollback S3 Control holds no such access point: GetMultiRegionAccessPoint answers NoSuchMultiRegionAccessPoint / "The specified Multi-Region Access Point does not exist."
Emulator: the stack settles CREATE_COMPLETE — the bad Regions entry is accepted, no resource fails, no rollback happens and no CREATE_FAILED reason is recorded.
Why: an IaC deploy that should have been rejected reports success, leaving the account with a stack that claims an access point over a bucket that does not exist; the operator gets no signal and later data-path calls fail unexplained.
Fix: validation in the AWS::S3::MultiRegionAccessPoint resource handler (resolve each Regions[].Bucket against S3 before creating), plus the resource-failure/rollback path that records the handler's reason on the CREATE_FAILED event. Related but not covering: open issue 20261029T192817Z-s3control-multi-region-access-point-create-validation-and (the same validation gap on the direct S3 Control API, not via CloudFormation).
Traces: cloudformation__s3control.deploy-s3-multi-region-access-point-missing-bucket-rejected.

## Evidence
Cause 1 — Expected: CreateStack returns a StackId and the stack provisions the Access Grants instance/location/grant. Actual: ClientError (ValidationError) ... Template format error: Unrecognized resource types: [AWS::S3::AccessGrantsInstance, AWS::S3::AccessGrantsLocation, AWS::S3::AccessGrant]
Cause 2 — Expected: CreateStack accepted; MRAP policy established. Actual: ClientError (ValidationError) ... Template format error: Unrecognized resource types: [AWS::S3::MultiRegionAccessPointPolicy]
Cause 3 — Expected: CreateStack accepted; Storage Lens group created. Actual: ClientError (ValidationError) ... Template format error: Unrecognized resource types: [AWS::S3::StorageLensGroup]
Cause 4 — Expected: DescribeStacks Stacks[0].Outputs == [{OutputKey: 'APRef', OutputValue: <access point name>}, {OutputKey:'APArn', OutputValue: arn:<partition>:s3:<region>:<account>:accesspoint/<name>}, {OutputKey:'APAlias', OutputValue: <alias>}, {OutputKey:'APNetworkOrigin', OutputValue: 'Internet'}]. Actual: KeyError: 'OutputValue' in `{o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}` — rows carry OutputKey only.
Cause 5 — Expected: wait_stack(...) == 'ROLLBACK_COMPLETE' with the MRAP resource CREATE_FAILED and handler reason code 'InvalidRequest', message 'Invalid request provided: The specified bucket does not exist.'; then GetMultiRegionAccessPoint answers NoSuchMultiRegionAccessPoint / 'The specified Multi-Region Access Point does not exist.'. Actual: AssertionError: rejected stack ended 'CREATE_COMPLETE'.

## Steps to Reproduce
Boot the emulator and use a botocore cloudformation + s3control client against it.
Cause 1: CreateStack with a template declaring AWS::S3::AccessGrantsInstance, AWS::S3::AccessGrantsLocation and AWS::S3::AccessGrant -> observe the ValidationError unrecognized-types refusal. Replay cloudformation__s3control.deploy-s3-access-grants-instance-location-grant-lifecycle.
Cause 2: CreateStack with AWS::S3::MultiRegionAccessPoint plus AWS::S3::MultiRegionAccessPointPolicy -> only the policy type is refused. Replay cloudformation__s3control.deploy-s3-multi-region-access-point-and-policy-lifecycle.
Cause 3: CreateStack with AWS::S3::StorageLensGroup -> refused. Replay cloudformation__s3control.deploy-s3-storage-lens-and-group-lifecycle.
Cause 4: CreateStack with an AWS::S3::Bucket plus an AWS::S3::AccessPoint over it and Outputs {Ref AP, GetAtt AP.Arn, GetAtt AP.Alias, GetAtt AP.NetworkOrigin}; wait for CREATE_COMPLETE, then DescribeStacks and read Stacks[0].Outputs -> rows have no OutputValue. Replay cloudformation__s3control.deploy-s3-access-point-lifecycle.
Cause 5: CreateStack with an AWS::S3::Bucket and an AWS::S3::MultiRegionAccessPoint whose Regions is [{Bucket: <a bucket name that exists nowhere>}]; wait for a terminal status -> CREATE_COMPLETE instead of ROLLBACK_COMPLETE. Replay cloudformation__s3control.deploy-s3-multi-region-access-point-missing-bucket-rejected.
