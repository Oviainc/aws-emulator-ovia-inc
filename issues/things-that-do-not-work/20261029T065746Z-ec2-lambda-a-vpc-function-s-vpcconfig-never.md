Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__lambda.xsvc-ec2-3-lambda-vpc-eni-dependency-violation-rejected ec2__lambda.xsvc-ec2-3-lambda-vpc-role-without-create-network-interface-rejected
Issue-Key: ec2__lambda.xsvc-ec2-3-lambda-vpc-eni-dependency-violation-rejected|run#24|AssertionError|assert vc.get("VpcId") == vpc, f"VpcConfig.VpcId is {vc.get('VpcId')!r}, expected {vpc!r}"
Issue-Key: ec2__lambda.xsvc-ec2-3-lambda-vpc-role-without-create-network-interface-rejected|_create_error#17|AssertionError|raise AssertionError(f"CreateFunction expected to be refused succeeded: {resp!r}")
Issue-Commit: c17c6e54c038586d9d6328bb68b9d1b3d6bcf360

# ec2__lambda: a VPC function's VpcConfig never reports VpcId, and CreateFunction is accepted when the execution role lacks ec2:CreateNetworkInterface (2 causes)

> _Note: this issue was identified against the code base at commit `c17c6e54c038586d9d6328bb68b9d1b3d6bcf360`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFunction

## Description
Two independent net-new root causes in Lambda's VPC-attachment surface. BOTH must be closed before submitting. The emulator answers in both cases (no scaffold stub involved). Neither check is listed by any open issue: 20261025T022656Z-lambda-ten-control-plane-writes-are-accepted-that.md covers neighbouring CreateFunction validation gaps and 20261027T070029Z-lambda-six-net-new-root-causes-layer-region.md covers layer-side gaps, but neither names these two checks or either cause below.

Cause 1 of 2: the VpcConfig a function reports omits VpcId.

AWS: CreateFunction and GetFunctionConfiguration both answer with the FunctionConfiguration shape, whose `VpcConfig` member targets `VpcConfigResponse`. Smithy (services/service-com-amazonaws-lambda-awsgirapiservice/smithy-model/com-amazonaws-lambda-awsgirapiservice.json): `VpcConfigResponse` = {SubnetIds (list, @length 0..16), SecurityGroupIds (list, @length 0..5), VpcId — documented "The ID of the VPC" —, Ipv6AllowedForDualStack}. The request shape `VpcConfig` has no VpcId member, so VpcId is response-only: Lambda derives it from the subnets it was given and reports it back on every read of the configuration.

Emulator: the response's VpcConfig carries only SubnetIds and SecurityGroupIds; VpcId is absent (the SDK surfaces it as None) on CreateFunction's 201 body and on every subsequent GetFunctionConfiguration, even once the function reaches State Active.

Why: a client that reads VpcConfig.VpcId to learn which VPC a function is attached to — to locate its service-managed ENIs, to check that a function and a database are in the same VPC, or to drive IaC drift detection — gets nothing back, and cannot distinguish "not attached to a VPC" from "attached but the field was dropped". The member is modeled and populated by AWS, so clients do rely on it.

Fix: the lambda service's CreateFunction / GetFunctionConfiguration / UpdateFunctionConfiguration response projection and the function state behind it — resolve the VPC from the configured SubnetIds at attach time, store it with the function, and serialize it into VpcConfigResponse.VpcId.

Traces: ec2__lambda.xsvc-ec2-3-lambda-vpc-eni-dependency-violation-rejected (mismatch, at `assert vc.get("VpcId") == vpc`).

Cause 2 of 2: CreateFunction with a VpcConfig is accepted although the execution role has no ec2:CreateNetworkInterface permission.

AWS: Lambda evaluates the execution role's EC2 permissions at create time and refuses the call with `InvalidParameterValueException`, HTTP 400 (Smithy: CreateFunction declares InvalidParameterValueException, errorKind client, httpError 400). The message is fixed text with no request-derived parts: `The provided execution role does not have permissions to call CreateNetworkInterface on EC2`. The refusal is complete: no function is created — a following GetFunction answers `ResourceNotFoundException` with the message `Function not found: <function ARN>`, whose only variable part is the ARN built from the request's region, account and function name — and no ENI carrying the requested security group exists. Attaching AWSLambdaVPCAccessExecutionRole to the same role and retrying then succeeds.

Emulator: the call succeeds — HTTP 201 with the full FunctionConfiguration (State `Pending`, StateReasonCode `Creating`, VpcConfig echoing the SubnetIds and SecurityGroupIds), so a function that AWS would never have created exists.

Why: the single most common Lambda-in-a-VPC misconfiguration is silently accepted. A client is told its function was created, learns nothing at the only point where it could fix the role, and the modeled 400/InvalidParameterValueException it branches on never arrives; the documented repair-and-retry path is unobservable, and the emulator holds a function that could never obtain an ENI.

Fix: execution-role permission pre-flight in the lambda service's CreateFunction handler (its validation step), ahead of any state write, together with the same check in UpdateFunctionConfiguration when a VpcConfig is added.

Traces: ec2__lambda.xsvc-ec2-3-lambda-vpc-role-without-create-network-interface-rejected (mismatch, at `raise AssertionError("CreateFunction expected to be refused succeeded: ...")`).

## Evidence
Cause 1 — Expected (AWS): GetFunctionConfiguration on an Active VPC function returns VpcConfig {"SubnetIds": [<subnet>], "SecurityGroupIds": [<sg>], "VpcId": <vpc of those subnets>}. Actual (emulator): VpcConfig {"SubnetIds": ["subnet-..."], "SecurityGroupIds": ["sg-..."]} — VpcId missing. Replay detail: `AssertionError: VpcConfig.VpcId is None, expected 'vpc-00000000000000022'` (the vpc id itself is a per-run value; the requirement is that the member is present and names the subnets' VPC).

Cause 2 — Expected (AWS): CreateFunction(VpcConfig={SubnetIds:[<subnet>], SecurityGroupIds:[<sg>]}) with a role carrying only AWSLambdaBasicExecutionRole -> HTTP 400, Code `InvalidParameterValueException`, Message exactly `The provided execution role does not have permissions to call CreateNetworkInterface on EC2` (all fixed text); then GetFunction -> HTTP 404 `ResourceNotFoundException`, `Function not found: arn:aws:lambda:<region>:<account>:function:<name>`; and no ENI carries <sg>. Actual (emulator): HTTP 201 and the function is created — replay detail `AssertionError: CreateFunction expected to be refused succeeded: {'ResponseMetadata': {'HTTPStatusCode': 201, ...}, 'FunctionName': ..., 'State': 'Pending', 'StateReason': 'The function is being created.', 'StateReasonCode': 'Creating', 'VpcConfig': {'SubnetIds': [...], 'SecurityGroupIds': [...]}}` (the names, ARNs, hashes and timestamps in that body are per-run values; the divergence is the 201-with-function instead of the 400 refusal).

## Steps to Reproduce
Cause 1: create a VPC, a subnet and a security group in it; create an IAM role trusted by lambda.amazonaws.com with arn:aws:iam::aws:policy/service-role/AWSLambdaVPCAccessExecutionRole attached; lambda:CreateFunction (python3.12, zip code) with VpcConfig={SubnetIds:[<subnet>], SecurityGroupIds:[<sg>]}; poll lambda:GetFunctionConfiguration until State=Active and read VpcConfig. Expected VpcConfig.VpcId == the subnet's VpcId; actual: the key is absent.

Cause 2: same VPC/subnet/security group; create an IAM role trusted by lambda.amazonaws.com with ONLY arn:aws:iam::aws:policy/service-role/AWSLambdaBasicExecutionRole attached (no ec2:CreateNetworkInterface); lambda:CreateFunction with that role and VpcConfig={SubnetIds:[<subnet>], SecurityGroupIds:[<sg>]}. Expected: HTTP 400 InvalidParameterValueException `The provided execution role does not have permissions to call CreateNetworkInterface on EC2`, then lambda:GetFunction -> ResourceNotFoundException `Function not found: <arn>` and ec2:DescribeNetworkInterfaces filtered on group-id:<sg> empty. Actual: HTTP 201 and the function exists. Then attach AWSLambdaVPCAccessExecutionRole and retry — that call must succeed and reach Active.

Re-replay both check ids in trace_ids to confirm the fix.
