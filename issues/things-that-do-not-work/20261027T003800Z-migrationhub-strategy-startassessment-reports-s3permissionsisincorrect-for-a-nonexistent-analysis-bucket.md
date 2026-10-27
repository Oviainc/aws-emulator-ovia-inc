Issue-Source: trace-sweep-divergence
Issue-Trace: migrationhubstrategy-assessment-lifecycle
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-ga-prod-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: migrationhubstrategy-assessment-lifecycle
Issue-Shape-Id: com.amazonaws.migrationhubstrategy#AWSMigrationHubStrategyRecommendation
Issue-Key: migrationhub-strategy|StartAssessment|4xx->4xx|$.message
Issue-Commit: 0550644949cffe934022bf1b5a8b285c9bc5e880

# StartAssessment reports S3PermissionsIsIncorrect for a nonexistent analysis bucket instead of S3BucketNotFound

> _Note: this issue was identified against the code base at commit `0550644949cffe934022bf1b5a8b285c9bc5e880`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartAssessment

## Description
The emulator's StartAssessment S3 pre-flight validation collapses every rejectable bucket into a single bucket-name-prefix permission complaint, so a client cannot tell "the bucket you named does not exist / I cannot reach it" from "the bucket exists but its name lacks the required prefix".

Cause 1 of 1: StartAssessment called with an s3bucketForAnalysisData / s3bucketForReportData naming a bucket that does not exist is answered with the S3PermissionsIsIncorrect message rather than the S3BucketNotFound message.

AWS: Smithy: StartAssessment (POST /start-assessment, success 200) takes s3bucketForAnalysisData and s3bucketForReportData as String with @length min 0 max 63 and @pattern [0-9a-z]+[0-9a-z\.\-]*[0-9a-z]+; the bucket submitted here satisfies both, so the rejection is not a modeled constraint failure but a runtime check against S3. The bound error set is AccessDeniedException (403), ServiceQuotaExceededException (402), ThrottlingException (429), InternalServerException (500); the service-common ValidationException is modeled errorKind=client with httpError 400 and a single `message` member, documented "The request body isn't valid.". Recorded behavior only for the discrimination itself: AWS answers HTTP 400 and the message 'S3 bucket "<bucket>" does not exist or is not accessible [S3BucketNotFound]'. Fixed text of that message is `S3 bucket "` ... `" does not exist or is not accessible [S3BucketNotFound]`; the only variable part is the bucket name, taken verbatim from the request member that failed the check. The bracketed trailing token is the service's reason code and is part of the fixed text for this error class.

Emulator: answers HTTP 400 (status matches) but with the message 'The service doesn't have permissions to specified S3 Bucket Service doesn't hv permissions to access bucket, please make sure bucket name begins with "migrationhub-strategy". [S3PermissionsIsIncorrect]'. That is a different error class entirely: it carries the reason code S3PermissionsIsIncorrect, it does not interpolate the bucket name from the request at all (the quoted string is the literal required prefix, not the submitted bucket), and it asserts a naming-prefix rule as the cause. The divergence is confined to $.message; expected and actual status are both 400.

Why: callers branch on the reason code and on the message body to decide what to do next. A client that gets S3BucketNotFound creates or corrects the bucket and retries; a client that gets S3PermissionsIsIncorrect renames the bucket to the required prefix or fixes a bucket policy. Because the emulator always returns the latter, a caller handling the missing-bucket case never sees it and either loops renaming an already correctly named bucket or surfaces a misleading diagnostic. The message also loses the submitted bucket name, so logs and user-facing errors cannot say which of the two bucket members was bad when both are supplied.

Fix: the StartAssessment handler's S3 bucket pre-flight validation in the migrationhubstrategy service crate. It needs to order and distinguish the checks: existence/reachability of the named bucket first, emitting the S3BucketNotFound-reason ValidationException with the requested bucket name interpolated, and only fall through to the S3PermissionsIsIncorrect-reason message when the bucket is reachable but fails the prefix/permission rule. The reason code must be selected per failing condition rather than hard-coded.

Traces: migrationhubstrategy-assessment-lifecycle#task#3 (StartAssessment with a bucket that does not exist and does not carry the 'migrationhub-strategy' prefix).

Not covered by any open issue: the open 'migrationhubstrategy StartAssessment assessmentTargets is stored and echoed but never scopes the assessment' issue concerns a different member and a success path, and the open 'StartImportFileTask accepts an import name/key AWS rejects and a bucket that does not exist' issue is a different operation.

## Evidence
Fixture migrationhubstrategy-assessment-lifecycle, step task#3, operation StartAssessment, service migrationhub-strategy.

Expected status: 400. Actual status: 400 (status agrees).
Diverging key: $.message (value_mismatch).

Expected $.message:
  S3 bucket "not-a-valid-prefix-bucket" does not exist or is not accessible [S3BucketNotFound]

Actual $.message:
  The service doesn't have permissions to specified S3 Bucket Service doesn't hv permissions to access bucket, please make sure bucket name begins with "migrationhub-strategy". [S3PermissionsIsIncorrect]

Message anatomy for the expected error class:
  fixed text  -> 'S3 bucket "' , '" does not exist or is not accessible [S3BucketNotFound]'
  from request -> the bucket name between the quotes (the value of the s3bucket* member that failed the check; 'not-a-valid-prefix-bucket' is only this recording's value)

The emulator's message interpolates no request value: the quoted token in it is the fixed required prefix, and its reason code S3PermissionsIsIncorrect names a different failure condition from the recorded S3BucketNotFound.

Replay summary for the run: emulator_up=true, 9 steps total, 8 passed, 1 mismatched, score 88.9%, not_implemented=0 — the service is serving, so this is a fidelity defect and not a bootstrap gap. Divergence key: service=migrationhub-strategy|StartAssessment|status=400->400|keys=$.message.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay the wire corpus (trace set simparity-wire-ga-prod-v1), or replay fixture 'migrationhubstrategy-assessment-lifecycle' alone and watch step 'task#3'.
3. Equivalently, by hand: POST /start-assessment to the migrationhubstrategy endpoint with s3bucketForAnalysisData (and/or s3bucketForReportData) set to the name of a bucket that does not exist in the emulator's state and that does not begin with 'migrationhub-strategy' — e.g. a 1..63 char name matching [0-9a-z]+[0-9a-z\.\-]*[0-9a-z]+ such as the fixture's 'not-a-valid-prefix-bucket'.
4. Observe HTTP 400 with $.message ending in '[S3PermissionsIsIncorrect]' and no occurrence of the submitted bucket name.
5. Expected: HTTP 400 with $.message exactly 'S3 bucket "<submitted-bucket>" does not exist or is not accessible [S3BucketNotFound]'.
6. Regression guard: with a reachable bucket whose name lacks the required prefix, the S3PermissionsIsIncorrect message must still be returned, so fixing this must not simply swap one hard-coded message for the other.
