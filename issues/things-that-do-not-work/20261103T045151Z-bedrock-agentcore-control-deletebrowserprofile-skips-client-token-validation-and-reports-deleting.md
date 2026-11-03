Issue-Source: trace-sweep-divergence
Issue-Trace: acc-browser-profile-name-scoped-listing
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: acc-browser-profile-name-scoped-listing bedrock-agentcore-control-browser-profile-duplicate-and-notfound bedrock-agentcore-control-browser-profile-lifecycle
Issue-Shape-Id: com.amazonaws.bedrockagentcorecontrol#AmazonBedrockAgentCoreControl
Issue-Key: bedrock-agentcore-control|DeleteBrowserProfile|4xx->2xx|$.message
Issue-Key: bedrock-agentcore-control|DeleteBrowserProfile|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: bedrock-agentcore-control|DeleteBrowserProfile|4xx->2xx|status
Issue-Key: bedrock-agentcore-control|DeleteBrowserProfile|2xx->2xx|$.status
Issue-Commit: 19755eee811c5d924ea257c82351bf2373edae9f

# DeleteBrowserProfile skips client-token validation and reports DELETING instead of DELETED

> _Note: this issue was identified against the code base at commit `19755eee811c5d924ea257c82351bf2373edae9f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteBrowserProfile

## Description
The emulator's DeleteBrowserProfile path diverges from AWS on both of its ends: it accepts a delete request AWS refuses for a blank client token, and when it does delete it reports a transitional status where AWS reports the terminal one. 2 causes must both be fixed before this issue is closed.

The open issue 20261031T201128Z-bedrock-agentcore-control-code-interpreter-and-browser-profile-lifecycle-statuses-and.md covers the browser-profile CREATE conflict message and the code-interpreter lifecycle statuses; it does not cover DeleteBrowserProfile's client-token validation or DeleteBrowserProfile's response status, which are the causes below. The open issue 20261030T221315Z-bedrock-agentcore-control-browser-lifecycle-statuses-post-delete-reads-and-conflict.md covers the BROWSER resource, not browser profiles.

Cause 1 of 2: DeleteBrowserProfile accepts a request with a blank/absent client token instead of refusing it.
AWS: DeleteBrowserProfile (Smithy: DELETE /browser-profiles/{profileId}, HTTP 200; input member `clientToken` -> ClientToken, @httpQuery("clientToken"), @idempotencyToken, @length min 33 max 256, @pattern ^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,256}$; modeled errors include ValidationException, errorKind client, httpError 400, members message/reason). The recording shows AWS refusing a browser-profile deletion whose clientToken query parameter is present but blank with HTTP 400, x-amzn-errortype: ValidationException, and the message "Client token cannot be null or blank for browser profile deletion". That whole sentence is fixed text for this error class — it has no variable parts taken from the request or state. (The model marks clientToken optional; the requirement that a supplied token be non-blank is recorded behavior for this operation.)
Emulator: answers HTTP 200 with a normal DeleteBrowserProfile success body — no `message`, no x-amzn-errortype header — i.e. it performs the deletion the service rejects.
Why: a client that relies on AWS's validation to catch an unset/blank idempotency token silently destroys a browser profile against the emulator while the same call is a no-op 400 against AWS; error-handling branches keyed on 400/ValidationException are never exercised, and idempotency-retry logic built on the token is validated against behavior that does not exist in the real service.
Fix: DeleteBrowserProfile's input validation (the clientToken query-parameter check), before the handler mutates browser-profile state, emitting ValidationException/400 with the sentence above.
Traces: acc-browser-profile-name-scoped-listing#task#5.

Cause 2 of 2: DeleteBrowserProfile reports the transitional status DELETING where AWS reports the terminal DELETED.
AWS: DeleteBrowserProfile output member `status` -> BrowserProfileStatus, required (Smithy enum values READY | DELETING | DELETED | SAVING). The recording shows AWS answering HTTP 200 with `status` DELETED — the deletion is complete when the call returns, so no DELETING phase is observable to the caller on this response.
Emulator: answers HTTP 200 (status code matches) but serializes `status` as "DELETING".
Why: a client that treats DELETED as the completion signal never sees it, so teardown waiters poll until they time out or report the delete as incomplete, and assertions that the delete response is terminal fail.
Fix: the DeleteBrowserProfile handler's state transition / status projection for the browser-profile resource (set the terminal state on successful delete rather than emitting the in-progress one).
Traces: acc-browser-profile-name-scoped-listing#teardown#6, and the same cluster's bedrock-agentcore-control-browser-profile-duplicate-and-notfound#teardown#5 and bedrock-agentcore-control-browser-profile-lifecycle#teardown#3.

## Evidence
Cause 1 — fixture acc-browser-profile-name-scoped-listing, step task#5, DeleteBrowserProfile, expected 400 / actual 200. Replay detail: "status: expected 400, got 200; $.message: missing (expected 'Client token cannot be null or blank for browser profile deletion', got None); $headers.x-amzn-errortype: missing (expected 'ValidationException', got None)". The expected message is entirely fixed text for this error class; nothing in it comes from the request or from emulator state.

Cause 2 — fixture acc-browser-profile-name-scoped-listing, step teardown#6, DeleteBrowserProfile, expected 200 / actual 200. Replay detail: "$.status: value_mismatch (expected 'DELETED', got 'DELETING')". Cluster members showing the same fault: bedrock-agentcore-control-browser-profile-duplicate-and-notfound#teardown#5, bedrock-agentcore-control-browser-profile-lifecycle#teardown#3. The HTTP status matched in this cause; the divergence is the status enum value in the body.

## Steps to Reproduce
1. Replay the wire corpus against the emulator (replay_wire_corpus).
2. Cause 1: fixture acc-browser-profile-name-scoped-listing, step task#5 — issue DELETE /browser-profiles/{profileId} with a blank clientToken query parameter and compare: HTTP 400, header x-amzn-errortype: ValidationException, body $.message == "Client token cannot be null or blank for browser profile deletion". The emulator answers 200 with a success body.
3. Cause 2: fixture acc-browser-profile-name-scoped-listing, step teardown#6 — issue a valid DeleteBrowserProfile for an existing profile and compare the 200 body's $.status against DELETED. The emulator returns DELETING. Same check reproduces at bedrock-agentcore-control-browser-profile-duplicate-and-notfound#teardown#5 and bedrock-agentcore-control-browser-profile-lifecycle#teardown#3.
