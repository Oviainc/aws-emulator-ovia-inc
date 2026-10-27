Issue-Source: trace-sweep-divergence
Issue-Trace: eks-access-entry-and-policy-operations-on-missing-cluster
Issue-Step: task#1
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: eks-access-entry-and-policy-operations-on-missing-cluster
Issue-Shape-Id: com.amazonaws.eks#AWSWesleyFrontend
Issue-Key: eks|ListAccessEntries|4xx->4xx|$.message
Issue-Key: eks|CreateAccessEntry|4xx->4xx|$.message
Issue-Commit: 1b15cfcf83a9fafcaa69f938ed17066b612a8ba0

# EKS access-entry not-found refusals do not name the missing cluster

> _Note: this issue was identified against the code base at commit `1b15cfcf83a9fafcaa69f938ed17066b612a8ba0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListAccessEntries

## Description
On the access-entry list/create paths AWS builds the ResourceNotFoundException message FROM THE REQUEST, naming the cluster that could not be resolved ('No cluster found for name: <clusterName>.'); the emulator answers a fixed, request-independent sentence instead. Because the message is derived from the request, the construction rule - not the recorded cluster name - is the requirement. TWO causes; the issue is not complete until both are closed. (Note: the same shared error constructor is the subject of the sibling wire issue on not-found bodies echoing request path members, where AWS's message is the generic one and the emulator's is the request-built one - i.e. the emulator applies ONE template everywhere instead of per-operation behavior. Fixing one direction must not regress the other.)

Cause 1 of 2 - ListAccessEntries on a cluster that does not exist.
AWS: Smithy (com-amazonaws-eks-awswesleyfrontend): ListAccessEntries models ResourceNotFoundException (httpError 404). Recorded: 404, error code ResourceNotFoundException, message built from the request - 'No cluster found for name: <clusterName>.'
Emulator: 404 ResourceNotFoundException with the fixed message 'The requested resource does not exist.', which names nothing.
Why: the cluster name is the only part of the refusal that identifies WHICH resource was missing on this path (the identity members are null), so a caller logging or parsing the refusal loses that entirely and cannot tell which of several clusters in a fan-out failed.
Fix: the ListAccessEntries handler's cluster-resolution failure path / the eks crate's not-found message builder, which must interpolate the requested clusterName for this operation.
Traces: eks-access-entry-and-policy-operations-on-missing-cluster#task#1.

Cause 2 of 2 - CreateAccessEntry on a cluster that does not exist.
AWS: Smithy: CreateAccessEntry models ResourceNotFoundException (404). Recorded: 404 ResourceNotFoundException with the same request-built message 'No cluster found for name: <clusterName>.'
Emulator: 404 ResourceNotFoundException with the fixed 'The requested resource does not exist.'
Why: as cause 1 - a create-then-retry client cannot report which cluster it must create first.
Fix: the CreateAccessEntry handler's cluster-resolution failure path.
Traces: eks-access-entry-and-policy-operations-on-missing-cluster#task#2.

## Evidence
Corpus simparity-wire-aws-v1. Status (404) and error code match in both causes; only the message body diverges, and it diverges by losing a request-derived value.

Cause 1: eks-access-entry-and-policy-operations-on-missing-cluster#task#1 ListAccessEntries - $.message: value_mismatch (expected 'No cluster found for name: sim2real-probe-eks-cluster-aef56670.', got 'The requested resource does not exist.').
Cause 2: eks-access-entry-and-policy-operations-on-missing-cluster#task#2 CreateAccessEntry - $.message: value_mismatch (expected 'No cluster found for name: sim2real-probe-eks-cluster-aef56670.', got 'The requested resource does not exist.').

## Steps to Reproduce
1. Boot the emulator and point an EKS client at it; pick CLUSTER=<a name never created>.
2. Cause 1: list_access_entries(clusterName=CLUSTER) -> expect 404 ResourceNotFoundException whose message interpolates CLUSTER: 'No cluster found for name: <CLUSTER>.'
3. Cause 2: create_access_entry(clusterName=CLUSTER, principalArn='arn:aws:iam::000000000000:role/x') -> expect the same request-built message.
4. Repeat with a second, different cluster name and confirm the message changes with the request rather than being a constant.
