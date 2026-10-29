Issue-Source: trace-sweep-divergence
Issue-Trace: appsync-api-key-lifecycle
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appsync-api-key-lifecycle
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync|ListApiKeys|2xx->2xx|$.apiKeys[id="da2-lvxu3wg6ctkwunme6xp74apzas"].deletes
Issue-Commit: 89c27cbcf326b5c1a1abaeca904759cc614d9072

# AppSync ListApiKeys projects the wrong ApiKey deletes grace period

> _Note: this issue was identified against the code base at commit `89c27cbcf326b5c1a1abaeca904759cc614d9072`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListApiKeys

## Description
Cause 1 of 1: ListApiKeys returns a da2 API key whose `deletes` value is derived with a shorter deletion grace period than AWS uses, so every listed key reports a deletion time that is a fixed interval too early.

AWS: `ApiKey$deletes` is "The time after which the API key is deleted. The date is represented as seconds since the epoch, rounded down to the nearest hour." (Smithy: com.amazonaws.appsync#ApiKey member `deletes`, target Long, default 0; the shape doc states that for da2 keys "ListApiKeys returns the expiration time and deletion time in seconds"). AWS derives `deletes` from the key's `expires` value plus the service's fixed deletion grace period, so for a given `expires` the `deletes` value is determined rather than arbitrary; the model does not state the length of that window, so the window itself is recorded behavior only. The recording shows the listed key's `deletes` sitting a fixed interval (60 days' worth of seconds) beyond the value the emulator computes, i.e. AWS keeps a da2 key for a longer grace window after expiry than the emulator does. The requirement is the relationship between `expires` and `deletes`, not the literal epoch seconds of any one run (those depend entirely on when the key was created).

Emulator: ListApiKeys serializes `deletes` using its own, shorter grace period, so the listed value is the expiry plus a too-small offset. Replay of appsync-api-key-lifecycle#task#4 reports 200/200 with `$.apiKeys[id="da2-..."].deletes` value_mismatch: expected 1803945600, got 1798761600 — a 5,184,000-second (60-day) shortfall against the same key's recorded value.

Why: a client that reads `deletes` off the key inventory to schedule rotation, cleanup or alarms acts on a reclamation date that does not exist on AWS. It believes keys are already reclaimed while AWS still serves them (so it provisions replacements that were never needed), and any assertion that `deletes` equals `expires` plus the AWS grace window fails against the emulator.

Fix: the ListApiKeys handler's API-key projection — it must emit the same expiry/deletion arithmetic AWS uses (the `deletes` derivation shared with CreateApiKey/UpdateApiKey) rather than recomputing a shorter window on the read path.

Traces: appsync-api-key-lifecycle#task#4 (ListApiKeys, 200 -> 200, $.apiKeys[id=...].deletes).

Related open issue that does NOT carry this replay key: "AppSync API-key deletion window and tagging responses diverge from AWS (10 causes)" (20261029T200818Z-appsync-api-key-deletion-window-and-tagging-responses.md). Its cause 2 describes the same ListApiKeys defect, but its recorded Issue-Key names a per-run API-key id (`$.apiKeys[id="da2-dq5e45sgwr3mamypsk3zabjbfx"].deletes`) which no longer matches this run's generated key id, so this replay step is not covered by that issue's pointer and would be closed unexercised. Fixing the shared derivation named there resolves this step too; this issue exists so the ListApiKeys step is actually re-replayed.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout.

appsync-api-key-lifecycle#task#4, operation ListApiKeys, expected status 200, actual status 200 (verdict: mismatch, owned_by: none, stub_path: none).
  detail: $.apiKeys[id="da2-lvxu3wg6ctkwunme6xp74apzas"].deletes: value_mismatch (expected 1803945600, got 1798761600)
  divergence_key: service=appsync|ListApiKeys|status=200->200|keys=$.apiKeys[id="da2-..."].deletes

Expected (AWS): deletes == the key's expires plus AWS's fixed da2 deletion grace window, seconds since epoch rounded down to the hour.
Actual (emulator): deletes == expires plus a shorter window; 1798761600 vs 1803945600, i.e. 5,184,000 s (60 days) early.
The epoch values above are this run's instance of the template; the invariant under test is the offset between expires and deletes, not those literals. Status (200), body shape and all other members of the listed key matched.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from that checkout.
2. Replay the simparity-wire-aws-v1 wire corpus; fixture appsync-api-key-lifecycle mismatches at step task#4.
3. By hand: CreateGraphqlApi with API_KEY auth, CreateApiKey with an explicit `expires` value, then call ListApiKeys for that api id.
4. Compare the listed key's `deletes` against its `expires`: the difference is the emulator's grace window, which is 60 days shorter than the window AWS applies for the same `expires` (recorded step shows 1798761600 where AWS returned 1803945600).
