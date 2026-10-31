Issue-Source: trace-sweep-divergence
Issue-Trace: appsync-api-key-lifecycle
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appsync-api-key-lifecycle appsync-resource-tagging appsync-resource-tagging-workflow
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync|CreateApiKey|2xx->2xx|$.apiKey.deletes
Issue-Key: appsync|ListApiKeys|2xx->2xx|$.apiKeys[id="da2-dq5e45sgwr3mamypsk3zabjbfx"].deletes
Issue-Key: appsync|UpdateApiKey|4xx->4xx|$.message
Issue-Key: appsync|UpdateApiKey|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: appsync|UpdateApiKey|4xx->4xx|status
Issue-Key: appsync|UpdateApiKey|2xx->2xx|$.apiKey.deletes
Issue-Key: appsync|DeleteApiKey|4xx->4xx|$.message
Issue-Key: appsync|TagResource|4xx->4xx|$.message
Issue-Key: appsync|TagResource|2xx->2xx|status
Issue-Key: appsync|UntagResource|2xx->2xx|status
Issue-Key: appsync|ListTagsForResource|4xx->4xx|$.message
Issue-Key: appsync|ListTagsForResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Commit: e22fc32f4dd53ee37fe3b4cc7fa000d8c822ec8a

# AppSync API-key deletion window and tagging responses diverge from AWS (10 causes)

> _Note: this issue was identified against the code base at commit `e22fc32f4dd53ee37fe3b4cc7fa000d8c822ec8a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApiKey

## Description
Two neighbouring surfaces diverge: the ApiKey deletion grace period is computed with the wrong offset on every API-key path and UpdateApiKey mis-classifies an unsupported key version, and the tagging operations answer the wrong HTTP status and emulator-authored refusal sentences. 10 net-new causes.

Cause 1 of 10 — CreateApiKey returns the wrong ApiKey$deletes value.
AWS: ApiKey$deletes is "the time after which the API key is deleted", seconds since the epoch rounded down to the hour (Smithy: ApiKey.deletes, target Long). AWS derives it from the key's expiry plus the service's fixed deletion grace period; for a given expires value the deletes value is therefore determined, not arbitrary. The recorded response shows deletes sitting a fixed interval beyond the emulator's value (expected 1803945600 vs got 1798761600 — a 60-day offset where the emulator applies a shorter one).
Emulator: computes deletes with a different (shorter) grace period than AWS.
Why: a client that schedules key rotation or cleanup off deletes acts on a date that does not exist on AWS, and keys it believes are already reclaimed are still live (or vice versa).
Fix: the API-key handler's expiry/deletion arithmetic (the deletes derivation shared by Create/Update/ListApiKeys).
Traces: appsync-api-key-lifecycle#task#3.

Cause 2 of 10 — ListApiKeys reports the same wrong deletes.
AWS: ListApiKeys returns expiration and deletion time in seconds for da2 keys (Smithy: ApiKey docs).
Emulator: the listed key's deletes carries the same wrong offset.
Why: the wrong value is also what inventory/listing clients see.
Fix: same derivation as cause 1, observed through ListApiKeys.
Traces: appsync-api-key-lifecycle#task#4.

Cause 3 of 10 — UpdateApiKey on a key version it cannot update answers NotFoundException instead of BadRequestException.
AWS: 400 BadRequestException, x-amzn-errortype BadRequestException, fixed message "Unsupported API key version." — AWS documents that UpdateApiKey is not available for da1 keys (Smithy: ApiKey docs "UpdateApiKey is not available for this key version"; UpdateApiKey errors include BadRequestException httpError 400 and NotFoundException httpError 404). The message is entirely fixed text.
Emulator: 404 NotFoundException "API key not found.".
Why: a client distinguishing "this key cannot be updated" (permanent, 4xx-bad-request) from "this key does not exist" takes the wrong branch, and retry logic keyed on NotFound will create a replacement key that was never needed.
Fix: UpdateApiKey validation — classify the key version before the existence lookup emits a not-found.
Traces: appsync-api-key-lifecycle#task#5.

Cause 4 of 10 — UpdateApiKey's success reply carries the wrong deletes.
AWS: the updated ApiKey's deletes follows the same derivation as cause 1.
Emulator: same wrong offset.
Fix: same derivation as cause 1.
Traces: appsync-api-key-lifecycle#task#6.

Cause 5 of 10 — DeleteApiKey's not-found message omits the key id.
AWS: 404 NotFoundException with the message template "API key not found: <id>", where the fixed text is "API key not found: " and the variable part is the api-key id taken from the request path (recorded behavior).
Emulator: "API key not found." — a different fixed sentence with no variable part at all.
Why: the refusal no longer identifies which key was missing, which is the only diagnostic a batch delete has.
Fix: DeleteApiKey handler's not-found refusal message template.
Traces: appsync-api-key-lifecycle#task#10.

Cause 6 of 10 — TagResource's reserved-tag-key refusal is emulator-authored.
AWS: 400 BadRequestException with the fixed sentence "Invalid tag key. Customers cannot set tag keys starting with aws: (case insensitive)" (recorded behavior; Smithy: TagResource errors include BadRequestException httpError 400). No part of it varies with the request.
Emulator: a different, longer sentence that embeds the offending key and recites the whole tag-key grammar ("Invalid tag key <key>: a key must be between 1 and 128 characters long, cannot begin with aws:, ...").
Why: clients and tests asserting AWS's wording diverge, and the emulator conflates the reserved-prefix rule with the general tag-key grammar in one message.
Fix: TagResource validation — a dedicated reserved-prefix refusal with AWS's fixed sentence.
Traces: appsync-resource-tagging#task#2.

Cause 7 of 10 — TagResource answers HTTP 200 where AWS answers 204.
AWS: 204 with an empty body. Smithy: TagResourceResponse has no members (the model's http code trait is 200, but the recorded wire response is 204 — recorded behavior takes precedence for the status).
Emulator: 200.
Why: a client or proxy that treats 204 as "no content" and 200 as "body follows" reads differently; status-code assertions fail.
Fix: the TagResource serializer's success status.
Traces: appsync-resource-tagging#task#3, appsync-resource-tagging#task#5, appsync-resource-tagging-workflow#task#2.

Cause 8 of 10 — UntagResource answers HTTP 200 where AWS answers 204.
AWS: 204, empty body (Smithy: UntagResourceResponse has no members; recorded behavior for the status).
Emulator: 200.
Fix: the UntagResource serializer's success status.
Traces: appsync-resource-tagging#task#7, appsync-resource-tagging-workflow#task#4.

Cause 9 of 10 — ListTagsForResource on a deleted API uses the wrong not-found sentence.
AWS: 404 NotFoundException with the fixed text "Resource was not found" — the tagging surface refuses by resource, not by API (recorded behavior; Smithy: ListTagsForResource errors include NotFoundException httpError 404).
Emulator: "API not found.".
Why: the tagging surface's refusal text differs from AWS's, and leaks that the emulator resolves the ARN as an API rather than as a tagged resource.
Fix: ListTagsForResource handler's not-found refusal message.
Traces: appsync-resource-tagging#teardown#12, appsync-resource-tagging-workflow#teardown#8.

Cause 10 of 10 — ListTagsForResource with a malformed resourceArn answers ValidationException and a constraint dump instead of BadRequestException.
AWS: 400 BadRequestException with the fixed sentence "Invalid input resource arn" (recorded behavior; Smithy: ListTagsForResource errors include BadRequestException httpError 400 — there is no ValidationException on this operation).
Emulator: 400 with x-amzn-errortype ValidationException and a generated "N validation errors detected: ..." dump of the ResourceArn @length (min 70, max 75) and @pattern constraints.
Why: the error code is not one the operation models, so a client's error-type dispatch falls through to its unknown-error branch rather than handling a bad ARN.
Fix: ListTagsForResource validation — the ARN check must raise the modeled BadRequestException with AWS's sentence instead of the generic constraint-violation renderer.
Traces: appsync-resource-tagging-workflow#task#6.

Related open issue that does NOT cover these: "AppSync tagging operations are not gated by identity-based IAM..." (20261027T222245Z) — that is an authorization gap and carries none of these replay keys.

## Evidence
Replay of simparity-wire-aws-v1.

1) appsync-api-key-lifecycle#task#3 CreateApiKey 200: $.apiKey.deletes expected 1803945600, got 1798761600.
2) appsync-api-key-lifecycle#task#4 ListApiKeys 200: $.apiKeys[...].deletes expected 1803945600, got 1798761600.
3) appsync-api-key-lifecycle#task#5 UpdateApiKey: status expected 400 got 404; $.message expected 'Unsupported API key version.' got 'API key not found.'; x-amzn-errortype expected 'BadRequestException' got 'NotFoundException'.
4) appsync-api-key-lifecycle#task#6 UpdateApiKey 200: $.apiKey.deletes expected 1803945600, got 1798761600.
5) appsync-api-key-lifecycle#task#10 DeleteApiKey 404: $.message expected 'API key not found: <id>' got 'API key not found.'
6) appsync-resource-tagging#task#2 TagResource 400: expected 'Invalid tag key. Customers cannot set tag keys starting with aws: (case insensitive)', got 'Invalid tag key aws:reserved: a key must be between 1 and 128 characters long, cannot begin with aws:, and can only contain letters, numbers, spaces and the characters + - = . _ : /'.
7) appsync-resource-tagging#task#3 TagResource: status expected 204, got 200.
8) appsync-resource-tagging#task#7 UntagResource: status expected 204, got 200.
9) appsync-resource-tagging#teardown#12 ListTagsForResource 404: expected 'Resource was not found', got 'API not found.'
10) appsync-resource-tagging-workflow#task#6 ListTagsForResource 400: expected 'Invalid input resource arn' with a shaped errortype, got x-amzn-errortype 'ValidationException' and "2 validation errors detected: ... at 'resourceArn' failed to satisfy constraint: Member must have length less than or equal to 75; ... must satisfy regular expression pattern ...".

## Steps to Reproduce
1. Clone the repo, boot the emulator built from it, replay simparity-wire-aws-v1.
2. Fixtures appsync-api-key-lifecycle, appsync-resource-tagging and appsync-resource-tagging-workflow mismatch at the steps under Traces.
3. By hand: CreateApiKey with an expires value and compare deletes (causes 1-2, 4); UpdateApiKey against a da1-version key (cause 3); DeleteApiKey with an unknown id (cause 5); TagResource with key 'aws:reserved' (cause 6); TagResource/UntagResource successfully and check the status code (causes 7-8); ListTagsForResource on a deleted API (cause 9) and with an ARN whose resource type is not apis/ (cause 10).
