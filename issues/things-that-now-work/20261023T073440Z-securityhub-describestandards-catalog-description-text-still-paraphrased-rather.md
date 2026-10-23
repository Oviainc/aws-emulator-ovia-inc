Issue-Source: trace-sweep-divergence
Issue-Trace: securityhub-catalog-describe-standards
Issue-Step: task#1
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: securityhub-catalog-describe-standards
Issue-Shape-Id: com.amazonaws.securityhub#SecurityHubAPIService
Issue-Key: securityhub|DescribeStandards|2xx->2xx|$.Standards[Name="Azure Foundational Security Best Practices v1.0.0"].Description
Issue-Key: securityhub|DescribeStandards|2xx->2xx|$.Standards[Name="CIS Azure Foundations Benchmark v4.0.0"].Description
Issue-Key: securityhub|DescribeStandards|2xx->2xx|$.Standards[Name="NIST Special Publication 800-171 Revision 2"].Description
Issue-Key: securityhub|DescribeStandards|2xx->2xx|$.Standards[Name="NIST Special Publication 800-53 Revision 5"].Description
Issue-Key: securityhub|DescribeStandards|2xx->2xx|$.Standards[Name="PCI DSS v4.0.1"].Description
Issue-Commit: df7594642179139d3edfbba7bea9d9f88aab159a

# securityhub DescribeStandards catalog Description text still paraphrased rather than AWS catalog text

> _Note: this issue was identified against the code base at commit `df7594642179139d3edfbba7bea9d9f88aab159a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStandards

## Description
Cause 1 of 1: the seeded standards catalog behind DescribeStandards now returns the right entry set (every Standard matches by Name) and the right status, but the Description member of five of those entries is still the emulator's own paraphrase instead of the catalog text AWS serves. Fixture securityhub-catalog-describe-standards, step task#1.

AWS: DescribeStandards is GET /standards -> 200 with DescribeStandardsResponse.Standards, a list of Standard structures (Smithy: operation DescribeStandards, http {method GET, uri /standards, code 200}; output shape DescribeStandardsResponse{Standards: list<Standard>, NextToken}; shape Standard{StandardsArn, Name, Description, EnabledByDefault, Provider, StandardsManagedBy}, where Description targets NonEmptyString @pattern \S and is documented only as "A description of the standard"). The model does not enumerate the managed standards or fix their prose, so the catalog contents are recorded behavior only: the recording shows AWS returning, for each managed standard, the published catalog Description for that standard. The general requirement is that DescribeStandards serve AWS's published catalog text for each standard entry, not a locally authored summary of it.

Emulator: status and shape are correct (expected 200, actual 200, verdict mismatch; no missing or extra Name-keyed entries this run), but five entries' Description values are value_mismatch against the recording: $.Standards[Name="Azure Foundational Security Best Practices v1.0.0"].Description, $.Standards[Name="CIS Azure Foundations Benchmark v4.0.0"].Description, $.Standards[Name="NIST Special Publication 800-171 Revision 2"].Description, $.Standards[Name="NIST Special Publication 800-53 Revision 5"].Description and $.Standards[Name="PCI DSS v4.0.1"].Description. The pattern across all five is the same: the emulator's seed carries a reworded variant of the same sentence (example, Azure Foundational Security Best Practices: recorded "...detect when Azure resources do not align to security best practices. ... and cover Azure's most popular and foundational services." vs. served "...detect when deployed Azure resources do not align with security best practices. ... helps improve your security posture for most popular Azure services."), i.e. the text was written locally rather than copied from the AWS catalog.

This is the residue of the already-resolved issue "DescribeStandards returns a standards catalog that diverges from AWS in both entry set and Description text" (20261023T023011Z-securityhub-describestandards-returns-a-standards-catalog-that-diverges-from): the entry-set half of that issue is fixed and the AWS/AI/Resource-Tagging/PCI-v3.2.1 descriptions now match, but the five entries above were not brought over verbatim, so the divergence is still reported. A resolved issue does not cover a still-failing step, hence this filing.

Why: a client that enumerates the catalog to drive standard enablement is unaffected (ARNs and Names match), but anything that surfaces or snapshots the catalog — console-style listings, compliance reports, golden-file tests over DescribeStandards — renders text that is not what AWS returns, and a byte-comparison against a recorded AWS response fails. Because the call is a 200 with a valid shape, nothing signals to the client that the catalog text is locally invented.

Fix: the seeded standards catalog data behind the DescribeStandards handler (the service's static standards seed/state). Serialization of the Standard structure is already correct, so the change is in the seed values: each entry's Description must be the AWS catalog text verbatim, including any non-ASCII punctuation, so the recorded and served Description agree for every entry.

Traces: securityhub-catalog-describe-standards#task#1 (DescribeStandards, expected 200, actual 200, verdict mismatch; 5 diverging .Description paths under $.Standards).

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up; /__admin/health 200): fixture securityhub-catalog-describe-standards, step task#1, service securityhub, operation DescribeStandards, expected_status 200, actual_status 200, verdict mismatch, stub_path empty, owned_by empty.

divergence_key: service=securityhub|DescribeStandards|status=200->200|keys=$.Standards[Name="Azure Foundational Security Best Practices v1.0.0"].Description,$.Standards[Name="CIS Azure Foundations Benchmark v4.0.0"].Description,$.Standards[Name="NIST Special Publication 800-171 Revision 2"].Description,$.Standards[Name="NIST Special Publication 800-53 Revision 5"].Description,$.Standards[Name="PCI DSS v4.0.1"].Description

Per-path detail reported by replay (expected = recorded AWS response, got = emulator):
- $.Standards[Name="Azure Foundational Security Best Practices v1.0.0"].Description — value_mismatch. Expected: "The Azure Foundational Security Best Practices standard is a set of automated security checks that detect when Azure resources do not align to security best practices. The standard is defined by AWS security experts. This curated set of controls helps improve your security posture in Azure, and cover Azure's most popular and foundational services." Got: "The Azure Foundational Security Best Practices standard is a set of automated security checks that detect when deployed Azure resources do not align with security best practices. The standard is defined by AWS security experts. This curated set of controls helps improve your security posture for most popular Azure services."
- $.Standards[Name="CIS Azure Foundations Benchmark v4.0.0"].Description — value_mismatch. Expected: "The Center for Internet Security (CIS) Microsoft Azure Foundations Benchmark v4.0.0 is a set of security configuration best practices for Azure. This Security Hub standard automatically checks your compliance readiness against a subset of CIS requirements." Got: "The Center for Internet Security (CIS) Azure Foundations Benchmark v4.0.0 is a set of security configuration best practices for Azure. This Security Hub standard automatically checks for your compliance readiness against a subset of CIS requirements."
- $.Standards[Name="NIST Special Publication 800-171 Revision 2"].Description — value_mismatch. Expected: "NIST Special Publication 800-171 Revision 2 provides recommended security requirements for protecting the confidentiality of Controlled Unclassified Information. This Security Hub standard automatically checks for your compliance readiness against a subset of NIST 800-171 R2 requirements." Got text begins "The National Institute of Standards and Technology (NIST) Special Publication 800-171 Revision 2 provides recommended requirements for protecting the confidentiality of controlled unclassifie..." (emulator expands the acronym and drops "security"/capitalisation of the defined term).
- $.Standards[Name="NIST Special Publication 800-53 Revision 5"].Description — value_mismatch (same paraphrase pattern; reported in the cluster's key list).
- $.Standards[Name="PCI DSS v4.0.1"].Description — value_mismatch (same paraphrase pattern; reported in the cluster's key list).

The recorded strings above are examples of the AWS catalog text; the requirement is that the served Description equal AWS's published catalog text for each standard, whatever its exact wording.

Model grounding: Smithy DescribeStandards -> GET /standards, code 200, errors InternalException(500)/InvalidAccessException(401)/InvalidInputException(400), paginated by NextToken/MaxResults; Standard.Description: NonEmptyString @pattern \S, doc "A description of the standard". The catalog itself is not modelled (recorded behavior only).

No state_reset_failures in this run (0), so this divergence is not contaminated by a previous fixture's state.

## Steps to Reproduce
1. Clone the repo and boot the emulator from this checkout.
2. Replay trace set simparity-wire-aws-v1, fixture securityhub-catalog-describe-standards, step task#1: GET /standards against the securityhub endpoint (DescribeStandards with no NextToken, MaxResults or Providers).
3. The response is HTTP 200 with a DescribeStandardsResponse; the entry set keyed by Name matches the recording.
4. Compare each $.Standards[*].Description with the recorded response. Observe five entries differ: Azure Foundational Security Best Practices v1.0.0, CIS Azure Foundations Benchmark v4.0.0, NIST Special Publication 800-171 Revision 2, NIST Special Publication 800-53 Revision 5 and PCI DSS v4.0.1 — each carries a locally reworded variant of the AWS sentence rather than the AWS catalog text.
5. Expected: every entry's Description matches the AWS catalog text served in the recording, byte-for-byte.
