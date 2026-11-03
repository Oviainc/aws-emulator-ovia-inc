Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda-core__lambda-microvms.microvm-denied-the-vpc-egress-connector-cannot-reach-the-private-subnet lambda-core__lambda-microvms.vpc-egress-connector-path-is-blocked-by-the-target-security-group
Issue-Key: lambda-core__lambda-microvms.microvm-denied-the-vpc-egress-connector-cannot-reach-the-private-subnet|_guest_get#25|Timeout|time.sleep(5)
Issue-Key: lambda-core__lambda-microvms.vpc-egress-connector-path-is-blocked-by-the-target-security-group|_guest_get#26|Timeout|time.sleep(5)
Issue-Commit: dd64cbf0b038e7955d699468def5671676941ec2

# lambda-core + lambda-microvms VPC-egress data-plane workflows never converge: each asynchronous lifecycle forces a polling client to burn a full poll interval, and the end-to-end check times out

> _Note: this issue was identified against the code base at commit `dd64cbf0b038e7955d699468def5671676941ec2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunMicrovm

## Description
One net-new root cause, grounded in both failing cross-service data-plane checks.

Cause 1 of 1 — the chained asynchronous lifecycles of a connector + MicroVM workflow (connector PENDING→ACTIVE, image CREATING→CREATED, MicroVM PENDING→RUNNING) are each gated behind a fixed wall-clock span and materialised only lazily on the next read, so every documented poll loop misses on its first probe and must sleep its whole interval; the workflow as a whole never completes and both checks end as "Check timed out after 60.0s" without a single AWS call being refused.

AWS: the model documents each step as asynchronous and converging — lambda-core CreateNetworkConnector: "This operation is asynchronous. The network connector starts in PENDING state while ENIs are provisioned in your VPC … Use GetNetworkConnector to poll the connector state until it reaches ACTIVE"; lambda-microvms CreateMicrovmImage: "The build is asynchronous — the image transitions from CREATING to CREATED on success"; lambda-microvms RunMicrovm: "The MicroVM starts in PENDING state and transitions to RUNNING once provisioning completes". The requirement is that a client polling these readers observes the terminal state; the states themselves (ACTIVE / CREATED / RUNNING), not any message, are the data under test. The end-to-end behaviour the two checks assert (a MicroVM attached to an ACTIVE VPC_EGRESS connector reaches a private address in the connector's subnet from a link-local source, while a MicroVM attached only to INTERNET_EGRESS times out) is recorded behaviour only.

Emulator: every one of those transitions is deferred behind a fixed 500 ms wall-clock span and applied lazily by a later read — lambda-microvms IMAGE_BUILD_SPAN, MICROVM_PROVISION_SPAN and MICROVM_TERMINATE_SPAN are each Duration::from_millis(500), and lambda-core's LIFECYCLE_TRANSITION is the same. Consequently each wait's first probe (taken immediately after the creating call) returns the transient state, and the client sleeps its entire poll interval before the probe that finally succeeds: 10 s for the interface VPC endpoint, 15 s for the connector, 5 s per MicroVM, plus the image build. Both checks chain four or more such waits and neither finishes inside its budget; the replay reports only the 60 s timeout, with no 4xx/5xx from any operation.

Hypothesis (stated by neither the model nor a recording): the aggregate of those forced poll intervals is what exhausts the budget — the suite's output carries no traceback for a timeout, so the exact wait in flight at 60 s is not identifiable from the report; the candidate waits are DescribeVpcEndpoints (available + NetworkInterfaceIds), GetNetworkConnector (ACTIVE), GetMicrovmImage (CREATED) and GetMicrovm (RUNNING).

Hypothesis, follow-on blocker a fixer will hit next: once the waits converge, these two checks GET /health and /connect on the published MicroVM endpoint. This checkout's endpoint deliberately answers 501 ("the emulator runs no MicroVM guest, so the guest's own response cannot be served") to every authenticated request, and the checks assert HTTP 200 with a JSON body from the guest, so closing this issue also requires the MicroVM endpoint to serve the guest's own response for the image's code artifact rather than 501.

Why a client gets this wrong: a client that follows AWS's documented poll-until-terminal-state loops cannot complete a connector + MicroVM workflow against the emulator at all — it is not told anything wrong, it simply never arrives, so every behaviour that depends on an ACTIVE connector or a RUNNING MicroVM (attachment, egress enforcement, endpoint access) is untestable and unusable.

Fix: this belongs in the two services' state/lifecycle layers, not in any serializer or validator. The provisioning/build/termination transitions must settle so that the first read following the call that started them already reports the terminal state (settle on read, or make the transition due at creation time), leaving the transient state observable in the creating call's own response; and the MicroVM endpoint must serve a guest response instead of 501.

Traces: lambda-core__lambda-microvms.microvm-denied-the-vpc-egress-connector-cannot-reach-the-private-subnet and lambda-core__lambda-microvms.vpc-egress-connector-path-is-blocked-by-the-target-security-group. Code citations for the fixer: services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/src/state.rs lines 46-68 (IMAGE_BUILD_SPAN / MICROVM_PROVISION_SPAN / MICROVM_TERMINATE_SPAN / IMAGE_DELETE_SPAN) and src/state/microvm_endpoint.rs lines 20-40 and 84-88 (the 501 guest answer); services/service-com-amazonaws-lambdacore-lambdacoreapiservice/src/state.rs line 153 (LIFECYCLE_TRANSITION). This cause shares its mechanism with the lambda-core issue filed in the same sweep, but it is a different service pairing and different checks, so both must be closed.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the emulator built from this checkout: 16/29 passed, bootstrap_signal false; per-service lambda-core__lambda-microvms 0/2 (0%).

[mismatch] lambda-core__lambda-microvms.microvm-denied-the-vpc-egress-connector-cannot-reach-the-private-subnet — Error: Check timed out after 60.0s
[mismatch] lambda-core__lambda-microvms.vpc-egress-connector-path-is-blocked-by-the-target-security-group — Error: Check timed out after 60.0s

No operation in either check returned an error status: nothing was refused, so this is not a wire/status divergence but a failure of the chained lifecycles to converge. The two checks' waits and intervals, from their sources: DescribeVpcEndpoints until State='available' and NetworkInterfaceIds non-empty, every 10 s; GetNetworkConnector until State in ('ACTIVE','FAILED'), every 15 s; GetMicrovm until state in ('RUNNING','FAILED','TERMINATED'), every 5 s per MicroVM; plus the CreateMicrovmImage build wait.

Expected (AWS): each poll loop observes its terminal state (connector ACTIVE, image CREATED, MicroVM RUNNING) and the workflow proceeds to the guest-reachability assertions.
Actual (emulator): each transition is held 500 ms of wall clock and applied only on a later read, so every loop misses its first probe and pays its full interval; the aggregate exceeds the check budget and the workflow never reaches the data-plane assertions. Grounded in this checkout's own constants, quoted in the description.

## Steps to Reproduce
1. git_clone, then run_behavioral_suite(); both lambda-core__lambda-microvms checks report "Check timed out after 60.0s".
2. By hand against the running emulator, in the order the checks use: CreateVpcEndpoint (Interface) in a fresh VPC/subnet/security group, then DescribeVpcEndpoints once immediately — observe the endpoint is not yet available with an ENI.
3. CreateNetworkConnector with a VpcEgressConfiguration naming that subnet and group, then GetNetworkConnector once immediately — observe State=PENDING (it only becomes ACTIVE on a read taken >500 ms later).
4. CreateMicrovmImage, then GetMicrovmImage once immediately — observe CREATING; RunMicrovm, then GetMicrovm once immediately — observe PENDING. Each step costs a polling client a whole poll interval, and the chained cost is what makes the end-to-end workflow exceed its budget.
5. For the follow-on blocker: once a MicroVM is RUNNING, CreateMicrovmAuthToken and GET <endpoint>/health with the X-aws-proxy-auth header — observe HTTP 501 instead of the guest's own 200 JSON response.
