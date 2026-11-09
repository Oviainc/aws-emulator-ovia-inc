Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__codeguruprofiler.deploy-codeguruprofiler-profilinggroup-round-trip-and-delete cloudformation__codeguruprofiler.deploy-codeguruprofiler-profilinggroup-invalid-computeplatform-rejected
Issue-Key: cloudformation__codeguruprofiler|CreateStack|ValidationError
Issue-Commit: 0cee419aadfb4868f9ccd3aa23bc5045ac08a29b

# cloudformation__codeguruprofiler: AWS::CodeGuruProfiler::ProfilingGroup is not a registered resource type, so CreateStack refuses every profiling-group template

> _Note: this issue was identified against the code base at commit `0cee419aadfb4868f9ccd3aa23bc5045ac08a29b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
One root cause behind both failing cloudformation__codeguruprofiler checks.

=== Cause 1 of 1: the CloudFormation resource type AWS::CodeGuruProfiler::ProfilingGroup is unknown to the emulator's template parser ===
AWS: Smithy CreateStack (cloudformation, awsQuery) returns 200 with a StackId for a template whose Resources name a registered type, and refuses an unknown type with 400 ValidationError 'Template format error: Unrecognized resource types: [<Type>]'. AWS::CodeGuruProfiler::ProfilingGroup IS a registered type (properties ProfilingGroupName, ComputePlatform, AgentPermissions, AnomalyDetectionNotificationConfiguration, Tags; Ref returns the group name and Fn::GetAtt 'Arn' its ARN), so AWS accepts both templates: the valid one provisions a profiling group that CodeGuru Profiler's own DescribeProfilingGroup/ListTagsForResource then read back, and the one whose ComputePlatform is outside the enum is accepted by CreateStack and fails the RESOURCE, rolling the stack back to ROLLBACK_COMPLETE with the registry handler's CREATE_FAILED reason 'Resource handler returned message: "Model validation failed (#/ComputePlatform: #: only 1 subschema matches out of 2)\n#/ComputePlatform: failed validation constraint for keyword [enum] (#/ComputePlatform)" (RequestToken: <token>, HandlerErrorCode: InvalidRequest)' — fixed text with the request-derived member path and a per-run token.
Emulator: CreateStack itself is refused, for both templates, with 400 ValidationError 'Template format error: Unrecognized resource types: [AWS::CodeGuruProfiler::ProfilingGroup]'. No stack is created, so nothing downstream (the CREATE_COMPLETE wait, Ref/Fn::GetAtt Outputs, the CodeGuru Profiler read-back, the rollback event's handler reason, the delete-then-ResourceNotFoundException absence proof) can run.
Why: every infrastructure-as-code path to CodeGuru Profiler is closed. A client deploying a template that real AWS accepts gets a hard 400 at submission time, and the rejection names a template-format error for a type that is perfectly valid — so an operator reading it concludes their template is malformed. The negative class is lost too: a template with a bad ComputePlatform must be ACCEPTED by CreateStack and then fail the resource with the handler's InvalidRequest reason, a distinction the emulator cannot express while the type is unregistered.
Fix: the CloudFormation resource-type registry and the provisioning handler for this type — register AWS::CodeGuruProfiler::ProfilingGroup, provision/delete it against the codeguruprofiler state (so DescribeProfilingGroup, ListTagsForResource and the post-delete ResourceNotFoundException all agree with the stack), implement Ref (group name) and the Arn GetAtt, and validate ComputePlatform against its enum in the resource handler so a bad value yields CREATE_FAILED with the handler-shaped reason and a ROLLBACK_COMPLETE stack rather than a CreateStack-time refusal.
Traces: cloudformation__codeguruprofiler.deploy-codeguruprofiler-profilinggroup-round-trip-and-delete, cloudformation__codeguruprofiler.deploy-codeguruprofiler-profilinggroup-invalid-computeplatform-rejected.

## Evidence
Both checks die on the first CreateStack:
  botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::CodeGuruProfiler::ProfilingGroup]

expected (round-trip check): CreateStack -> 200 StackId; stack reaches CREATE_COMPLETE; Outputs PgName == {'Ref': 'Pg'} == the profiling-group name and PgArn == {'Fn::GetAtt': ['Pg','Arn']} == DescribeProfilingGroup(...)['profilingGroup']['arn']; computePlatform 'Default'; ListTagsForResource(resourceArn=PgArn) == the template's own tags only ({'sim2real-pg': <name>}, no stack-level tag); after DeleteStack, DescribeProfilingGroup refuses with ResourceNotFoundException 'CodeGuru Profiler can’t complete the operation because <name> was not found.'

expected (invalid-ComputePlatform check): the SAME template with ComputePlatform='NotAPlatform' is ACCEPTED by CreateStack, the stack ends ROLLBACK_COMPLETE, the single CREATE_FAILED event for logical id 'Pg' carries reason 'Resource handler returned message: "Model validation failed (#/ComputePlatform: #: only 1 subschema matches out of 2)\n#/ComputePlatform: failed validation constraint for keyword [enum] (#/ComputePlatform)" (RequestToken: <token>, HandlerErrorCode: InvalidRequest)', and no profiling group of that name exists afterwards; the control template with ComputePlatform='Default' reaches CREATE_COMPLETE.
actual (both): HTTP 400 ValidationError at CreateStack, no stack and no profiling group.

## Steps to Reproduce
1. Boot the emulator and point botocore cloudformation and codeguruprofiler clients at it.
2. CreateStack(StackName=S, TemplateBody=json of {'AWSTemplateFormatVersion':'2010-09-09','Resources':{'Pg':{'Type':'AWS::CodeGuruProfiler::ProfilingGroup','Properties':{'ProfilingGroupName':N,'ComputePlatform':'Default','Tags':[{'Key':'sim2real-pg','Value':N}]}}},'Outputs':{'PgName':{'Value':{'Ref':'Pg'}},'PgArn':{'Value':{'Fn::GetAtt':['Pg','Arn']}}}}, Tags=[{'Key':'sim2real','Value':S}]) -> 400 ValidationError 'Template format error: Unrecognized resource types: [AWS::CodeGuruProfiler::ProfilingGroup]' instead of a StackId.
3. Repeat step 2 with ComputePlatform='NotAPlatform' -> the same CreateStack-time 400, where AWS accepts the stack and fails the resource with HandlerErrorCode InvalidRequest and rolls back.
4. Or replay the two checks listed in trace_ids.
