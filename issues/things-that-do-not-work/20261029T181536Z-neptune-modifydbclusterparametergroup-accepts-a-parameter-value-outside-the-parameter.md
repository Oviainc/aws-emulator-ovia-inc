Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: neptune.cluster-parameter-group-rejects-out-of-range-value
Issue-Shape-Id: com.amazonaws.neptune#AmazonRDSv19
Issue-Key: neptune.cluster-parameter-group-rejects-out-of-range-value|_error#7|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Commit: d9bf726e3fb20c68357b67dbe525252e1e62354e

# ModifyDBClusterParameterGroup accepts a parameter value outside the parameter's documented AllowedValues range instead of refusing it with InvalidParameterValue

> _Note: this issue was identified against the code base at commit `d9bf726e3fb20c68357b67dbe525252e1e62354e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyDBClusterParameterGroup

## Description
ModifyDBClusterParameterGroup performs no range/AllowedValues validation on a supplied ParameterValue: a value outside the parameter's published valid range is accepted and (for an in-range value) stored, where AWS refuses the whole modify. THIS ISSUE HAS 1 CAUSE.

NOTE: this is NOT the cause already filed as 20261029T174713Z-neptune-parameter-group-and-event-subscription-handlers-skip.md. That issue's Cause 1 is about an *unknown ParameterName* (name-existence check, message 'Could not find parameter with name: <name>'). This one is about a *known, modifiable* parameter whose VALUE falls outside the range the parameter catalog publishes in Parameter.AllowedValues — a separate validation step in the same handler, with a different error message template. Fixing name-existence alone leaves this check failing.

Cause 1 of 1: a value outside a parameter's AllowedValues range is accepted.
AWS: Smithy ModifyDBClusterParameterGroup (com.amazonaws.neptune#AmazonRDSv19) takes DBClusterParameterGroupName (required) and Parameters (required, ParametersList of Parameter); Parameter.AllowedValues is documented as "Specifies the valid range of values for the parameter", and Parameter.IsModifiable says whether it may be changed at all. The modelled error list for the operation is DBParameterGroupNotFoundFault (404) and InvalidDBParameterGroupStateFault (400) only — the value-validation refusal is a service-common query error, so this is recorded behavior for that class: AWS answers HTTP 400 with Error.Type 'Sender', Error.Code 'InvalidParameterValue' and the message "Value: <value> is outside of range: <allowed range> for parameter: <parameter name>". In that template the fixed text is "Value: ", " is outside of range: ", " for parameter: "; the variable parts are the rejected ParameterValue from the request, the range taken from the parameter catalog's AllowedValues for that parameter in that DBParameterGroupFamily, and the ParameterName from the request. For family neptune1.4 / parameter neptune_streams_expiry_days the catalog range is 1-90, so a request carrying ParameterValue '91' is refused with Message "Value: 91 is outside of range: 1-90 for parameter: neptune_streams_expiry_days". The refusal is atomic: none of the Parameters in the rejected request take effect, so DescribeDBClusterParameters with Source='user' still reports no user override for that parameter afterwards.
Emulator: the call succeeds. HTTP 200 with a DBClusterParameterGroupNameMessage body echoing DBClusterParameterGroupName, no Error element at all. The out-of-range value is not refused, and (per the check's intent) is taken as a user override.
Why: a client that sends a mistyped or out-of-bounds tuning value is told the change succeeded. Against real AWS the call fails loudly and the operator fixes the value; against the emulator the group silently carries a value the engine would never accept, so configuration drift is invisible and any test that asserts the guardrail (negative-path tests, policy checks, IaC validation) passes against the emulator and fails in production.
Fix: the ModifyDBClusterParameterGroup handler's parameter validation, in the Neptune crate's handler for that operation. It must, for each supplied Parameter, look the ParameterName up in the group's DBParameterGroupFamily catalog (the same catalog DescribeDBClusterParameters/DescribeDBParameters serve, which already carries AllowedValues, DataType and IsModifiable) and reject the request with the 400 InvalidParameterValue above when the ParameterValue falls outside AllowedValues — before any parameter in the batch is applied, so the modify stays atomic.
Traces: neptune.cluster-parameter-group-rejects-out-of-range-value.

## Evidence
Replay of simparity-behavioral-aws-v1, check neptune.cluster-parameter-group-rejects-out-of-range-value (verdict: mismatch):

Setup: CreateDBClusterParameterGroup(DBParameterGroupFamily='neptune1.4'); DescribeDBClusterParameters(Source='user') correctly reports no override for neptune_streams_expiry_days.

Failing call: ModifyDBClusterParameterGroup(DBClusterParameterGroupName=<group>, Parameters=[{ParameterName:'neptune_streams_expiry_days', ParameterValue:'91', ApplyMethod:'pending-reboot'}]).
  Expected: ClientError, HTTP 400, Error.Code 'InvalidParameterValue', Error.Message "Value: 91 is outside of range: 1-90 for parameter: neptune_streams_expiry_days" (91 and the parameter name come from the request; 1-90 is the catalog AllowedValues for that parameter in family neptune1.4).
  Actual: HTTP 200, body {'DBClusterParameterGroupName': '<group>'}, no error.
  Harness: AssertionError: expected the call to be refused, but it succeeded: {'DBClusterParameterGroupName': ..., 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}

(The check's follow-on assertions — that the rejected modify left DescribeDBClusterParameters Source='user' empty, and that an in-range value of '45' is then accepted and reported as the user value — were never reached because the refusal did not happen.)

## Steps to Reproduce
1. Boot the emulator and point a botocore 'neptune' client at it.
2. CreateDBClusterParameterGroup(DBClusterParameterGroupName='probe-cpg', DBParameterGroupFamily='neptune1.4', Description='probe').
3. DescribeDBClusterParameters(DBClusterParameterGroupName='probe-cpg', Source='user') — confirm no value for neptune_streams_expiry_days.
4. ModifyDBClusterParameterGroup(DBClusterParameterGroupName='probe-cpg', Parameters=[{'ParameterName':'neptune_streams_expiry_days','ParameterValue':'91','ApplyMethod':'pending-reboot'}]).
   Observed: HTTP 200. Expected: HTTP 400 InvalidParameterValue, Message "Value: 91 is outside of range: 1-90 for parameter: neptune_streams_expiry_days".
5. DescribeDBClusterParameters(..., Source='user') — must still report no user override for that parameter.
6. ModifyDBClusterParameterGroup with ParameterValue='45' must succeed and that Describe must then report '45'.
Or replay the behavioral check directly: neptune.cluster-parameter-group-rejects-out-of-range-value.
