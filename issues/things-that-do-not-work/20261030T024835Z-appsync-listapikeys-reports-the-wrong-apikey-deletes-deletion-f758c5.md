Issue-Source: trace-sweep-divergence
Issue-Trace: appsync-api-key-lifecycle
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appsync-api-key-lifecycle
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync|ListApiKeys|2xx->2xx|$.apiKeys[id="da2-fkih6glzydpyrbc3fa2ioppmc2"].deletes
Issue-Commit: 351eb6a8cf8c77d5b7b02e416baa64b0c1bc3489

# AppSync ListApiKeys reports the wrong ApiKey deletes deletion timestamp

> _Note: this issue was identified against the code base at commit `351eb6a8cf8c77d5b7b02e416baa64b0c1bc3489`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListApiKeys

## Description
1 net-new cause.

Cause 1 of 1 - ListApiKeys projects an ApiKey$deletes value derived with the wrong deletion grace period.

AWS: Smithy: ApiKey.deletes, target Long, default 0 - "The time after which the API key is deleted. The date is represented as seconds since the epoch, rounded down to the nearest hour." The ApiKey doc further states that for the current (da2) key version "ListApiKeys returns the expiration time and deletion time in seconds". AWS derives deletes from the key's expires value plus the service's fixed deletion grace period, so for a given expires the deletes value is fully determined and must be identical on CreateApiKey, UpdateApiKey and ListApiKeys. In the recording the listed key's deletes sits a fixed interval beyond the value the emulator computes - a longer grace period than the emulator applies. The variable part of the value is the key's own expires timestamp from the create request; the grace period added to it is the fixed part of the rule.

Emulator: ListApiKeys returns the key with a deletes value computed from a shorter grace period than AWS uses (recorded step: expected 1803945600, got 1798761600 - those are one run's values, the requirement is the derivation, not the literal numbers). The status is 200 and the rest of the ApiKey projection matches; only deletes diverges.

Why: a client that inventories keys with ListApiKeys and schedules rotation, alerting or cleanup off deletes computes a reclamation date that does not exist on AWS. It will believe keys are already reclaimed while they are still live on AWS (or re-create keys it did not need to), and it will see ListApiKeys disagree with the deletes it was handed at CreateApiKey time if only one of the two paths is corrected.

Fix: the AppSync API-key state/handler's deletes derivation - the shared expiry-plus-grace-period arithmetic that CreateApiKey, UpdateApiKey and ListApiKeys all project; ListApiKeys must read the same derived value, not recompute it with a different offset.

Traces: appsync-api-key-lifecycle#task#4 (ListApiKeys, 200 vs 200, $.apiKeys[id=...].deletes).

Related open issue that does NOT cover this step: "AppSync API-key deletion window and tagging responses diverge from AWS (10 causes)" (20261029T200818Z-appsync-api-key-deletion-window-and-tagging-responses.md) describes the same deletes derivation for CreateApiKey/UpdateApiKey and carries a ListApiKeys replay key, but that key embeds a per-run api-key id (da2-dq5e45sgwr3mamypsk3zabjbfx) that does not match this run's listed key (da2-fkih6glzydpyrbc3fa2ioppmc2), so appsync-api-key-lifecycle#task#4 is NOT gated by it and the replay of that issue's fix never exercises this step. Fixing the shared derivation should close both; this issue exists so the ListApiKeys projection is re-replayed.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from the checkout.

Fixture appsync-api-key-lifecycle, step task#4, operation ListApiKeys:
  verdict: mismatch
  expected status 200, actual status 200
  detail: $.apiKeys[id="da2-fkih6glzydpyrbc3fa2ioppmc2"].deletes: value_mismatch (expected 1803945600, got 1798761600)
  divergence_key: service=appsync|ListApiKeys|status=200->200|keys=$.apiKeys[id="da2-fkih6glzydpyrbc3fa2ioppmc2"].deletes
  owned_by: (none) - no open issue carries this replay key
  affected_count: 1

Expected (AWS): the listed da2 key's deletes equals its expires plus AWS's fixed deletion grace period, in seconds since the epoch rounded down to the hour, and equals the deletes CreateApiKey returned for the same key.
Actual (emulator): deletes is the same key's expires plus a shorter grace period, so it is earlier than AWS's value by a fixed interval.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from that checkout.
2. Replay the simparity-wire-aws-v1 wire corpus; fixture appsync-api-key-lifecycle mismatches at step task#4 (ListApiKeys).
3. By hand: CreateApiKey on a GraphQL API with an explicit expires value, note the returned apiKey.deletes, then call ListApiKeys for the same API and compare the listed key's deletes against the AWS-recorded value for that expires (expires + AWS's fixed deletion grace period, truncated to the hour). The emulator's value is earlier than AWS's by a fixed interval.
