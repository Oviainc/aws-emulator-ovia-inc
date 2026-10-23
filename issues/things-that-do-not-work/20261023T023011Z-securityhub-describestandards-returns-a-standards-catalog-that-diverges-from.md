Issue-Source: trace-sweep-divergence
Issue-Trace: securityhub-catalog-describe-standards
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: securityhub-catalog-describe-standards
Issue-Shape-Id: com.amazonaws.securityhub#SecurityHubAPIService
Issue-Key: securityhub|DescribeStandards|2xx->2xx|$.Standards[Name="AI Security Best Practices v1.0.0"].Description
Issue-Key: securityhub|DescribeStandards|2xx->2xx|$.Standards[Name="AWS Foundational Security Best Practices v1.0.0"].Description
Issue-Key: securityhub|DescribeStandards|2xx->2xx|$.Standards[Name="AWS Resource Tagging Standard v1.0.0"].Description
Issue-Key: securityhub|DescribeStandards|2xx->2xx|$.Standards[Name="Azure Foundational Security Best Practices v1.0.0"].Description
Issue-Key: securityhub|DescribeStandards|2xx->2xx|$.Standards[Name="CIS AWS Foundations Benchmark v5.0.0"]
Issue-Key: securityhub|DescribeStandards|2xx->2xx|$.Standards[Name="CIS Azure Foundations Benchmark v4.0.0"]
Issue-Key: securityhub|DescribeStandards|2xx->2xx|$.Standards[Name="NIST SP 800-53 Rev. 5"]
Issue-Key: securityhub|DescribeStandards|2xx->2xx|$.Standards[Name="NIST Special Publication 800-171 Revision 2"]
Issue-Key: securityhub|DescribeStandards|2xx->2xx|$.Standards[Name="NIST Special Publication 800-53 Revision 5"]
Issue-Key: securityhub|DescribeStandards|2xx->2xx|$.Standards[Name="PCI DSS v3.2.1"].Description
Issue-Key: securityhub|DescribeStandards|2xx->2xx|$.Standards[Name="PCI DSS v4.0.1"].Description
Issue-Commit: 1b15cfcf83a9fafcaa69f938ed17066b612a8ba0

# DescribeStandards returns a standards catalog that diverges from AWS in both entry set and Description text

> _Note: this issue was identified against the code base at commit `1b15cfcf83a9fafcaa69f938ed17066b612a8ba0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStandards

## Description
Cause 1 of 1: the seeded standards catalog served by DescribeStandards does not match the catalog AWS returns, in two linked ways — the set of Standard entries (keyed by Name) and the Description text of the entries that do line up. Fixture securityhub-catalog-describe-standards, step task#1.

AWS: DescribeStandards is GET /standards -> 200 with DescribeStandardsResponse.Standards, a list of Standard structures (Smithy: operation DescribeStandards, http GET /standards code 200, output shape DescribeStandardsResponse; shape Standard with members StandardsArn, Name, Description, EnabledByDefault, Provider, StandardsManagedBy, each string member targeting NonEmptyString @pattern \\S). The model documents Name as "The name of the standard" and Description as "A description of the standard", but does not enumerate which standards exist or fix their text, so the catalog contents themselves are recorded behavior only. The recording shows AWS enumerating each managed standard under its full AWS catalog name (e.g. "NIST Special Publication 800-53 Revision 5", "NIST Special Publication 800-171 Revision 2", "CIS AWS Foundations Benchmark v5.0.0", "CIS Azure Foundations Benchmark v4.0.0", "AWS Foundational Security Best Practices v1.0.0", "AWS Resource Tagging Standard v1.0.0", "AI Security Best Practices v1.0.0", "Azure Foundational Security Best Practices v1.0.0", "PCI DSS v3.2.1", "PCI DSS v4.0.1") with the catalog Description text AWS publishes for each.

Emulator: status is correct (200) and the response shape is correct, but the catalog body diverges. Replay reports Name-keyed entries that exist on only one side — $.Standards[Name="NIST Special Publication 800-53 Revision 5"], $.Standards[Name="NIST Special Publication 800-171 Revision 2"], $.Standards[Name="CIS AWS Foundations Benchmark v5.0.0"], $.Standards[Name="CIS Azure Foundations Benchmark v4.0.0"] and an emulator-only abbreviated form $.Standards[Name="NIST SP 800-53 Rev. 5"] — i.e. the emulator's seed uses its own naming forms rather than the AWS catalog names, so those standards cannot be matched at all. For the entries that do match by Name, .Description is value_mismatch on AI Security Best Practices v1.0.0, AWS Foundational Security Best Practices v1.0.0, AWS Resource Tagging Standard v1.0.0, Azure Foundational Security Best Practices v1.0.0, PCI DSS v3.2.1 and PCI DSS v4.0.1 — the emulator serves paraphrased text (example: for AI Security Best Practices the emulator returns "...detect when your artificial intelligence and machine learning workloads, and the resources they run on, do not align with security best practices." where the recording has "...detect when deployed AI resources do not align with security best practices. The standard is defined by AWS security experts. This curated set of controls helps improve your security posture for most popular AI services."). One mismatch is purely a character-encoding difference in the same sentence: AWS Foundational Security Best Practices ends "AWS\u2019s most popular and foundational services" with a U+2019 right single quotation mark, where the emulator emits an ASCII apostrophe.

Why: DescribeStandards is the discovery call clients use to enumerate the managed standards before acting on them. A client that reads a standard out of the catalog and then drives BatchEnableStandards / GetEnabledStandards / BatchDisableStandards from it finds no entry for the standards the emulator renamed, so standard enablement flows fail against the emulator while they succeed on AWS; and console/report tooling that surfaces the catalog renders the wrong (and in one case mis-encoded) text. Because the status is 200 and the shape is valid, nothing in the client signals the catalog is wrong — it just silently lacks the standards.

Fix: the seeded standards catalog behind the DescribeStandards handler (the service's static standards state/seed data, not the serializer — serialization of the Standard structure is already correct). Each seeded entry's Name must be the AWS catalog name verbatim and its Description the AWS catalog text verbatim, including non-ASCII punctuation such as U+2019, so that the entry set keyed by Name matches the recording.

Traces: securityhub-catalog-describe-standards#task#1 (DescribeStandards, expected 200, actual 200, verdict mismatch; 11 diverging JSON paths under $.Standards).

## Evidence
Fixture securityhub-catalog-describe-standards, step task#1, operation DescribeStandards: expected_status 200, actual_status 200, verdict mismatch.

Diverging paths reported by replay (divergence_key keys=...):
- $.Standards[Name="AI Security Best Practices v1.0.0"].Description — value_mismatch. Expected: 'The AI Security Best Practices standard is a set of automated security checks that detect when deployed AI resources do not align with security best practices. The standard is defined by AWS security experts. This curated set of controls helps improve your security posture for most popular AI services.' Actual: 'The AI Security Best Practices standard is a set of automated security checks that detect when your artificial intelligence and machine learning workloads, and the resources they run on, do not align with security best practices.'
- $.Standards[Name="AWS Foundational Security Best Practices v1.0.0"].Description — value_mismatch; identical sentence except the expected text ends "AWS\u2019s most popular and foundational services" (U+2019) while the actual uses an ASCII apostrophe.
- $.Standards[Name="AWS Resource Tagging Standard v1.0.0"].Description — value_mismatch. Expected: 'The AWS Resource Tagging standard is a set of automated security checks that detect when AWS resources are not tagged.' Actual: 'The AWS Resource Tagging Standard is a set of automated controls that detect when deployed resources do not carry the tags your organization requires, so that you can keep your tagging strategy consistent.'
- $.Standards[Name="Azure Foundational Security Best Practices v1.0.0"].Description — value_mismatch.
- $.Standards[Name="PCI DSS v3.2.1"].Description — value_mismatch.
- $.Standards[Name="PCI DSS v4.0.1"].Description — value_mismatch.
- Whole-entry divergences (no member suffix, i.e. the Name-keyed entry is present on only one side): $.Standards[Name="CIS AWS Foundations Benchmark v5.0.0"], $.Standards[Name="CIS Azure Foundations Benchmark v4.0.0"], $.Standards[Name="NIST Special Publication 800-53 Revision 5"], $.Standards[Name="NIST Special Publication 800-171 Revision 2"], and the emulator-only $.Standards[Name="NIST SP 800-53 Rev. 5"].

Model grounding: Smithy DescribeStandards -> GET /standards, code 200, output DescribeStandardsResponse{Standards: list<Standard>, NextToken}; Standard{StandardsArn, Name, Description: NonEmptyString @pattern \\S, EnabledByDefault, Provider, StandardsManagedBy}. The model does not enumerate the catalog, so the expected entry set and text are taken from the recording (recorded behavior only).

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Replay fixture securityhub-catalog-describe-standards, step task#1 — an unauthenticated-shape GET to the securityhub endpoint: GET /standards (DescribeStandards, no NextToken/MaxResults/Providers).
3. The response is HTTP 200 with a DescribeStandardsResponse. Compare $.Standards to the recorded response keyed by each entry's Name.
4. Observe (a) the recorded entries named "NIST Special Publication 800-53 Revision 5", "NIST Special Publication 800-171 Revision 2", "CIS AWS Foundations Benchmark v5.0.0" and "CIS Azure Foundations Benchmark v4.0.0" have no counterpart in the emulator's catalog, which instead carries an entry named "NIST SP 800-53 Rev. 5"; and (b) for the six entries that do match by Name, .Description differs from the recorded AWS catalog text, including one difference that is only the U+2019 apostrophe in the AWS Foundational Security Best Practices description.
5. Expected: the catalog entry set keyed by Name, and each entry's Description, match the recording byte-for-byte.
