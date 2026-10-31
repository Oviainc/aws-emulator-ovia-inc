Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: signer.put-signing-profile-invalid-name-pattern-rejected signer.sign-payload-payload-too-large-rejected signer.add-profile-permission-stale-revision-conflict-rejected signer.add-profile-permission-add-and-remove-round-trip signer.start-signing-job-client-request-token-idempotent
Issue-Shape-Id: com.amazonaws.signer#WallabyService
Issue-Key: signer.put-signing-profile-invalid-name-pattern-rejected|run#6|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: signer.sign-payload-payload-too-large-rejected|run#9|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: signer.add-profile-permission-stale-revision-conflict-rejected|run#10|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: signer.add-profile-permission-add-and-remove-round-trip|run#15|AssertionError|raise AssertionError(
Issue-Key: signer.start-signing-job-client-request-token-idempotent|_poll_until#26|Timeout|time.sleep(interval_s)
Issue-Commit: 78295efc92da5c3ab6dc723b39af1e4944870efd

# signer: 5 net-new causes — two refusals answer generic framework constraint text, stale-revision conflict is misworded, an emptied permission policy is answered 200, and jobInvoker is never recorded

> _Note: this issue was identified against the code base at commit `78295efc92da5c3ab6dc723b39af1e4944870efd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutSigningProfile

## Description
Five net-new root causes in the signer crate, all on the same service, found by replaying the behavioral suite (6/23 passed). They are independent and ALL FIVE must be closed before this issue is submitted. Related open issue 20261029T191542Z-signer-error-paths-diverge-from-aws-in-message.md was filed from the WIRE corpus and covers the sibling RemoveProfilePermission stale-revision defect (its cause 2) and the ListProfilePermissions emptied-policy gate (its cause 8); causes 3 and 4 below are the same defects reached from the behavioral corpus through AddProfilePermission and through the add/remove round trip, and are listed here so the behavioral checks that witness them are replayable. Causes 1, 2 and 5 are not covered by any open issue.

Cause 1 of 5: PutSigningProfile answers the generic framework constraint sentence for an invalid profileName instead of Signer's own ValidationException text.
AWS: 400 ValidationException (Smithy: PutSigningProfile error ValidationException, httpError 400, members {message, code}; PutSigningProfile.profileName targets ProfileName, @length 2..64, @pattern ^[a-zA-Z0-9_]{2,}$). Recorded behavior: the message is the fixed sentence "Profile name must contain only alphanumeric characters and underscore." — the whole string is fixed text, it carries NO value from the request (the rejected name is not echoed, and no regex is shown).
Emulator: 400 ValidationException whose message is the shared framework constraint rendering: "1 validation error detected: Value 'sim2real-bad-name!' at 'profileName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9_]{2,}$" — a different template, which also echoes the caller's rejected value and leaks the anchored model regex.
Why: a client (or a test) that matches Signer's documented refusal wording never matches, and callers are handed the raw Smithy pattern instead of the service's human rule; the echoed value also contradicts the no-echo ceiling this crate states for its other refusals.
Fix: PutSigningProfile's own profileName validation in the signer crate's handler must run and compose the ValidationException message itself, rather than letting the generic schema constraint validator in the request walk answer for it.
Traces: signer.put-signing-profile-invalid-name-pattern-rejected.

Cause 2 of 5: SignPayload answers the generic framework constraint sentence for an oversized payload instead of Signer's size-requirements text.
AWS: 400 ValidationException (Smithy: SignPayload.payload targets Payload, a blob with @length min 1 max 4096; error ValidationException httpError 400). Recorded behavior: the message is "Payload does not meet size requirements: [1B - 4096B]". Fixed text: "Payload does not meet size requirements: [" … "B - " … "B]"; the variable parts are the modeled minimum and maximum of the payload blob (1 and 4096), not anything from the request — the offending length is NOT reported.
Emulator: right status and right error class, but message "1 validation error detected: Value '4097 bytes' at 'payload' failed to satisfy constraint: Member must have length less than or equal to 4096" — the generic constraint template, naming the request's length and the member path.
Why: the error message is part of the contract; a caller cannot read the admissible RANGE (the 1-byte minimum is never stated) out of the emulator's text, and wording-sensitive clients diverge.
Fix: SignPayload's payload size validation in the signer crate (ahead of the handler body), composing Signer's message; the generic constraint validator must not be the thing that words this refusal. Note the operation itself is still a scaffold stub (own issue 20261024T054020Z-signer-signpayload-stub.md) — this cause is only about the refusal text, which the emulator already produces before the stub is reached.
Traces: signer.sign-payload-payload-too-large-rejected.

Cause 3 of 5: AddProfilePermission's stale-revisionId conflict carries emulator-authored prose that withholds the current revision id.
AWS: 409 ConflictException (Smithy: AddProfilePermission error ConflictException, httpError 409, members {message, code}) with message "Specified revisionId (<supplied revisionId>) does not match the revisionId of the existing policy (<the policy's current revision id>)". Fixed text: "Specified revisionId (" … ") does not match the revisionId of the existing policy (" … ")"; variable parts: the revisionId member of the request, and the current revision id read from the profile's stored policy. Hypothesis: by analogy with the RECORDED sibling refusal RemoveProfilePermission (see cause 2 of 20261029T191542Z-signer-error-paths-diverge-from-aws-in-message.md), the body's `code` member should be "PolicyRevisionIdMismatch" rather than the Smithy shape name; the behavioral check only asserts the message, so the code is flagged as a hypothesis, not as a measured divergence.
Emulator: status and error class are right, but the message is "revision id <supplied> is not the current revision of the permissions of profile <profileName>" — emulator prose that names the profile instead of the current revision id.
Why: a client that lost the optimistic-concurrency race must retry with the policy's current revision; AWS hands that id back in the message, the emulator does not, so the retry cannot be built from the refusal and the caller must issue an extra ListProfilePermissions (and a wording-sensitive client does not match at all).
Fix: the revision check and ConflictException construction in AddProfilePermission's handler in the signer crate — format AWS's template from the request's revisionId and the stored policy revision. The same wording is wrong in RemoveProfilePermission, so the template belongs with the policy state (crate::state::permissions) and should be shared by both operations.
Traces: signer.add-profile-permission-stale-revision-conflict-rejected.

Cause 4 of 5: once the last permission statement is removed, ListProfilePermissions answers 200 with an empty policy instead of refusing with the policy-not-found 404.
AWS: after RemoveProfilePermission takes out the profile's last statement the resource policy no longer exists, and the read answers 404 ResourceNotFoundException (Smithy: ListProfilePermissions errors include ResourceNotFoundException, httpError 404, members {message, code}) with message "No policies associated with profile <profileName>". Fixed text: "No policies associated with profile "; the variable part is the profileName label from the request. The recorded reply also carries an x-amzn-errortype header.
Emulator: 200 with a body carrying the surviving policy — revisionId set, policySizeBytes nonzero and permissions: [] — because RemoveProfilePermission leaves an empty policy record behind and the handler only takes its absent-policy branch when no policy record was ever created.
Why: a client that removes the last grant and re-reads cannot tell "policy gone" from "policy that exists with no statements", so it skips the re-create AWS requires, and it never sees the 404 it must handle. The reported policySizeBytes for a policy with no statements is also non-zero.
Fix: the absent-policy gate in ListProfilePermissions' handler (services/service-com-amazonaws-signer-wallabyservice/src/ops/list_profile_permissions.rs, the store.profile_policy(...) branch), plus the state that must drop the policy record when its last statement is removed (src/state/permissions.rs). Distinct from open issue 20261028T170733Z-signer-listprofilepermissions-pagination.md (a nextToken refused after a statement is removed).
Traces: signer.add-profile-permission-add-and-remove-round-trip.

Cause 5 of 5: a signing job started by the caller's own account records no jobInvoker, so ListSigningJobs filtered by that account id can never report any job.
AWS: recorded behavior only — the model documents SigningJob.jobInvoker as "The AWS account ID of the job invoker" (AccountId, @length 12..12, @pattern ^[0-9]{12}$) and ListSigningJobs.jobInvoker as "Filters results to return only signing jobs initiated by a specified IAM entity". The recording shows that listing jobs with jobInvoker set to the CALLER's own account id reports the jobs that caller started: the check starts jobs and then enumerates them with ListSigningJobs(jobInvoker=<caller account>, maxResults=25) to prove the clientRequestToken deduplicated to exactly one job.
Emulator: StartSigningJob stores job_invoker: None for every job (services/service-com-amazonaws-signer-wallabyservice/src/ops/start_signing_job.rs, the record it inserts), and JobListing::includes keeps a job only when the filter equals the RECORDED value (src/state.rs, kept(self.job_invoker, job.job_invoker)). So ListSigningJobs(jobInvoker=<any account>) matches nothing, the listing is always empty, and the check's poll over it never becomes satisfiable — the check is consumed by the 60s poll and times out ("Check timed out after 60.0s") instead of reaching its assertions. The same omission makes jobInvoker absent from every DescribeSigningJob / ListSigningJobs job summary.
Why: any client that enumerates its own signing jobs by invoker — the documented way to scope a listing to the entity that started the jobs — gets an empty page forever, and a job summary never reports who invoked it, so job inventory, idempotency verification and cross-account attribution all silently return nothing.
Fix: StartSigningJob (and the SignPayload job path, once implemented) must record the invoking account on the job record, and the jobInvoker filter in the signer crate's job listing must match it; the stored value is what both the summary member and the filter read.
Traces: signer.start-signing-job-client-request-token-idempotent.

## Evidence
Behavioral suite replay against the emulator built from mainline: 6/23 passed (26.1%), 10 mismatched, 7 errored; service signer.

Cause 1 — signer.put-signing-profile-invalid-name-pattern-rejected (mismatch), PutSigningProfile(profileName='sim2real-bad-name!', platformId='AWSLambda-SHA384-ECDSA'); error code asserted as ValidationException and matched:
  AssertionError: expected message 'Profile name must contain only alphanumeric characters and underscore.', got "1 validation error detected: Value 'sim2real-bad-name!' at 'profileName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9_]{2,}$"

Cause 2 — signer.sign-payload-payload-too-large-rejected (mismatch), SignPayload with a 4097-byte payload; code ValidationException matched:
  AssertionError: expected message 'Payload does not meet size requirements: [1B - 4096B]', got "1 validation error detected: Value '4097 bytes' at 'payload' failed to satisfy constraint: Member must have length less than or equal to 4096"

Cause 3 — signer.add-profile-permission-stale-revision-conflict-rejected (mismatch), AddProfilePermission(statementId='sim2real-stmt-b', revisionId='stalerev0') after a first grant; code ConflictException matched:
  AssertionError: expected message 'Specified revisionId (stalerev0) does not match the revisionId of the existing policy (<current revision id>)', got 'revision id stalerev0 is not the current revision of the permissions of profile <profileName>'
  (the recorded run's values were the minted revision 00000000-0000-0002-0000-000000000001 and profile sim2real_signer_stalerev_<suffix>; both are per-run values filling the template, not part of it)

Cause 4 — signer.add-profile-permission-add-and-remove-round-trip (mismatch): add one statement, RemoveProfilePermission it with the current revisionId, then ListProfilePermissions on the same profile:
  AssertionError: expected ResourceNotFoundException for an empty policy, got {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'revisionId': '00000000-0000-0002-0000-000000000002', 'policySizeBytes': 39, 'permissions': []}
  expected instead: HTTP 404, Error.Code ResourceNotFoundException (body code 'PolicyNotFound'), message 'No policies associated with profile <profileName>'.

Cause 5 — signer.start-signing-job-client-request-token-idempotent (mismatch): 'Check timed out after 60.0s'. The check's only polling loop is _poll_until(lambda: _list_jobs_for_profile(signer, account_id, name), len(jobs) >= 1, timeout_s=60.0), where _list_jobs_for_profile calls signer.list_signing_jobs(jobInvoker=<caller account id>, maxResults=25). Source grounding for why that loop can never terminate: start_signing_job.rs inserts the job with `job_invoker: None` (commented "a job on this one account's own profile has none"), and state.rs JobListing::includes drops any job whose recorded job_invoker does not equal the filter (`kept(self.job_invoker.as_deref(), job.job_invoker.as_deref())`, where kept() requires recorded == Some(filter)). Every ListSigningJobs call carrying jobInvoker therefore returns jobs: [].

Model facts used (smithy_query on services/service-com-amazonaws-signer-wallabyservice/smithy-model/com-amazonaws-signer-wallabyservice.json): ProfileName @length 2..64 @pattern ^[a-zA-Z0-9_]{2,}$; Payload blob @length 1..4096; ValidationException errorKind client httpError 400 members {message, code}; ConflictException httpError 409 members {message, code}; ResourceNotFoundException httpError 404; SigningJob.jobInvoker AccountId @length 12..12 @pattern ^[0-9]{12}$ "The AWS account ID of the job invoker"; ListSigningJobs.jobInvoker "Filters results to return only signing jobs initiated by a specified IAM entity".

## Steps to Reproduce
1. Clone the repo and build/boot the emulator from mainline.
2. Replay the simparity-behavioral-aws-v1 suite against it (service signer).
3. Cause 1: run check signer.put-signing-profile-invalid-name-pattern-rejected — PutSigningProfile with profileName 'sim2real-bad-name!' and platformId AWSLambda-SHA384-ECDSA; compare Error.Message against 'Profile name must contain only alphanumeric characters and underscore.'
4. Cause 2: run check signer.sign-payload-payload-too-large-rejected — PutSigningProfile on Notation-OCI-SHA384-ECDSA, then SignPayload with a 4097-byte payload; compare Error.Message against 'Payload does not meet size requirements: [1B - 4096B]'.
5. Cause 3: run check signer.add-profile-permission-stale-revision-conflict-rejected — AddProfilePermission once, then AddProfilePermission again with revisionId='stalerev0'; compare Error.Message against 'Specified revisionId (stalerev0) does not match the revisionId of the existing policy (<current revision>)'.
6. Cause 4: run check signer.add-profile-permission-add-and-remove-round-trip — AddProfilePermission, RemoveProfilePermission with the returned revisionId, then ListProfilePermissions on the same profile; expect 404 ResourceNotFoundException / 'No policies associated with profile <profileName>', observe 200 with permissions: [].
7. Cause 5: run check signer.start-signing-job-client-request-token-idempotent — create a versioned source bucket and a destination bucket, StartSigningJob twice with the same clientRequestToken, then call ListSigningJobs(jobInvoker=<caller account id>, maxResults=25); observe jobs: [] (the check then burns its 60s budget polling) instead of the one deduplicated job.
