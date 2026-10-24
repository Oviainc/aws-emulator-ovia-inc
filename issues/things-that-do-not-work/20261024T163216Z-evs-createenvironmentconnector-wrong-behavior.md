Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 0831f46a299ca00fdb6555dd0d1d0166298aa2f3

# Connector$secretArn echoes a bare secret name instead of the secret's ARN

_Filed: 2026-10-24T16:32:16Z_
_Operation: CreateEnvironmentConnector | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `0831f46a299ca00fdb6555dd0d1d0166298aa2f3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateEnvironmentConnectorRequest$secretIdentifier` accepts "The ARN or name of the Amazon Web Services Secrets Manager secret that stores the credentials for the VCF appliance", while the output member it feeds is `Connector$secretArn`: "The Amazon Resource Name (ARN) of the Amazon Web Services Secrets Manager secret that stores the credentials for the VCF appliance".

Expected: a create (or update) that names the secret by NAME — the modelled pattern explicitly admits the bare `[a-zA-Z0-9/_+=.@!-]+` alternative — reports the resolved full ARN in `connector.secretArn`, so every connector read (`CreateEnvironmentConnector`, `UpdateEnvironmentConnector`, `ListEnvironmentConnectors`) hands the caller an ARN it can use directly.

Actual: the stored value is the request's string verbatim (`StoredConnector::creating` sets `secret_arn: secret_identifier.to_string()`, documented as "EXACTLY as the request supplied it"), so `secretArn` comes back as `vcf-vcenter-creds` rather than `arn:aws:secretsmanager:<region>:<account>:secret:vcf-vcenter-creds-<suffix>`. A client that passes the name on create and then reads `secretArn` expecting an ARN sees a name, and two connectors created with the name and with the ARN of the SAME secret report two different `secretArn` values for one secret.

## Evidence

Spec: `CreateEnvironmentConnectorRequest$secretIdentifier` doc ("The ARN or name of the ... secret") versus `Connector$secretArn` doc ("The Amazon Resource Name (ARN) of the ... secret") — the input is either form, the output is named and documented as the ARN. Handler: src/state.rs `StoredConnector::creating` lines 511-531 (`secret_arn: secret_identifier.to_string()`, with the doc comment at lines 483-490 stating a name is reported as a name) and `to_wire` line 576; src/ops/create_environment_connector.rs lines 281-288; src/ops/update_environment_connector.rs lines 276-280 writes the new `secretIdentifier` into `secret_arn` the same way. Tests: tests/all/connectors.rs only ever supplies a full ARN (the SECRET_ARN constant, line 58, and a rotated ARN at line 436), so no test exercises a bare-name identifier and nothing asserts what `secretArn` reports for one. Distinct from the open issue "secretIdentifier and applianceFqdn are never resolved…" (CreateEnvironmentConnector/validation), which is about refusing/failing a secret that does not exist; this one is about the VALUE reported for a secret named by name.

## Steps to Reproduce

1. Create an environment and wait for it to reach CREATED.
2. Call CreateEnvironmentConnector with type=VCENTER, applianceFqdn=vcenter.example.com and secretIdentifier="vcf-vcenter-creds" (the bare name form the modelled pattern admits).
3. Read `connector.secretArn` in the response, and again through ListEnvironmentConnectors: it is the bare string "vcf-vcenter-creds" instead of the secret's ARN.
4. Repeat with UpdateEnvironmentConnector(secretIdentifier="vcf-vcenter-creds-2") — the updated connector reports the name in `secretArn` as well.
