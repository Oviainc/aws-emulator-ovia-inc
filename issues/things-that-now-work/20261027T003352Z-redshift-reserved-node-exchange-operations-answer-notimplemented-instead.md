Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: redshift.reserved-node-exchange-offering-consistency redshift.reserved-node-read-and-exchange-semantics
Issue-Shape-Id: com.amazonaws.redshift#RedshiftServiceVersion20121201
Issue-Key: redshift.reserved-node-exchange-offering-consistency|run#6|AssertionError|assert offerings_code == "ReservedNodeNotFound", offerings_code
Issue-Key: redshift.reserved-node-read-and-exchange-semantics|run#12|AssertionError|assert observed == expected, f"reserved-node contract drift:\n expected={expected}\n observed={observed}"
Issue-Commit: 4bfacb2b54a1c65638c52b387b9afadee41617eb

# Redshift reserved-node exchange operations answer NotImplemented instead of their modeled validation and not-found errors (4 root causes)

> _Note: this issue was identified against the code base at commit `4bfacb2b54a1c65638c52b387b9afadee41617eb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetReservedNodeExchangeOfferings

## Description
Four distinct root causes, all in the redshift service, all with the same shape: a reserved-node *exchange* operation's handler answers the generic `NotImplemented` error instead of validating its input and dereferencing the caller's actual reserved-node / cluster inventory. The plain inventory reads (DescribeReservedNodes, DescribeReservedNodeOfferings) DO answer, so this is not a whole-subsystem outage: only the four exchange operations below are inert, and each is a separate handler that must be closed separately. No open issue covers this family (the closest, 20261024T024248Z-redshift-cluster-scoped-credential-and-notification-operations-answer.md, enumerates cluster-, snapshot-, endpoint-access- and datashare-scoped handlers and does not name any reserved-node operation; 20261022T235108Z-ec2-reserved-instance-exchange-capacity-reservation-usage-dedicated.md is the EC2 reserved-INSTANCE family, a different service).

Cause 1 of 4: GetReservedNodeExchangeOfferings neither format-validates ReservedNodeId nor resolves it.
AWS: Smithy (com.amazonaws.redshift#RedshiftServiceVersion20121201) GetReservedNodeExchangeOfferings requires ReservedNodeId and models errors ReservedNodeNotFoundFault (awsQueryError code `ReservedNodeNotFound`, HTTP 404), ReservedNodeOfferingNotFoundFault (`ReservedNodeOfferingNotFound`, 404), InvalidReservedNodeStateFault (`InvalidReservedNodeState`, 400), ReservedNodeAlreadyMigratedFault (400), UnsupportedOperationFault (`UnsupportedOperation`, 400), DependentServiceUnavailableFault (400). Recorded behavior: reserved-node ids are UUID-shaped, and the FORMAT is checked before any lookup — a non-UUID id (e.g. `sim2real-behav-missing-reserved-node`) is refused with `InvalidParameterValue` / HTTP 400, while a syntactically valid but never-purchased UUID is refused with `ReservedNodeNotFound` / HTTP 404 and message `Reserved Node <ReservedNodeId> not found.` — fixed text `Reserved Node ` ... ` not found.` with the request's ReservedNodeId value substituted (never a literal id from one run).
Emulator: both legs answer error code `NotImplemented` (message of the form "operation GetReservedNodeExchangeOfferings is scaffolded but not implemented in this emulator").
Why: a client cannot tell a malformed reserved-node id from one it does not own, so exchange tooling that branches on ReservedNodeNotFound to decide "nothing to exchange" vs "bad input" cannot branch at all, and SDK error classifiers see an unmodelled code rather than a 400/404 client fault.
Fix: the GetReservedNodeExchangeOfferings handler in the redshift service's ops directory — add ReservedNodeId format validation, then resolve the id against the account's reserved-node inventory state and raise ReservedNodeNotFoundFault.
Traces: redshift.reserved-node-exchange-offering-consistency, redshift.reserved-node-read-and-exchange-semantics.

Cause 2 of 4: AcceptReservedNodeExchange does not validate its identifiers.
AWS: Smithy AcceptReservedNodeExchange requires ReservedNodeId and TargetReservedNodeOfferingId and models ReservedNodeNotFoundFault (`ReservedNodeNotFound`, 404), ReservedNodeOfferingNotFoundFault (404), ReservedNodeAlreadyExistsFault, ReservedNodeAlreadyMigratedFault, InvalidReservedNodeStateFault, UnsupportedOperationFault, DependentServiceUnavailableFault. Recorded behavior: a non-UUID ReservedNodeId is refused with `InvalidParameterValue` / HTTP 400 before any inventory lookup.
Emulator: answers `NotImplemented`.
Why: the mutating step of an exchange workflow is unreachable and its refusal class is unobservable, so a caller cannot distinguish bad input from an already-migrated node.
Fix: the AcceptReservedNodeExchange handler — identifier format validation first, then reserved-node/offering resolution and the exchange state transition.
Traces: redshift.reserved-node-read-and-exchange-semantics.

Cause 3 of 4: DescribeReservedNodeExchangeStatus does not enforce the "supply at least one filter" rule.
AWS: Smithy DescribeReservedNodeExchangeStatus takes optional ReservedNodeId and ReservedNodeExchangeRequestId and models ReservedNodeExchangeNotFoundFault (404), ReservedNodeNotFoundFault (`ReservedNodeNotFound`, 404), UnsupportedOperationFault (400). Recorded behavior: a call that supplies NEITHER filter (only MaxRecords) is refused with `InvalidParameterCombination` / HTTP 400 — the request is rejected on the parameter combination before any lookup.
Emulator: answers `NotImplemented`.
Why: a client polling exchange progress gets an unmodelled code for an unfiltered call instead of the 400 that tells it to pass a filter, and never reaches the filtered read either.
Fix: the DescribeReservedNodeExchangeStatus handler — parameter-combination validation plus the exchange-status read from the redshift service's reserved-node exchange state.
Traces: redshift.reserved-node-read-and-exchange-semantics.

Cause 4 of 4: GetReservedNodeExchangeConfigurationOptions does not dereference ClusterIdentifier.
AWS: Smithy GetReservedNodeExchangeConfigurationOptions requires ActionType (enum ReservedNodeExchangeActionType) and takes ClusterIdentifier / SnapshotIdentifier; it models ClusterNotFoundFault (awsQueryError `ClusterNotFound`, HTTP 404), ClusterSnapshotNotFoundFault (404), UnsupportedOperationFault (`UnsupportedOperation`, 400), ReservedNodeNotFoundFault, ReservedNodeOfferingNotFoundFault, InvalidReservedNodeStateFault, ReservedNodeAlreadyMigratedFault, DependentServiceUnavailableFault. Recorded behavior: with ActionType=resize-cluster and a ClusterIdentifier that does not exist, the call is refused `ClusterNotFound` / 404 with message `The cluster <ClusterIdentifier> was not found.` (fixed text `The cluster ` ... ` was not found.`, the identifier taken from the request); with a ClusterIdentifier that DOES exist but whose node type is not DS2, it is refused `UnsupportedOperation` / 400 with the fixed message `You can only exchange DS2 reserved nodes.` — i.e. the service resolves the identifier to a real cluster and inspects its node type, so "exists but not exchange-eligible" is distinguishable from "does not exist".
Emulator: answers `NotImplemented` for the nonexistent-cluster leg. The existing-but-non-DS2 leg is behind that same handler and was never reached in this run, but it is part of the same contract and must be satisfied: a handler that answers ClusterNotFound for every identifier is still divergent.
Why: resize/restore planning tools cannot tell a typo'd cluster name from a cluster that simply is not DS2, and get no configuration options at all.
Fix: the GetReservedNodeExchangeConfigurationOptions handler — resolve ClusterIdentifier (or SnapshotIdentifier) from the redshift service's cluster/snapshot state, raise ClusterNotFoundFault when absent, and raise UnsupportedOperationFault after inspecting a resolved cluster's node type.
Traces: redshift.reserved-node-exchange-offering-consistency, redshift.reserved-node-read-and-exchange-semantics.

## Evidence
Cause 1 — redshift.reserved-node-exchange-offering-consistency (check_redshift_reserved_node_exchange_offering_consistency.py line 59): positive control `DescribeReservedNodes` SUCCEEDS (ReservedNodes present), then `GetReservedNodeExchangeOfferings(ReservedNodeId=<random uuid4>)` is asserted to fail `ReservedNodeNotFound` with message "Reserved Node <uuid> not found." —
  AssertionError: NotImplemented

Causes 1-4 — redshift.reserved-node-read-and-exchange-semantics (check_redshift_reserved_node_read_and_exchange_semantics.py line 104): DescribeReservedNodeOfferings and DescribeReservedNodes both answer well-formed lists, then the four exchange probes:
  AssertionError: reserved-node contract drift:
   expected={'DescribeReservedNodeExchangeStatus(no-filter)': 'InvalidParameterCombination', 'GetReservedNodeExchangeOfferings': 'InvalidParameterValue', 'AcceptReservedNodeExchange': 'InvalidParameterValue', 'GetReservedNodeExchangeConfigurationOptions': 'ClusterNotFound'}
   observed={'DescribeReservedNodeExchangeStatus(no-filter)': 'NotImplemented', 'GetReservedNodeExchangeOfferings': 'NotImplemented', 'AcceptReservedNodeExchange': 'NotImplemented', 'GetReservedNodeExchangeConfigurationOptions': 'NotImplemented'}

In every case the emulator answers error code `NotImplemented` (HTTP 501) with the scaffold message, where AWS answers a modeled client fault: HTTP 400 `InvalidParameterValue` / `InvalidParameterCombination`, or HTTP 404 `ClusterNotFound` / `ReservedNodeNotFound`.

## Steps to Reproduce
1. Boot the emulator and point a redshift client at it.
2. Confirm the positive controls still answer: `DescribeReservedNodes(MaxRecords=20)` and `DescribeReservedNodeOfferings(MaxRecords=20)` return lists (offerings carry ReservedNodeOfferingId, NodeType, Duration > 0).
3. Cause 1: call `GetReservedNodeExchangeOfferings(ReservedNodeId=<a random uuid4>)`. Expect `ReservedNodeNotFound`, HTTP 404, message "Reserved Node <that uuid> not found."; observe NotImplemented. Then call it with a non-UUID id (e.g. "sim2real-behav-missing-reserved-node") and expect `InvalidParameterValue`, HTTP 400.
4. Cause 2: call `AcceptReservedNodeExchange(ReservedNodeId="sim2real-behav-missing-reserved-node", TargetReservedNodeOfferingId="sim2real-behav-missing-offering")`. Expect `InvalidParameterValue`, HTTP 400.
5. Cause 3: call `DescribeReservedNodeExchangeStatus(MaxRecords=20)` with neither ReservedNodeId nor ReservedNodeExchangeRequestId. Expect `InvalidParameterCombination`, HTTP 400.
6. Cause 4: call `GetReservedNodeExchangeConfigurationOptions(ActionType="resize-cluster", ClusterIdentifier=<a name that does not exist>)`. Expect `ClusterNotFound`, HTTP 404, message "The cluster <that identifier> was not found.". Then create a single-node non-DS2 cluster (e.g. dc2.large), wait for `available`, and call the same operation with that ClusterIdentifier: expect `UnsupportedOperation`, HTTP 400, message "You can only exchange DS2 reserved nodes.".
7. Re-replay redshift.reserved-node-exchange-offering-consistency and redshift.reserved-node-read-and-exchange-semantics to confirm.
