Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__iam__lambda__s3.cfn-service-role-lacks-s3-getobject-for-template-negative cloudformation__iam__lambda__s3.cfn-delete-stack-removes-iam-role-and-lambda-positive
Issue-Key: cloudformation__iam__lambda__s3.cfn-service-role-lacks-s3-getobject-for-template-negative|_error_code#9|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: cloudformation__iam__lambda__s3.cfn-delete-stack-removes-iam-role-and-lambda-positive|run#26|AssertionError|assert invoke_missing_message == invoke_not_found, (
Issue-Commit: f5ba8024c64455531a4bd3d9779b46ec0fe104e4

# cloudformation__iam__lambda__s3: a TemplateURL Amazon S3 will not serve is refused as CloudFormation words it, and an Invoke refusal names the ARN the invocation resolved

> This report is what is left of the eight-cause report filed at
> `20261029T192356Z-cloudformation-iam-lambda-s3-8-net-new-root.md`: the two causes below are
> CLOSED, and the six that are not are re-filed as
> `20261104T160000Z-cloudformation-iam-lambda-s3-six-causes-awaiting-the-lambda-provisioner-and-an-in-process-runtime.md`
> with their own replay keys. Nothing here claims those six work.

## Operation
CreateStack, Invoke

## Description

Cause A (cause 3 of the original eight): a `TemplateURL` whose object Amazon S3 will not hand
over was refused with a sentence of this emulator's own.
AWS: CloudFormation reports what AMAZON S3 answered — `S3 error: <that service's message>`
followed by the line that error is documented on
(`For more information check http://docs.aws.amazon.com/AmazonS3/latest/API/ErrorResponses.html`)
— and reports an object's ABSENCE as absence (`S3 object does not exist. Bucket: <b>, Key: <k>.
Error: <code>`) only where Amazon S3 established it, which is a key or an object version the
bucket does not hold.
Was: every refusal, whatever Amazon S3 said, became the absence sentence — so a bucket that
does not exist was reported as a missing object.
Now: `state/template_url.rs` words each class as CloudFormation words it, in the ONE read every
operation that resolves a reference shares (CreateStack, UpdateStack, CreateChangeSet,
CreateStackSet, UpdateStackSet, ValidateTemplate, GetTemplateSummary, EstimateTemplateCost,
CreateStackRefactor, ImportStacksToStackSet).
Replay: `cfn-service-role-lacks-s3-getobject-for-template-negative` passes end to end.

Why its recorded key (`_error_code#9`, "expected a ClientError, but the call SUCCEEDED") closes on a
WORDING change: when the report was filed, `CreateStack` with a `TemplateURL` was not implemented at
all, so nothing about the reference was refused and the finder saw the call accepted. By the time
this fix was made, the fetch itself had landed (`state/template_url.rs`), so the reference was
already refused and the ONE divergence left in that check was the sentence — which is what this
change closes, and the whole check then passes. The other half the report claims for this cause —
authorizing the fetch against the stack's `RoleARN` and refusing a role not granted `s3:GetObject` —
is deliberately NOT implemented: implemented, it refused a `CreateStack` the recorded run SERVES
(`ValidationError`/`S3 error: Access Denied` where the recording carries no error), because the
principal AWS evaluated there is not one this emulator models. The recording outranks the report's
account of AWS, so the grant is not required of the role.

Cause B (cause 5 of the original eight): `Invoke`'s `ResourceNotFoundException` named the
UNQUALIFIED function ARN.
AWS: the resource an invocation addresses is a function AT a version, so the refusal names the
ARN the invocation resolved, qualifier included —
`Function not found: arn:aws:lambda:<region>:<account>:function:<name>:$LATEST` for a request
that names no qualifier.
Was: the message stopped at the unqualified ARN, so a client comparing the refused ARN against
the one it invoked could not match it.
Now: `crate::state::addressed_function_arn` is the one construction of that ARN, shared by
`Invoke` and `InvokeWithResponseStream`.
Replay: `cfn-delete-stack-removes-iam-role-and-lambda-positive` passes end to end.

## Evidence
Cause A — expected `('ValidationError', 'S3 error: The specified bucket does not exist\nFor more
information check http://docs.aws.amazon.com/AmazonS3/latest/API/ErrorResponses.html')`; was
`('ValidationError', 'S3 object does not exist. Bucket: <bucket>, Key: <key>. Error:
NoSuchBucket')`.
Cause B — expected `Function not found:
arn:aws:lambda:us-west-2:000000000000:function:<name>:$LATEST`; was the same sentence without
the `:$LATEST` qualifier.

## Tests
* `services/service-com-amazonaws-cloudformation-cloudformation/tests/all/template_by_reference.rs`
  — the refusal table spans the classes the rule distinguishes (a key the bucket does not hold,
  an object version it does not hold, a bucket nothing created, and the two no-object-at-all
  rows), each driven through `CreateStack` and `CreateChangeSet`; the per-class wording itself is
  pinned by a unit table in `src/state/template_url.rs`.
* `services/service-com-amazonaws-lambda-awsgirapiservice/tests/all/invoke_addressed_function.rs`
  — every documented `FunctionName` form × where the qualifier is named × what is missing × all
  four `InvocationType` values, with a control that an invocation which resolves is not refused.
