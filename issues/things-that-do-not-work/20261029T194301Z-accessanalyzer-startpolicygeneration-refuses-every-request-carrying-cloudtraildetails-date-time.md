Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: accessanalyzer.start-policy-generation-completes-then-cancel-terminal-rejected
Issue-Shape-Id: com.amazonaws.accessanalyzer#AccessAnalyzer
Issue-Key: accessanalyzer.start-policy-generation-completes-then-cancel-terminal-rejected|run#22|AssertionError|assert msg2 == (
Issue-Commit: a2b95246df63384d1b9b9d3ee28f05a727c934df

# StartPolicyGeneration refuses every request carrying cloudTrailDetails: date-time timestamps fail to deserialize, and an unknown cloudTrailArn is never validated

> _Note: this issue was identified against the code base at commit `a2b95246df63384d1b9b9d3ee28f05a727c934df`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartPolicyGeneration

## Description
REOPENS 20261023T110741Z-accessanalyzer-startpolicygeneration-is-a-501-scaffold-stub-so-the.md: its fix left the call refused. StartPolicyGeneration is no longer a 501 stub and its missing-cloudTrailDetails rejection is now correct (that assertion passes), but every request that DOES carry a cloudTrailDetails is still refused — now with HTTP 400 ValidationException and the message "invalid timestamp: failed to parse int" — so no policy-generation job can be minted and the whole jobId lifecycle (GetGeneratedPolicy, CancelPolicyGeneration on a real job) remains unreachable.

Two causes are bundled here; both are in the accessanalyzer StartPolicyGeneration request path and both must be closed. Cause 2 is MASKED by Cause 1 — the check cannot observe it until Cause 1 is fixed — but the same check asserts it, so a fix for Cause 1 alone will not make the check pass.

Cause 1 of 2: the cloudTrailDetails timestamps are deserialized as epoch-seconds integers, so AWS's documented date-time wire form is rejected.
AWS: Smithy (services/service-com-amazonaws-accessanalyzer-accessanalyzer/smithy-model/com-amazonaws-accessanalyzer-accessanalyzer.json): StartPolicyGeneration is PUT /policy/generation, success 200 with required output member jobId; its input member cloudTrailDetails targets CloudTrailDetails, whose members startTime (required) and endTime both target Timestamp AND carry @timestampFormat("date-time"). An AWS SDK therefore serializes them as RFC-3339 strings (e.g. "2026-10-22T11:07:41Z"), not as epoch seconds, and real AWS accepts that body: with a cloudTrailDetails naming a real trail the call returns 200 and a jobId (the check's positive control), and with a trail that does not exist it returns the trail-specific rejection of Cause 2 — in neither case does it complain about the timestamps.
Emulator: the request is refused at the input boundary with HTTP 400, error code ValidationException and message "invalid timestamp: failed to parse int". That message is not produced by any path in the operation's handler (services/service-com-amazonaws-accessanalyzer-accessanalyzer/src/ops/start_policy_generation.rs, whose refusals read "Missing cloudTrailDetails", "cloudTrailDetails.trails must name at least one trail to analyze", "cloudTrailDetails.endTime must be at or after cloudTrailDetails.startTime"), so the handler is never entered. Hypothesis: the shared restJson1 JSON deserializer parses every timestamp member as an epoch-seconds number and ignores the member-level @timestampFormat("date-time") trait, so the SDK's RFC-3339 string fails an integer parse.
Why: StartPolicyGeneration is unusable from any AWS SDK — every SDK emits the date-time form the model demands, so a client that supplies a perfectly valid cloudTrailDetails gets a 400 naming a timestamp it never mis-formatted, cannot mint a jobId, and cannot reach GetGeneratedPolicy/CancelPolicyGeneration/ListPolicyGenerations at all. Any other operation in this service with a date-time-formatted timestamp member is broken the same way.
Fix: the input deserialization for this operation — the shared Smithy restJson1 timestamp deserializer must honour the member's @timestampFormat (date-time => RFC-3339 string) for CloudTrailDetails.startTime/endTime rather than defaulting every timestamp to epoch-seconds; if the generated member schema for CloudTrailDetails is what drops the trait, that schema generation is the defect. No valid request should be able to reach the handler-independent "invalid timestamp" refusal.
Traces: accessanalyzer.start-policy-generation-completes-then-cancel-terminal-rejected (the second StartPolicyGeneration call of the check, the nonexistent-trail case; the same body shape is reused by the positive control that follows).

Cause 2 of 2: an unknown cloudTrailArn is accepted instead of being refused with AWS's trail-existence ValidationException.
AWS: Smithy models ValidationException (HTTP 400) for StartPolicyGeneration; the recorded behavior is that AWS resolves each Trail.cloudTrailArn (pattern ^arn:[^:]*:cloudtrail:[^:]*:[^:]*:trail/.{1,576}$) through cloudtrail:GetTrail SYNCHRONOUSLY, before minting a job, and refuses a trail that does not exist with HTTP 400 ValidationException and the message template "Invalid cloudTrailArn: {cloudTrailArn}. Unknown trail: {trailName} for the user: {accountId}". Fixed text: "Invalid cloudTrailArn: ", ". Unknown trail: ", " for the user: ". Variable parts, all taken from the request and the caller's identity: {cloudTrailArn} is the arn exactly as sent, {trailName} is the trail name component of that arn (the part after "trail/"), {accountId} is the calling account. No job is created, so nothing is left to poll or cancel. The paired positive control proves this is trail-specific and not a blanket refusal: the identical call against a trail the caller really created is accepted with 200 + jobId.
Emulator: the handler validates only that cloudTrailDetails exists, that trails is non-empty and that endTime >= startTime (start_policy_generation.rs lines 169-213); it never resolves a cloudTrailArn against any trail registry, and then mints and stores a job unconditionally. A made-up trail arn would therefore be ACCEPTED with 200 + jobId once Cause 1 is fixed.
Why: a client that mistypes or references a deleted trail gets a jobId instead of the 400 that tells it to fix the request, then polls a job that can never analyze anything; validation-driven control flow (fix-the-request vs. wait-for-the-job) takes the wrong branch, and error-message-keyed handling never matches.
Fix: trail-existence validation in the StartPolicyGeneration handler, raising the modeled ValidationException with the message template above before any job is written to state.
Traces: accessanalyzer.start-policy-generation-completes-then-cancel-terminal-rejected (same check; this is the assertion the check fails on, at `assert msg2 == (...)`).

## Evidence
Check accessanalyzer.start-policy-generation-completes-then-cancel-terminal-rejected [mismatch]. The check reached the emulator and was answered; the earlier assertions in the same check pass (case 1: missing cloudTrailDetails => 400 ValidationException "Missing cloudTrailDetails"; and code2 == "ValidationException").

Failing assertion (check file line 109):
  assert msg2 == (
      f"Invalid cloudTrailArn: {fake_trail_arn}. Unknown trail: {role_name}-notrail for the user: {account_id}"
  ), msg2
  AssertionError: invalid timestamp: failed to parse int

Request that produced it (StartPolicyGeneration, PUT /policy/generation):
  policyGenerationDetails={"principalArn": <role arn>}
  cloudTrailDetails={"accessRole": <role arn>, "startTime": <datetime, now-1d>, "endTime": <datetime, now>,
                     "trails": [{"cloudTrailArn": <arn:aws:cloudtrail:...:trail/...-notrail>, "allRegions": True}]}
botocore serializes startTime/endTime as RFC-3339 strings because the model marks both @timestampFormat("date-time").

Expected (recorded AWS): HTTP 400, code ValidationException, message "Invalid cloudTrailArn: <the arn sent>. Unknown trail: <trail name from that arn> for the user: <calling account id>" — fixed text "Invalid cloudTrailArn: " / ". Unknown trail: " / " for the user: ", with the arn, trail name and account id all coming from the request and caller identity. And for a trail that DOES exist: HTTP 200 with a jobId.
Actual (emulator): HTTP 400, code ValidationException, message "invalid timestamp: failed to parse int" — a boundary deserialization failure, not any refusal the handler can emit. Consequently the positive control (same body, real trail) can never return a jobId either, and the SUCCEEDED-then-cancel and never-issued-jobId assertions that follow are unreachable.

Model evidence: CloudTrailDetails.startTime {target: Timestamp, required: true, timestampFormat: "date-time"}; CloudTrailDetails.endTime {target: Timestamp, timestampFormat: "date-time"}.
Handler evidence: services/service-com-amazonaws-accessanalyzer-accessanalyzer/src/ops/start_policy_generation.rs lines 169-213 are the only validation (presence of cloudTrailDetails, non-empty trails, endTime >= startTime); lines 254-283 mint and store the job with no trail lookup.

## Steps to Reproduce
1. Boot the emulator and point an accessanalyzer client (any AWS SDK, e.g. botocore) at it.
2. Call StartPolicyGeneration with policyGenerationDetails={'principalArn': '<an existing role arn>'} and cloudTrailDetails={'accessRole': '<that role arn>', 'startTime': <a datetime>, 'endTime': <a later datetime>, 'trails': [{'cloudTrailArn': 'arn:aws:cloudtrail:<region>:<account>:trail/does-not-exist', 'allRegions': True}]}.
   Emulator: HTTP 400 ValidationException "invalid timestamp: failed to parse int".
   Real AWS: HTTP 400 ValidationException "Invalid cloudTrailArn: arn:aws:cloudtrail:<region>:<account>:trail/does-not-exist. Unknown trail: does-not-exist for the user: <account>", and no job created.
3. Repeat step 2 with a cloudTrailArn of a trail that really exists (CloudTrail CreateTrail first).
   Real AWS: HTTP 200 with a jobId that GetGeneratedPolicy/CancelPolicyGeneration then track.
   Emulator: the same HTTP 400 "invalid timestamp: failed to parse int" — the request never reaches the handler.
4. After the timestamp deserialization is fixed, repeat step 2: the emulator mints a jobId for the nonexistent trail instead of refusing it (Cause 2).
