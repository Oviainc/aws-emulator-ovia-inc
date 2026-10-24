Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 1f316ce023d643c000ad0b8fb7b5799232d706fd

# An EVS-installed environment is created without its VCF credential secrets and without the automatic SDDC_MANAGER connector

_Filed: 2026-10-24T01:22:38Z_
_Operation: CreateEnvironment | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `1f316ce023d643c000ad0b8fb7b5799232d706fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

When `vcfVersion` is anything other than SELF_DEPLOYED, "Amazon EVS installs and configures VCF for you" (CreateEnvironment doc), and two artifacts of that install are part of the API surface:

1. `Environment$credentials` (SecretList): "During environment creation, the Amazon EVS control plane uses Amazon Web Services Secrets Manager to create, encrypt, validate, and store secrets" that hold "the credentials for installing vCenter Server, NSX, and SDDC Manager" (`Secret` doc), encrypted with the request's `kmsKeyId` ("the KMS key ID that Secrets Manager uses to encrypt secrets that are associated with the environment").
2. The SDDC Manager connector: "For environments where Amazon EVS installs VCF, the SDDC_MANAGER connector is created automatically" (CreateEnvironmentConnector doc).

Expected: after an EVS-installed create settles, GetEnvironment reports a non-empty `credentials` list of Secret ARNs, and ListEnvironmentConnectors reports one SDDC_MANAGER connector the caller never created (so a caller CreateEnvironmentConnector of type SDDC_MANAGER hits the documented "only one connector of each type per environment" rule).

Actual: `StoredEnvironment::to_wire` hard-codes `credentials: None`, so every Environment this service reports — from CreateEnvironment, GetEnvironment and DeleteEnvironment — omits the member for every VCF version; and CreateEnvironment inserts `connectors: BTreeMap::new()`, so ListEnvironmentConnectors answers an empty list for an EVS-installed environment and a caller-supplied SDDC_MANAGER connector is accepted as the first of its type. `kmsKeyId` is stored and echoed but encrypts nothing.

## Evidence

Spec: `Environment$credentials` + `Secret` shape doc ("During environment creation, the Amazon EVS control plane uses Amazon Web Services Secrets Manager to create, encrypt, validate, and store secrets"), CreateEnvironment doc ("When you specify any other VCF version, Amazon EVS installs and configures VCF for you"), CreateEnvironmentConnector doc ("For environments where Amazon EVS installs VCF, the SDDC_MANAGER connector is created automatically"). Handler: src/ops/create_environment.rs line 413 (`connectors: std::collections::BTreeMap::new()`); src/state.rs `StoredEnvironment::to_wire` line 996 (`credentials: None`, with the doc comment at lines 972-974 conceding the secrets are never created). src/ops/create_environment_connector.rs module doc lines 25-28 states the automatic SDDC_MANAGER connector is deliberately not minted. Tests: tests/all/connectors.rs line 387 asserts an environment with no connector lists NOTHING ("case an environment with no connector"), i.e. the current behaviour is only exercised for the SELF_DEPLOYED-style path; no test asserts `credentials`.

## Steps to Reproduce

1. Register a VPC and service access subnet in the EC2 store, then CreateEnvironment with vcfVersion=VCF-5.2.1, termsAccepted=true, licenseInfo, vcfHostnames, siteId, connectivityInfo and four hosts.
2. Wait for the environment to reach CREATED, then call GetEnvironment — the `environment.credentials` member is absent instead of listing the VCF credential secret ARNs.
3. Call ListEnvironmentConnectors for the same environmentId — the list is empty instead of containing the automatically created SDDC_MANAGER connector.
4. Call CreateEnvironmentConnector with type=SDDC_MANAGER — it is accepted as the first connector of that type instead of colliding with the automatic one.
