Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__iam__sts.ddb-rbp-resource-policy-only-grant-is-action-scoped-write-denied dynamodb__iam__sts.xsvc-dynamodb-2-explicit-deny-beats-identity-and-resource-policy-allow dynamodb__iam__sts.xsvc-dynamodb-2-leading-keys-scan-unguarded-leaks-guarded-denied dynamodb__iam__sts.xsvc-dynamodb-2-leading-keys-session-policy-other-partition-denied dynamodb__iam__sts.xsvc-dynamodb-2-attributes-select-specific-full-item-read-denied
Issue-Key: dynamodb__iam__sts.ddb-rbp-resource-policy-only-grant-is-action-scoped-write-denied|_denied#11|AssertionError|raise AssertionError(f"the call was expected to be refused but succeeded: {result!r}")
Issue-Key: dynamodb__iam__sts.xsvc-dynamodb-2-explicit-deny-beats-identity-and-resource-policy-allow|_denied#21|AssertionError|raise AssertionError(f"call was expected to be DENIED but succeeded: {result!r}")
Issue-Key: dynamodb__iam__sts.xsvc-dynamodb-2-leading-keys-scan-unguarded-leaks-guarded-denied|_denied#25|AssertionError|raise AssertionError(f"call was expected to be DENIED but succeeded: {result!r}")
Issue-Key: dynamodb__iam__sts.xsvc-dynamodb-2-leading-keys-session-policy-other-partition-denied|_denied#23|AssertionError|raise AssertionError(f"call was expected to be DENIED but succeeded: {result!r}")
Issue-Key: dynamodb|GetItem|NotImplemented
Issue-Commit: 2e744e838767d761bb8115d027d1acb9d75669f4

# DynamoDB data-plane reads are never authorized (identity, session and resource policies all ignored), and GetItem with ProjectionExpression is a 501 stub (2 causes)

> _Note: this issue was identified against the code base at commit `2e744e838767d761bb8115d027d1acb9d75669f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetItem

## Description
This issue bundles TWO distinct net-new root causes for the DynamoDB data plane as exercised by assumed-role sessions. A fixer must close both before submitting.

Cause 1 of 2: GetItem/Query/Scan under a scoped session are served regardless of the identity policy, the session policy, the table's resource-based policy and any explicit Deny.
AWS: DynamoDB authorizes every data-plane call against the caller's effective permissions — identity policy INTERSECT session policy INTERSECT resource-based policy, with an explicit Deny in any of them winning. A caller outside that set is refused with AccessDeniedException, HTTP 400 (message of the form "User: <session arn> is not authorized to perform: dynamodb:GetItem on resource: <table arn> ..."), and the condition keys dynamodb:LeadingKeys and dynamodb:Attributes further restrict which partitions and which attributes a read may touch (recorded behavior; the DynamoDB model carries no authorization trait, so the oracle is the recorded AWS response).
Emulator: every one of these reads returns HTTP 200 with the item. Replay shows the resource-policy-only principal reading an item its action-scoped grant does not cover, a principal with an explicit Deny reading the item anyway, and a session restricted by dynamodb:LeadingKeys to its own tenant partition reading `pk=tenant-other` ({'pk': {'S': 'tenant-other'}, 'v': {'S': 'tenant-other-seed'}}).
Why: multi-tenant isolation built on LeadingKeys/Attributes — the standard DynamoDB pattern — is unenforced, so a design validated against the emulator leaks other tenants' items; and the AccessDeniedException path clients must handle never occurs. Open issue 20261024T015148Z-dynamodb-does-not-enforce-iam-listtables-succeeds-for.md records the control-plane ListTables instance of the same absence and OWNS the one-denied-principal check; it does not carry these item-level reads or the LeadingKeys/Attributes condition keys, so they are filed here and that issue is named for cross-reference.
Fix: an authorization step in the DynamoDB data-plane handlers (GetItem/Query/Scan) that evaluates the request against the caller's session, the table's resource policy, and the dynamodb:LeadingKeys / dynamodb:Attributes condition keys.
Traces: dynamodb__iam__sts.ddb-rbp-resource-policy-only-grant-is-action-scoped-write-denied, dynamodb__iam__sts.xsvc-dynamodb-2-explicit-deny-beats-identity-and-resource-policy-allow, dynamodb__iam__sts.xsvc-dynamodb-2-leading-keys-scan-unguarded-leaks-guarded-denied, dynamodb__iam__sts.xsvc-dynamodb-2-leading-keys-session-policy-other-partition-denied.

Cause 2 of 2: GetItem with a ProjectionExpression is refused as unimplemented.
AWS: GetItem accepts ProjectionExpression (with ExpressionAttributeNames) and returns only the projected attributes, HTTP 200; it is a modeled member of GetItemInput, not an optional extension.
Emulator: the call is refused with code NotImplemented, message "operation GetItem with ProjectionExpression is scaffolded but not implemented in this emulator" — so the projected read cannot even be attempted, and the dynamodb:Attributes authorization leg that depends on it is unreachable.
Why: any client that projects attributes (the recommended way to limit read cost) gets a 501-class refusal for a request AWS serves.
Fix: implement ProjectionExpression handling in the DynamoDB GetItem handler (and the expression evaluator it shares).
Traces: dynamodb__iam__sts.xsvc-dynamodb-2-attributes-select-specific-full-item-read-denied.

## Evidence
Replay of suite simparity-behavioral-ga-prod-v1.

Cause 1 — "AssertionError: the call was expected to be refused but succeeded: {'Item': {'v': {'S': 'seed'}, 'id': {'S': 'seed'}}}" (resource-policy-only grant, action-scoped); "call was expected to be DENIED but succeeded: {'Item': {'pk': {'S': 'seed'}, 'v': {'S': 'seed'}}}" (explicit Deny beats identity+resource allow); "call was expected to be DENIED but succeeded: {'Item': {'sk': {'S': 'seed'}, 'v': {'S': 'tenant-other-seed'}, 'pk': {'S': 'tenant-other'}}}" (LeadingKeys-guarded partition read, twice: unguarded-scan and session-policy variants). Expected in each case: AccessDeniedException, HTTP 400.

Cause 2 — "projected GetItem on the allowed partition still failing with 'NotImplemented' after 90.0s: 'operation GetItem with ProjectionExpression is scaffolded but not implemented in this emulator'".

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite.
2. Re-replay the five check ids in trace_ids.
3. Minimal manual repro for cause 1: create a table, put an item at pk='tenant-other', assume a role whose session policy allows dynamodb:GetItem only with Condition dynamodb:LeadingKeys = ['tenant-self'], then GetItem pk='tenant-other' with those credentials — the emulator returns the item instead of AccessDeniedException/400.
4. Minimal manual repro for cause 2: GetItem on any table with ProjectionExpression='v' — the emulator answers NotImplemented.
