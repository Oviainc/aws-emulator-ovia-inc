Issue-Source: trace-sweep-divergence
Issue-Trace: signer-catalog-list-signing-platforms
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: signer-catalog-list-signing-platforms signer-signing-platform-catalog
Issue-Shape-Id: com.amazonaws.signer#WallabyService
Issue-Key: signer|ListSigningPlatforms|2xx->2xx|$.platforms[*]
Issue-Key: signer|GetSigningPlatform|2xx->2xx|$.category
Issue-Key: signer|GetSigningPlatform|2xx->2xx|$.displayName
Issue-Key: signer|GetSigningPlatform|2xx->2xx|$.maxSizeInMB
Issue-Key: signer|GetSigningPlatform|2xx->2xx|$.partner
Issue-Key: signer|GetSigningPlatform|2xx->2xx|$.target
Issue-Commit: f04d881c92d4dd0b362060ab10db1f8e619a8cb7

# Signer signing-platform catalog serves the wrong platform rows and per-row values

> _Note: this issue was identified against the code base at commit `f04d881c92d4dd0b362060ab10db1f8e619a8cb7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListSigningPlatforms

## Description
Two net-new wire divergences share one root cause: the static signing-platform catalog the emulator publishes is not the catalog AWS Signer publishes. Both the list reader and the single-platform reader serve it, so both diverge. Both causes must be closed before this issue is submitted.

Cause 1 of 2: ListSigningPlatforms serves a different row set, ordering and per-row values (fixture signer-catalog-list-signing-platforms, step task#1).
AWS: recorded behavior only for the catalog's contents — the Smithy model defines the SigningPlatform shape (platformId, displayName, partner, target, category, signingConfiguration, signingImageFormat, maxSizeInMB, revocationSupported) and ListSigningPlatforms' paginated output, but not which rows the service publishes, so the recording is the reference. The recorded first page's leading rows are: [0] platformId AWSIoTDeviceManagement-SHA256-ECDSA with displayName "AWS IoT Device Management SHA256-ECDSA " (the published display name carries a trailing space), partner AWSIoTDeviceManagement, target SHA256-ECDSA, category "AWS", SHA256/ECDSA, JSONDetached, maxSizeInMB 2048, revocationSupported false; [1] platformId Notation-OCI-SHA384-ECDSA with displayName "Notation for container registries", partner "Notary", target "OCI", category "External", SHA384/ECDSA, maxSizeInMB 1, revocationSupported true, and no signingImageFormat member at all. The divergence spans $.platforms[0] through $.platforms[4], so the whole leading page differs, not one field.
Emulator: row [0] has the same platformId but the display name without its trailing space, category "AWSIoT" instead of "AWS", and revocationSupported true instead of false; row [1] is AWSLambda-SHA384-ECDSA (maxSizeInMB 250, category AWS) where AWS publishes the Notation row — i.e. the Notation/External rows are absent and the remaining rows are differently ordered and differently valued. Note the model's Category enum only lists AWSIoT while the live catalog serves "AWS" and "External" as well; the emulator appears to have clamped category to that enum, which is why every AWS/External row reports AWSIoT or AWS.
Why: clients enumerate this catalog to choose a platformId and to learn a platform's capabilities. A caller cannot discover Notation-OCI-SHA384-ECDSA at all, cannot filter by category=External or partner=Notary (both are httpQuery filters on this operation), and reads revocationSupported/maxSizeInMB values that contradict what PutSigningProfile, StartSigningJob and RevokeSignature will actually accept for the same platform.
Fix: the static signing-platform catalog table in the signer crate's platform catalog/state module — restore the published row set, their order, and each row's field values including a category value set wider than the model's enum.

Cause 2 of 2: GetSigningPlatform returns wrong fields for Notation-OCI-SHA384-ECDSA (fixture signer-signing-platform-catalog, step task#2).
AWS: 200 with displayName "Notation for container registries", partner "Notary", target "OCI", category "External", maxSizeInMB 1 — the same row the list reader publishes.
Emulator: 200 but omits displayName entirely, reports partner "Notation", target "OCI-SHA384-ECDSA" (it appears to split the platformId instead of using the published partner/target), category "AWS", and maxSizeInMB 16.
Why: a client resolving a platform before creating a signing profile gets the wrong size ceiling and partner/target identity, and has no display name to show; the derived-from-platformId partner/target is a guess, not the published data.
Fix: the same catalog table — GetSigningPlatform must return the stored row verbatim rather than deriving fields from the platform id.

Traces: signer-catalog-list-signing-platforms#task#1 (representative), signer-signing-platform-catalog#task#2.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from mainline.

Cause 1 — ListSigningPlatforms, signer-catalog-list-signing-platforms#task#1, status 200/200, diverging keys $.platforms[0] … $.platforms[4]:
  $.platforms[0] expected {'platformId': 'AWSIoTDeviceManagement-SHA256-ECDSA', 'displayName': 'AWS IoT Device Management SHA256-ECDSA ', 'partner': 'AWSIoTDeviceManagement', 'target': 'SHA256-ECDSA', 'category': 'AWS', signingConfiguration SHA256/ECDSA, signingImageFormat JSONDetached, 'maxSizeInMB': 2048, 'revocationSupported': False}
              got      {'platformId': 'AWSIoTDeviceManagement-SHA256-ECDSA', 'displayName': 'AWS IoT Device Management SHA256-ECDSA' (no trailing space), 'partner': 'AWSIoTDeviceManagement', 'target': 'SHA256-ECDSA', 'category': 'AWSIoT', …, 'maxSizeInMB': 2048, 'revocationSupported': True}
  $.platforms[1] expected {'platformId': 'Notation-OCI-SHA384-ECDSA', 'displayName': 'Notation for container registries', 'partner': 'Notary', 'target': 'OCI', 'category': 'External', signingConfiguration SHA384/ECDSA, 'maxSizeInMB': 1, 'revocationSupported': True} (no signingImageFormat member)
              got      {'platformId': 'AWSLambda-SHA384-ECDSA', 'displayName': 'AWS Lambda', 'partner': 'AWSLambda', 'target': 'SHA384-ECDSA', 'category': 'AWS', 'maxSizeInMB': 250, …}

Cause 2 — GetSigningPlatform, signer-signing-platform-catalog#task#2, status 200/200:
  $.displayName: expected 'Notation for container registries', got None (missing)
  $.partner:     expected 'Notary', got 'Notation'
  $.target:      expected 'OCI', got 'OCI-SHA384-ECDSA'
  $.category:    expected 'External', got 'AWS'
  $.maxSizeInMB: expected 1, got 16

Model facts used: Smithy signer — SigningPlatform structure members {platformId, displayName, partner, target, category, signingConfiguration, signingImageFormat, maxSizeInMB (default 0), revocationSupported (default false)}; Category enum values ['AWSIoT'] only, narrower than the catalog the recording shows; ListSigningPlatforms GET /signing-platforms with httpQuery filters category/partner/target/maxResults(range 1-25)/nextToken, paginated; GetSigningPlatform GET /signing-platforms/{platformId}.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from mainline.
2. Replay the simparity-wire-aws-v1 wire corpus against it.
3. Cause 1: replay signer-catalog-list-signing-platforms step task#1 — ListSigningPlatforms with no filters; compare $.platforms[0] through $.platforms[4] against the recorded page (row identity, order, displayName, partner, target, category, maxSizeInMB, revocationSupported, presence of signingImageFormat).
4. Cause 2: replay signer-signing-platform-catalog step task#2 — GetSigningPlatform with platformId Notation-OCI-SHA384-ECDSA; compare $.displayName, $.partner, $.target, $.category, $.maxSizeInMB.
