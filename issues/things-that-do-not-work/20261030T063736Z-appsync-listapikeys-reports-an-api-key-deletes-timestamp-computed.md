Issue-Source: trace-sweep-divergence
Issue-Trace: appsync-api-key-lifecycle
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appsync-api-key-lifecycle
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync|ListApiKeys|2xx->2xx|$.apiKeys[id="da2-nahn5wbgnjk4zftcoo7wa3z7ov"].deletes
Issue-Commit: aa75f5d199f9816d8a358732bf51b7bd305abb42

# ListApiKeys reports an API key deletes timestamp computed with the wrong deletion grace period

> _Note: this issue was identified against the code base at commit `aa75f5d199f9816d8a358732bf51b7bd305abb42`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListApiKeys

## Description
1 net-new cause.

Cause 1 of 1: ListApiKeys projects ApiKey$deletes using a shorter deletion grace period than AWS, so every listed da2 API key carries a deletion timestamp that is earlier than the one AWS reports for the same key.

AWS: ApiKey$deletes is "The time after which the API key is deleted. The date is represented as seconds since the epoch, rounded down to the nearest hour." (Smithy: com.amazonaws.appsync#ApiKey member `deletes`, target Long, default 0; sibling member `expires` is documented the same way). The ApiKey docs state that for da2 keys "ListApiKeys returns the expiration time and deletion time in seconds". AWS therefore derives `deletes` deterministically from the key's `expires` value plus the service's fixed post-expiry deletion grace period: for a given `expires`, `deletes` is determined, and it must be the SAME value that CreateApiKey/UpdateApiKey returned for that key. Status is 200; there is no error involved. The exact grace-period length is recorded behavior only: in this recording the listed key's AWS `deletes` sits exactly 5,184,000 seconds (60 days) later than the emulator's, i.e. AWS adds a longer grace window than the emulator does.

Emulator: the same key listed through ListApiKeys comes back with a `deletes` value short of AWS's by that fixed interval (replay: $.apiKeys[id="<api-key-id>"].deletes expected 1803945600, got 1798761600 — the id is a per-run generated value, the offset is not). The status and the rest of the ApiKey projection match; only the derived deletion instant is wrong, and it is wrong by a constant, so this is a wrong constant in the derivation rather than clock skew.

Why: a client that enumerates keys with ListApiKeys and schedules rotation, cleanup or alerting off `deletes` acts on a reclamation date that does not exist on AWS. It will treat keys as already reclaimed while AWS still holds them (or re-create keys that were never going to disappear), and any consistency check that compares a key's `deletes` as listed against the value AWS returned at creation time disagrees.

Fix: the ListApiKeys handler's ApiKey projection must reuse the single shared expiry/deletion derivation for API keys (the same one Create/UpdateApiKey use) with AWS's grace period, rather than recomputing it with a shorter offset; the constant belongs in that one derivation in the API-key state/handler, not per operation.

Related open issue that does NOT cover this replay key: 20261029T200818Z-appsync-api-key-deletion-window-and-tagging-responses.md describes the same deletion-window arithmetic for CreateApiKey/UpdateApiKey and carries a ListApiKeys key bound to a DIFFERENT, per-run api-key id (`$.apiKeys[id="da2-dq5e45sgwr3mamypsk3zabjbfx"].deletes`), so this cluster is not owned by it and its replay never exercises this step. Fixing the shared derivation is expected to close both.

Traces: appsync-api-key-lifecycle#task#4 (ListApiKeys, 200).

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout.

appsync-api-key-lifecycle#task#4 — ListApiKeys, expected status 200, actual status 200, verdict mismatch:
  $.apiKeys[id="da2-nahn5wbgnjk4zftcoo7wa3z7ov"].deletes: value_mismatch (expected 1803945600, got 1798761600)

Expected minus actual = 5,184,000 s = 60 days, a constant offset, not a per-run clock difference. The api-key id in the JSON path is generated per run and is not part of the requirement; the requirement is that `deletes` equals `expires` plus AWS's fixed deletion grace period, and that it equals the `deletes` the same key reported from CreateApiKey/UpdateApiKey.

Smithy (services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice): ApiKey.deletes -> Long, doc "The time after which the API key is deleted. The date is represented as seconds since the epoch, rounded down to the nearest hour."; ApiKey doc, da2 section: "ListApiKeys returns the expiration time and deletion time in seconds."

## Steps to Reproduce
1. Clone the repo and boot the emulator built from that checkout.
2. Replay the simparity-wire-aws-v1 wire corpus; fixture appsync-api-key-lifecycle mismatches at step task#4.
3. By hand: CreateApiKey on a GraphQL API with an explicit `expires` value, note the returned `deletes`, then call ListApiKeys for the same API and compare the listed key's `deletes` against AWS's value for that `expires` (expires + AWS's fixed deletion grace period, rounded down to the hour). The emulator's listed value is short by a constant interval (60 days in the recording).
