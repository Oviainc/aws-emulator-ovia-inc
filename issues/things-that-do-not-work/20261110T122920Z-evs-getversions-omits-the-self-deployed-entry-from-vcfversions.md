Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: evs.get-versions-returns-known-vcf-versions
Issue-Shape-Id: com.amazonaws.evs#AmazonElasticVMwareService
Issue-Key: evs.get-versions-returns-known-vcf-versions|run#13|AssertionError|assert "SELF_DEPLOYED" in seen_vcf_versions, (
Issue-Commit: e6401f4b3684d775dd4b2ce4f19c447c64a835c8

# GetVersions omits the SELF_DEPLOYED entry from vcfVersions, so the enum value CreateEnvironment accepts is absent from the service's own version catalogue

> _Note: this issue was identified against the code base at commit `e6401f4b3684d775dd4b2ce4f19c447c64a835c8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetVersions

## Description
One net-new root cause in the evs crate, grounded in one failing behavioral check.

=== Cause 1 of 1: vcfVersions never contains a SELF_DEPLOYED entry ===
AWS: Smithy GetVersions output members `vcfVersions` (VcfVersionList) and `instanceTypeEsxVersions` (InstanceTypeEsxVersionsList) are both @required; VcfVersionInfo requires `vcfVersion` (enum VcfVersion), `status` (documented valid values AVAILABLE | RESTRICTED), `defaultEsxVersion` ("The default ESX version for this VCF version. It is based on Broadcom's Bill Of Materials (BOM)") and `instanceTypes` (InstanceTypeList). The VcfVersion enum declares exactly three values: `VCF-5.2.1`, `VCF-5.2.2`, `SELF_DEPLOYED` — SELF_DEPLOYED is a modelled VcfVersion, not a pseudo-value, and CreateEnvironment accepts it. Recorded behavior: GetVersions reports a vcfVersions entry for EVERY VcfVersion the model declares, SELF_DEPLOYED included, each entry carrying a status drawn from {AVAILABLE, RESTRICTED}, a `defaultEsxVersion` in ESXi build-id form (literal prefix `ESXi-`), and an `instanceTypes` list whose members all appear in `instanceTypeEsxVersions[].instanceType`, with that entry's `defaultEsxVersion` present among the `esxVersions` of every instance type it names. The requirement is the FORM and the completeness of the catalogue (one entry per declared VcfVersion, mutually consistent with instanceTypeEsxVersions); the particular build ids and statuses are catalogue data that varies and must never be asserted as literals.
Emulator: GetVersions answers HTTP 200 but `vcfVersions` contains only `['VCF-5.2.1', 'VCF-5.2.2']` — the SELF_DEPLOYED entry is omitted entirely. The omission is deliberate in the catalogue code: `crate::state::provided_vcf_versions` filters the modelled enum with `**version != VCF_VERSION_SELF_DEPLOYED` and then drops any version `OFFERED_ESX_BUILDS` names no `default_for` build for, and no EsxBuild entry names SELF_DEPLOYED, so the value is excluded twice over.
Why: GetVersions is the discovery call a client uses to learn which VCF versions it may pass to CreateEnvironment and which ESX versions it may pass to CreateEnvironmentHost. A client that enumerates vcfVersions to build its version picker, to validate user input, or to decide whether the self-deployed mode is offered in this account concludes SELF_DEPLOYED is unavailable — while CreateEnvironment in this very emulator accepts it — so the catalogue contradicts the operation it describes, and self-deployed onboarding paths are never exercised.
Fix: the service version catalogue in crate::state (services/service-com-amazonaws-evs-amazonelasticvmwareservice/src/state.rs — `provided_vcf_versions` and the `OFFERED_ESX_BUILDS` / `EsxBuild::default_for` table it reads, consumed by src/ops/get_versions.rs): the catalogue must report an entry for every modelled VcfVersion including SELF_DEPLOYED, with a status, an ESXi build-id defaultEsxVersion and instanceTypes that stay consistent with instanceTypeEsxVersions by construction. NOTE: open issue 20261029T183050Z-evs-5-net-new-root-causes-createenvironment-never.md (its Cause 3) covers the FORM of the ESX versions this catalogue reports (bare '8.0.3' instead of 'ESXi-' build ids); that form now replays clean and it does not cover the missing SELF_DEPLOYED entry, which is this cause.
Traces: evs.get-versions-returns-known-vcf-versions.

## Evidence
From the simparity-behavioral-aws-v1 replay (evs 17/20 passed, 3 mismatches, 0 errors, 0 skipped; the other two failures are marked OWNED and are not part of this issue).

Cause 1 — evs.get-versions-returns-known-vcf-versions:
  AssertionError: SELF_DEPLOYED mode is missing from vcfVersions: ['VCF-5.2.1', 'VCF-5.2.2']

The check walks the whole response first: every instanceTypeEsxVersions entry has an instanceType and ESXi-prefixed esxVersions, and every vcfVersions entry has a VcfVersion-shaped vcfVersion, a status in {AVAILABLE, RESTRICTED}, an ESXi-prefixed defaultEsxVersion and instanceTypes that are a subset of the instanceTypeEsxVersions instance types with the default build present in each. All of that passes; the single divergence is that the set of vcfVersion values the emulator reports is {VCF-5.2.1, VCF-5.2.2} where AWS also reports SELF_DEPLOYED (the third value of com.amazonaws.evs#VcfVersion).

Source of the omission: src/state.rs `provided_vcf_versions()` — `.filter(|version| **version != VCF_VERSION_SELF_DEPLOYED)` followed by `.filter_map(|version| default_esx_version(version).map(...))`, and no entry of `OFFERED_ESX_BUILDS` lists SELF_DEPLOYED in its `default_for`.

## Steps to Reproduce
1. Call evs GetVersions (no input members).
2. Collect the set of `vcfVersions[].vcfVersion` values. Emulator: {'VCF-5.2.1', 'VCF-5.2.2'}. AWS: the same two plus 'SELF_DEPLOYED'.
3. For the SELF_DEPLOYED entry AWS returns, confirm it carries a `status` of AVAILABLE or RESTRICTED, a `defaultEsxVersion` beginning with 'ESXi-', and an `instanceTypes` list whose every member appears as an `instanceTypeEsxVersions[].instanceType` with that defaultEsxVersion among its `esxVersions`.
4. Cross-check against CreateEnvironment, which accepts vcfVersion='SELF_DEPLOYED' in this emulator: the discovery catalogue must not omit a version the create operation accepts.
