Issue-Source: trace-sweep-divergence
Issue-Trace: acm-account-configuration-expiry-events
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: acm-account-configuration-expiry-events acm-account-configuration-expiry-events-update acm-account-configuration-roundtrip acm-get-baseline-account-configuration acm-account-expiry-notification-configuration acm-imported-certificate-options-and-listing
Issue-Shape-Id: com.amazonaws.acm#CertificateManager
Issue-Key: acm|GetAccountConfiguration|2xx->5xx|status
Issue-Key: acm|GetAccountConfiguration|2xx->5xx|$.ExpiryEvents
Issue-Commit: 30db2a8818f1221c3278e244a1430bae2ca83b41

# GetAccountConfiguration is not served: HTTP 501 instead of 200 with ExpiryEvents

> _Note: this issue was identified against the code base at commit `30db2a8818f1221c3278e244a1430bae2ca83b41`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAccountConfiguration

## Description
ACM's account-level expiry-event configuration cannot be read from the emulator at all: every GetAccountConfiguration call answers HTTP 501. Two causes, distinguished by what the caller needed from the response.

Cause 1 of 2 - baseline read of the account configuration fails.
AWS: Smithy GetAccountConfiguration takes Unit input and returns GetAccountConfigurationResponse{ExpiryEvents: ExpiryEventsConfiguration}; its only modeled errors are AccessDeniedException (403) and ThrottlingException (400). A successful call is HTTP 200. The recording shows 200 for the setup read.
Emulator: HTTP 501 (operation not served), which is not a modeled outcome for this operation.
Why: a client reading the account's expiry configuration - including every test or tool that snapshots it before changing it - fails outright on a status the SDK cannot map to a modeled ACM error, so it surfaces as an unknown/protocol error rather than as data.
Fix: implement the GetAccountConfiguration handler in the ACM service crate and back it with account-configuration state.

Cause 2 of 2 - read-back after PutAccountConfiguration fails, so DaysBeforeExpiry is unobservable.
AWS: the recorded read-back returns 200 with $.ExpiryEvents={'DaysBeforeExpiry': 30}, i.e. the value previously written by PutAccountConfiguration is reflected.
Emulator: HTTP 501 and $.ExpiryEvents absent.
Why: the Put/Get round trip that proves the configuration was stored cannot be completed; a client has no way to confirm or read the expiry-notification window. This also blocks six replayed steps across the account-configuration and imported-certificate fixtures.
Fix: same handler, plus persisting ExpiryEvents.DaysBeforeExpiry written by PutAccountConfiguration into service state so Get reflects it.

Traces: acm-account-configuration-expiry-events#setup#1 (representative; cluster also covers acm-account-configuration-expiry-events-update#setup#1, acm-account-configuration-roundtrip#setup#1, acm-get-baseline-account-configuration#setup#1), acm-account-configuration-expiry-events-update#task#3 (cluster also covers acm-account-configuration-roundtrip#task#3 and #task#5, acm-account-expiry-notification-configuration#task#1, acm-get-baseline-account-configuration#task#3, acm-imported-certificate-options-and-listing#task#6).

## Evidence
acm-account-configuration-expiry-events#setup#1: status expected 200, got 501. Cluster affected_count 4.
acm-account-configuration-expiry-events-update#task#3: status expected 200, got 501; $.ExpiryEvents missing (expected {'DaysBeforeExpiry': 30}, got None). Cluster affected_count 6.
No stub_path and no owning open issue were reported for either cluster.
Smithy: GetAccountConfiguration input Unit, output GetAccountConfigurationResponse{ExpiryEvents: ExpiryEventsConfiguration}, errors AccessDeniedException(403) and ThrottlingException(400) only - 501 is not a modeled response.

## Steps to Reproduce
1. Replay the ACM wire corpus against the emulator.
2. Call GetAccountConfiguration with no parameters -> emulator answers 501 instead of 200 (acm-account-configuration-expiry-events#setup#1).
3. Call PutAccountConfiguration with ExpiryEvents.DaysBeforeExpiry=30 and a valid idempotency token, then GetAccountConfiguration -> still 501, and $.ExpiryEvents is absent instead of {'DaysBeforeExpiry': 30} (acm-account-configuration-expiry-events-update#task#3).
