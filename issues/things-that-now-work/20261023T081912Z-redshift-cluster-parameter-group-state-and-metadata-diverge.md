Issue-Source: trace-sweep-divergence
Issue-Trace: redshift-parameter-group-lifecycle
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: redshift-parameter-group-lifecycle
Issue-Shape-Id: com.amazonaws.redshift#RedshiftServiceVersion20121201
Issue-Key: redshift|DescribeClusterParameters|2xx->2xx|$.DescribeClusterParametersResponse.DescribeClusterParametersResult.Parameters
Issue-Key: redshift|DescribeClusterParameters|2xx->2xx|$.DescribeClusterParametersResponse.DescribeClusterParametersResult.Parameters.Parameter[*].ApplyType
Issue-Key: redshift|DescribeClusterParameters|2xx->2xx|$.DescribeClusterParametersResponse.DescribeClusterParametersResult.Parameters.Parameter[*].Description
Issue-Key: redshift|ModifyClusterParameterGroup|2xx->2xx|$.ModifyClusterParameterGroupResponse.ModifyClusterParameterGroupResult.ParameterGroupStatus
Issue-Commit: 0dfa9e823e78890cf2fe273b1e49d27d43c189eb

# Redshift cluster parameter group state and metadata diverge from AWS

> _Note: this issue was identified against the code base at commit `0dfa9e823e78890cf2fe273b1e49d27d43c189eb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeClusterParameters

## Description
Three causes in the cluster-parameter-group subsystem: a reset does not take effect on the stored parameters, and the parameter metadata the emulator serves does not match AWS's engine defaults.

Cause 1 of 3 — a parameter-group reset leaves the user-set parameters in place (fixture redshift-parameter-group-lifecycle, step task#6).
AWS: Smithy: ResetClusterParameterGroup takes ParameterGroupName (required), ResetAllParameters (Boolean) and Parameters, and returns ClusterParameterGroupNameMessage. DescribeClusterParameters returns ClusterParameterGroupDetails.Parameters (ParametersList of Parameter), and each Parameter carries Source, documented as "The source of the parameter value, such as 'engine-default' or 'user'". After the reset performed earlier in this fixture, the recording shows DescribeClusterParametersResult.Parameters as empty for this query — the user-sourced overrides are gone.
Emulator: status matches (200) but Parameters still contains the two user overrides set before the reset — require_ssl with ParameterValue 'true' / Source 'user' and statement_timeout with ParameterValue '60000' / Source 'user' — a type_mismatch against the expected empty element.
Why: the reset is not observable, so a client cannot roll a parameter group back to engine defaults. Configuration-drift tooling that resets a group and then verifies by describing it concludes the reset failed; worse, a client that trusts the reset proceeds believing the cluster is back on defaults while the emulator still reports (and would apply) the overrides.
Fix: ResetClusterParameterGroup's state mutation in the redshift service crate — the reset must clear the user-sourced parameter values from the stored group (all of them for ResetAllParameters, or the named subset), so that the subsequent DescribeClusterParameters reflects it.

Cause 2 of 3 — engine-default parameter metadata is wrong (fixture redshift-parameter-group-lifecycle, step task#4).
AWS: Smithy: Parameter models Description (String), ApplyType (ParameterApplyType enum, documented as whether the parameter applies dynamically or requires a reboot), Source, DataType, AllowedValues, IsModifiable, MinimumEngineVersion. Recorded behavior: for the default parameter group family, require_ssl's Description is "require ssl for all databaseconnections", and the second parameter (statement_timeout) has ApplyType 'static' and Description "Aborts any statement that takes over the specified number of milliseconds.".
Emulator: require_ssl Description is "Requires SSL for every connection to the database."; statement_timeout ApplyType is 'dynamic' (expected 'static') and its Description is "Aborts any statement that takes over the specified number of milliseconds; 0 turns the limit off.". These are response-body member values, not error messages.
Why: ApplyType is the member a client reads to decide whether a parameter change needs a cluster reboot. Reporting 'dynamic' where AWS reports 'static' tells the client the change is already live, so it skips the RebootCluster that AWS requires and the setting silently never applies. The Description differences are catalog-fidelity divergences in a modeled member that console-like clients render verbatim.
Fix: the engine-default parameter catalog backing DescribeClusterParameters — align each parameter's ApplyType, Description and related metadata with the recorded engine defaults for the parameter group family.

Cause 3 of 3 — ModifyClusterParameterGroup returns the wrong ParameterGroupStatus (fixture redshift-parameter-group-lifecycle, step task#3).
AWS: Smithy: ModifyClusterParameterGroup returns ClusterParameterGroupNameMessage{ParameterGroupName, ParameterGroupStatus}. Recorded behavior only — the model does not constrain the text. AWS returns the status sentence that distinguishes the two outcomes: dynamic-only changes are being applied to associated clusters now, while static changes wait for a reboot ("Your parameter group has been updated. If you changed only dynamic parameters, associated clusters are being modified now. If you changed static parameters, all updates, including dynamic parameters, will be applied when you reboot the associated clusters.").
Emulator: returns a shorter, unconditional sentence stating changes will not apply until the associated clusters are rebooted ("Your parameter group has been updated but changes won't get applied until you reboot the associated Clusters.").
Why: ParameterGroupStatus is a modeled response member, not an error message, and it is the only thing this operation tells the caller about when the change takes effect. The emulator's text asserts a reboot is always required, which contradicts AWS for a dynamic-only modify — and it is also inconsistent with cause 2, where the emulator calls statement_timeout dynamic. A client that parses or displays this member gets the wrong answer about whether a reboot is pending. Hypothesis: the handler returns one hard-coded string rather than the status AWS reports for this operation.
Fix: the ModifyClusterParameterGroup handler — return the ParameterGroupStatus AWS returns for a parameter-group modify, consistent with the ApplyType the parameter catalog reports.

Traces: redshift-parameter-group-lifecycle#task#6 (cause 1), redshift-parameter-group-lifecycle#task#4 (cause 2), redshift-parameter-group-lifecycle#task#3 (cause 3). All three must be closed before submitting. The 404/already-exists steps of this same fixture (task#8, task#9, teardown#12) are separate defects filed in the RequestId and existence-precondition issues.

## Evidence
Cause 1 — redshift-parameter-group-lifecycle#task#6, DescribeClusterParameters, expected 200 / actual 200. Replay detail: "$.DescribeClusterParametersResponse.DescribeClusterParametersResult.Parameters: type_mismatch (expected '', got {'Parameter': [{'ParameterName': 'require_ssl', 'ParameterValue': 'true', ..., 'Source': 'user', ...}, {'ParameterName': 'statement_timeout', 'ParameterValue': '60000', ..., 'Source': 'user', ...}]})" — i.e. the user-sourced overrides survive the reset.

Cause 2 — redshift-parameter-group-lifecycle#task#4, DescribeClusterParameters, expected 200 / actual 200. Replay detail: "Parameters.Parameter[0].Description: value_mismatch (expected 'require ssl for all databaseconnections', got 'Requires SSL for every connection to the database.')"; "Parameters.Parameter[1].ApplyType: value_mismatch (expected 'static', got 'dynamic')"; "Parameters.Parameter[1].Description: value_mismatch (expected 'Aborts any statement that takes over the specified number of milliseconds.', got 'Aborts any statement that takes over the specified number of milliseconds; 0 turns the limit off.')".

Cause 3 — redshift-parameter-group-lifecycle#task#3, ModifyClusterParameterGroup, expected 200 / actual 200. Replay detail: "$.ModifyClusterParameterGroupResponse.ModifyClusterParameterGroupResult.ParameterGroupStatus: value_mismatch (expected 'Your parameter group has been updated. If you changed only dynamic parameters, associated clusters are being modified now. If you changed static parameters, all updates, including dynamic parameters, will be applied when you reboot the associated clusters.', got \"Your parameter group has been updated but changes won't get applied until you reboot the associated Clusters.\")".

Model citations: Smithy ResetClusterParameterGroup input {ParameterGroupName required, ResetAllParameters, Parameters} / output ClusterParameterGroupNameMessage; DescribeClusterParameters output ClusterParameterGroupDetails.Parameters: ParametersList; Parameter members Description, Source ("engine-default" or "user"), ApplyType (ParameterApplyType enum, dynamic vs reboot-required), DataType, AllowedValues, IsModifiable, MinimumEngineVersion; ModifyClusterParameterGroup output ClusterParameterGroupNameMessage{ParameterGroupName, ParameterGroupStatus}.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1 (replay_wire_corpus).
2. Replay fixture redshift-parameter-group-lifecycle in order: CreateClusterParameterGroup, then ModifyClusterParameterGroup setting require_ssl=true and statement_timeout=60000.
3. Cause 3: at step task#3 read ModifyClusterParameterGroupResult.ParameterGroupStatus — observe the unconditional reboot sentence instead of the recorded dynamic/static sentence.
4. Cause 2: at step task#4 call DescribeClusterParameters for the group — compare Parameter[0].Description, Parameter[1].ApplyType ('dynamic' vs recorded 'static') and Parameter[1].Description against the recording.
5. Cause 1: continue to the reset and then step task#6's DescribeClusterParameters — expect no parameters, observe the two Source='user' overrides still present.
