Issue-Source: trace-sweep-divergence
Issue-Trace: appsync-api-key-lifecycle
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appsync-api-key-lifecycle
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync|ListApiKeys|2xx->2xx|$.apiKeys[id="da2-bvrqc7egtlk5ngqdij5mtun3qe"].deletes
Issue-Commit: 80128cf5220e52d7fa07858b1a17d4e0aeba15d4

# AppSync ListApiKeys understates ApiKey deletes by the 60-day deletion grace window

> _Note: this issue was identified against the code base at commit `80128cf5220e52d7fa07858b1a17d4e0aeba15d4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListApiKeys

## Description
1 cause.

Cause 1 of 1: ListApiKeys projects each listed API key's ApiKey$deletes using a deletion grace window shorter than the one AWS applies, so every key in the listing reports a deletion time that is a fixed interval earlier than the real one, and disagrees with the deletes the emulator itself returned from CreateApiKey/UpdateApiKey for the same key.

AWS: Smithy (services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice) ListApiKeys -> http GET /v1/apis/{apiId}/apikeys, code 200, output apiKeys: ApiKeys (list of ApiKey); errors BadRequestException(400), NotFoundException(404), UnauthorizedException(401), InternalFailureException(500). ApiKey$deletes is documented as "The time after which the API key is deleted. The date is represented as seconds since the epoch, rounded down to the nearest hour." AWS derives deletes deterministically from the key's expires plus the service's fixed post-expiry deletion grace period (AppSync keeps an expired key for 60 days before reclaiming it), floored to the hour. The requirement is therefore a derivation, not a literal: for any key, ListApiKeys MUST report exactly the deletes the service computed for it — the same value CreateApiKey and UpdateApiKey returned for that key.

Emulator: status matches (200 on both sides); only the member value diverges. For the key listed in the replayed step the emulator reported a deletes exactly 5,184,000 seconds (60 days) short of the recorded AWS value (expected 1803945600, got 1798761600). Those two epoch-second integers and the generated key id are this run's values filling the template; the defect is the systematic 60-day shortfall in the grace-window arithmetic used when projecting the list, which also makes the list disagree with the Create/Update projections of the same key.

Why: a client that enumerates keys with ListApiKeys and schedules rotation, re-issue or cleanup off deletes computes a deletion date that does not exist on AWS. It believes keys have already been reclaimed while they are still live on AWS (or re-issues early), and an inventory reconciled against AWS never converges. The same client also sees two different deletes for one key depending on whether it read it from CreateApiKey or from ListApiKeys.

Fix: the API-key handler's deletes derivation as projected by the ListApiKeys handler/serializer — the expiry-plus-grace-period arithmetic must be shared with CreateApiKey/UpdateApiKey so all three agree on one value per key, using AWS's 60-day post-expiry window floored to the hour.

Related open issues that do NOT own this replay key: "AppSync API-key deletion window and tagging responses diverge from AWS" (20261029T200818Z-appsync-api-key-deletion-window-and-tagging-responses.md), "AppSync ListApiKeys projects the wrong ApiKey deletes grace period" (20261029T214854Z-...), "AppSync ListApiKeys reports the wrong ApiKey deletes value" (20261029T222256Z-...) and "AppSync ListApiKeys reports the wrong ApiKey deletes deletion-window value" (20261029T234317Z-...). Each describes the same arithmetic, but their recorded Issue-Keys pin a per-run generated api-key id inside the asserted JSON path ($.apiKeys[id="da2-..."].deletes), so each new replay generates a fresh id and this divergence is reported as owned by nothing. It is filed here so the replay key is carried; a fixer landing the shared derivation fix closes all of them and should re-replay this key to confirm.

Traces: appsync-api-key-lifecycle#task#4 (ListApiKeys).

## Evidence
Replay of simparity-wire-aws-v1 (emulator_up true, /__admin/health answered 200, data listener accepting). 91 fixtures replayed, score 60.3%.

Cause 1 — appsync-api-key-lifecycle#task#4, operation ListApiKeys, verdict mismatch, expected status 200 / actual status 200:
  $.apiKeys[id="da2-bvrqc7egtlk5ngqdij5mtun3qe"].deletes: value_mismatch (expected 1803945600, got 1798761600)

Divergence key: service=appsync|ListApiKeys|status=200->200|keys=$.apiKeys[id="da2-bvrqc7egtlk5ngqdij5mtun3qe"].deletes; affected_count 1; stub_path empty; owned_by empty (no open issue carries this replay key).

Expected vs actual, stated generally: expected the listed key's deletes to be the service-computed deletion time (the key's expires plus AWS's fixed 60-day post-expiry deletion grace window, rounded down to the nearest hour — the identical value Create/UpdateApiKey returned for that key); actual is that value minus exactly the 60-day window (a 5,184,000-second shortfall). The api-key id and both epoch-second numbers above are this run's generated/clock-derived values, not part of the requirement; the fixed 60-day offset is.

## Steps to Reproduce
1. git clone the repository and boot the emulator built from that checkout.
2. Replay the simparity-wire-aws-v1 wire corpus. Fixture appsync-api-key-lifecycle mismatches at step task#4 (ListApiKeys) on $.apiKeys[...].deletes.
3. By hand: CreateGraphqlApi; CreateApiKey with an explicit expires; note the returned apiKey.deletes. Then GET /v1/apis/{apiId}/apikeys (ListApiKeys) for the same apiId and compare the listed key's deletes with both (a) the value CreateApiKey returned and (b) expires plus AWS's 60-day post-expiry deletion window floored to the hour. The emulator's listed value is short by exactly 5,184,000 seconds.
