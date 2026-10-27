Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__iot.iot-dangling-sourcevpce-reference-accepted-over-permissive
Issue-Key: ec2__iot.iot-dangling-sourcevpce-reference-accepted-over-permissive|run#13|AssertionError|assert described["State"] == "available", (
Issue-Commit: 3d2bed9d9c0136653e6daec548ca7ef547e87a29

# EC2 interface VPC endpoints report VpcEndpoint.State as the PascalCase 'Pending' and never reach 'available', so every endpoint-readiness wait fails (1 cause)

> _Note: this issue was identified against the code base at commit `3d2bed9d9c0136653e6daec548ca7ef547e87a29`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeVpcEndpoints

## Description
One net-new root cause, grounded in the single failing behavioral check whose id leads with `ec2__iot.`. The replay report marks it neither ALREADY FILED nor OWNED, so no open issue carries its replay key. Cause count: 1 — numbered below with the failing check id it must be verified against before this issue is submitted.

RELATED OPEN ISSUES (neither covers this check, and neither carries its id): 20261026T090242Z-apigateway-every-private-custom-domain-domain-name-access.md describes the SAME EC2 endpoint-state machine, but under the `apigateway` service token and grounded only in eight `apigateway.` checks that provision a com.amazonaws.<region>.execute-api endpoint; 20261026T063806Z-logs-cloudwatch-logs-createdelivery-refuses-an-idempotent-re-pair.md names it as its cause 2 under the `logs` token, grounded in logs.list-syslog-configurations-reflects-put-and-delete (a com.amazonaws.<region>.syslog-logs endpoint). The check below provisions a com.amazonaws.<region>.iot.data endpoint and is replayed by neither issue, so a fix to the state machine must be confirmed against THIS check as well as those two lists. It also pins down the spelling half of the divergence directly, which the timeout-shaped failures in those issues could not show.

=== Cause 1 of 1: a created interface VPC endpoint is reported in state `Pending` — the model's PascalCase member name, not the wire spelling — and never becomes `available` ===
AWS: Smithy (com.amazonaws.ec2#AmazonEC2) CreateVpcEndpoint returns `VpcEndpoint`, whose `State` member ("The state of the endpoint", xmlName `state`, ec2QueryName `State`, target enum com.amazonaws.ec2#State with members PendingAcceptance | Pending | Available | Deleting | Deleted | Rejected | Failed | Expired | Partial) is non-terminal on create. On the wire EC2 serializes this member in its lowercase/camelCase spelling — `pending`, then `available` (and `pendingAcceptance`, `deleting`, `deleted`, `rejected`, `failed`, `expired` for the rest) — never the PascalCase member names the model lists; every client, waiter and poller compares against those lowercase values exactly. An interface endpoint to an AWS-provided service (here com.amazonaws.<region>.iot.data) is answered `pending` by the create and reaches `available` with NO further caller action, which is why DescribeVpcEndpoints ("Describes your VPC endpoints") is the documented way to observe readiness. Recorded behavior: the check creates the endpoint with CreateVpcEndpoint(VpcEndpointType='Interface', ServiceName='com.amazonaws.<region>.iot.data', SubnetIds=[S], SecurityGroupIds=[G], PrivateDnsEnabled=False, IpAddressType='ipv4'), then polls `while described["State"] == "pending"` for up to 240 s and requires the terminal read to be exactly `available`.
Emulator: DescribeVpcEndpoints reports `State` as `Pending` — the PascalCase spelling — and never any other value. Because `Pending` != `pending`, the check's poll loop exits on its FIRST read without waiting at all, and the assertion fails with `the live endpoint vpce-<id> is in state 'Pending', not 'available'; the control half of this check needs a working endpoint` (the endpoint id in that message is a per-run value; the diverging thing is the state VALUE). No status code and no error code diverge here — a response VALUE is wrong, and it is wrong in two compounding ways from one place: the spelling, and the fact that the stored value never advances. The checkout shows both: services/service-com-amazonaws-ec2-amazonec2/src/ops/create_vpc_endpoint.rs stores `state: Some(state::VPC_ENDPOINT_STATE_PENDING.to_string())` (line 777) and records the owed move `store.vpc_endpoint_moves.insert(endpoint_id.clone(), state::PendingStateMove::starting_now(state::VPC_ENDPOINT_STATE_AVAILABLE))` (lines 831-834), with its own module doc stating it "answers it in `Pending`"; the related apigateway issue establishes that `ComAmazonawsEc2Amazonec2State::materialize_due_transitions` in services/service-com-amazonaws-ec2-amazonec2/src/state.rs never consumes `vpc_endpoint_moves`, so the owed Pending -> Available move is applied by nothing, which is why no amount of waiting would have helped either.
Why: endpoint readiness is the gate in front of every PrivateLink-dependent workflow. A client that creates an interface endpoint and waits for it — CloudFormation/CDK, Terraform, botocore's own `vpc_endpoint` waiters, or a hand-rolled poll — either exits immediately on an unrecognised state value and proceeds against an endpoint it believes failed, or blocks until its own deadline; both read as a provisioning failure. A client that branches on the state string (`state == 'available'`, `state in ('available','failed')`) takes the wrong branch, and a client that filters with DescribeVpcEndpoints(Filters=[{'Name':'vpc-endpoint-state','Values':['available']}]) selects nothing. Here it also means the IoT half of the check is never exercised at all: whether an IoT policy stores an `aws:SourceVpce` condition naming a nonexistent endpoint as readily as a live one, and whether both pinned policies refuse a publish from outside the VPC while an unpinned one succeeds, remains unobserved.
Fix: the EC2 VPC-endpoint state machine, not anything in the iot crate. (a) The state values written by services/service-com-amazonaws-ec2-amazonec2/src/ops/create_vpc_endpoint.rs and reported by services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_vpc_endpoints.rs must be the wire spellings EC2 puts on the wire (`pending`, `available`, `pendingAcceptance`, `deleting`, `deleted`, `rejected`, `failed`, `expired`), not the model's PascalCase member names. (b) `ComAmazonawsEc2Amazonec2State::materialize_due_transitions` in services/service-com-amazonaws-ec2-amazonec2/src/state.rs must apply the moves held in `vpc_endpoint_moves` — the same retain-and-apply shape the VPC, subnet and carrier-gateway moves beside it use — so an endpoint is reported available once TRANSITION_DELAY has passed whether or not anyone called in between. The same materialized value must be what the `vpc-endpoint-state` filter and the delete path see, so one endpoint is never reported two ways.
Traces: ec2__iot.iot-dangling-sourcevpce-reference-accepted-over-permissive.

## Evidence
Replay report, suite simparity-behavioral-ga-prod-v1 (106/478 passed; service group `ec2__iot` 0/1 with 1 failure):

  [mismatch] ec2__iot.iot-dangling-sourcevpce-reference-accepted-over-permissive
      File "check_ec2__iot_iot_dangling_sourcevpce_reference_accepted_over_permissive.py", line 71, in run
        assert described["State"] == "available", (
      AssertionError: the live endpoint vpce-00000000000000029 is in state 'Pending', not 'available';
      the control half of this check needs a working endpoint

Where it fails, from the check source carried in the report (SERVICES = ["ec2", "iot"], _AVAILABLE_POLL_S = 240.0):
    service = f"com.amazonaws.{ctx.region}.iot.data"
    live = ec2.create_vpc_endpoint(VpcEndpointType="Interface", VpcId=..., ServiceName=service,
                                   SubnetIds=[...], SecurityGroupIds=[...],
                                   PrivateDnsEnabled=False, IpAddressType="ipv4",
                                   TagSpecifications=[...])["VpcEndpoint"]["VpcEndpointId"]
    described = _poll_available(ec2, live)
    assert described["State"] == "available", (...)
  and the poller it calls:
    def _poll_available(ec2, endpoint_id):
        """Poll DescribeVpcEndpoints until the endpoint leaves 'pending'."""
        deadline = time.monotonic() + 240.0
        described = ec2.describe_vpc_endpoints(VpcEndpointIds=[endpoint_id])["VpcEndpoints"][0]
        while described["State"] == "pending" and time.monotonic() < deadline:
            time.sleep(5.0)
            described = ec2.describe_vpc_endpoints(VpcEndpointIds=[endpoint_id])["VpcEndpoints"][0]
        return described
The loop's guard is the lowercase wire spelling, so a report of `Pending` makes it exit after ONE read: the failure is a wrong state value on the first DescribeVpcEndpoints, not a timeout. Everything before it answered without raising (ec2 DescribeVpcEndpoints for a dangling id correctly raised InvalidVpcEndpointId.NotFound, CreateVpc, CreateSubnet, CreateSecurityGroup, DescribeVpcEndpointServices, CreateVpcEndpoint), and everything after it — the IoT policy/certificate attachment, GetPolicy round-trip of the aws:SourceVpce condition, and the three mutual-TLS publishes — was never reached.

Checkout corroboration (same commit under replay):
  services/service-com-amazonaws-ec2-amazonec2/src/ops/create_vpc_endpoint.rs:777 — `state: Some(state::VPC_ENDPOINT_STATE_PENDING.to_string())`
  services/service-com-amazonaws-ec2-amazonec2/src/ops/create_vpc_endpoint.rs:831-834 — `store.vpc_endpoint_moves.insert(endpoint_id.clone(), state::PendingStateMove::starting_now(state::VPC_ENDPOINT_STATE_AVAILABLE));` under the comment "What the clock now owes this endpoint: `Available`, one TRANSITION_DELAY from now"
  services/service-com-amazonaws-ec2-amazonec2/src/ops/create_vpc_endpoint.rs:7 — module doc: "answers it in `Pending` with the instant the clock owes it `Available` recorded beside it"
  Smithy com.amazonaws.ec2#State enum members: PendingAcceptance, Pending, Available, Deleting, Deleted, Rejected, Failed, Expired, Partial — model member names, not the wire values EC2 serializes.

Expected: DescribeVpcEndpoints(VpcEndpointIds=[E]) reports State `pending` immediately after the create and `available` shortly after, with no further caller action. Actual: State is `Pending` on the first read and never changes.

## Steps to Reproduce
Boot the emulator, then (region = the Region the client signs for):
  1. ec2 CreateVpc(CidrBlock='10.45.0.0/16') -> V
  2. ec2 DescribeVpcEndpointServices(Filters=[{'Name':'service-name','Values':['com.amazonaws.<region>.iot.data']}]) -> pick an AZ from ServiceDetails[0].AvailabilityZones that is also available
  3. ec2 CreateSubnet(VpcId=V, CidrBlock='10.45.1.0/24', AvailabilityZone=<that AZ>) -> S; ec2 CreateSecurityGroup(VpcId=V, GroupName=<name>, Description=<text>) -> G
  4. ec2 CreateVpcEndpoint(VpcId=V, ServiceName='com.amazonaws.<region>.iot.data', VpcEndpointType='Interface', SubnetIds=[S], SecurityGroupIds=[G], PrivateDnsEnabled=False, IpAddressType='ipv4') -> note VpcEndpointId as E; the create's VpcEndpoint.State should be the non-terminal pending value spelled `pending`.
     -> OBSERVE it is spelled `Pending`.
  5. ec2 DescribeVpcEndpoints(VpcEndpointIds=[E]) immediately, then again after 2 s, 10 s and 60 s
     -> OBSERVE VpcEndpoints[0].State is `Pending` on every read and never changes.
     -> EXPECT `pending` on the first read and `available` on a later one, with no further caller action.
  6. ec2 DescribeVpcEndpoints(Filters=[{'Name':'vpc-endpoint-state','Values':['available']}]) -> EXPECT E to be selected once it is available; OBSERVE it is not.
  7. Once the gate opens, confirm the surface behind it, which no run has reached: iot CreatePolicy with a statement conditioned on {'StringEquals': {'aws:SourceVpce': 'vpce-00000000000000000'}} (an id ec2 DescribeVpcEndpoints answers InvalidVpcEndpointId.NotFound for) -> expect 200 and GetPolicy to return that condition value verbatim, with no EC2 endpoint minted; the same document pinned to E -> expect 200, and ec2 DescribeVpcEndpoints(VpcEndpointIds=[E]) to resolve to ServiceName 'com.amazonaws.<region>.iot.data' in state 'available'.
Re-replay ec2__iot.iot-dangling-sourcevpce-reference-accepted-over-permissive to confirm the fix, and re-replay the eight `apigateway.` checks and logs.list-syslog-configurations-reflects-put-and-delete named in the related issues, since they exercise the same state machine.
