Issue-Source: trace-sweep-divergence
Issue-Trace: docdb-elastic-pending-maintenance-action-missing-and-create-cluster-invalid-shard-capacity
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: docdb-elastic-pending-maintenance-action-missing-and-create-cluster-invalid-shard-capacity
Issue-Shape-Id: com.amazonaws.docdbelastic#ChimeraDbLionfishServiceLambda
Issue-Key: docdb-elastic|CreateCluster|4xx->4xx|$.fieldList
Issue-Key: docdb-elastic|CreateCluster|4xx->4xx|$.message
Issue-Key: docdb-elastic|CreateCluster|4xx->4xx|$.reason
Issue-Commit: b1cd28f1ca277d6a032df605f83af5017e934043

# CreateCluster shardCapacity refusal uses generic constraint wording plus unmodeled reason and fieldList

> _Note: this issue was identified against the code base at commit `b1cd28f1ca277d6a032df605f83af5017e934043`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCluster

## Description
CreateCluster rejects an out-of-domain shardCapacity with the right status (400 ValidationException) but the wrong body: the message is the emulator's generic framework-style constraint sentence, and the response carries two members AWS leaves out of this refusal.

Cause 1 of 1: CreateCluster with shardCapacity outside the allowed set (fixture docdb-elastic-pending-maintenance-action-missing-and-create-cluster-invalid-shard-capacity, step task#2).
AWS: 400 ValidationException whose message is the service's own sentence for this field: fixed text "Invalid Input - '<shardCapacity>'. ShardCapacity must be a power of 2 within [2, 64]." — the only variable part is the rejected shardCapacity value taken verbatim from the request; the bound text "[2, 64]" and the "power of 2" phrasing are fixed. The recorded body carries message only: neither reason nor fieldList is present. (Smithy: CreateClusterInput.shardCapacity is a required Integer documented "Maximum is 64. Allowed values are 2, 4, 8, 16, 32, 64"; error ValidationException, httpError 400, members message/reason/fieldList. The recording is what pins the wording and the absence of reason/fieldList.)
Emulator: 400 whose message is the generic boundary sentence "1 validation error detected: Value at 'shardCapacity' failed to satisfy constraint: Member must be one of the allowed values [2, 4, 8, 16, 32, 64]", plus reason: 'fieldValidationFailed' and fieldList: [{name: 'shardCapacity', message: 'Member must be one of the allowed values [2, 4, 8, 16, 32, 64]'}].
Why: a client that surfaces or matches on the refusal text gets a sentence AWS never emits for this field, loses the "power of 2 within [2, 64]" rule AWS states, and sees structured members (reason, fieldList) on a refusal where AWS sends none — so code that branches on the presence of fieldList to decide "this is a per-field boundary failure" classifies this refusal differently than against AWS.
Fix: CreateCluster's shardCapacity validation — the domain check must be raised by the operation's own validator with the service-specific message, instead of falling through to the crate's generic enum/allowed-values boundary validator that also populates reason and fieldList.
Traces: docdb-elastic-pending-maintenance-action-missing-and-create-cluster-invalid-shard-capacity#task#2.

## Evidence
Replay of trace set simparity-wire-aws-v1, verdict mismatch, CreateCluster, fixture docdb-elastic-pending-maintenance-action-missing-and-create-cluster-invalid-shard-capacity step task#2: expected_status 400, actual_status 400.
- $.message: expected "Invalid Input - '3'. ShardCapacity must be a power of 2 within [2, 64].", got "1 validation error detected: Value at 'shardCapacity' failed to satisfy constraint: Member must be one of the allowed values [2, 4, 8, 16, 32, 64]". Fixed text: "Invalid Input - '" ... "'. ShardCapacity must be a power of 2 within [2, 64]."; variable part: the rejected shardCapacity from the request (3 in this recording, a per-request value, not part of the template).
- $.reason: type_mismatch (expected None, got 'fieldValidationFailed') — AWS omits the member on this refusal.
- $.fieldList: type_mismatch (expected None, got [{'name': 'shardCapacity', 'message': 'Member must be one of the allowed values [2, 4, 8, 16, 32, 64]'}]) — AWS omits the member on this refusal.

## Steps to Reproduce
1. Boot the emulator and target the DocDB Elastic (docdb-elastic) endpoint.
2. POST /cluster (CreateCluster) with a valid clusterName/authType/adminUserName/adminUserPassword/shardCount and shardCapacity set to a value outside {2,4,8,16,32,64} (e.g. 3).
3. Observe 400 with the generic "1 validation error detected: Value at 'shardCapacity' failed to satisfy constraint: ..." message plus reason='fieldValidationFailed' and a fieldList entry, instead of AWS's "Invalid Input - '3'. ShardCapacity must be a power of 2 within [2, 64]." with message as the only member.
4. Or replay fixture docdb-elastic-pending-maintenance-action-missing-and-create-cluster-invalid-shard-capacity (task#2).
