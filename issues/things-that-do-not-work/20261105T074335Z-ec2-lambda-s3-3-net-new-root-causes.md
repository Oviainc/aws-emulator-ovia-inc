Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__lambda__s3.delete-vpc-rejected-while-vpc-endpoint-exists ec2__lambda__s3.second-s3-gateway-endpoint-on-same-route-table-rejected ec2__lambda__s3.e2e-neptune-bridge-carve-lifecycle
Issue-Key: ec2__lambda__s3.delete-vpc-rejected-while-vpc-endpoint-exists|_err#9|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Key: ec2__lambda__s3.second-s3-gateway-endpoint-on-same-route-table-rejected|run#12|AssertionError|assert message == (
Issue-Key: ec2__lambda__s3.e2e-neptune-bridge-carve-lifecycle|_poll_until#7|Timeout|time.sleep(interval_s)
Issue-Commit: 6cc7c0f4d58adfa1ddf47e8354a4acf1282f7003

# ec2__lambda__s3: 3 net-new root causes — DeleteVpc ignores a live gateway VPC endpoint, RouteAlreadyExists is misworded, and the Neptune-bridge lifecycle never converges

> _Note: this issue was identified against the code base at commit `6cc7c0f4d58adfa1ddf47e8354a4acf1282f7003`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteVpc

## Description
Three distinct root causes, all reached through the ec2__lambda__s3 behavioural checks. None is marked ALREADY FILED or OWNED by the replay report.

Cause 1 of 3: DeleteVpc succeeds while a gateway VPC endpoint still targets the VPC's route table.
AWS: a VPC cannot be deleted while any dependency still exists; a gateway VPC endpoint attached to one of the VPC's route tables is such a dependency even when the VPC has no subnet. DeleteVpc then refuses with HTTP 400, EC2 query error code `DependencyViolation`, message `The vpc '<VpcId>' has dependencies and cannot be deleted.` — the fixed text is `The vpc '` … `' has dependencies and cannot be deleted.` and the only variable part is the VpcId taken verbatim from the request. Smithy: `DeleteVpc` input `DeleteVpcRequest` (VpcId required), output `Unit`, and the model declares NO errors (EC2's ec2Query error set is unmodeled), so the error code and sentence are recorded/documented behaviour, not modelled. Recorded behaviour confirms the refusal is followed by the VPC and the endpoint both still describing, and the identical DeleteVpc succeeding only after DeleteVpcEndpoints.
Emulator: DeleteVpc answers HTTP 200 with an empty DeleteVpcResponse body while the gateway endpoint is still live — the check reports `AssertionError: expected the call to be rejected, but it succeeded: {'ResponseMetadata': {'HTTPStatusCode': 200, ...}}`. The endpoint is modelled as detached from the VPC's deletability.
Why: a client that relies on AWS's dependency ordering (tear endpoints down, then the VPC) sees a success where AWS refuses; teardown code that treats the 200 as proof the VPC is gone then diverges from AWS, and a negative-path test of DependencyViolation cannot be written against the emulator at all.
Fix: the DeleteVpc handler's pre-delete dependency check (it must enumerate VPC endpoints whose VpcId is the target, alongside the subnet/ENI/gateway dependencies it already considers) and the DependencyViolation error construction in that handler.
Traces: ec2__lambda__s3.delete-vpc-rejected-while-vpc-endpoint-exists.

Cause 2 of 3: CreateVpcEndpoint's RouteAlreadyExists message does not name the conflicting route table.
AWS: a second Gateway endpoint for the same service on a route table that already carries that service's managed-prefix-list route is refused with HTTP 400, code `RouteAlreadyExists`, message `route table <RouteTableId> already has a route with destination-prefix-list-id <PrefixListId>`. Fixed text: `route table ` … ` already has a route with destination-prefix-list-id ` …; the variable parts are the route table id from the request's RouteTableIds and the managed prefix list id of the requested ServiceName (as returned by DescribePrefixLists). Smithy: `CreateVpcEndpoint` models no errors, so the code/sentence are recorded/documented behaviour. The conflict is over the prefix-list route, not over the endpoint object.
Emulator: the code is right but the message is the emulator's own prose and drops the route table entirely — `The route identified by pl-fffbb61c already exists.` (the prefix-list id in that run is a per-run value; the defect is that the route table id is absent and the sentence is reworded).
Why: a client or operator reading the refusal cannot tell WHICH route table is occupied, which is the whole actionable content of the message when several tables are named in one CreateVpcEndpoint call; message-matching clients break.
Fix: the CreateVpcEndpoint handler's duplicate-prefix-list-route validation, where the RouteAlreadyExists error message is formatted.
Traces: ec2__lambda__s3.second-s3-gateway-endpoint-on-same-route-table-rejected.

Cause 3 of 3: the Neptune-bridge carve lifecycle never converges and is cut off at the harness cap.
AWS: recorded behaviour only. The lifecycle deploys a CFN stack, creates a VPC-attached Lambda bridge, polls GetFunctionConfiguration until State=Active (the Hyperplane ENI is provisioned), polls DescribeNetworkInterfaces for the function's `lambda` interface-type ENI in the subnet, invokes the bridge, and then polls for the prefix-list route to be withdrawn with the endpoint and for the function/role to become not-found (`ResourceNotFoundException` / `NoSuchEntity`). Every one of those is an observable AWS convergence.
Emulator: the check ends with `Error: Check timed out after 60.0s` — no step attribution is produced, so one of the polled conditions never converges against the emulator. Hypothesis: the same VPC-attached-Lambda gap already filed as 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md and 20261104T044826Z-ec2-lambda-s3-createvpcendpoint-with-route-table-associations-is-refused-notimplemented.md blocks it before any later assertion; neither issue carries this check's replay key, so it is reported here rather than assumed covered.
Why: the whole carve/restore lifecycle is ungradable — a client cannot observe that a VPC-attached function becomes Active, that its ENI appears, or that the prefix-list route is withdrawn when the endpoint goes away.
Fix: whichever convergence the lifecycle stalls on — the Lambda function-state/ENI provisioning path and the VPC-endpoint route withdrawal in the EC2 state model; attribution should be re-established by replaying this check once causes 1 and 2 and the two named open issues are closed.
Traces: ec2__lambda__s3.e2e-neptune-bridge-carve-lifecycle.

## Evidence
Cause 1 — ec2__lambda__s3.delete-vpc-rejected-while-vpc-endpoint-exists
  Expected: DeleteVpc -> HTTP 400 DependencyViolation, "The vpc '<VpcId>' has dependencies and cannot be deleted."; VPC still available and endpoint still describable afterwards.
  Actual:   DeleteVpc -> HTTP 200, empty response. AssertionError: expected the call to be rejected, but it succeeded: {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'text/xml;charset=UTF-8', 'content-length': '167', ...}}}

Cause 2 — ec2__lambda__s3.second-s3-gateway-endpoint-on-same-route-table-rejected
  Expected: ('RouteAlreadyExists', 'route table <RouteTableId> already has a route with destination-prefix-list-id <PrefixListId>')
  Actual:   ('RouteAlreadyExists', 'The route identified by <PrefixListId> already exists.')
  AssertionError: RouteAlreadyExists must name the conflicting route table 'rtb-...' and prefix list 'pl-...', got message 'The route identified by pl-... already exists.'  (ids shown are per-run values)

Cause 3 — ec2__lambda__s3.e2e-neptune-bridge-carve-lifecycle
  Expected: each polled convergence is reached (Lambda State=Active with a `lambda` interface-type ENI in the subnet; prefix-list route withdrawn with the endpoint; function -> ResourceNotFoundException "Function not found: <arn>"; role -> NoSuchEntity "The role with name <name> cannot be found.").
  Actual:   Error: Check timed out after 60.0s — no condition converged within the budget and no step attribution was produced.

## Steps to Reproduce
Cause 1: CreateVpc (10.87.0.0/16); find its main route table; CreateVpcEndpoint VpcEndpointType=Gateway ServiceName=com.amazonaws.<region>.s3 RouteTableIds=[main]; DescribeVpcEndpoints confirms it; call DeleteVpc(VpcId=vpc). Expect 400 DependencyViolation "The vpc '<vpc>' has dependencies and cannot be deleted."; the emulator answers 200 and removes the VPC.

Cause 2: in a fresh VPC, CreateVpcEndpoint Gateway for com.amazonaws.<region>.s3 on the main route table (succeeds); repeat the identical CreateVpcEndpoint. Expect 400 RouteAlreadyExists with "route table <rtb-id> already has a route with destination-prefix-list-id <pl-id>"; the emulator returns the right code with the message "The route identified by <pl-id> already exists."

Cause 3: replay ec2__lambda__s3.e2e-neptune-bridge-carve-lifecycle; it exhausts the 60s budget with no attributed step.
