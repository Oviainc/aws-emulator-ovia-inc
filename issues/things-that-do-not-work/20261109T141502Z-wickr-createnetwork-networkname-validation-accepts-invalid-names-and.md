Issue-Source: trace-sweep-divergence
Issue-Trace: wickr-network-create-and-validation
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: wickr-network-create-and-validation wickr-network-lifecycle-and-not-found-errors
Issue-Shape-Id: com.amazonaws.wickr#WickrAdminApi
Issue-Key: wickr|CreateNetwork|4xx->2xx|$.reasons
Issue-Key: wickr|CreateNetwork|4xx->2xx|$headers.content-type
Issue-Key: wickr|CreateNetwork|4xx->2xx|$headers.etag
Issue-Key: wickr|CreateNetwork|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: wickr|CreateNetwork|4xx->2xx|status
Issue-Key: wickr|CreateNetwork|4xx->4xx|$.message
Issue-Key: wickr|CreateNetwork|4xx->4xx|$.reasons[*].reason
Issue-Key: wickr|CreateNetwork|4xx->4xx|$headers.content-type
Issue-Key: wickr|CreateNetwork|4xx->4xx|$headers.etag
Issue-Commit: 22941ee6d43df66d29bcadd7a62b7906e5868e84

# wickr: CreateNetwork networkName validation accepts invalid names and words its ValidationError wrongly

> _Note: this issue was identified against the code base at commit `22941ee6d43df66d29bcadd7a62b7906e5868e84`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateNetwork

## Description
Two net-new causes in CreateNetwork's networkName validation: one invalid-name class is accepted outright, and the class the emulator does refuse is refused with the wrong body. A fixer must close BOTH before submitting.

Cause 1 of 2: CreateNetwork accepts a networkName AWS refuses, creating a network instead of returning 422.
  AWS: recorded behavior — the model constrains networkName only by @pattern ^[\S\s]*$ (which matches anything), so the name rule is not expressible from the model; the recording shows AWS refusing the request with status 422, the modeled ValidationError (Smithy: ValidationError, @error client, @httpError 422, members `reasons` (ErrorDetailList) and `message`), an x-amzn-errortype header, and body {"reasons": [{"field": "networkName", "reason": "Network Name is not a valid name"}]} and NO top-level `message`. In that body "networkName" is the rejected input member's name and "Network Name is not a valid name" is fixed text — neither part echoes the submitted value.
  Emulator: status 200 — the name passes every check it applies and the network is created. Its only networkName rule is the length cap derived from the member documentation (services/service-com-amazonaws-wickr-wickradminapi/src/state.rs line 31, NETWORK_NAME_MAX_CHARS = 20), so a name that is short enough but not a valid Wickr network name is accepted.
  Why: a client relying on AWS to reject an invalid network name gets a created network with a name AWS would never have minted, and its negative test sees a success; all downstream reads then report a network state AWS cannot produce.
  Fix: CreateNetwork's input validation in the wickr service crate — the networkName rule needs the validity check AWS applies, not only a length cap — and the ValidationError it raises.
  Traces: wickr-network-create-and-validation#task#4.

Cause 2 of 2: when the emulator DOES refuse a networkName, the ValidationError body is worded as a generic constraint failure and adds a `message` AWS does not send.
  AWS: for this error class the recorded 422 body carries reasons[0].field = "networkName" and reasons[0].reason = the fixed text "Network Name is not a valid name" — the same text regardless of which name rule was broken and regardless of the submitted value's length — and carries no top-level `message` member.
  Emulator: 422 with reasons[0].reason = "networkName must be between 1 and 20 characters, got <n>" (its own constraint wording, interpolating its cap and the submitted length) plus a top-level $.message = "1 validation error detected" that AWS omits for this class.
  Why: a client that matches on the reason text to tell the user what is wrong with the name, or that treats presence of top-level `message` as a different error shape, parses a body AWS never serves — so the status matches while the error payload does not.
  Fix: the ValidationError construction/serializer for CreateNetwork's networkName refusal (reason text, and suppressing the generic envelope `message` for this class).
  Traces: wickr-network-lifecycle-and-not-found-errors#task#4.

Both responses also omit the content-type charset and the ETag header; that service-wide header defect is filed as its own cause in the wickr write-path envelope issue. No open wickr issue covers either cause here.

## Evidence
Cause 1 — wickr-network-create-and-validation#task#4 (CreateNetwork): expected status 422, got 200; $.reasons missing (expected [{'field': 'networkName', 'reason': 'Network Name is not a valid name'}], got None); $headers.x-amzn-errortype missing (expected present, got None).
Cause 2 — wickr-network-lifecycle-and-not-found-errors#task#4 (CreateNetwork, 422 both sides): $.reasons[0].reason value_mismatch (expected 'Network Name is not a valid name', got 'networkName must be between 1 and 20 characters, got 42'); $.message unexpected_present (expected None, got '1 validation error detected').

## Steps to Reproduce
1. POST /networks with accessLevel STANDARD and a networkName that is within 20 characters but is not a valid Wickr network name (the name class the fixture submits): AWS answers 422 with reasons [{field: networkName, reason: 'Network Name is not a valid name'}] and an x-amzn-errortype header; the emulator answers 200 and creates the network.
2. POST /networks with a networkName longer than the name limit: both answer 422, but AWS's reasons[0].reason is 'Network Name is not a valid name' with no top-level message, while the emulator returns its own length wording plus message '1 validation error detected'.
3. Replay fixtures wickr-network-create-and-validation (task#4) and wickr-network-lifecycle-and-not-found-errors (task#4).
