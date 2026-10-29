Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.delete-custom-engine-version-boundary
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds.delete-custom-engine-version-boundary|run#6|AssertionError|assert error.get("Message") == (
Issue-Commit: 1aea0a827cbbaf06353cebaab6cc8375c4166c22

# rds DeleteCustomDBEngineVersion AccessDenied carries a generic IAM "not authorized to perform" sentence instead of RDS's fixed custom-engine-version wording

> _Note: this issue was identified against the code base at commit `1aea0a827cbbaf06353cebaab6cc8375c4166c22`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteCustomDBEngineVersion

## Description
REOPENS 20261029T195009Z-rds-deletecustomdbengineversion-stub.md: its fix left the call refused. The handler is no longer a 501 scaffold and it now refuses with the right code (AccessDenied), but the refusal it serves is not the refusal RDS serves, so the one check that issue listed still fails — now on the message assertion rather than the code assertion.

Cause 1 of 1: DeleteCustomDBEngineVersion's AccessDenied message is emulator-authored generic IAM text instead of RDS's fixed CEV-management sentence.

AWS: recorded behavior only for the message (the Smithy model binds only CustomDBEngineVersionNotFoundFault (404) and InvalidCustomDBEngineVersionStateFault (400) to DeleteCustomDBEngineVersion; Engine is @required CustomEngineName pattern ^[A-Za-z0-9-]{1,35}$ and EngineVersion is @required CustomEngineVersion pattern ^[a-z0-9_.-]{1,60}$, so neither AccessDenied nor its text is modeled). The recorded response for a caller without the RDS-Custom entitlement — or for an (Engine, EngineVersion) pair that does not exist — is an AccessDenied client error whose message is deliberately identical in both cases so the action cannot be used to probe existence. Its text is entirely FIXED, with no variable parts interpolated from the request or from state: "The custom engine version you requested may not exist or you don't have the required permission." RDS answers this before the (Engine, EngineVersion) pair is evaluated, so no CEV, account id, ARN or caller identity appears in it.

Emulator: returns Code 'AccessDenied' (correct) with Type 'Sender', but the message is the generic IAM authorization sentence: "User: <caller-arn> is not authorized to perform: rds:DeleteCustomDBEngineVersion on resource: <cev-arn>/* because no identity-based policy allows the rds:DeleteCustomDBEngineVersion action". Everything in it except the fixed IAM boilerplate is interpolated — the caller principal ARN, the constructed cev ARN built from the requested Engine and EngineVersion, and a trailing "/*" — i.e. the emulator both invents wording AWS does not use for this action and leaks the requested engine version back to an unauthorized caller.

Why: a client that matches or surfaces RDS's documented CEV wording (log scrapers, retry/classification wrappers, and operator runbooks that distinguish "entitlement gate" from a per-request validation failure) sees a message that never occurs against RDS. Worse, because the emulator echoes the requested Engine/EngineVersion inside an authorization failure, it turns an intentionally existence-opaque refusal into an existence/identity oracle, which is exactly the property RDS's single fixed sentence exists to remove.

Fix: the refusal path of the DeleteCustomDBEngineVersion handler (services/service-com-amazonaws-rds-amazonrdsv19/src/ops/delete_custom_db_engine_version.rs) must emit the fixed RDS sentence for this action's AccessDenied rather than delegating to the crate's generic IAM not-authorized message builder; the code (AccessDenied) and the fact that it fires before the (Engine, EngineVersion) lookup are already right and must stay that way.

Traces: rds.delete-custom-engine-version-boundary (mismatch) — the check's positive control DescribeDBEngineVersions(Engine="mysql") succeeds, so general RDS read access in the target is healthy and only this action's refusal text diverges.

## Evidence
Check rds.delete-custom-engine-version-boundary, assertion at check_rds_delete_custom_engine_version_boundary.py:53.

Expected (AWS): Code 'AccessDenied', Message exactly "The custom engine version you requested may not exist or you don't have the required permission." (whole string is fixed text; no request- or state-derived parts).

Actual (emulator): {'Type': 'Sender', 'Code': 'AccessDenied', 'Message': 'User: arn:aws:iam::000000000000:user/emulator is not authorized to perform: rds:DeleteCustomDBEngineVersion on resource: arn:aws:rds:us-west-2:000000000000:cev:custom-oracle-ee/<generated-engine-version>/* because no identity-based policy allows the rds:DeleteCustomDBEngineVersion action'}

AssertionError: AccessDenied message did not match the known entitlement-gate phrasing.

In the actual message the fixed parts are the IAM boilerplate "User: ... is not authorized to perform: ... on resource: ... because no identity-based policy allows the ... action"; the variable parts are the caller principal ARN, the cev ARN assembled from the request's Engine (custom-oracle-ee) and EngineVersion (a per-run generated name), and the account id/region of the emulator — none of which belong in this error class at all.

Smithy: DeleteCustomDBEngineVersion errors = CustomDBEngineVersionNotFoundFault (404), InvalidCustomDBEngineVersionStateFault (400); AccessDenied is unmodeled, so the recorded wording is the requirement.

## Steps to Reproduce
1. Boot the emulator and point an rds client at it.
2. Call DescribeDBEngineVersions(Engine="mysql", MaxRecords=20) — succeeds, confirming RDS reads are healthy.
3. Call DeleteCustomDBEngineVersion(Engine="custom-oracle-ee", EngineVersion=<any name that does not exist>).
4. Observe the ClientError: Code is AccessDenied (correct) but Message is the generic IAM "User: ... is not authorized to perform: rds:DeleteCustomDBEngineVersion ..." sentence, with the requested engine version interpolated, instead of AWS's fixed "The custom engine version you requested may not exist or you don't have the required permission."
5. Replay rds.delete-custom-engine-version-boundary to confirm.
