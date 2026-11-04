Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.ec2mr-inv2-stopping-an-instance-releases-its-public-ip-and-drops-it-from-the-running-filter ec2.hibernate-restores-the-running-process-and-a-plain-stop-does-not ec2.droute-deleted-peering-target-blackholes-the-route-and-drops-traffic ec2.droute-longest-prefix-match-decides-between-overlapping-route-targets ec2.asvpce-streaming-vpce-eni-delete-rejected ec2.op-contract-reject-vpc-endpoint-connections ec2.e2e-route-server-bgp-propagation-flow
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.ec2mr-inv2-stopping-an-instance-releases-its-public-ip-and-drops-it-from-the-running-filter|_poll_until#8|Timeout|time.sleep(interval_s)
Issue-Key: ec2.hibernate-restores-the-running-process-and-a-plain-stop-does-not|_retry_dependency#3|Timeout|time.sleep(interval_s)
Issue-Key: ec2.droute-deleted-peering-target-blackholes-the-route-and-drops-traffic|_retry_dependency#5|Timeout|time.sleep(interval_s)
Issue-Key: ec2.droute-longest-prefix-match-decides-between-overlapping-route-targets|_retry_dependency#5|Timeout|time.sleep(interval_s)
Issue-Key: ec2.asvpce-streaming-vpce-eni-delete-rejected|_poll#8|Timeout|time.sleep(interval_s)
Issue-Key: ec2.op-contract-reject-vpc-endpoint-connections|_poll_until#23|Timeout|time.sleep(interval_s)
Issue-Key: ec2.e2e-route-server-bgp-propagation-flow|run#22|EgressRefused|eni_address, target_port = ctx.endpoint_host(ep["EniAddress"], BGP_PORT)
Issue-Commit: c832bd0d619782e17fabc455ea6e3058ac5fd237

# EC2 state never converges for six waited-on transitions (checks hang to the 60s cap), and the route-server flow announces an address outside the endpoint

> _Note: this issue was identified against the code base at commit `c832bd0d619782e17fabc455ea6e3058ac5fd237`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StopInstances

## Description
This issue bundles 2 net-new root causes in the EC2 emulator. Smithy: the ec2 model binds awsQuery and models no operation errors, so the terminal states below are recorded behavior only.

Cause 1 of 2: resources never reach the terminal state the API promises, so a client that polls DescribeX until the documented state never finishes.
AWS: each of these transitions completes and is observable: deleting the ENI that backs a VPC endpoint is refused while the endpoint owns it (and the endpoint's own deletion releases it); a route whose peering target is deleted becomes blackhole and stops carrying traffic; between overlapping routes the longest prefix wins; StopInstances moves the instance to 'stopped', releases its public IPv4 address and drops it from the instance-state-name=running filter; StopInstances(Hibernate=true) followed by StartInstances restores the running process while a plain stop does not; RejectVpcEndpointConnections moves the connection to 'rejected'.
Emulator: all six checks hang and are cut off — "Check timed out after 60.0s" — i.e. the polled state is never reached (no refusal, no progress, no terminal state).
Why: a client that waits on EC2's documented state machine (every SDK waiter does) blocks forever instead of either converging or being refused; the failure is indistinguishable from a hung service.
Fix: EC2 instance/route/endpoint state transitions — the stop/start (and hibernate) lifecycle in the instance state machine, route blackholing and longest-prefix selection in the route-table state, the VPC-endpoint ENI dependency and the endpoint-connection rejection handler.
Traces: ec2.ec2mr-inv2-stopping-an-instance-releases-its-public-ip-and-drops-it-from-the-running-filter, ec2.hibernate-restores-the-running-process-and-a-plain-stop-does-not, ec2.droute-deleted-peering-target-blackholes-the-route-and-drops-traffic, ec2.droute-longest-prefix-match-decides-between-overlapping-route-targets, ec2.asvpce-streaming-vpce-eni-delete-rejected, ec2.op-contract-reject-vpc-endpoint-connections.

Cause 2 of 2: the route-server/BGP flow hands back an address that is not the endpoint it is served from.
AWS: addresses an EC2 response reports for its own data path are reachable in the caller's network context; nothing in the response points a client at an unrelated host.
Emulator: the flow returns 10.241.1.4 as the host to talk to while the emulator is reached on 127.0.0.1, so following the emulator's own answer leaves the endpoint under test (the harness refuses the egress: "'10.241.1.4' is not the endpoint under test (127.0.0.1)").
Why: a client that follows an address the API returned is sent off the service it is talking to — the announced endpoint is wrong, not merely unreachable.
Fix: the route-server endpoint/peer serializer — announce the address the emulator is reached on rather than an internal network address.
Traces: ec2.e2e-route-server-bgp-propagation-flow.

## Evidence
Cause 1: six checks report exactly "Error: Check timed out after 60.0s" with no response recorded (ec2mr-inv2-stopping-an-instance..., hibernate-restores..., droute-deleted-peering-target..., droute-longest-prefix-match..., asvpce-streaming-vpce-eni-delete-rejected, op-contract-reject-vpc-endpoint-connections). Expected: the polled terminal state ('stopped' with the public IP released and absent from the running filter; 'blackhole'; the longest-prefix target; the ENI delete refused while the endpoint holds it; the connection 'rejected') within the wait.
Cause 2: "simparity.egress.EgressRefused: egress refused: '10.241.1.4' is not the endpoint under test (127.0.0.1); an emulator run may reach only the emulator. If the host came from a URL the emulator returned, the emulator announces an address other than the endpoint it is reached on".

## Steps to Reproduce
Replay the checks in trace_ids. For cause 1: RunInstances into a public subnet, StopInstances and poll DescribeInstances for state 'stopped' plus the absence of PublicIpAddress and of the instance under Filters=[{'Name':'instance-state-name','Values':['running']}]; RunInstances with HibernationOptions, StopInstances(Hibernate=True), StartInstances and read the restored process; delete a VPC peering that a route targets and poll the route's State for 'blackhole'; create overlapping routes and observe which target forwards; DeleteNetworkInterface on the ENI of a live interface endpoint; CreateVpcEndpoint against a service requiring acceptance and RejectVpcEndpointConnections, then poll the connection state. For cause 2: run the route-server BGP propagation flow and inspect the address the emulator returns for the route-server endpoint/peer.
