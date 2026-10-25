Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codedeploy.register-application-revision-description-too-long-rejected codedeploy.register-on-premises-instance-missing-iam-arn-rejected codedeploy.create-deployment-revision-does-not-exist-rejected codedeploy.appspec-naming-nonexistent-alias-rejected-and-alias-not-created
Issue-Shape-Id: com.amazonaws.codedeploy#CodeDeploy_20141006
Issue-Key: codedeploy.register-application-revision-description-too-long-rejected|run#17|AssertionError|assert error == (
Issue-Key: codedeploy.register-on-premises-instance-missing-iam-arn-rejected|run#4|AssertionError|assert error == (
Issue-Key: codedeploy.create-deployment-revision-does-not-exist-rejected|run#10|AssertionError|assert message == "No application revision found for revision.", f"unexpected message: {message!r}"
Issue-Key: codedeploy.appspec-naming-nonexistent-alias-rejected-and-alias-not-created|run#12|AssertionError|assert got_message == ghost_message, f"get_alias message {got_message!r}, expected {ghost_message!r}"
Issue-Commit: 35f0669fbed72946ad7349f4657cbc8fac9f7c2b

# CodeDeploy (and Lambda GetAlias) refusal messages use emulator/model-doc prose instead of AWS's runtime sentence templates (4 causes)

> _Note: this issue was identified against the code base at commit `35f0669fbed72946ad7349f4657cbc8fac9f7c2b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterApplicationRevision

## Description
Four refusal paths exercised by the codedeploy behavioral checks return the CORRECT modeled error code and HTTP status but build the `Message` from the emulator's own prose (or, in one case, from the Smithy error doc sentence) instead of the fixed sentence template AWS's runtime emits. Each template is a known rule in the sense already accepted for this service by the fixed issue 20261022T171541Z-codedeploy-client-error-messages-do-not-follow-aws.md: a fixed sentence, measured against real AWS by the check, interpolating the request value where there is one — so the TEMPLATE, not one recording's generated names, is the requirement. Three of these four refusals abort their check at its first (or setup) assertion, so every downstream behavioral assertion in those checks — effect-absent checks, positive controls, the whole appspec alias flow — is never reached. Four distinct message-construction sites, four root causes. None of these four are covered by the open issue 20261025T035705Z-codedeploy-refusal-messages-omit-request-derived-identifiers-getdeploymenttarget.md (that one carries GetDeploymentTarget and UpdateDeploymentGroup), and none are the already-fixed templates of 20261022T171541Z-codedeploy-client-error-messages-do-not-follow-aws.md (ApplicationDoesNotExist / DeploymentGroupDoesNotExist / DeploymentDoesNotExist / ApplicationAlreadyExists / DeploymentGroupAlreadyExists / ApplicationNameRequired).

Cause 1 of 4: DescriptionTooLongException message on RegisterApplicationRevision.
AWS: `Revision description has a maximum length of 512` — a fixed sentence stating the limit, with NO echo of the submitted length. Smithy: RegisterApplicationRevision models DescriptionTooLongException (client error, HTTP 400, doc "The description is too long."); the `description` member targets the unconstrained `Description` string, so the 512 limit is a runtime rule the message states, not a modeled @length.
Emulator: `The description is too long: it is 4001 characters and at most 512 are allowed` — the emulator's own generic constraint sentence, interpolating the submitted length AWS never reports.
Why: a client or operator tool that matches AWS's sentence to recover the limit (the only machine-readable part of the error beyond the code) mis-parses it; the sentence also exposes a length-validator channel where AWS exposes a service-specific modeled error.
Fix: the message construction for DescriptionTooLongException in RegisterApplicationRevision's validation layer in the codedeploy service crate — it must raise the modeled error with its own fixed sentence rather than deferring to the generic length-constraint text.
Traces: codedeploy.register-application-revision-description-too-long-rejected (assertion at check line 56; the error CODE DescriptionTooLongException matched).

Cause 2 of 4: IamArnRequiredException message on RegisterOnPremisesInstance.
AWS: `The on-premises instance could not be registered because no IAM ARN was included in the request. Include either an IAM session ARN or IAM user ARN in the request, and then try again.` — two sentences, naming the failed action first and ending with the "try again" clause. Smithy: RegisterOnPremisesInstance models IamArnRequiredException (client error, HTTP 400); its doc sentence is "No IAM ARN was included in the request. You must use an IAM session ARN or user ARN in the request."
Emulator: `No IAM ARN was included in the request. You must use an IAM session ARN or IAM user ARN in the request` — i.e. the Smithy error DOC text, not the runtime message. Hypothesis: the handler emits the model's documentation string for this error shape.
Why: the documentation sentence and the runtime message are different strings in AWS; a client keyed to the runtime sentence (and operators reading it) see a shape description instead of the account-level explanation, and the pattern will repeat for every error whose message is sourced from the doc trait.
Fix: the message construction for IamArnRequiredException in RegisterOnPremisesInstance's validation in the codedeploy service crate; stop sourcing runtime messages from the error shape's doc text.
Traces: codedeploy.register-on-premises-instance-missing-iam-arn-rejected (assertion at check line 21; the error CODE IamArnRequiredException matched).

Cause 3 of 4: RevisionDoesNotExistException message on GetApplicationRevision.
AWS: `No application revision found for revision.` — a fixed sentence with a trailing period that does NOT name the application or the S3 location. Smithy: GetApplicationRevision models RevisionDoesNotExistException (client error, HTTP 400, doc "The named revision does not exist with the user or Amazon Web Services account.").
Emulator: `No revision found for application <applicationName>: the revision is not registered with it` — the emulator's own sentence, leaking the application name into an error AWS scopes to the revision only (observed: 'No revision found for application sim2real-cdapp-c52891e2b0bf: the revision is not registered with it').
Why: same mis-parse class as the already-fixed CodeDeploy templates; a harness or client matching AWS's sentence fails, and the emulator discloses a request value AWS's template omits.
Fix: the message construction for RevisionDoesNotExistException in GetApplicationRevision's handler/error layer in the codedeploy service crate.
Traces: codedeploy.create-deployment-revision-does-not-exist-rejected (assertion at check line 37; the error CODE RevisionDoesNotExistException matched, and the check's later list/positive-control assertions were never reached).

Cause 4 of 4: Lambda GetAlias ResourceNotFoundException message, which aborts the codedeploy appspec check in its setup.
AWS: `Cannot find alias arn: <aliasArn>` — a fixed sentence interpolating the fully-qualified alias ARN (function ARN + ':' + alias name) built from the request. Smithy (lambda): GetAlias models ResourceNotFoundException at HTTP 404; FunctionName and Name are @httpLabel members, so the ARN in the sentence is wholly request-derived.
Emulator: `Alias not found: arn:aws:lambda:<region>:<account>:function:<fn>:<alias>` — the right ARN, the wrong sentence template (observed: 'Alias not found: arn:aws:lambda:us-west-2:000000000000:function:sim2real-behav-cd-fn-64fc1ab941c2:ghostd59bc36f' vs expected 'Cannot find alias arn: arn:aws:lambda:us-west-2:000000000000:function:sim2real-behav-cd-fn-64fc1ab941c2:ghostd59bc36f').
Why: a client that strips AWS's prefix to recover the missing alias ARN gets a different prefix and fails; and because the codedeploy appspec check probes for alias absence in SETUP, this one sentence prevents the entire AppSpec-names-a-nonexistent-alias behavior (INVALID_LAMBDA_FUNCTION outcome, ghost alias not created, real alias undisturbed) from ever being exercised against the emulator.
Fix: the message construction for ResourceNotFoundException in the Lambda service crate's GetAlias handler (grounded here in a codedeploy-prefixed check, which is why it rides in this issue's service token).
Traces: codedeploy.appspec-naming-nonexistent-alias-rejected-and-alias-not-created (assertion at check line 66 / source line 1199; the error CODE ResourceNotFoundException matched).

## Evidence
Replayed behavioral suite (simparity-behavioral-ga-prod-v1), 53/91 passed. In all four causes the HTTP status and the error Code match AWS; only the Message sentence diverges. Verbatim failing assertions from the replay report:

- codedeploy.register-application-revision-description-too-long-rejected: expected ('DescriptionTooLongException', 'Revision description has a maximum length of 512'), got AssertionError: ('DescriptionTooLongException', 'The description is too long: it is 4001 characters and at most 512 are allowed').
- codedeploy.register-on-premises-instance-missing-iam-arn-rejected: expected ('IamArnRequiredException', 'The on-premises instance could not be registered because no IAM ARN was included in the request. Include either an IAM session ARN or IAM user ARN in the request, and then try again.'), got AssertionError: expected IamArnRequiredException, got ('IamArnRequiredException', 'No IAM ARN was included in the request. You must use an IAM session ARN or IAM user ARN in the request').
- codedeploy.create-deployment-revision-does-not-exist-rejected: `assert message == "No application revision found for revision."` -> AssertionError: unexpected message: 'No revision found for application sim2real-cdapp-c52891e2b0bf: the revision is not registered with it' (the preceding `assert code == "RevisionDoesNotExistException"` PASSED).
- codedeploy.appspec-naming-nonexistent-alias-rejected-and-alias-not-created: `assert got_message == ghost_message` -> AssertionError: get_alias message 'Alias not found: arn:aws:lambda:us-west-2:000000000000:function:sim2real-behav-cd-fn-64fc1ab941c2:ghostd59bc36f', expected 'Cannot find alias arn: arn:aws:lambda:us-west-2:000000000000:function:sim2real-behav-cd-fn-64fc1ab941c2:ghostd59bc36f' (the preceding `assert ghost_code == "ResourceNotFoundException"` PASSED).

Resource names, account id, region and generated suffixes above are environment-specific and are not requirements; the requirement in each cause is the sentence template with the request-derived value interpolated where AWS interpolates one. Smithy facts used: codedeploy RegisterApplicationRevision errors include DescriptionTooLongException (400) and `description` targets the unconstrained Description string; RegisterOnPremisesInstance errors include IamArnRequiredException (400) whose doc text is what the emulator returns; GetApplicationRevision errors include RevisionDoesNotExistException (400); lambda GetAlias errors include ResourceNotFoundException (404) with FunctionName/Name as @httpLabel request members.

## Steps to Reproduce
1. Boot the emulator and point botocore clients for codedeploy, lambda and s3 at it.
2. CreateApplication(applicationName=app, computePlatform='Server'); create a bucket and put a small zip; then RegisterApplicationRevision(applicationName=app, description='x'*4001, revision={S3 ...}) -> expect Code DescriptionTooLongException, HTTP 400, Message exactly 'Revision description has a maximum length of 512'.
3. RegisterOnPremisesInstance(instanceName=<new name>) with neither iamUserArn nor iamSessionArn -> expect Code IamArnRequiredException, HTTP 400, Message exactly 'The on-premises instance could not be registered because no IAM ARN was included in the request. Include either an IAM session ARN or IAM user ARN in the request, and then try again.'
4. GetApplicationRevision(applicationName=app, revision={revisionType:'S3', s3Location:{bucket, key:'never/registered.zip', bundleType:'zip'}}) for a revision never registered -> expect Code RevisionDoesNotExistException, HTTP 400, Message exactly 'No application revision found for revision.' (no application name in the sentence).
5. Create a Lambda function and call lambda GetAlias(FunctionName=fn, Name=<alias that was never created>) -> expect HTTP 404, Code ResourceNotFoundException, Message 'Cannot find alias arn: <functionArn>:<aliasName>'.
6. Re-replay the four checks listed in trace_ids to confirm; note that checks 2-5 above each abort their check at its first/setup assertion today, so fixing them also unblocks the downstream effect-absent and positive-control assertions in those checks.
