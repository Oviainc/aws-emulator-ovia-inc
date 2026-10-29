Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-conflict-elastic-ip-association
Issue-Step: teardown#12
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-conflict-elastic-ip-association ec2-t12-eip-disassociate-and-domain-attribute ec2-application-status-check-lifecycle
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|ReleaseAddress|2xx->4xx|status
Issue-Key: ec2|AssociateAddress|2xx->5xx|status
Issue-Key: ec2|DescribeApplicationStatusChecks|2xx->2xx|$.DescribeApplicationStatusChecksResponse.applicationStatusCheckSet
Issue-Commit: c42b5bef1562db44010b2579a8522ff39a3564f6

# EC2 elastic-IP and application-status-check state is not released or honored after lifecycle calls

> _Note: this issue was identified against the code base at commit `c42b5bef1562db44010b2579a8522ff39a3564f6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ReleaseAddress

## Description
Three net-new causes where a request AWS answers 200 is refused or not reflected, because the emulator's record of the resource is out of step with the calls it just served.

Cause 1 of 3 - ReleaseAddress refuses an address AWS releases (fixture ec2-conflict-elastic-ip-association, step teardown#12). AWS: releasing an allocation that the account owns and that is not associated succeeds with HTTP 200 and `return` true. Emulator: HTTP 400. The same fixture's AssociateAddress step reports the allocation as non-existent (see the EC2 error-class bundle), so the underlying fault is the address record being lost after the association attempt, which then makes teardown impossible. Why: the fixture's own teardown cannot complete, and a client leaks an Elastic IP it believes is released. Fix: the Elastic-IP address record in state plus the ReleaseAddress handler's ownership/association precondition.

Cause 2 of 3 - AssociateAddress answers 501 NotImplemented on a path the corpus exercises as 200 (ec2-t12-eip-disassociate-and-domain-attribute#setup#7). AWS: associating an allocated VPC address with an instance or interface returns 200 with an associationId. Emulator: 501 - the operation is reachable in other fixtures, so the whole operation is not a scaffold; this request shape falls through to the not-implemented branch. The cluster carries NO stub_path, so no stub issue owns it. Why: the fixture cannot even reach its assertions, and a client sees an unmodeled 501 for a core EIP call. Fix: the AssociateAddress handler must handle this request shape instead of falling through.

Cause 3 of 3 - DescribeApplicationStatusChecks still lists a deleted check (ec2-application-status-check-lifecycle#task#4). AWS: after the check is deleted the describe returns an empty applicationStatusCheckSet. Emulator: returns the deleted check (asc- id, aggregation, protocol, port, path, interval, timeout, thresholds, creationTime). Why: a deletion-convergence poll never terminates. Fix: the delete path must remove the record from state (the DeleteApplicationStatusCheck scaffold is tracked separately as 20261029T175114Z-ec2-deleteapplicationstatuscheck-stub.md, but this describe-side divergence is reported as its own cluster with no stub_path and no owner, so the read path must stop serving deleted checks once the delete lands).

Traces: representative step plus the two fixture#step bindings in trace_ids.

## Evidence
Replay detail, verbatim:
1. ReleaseAddress (ec2-conflict-elastic-ip-association#teardown#12): 'status: expected 200, got 400'.
2. AssociateAddress (ec2-t12-eip-disassociate-and-domain-attribute#setup#7): 'status: expected 200, got 501'; divergence_key service=ec2|AssociateAddress|status=200->501, stub_path empty.
3. DescribeApplicationStatusChecks (ec2-application-status-check-lifecycle#task#4): $.DescribeApplicationStatusChecksResponse.applicationStatusCheckSet type_mismatch (expected '', got {'item': {'applicationStatusCheckId': 'asc-00000000000000022', 'aggregation': 'included', 'protocol': 'http', 'port': '80', 'path': '/health', 'interval': '60', 'timeout': '6', 'failureThreshold': '2', 'successThreshold': '2', 'statusCodeMatcher': '200', 'initializationGracePeriodSeconds': '300', ...}}).
All three clusters: verdict=mismatch, stub_path empty, owned_by empty.

## Steps to Reproduce
1. Clone, boot the emulator built from the checkout.
2. replay_wire_corpus on simparity-wire-aws-v1.
3. Replay ec2-conflict-elastic-ip-association through teardown#12 (allocate, associate, disassociate, release) and the two fixture#step bindings in trace_ids; compare the HTTP status of ReleaseAddress/AssociateAddress and the body of DescribeApplicationStatusChecks after the delete with the recordings.
