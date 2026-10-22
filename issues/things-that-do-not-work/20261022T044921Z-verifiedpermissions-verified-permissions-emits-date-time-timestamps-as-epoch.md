Issue-Source: trace-sweep-divergence
Issue-Trace: verifiedpermissions-authorization-decisions
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: verifiedpermissions-authorization-decisions verifiedpermissions-batch-get-policy-across-stores verifiedpermissions-policy-crud-with-schema verifiedpermissions-policy-store-alias-workflow verifiedpermissions-policy-store-deletion-protection verifiedpermissions-policy-template-workflow verifiedpermissions-schema-management
Issue-Shape-Id: com.amazonaws.verifiedpermissions#VerifiedPermissions
Issue-Key: verifiedpermissions|CreatePolicyStore|2xx->2xx|$.createdDate
Issue-Key: verifiedpermissions|CreatePolicyStore|2xx->2xx|$.lastUpdatedDate
Issue-Key: verifiedpermissions|CreatePolicy|2xx->2xx|$.createdDate
Issue-Key: verifiedpermissions|CreatePolicy|2xx->2xx|$.lastUpdatedDate
Issue-Commit: 38b62e174af309db3e46216fa3b3f7ac7f6f5b84

# Verified Permissions emits date-time timestamps as epoch numbers instead of ISO-8601 strings

> _Note: this issue was identified against the code base at commit `38b62e174af309db3e46216fa3b3f7ac7f6f5b84`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePolicyStore

## Description
The emulator serializes Verified Permissions timestamp members as bare epoch-second floating point numbers rather than as the ISO-8601 date-time strings the model requires. This bundle has 2 causes.

Cause 1 of 2: CreatePolicyStore response timestamps.
AWS: Smithy com.amazonaws.verifiedpermissions#CreatePolicyStore output members createdDate and lastUpdatedDate are both required and target com.amazonaws.verifiedpermissions#TimestampFormat, a timestamp shape carrying @timestampFormat("date-time"). On the awsJson/REST-JSON wire that means an RFC3339 / ISO-8601 string (e.g. "2026-09-06T06:27:19.145260Z"), not a numeric epoch value. The recorded responses carry a date-time string in both members.
Emulator: returns both members as JSON numbers, e.g. $.createdDate = 1792644246.914848 and $.lastUpdatedDate = 1792644246.914848. Status is 200 and every other member matches; the divergence is confined to the wire type/format of the two timestamp members. The same wrong encoding reproduces on 8 clustered steps across the authorization-decisions, batch-get-policy-across-stores, policy-crud-with-schema, policy-store-alias-workflow, policy-store-deletion-protection, policy-template-workflow and schema-management fixtures.
Why: @timestampFormat("date-time") makes the string form part of the contract, so a generated SDK client deserializing CreatePolicyStoreOutput either raises a parse error on the number or silently produces a wrong instant. Any client that round-trips the value (logging a store's creation time, comparing it to a CloudTrail record, persisting it) gets a type it cannot interpret as AWS does.
Fix: the verifiedpermissions crate's timestamp serialization for the TimestampFormat shape, in the CreatePolicyStore response serializer (services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/src/ops/create_policy_store.rs and whatever shared timestamp helper it calls) must render date-time timestamps as RFC3339 strings.
Traces: verifiedpermissions-authorization-decisions#setup#1.

Cause 2 of 2: CreatePolicy response timestamps.
AWS: Smithy CreatePolicy output members createdDate and lastUpdatedDate are required and target the same TimestampFormat shape with @timestampFormat("date-time"), so both are ISO-8601 strings.
Emulator: returns them as numbers, e.g. $.createdDate = 1792644246.9459229, $.lastUpdatedDate = 1792644246.9459229, with status 200. Clustered over 4 steps (authorization-decisions, batch-get-policy-across-stores x2, policy-store-alias-workflow).
Why: same as cause 1 — CreatePolicyOutput no longer deserializes to the modeled type, so policy creation timestamps are unusable by a real SDK client.
Fix: the CreatePolicy response serializer in the same crate (src/ops/create_policy.rs) / the shared TimestampFormat encoder.
Traces: verifiedpermissions-authorization-decisions#setup#2.

Note: the same epoch-number encoding also shows up inside the GetPolicyStore and policy-store-alias clusters filed separately (those clusters carry additional, distinct defects); fixing the shared TimestampFormat encoder should clear the timestamp portion of all of them, but each of those issues must still be closed on its own remaining causes.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator.

Cause 1 — fixture verifiedpermissions-authorization-decisions, step setup#1, CreatePolicyStore:
  expected status 200, actual status 200
  $.createdDate: value_mismatch (expected a <date-time> string, got 1792644246.914848)
  $.lastUpdatedDate: value_mismatch (expected a <date-time> string, got 1792644246.914848)
  cluster of 8 affected steps: verifiedpermissions-authorization-decisions#setup#1, verifiedpermissions-batch-get-policy-across-stores#setup#1, verifiedpermissions-batch-get-policy-across-stores#setup#2, verifiedpermissions-policy-crud-with-schema#setup#1, verifiedpermissions-policy-store-alias-workflow#task#1, verifiedpermissions-policy-store-deletion-protection#task#1, verifiedpermissions-policy-template-workflow#setup#1, verifiedpermissions-schema-management#setup#1

Cause 2 — fixture verifiedpermissions-authorization-decisions, step setup#2, CreatePolicy:
  expected status 200, actual status 200
  $.createdDate: value_mismatch (expected a <date-time> string, got 1792644246.9459229)
  $.lastUpdatedDate: value_mismatch (expected a <date-time> string, got 1792644246.9459229)
  cluster of 4 affected steps: verifiedpermissions-authorization-decisions#setup#2, verifiedpermissions-batch-get-policy-across-stores#setup#3, verifiedpermissions-batch-get-policy-across-stores#setup#4, verifiedpermissions-policy-store-alias-workflow#task#5

Model evidence: com.amazonaws.verifiedpermissions#TimestampFormat = timestamp with timestampFormat "date-time"; CreatePolicyStoreOutput.createdDate/.lastUpdatedDate and CreatePolicyOutput.createdDate/.lastUpdatedDate are required and target it. Other recorded GetPolicyStore responses in the same corpus show the concrete expected form, e.g. "2026-09-06T06:27:19.14526Z".

## Steps to Reproduce
1. Boot the emulator and point a Verified Permissions client at its data listener.
2. CreatePolicyStore(validationSettings={"mode":"OFF"}) and inspect the raw JSON response body: createdDate and lastUpdatedDate come back as JSON numbers rather than ISO-8601 strings (cause 1).
3. CreatePolicy(policyStoreId=<store>, definition={"static":{"statement":"permit(principal, action, resource);"}}) and inspect the raw body: createdDate and lastUpdatedDate are again numbers (cause 2).
4. Re-replay fixture verifiedpermissions-authorization-decisions step setup#1 (cause 1) and step setup#2 (cause 2).
