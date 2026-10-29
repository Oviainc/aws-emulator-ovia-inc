Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: compute-optimizer.export-concurrent-jobs-limit-exceeded compute-optimizer.get-effective-recommendation-preferences-malformed-arn-rejected compute-optimizer.put-recommendation-preferences-asg-account-scope-rejected compute-optimizer.update-enrollment-status-pending-value-rejected
Issue-Shape-Id: com.amazonaws.computeoptimizer#ComputeOptimizerService
Issue-Key: compute-optimizer.export-concurrent-jobs-limit-exceeded|run#9|AssertionError|assert message == "Export job limit exceeded. You can only have one active job per resource type at a time.", f"unexpected LimitExceededException message: {mess
Issue-Key: compute-optimizer.get-effective-recommendation-preferences-malformed-arn-rejected|run#6|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: compute-optimizer.put-recommendation-preferences-asg-account-scope-rejected|run#8|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: compute-optimizer.update-enrollment-status-pending-value-rejected|run#8|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Commit: 0711e0a570adac04732f4f058a2849d70bc38321

# compute-optimizer: four client-error messages are emulator-invented instead of AWS's wording (generic "value supplied for parameter X is not valid" template, and a custom export-limit text)

> _Note: this issue was identified against the code base at commit `0711e0a570adac04732f4f058a2849d70bc38321`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateEnrollmentStatus

## Description
Four distinct refusal paths in the compute-optimizer crate return the correct error CODE and HTTP status but a message the service never emits. Three of them (Causes 2-4) are worded by a shared generic builder that emits "The value supplied for parameter <memberName> is not valid: <explanation>" and interpolates the offending request value; AWS words each of these refusals with its own fixed sentence that does NOT name the member and does NOT echo the request value. The fourth (Cause 1) invents an entirely different export-limit sentence and interpolates the in-flight jobId. Clients and SDK-level test suites that match on the documented message text see a message that is not AWS's. The existing open issue 20261023T062035Z-compute-optimizer-wrong-rejection-class-for-out-of.md covers a wrong rejection CLASS for out-of-range maxResults and the unresolved effectiveIncludeList — it does not cover any of the four message-wording causes below.

=== Cause 1 of 4: ExportEC2InstanceRecommendations concurrent-job refusal message ===
AWS: Smithy: ExportEC2InstanceRecommendations errors include LimitExceededException (client, httpError 400, doc "The request exceeds a limit of the service."). Recorded behavior: with one Queued/InProgress Ec2Instance export job already in flight, a second ExportEC2InstanceRecommendations call is refused 400 LimitExceededException with the message "Export job limit exceeded. You can only have one active job per resource type at a time." That message is entirely FIXED text — it has no variable parts: no jobId, no resource type, no region.
Emulator: returns LimitExceededException (code correct) with the message "You can have only one Amazon EC2 instance export job in progress per Amazon Web Services Region. Export job <jobId> is still in progress." — different fixed text, plus an interpolated jobId that AWS's message does not contain.
Why: a client that matches the documented limit message, or that parses the message for a job identifier it will never see in AWS, behaves differently against the emulator than against AWS; a caller cannot distinguish this refusal class by its text.
Fix: the ExportEC2InstanceRecommendations handler's concurrency-limit refusal site in the compute-optimizer crate (the error-wording helper that builds its LimitExceededException body).
Traces: compute-optimizer.export-concurrent-jobs-limit-exceeded

=== Cause 2 of 4: GetEffectiveRecommendationPreferences malformed resourceArn message ===
AWS: Smithy: GetEffectiveRecommendationPreferences input member resourceArn (target ResourceArn, required); modeled errors include InvalidParameterValueException (client, httpError 400, doc "The value supplied for the input parameter is out of range or not valid."). Recorded behavior: resourceArn="not-an-arn" is refused 400 InvalidParameterValueException with the message "Invalid Amazon Resource Name (ARN) specified. Please check the resource name and try again." All of that is FIXED text; the rejected ARN value is NOT echoed.
Emulator: code InvalidParameterValueException is correct, but the message is "The value supplied for parameter resourceArn is not valid: <value> is not the Amazon Resource Name (ARN) of an EC2 instance or an Auto Scaling group." — the generic parameter template, naming the member and echoing the request value.
Why: a client keying off the documented ARN-validation sentence does not recognise the refusal; and the emulator reflects caller-supplied input back into an error body where AWS does not.
Fix: the GetEffectiveRecommendationPreferences handler's resourceArn validation in the compute-optimizer crate — it must raise the ARN-specific fixed message rather than the shared "value supplied for parameter" builder.
Traces: compute-optimizer.get-effective-recommendation-preferences-malformed-arn-rejected

=== Cause 3 of 4: PutRecommendationPreferences AutoScalingGroup-at-account-scope message ===
AWS: Smithy: PutRecommendationPreferences input members resourceType (ResourceType enum, required) and scope (Scope); ScopeName enum = Organization | AccountId | ResourceArn; modeled error InvalidParameterValueException (client, 400). Recorded behavior: resourceType=AutoScalingGroup with scope {name: AccountId} is refused 400 InvalidParameterValueException with the message 'You cannot create a recommendation preference for an Auto Scaling Group with the "Organization" or "AccountId" scope name. Specify "ResourceArn" as the scope name, the Auto Scaling group ARN ID as the scope value, and try again.' That whole sentence pair is FIXED text (including the literal quoted scope-name tokens); nothing is taken from the request.
Emulator: code InvalidParameterValueException is correct, but the message is "The value supplied for parameter scope is not valid: a AutoScalingGroup recommendation preference can only be created at the resource level, with a scope name of ResourceArn." — again the generic parameter template (and ungrammatical "a AutoScalingGroup").
Why: the refusal is correct but unrecognisable by text; a client that surfaces AWS's remediation sentence ("Specify \"ResourceArn\" as the scope name, the Auto Scaling group ARN ID as the scope value") shows the user different guidance.
Fix: the PutRecommendationPreferences handler's scope/resourceType compatibility validation in the compute-optimizer crate.
Traces: compute-optimizer.put-recommendation-preferences-asg-account-scope-rejected

=== Cause 4 of 4: UpdateEnrollmentStatus non-input status value message ===
AWS: Smithy: UpdateEnrollmentStatus input member status (target Status, required); Status enum = Active | Inactive | Pending | Failed, of which Pending and Failed are output-only states; modeled error InvalidParameterValueException (client, 400). Recorded behavior: status=Pending is refused 400 InvalidParameterValueException with the message "Invalid status, should be 'Active' or 'Inactive'." — entirely FIXED text; the rejected value is not echoed and the member name is not mentioned.
Emulator: code InvalidParameterValueException is correct, but the message is "The value supplied for parameter status is not valid: Pending cannot be used to update the enrollment status of an account. Valid values are Active and Inactive." — the generic parameter template, echoing the rejected enum value.
Why: a client matching the documented short sentence treats the emulator's refusal as an unknown error; the enrollment state is correctly left unmutated, so only the wording diverges.
Fix: the UpdateEnrollmentStatus handler's status validation in the compute-optimizer crate.
Traces: compute-optimizer.update-enrollment-status-pending-value-rejected

## Evidence
All four failures are message-text mismatches; the error code and HTTP status matched in every case.

Cause 1 — compute-optimizer.export-concurrent-jobs-limit-exceeded [mismatch]
  code: LimitExceededException (as expected, 400)
  expected message: "Export job limit exceeded. You can only have one active job per resource type at a time."  (100% fixed text)
  actual message:   "You can have only one Amazon EC2 instance export job in progress per Amazon Web Services Region. Export job 14220e36-afdc-48e8-8e8c-b270a4334573 is still in progress."
  (the UUID is a per-run value filling the emulator's own template — the defect is the template, not that id)
  AssertionError: unexpected LimitExceededException message

Cause 2 — compute-optimizer.get-effective-recommendation-preferences-malformed-arn-rejected [mismatch]
  code: InvalidParameterValueException (as expected, 400)
  expected message: "Invalid Amazon Resource Name (ARN) specified. Please check the resource name and try again."  (100% fixed text)
  actual message:   "The value supplied for parameter resourceArn is not valid: not-an-arn is not the Amazon Resource Name (ARN) of an EC2 instance or an Auto Scaling group."
  ("resourceArn" is the member name and "not-an-arn" is the request value — both are interpolated by the emulator's generic builder; neither appears in AWS's message)

Cause 3 — compute-optimizer.put-recommendation-preferences-asg-account-scope-rejected [mismatch]
  code: InvalidParameterValueException (as expected, 400)
  expected message: 'You cannot create a recommendation preference for an Auto Scaling Group with the "Organization" or "AccountId" scope name. Specify "ResourceArn" as the scope name, the Auto Scaling group ARN ID as the scope value, and try again.'  (100% fixed text)
  actual message:   "The value supplied for parameter scope is not valid: a AutoScalingGroup recommendation preference can only be created at the resource level, with a scope name of ResourceArn."
  The check's follow-up assertions confirm the effect was correctly absent (no AccountId-scoped AutoScalingGroup entry created), so only the wording diverges.

Cause 4 — compute-optimizer.update-enrollment-status-pending-value-rejected [mismatch]
  code: InvalidParameterValueException (as expected, 400)
  expected message: "Invalid status, should be 'Active' or 'Inactive'."  (100% fixed text)
  actual message:   "The value supplied for parameter status is not valid: Pending cannot be used to update the enrollment status of an account. Valid values are Active and Inactive."
  ("status" is the member name and "Pending" is the request value — both interpolated; AWS's message contains neither)

Shared signature: Causes 2, 3 and 4 all emit the prefix "The value supplied for parameter <member> is not valid: ", indicating one generic message builder is used where AWS has three distinct fixed sentences.

## Steps to Reproduce
Against the emulator, with the account opted in (UpdateEnrollmentStatus status=Active, GetEnrollmentStatus reporting Active):

Cause 1: create an S3 bucket with the Compute Optimizer export bucket policy, then call ExportEC2InstanceRecommendations(s3DestinationConfig={bucket}) once (it returns a jobId, job Queued/InProgress). Immediately call ExportEC2InstanceRecommendations again for the same resource type with retries disabled (botocore Config(retries={'total_max_attempts': 1}), since LimitExceededException is retried as throttling). Read Error.Message: it is the emulator's region/jobId sentence, not "Export job limit exceeded. You can only have one active job per resource type at a time."

Cause 2: call GetEffectiveRecommendationPreferences(resourceArn="not-an-arn"). Read Error.Code (InvalidParameterValueException, correct) and Error.Message: it is the generic "The value supplied for parameter resourceArn is not valid: ..." rather than "Invalid Amazon Resource Name (ARN) specified. Please check the resource name and try again."

Cause 3: call PutRecommendationPreferences(resourceType="AutoScalingGroup", scope={"name": "AccountId", "value": <callerAccountId>}, lookBackPeriod="DAYS_32"). Read Error.Message: it is the generic parameter sentence rather than AWS's 'You cannot create a recommendation preference for an Auto Scaling Group with the "Organization" or "AccountId" scope name. ...' text. (The same call with resourceType="Ec2Instance" succeeds, confirming the rejection path itself is reached correctly.)

Cause 4: call UpdateEnrollmentStatus(status="Pending"). Read Error.Message: it is the generic parameter sentence rather than "Invalid status, should be 'Active' or 'Inactive'." GetEnrollmentStatus still reports Active afterwards, so only the wording diverges.

Replay list: run_behavioral_suite and re-run the four checks named in trace_ids.
