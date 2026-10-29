Issue-Source: trace-sweep-divergence
Issue-Trace: rolesanywhere-validation-error-paths
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rolesanywhere-validation-error-paths
Issue-Shape-Id: com.amazonaws.rolesanywhere#RolesAnywhere
Issue-Key: rolesanywhere|CreateProfile|4xx->4xx|$.message
Issue-Key: rolesanywhere|CreateTrustAnchor|4xx->2xx|$.message
Issue-Key: rolesanywhere|CreateTrustAnchor|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: rolesanywhere|CreateTrustAnchor|4xx->2xx|status
Issue-Key: rolesanywhere|PutAttributeMapping|4xx->4xx|$.message
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# rolesanywhere validation refusals render patterns and enum sets wrongly and skip the PCA source check

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateProfile

## Description
Three divergences on the validation surface: the generic constraint-violation message renders the pattern and the enum value set differently from AWS, and CreateTrustAnchor performs no validation of an AWS_ACM_PCA source, accepting a request AWS refuses. Three causes, one per failing replay cluster.

Cause 1 of 3 (CreateProfile, fixture rolesanywhere-validation-error-paths step task#4):
AWS: CreateProfile.name targets ResourceName with @length 1..255 and @pattern ^[ a-zA-Z0-9-_]*$ (Smithy: CreateProfile.name, error ValidationException httpError 400). A name violating the pattern is refused 400 ValidationException whose message is the standard constraint envelope: '1 validation error detected: Value '<submitted value>' at '<member name>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>'. The fixed text is that envelope; the variable parts are the submitted value, the member name ('name') and the pattern - and the pattern is rendered WITHOUT the ^ and $ anchors, i.e. '[ a-zA-Z0-9-_]*'.
Emulator: same status and envelope, but prints the model's raw anchored pattern '^[ a-zA-Z0-9-_]*$'.
Why: the message is a requirement of the error class; a client or test that compares the reported constraint against AWS's text, or a human copying the pattern out of the message to build a valid name, gets a string AWS never emits.
Fix: the shared constraint-violation message renderer used by request validation - strip the leading ^ and trailing $ when rendering a @pattern constraint.
Traces: rolesanywhere-validation-error-paths#task#4.

Cause 2 of 3 (CreateTrustAnchor, fixture rolesanywhere-validation-error-paths step task#5):
AWS: CreateTrustAnchor validates the trust anchor's source before creating anything (Smithy: CreateTrustAnchor.source -> Source {sourceType: TrustAnchorType, sourceData}, errors include ValidationException httpError 400). A source naming an AWS Private CA that the service cannot read - a nonexistent or malformed CA ARN - is refused 400 ValidationException with the fixed message 'Error creating TrustAnchor. Given AWS PCA did not allow get request, or invalid ARN.' (no variable part), and the response carries the header x-amzn-errortype identifying ValidationException; no trust anchor is created.
Emulator: answers 201 Created with a trustAnchor body, no message and no x-amzn-errortype header - the source is never validated, so an unusable trust anchor is minted and stored.
Why: this is the one divergence here that changes the outcome rather than the text. A client that depends on the service to reject a bad CA reference believes its trust anchor is usable; it will only discover otherwise when certificate-based sessions fail, and against the emulator the negative path cannot be tested at all.
Fix: the CreateTrustAnchor handler's validation of the source member (reject an unreadable/invalid AWS_ACM_PCA ARN with ValidationException before any state write).
Traces: rolesanywhere-validation-error-paths#task#5.

Cause 3 of 3 (PutAttributeMapping, fixture rolesanywhere-validation-error-paths step task#7):
AWS: PutAttributeMapping.certificateField is @required and targets the CertificateField enum (Smithy: CertificateField values x509Subject, x509Issuer, x509SAN; PutAttributeMapping errors include ValidationException httpError 400). An unknown value is refused 400 ValidationException with the constraint envelope '1 validation error detected: Value '<submitted value>' at '<member name>' failed to satisfy constraint: Member must satisfy enum value set: [x509Subject, x509SAN, x509Issuer]' - the fixed parts are the envelope and the bracketed value set in that order, and the variable parts are the submitted value and the member name ('certificateField').
Emulator: same status and envelope but lists the set in the model's declaration order, '[x509Subject, x509Issuer, x509SAN]', so the enumerated values appear in the wrong order.
Why: as cause 1 - the message text is part of the error contract, and an exact-match comparison against AWS's refusal fails even though the rejection itself is correct.
Fix: the shared constraint-violation message renderer - the enum value set for CertificateField must be emitted in the service's order (x509Subject, x509SAN, x509Issuer), not the model's declaration order.
Traces: rolesanywhere-validation-error-paths#task#7.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator, 3 mismatch clusters in fixture rolesanywhere-validation-error-paths.

Cause 1 - #task#4 CreateProfile, expected 400 / actual 400:
  $.message: value_mismatch (expected "1 validation error detected: Value '<bad name>' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: [ a-zA-Z0-9-_]*", got "1 validation error detected: Value '<bad name>' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[ a-zA-Z0-9-_]*$")
  Difference is solely the retained ^ ... $ anchors in the rendered pattern.

Cause 2 - #task#5 CreateTrustAnchor, expected 400 / actual 201:
  status: expected 400, got 201
  $.message: missing (expected 'Error creating TrustAnchor. Given AWS PCA did not allow get request, or invalid ARN.', got None)
  $headers.x-amzn-errortype: missing (expected a ValidationException error-type header, got None)
  The emulator instead returned a created trustAnchor body.

Cause 3 - #task#7 PutAttributeMapping, expected 400 / actual 400:
  $.message: value_mismatch (expected "1 validation error detected: Value '<bad field>' at 'certificateField' failed to satisfy constraint: Member must satisfy enum value set: [x509Subject, x509SAN, x509Issuer]", got "1 validation error detected: Value '<bad field>' at 'certificateField' failed to satisfy constraint: Member must satisfy enum value set: [x509Subject, x509Issuer, x509SAN]")
  Difference is solely the order of the bracketed enum value set.

## Steps to Reproduce
1. Replay the simparity-wire-aws-v1 wire corpus against the emulator built from this checkout.
2. Cause 1: POST /profiles with a name containing characters outside [ a-zA-Z0-9-_] - fixture rolesanywhere-validation-error-paths step task#4. The 400 message prints the anchored pattern.
3. Cause 2: POST /trustanchors with an AWS_ACM_PCA source whose CA ARN is invalid/unreadable - same fixture, step task#5. The emulator answers 201 and creates the anchor instead of 400 ValidationException.
4. Cause 3: PUT /profiles/{profileId}/mappings with certificateField set to a value outside the CertificateField enum - same fixture, step task#7. The 400 message lists the enum set in the wrong order.
