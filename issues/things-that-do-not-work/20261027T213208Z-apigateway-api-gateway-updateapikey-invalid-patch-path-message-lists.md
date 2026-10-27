Issue-Source: trace-sweep-divergence
Issue-Trace: apigw-patch-apikey-description-op-support
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-ga-prod-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigw-patch-apikey-description-op-support
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway|UpdateApiKey|4xx->4xx|$.message
Issue-Commit: dae6e7ecbed2f75cbdde9154518aba02892291c8

# API Gateway UpdateApiKey: invalid-patch-path message lists the replace-supported paths regardless of the op, so 'remove' reports the wrong allowed set

> _Note: this issue was identified against the code base at commit `dae6e7ecbed2f75cbdde9154518aba02892291c8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateApiKey

## Description
One cause.

Cause 1 of 1: UpdateApiKey's patch-path validation is not op-aware. The emulator picks the set of permitted JSON Pointer paths for an API key from a single static list and reports that same list for every op, so a PATCH whose op is not supported for the requested path is refused with a message naming the paths supported by a DIFFERENT op.

AWS: PATCH /apikeys/{apiKey} (UpdateApiKey, input UpdateApiKeyRequest.patchOperations: ListOfPatchOperation; Smithy: PatchOperation members op -> Op enum [add, remove, replace, move, copy, test] and path -> String; modeled refusal BadRequestException, httpError 400). The Smithy model itself does not enumerate which paths each op supports — PatchOperation.op's own documentation states only "Not all valid operations are supported for a given resource" and defers to the Patch Operations reference — so the per-op path sets are documented/recorded behavior only. For an API key, AWS supports `replace` on the scalar members (/description, /enabled, /name, /customerId) and supports `remove` only on the multi-valued members, and when an op/path pair is unsupported it answers 400 BadRequestException whose message enumerates the paths permitted FOR THE OP THAT WAS REQUESTED. The recorded response for op 'remove' on path '/description' is 400 with message "Invalid patch path  '/description' specified for op 'remove'. Must be one of: [/stages, /labels]". Fixed text of that message class: "Invalid patch path  '" (two spaces after "path" — AWS's literal spacing), then "' specified for op '", then "'. Must be one of: [", then "]". Variable parts taken from the request and from the op's own supported-path table: the rejected path ('/description' here) and the op name ('remove' here) come straight from the submitted PatchOperation; the bracketed list is the set of paths supported for THAT op on an API key (for 'remove': /stages, /labels).

Emulator: same status and same error class (400), and the same message template — but the bracketed list is wrong, because it is the op-independent list. Replay reports $.message value_mismatch: expected "Invalid patch path  '/description' specified for op 'remove'. Must be one of: [/stages, /labels]", got "Invalid patch path  '/description' specified for op 'remove'. Must be one of: [/description, /enabled, /name, /customerId]". The emulator has emitted the replace-supported scalar paths while naming op 'remove', so the message contradicts itself: it rejects '/description' and then lists '/description' as one of the permitted paths.

Why: a client relying on AWS's behavior is actively misled rather than merely under-informed. The message is the only machine- and human-readable description of what the caller may do next, and here it tells the caller that '/description' is an allowed path for the op that just rejected it. A caller (or an SDK/CLI/IaC provider surfacing the message, or a retry layer that parses the permitted-path list out of the brackets) will retry the identical op/path pair and be rejected again, or conclude the API is inconsistent; conversely it never learns the paths 'remove' actually accepts (/stages, /labels), which is the information AWS's message exists to convey. Tests that assert on the refusal text for an unsupported op/path pair fail against the emulator even though the status and error class match.

Fix: the UpdateApiKey handler's patch validation. The permitted-path set must be selected per PatchOperation.op (the op/path support table for the ApiKey resource), not from one resource-wide list, and the message's bracketed enumeration must be rendered from the set belonging to the op actually submitted. The same op-awareness applies to the other patchable API Gateway resources whose remove-supported and replace-supported paths differ.

Note: this is NOT covered by the existing open issue "API Gateway: 6 control-plane / convergence causes — UpdateAccount cloudwatchRoleArn patch validation is wrong in three ways..." (20261024T043741Z-apigateway-api-gateway-6-control-plane-convergence-causes-updateaccount.md), which concerns UpdateAccount's cloudwatchRoleArn patch; replay reports no owner for this step.

Traces: apigw-patch-apikey-description-op-support#task#2 (representative) and #task#3 in the same cluster (divergence_key service=apigateway|UpdateApiKey|status=400->400|keys=$.message).

## Evidence
Replay cluster, verdict=mismatch, service=apigateway, operation=UpdateApiKey, fixture apigw-patch-apikey-description-op-support, step task#2 (affected_count=2: task#2, task#3).

Expected status 400, actual status 400 — status and error class agree; the divergence is confined to $.message.

detail: $.message: value_mismatch
  expected: "Invalid patch path  '/description' specified for op 'remove'. Must be one of: [/stages, /labels]"
  actual:   "Invalid patch path  '/description' specified for op 'remove'. Must be one of: [/description, /enabled, /name, /customerId]"

Message anatomy — fixed text: "Invalid patch path  '" / "' specified for op '" / "'. Must be one of: [" / "]". Values from the request: the rejected path and the op name (both echoed correctly by the emulator). Value that is wrong: the bracketed permitted-path list, which must be the set supported for the submitted op ('remove' -> /stages, /labels) but is emitted as the replace-supported scalar set (/description, /enabled, /name, /customerId) — a list that includes the very path being rejected.

Model evidence (smithy_query): UpdateApiKey PATCH /apikeys/{apiKey}, input member patchOperations: ListOfPatchOperation, errors include BadRequestException (client, httpError 400). PatchOperation.op targets enum Op with values [add, remove, replace, move, copy, test]; its doc says "Not all valid operations are supported for a given resource", i.e. the per-op path sets are not modeled and come from AWS's Patch Operations reference and the recorded response.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay fixture apigw-patch-apikey-description-op-support (steps task#2 and task#3).
2. Equivalently, by hand: create an API key, then call UpdateApiKey (PATCH /apikeys/{apiKey}) with a single patchOperations entry whose op is "remove" and whose path is "/description" — a scalar path that API Gateway supports for "replace" but not for "remove".
3. Observe the 400 BadRequestException body's `message`.
4. Expected: the bracketed list enumerates the paths supported for op 'remove' on an API key — "Invalid patch path  '/description' specified for op 'remove'. Must be one of: [/stages, /labels]".
5. Actual: the bracketed list enumerates the replace-supported scalar paths — "... Must be one of: [/description, /enabled, /name, /customerId]" — listing the rejected path '/description' as permitted.
6. Repeat with other ops (e.g. op "add" on a scalar path) to confirm the list does not vary with the op.
