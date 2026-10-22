Issue-Source: trace-sweep-divergence
Issue-Trace: accessanalyzer-analyzer-lifecycle
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: accessanalyzer-analyzer-lifecycle
Issue-Shape-Id: com.amazonaws.accessanalyzer#AccessAnalyzer
Issue-Key: access-analyzer|TagResource|2xx->2xx|status
Issue-Key: access-analyzer|UntagResource|2xx->2xx|status
Issue-Commit: c73ebf67a23d923b166243bec836d38257c16c68

# Access Analyzer empty-output tag operations answer 200 instead of 204 No Content

> _Note: this issue was identified against the code base at commit `c73ebf67a23d923b166243bec836d38257c16c68`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
Both of IAM Access Analyzer's tag-mutation operations have an empty modeled output and AWS answers them with 204 No Content; the emulator answers 200 for both. This is a single serializer/status-selection fault in the REST-JSON response path for operations whose output structure has no members: the status is being taken from the Smithy default (200) rather than from the service's actual empty-body response. 2 causes.

Cause 1 of 2: TagResource (fixture accessanalyzer-analyzer-lifecycle, step task#4).
AWS: Smithy: TagResource http {method: POST, uri: /tags/{resourceArn}}, output shape TagResourceResponse has NO members and the model's http code is defaulted (codeDefaulted: true), so the model does not state a status; recorded behavior only - the recording shows AWS answering 204 with an empty body.
Emulator: answers 200 (status: expected 204, got 200).
Why: a client or SDK middleware that distinguishes a no-content response from a content-bearing one (conditional body parsing, proxies and caches that key on 204, test assertions on the response status) sees a 200 that promises a body the emulator does not send; code paths gated on 204 never run against the emulator and then behave differently against AWS.
Fix: the status selection for empty-output operations in TagResource's response serializer (the REST-JSON serializer must emit 204 for an output structure with no members, not the defaulted 200).
Traces: accessanalyzer-analyzer-lifecycle#task#4.

Cause 2 of 2: UntagResource (fixture accessanalyzer-analyzer-lifecycle, step task#6).
AWS: Smithy: UntagResource http {method: DELETE, uri: /tags/{resourceArn}}, output shape UntagResourceResponse has NO members, http code defaulted; recorded behavior only - the recording shows AWS answering 204 with an empty body.
Emulator: answers 200 (status: expected 204, got 200).
Why: same as cause 1 - the untag call reports a content-bearing success where AWS reports no content.
Fix: same component, UntagResource's response serializer / shared empty-output status selection.
Traces: accessanalyzer-analyzer-lifecycle#task#6.

## Evidence
Replay of trace set simparity-wire-aws-v1 (9 fixtures, 66 steps):
- accessanalyzer-analyzer-lifecycle#task#4 TagResource: expected status 204, actual status 200. divergence_key service=access-analyzer|TagResource|status=204->200|keys=status, verdict mismatch.
- accessanalyzer-analyzer-lifecycle#task#6 UntagResource: expected status 204, actual status 200. divergence_key service=access-analyzer|UntagResource|status=204->200|keys=status, verdict mismatch.
No stub_path and no owning issue on either cluster; no state_reset_failures in the run, so neither step ran against stale state.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay fixture accessanalyzer-analyzer-lifecycle.
2. Step task#4 POSTs /tags/{resourceArn} (TagResource) on the analyzer created earlier in the fixture; observe the response status is 200 where the recording has 204 with an empty body.
3. Step task#6 DELETEs /tags/{resourceArn}?tagKeys=... (UntagResource) on the same analyzer; observe the response status is 200 where the recording has 204.
4. Both steps pass once the empty-output responses carry 204.
