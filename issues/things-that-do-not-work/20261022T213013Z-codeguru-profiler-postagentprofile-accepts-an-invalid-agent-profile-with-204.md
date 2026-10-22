Issue-Source: trace-sweep-divergence
Issue-Trace: codeguruprofiler-agent-report-workflow
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codeguruprofiler-agent-report-workflow
Issue-Shape-Id: com.amazonaws.codeguruprofiler#CodeGuruProfiler
Issue-Key: codeguru-profiler|PostAgentProfile|4xx->2xx|$
Issue-Key: codeguru-profiler|PostAgentProfile|4xx->2xx|status
Issue-Key: codeguru-profiler|GetProfile|4xx->2xx|$.__type
Issue-Key: codeguru-profiler|GetProfile|4xx->2xx|$.message
Issue-Key: codeguru-profiler|GetProfile|4xx->2xx|status
Issue-Commit: bd3308b0c8d992d57414e8fe0564c68280e702c4

# PostAgentProfile accepts an invalid agent profile with 204 and GetProfile then serves it instead of 404

> _Note: this issue was identified against the code base at commit `bd3308b0c8d992d57414e8fe0564c68280e702c4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PostAgentProfile

## Description
Two wire-replay causes, one fault and its downstream consequence: the emulator performs NO validation of the agent-profile payload, so a profile AWS refuses is stored and then served. Both causes must be closed before this issue is resubmitted.

=== Cause 1 of 2: PostAgentProfile returns 204 success where AWS refuses with 400 ValidationException ===
AWS: Smithy PostAgentProfile is POST /profilingGroups/{profilingGroupName}/agentProfile with success code 204; required members `agentProfile` (blob, @httpPayload) and `contentType` (Content-Type header); modeled errors include ValidationException (@error client, httpError 400, required `message`). Recorded behavior: AWS parses the submitted body as an agent profile and refuses a structurally invalid one with HTTP 400, error code `ValidationException` — the recorded example message is 'Invalid agent profile: Profile must contain a callgraph field..', i.e. the refusal names the missing required field of the profile document. The 400/ValidationException pair is the requirement; the message text is an example of the rule "a profile missing a required field is refused and the field is named".
Emulator: answers 204 with an empty body — the payload is stored verbatim with no parsing or validation.
Why: an agent (or any client) that relies on the service rejecting a malformed submission gets silent acceptance, so a broken profiler agent never learns its upload is unusable, and the group's profiling state is advanced from data AWS would never have accepted. A 400 error class becoming a 204 success is an overly-permissive data-plane divergence that no client-side error handling can detect.
Fix: PostAgentProfile's handler/validation — parse the `agentProfile` payload and return the modeled ValidationException (400) for a profile missing a required field (here, `callgraph`), WITHOUT mutating state.
Traces: codeguruprofiler-agent-report-workflow#task#3.

=== Cause 2 of 2: GetProfile returns 200 with a profile built from the rejected submission, where AWS answers 404 ResourceNotFoundException ===
AWS: Smithy GetProfile is GET /profilingGroups/{profilingGroupName}/profile with success 200 and modeled ResourceNotFoundException (@error client, httpError 404, required `message`). Recorded behavior: because the only submission for this group was refused by Cause 1, no aggregated profile exists in the requested window, so AWS answers 404 with code `ResourceNotFoundException` and its fixed no-data text ('No profiles found for the requested time range!' in the recording).
Emulator: answers 200 and returns a profile payload, with neither `message` nor `__type` in the body — it is serving the very submission AWS refused.
Why: this is the state consequence of Cause 1 and is why Cause 1 matters beyond the status code: a client sees profile data that never existed on AWS, so any workflow that gates on "has this group received a usable profile yet?" takes the wrong branch, and the modeled 404 path is unreachable in the emulator for a group whose only upload was invalid. Fixing Cause 1 so the refused profile is not stored is a prerequisite; the no-data 404 path must also be reachable on its own.
Fix: PostAgentProfile's state write (do not record a refused profile) together with GetProfile's no-data path, which must return 404 ResourceNotFoundException when no aggregated profile covers the requested range.
Traces: codeguruprofiler-agent-report-workflow#task#5.

Related already-open behavioral issue: "CodeGuru Profiler: includeDescription does not switch the ListProfilingGroups response shape, PostAgentProfile accepts payloads AWS refuses..." (20261022T185439Z-codeguruprofiler-codeguru-profiler-includedescription-does-not-switch-the-listprofilinggroups) reports missing PostAgentProfile payload validation for two OTHER payloads (a bare symbol under text/plain and `{}` under application/json) from the behavioral corpus. It does not carry either wire replay key below, so these two causes — including the missing-callgraph case and the GetProfile state consequence — are filed here with the keys that exercise them.

## Evidence
Both steps are in fixture codeguruprofiler-agent-report-workflow, which creates a profiling group and submits one agent profile.

Cause 1 — step task#3, PostAgentProfile:
  expected status 400, actual 204
  $: empty_body — expected {'message': 'Invalid agent profile: Profile must contain a callgraph field..', '__type': 'ValidationException'}, got {}

Cause 2 — step task#5, GetProfile:
  expected status 404, actual 200
  $.message: missing — expected 'No profiles found for the requested time range!', got None
  $.__type: missing — expected 'ResourceNotFoundException', got None

The requirements are the statuses (400, 404) and the error codes (ValidationException, ResourceNotFoundException); the quoted messages are examples from the recording.

## Steps to Reproduce
1. Boot the emulator and point a codeguru-profiler client at it.
2. CreateProfilingGroup(profilingGroupName=<new>, clientToken=uuid4()).
3. Cause 1: PostAgentProfile(profilingGroupName=<group>, contentType='application/json', agentProfile=<a JSON agent profile with no `callgraph` field>) — the emulator answers 204; AWS answers 400 ValidationException.
4. Cause 2: GetProfile(profilingGroupName=<group>, period='PT5M') — the emulator answers 200 with the profile stored in step 3; AWS answers 404 ResourceNotFoundException because the submission was refused.
5. Or replay fixture codeguruprofiler-agent-report-workflow steps task#3 and task#5.
