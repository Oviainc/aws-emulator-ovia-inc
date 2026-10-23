Issue-Source: trace-sweep-divergence
Issue-Trace: securityhub-catalog-describe-standards
Issue-Step: task#1
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: securityhub-catalog-describe-standards
Issue-Shape-Id: com.amazonaws.securityhub#SecurityHubAPIService
Issue-Key: securityhub|DescribeStandards|2xx->2xx|$.Standards[Name="PCI DSS v4.0.1"].Description
Issue-Commit: 3dc196f76d19116b5d570e1dc438e2b93d0e0d47

# securityhub DescribeStandards returns a stale PCI DSS v4.0.1 standard description

> _Note: this issue was identified against the code base at commit `3dc196f76d19116b5d570e1dc438e2b93d0e0d47`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStandards

## Description
The emulator's built-in Security Hub standards catalog carries the wrong Description string for the PCI DSS v4.0.1 standard: it serves the generic, version-less wording instead of the version-qualified wording AWS serves for that standard ARN.

Cause 1 of 1: DescribeStandards -> Standards[Name="PCI DSS v4.0.1"].Description (fixture securityhub-catalog-describe-standards, step task#1).

AWS: Smithy: DescribeStandards -> DescribeStandardsResponse.Standards: list<Standard>; Standard.Description -> NonEmptyString, pattern "\\S", documented as "A description of the standard." The model does not pin the text of any particular standard, so for the per-standard catalog content this is recorded behavior only: AWS returns HTTP 200 and, for the entry whose Name is "PCI DSS v4.0.1", a Description that ends "...automatically checks for your compliance readiness against a subset of PCI DSS V4.0.1 requirements." — the requirement subset is named with the standard's own version.

Emulator: answers HTTP 200 with the same standard entry (status matches, ARN/Name/EnabledByDefault all match) but a Description ending "...against a subset of PCI DSS requirements." — the version qualifier "V4.0.1" is missing from the seeded catalog string. Replay verdict: mismatch, expected_status 200, actual_status 200, diverging key $.Standards[Name="PCI DSS v4.0.1"].Description.

Why: DescribeStandards is the discovery call clients and consoles use to enumerate and present the available standards. A caller that renders or indexes the Description shows text that does not identify the standard version, and any client (or test fixture) that matches the catalog description to distinguish the v3.2.1 and v4.0.1 PCI DSS standards — which share nearly identical wording apart from the version token — cannot tell them apart against the emulator. Because the two PCI DSS standards differ only in that token, the stale string is also indistinguishable from the older standard's description.

Fix: the seeded standards catalog data behind the DescribeStandards handler in the securityhub service crate (the static standards table, not the serializer — the envelope and all other members of the entry replay correctly). The PCI DSS v4.0.1 catalog row's Description must carry the version-qualified requirement-subset wording. Worth auditing the other seeded catalog rows at the same time for the same copy/paste-from-the-previous-version defect.

Note: the only other open securityhub issue is 20261023T110408Z-securityhub-updatefindings-stub.md (an unimplemented UpdateFindings scaffold); it does not cover this catalog-data divergence.

Traces: securityhub-catalog-describe-standards#task#1.

## Evidence
Fixture securityhub-catalog-describe-standards, step task#1, operation DescribeStandards, service securityhub.
verdict: mismatch; expected_status 200, actual_status 200 (status and error-free envelope agree).
divergence_key: service=securityhub|DescribeStandards|status=200->200|keys=$.Standards[Name="PCI DSS v4.0.1"].Description

Expected ($.Standards[Name="PCI DSS v4.0.1"].Description):
  "The Payment Card Industry Data Security Standard (PCI DSS) v4.0.1 is an information security standard for entities that store, process, and/or transmit cardholder data. This Security Hub standard automatically checks for your compliance readiness against a subset of PCI DSS V4.0.1 requirements."

Actual:
  "The Payment Card Industry Data Security Standard (PCI DSS) v4.0.1 is an information security standard for entities that store, process, and/or transmit cardholder data. This Security Hub standard automatically checks for your compliance readiness against a subset of PCI DSS requirements."

Difference: the trailing requirement-subset phrase drops the standard version — expected "a subset of PCI DSS V4.0.1 requirements", served "a subset of PCI DSS requirements". No other member of the entry diverged, and no other standard in the list diverged (affected_count 1).

Model reference: Smithy Standard.Description -> NonEmptyString (pattern "\\S"), doc "A description of the standard."; the per-standard text itself is recorded behavior only.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it (the securityhub service is bootstrapped; /__admin/health answers 200).
2. Replay wire fixture securityhub-catalog-describe-standards, step task#1 — a securityhub DescribeStandards call with no filters, enumerating the whole standards catalog.
3. Both AWS and the emulator answer HTTP 200 with a Standards list containing the entry Name="PCI DSS v4.0.1".
4. Compare that entry's Description. Expected: the sentence ends "...against a subset of PCI DSS V4.0.1 requirements.". Actual: it ends "...against a subset of PCI DSS requirements." — the version qualifier is absent.
5. Equivalent by hand: `aws securityhub describe-standards` and read the Description of the standard named "PCI DSS v4.0.1".
