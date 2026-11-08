Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.get-stack-output-reads-unexported-output-as-snapshot
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation|CreateStack|ValidationError
Issue-Commit: 18c9e77d26f229766ffd8c87453fda7296284d7c

# REOPENS: CreateStack refuses Fn::GetStackOutput as an "unsupported function" while the same refusal lists it as supported

> _Note: this issue was identified against the code base at commit `18c9e77d26f229766ffd8c87453fda7296284d7c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
REOPENS 20261027T110526Z-cloudformation-5-net-new-behavioral-root-causes-circular.md: its fix left the call refused.

That issue's Cause 3 was the 'Encountered unsupported function' roster of CreateStack/UpdateStack/ValidateTemplate template validation. The roster constant is now AWS's exact list — and that list contains Fn::GetStackOutput — but the validator still refuses a template that uses Fn::GetStackOutput, so the one response the fix touched now names the same function as unsupported and supported in a single sentence.

AWS: recorded behavior only — template intrinsics are not in the Smithy model (CreateStack models AlreadyExistsException / InsufficientCapabilitiesException / LimitExceededException / TokenAlreadyExistsException plus the service-common ValidationError, HTTP 400). Fn::GetStackOutput is a supported intrinsic: it appears, first, in the fixed roster AWS itself quotes in the unsupported-function refusal, and a CreateStack over a template that uses it to read another stack's UNEXPORTED Output is ACCEPTED (HTTP 200 with a StackId); the function resolves to a snapshot of that output's value taken when the stack is created/updated.
Emulator: CreateStack is refused 400 ValidationError with Message "Template Error: Encountered unsupported function: Fn::GetStackOutput Supported functions are: [Fn::GetStackOutput, Fn::AccountIdFromAlias, Fn::GetConstantAZs, Fn::Base64, Fn::GetAtt, Fn::GetAZs, Fn::ImportValue, Fn::Join, Fn::Split, Fn::FindInMap, Fn::Select, Ref, Fn::Equals, Fn::If, Fn::Not, Condition, Fn::And, Fn::Or, Fn::Contains, Fn::EachMemberEquals, Fn::EachMemberIn, Fn::ValueOf, Fn::ValueOfAll, Fn::RefAll, Fn::Sub, Fn::Cidr]". In that message the fixed text is "Template Error: Encountered unsupported function: ", " Supported functions are: [", the roster and "]"; the only variable part is the function name taken from the request. The roster is now correct — the defect is that the function it advertises is not implemented, so the call is refused instead of creating the stack.
Why: a client cannot create any stack that reads another stack's unexported output — the whole cross-stack snapshot pattern is unavailable — and the refusal it does get is self-contradictory, so there is nothing for the caller to act on.
Fix: the template intrinsic resolver in the cloudformation crate must implement Fn::GetStackOutput (resolve <stack>.<OutputKey> against the named stack's Outputs at create/update time) on the path reached from CreateStack, UpdateStack and ValidateTemplate, instead of only listing it in the supported-function roster constant.
Traces: cloudformation.get-stack-output-reads-unexported-output-as-snapshot.

## Evidence
[error] cloudformation.get-stack-output-reads-unexported-output-as-snapshot
botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template Error: Encountered unsupported function: Fn::GetStackOutput Supported functions are: [Fn::GetStackOutput, Fn::AccountIdFromAlias, Fn::GetConstantAZs, Fn::Base64, Fn::GetAtt, Fn::GetAZs, Fn::ImportValue, Fn::Join, Fn::Split, Fn::FindInMap, Fn::Select, Ref, Fn::Equals, Fn::If, Fn::Not, Condition, Fn::And, Fn::Or, Fn::Contains, Fn::EachMemberEquals, Fn::EachMemberIn, Fn::ValueOf, Fn::ValueOfAll, Fn::RefAll, Fn::Sub, Fn::Cidr]
Expected: HTTP 200 with a StackId, the stack reaching CREATE_COMPLETE, and the Fn::GetStackOutput reference resolved to the producer stack's current (unexported) output value.
The roster quoted back in the refusal is byte-for-byte the roster the resolved issue 20261027T110526Z asked for, and its FIRST entry is the function being refused.

## Steps to Reproduce
1. Replay the behavioral suite and re-run cloudformation.get-stack-output-reads-unexported-output-as-snapshot.
2. Minimal repro: CreateStack a producer stack with an Output that has a Value but NO Export, wait for CREATE_COMPLETE. Then CreateStack a consumer stack whose resource property (or Output) is {"Fn::GetStackOutput": ["<producer stack name>", "<OutputKey>"]}. Expect HTTP 200 + CREATE_COMPLETE with the value resolved as a snapshot; the emulator refuses the CreateStack with ValidationError 'Template Error: Encountered unsupported function: Fn::GetStackOutput ...' whose own roster lists Fn::GetStackOutput.
