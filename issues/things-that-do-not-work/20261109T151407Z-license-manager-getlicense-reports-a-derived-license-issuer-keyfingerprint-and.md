Issue-Source: trace-sweep-divergence
Issue-Trace: license-manager-checkout-checkin-tracks-entitlement-usage
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: license-manager-checkout-checkin-tracks-entitlement-usage
Issue-Shape-Id: com.amazonaws.licensemanager#AWSLicenseManager
Issue-Key: license-manager|GetLicense|2xx->2xx|$.License.CreateTime
Issue-Key: license-manager|GetLicense|2xx->2xx|$.License.Issuer.KeyFingerprint
Issue-Commit: 3d0a54f64a8eacec16ca43902aa47fe37888ae5e

# GetLicense reports a derived License.Issuer.KeyFingerprint and an over-precise CreateTime

> _Note: this issue was identified against the code base at commit `3d0a54f64a8eacec16ca43902aa47fe37888ae5e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetLicense

## Description
The License body GetLicense serves for a license minted by CreateLicense diverges from AWS in two of its members. Both are produced on the same read path (the stored record's wire projection), and both are visible in one recorded step. TWO causes; a fixer must close BOTH before submitting.

Cause 1 of 2: License.Issuer.KeyFingerprint is a SHA-256 digest instead of AWS's fixed-suffix issuer fingerprint.
AWS: Smithy: License.Issuer targets IssuerDetails, whose members are Name, SignKey and KeyFingerprint ('Issuer key fingerprint'); KeyFingerprint is a plain String on the READ shape only (the Issuer shape a CreateLicense carries has Name + SignKey), so the service derives it and the model publishes no pattern for it. Recorded behavior only: the recorded License Manager response renders KeyFingerprint as the colon-delimited form 'aws:<accountId>:<issuer Name>:issuer-fingerprint' — fixed text 'aws:', the caller's account id, the Issuer.Name exactly as the CreateLicense request supplied it, and the literal fixed final segment 'issuer-fingerprint'. Only the account id and the issuer name are variable; the last segment is fixed text, not a per-run value.
Emulator: the final segment is a 64-character lowercase hex SHA-256 digest over a domain tag plus the length-prefixed issuer name and sign key (crate::state::issuer_key_fingerprint, called from crate::state::issuer_details). Replay: $.License.Issuer.KeyFingerprint expected 'aws:000000000000:<issuer name>:issuer-fingerprint', got 'aws:000000000000:<issuer name>:<64 hex>' (that run's issuer name and digest are values filling the template, not part of it).
Why: KeyFingerprint is not decorative — it is half of the (ProductSKU, KeyFingerprint) key CheckoutLicense resolves a license by. Against AWS a client can construct or assert the fingerprint from the account id and the issuer name it created the license with; against the emulator that constructed value resolves no license, so a checkout that works on AWS is refused here, and any assertion or log comparison on the read value fails.
Fix: the issuer-fingerprint derivation in this service's state (crate::state::issuer_key_fingerprint / crate::state::issuer_details), which every path that stores a license issuer goes through — not the GetLicense handler, which only projects the stored record. The derived value must stay stable per issuer identity, which the fixed-suffix form already is.
Traces: license-manager-checkout-checkin-tracks-entitlement-usage#task#3.

Cause 2 of 2: License.CreateTime is rendered with sub-millisecond precision, which the recorded shape does not admit.
AWS: Smithy: License.CreateTime targets ISO8601DateTime (@length 0..50, pattern '^(-?(?:[1-9][0-9]*)?[0-9]{4})-(1[0-2]|0[1-9])-(3[0-1]|0[1-9]|[1-2][0-9])T(2[0-3]|[0-1][0-9]):([0-5][0-9]):([0-5][0-9])(\.[0-9]+)?(Z|[+-]...)+$'), doc 'License creation time'. Recorded behavior only: the recorded response's CreateTime is compared by SHAPE rather than by value (the instant is per-run), and the emulator's rendering fails that shape comparison; the recorded form carries at most millisecond precision (Z-suffixed UTC, three fractional digits or none).
Emulator: crate::state::now_iso8601 formats SystemTime::now with aws_smithy_types Format::DateTime, which emits the full available sub-second precision — six fractional digits, e.g. a value ending '.372014Z'. Replay: $.License.CreateTime expected '<shaped-not-asserted>', got a six-fractional-digit instant.
Hypothesis: the shape comparison rejects the value because of that extra precision (microseconds where AWS renders milliseconds); the instant itself is per-run and is not the divergence.
Why: a client that parses or string-matches CreateTime against the precision real License Manager emits (or round-trips it through a strict ISO8601 reader that accepts only millisecond precision) sees a value AWS never produces, and any golden-response comparison of a license body diverges on every run.
Fix: the timestamp rendering helper in this service's state (crate::state::now_iso8601) must truncate to the precision AWS emits before it is stored in StoredLicense.create_time and projected by License::wire().
Traces: license-manager-checkout-checkin-tracks-entitlement-usage#task#3.

Not covered by any open issue: the one open license-manager wire issue, '20261029T234804Z-license-manager-license-configuration-validation-answers-the-wrong-error-type.md', is entirely about refusal type/wording on the license-CONFIGURATION operations (CreateLicenseConfiguration, GetLicenseConfiguration, UpdateLicenseConfiguration, ListTagsForResource, ListLicenseConfigurations); it says nothing about the License read body served by GetLicense. The other open license-manager issues are against license-manager-linux-subscriptions / license-manager-user-subscriptions operations.

## Evidence
Replay of fixture license-manager-checkout-checkin-tracks-entitlement-usage (trace set simparity-wire-aws-v1), step task#3, operation GetLicense: expected status 200, actual status 200 — the response BODY diverges.

Divergence detail as replayed:
  $.License.CreateTime: value_mismatch (expected '<shaped-not-asserted>', got '2026-11-09T15:11:50.372014Z')
  $.License.Issuer.KeyFingerprint: value_mismatch (expected 'aws:000000000000:sim2real-probe-license-manager-issuer-323fa8e3:issuer-fingerprint', got 'aws:000000000000:sim2real-probe-license-manager-issuer-323fa8e3:6a13a18091edf41e2b766bc8092539cb44d7641be5d8bfbd62ff09f0d49848f4')

In the KeyFingerprint comparison the account id (000000000000) and the issuer name (sim2real-probe-license-manager-issuer-323fa8e3) are values the environment and the CreateLicense request supplied and they MATCH; the divergence is the final segment only — fixed text 'issuer-fingerprint' on AWS versus a 64-hex digest from the emulator. The instant in the CreateTime comparison is per-run; what diverges is its fractional-second precision, not its value.

Model evidence (services/service-com-amazonaws-licensemanager-awslicensemanager/smithy-model/com-amazonaws-licensemanager-awslicensemanager.json): GetLicense input LicenseArn (Arn, required) + Version (String), output License; License.Issuer targets IssuerDetails {Name, SignKey, KeyFingerprint: String}; Issuer (the create shape) has only {Name required, SignKey} — so KeyFingerprint can come from nowhere but the service; License.CreateTime targets ISO8601DateTime with @length 0..50 and the RFC3339-style pattern quoted above.

Code evidence: ops/get_license.rs handle() projects the stored record verbatim (license.wire()); ops/create_license.rs builds the record with issuer: crate::state::issuer_details(input.issuer) and create_time: crate::state::now_iso8601(); src/state.rs issuer_key_fingerprint() formats "aws:{account}:{name}:{hex sha256}" and now_iso8601() formats SystemTime::now() with aws_smithy_types::date_time::Format::DateTime without truncating sub-second precision.

## Steps to Reproduce
Replay fixture license-manager-checkout-checkin-tracks-entitlement-usage from trace set simparity-wire-aws-v1 against the emulator (all steps are in its 'task' phase):
1. The fixture's earlier task steps CreateLicense with an Issuer whose Name is the probe's issuer name and no SignKey.
2. task#3 — GetLicense on the returned LicenseArn. Status is 200 on both sides; compare the body:
   a. $.License.Issuer.KeyFingerprint must be 'aws:' + the account id + ':' + the Issuer.Name the create supplied + ':issuer-fingerprint'. The emulator instead ends the value with a 64-character hex SHA-256 digest.
   b. $.License.CreateTime must be an ISO8601-UTC instant at the precision License Manager emits (Z-suffixed, at most three fractional digits). The emulator emits six fractional digits (microseconds).
3. Secondary check for cause 1: take the AWS-form fingerprint ('aws:<account>:<issuer name>:issuer-fingerprint') and call CheckoutLicense with it plus the license's ProductSKU — AWS resolves the license, the emulator resolves nothing because its stored fingerprint is the digest form.
