Issue-Source: trace-sweep-divergence
Issue-Trace: cloudformation-aorigin-physical-id-candidate-walk
Issue-Step: task#9
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudformation-aorigin-physical-id-candidate-walk
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: glue|GetTable|2xx->2xx|$.Table.Parameters
Issue-Key: glue|GetTable|2xx->2xx|$.Table.StorageDescriptor.Parameters
Issue-Key: glue|GetTable|2xx->2xx|$.Table.StorageDescriptor.SerdeInfo.Parameters
Issue-Commit: 2d97abf0bacad2832802b49de9c950da1a09c669

# Glue GetTable omits the empty Parameters maps AWS always returns

> _Note: this issue was identified against the code base at commit `2d97abf0bacad2832802b49de9c950da1a09c669`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTable

## Description
Cause 1 of 1 - GetTable omits the Parameters maps when a table declares none.
AWS: GetTable returns Table.Parameters, Table.StorageDescriptor.Parameters and Table.StorageDescriptor.SerdeInfo.Parameters as PRESENT-but-empty maps when the table, its storage descriptor and its serde carry no parameters. The members are modeled maps on Table/StorageDescriptor/SerdeInfo, and the recorded response shows all three serialized as empty objects.
Emulator: all three members are absent from the response.
Why: a client that reads table.Parameters without a presence check dereferences a missing key rather than an empty map, and code that merges parameter maps cannot distinguish 'no parameters' from 'field not returned'.
Fix: the GetTable response serializer (the Table / StorageDescriptor / SerdeInfo projection) must emit the empty maps rather than eliding them.
Traces: cloudformation-aorigin-physical-id-candidate-walk#task#9.

## Evidence
Replay of simparity-wire-aws-v1, fixture cloudformation-aorigin-physical-id-candidate-walk step task#9 (stub_path='', owned_by=[]). Status 200 both sides. Divergences: $.Table.Parameters missing (expected {}, got None); $.Table.StorageDescriptor.Parameters missing (expected {}, got None); $.Table.StorageDescriptor.SerdeInfo.Parameters missing (expected {}, got None).

## Steps to Reproduce
1. Replay the simparity-wire-aws-v1 corpus against the emulator built from this checkout.
2. Fixture cloudformation-aorigin-physical-id-candidate-walk, step task#9: call GetTable for a table created with no table/storage-descriptor/serde parameters.
3. Check that Table.Parameters, Table.StorageDescriptor.Parameters and Table.StorageDescriptor.SerdeInfo.Parameters are present as empty maps.
