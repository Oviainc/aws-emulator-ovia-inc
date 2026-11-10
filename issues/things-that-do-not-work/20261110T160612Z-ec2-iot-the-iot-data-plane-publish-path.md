Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__iot.iot-dangling-sourcevpce-reference-accepted-over-permissive
Issue-Key: ec2__iot.iot-dangling-sourcevpce-reference-accepted-over-permissive|run#32|Timeout|time.sleep(5.0) # policy attachment propagates to the data plane asynchronously
Issue-Commit: b11785bf9bf8af57a00ad7e4d814b1c5f1d66117

# ec2__iot: the IoT data-plane publish path never answers, so the dangling aws:SourceVpce policy scenario never reaches a verdict

> _Note: this issue was identified against the code base at commit `b11785bf9bf8af57a00ad7e4d814b1c5f1d66117`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeEndpoint

## Description
Cause 1 of 1: the scenario that proves an IoT policy pinned to a nonexistent VPC endpoint is accepted and is indistinguishable at the data plane never completes — no step produces a comparable response inside the check's budget.

AWS:      DescribeEndpoint(endpointType='iot:Data-ATS') returns a host that accepts a mutual-TLS HTTPS publish at POST /topics/<topic>?qos=1; a principal whose attached policy carries no aws:SourceVpce condition publishes successfully (HTTP 200) from outside the VPC, while a principal whose policy pins a SourceVpce — live or dangling — is refused HTTP 403. A newly created interface VPC endpoint for com.amazonaws.<region>.iot.data leaves 'pending' for 'available'. Recorded behavior only (the data-plane HTTP contract is documented behaviour, not Smithy shapes).
Emulator: the check produces no verdict — `Error: Check timed out after 60.0s`. Its unbounded waits are the DescribeVpcEndpoints 'pending'->'available' poll for the live control endpoint and the retry loop that publishes as the unconditioned control principal against the host DescribeEndpoint announced. Hypothesis: the announced iot:Data-ATS host never answers 200 for the unconditioned principal, so the positive control never establishes and the two 403 assertions are never reached; the endpoint the emulator announces is the emulator's own responsibility, since it is the address it tells clients to use.
Why:      without a working publish path the whole SourceVpce authorization contract is unverifiable: a policy pinned to an endpoint that does not exist looks exactly like one pinned to a live endpoint, and a customer cannot tell an over-permissive accepted reference from an enforced one.
Fix:      the IoT data-plane publish path (the mutual-TLS HTTPS topic endpoint the emulator announces through DescribeEndpoint) and the policy evaluation for aws:SourceVpce on it; secondarily the interface-VPC-endpoint state transition the control half polls.
Traces:   ec2__iot.iot-dangling-sourcevpce-reference-accepted-over-permissive.

## Evidence
Expected (AWS): DescribeVpcEndpoints for a syntactically valid but nonexistent id answers InvalidVpcEndpointId.NotFound "The Vpc Endpoint Id '<id>' does not exist"; the live endpoint reaches State='available'; the unconditioned principal publishes HTTP 200 to the iot:Data-ATS host; both the dangling-pinned and live-pinned principals are refused HTTP 403.
Actual (emulator): `Error: Check timed out after 60.0s` — the scenario produced no verdict; neither the control publish nor the two refusals were observed.

## Steps to Reproduce
1. DescribeEndpoint(endpointType='iot:Data-ATS') and note the host.
2. Create a VPC, subnet and an interface VPC endpoint for com.amazonaws.<region>.iot.data; poll DescribeVpcEndpoints until it leaves 'pending'.
3. Create three IoT things/certificates/policies: one with aws:SourceVpce pinned to a nonexistent vpce- id, one pinned to the live endpoint, one with no condition.
4. From outside the VPC, POST /topics/<topic>?qos=1 to the announced host with each certificate — the unconditioned one never answers HTTP 200.
