Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.parameter-type-mismatch-rejected cloudformation.parameter-constraint-violations-rejected cloudformation.cdk-nested-stack-is-tracked-and-cascades-on-delete cloudformation.logstream-replacement-orphans-old-stream-in-former-group
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation.parameter-type-mismatch-rejected|run#8|AssertionError|assert message == expected, f"expected type-mismatch message {expected!r}, got {message!r}"
Issue-Key: cloudformation.parameter-constraint-violations-rejected|run#21|AssertionError|assert message == expected, f"{label}: expected message {expected!r}, got {message!r}"
Issue-Key: cloudformation.cdk-nested-stack-is-tracked-and-cascades-on-delete|run#13|KeyError|parent_outputs = {o["OutputKey"]: o["OutputValue"] for o in parent.get("Outputs", [])}
Issue-Key: cloudformation.logstream-replacement-orphans-old-stream-in-former-group|run#18|AssertionError|assert status == "UPDATE_COMPLETE", f"move update ended {status!r}, expected UPDATE_COMPLETE"
Issue-Commit: 18c9e77d26f229766ffd8c87453fda7296284d7c

# cloudformation: 3 net-new causes — the Number-parameter typing refusal drops AWS's trailing period, a CDK-deployed parent stack's Outputs carry no OutputValue, and replacing an AWS::Logs::LogStream rolls the update back

> _Note: this issue was identified against the code base at commit `18c9e77d26f229766ffd8c87453fda7296284d7c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Three distinct net-new root causes of service `cloudformation`, each grounded in failing behavioral checks the replay report left unattributed (no ALREADY FILED, no OWNED by). A fixer must close all THREE before submitting.

Cause 1 of 3: the Number-parameter type-mismatch refusal omits the period AWS ends the sentence with.
AWS: recorded behavior only — parameter typing is not in the Smithy model; the refusal class is the service-common ValidationError (HTTP 400) on CreateStack/UpdateStack. A non-numeric ParameterValue for a parameter declared Type: Number is refused with Message "Parameter '<ParameterKey>' must be a number." — the fixed text is "Parameter '" + "' must be a number." INCLUDING the terminating period, and the only variable part is the parameter key taken from the request. Its two siblings deliberately carry NO final period: "Parameter '<key>' must be a number not less than <MinValue>" and "Parameter '<key>' must be a number not greater than <MaxValue>" — the emulator already matches those two, which pins the divergence to the typing message alone.
Emulator: correct status (400) and code (ValidationError), and no stack is left behind, but the message is "Parameter 'Count' must be a number" / "Parameter 'Retention' must be a number" — the final period is missing.
Why: this sentence is the only machine-readable discriminator between 'not a number at all' and 'a number out of range'; a client or golden-output comparison matching AWS's documented text never matches, even though the request was correctly refused.
Fix: the message template for the Number type-coercion failure in the parameter-validation stage of the CreateStack/UpdateStack handlers. Named but NOT covering: the resolved issue 20261102T101202Z-cloudformation-10-net-new-root-causes-stack-level.md implemented the parameter-constraint enforcement itself (its Causes 2 and 3) — enforcement now happens and the range messages are right; what remains is this one message's wording, and neither of these two replay keys is carried by that issue.
Traces: cloudformation.parameter-type-mismatch-rejected, cloudformation.parameter-constraint-violations-rejected.

Cause 2 of 3: DescribeStacks returns a CDK-deployed parent stack's Outputs entries with no OutputValue member.
AWS: Smithy com.amazonaws.cloudformation#Output declares OutputKey / OutputValue / Description / ExportName, and DescribeStacks on a stack that reached CREATE_COMPLETE returns every Output with its RESOLVED OutputValue — here a parent stack whose output is fed from the output of its nested AWS::CloudFormation::Stack child.
Emulator: the Output entry is present but the value member is absent, so every consumer raises KeyError: 'OutputValue'.
Why: the deploy-then-read pattern is dead — a client cannot take a name/id/url out of a deployed stack and drive the next call with it, and nested-stack output propagation cannot be verified at all.
Fix: the DescribeStacks serializer and stack-output resolution in CloudFormation state, including propagation of a nested child stack's outputs up to the parent's Outputs.
Traces: cloudformation.cdk-nested-stack-is-tracked-and-cascades-on-delete. Named but NOT covering: open issue 20261107T085452Z-cloudformation-8-net-new-behavioral-causes-stack-outputs.md describes the same symptom as its Cause 1, but its Issue-Trace-Ids carry cloudformation.nested-stack-parentage-and-cascade-delete, NOT this check id, so this replay key is not covered by it and must be re-replayed here; open issue 20261022T235108Z-cloudformation-stack-outputs-carry-no-outputvalue-failing-resources.md names the same family and likewise does not carry this key.

Cause 3 of 3: an UpdateStack that replaces a named AWS::Logs::LogStream by changing its LogGroupName ends UPDATE_ROLLBACK_COMPLETE instead of UPDATE_COMPLETE.
AWS: recorded behavior only — AWS::Logs::LogStream's LogGroupName is a replacement property: an UsePreviousTemplate update that only changes the parameter feeding LogGroupName creates the stream in the new group, emits the old stream's DELETE_COMPLETE event, and the stack reaches UPDATE_COMPLETE (the old stream then survives, readable with its content, in the former group).
Emulator: the update terminates in UPDATE_ROLLBACK_COMPLETE, so the replacement never lands and none of the post-replacement contract (new physical id, old stream orphaned, DeleteStack removing only the tracked stream) is reachable.
Why: any stack that owns a LogStream becomes un-updatable; a caller that moves a stream between groups is bounced back to the previous state with no successful path.
Fix: the AWS::Logs::LogStream resource provisioner's replacement path in the cloudformation crate (create the new stream in the new group, then delete the old one) and the update engine's handling of its replacement verdict.
Traces: cloudformation.logstream-replacement-orphans-old-stream-in-former-group.

## Evidence
Cause 1 — [mismatch] cloudformation.parameter-type-mismatch-rejected: assert message == expected -> AssertionError: expected type-mismatch message "Parameter 'Count' must be a number.", got "Parameter 'Count' must be a number" (code ValidationError matched; template declared Parameters.Count Type: Number, ParameterValue 'not-a-number').
Cause 1 (second grounding) — [mismatch] cloudformation.parameter-constraint-violations-rejected: AssertionError: Number typing: expected message "Parameter 'Retention' must be a number.", got "Parameter 'Retention' must be a number". The preceding cases in the same check PASSED, including 'Parameter 'Retention' must be a number not less than 1' and '... not greater than 30' and the AllowedPattern/ConstraintDescription cases — one shared message template, two checks. Parameter names above are request-supplied fillers, not requirements.
Cause 2 — [error] cloudformation.cdk-nested-stack-is-tracked-and-cascades-on-delete: KeyError: 'OutputValue' at `parent_outputs = {o["OutputKey"]: o["OutputValue"] for o in parent.get("Outputs", [])}`; the assertions before it passed (parent StackStatus == CREATE_COMPLETE, exactly one AWS::CloudFormation::Stack resource with a PhysicalResourceId), so the stack deployed and only the output value is missing.
Cause 3 — [mismatch] cloudformation.logstream-replacement-orphans-old-stream-in-former-group: AssertionError: move update ended 'UPDATE_ROLLBACK_COMPLETE', expected UPDATE_COMPLETE. The create arm passed: CREATE_COMPLETE, the stream's PhysicalResourceId == 'application', both the stream and the unrelated sibling present in the first group, and the stream writable.

## Steps to Reproduce
Replay the suite and re-run exactly the check ids in trace_ids. Minimal repros:
1. Cause 1: CreateStack over a template declaring {"Parameters": {"Count": {"Type": "Number"}}} with ParameterValue 'not-a-number'. Expect 400 ValidationError with Message "Parameter 'Count' must be a number."; the emulator drops the final period. Repeat with a Number parameter carrying MinValue/MaxValue and values 0 / 99 to see that those two messages already match.
2. Cause 2: `cdk deploy` (or CreateStack) a parent template containing a nested AWS::CloudFormation::Stack whose output feeds a parent Output, wait for CREATE_COMPLETE, then DescribeStacks the parent and read Stacks[0].Outputs[*].OutputValue — the member is absent.
3. Cause 3: create two log groups, CreateStack a template with one AWS::Logs::LogStream whose LogStreamName is fixed and whose LogGroupName is {"Ref": "GroupName"} (parameter = first group); after CREATE_COMPLETE, UpdateStack with UsePreviousTemplate=True and GroupName = the second group. Expect UPDATE_COMPLETE plus a Stream DELETE_COMPLETE event; the emulator ends UPDATE_ROLLBACK_COMPLETE.
