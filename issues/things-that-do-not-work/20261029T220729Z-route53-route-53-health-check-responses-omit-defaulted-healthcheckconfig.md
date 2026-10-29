Issue-Source: trace-sweep-divergence
Issue-Trace: route53-duplicate-caller-reference-idempotent-and-conflict
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: route53-duplicate-caller-reference-idempotent-and-conflict route53-health-check-duplicate-caller-reference route53-health-check-not-found-errors route53-healthcheck-status-error-paths route53-tcp-and-calculated-health-checks-isolation route53-health-check-calculated-type-status-rejected
Issue-Shape-Id: com.amazonaws.route53#AWSDnsV20130401
Issue-Key: route53|CreateHealthCheck|2xx->2xx|$.CreateHealthCheckResponse.HealthCheck.HealthCheckConfig.Disabled
Issue-Key: route53|CreateHealthCheck|2xx->2xx|$.CreateHealthCheckResponse.HealthCheck.HealthCheckConfig.EnableSNI
Issue-Key: route53|CreateHealthCheck|2xx->2xx|$.CreateHealthCheckResponse.HealthCheck.HealthCheckConfig.Inverted
Issue-Key: route53|CreateHealthCheck|2xx->2xx|$.CreateHealthCheckResponse.HealthCheck.HealthCheckConfig.MeasureLatency
Issue-Key: route53|CreateHealthCheck|2xx->2xx|$.CreateHealthCheckResponse.HealthCheck.HealthCheckConfig.ChildHealthChecks
Issue-Key: route53|GetHealthCheck|2xx->2xx|$.GetHealthCheckResponse.HealthCheck.HealthCheckConfig.ChildHealthChecks
Issue-Key: route53|GetHealthCheck|2xx->2xx|$.GetHealthCheckResponse.HealthCheck.HealthCheckConfig.Disabled
Issue-Key: route53|GetHealthCheck|2xx->2xx|$.GetHealthCheckResponse.HealthCheck.HealthCheckConfig.Inverted
Issue-Commit: a9b33a473a9762dcd2804e6444a9b0695dc27129

# Route 53 health-check responses omit defaulted HealthCheckConfig members

> _Note: this issue was identified against the code base at commit `a9b33a473a9762dcd2804e6444a9b0695dc27129`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateHealthCheck

## Description
Route 53 materialises HealthCheckConfig's optional boolean members (and the CALCULATED child-check list) when it stores a health check, and serialises them on every read of that check. The emulator stores and serialises only what the request named, so the defaulted members are absent from the response envelope. Which members go missing depends on the check type, which is why this shows up as four clusters. 4 causes.

Cause 1 of 4 — CreateHealthCheck for an HTTPS/str-match style endpoint check.
AWS: Smithy: HealthCheckConfig.MeasureLatency, .Inverted, .Disabled and .EnableSNI are boolean members of the config; CreateHealthCheck's output requires HealthCheck (success code 201). AWS fills each unspecified boolean with the service default and serialises it: MeasureLatency, Inverted and Disabled render as `false`, and EnableSNI renders as `true` for the TLS-bearing check types, whose documentation describes SNI as sent unless disabled.
Emulator: omits all four elements from $.CreateHealthCheckResponse.HealthCheck.HealthCheckConfig.
Why: a client reading the created config cannot tell whether inversion/latency-measurement/SNI are on or off, and XML deserialisers that map absent to a language default may invert the SNI meaning (absent -> false, where AWS says true).
Fix: CreateHealthCheck's handler and the HealthCheckConfig serializer — normalise the config with its type-dependent defaults at store time and always serialise the boolean members.
Traces: route53-duplicate-caller-reference-idempotent-and-conflict#task#2 (cluster also covers route53-health-check-duplicate-caller-reference#setup#1, route53-health-check-not-found-errors#task#1, route53-healthcheck-status-error-paths#task#1, route53-tcp-and-calculated-health-checks-isolation#setup#1).

Cause 2 of 4 — CreateHealthCheck for a check type with no TLS/latency surface.
AWS: same model members. For a check type where MeasureLatency/EnableSNI do not apply, AWS still serialises Inverted and Disabled as `false`.
Emulator: omits both elements.
Why: same as cause 1 — the inversion and enabled state of a freshly created check are unreadable.
Fix: same serializer change as cause 1.
Traces: route53-health-check-calculated-type-status-rejected#setup#1.

Cause 3 of 4 — CreateHealthCheck for a CALCULATED check.
AWS: Smithy: HealthCheckConfig.ChildHealthChecks targets ChildHealthCheckList (@length 0..256) and is the member that names a CALCULATED check's children. AWS serialises Inverted and Disabled as `false` and emits the ChildHealthChecks container element even when it holds no children (an empty element).
Emulator: omits Inverted, Disabled and ChildHealthChecks entirely.
Why: a client cannot distinguish "this calculated check has no children" from "this response does not report children", so child-set reconciliation has nothing to compare against.
Fix: same serializer change, plus emitting the empty ChildHealthChecks container for CALCULATED checks.
Traces: route53-tcp-and-calculated-health-checks-isolation#setup#2.

Cause 4 of 4 — GetHealthCheck read-back of a CALCULATED check.
AWS: Smithy: GetHealthCheck returns the same HealthCheck shape, so the read-back carries the identical normalised config — Inverted `false`, Disabled `false`, and the ChildHealthChecks container.
Emulator: omits the same three elements, confirming the gap is in the stored config and the shared serializer, not in CreateHealthCheck's response assembly alone.
Why: read-after-write of a health check does not round-trip the configuration AWS reports.
Fix: the HealthCheckConfig serializer / health-check state shared by CreateHealthCheck and GetHealthCheck.
Traces: route53-tcp-and-calculated-health-checks-isolation#task#5.

## Evidence
Replay of trace set simparity-wire-aws-v1 — statuses matched (201/200) and only body members diverged:
- route53-duplicate-caller-reference-idempotent-and-conflict#task#2 CreateHealthCheck (cluster of 5 steps): HealthCheckConfig.MeasureLatency missing (expected 'false'), .Inverted missing (expected 'false'), .Disabled missing (expected 'false'), .EnableSNI missing (expected 'true').
- route53-health-check-calculated-type-status-rejected#setup#1 CreateHealthCheck: .Inverted missing (expected 'false'), .Disabled missing (expected 'false').
- route53-tcp-and-calculated-health-checks-isolation#setup#2 CreateHealthCheck: .Inverted missing (expected 'false'), .Disabled missing (expected 'false'), .ChildHealthChecks missing (expected empty element).
- route53-tcp-and-calculated-health-checks-isolation#task#5 GetHealthCheck: .Inverted, .Disabled and .ChildHealthChecks all missing with the same expectations.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay the simparity-wire-aws-v1 wire corpus and inspect the four steps under Traces.
3. Individually: POST /2013-04-01/healthcheck with a HealthCheckConfig that omits MeasureLatency, Inverted, Disabled and EnableSNI — the response's HealthCheckConfig contains none of them.
4. Create a CALCULATED health check and GET /2013-04-01/healthcheck/{id} — Inverted, Disabled and the ChildHealthChecks container are all absent.
