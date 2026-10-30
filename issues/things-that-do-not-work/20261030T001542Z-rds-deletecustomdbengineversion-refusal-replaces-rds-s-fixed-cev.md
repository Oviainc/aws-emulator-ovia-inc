Issue-Source: trace-sweep-divergence
Issue-Trace: rds-custom-engine-version
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rds-custom-engine-version
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|DeleteCustomDBEngineVersion|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: 263537c8d0bbf6adc085b4147c3053be10a9244f

# rds DeleteCustomDBEngineVersion refusal replaces RDS's fixed CEV sentence with generic IAM text and leaks the request

> _Note: this issue was identified against the code base at commit `263537c8d0bbf6adc085b4147c3053be10a9244f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteCustomDBEngineVersion

## Description
There is 1 cause to close. The same defect is described from the behavioral suite in the open issue 20261029T233146Z-rds-deletecustomdbengineversion-accessdenied-carries-a-generic-iam-not.md, which is grounded in check rds.delete-custom-engine-version-boundary and carries no wire replay key; this issue binds the wire fixture step so the fix can be re-replayed against the recorded transcript.

Cause 1 of 1: the AccessDenied refusal message is emulator-authored IAM boilerplate instead of RDS's fixed custom-engine-version sentence.
AWS: Smithy binds only CustomDBEngineVersionNotFoundFault (404, awsQueryError CustomDBEngineVersionNotFoundFault) and InvalidCustomDBEngineVersionStateFault (400) to DeleteCustomDBEngineVersion; Engine is @required (CustomEngineName, length 1..35, pattern ^[A-Za-z0-9-]{1,35}$) and EngineVersion is @required (CustomEngineVersion, length 1..60, pattern ^[a-z0-9_.-]{1,60}$). AccessDenied and its wording are therefore unmodeled - recorded behavior only. The recorded refusal is HTTP 403 with Code AccessDenied and a message that is entirely FIXED text, with no variable parts at all: "The custom engine version you requested may not exist or you don't have the required permission." RDS deliberately serves the same sentence whether the caller lacks the RDS Custom entitlement or the (Engine, EngineVersion) pair does not exist, and serves it before the pair is evaluated, so no CEV name, ARN, account id, region or caller identity appears in it.
Emulator: answers the right status (403) and the right Code (AccessDenied) but renders the crate's generic IAM denial instead: "User: <caller-principal-arn> is not authorized to perform: rds:DeleteCustomDBEngineVersion on resource: <cev-arn>/* because no identity-based policy allows the rds:DeleteCustomDBEngineVersion action". In that string the IAM boilerplate ("User: … is not authorized to perform: … on resource: … because no identity-based policy allows the … action") is the fixed part, while the caller principal ARN, the cev ARN assembled from the request's Engine and EngineVersion, the account id/region and the trailing "/*" are interpolated from the request and from the emulator's own identity - none of which belong in this error class.
Why: a client that classifies or surfaces this refusal by its documented wording (retry/classification wrappers, log scrapers, operator runbooks distinguishing an entitlement gate from a per-request validation failure) sees a message that never occurs against RDS. It is also an information leak in the opposite direction from AWS's intent: by echoing the requested Engine/EngineVersion back inside an authorization failure, the emulator turns an intentionally existence-opaque refusal into an existence oracle, which is precisely the property the single fixed sentence exists to remove.
Fix: the refusal path of the DeleteCustomDBEngineVersion handler (services/service-com-amazonaws-rds-amazonrdsv19/src/ops/delete_custom_db_engine_version.rs) must emit this action's fixed RDS sentence rather than delegating to the generic IAM not-authorized message builder; the 403 status, the AccessDenied code and the fact that the gate fires before the (Engine, EngineVersion) lookup are already correct and must stay.
Traces: rds-custom-engine-version#task#2.

## Evidence
Replay of simparity-wire-aws-v1 (emulator_up=true, rds answers).

rds-custom-engine-version#task#2, DeleteCustomDBEngineVersion, expected 403 / actual 403: $.ErrorResponse.Error.Message value_mismatch.
Expected (AWS): "The custom engine version you requested may not exist or you don't have the required permission." - the entire string is fixed text; there are no request- or state-derived parts.
Actual (emulator): "User: arn:aws:iam::000000000000:user/emulator is not authorized to perform: rds:DeleteCustomDBEngineVersion on resource: arn:aws:rds:us-west-2:000000000000:cev:custom-oracle-ee/sim2real-nonexistent-cev/* because no identity-based policy allows the rds:DeleteCustomDBEngineVersion action".

In the actual message the fixed text is the IAM boilerplate; the variable parts are the caller principal ARN, the cev ARN built from the request's Engine (custom-oracle-ee) and EngineVersion, and the emulator's account id and region. The recorded principal ARN, account id, region and CEV name are per-run values and are not themselves the requirement - the requirement is the fixed AWS sentence with no interpolation.

Status (403) and Code (AccessDenied) match; only the message diverges.

Smithy consulted: DeleteCustomDBEngineVersion input {Engine @required CustomEngineName ^[A-Za-z0-9-]{1,35}$, EngineVersion @required CustomEngineVersion ^[a-z0-9_.-]{1,60}$}, output DBEngineVersion, errors [CustomDBEngineVersionNotFoundFault 404, InvalidCustomDBEngineVersionStateFault 400] - AccessDenied is unmodeled, so the recorded wording is the requirement.

## Steps to Reproduce
1. Boot the emulator and point an rds client at it.
2. Sanity-check RDS reads with DescribeDBEngineVersions(Engine='mysql', MaxRecords=20).
3. Call DeleteCustomDBEngineVersion(Engine='custom-oracle-ee', EngineVersion=<any name that does not exist>).
4. Observe HTTP 403 with Code 'AccessDenied' (correct) but Message set to the generic IAM 'User: … is not authorized to perform: rds:DeleteCustomDBEngineVersion …' sentence with the requested engine version interpolated, instead of AWS's fixed "The custom engine version you requested may not exist or you don't have the required permission."
5. Re-replay rds-custom-engine-version step task#2 to confirm.
