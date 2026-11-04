Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__iam__sts.xsvc-dynamodb-2-leading-keys-principal-tag-variable-cross-tenant-denied
Issue-Key: dynamodb__iam__sts.xsvc-dynamodb-2-leading-keys-principal-tag-variable-cross-tenant-denied|_denied#20|AssertionError|raise AssertionError(f"call was expected to be DENIED but succeeded: {result!r}")
Issue-Commit: f4e5251e6df88363d8e23ace2ff5bff3be31b423

# DynamoDB ignores dynamodb:LeadingKeys with a principal-tag variable, so cross-tenant reads succeed

> _Note: this issue was identified against the code base at commit `f4e5251e6df88363d8e23ace2ff5bff3be31b423`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetItem

## Description
One net-new root cause in the dynamodb__iam__sts group.

Cause 1 of 1: a `dynamodb:LeadingKeys` condition whose value is a principal-tag variable (`${aws:PrincipalTag/...}`) is not enforced.
AWS: the identity policy allows the data-plane read only when the request's leading key equals the caller's principal tag; a read of another tenant's partition is refused with `AccessDeniedException` (HTTP 400) naming the principal, the action and the resource.
Emulator: the call SUCCEEDS and returns the other tenant's item.
Why: the single mechanism used to isolate tenants inside one DynamoDB table does not hold, so one tenant reads another's data — anything built on this pattern is validated against an emulator that does not enforce it. (The open issue 20261024T065715Z-dynamodb-iam-sts-dynamodb-data-plane-reads-are-never-authorized-identity.md carries sibling leading-keys checks; it does not carry this replay key, and the condition-variable substitution is a distinct behavior from plain leading-keys enforcement.)
Fix: the DynamoDB data-plane authorization path — condition-key evaluation for dynamodb:LeadingKeys, including `${aws:PrincipalTag/<key>}` substitution from the session.
Traces: dynamodb__iam__sts.xsvc-dynamodb-2-leading-keys-principal-tag-variable-cross-tenant-denied

## Evidence
[mismatch] dynamodb__iam__sts.xsvc-dynamodb-2-leading-keys-principal-tag-variable-cross-tenant-denied — AssertionError: call was expected to be DENIED but succeeded: {'Item': {'v': {'S': 'tenant-two'}, 'sk': {'S': 'seed'}, 'pk': {'S': 'tenant-two'}}}.

## Steps to Reproduce
Boot the emulator and replay dynamodb__iam__sts.xsvc-dynamodb-2-leading-keys-principal-tag-variable-cross-tenant-denied: assume a role with a principal tag, attach a policy whose dynamodb:LeadingKeys condition is ${aws:PrincipalTag/tenant}, then GetItem a partition belonging to another tenant.
