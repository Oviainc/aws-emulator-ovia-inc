Issue-Source: trace-sweep-divergence
Issue-Trace: workspaces-connection-alias-lifecycle
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: workspaces-connection-alias-lifecycle
Issue-Shape-Id: com.amazonaws.workspaces#WorkspacesService
Issue-Key: workspaces|DescribeConnectionAliases|2xx->2xx|$.ConnectionAliases[*]
Issue-Commit: 9aaeda1848c43b0bda5e2e66a82e857bd94691b3

# DescribeConnectionAliases reports a stuck CREATING state and omits Associations

> _Note: this issue was identified against the code base at commit `9aaeda1848c43b0bda5e2e66a82e857bd94691b3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeConnectionAliases

## Description
Cause 1 of 1: after CreateConnectionAlias, the emulator's DescribeConnectionAliases describes the alias as still CREATING and with no Associations member, so the alias never becomes usable or inspectable.
AWS: Smithy - CreateConnectionAlias(ConnectionString @required) returns AliasId (@pattern ^wsca-[0-9a-z]{8,63}$); DescribeConnectionAliases returns ConnectionAliases of ConnectionAlias{ConnectionString, AliasId, State: ConnectionAliasState enum [CREATING, CREATED, DELETING], OwnerAccountId, Associations: ConnectionAliasAssociationList of ConnectionAliasAssociation{AssociationStatus, AssociatedAccountId, ResourceId, ConnectionIdentifier}}. Recorded behavior: the alias created in the previous step is reported with State 'CREATED' and an Associations list carrying one entry whose AssociationStatus is 'NOT_ASSOCIATED' (the alias exists but is not yet associated with a directory). ConnectionString, AliasId and OwnerAccountId already match.
Emulator: the same alias comes back with State 'CREATING' and no Associations member at all (expected {'AliasId': 'wsca-00000001', 'Associations': [{'AssociationStatus': 'NOT_ASSOCIATED'}], 'ConnectionString': ..., 'OwnerAccountId': ..., 'State': 'CREATED'}, got the same identity fields with State 'CREATING' and Associations absent). Hypothesis: the alias is stored in its initial creation state and nothing advances it, and the association list is never materialized for an unassociated alias.
Why: cross-Region-redirection workflows poll DescribeConnectionAliases until State is CREATED before associating the alias; against the emulator that poll never terminates, so the client either hangs or times out. Code that reads Associations to decide whether the alias is free gets a missing field instead of a NOT_ASSOCIATED entry and must special-case a shape AWS does not produce.
Fix: the connection-alias state in the workspaces service - CreateConnectionAlias must settle the alias at State CREATED (so the describe read-back reports CREATED, with DELETING reserved for an in-flight delete), and the DescribeConnectionAliases serializer must emit the Associations list, with AssociationStatus NOT_ASSOCIATED while the alias is associated with no directory.
Traces: workspaces-connection-alias-lifecycle#task#2 (DescribeConnectionAliases, status 200 vs 200, body mismatch).

## Evidence
Replay of trace set simparity-wire-aws-v1, service workspaces, operation DescribeConnectionAliases, expected 200 / actual 200:
- workspaces-connection-alias-lifecycle#task#2: $.ConnectionAliases[0] missing/mismatched - expected {'AliasId': 'wsca-00000001', 'Associations': [{'AssociationStatus': 'NOT_ASSOCIATED'}], 'ConnectionString': 'sim2real-probe-workspaces-alias-f0a25bad.example.com', 'OwnerAccountId': '000000000000', 'State': 'CREATED'}, got {'ConnectionString': 'sim2real-probe-workspaces-alias-f0a25bad.example.com', 'AliasId': 'wsca-00000001', 'State': 'CREATING', 'OwnerAccountId': '000000000000'}.
The two diverging facts are State (CREATING instead of CREATED) and the entirely absent Associations list; the generated alias id, connection string and account id are environment values and already agree.
Model grounding: ConnectionAliasState = [CREATING, CREATED, DELETING]; ConnectionAlias.Associations targets ConnectionAliasAssociationList, whose member documents AssociationStatus as 'The association status of the connection alias.'

## Steps to Reproduce
1. Replay fixture workspaces-connection-alias-lifecycle against the emulator (replay_wire_corpus).
2. CreateConnectionAlias with an FQDN connection string; note the returned AliasId.
3. Call DescribeConnectionAliases for that AliasId (step task#2).
4. The alias is returned with State 'CREATING' (recording: 'CREATED') and with no Associations member (recording: [{'AssociationStatus': 'NOT_ASSOCIATED'}]); repeating the describe never advances the state.
