Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: redshift.parameter-group-non-boolean-value-rejected
Issue-Shape-Id: com.amazonaws.redshift#RedshiftServiceVersion20121201
Issue-Key: redshift.parameter-group-non-boolean-value-rejected|run#10|AssertionError|assert message == expected_message, f"expected the message {expected_message!r}, got {message!r}"
Issue-Commit: 45af2140f358870cdf193287cbc1a2501641d010

# ModifyClusterParameterGroup boolean-value refusal: InvalidParameterValue message is the emulator's wire-location template, not AWS's 'parameter value is not a valid boolean:<value>'

> _Note: this issue was identified against the code base at commit `45af2140f358870cdf193287cbc1a2501641d010`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyClusterParameterGroup

## Description
One net-new root cause in the redshift (query-protocol) control plane.

Cause 1 of 1: the message of the InvalidParameterValue refusal ModifyClusterParameterGroup returns when a boolean parameter is given a non-boolean value is authored by the emulator instead of reproducing AWS's wording for that error class.
Failing check: redshift.parameter-group-non-boolean-value-rejected

AWS: ModifyClusterParameterGroup with a boolean cluster parameter set to a value that is not a boolean (e.g. require_ssl = 'maybe') is refused with HTTP 400 and the service-common query-protocol error code InvalidParameterValue, and the message is `parameter value is not a valid boolean:<value>` — fixed text `parameter value is not a valid boolean:` (no space after the colon) plus ONE variable part, the rejected parameter value taken verbatim from the request. The message names neither the parameter nor the position of the member in the query serialization, and it does not enumerate the allowed values. Smithy (com.amazonaws.redshift#RedshiftServiceVersion20121201, ModifyClusterParameterGroup) models only ClusterParameterGroupNotFoundFault (awsQueryError ClusterParameterGroupNotFound, httpError 404) and InvalidClusterParameterGroupStateFault (awsQueryError InvalidClusterParameterGroupState, httpError 400), so the parameter-value check is the service-common InvalidParameterValue and this message pairing is recorded behavior only; the recording shows the code InvalidParameterValue with exactly the text above.
Emulator: returns the right code (InvalidParameterValue — the code assertion that the open issue 20261102T105531Z-redshift-refusal-codes-and-messages-are-emulator-authored.md recorded as failing now passes) but the message `Invalid value for Parameters.Parameter.<n>.ParameterValue: '<value>' is not a value parameter '<name>' allows. Allowed values: true,false` — entirely different fixed text, built from the query-serialization member path (`Parameters.Parameter.<n>.ParameterValue`), the parameter name and an allowed-value enumeration that AWS does not include in this refusal. (`<n>` is the member's index in the submitted Parameters list, `<value>`/`<name>` come from the request; none of these are part of AWS's template.)
Why: the message is a requirement of the refusal, not decoration. Clients that surface or match the refusal text — CLI/transcript snapshot tests, error-mapping layers that distinguish a type/parse failure from an allowed-value failure, log scrapers and IaC drift handlers — diverge from AWS, and the emulator's text additionally leaks the wire serialization index of the member rather than speaking in the vocabulary the operator used. Because the code is now correct, the divergence is silent to code that only branches on Error.Code and shows up only in the text, which is harder to notice.
Fix: redshift parameter-value validation for ModifyClusterParameterGroup and the error serializer that renders its InvalidParameterValue — the boolean-type check must render AWS's per-class template (`parameter value is not a valid boolean:` + rejected value) instead of the generic "Invalid value for <wire member path>: ... Allowed values: ..." formatter. This is the same formatter cause 4 of the open issue 20261102T105531Z-redshift-refusal-codes-and-messages-are-emulator-authored.md describes for the out-of-range max_concurrency_scaling_clusters path; that issue does not carry this check's current replay key (its key for this check is the already-satisfied code assertion `assert code == "InvalidParameterValue"`, and the suite did not mark this failure owned), so this message divergence is filed here and that issue is named for context. Note that the check's later assertions — that the whole batch is refused atomically (the valid auto_mv in the same Parameters list is not applied) and that a pre-existing require_ssl user override survives the refusal — were never reached, so they are unverified rather than passing.
Traces: redshift.parameter-group-non-boolean-value-rejected.

## Evidence
Cause 1 — redshift.parameter-group-non-boolean-value-rejected.
Setup (passed): CreateClusterParameterGroup(ParameterGroupFamily='redshift-1.0'); ModifyClusterParameterGroup(Parameters=[{ParameterName:'require_ssl', ParameterValue:'true'}]); DescribeClusterParameters(Source='user') == {require_ssl: 'true'}.
Failing call: ModifyClusterParameterGroup(Parameters=[{ParameterName:'auto_mv', ParameterValue:'false'}, {ParameterName:'require_ssl', ParameterValue:'maybe'}]).
Error.Code: 'InvalidParameterValue' — matches AWS.
Error.Message expected: 'parameter value is not a valid boolean:maybe'
Error.Message actual:   "Invalid value for Parameters.Parameter.2.ParameterValue: 'maybe' is not a value parameter 'require_ssl' allows. Allowed values: true,false"
Replay assertion: AssertionError: expected the message 'parameter value is not a valid boolean:maybe', got "Invalid value for Parameters.Parameter.2.ParameterValue: 'maybe' is not a value parameter 'require_ssl' allows. Allowed values: true,false" (check_redshift_parameter_group_non_boolean_value_rejected.py line 34).
Fixed text of the requirement: 'parameter value is not a valid boolean:'. Variable part: the rejected value from the request ('maybe' here). The '2' in the emulator's text is the member index in the submitted Parameters list, 'require_ssl' the parameter name, 'true,false' the catalogue's allowed values — all per-request/per-catalogue values, and none of them appear in AWS's message at all.

## Steps to Reproduce
1. redshift CreateClusterParameterGroup(ParameterGroupName=<new>, ParameterGroupFamily='redshift-1.0', Description=<any>).
2. ModifyClusterParameterGroup(ParameterGroupName=<same>, Parameters=[{ParameterName:'require_ssl', ParameterValue:'true'}]); confirm DescribeClusterParameters(ParameterGroupName=<same>, Source='user') returns {require_ssl: 'true'}.
3. ModifyClusterParameterGroup(ParameterGroupName=<same>, Parameters=[{ParameterName:'auto_mv', ParameterValue:'false'}, {ParameterName:'require_ssl', ParameterValue:'maybe'}]).
4. Read Error.Code (expect InvalidParameterValue, HTTP 400) and Error.Message (expect 'parameter value is not a valid boolean:maybe').
5. Re-read DescribeClusterParameters(Source='user'): it must still be exactly {require_ssl: 'true'} — the refused batch applies nothing and does not revert the existing override.
Replay: redshift.parameter-group-non-boolean-value-rejected.
