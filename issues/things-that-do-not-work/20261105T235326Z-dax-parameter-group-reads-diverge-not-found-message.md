Issue-Source: trace-sweep-divergence
Issue-Trace: dax-error-paths
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dax-error-paths dax-parameter-group-lifecycle
Issue-Shape-Id: com.amazonaws.dax#AmazonDAXV3
Issue-Key: dax|DescribeParameters|4xx->4xx|$.message
Issue-Key: dax|DescribeParameters|2xx->2xx|$.Parameters[*]
Issue-Commit: 9327a6c75d367c434372287e7f386bd60fe36371

# dax parameter-group reads diverge: not-found message omits the requested group and DescribeParameters reports catalog defaults

> _Note: this issue was identified against the code base at commit `9327a6c75d367c434372287e7f386bd60fe36371`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeParameters

## Description
Two independent defects in the DAX parameter-group read path, both observed on DescribeParameters. THIS ISSUE HAS 2 CAUSES; both must be fixed before it is closed.

Cause 1 of 2: ParameterGroupNotFoundFault answers an emulator-authored, request-independent message instead of AWS's template that names the group asked for. (fixture dax-error-paths, step task#8; also seen at dax-parameter-group-lifecycle#teardown#9)
  AWS: Smithy: DescribeParameters takes ParameterGroupName (String, required) and models ParameterGroupNotFoundFault ("The specified parameter group does not exist.", httpError 404, awsQueryError code ParameterGroupNotFound). The model does not state the message text, so for the wording this is recorded behavior only: the recorded DAX response is HTTP 400 with __type "ParameterGroupNotFoundFault" and message "CacheParameterGroupnot found: <ParameterGroupName>". Fixed text of that template is "CacheParameterGroupnot found: " (AWS's own wording, with no space between "CacheParameterGroup" and "not"); the only variable part is the ParameterGroupName taken verbatim from the request. The probe name in the recording (a sim2real-probe-dax-missing-pg-* string) is the request value filling that slot, not part of the template.
  Emulator: same status (400) and same __type, but message "Parameter Group not found." — a fixed string that echoes nothing of the request. state.rs documents this deliberately: parameter_group_not_found_message() says "This fault has one template and it names no identifier: the requested name is NOT echoed back, on any path that raises it."
  Why: a client (or an operator reading logs) that parses or displays the fault cannot tell WHICH parameter group was missing, and any assertion on AWS's message text fails. Batch/driver code that matches the AWS wording treats the emulator's reply as an unrecognised error.
  Fix: the fault's message builder in the service's state module (the single site every raise path uses), which must take the requested ParameterGroupName as a parameter — the same way subnet_group_not_found(name) and cluster_not_found_named(name) already do in that file — and every raise site in the parameter-group operations must pass the requested name through.
  Traces: dax-error-paths#task#8 (DescribeParameters on a group that was never created); dax-parameter-group-lifecycle#teardown#9 (same fault after the group was deleted).

Cause 2 of 2: DescribeParameters reports the system-defined catalog default for every parameter instead of the value the group actually holds after a parameter update. (fixture dax-parameter-group-lifecycle, step task#7)
  AWS: Smithy: DescribeParameters -> Parameters (ParameterList) "A list of parameters within a parameter group"; UpdateParameterGroup takes ParameterGroupName + ParameterNameValues (both required) and returns the updated ParameterGroup. So a DescribeParameters issued after a successful UpdateParameterGroup must report, for each parameter named in that update, the VALUE THAT UPDATE SET, with the rest of the fields (ParameterName, ParameterType, Description, Source, DataType, AllowedValues, IsModifiable, ChangeType, NodeTypeSpecificValues) unchanged from the catalog entry. The recording confirms this: the group's two parameters query-ttl-millis and record-ttl-millis come back with the per-group values the run had set (recorded as 75000 and 88000 respectively), not the 300000 default.
  Emulator: HTTP 200 with both parameters reporting ParameterValue "300000", the catalog default from DEFAULT_PARAMETERS in the service's state module; every other member matches. So the updated value is never persisted into, or never read back out of, the group's stored parameter map.
  Why: a client that tunes query-ttl-millis / record-ttl-millis and then reads the group back sees its change silently discarded — a configuration write that reports success but has no observable effect. Any convergence loop or test that waits for the new TTL to be reported never converges.
  Fix: the parameter-group state (the per-group parameter value map that DescribeParameters projects through the group's parameter view) plus the handler that writes it. The write side is UpdateParameterGroup, which in this checkout is still an unimplemented scaffold returning 501 — open issue 20261105T204401Z-dax-updateparametergroup-stub.md asks for that operation to be implemented, and it does NOT cover this divergence: that issue is keyed on UpdateParameterGroup answering 501, while this one is the DescribeParameters read reporting defaults instead of the group's stored values, and it must be re-replayed on this fixture step to confirm the read path projects stored values rather than the catalog.
  Traces: dax-parameter-group-lifecycle#task#7 (DescribeParameters after the group's parameters were set).

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, 42 steps, 26 passed, 16 mismatched, score 61.9%).

Cause 1 — fixture dax-error-paths, step task#8, operation DescribeParameters, divergence_key service=dax|DescribeParameters|status=400->400|keys=$.message:
  expected status 400, actual 400 (match)
  $.__type: ParameterGroupNotFoundFault (match)
  $.message: value_mismatch — expected 'CacheParameterGroupnot found: sim2real-probe-dax-missing-pg-23bbaef0', got 'Parameter Group not found.'
  cluster also contains dax-parameter-group-lifecycle#teardown#9.
  Code: services/service-com-amazonaws-dax-amazondaxv3/src/state.rs:1021-1023 (parameter_group_not_found_message) and :1170-1174 (parameter_group_not_found); raise site services/service-com-amazonaws-dax-amazondaxv3/src/ops/describe_parameters.rs:199-204.

Cause 2 — fixture dax-parameter-group-lifecycle, step task#7, operation DescribeParameters, divergence_key service=dax|DescribeParameters|status=200->200|keys=$.Parameters[0],$.Parameters[1]:
  expected status 200, actual 200
  $.Parameters[0] query-ttl-millis: expected ParameterValue '75000', got '300000' (all other members equal)
  $.Parameters[1] record-ttl-millis: expected ParameterValue '88000', got '300000' (all other members equal)
  Code: services/service-com-amazonaws-dax-amazondaxv3/src/state.rs:726-748 (DEFAULT_PARAMETERS, both default_value "300000") and :766-778 (default_parameter_values); read path services/service-com-amazonaws-dax-amazondaxv3/src/ops/describe_parameters.rs:218-228 (group.parameter_view).

## Steps to Reproduce
Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus), then:

Cause 1: call DescribeParameters with a ParameterGroupName no parameter group exists for (as fixture dax-error-paths step task#8 does). Observe HTTP 400 with __type ParameterGroupNotFoundFault and message 'Parameter Group not found.' instead of AWS's 'CacheParameterGroupnot found: <the ParameterGroupName sent>'. Repeat after deleting an existing group (dax-parameter-group-lifecycle teardown#9) for the same wrong message.

Cause 2: create a parameter group, set its parameters query-ttl-millis and record-ttl-millis to values other than the 300000 default (UpdateParameterGroup with ParameterNameValues), then call DescribeParameters on that group (fixture dax-parameter-group-lifecycle step task#7). Observe HTTP 200 where both parameters report ParameterValue '300000' instead of the values just set.
