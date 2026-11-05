Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ssm.parameter-ssm-list-resolved-value-until-update
Issue-Key: cloudformation__ssm.parameter-ssm-list-resolved-value-until-update|run#11|AssertionError|assert params == {"Zones": (name, "alpha,beta,gamma"), "Extra": ("x,y,z", None)}, f"Parameters: {params!r}"
Issue-Commit: 8de6682f074d8bd25eca3d1022f82b7ad69da0d3

# cloudformation__ssm: a CommaDelimitedList parameter value is echoed verbatim instead of whitespace-normalized, so DescribeStacks reports the untrimmed string

> _Note: this issue was identified against the code base at commit `8de6682f074d8bd25eca3d1022f82b7ad69da0d3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStacks

## Description
One cause.

Cause 1 of 1: a CommaDelimitedList stack parameter keeps the caller's (or the template default's) surrounding whitespace in the value CloudFormation stores and reports.

AWS: CloudFormation treats a `CommaDelimitedList` parameter as an array of strings split on commas, and it strips the whitespace surrounding each element before the value is stored on the stack. A subsequent DescribeStacks therefore reports the NORMALIZED, re-joined string in `Stacks[].Parameters[].ParameterValue` (Smithy: cloudformation#Parameter member ParameterValue — "The input value associated with the parameter"; ResolvedValue is "Read-only ... returned only for Systems Manager parameter types"). The Smithy model states no normalization rule, so the normalization itself is recorded behavior only: for a template parameter `{"Type": "CommaDelimitedList", "Default": "<a>, <b> ,<c>"}` the recording shows ParameterValue `"<a>,<b>,<c>"` with no ResolvedValue member, and the per-element trimming is what makes `Fn::Select [2, {"Ref": <param>}]` resolve to the bare third element. The general rule is the requirement: each comma-separated element is trimmed of leading and trailing whitespace, and the reported ParameterValue is those trimmed elements re-joined with a single comma and no spaces. The values in any one recording only fill that template.

Emulator: the stack's Parameters entry for the CommaDelimitedList parameter carries the raw, untrimmed literal exactly as it appeared in the template default — the replay shows `'Extra': ('x, y ,z', None)` where AWS reports `('x,y,z', None)`. The sibling `AWS::SSM::Parameter::Value<List<String>>` parameter in the same stack resolved correctly (ParameterValue = the SSM name, ResolvedValue = the comma-joined StringList), so only the CommaDelimitedList normalization is missing.

Why: a client that reads back its own stack parameters — a drift check, a redeploy that re-submits DescribeStacks values, or any code that splits ParameterValue on "," to recover the list — gets elements with embedded spaces (" y ") that AWS would never hand it, so comparisons against the intended list fail and downstream Fn::Select/Fn::Join-style consumers see padded values. The same untrimmed string is then what UsePreviousValue carries into the next update.

Fix: the CloudFormation stack-parameter resolution component — where a template/declared parameter value is coerced per its declared Type before being stored on the stack (the same place that already produces ResolvedValue for the SSM parameter types) must trim each element of a CommaDelimitedList and store the re-joined form; DescribeStacks then serializes the stored value unchanged.

Traces: cloudformation__ssm.parameter-ssm-list-resolved-value-until-update.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator built from this checkout; the check is listed under '### cloudformation__ssm' in the suite report.

[mismatch] cloudformation__ssm.parameter-ssm-list-resolved-value-until-update
check_cloudformation__ssm_parameter_ssm_list_resolved_value_until_update.py line 107:
  assert params == {"Zones": (name, "alpha,beta,gamma"), "Extra": ("x,y,z", None)}
AssertionError: Parameters: {'Extra': ('x, y ,z', None), 'Zones': ('/sim2real-behav-ssmlist-<id>', 'alpha,beta,gamma')}

Expected (AWS): the CommaDelimitedList parameter reports ParameterValue with every element trimmed and re-joined — for the template default "x, y ,z" that is "x,y,z" — and no ResolvedValue.
Actual (emulator): ParameterValue 'x, y ,z', the raw template default.

The stack reached CREATE_COMPLETE and the Value<List<String>> parameter resolved ('alpha,beta,gamma'), so the failure is isolated to CommaDelimitedList value normalization.

Smithy model consulted: services/service-com-amazonaws-cloudformation-cloudformation/smithy-model/com-amazonaws-cloudformation-cloudformation.json (shape cloudformation#Parameter: members ParameterKey, ParameterValue, UsePreviousValue, ResolvedValue).

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. PutParameter a StringList SSM parameter, e.g. Name=/<unique>, Type=StringList, Value="alpha,beta,gamma".
3. CreateStack with a template declaring two parameters: Zones of Type "AWS::SSM::Parameter::Value<List<String>>" (passed the SSM parameter name) and Extra of Type "CommaDelimitedList" with Default "x, y ,z"; give it any resource (e.g. AWS::CloudFormation::WaitConditionHandle) and Outputs using Fn::Select/Fn::Join over both Refs.
4. Poll DescribeStacks by StackId until CREATE_COMPLETE, then read Stacks[0].Parameters.
5. Observe Extra's ParameterValue is 'x, y ,z'; AWS reports 'x,y,z' (each element trimmed, re-joined with single commas) and no ResolvedValue.
6. Re-replay check id cloudformation__ssm.parameter-ssm-list-resolved-value-until-update to confirm.
