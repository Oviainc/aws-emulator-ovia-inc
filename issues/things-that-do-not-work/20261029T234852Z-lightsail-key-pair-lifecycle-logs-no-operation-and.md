Issue-Source: trace-sweep-divergence
Issue-Trace: lightsail-catalog-and-operations-lifecycle
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lightsail-catalog-and-operations-lifecycle
Issue-Shape-Id: com.amazonaws.lightsail#Lightsail_20161128
Issue-Key: lightsail|GetOperationsForResource|2xx->2xx|$.operations[*]
Issue-Key: lightsail|DeleteKeyPair|2xx->4xx|$.operation
Issue-Key: lightsail|DeleteKeyPair|2xx->4xx|status
Issue-Commit: c1b8e21acf710dd592fea1840f63adefca5c3a99

# Lightsail key-pair lifecycle logs no operation and DeleteKeyPair refuses the key pair

> _Note: this issue was identified against the code base at commit `c1b8e21acf710dd592fea1840f63adefca5c3a99`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetOperationsForResource

## Description
Two net-new root causes on the Lightsail key-pair path: the operation log that every mutating Lightsail call must append to is never written for a key pair, and the delete of that key pair is refused instead of returning its Operation. Both must be closed separately.

Cause 1 of 2: GetOperationsForResource for a KeyPair resource returns an EMPTY operations list, so the CreateKeyPair operation record is never reported.
AWS: Smithy (com.amazonaws.lightsail#GetOperationsForResource) — input resourceName (ResourceName, required); output `operations` (OperationList) plus nextPageToken. Every mutating Lightsail call appends an Operation record (com.amazonaws.lightsail#Operation: id, resourceName, resourceType, createdAt, location, isTerminal, operationType, status, statusChangedAt). Recorded behavior: after a key pair is created, this operation returns at least one record for that resource with operationType "CreateKeyPair", resourceType "KeyPair", status "Succeeded", isTerminal true, and location {availabilityZone "all", regionName <the request's region>}; the id and the timestamps are per-run values, the operationType/resourceType/status/isTerminal/location shape is the requirement.
Emulator: `operations[0]` is absent — the list comes back empty with HTTP 200.
Why: the operation log is how a client waits for asynchronous Lightsail work (poll until isTerminal/status Succeeded) and how it audits what happened to a resource. An empty list is indistinguishable from "nothing ever happened", so a waiter either spins forever or concludes the create never occurred.
Hypothesis: CreateKeyPair in this crate is still an unimplemented scaffold (open issue 20261022T050650Z-lightsail-createkeypair-stub.md, and 20261022T050650Z-lightsail-key-pair-control-plane-createkeypair-and-deletekeypair.md), so no key pair and no operation record are ever created on this fixture's earlier step; this divergence is the separately observable wire consequence on GetOperationsForResource and is not owned by either of those issues. Closely related but distinct open issue: 20261028T190333Z-lightsail-createcontainerservice-wrong-behavior.md reports the same missing-operation-log fault for container services.
Fix: the key-pair creation handler must append its Operation record to the shared operation log, and the GetOperationsForResource handler must report the records held for a KeyPair resource name.
Traces: lightsail-catalog-and-operations-lifecycle#task#3.

Cause 2 of 2: DeleteKeyPair answers HTTP 400 where AWS answers HTTP 200 with an Operation.
AWS: Smithy (com.amazonaws.lightsail#DeleteKeyPair) — input keyPairName (ResourceName, required, pattern ^\w[\w\-]*\w$) plus optional expectedFingerprint; output `operation` (Operation); http code 200. Recorded behavior: deleting an existing key pair returns HTTP 200 with `operation` carrying operationType "DeleteKeyPair", resourceType "KeyPair", resourceName equal to the requested keyPairName, status "Succeeded", isTerminal true, operationDetails "" and location {availabilityZone "all", regionName <request region>}; id/createdAt/statusChangedAt are per-run values.
Emulator: HTTP 400 with no `operation` member at all, so the teardown of the key pair fails.
Why: the delete is the teardown step of every key-pair workflow; a 400 makes it unrecoverable for a client that has a key pair it believes exists, and the absent Operation means there is nothing to poll or audit. It also leaves the resource behind from the client's point of view.
Hypothesis: the 400 is the handler's not-found path firing because no key pair was ever created (see cause 1 and the CreateKeyPair stub issue). Note this contradicts the earlier behavioral issue that recorded DeleteKeyPair answering 501 NotImplemented — on this checkout it answers 400 — so the key-pair delete path has drifted and needs re-checking regardless.
Fix: the DeleteKeyPair handler and the key-pair state it reads — once creation stores the key pair, delete must remove it and return the 200 `operation` record (and append that record to the operation log); the not-found path must only fire for a name that genuinely does not exist.
Traces: lightsail-catalog-and-operations-lifecycle#teardown#8.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout.

Cause 1 — lightsail-catalog-and-operations-lifecycle#task#3, GetOperationsForResource, verdict mismatch, status 200 -> 200:
  $.operations[0]: missing. Expected {'createdAt': <per-run>, 'id': <per-run>, 'isTerminal': True, 'location': {'availabilityZone': 'all', 'regionName': 'us-west-2'}, 'operationType': 'CreateKeyPair', 'resourceName': <the key pair created earlier in the fixture>, 'resourceType': 'KeyPair', 'status': 'Succeeded', 'statusChangedAt': <per-run>}; got None (empty operations list).

Cause 2 — lightsail-catalog-and-operations-lifecycle#teardown#8, DeleteKeyPair, verdict mismatch, status 200 -> 400:
  status: expected 200, got 400.
  $.operation: missing. Expected {'createdAt': <per-run>, 'id': <per-run>, 'isTerminal': True, 'location': {'availabilityZone': 'all', 'regionName': 'us-west-2'}, 'operationDetails': '', 'operationType': 'DeleteKeyPair', 'resourceName': <the requested keyPairName>, 'resourceType': 'KeyPair', 'status': 'Succeeded', 'statusChangedAt': <per-run>}; got None.

(The key-pair name, operation id, region and timestamps above are per-run/request values; the 200 status and the operation record's operationType/resourceType/status/isTerminal/location shape are the requirement.)

## Steps to Reproduce
Boot the emulator and point a botocore lightsail client at its data listener.

1. ls.create_key_pair(keyPairName=N) for a fresh name N (today this is an unimplemented scaffold; the two steps below are what must then hold).
2. ls.get_operations_for_resource(resourceName=N) -> operations must contain a record with operationType 'CreateKeyPair', resourceType 'KeyPair', status 'Succeeded', isTerminal True and location.availabilityZone 'all'. Observed HTTP 200 with an empty operations list.
3. ls.delete_key_pair(keyPairName=N) -> HTTP 200 with operation.operationType 'DeleteKeyPair', operation.resourceName == N, status 'Succeeded', isTerminal True, operationDetails ''. Observed HTTP 400 with no operation member.

Re-replay: lightsail-catalog-and-operations-lifecycle#task#3, lightsail-catalog-and-operations-lifecycle#teardown#8.
