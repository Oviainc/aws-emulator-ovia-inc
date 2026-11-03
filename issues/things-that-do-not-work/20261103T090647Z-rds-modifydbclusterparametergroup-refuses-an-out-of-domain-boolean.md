Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.modify-cluster-parameter-invalid-value-rejected
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds.modify-cluster-parameter-invalid-value-rejected|run#14|AssertionError|assert err.get("Message") == expected, f"expected message {expected!r}, got {err.get('Message')!r}"
Issue-Commit: f191aa5d5c1e52cec1d6b06ba3f355db374557d7

# rds: ModifyDBClusterParameterGroup refuses an out-of-domain boolean ParameterValue with a generic "Invalid parameter value: … allowed values are: …" message instead of AWS's "Invalid boolean value: <value>"

> _Note: this issue was identified against the code base at commit `f191aa5d5c1e52cec1d6b06ba3f355db374557d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyDBClusterParameterGroup

## Description
1 net-new root cause in rds. Cause 1 of 1: the InvalidParameterValue refusal ModifyDBClusterParameterGroup raises for a value outside a BOOLEAN parameter's domain carries the wrong message text.

AWS: Smithy (services/service-com-amazonaws-rds-amazonrdsv19/smithy-model/com-amazonaws-rds-amazonrdsv19.json) models ModifyDBClusterParameterGroup with input ModifyDBClusterParameterGroupMessage{DBClusterParameterGroupName required, Parameters: ParametersList required} and declares only two operation errors — DBParameterGroupNotFoundFault (awsQueryError DBParameterGroupNotFound, HTTP 404) and InvalidDBParameterGroupStateFault (HTTP 400); the refusal of a value outside a parameter's own domain is therefore the awsQuery common error InvalidParameterValue, HTTP 400, Type Sender. com.amazonaws.rds#Parameter models DataType ("Specifies the valid data type for the parameter") and AllowedValues ("Specifies the valid range of values for the parameter"), so the parameter's declared TYPE is what AWS words the refusal against. Recorded behavior only for the message: for a modifiable parameter whose DataType is `boolean` in an aurora-postgresql DBParameterGroupFamily, a ParameterValue that cannot be a boolean is refused with Code InvalidParameterValue and Message "Invalid boolean value: <ParameterValue>". Fixed text of that message: "Invalid boolean value: ". Its only variable part: the ParameterValue echoed verbatim from the request (in this run "sim2real-not-a-valid-value-xyz" — a per-run value that fills the template, not part of it). The message names neither the ParameterName nor the AllowedValues list.

Emulator: answers HTTP 400 with Code InvalidParameterValue (status and code are correct) but Message "Invalid parameter value: <ParameterValue> for: <ParameterName> allowed values are: <AllowedValues>" — observed as 'Invalid parameter value: sim2real-not-a-valid-value-xyz for: rds.force_ssl allowed values are: 0,1'. It words every parameter's domain refusal from the generic AllowedValues catalog entry rather than from the parameter's DataType, so a boolean parameter gets the allowed-values form AWS does not use here.

Why: an error message is part of the contract. A client, CLI user or negative test that asserts on, logs or parses the refusal text for a boolean engine parameter sees a different sentence than AWS produces: assertions on "Invalid boolean value: " fail, and anything scraping the message for the rejected value finds it followed by parameter-catalog prose AWS never emits. The divergence is in the text only — the operation is correctly refused and (per the same check) no override is written — so it is silent until a consumer reads the message.

Fix: the shared parameter-domain validator the handler calls — `crate::state::validate_cluster_parameter_setting` in services/service-com-amazonaws-rds-amazonrdsv19/src/state.rs, invoked from services/service-com-amazonaws-rds-amazonrdsv19/src/ops/modify_db_cluster_parameter_group.rs line 226 and mapped through `ModifyDBClusterParameterGroupError::invalid_parameter_value` — must word the refusal per the parameter's DataType: when the catalog entry's DataType is `boolean`, the message is "Invalid boolean value: " followed by the rejected ParameterValue, with no ParameterName and no allowed-values clause; the generic allowed-values wording stays for the non-boolean data types it was written for.

Related already-filed issues, neither of which covers this cause: open 20261102T055331Z-rds-2-net-new-root-causes-modifydbclusterparametergroup-accepts.md lists this same check in its trace ids, but its Cause 1 is the operation ACCEPTING the out-of-domain value with HTTP 200 and storing it — the emulator now refuses it, so that key no longer reproduces and the suite did not mark this failure as owned; fixed 20261029T181536Z-neptune-modifydbclusterparametergroup-accepts-a-parameter-value-outside-the-parameter.md added domain validation for the neptune twin of this handler, and the validation it introduced is what now emits the diverging wording. This is the residual message defect left behind by that validation, not a re-refusal of a call AWS accepts (AWS refuses this request too), so it is filed as a net-new cause rather than a reopen.

Traces: rds.modify-cluster-parameter-invalid-value-rejected.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the emulator built from this checkout (77/530 checks passed; the emulator was serving — rds answered 309 checks, 70 passing).

Cause 1 — check rds.modify-cluster-parameter-invalid-value-rejected [mismatch]:
  The check discovers the current default aurora-postgresql DBParameterGroupFamily via DescribeDBEngineVersions(DefaultOnly=true), creates a DB cluster parameter group in that family, picks a parameter that is IsModifiable=true with DataType="boolean" and a short comma-separated AllowedValues (this run: rds.force_ssl, AllowedValues "0,1"), and calls ModifyDBClusterParameterGroup with ParameterValue="sim2real-not-a-valid-value-xyz", ApplyMethod=pending-reboot.
  Expected (AWS): ClientError, HTTP 400, Code "InvalidParameterValue", Message "Invalid boolean value: sim2real-not-a-valid-value-xyz".
  Actual (emulator): HTTP 400, Code "InvalidParameterValue" (correct), Message 'Invalid parameter value: sim2real-not-a-valid-value-xyz for: rds.force_ssl allowed values are: 0,1'.
  Assertion: check_rds_modify_cluster_parameter_invalid_value_rejected.py line 69 — AssertionError: expected message 'Invalid boolean value: sim2real-not-a-valid-value-xyz', got 'Invalid parameter value: sim2real-not-a-valid-value-xyz for: rds.force_ssl allowed values are: 0,1'.
  The parameter name, the allowed-values list and the bogus value above are this run's values; the requirement is the fixed text "Invalid boolean value: " followed by the ParameterValue taken from the request, for any boolean-DataType parameter of any family.
  The check never reached its later legs (effect-absent re-read, positive control with a valid value, delete-then-mutate not-found messages), so nothing is claimed about those here.

## Steps to Reproduce
1. git_clone the repo and build/boot the emulator from this checkout.
2. Replay the behavioral suite (simparity-behavioral-aws-v1) and re-run exactly the check in trace_ids: rds.modify-cluster-parameter-invalid-value-rejected.
3. By hand: `aws rds describe-db-engine-versions --engine aurora-postgresql --default-only` to get the DBParameterGroupFamily F; `aws rds create-db-cluster-parameter-group --db-cluster-parameter-group-name PG --db-parameter-group-family F --description d`; `aws rds describe-db-cluster-parameters --db-cluster-parameter-group-name PG` and pick a parameter P with IsModifiable=true and DataType=boolean.
4. `aws rds modify-db-cluster-parameter-group --db-cluster-parameter-group-name PG --parameters ParameterName=P,ParameterValue=sim2real-not-a-valid-value-xyz,ApplyMethod=pending-reboot`.
5. Observe the refusal: the emulator answers 400 InvalidParameterValue with message "Invalid parameter value: sim2real-not-a-valid-value-xyz for: P allowed values are: <AllowedValues>"; AWS answers 400 InvalidParameterValue with message "Invalid boolean value: sim2real-not-a-valid-value-xyz".
