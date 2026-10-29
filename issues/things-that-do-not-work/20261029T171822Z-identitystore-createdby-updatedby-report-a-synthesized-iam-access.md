Issue-Source: trace-sweep-divergence
Issue-Trace: identitystore-create-user-describe-reflects-fields
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: identitystore-create-user-describe-reflects-fields identitystore-user-without-optional-fields-omitted identitystore-group-without-description-field-omitted identitystore-update-group-description-persists
Issue-Shape-Id: com.amazonaws.identitystore#AWSIdentityStore
Issue-Key: identitystore|DescribeUser|2xx->2xx|$.CreatedBy
Issue-Key: identitystore|DescribeUser|2xx->2xx|$.UpdatedBy
Issue-Key: identitystore|DescribeGroup|2xx->2xx|$.CreatedBy
Issue-Key: identitystore|DescribeGroup|2xx->2xx|$.UpdatedBy
Issue-Key: identitystore|ListGroups|2xx->2xx|$.Groups[*].CreatedBy
Issue-Key: identitystore|ListGroups|2xx->2xx|$.Groups[*].UpdatedBy
Issue-Commit: 9fc346ef5fefb977b4ef554789eaaa17a76a6a1a

# identitystore CreatedBy/UpdatedBy report a synthesized IAM access-key ARN

> _Note: this issue was identified against the code base at commit `9fc346ef5fefb977b4ef554789eaaa17a76a6a1a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeUser

## Description
Every identity-store read that reports the audit members CreatedBy / UpdatedBy reports them in a form the recorded corpus does not accept. The three causes below are the same defect surfacing on the three operations that serialize those members, and all three come from one helper, so they are bundled; a fixer must close all THREE before submitting.

Shared root cause: crate::state::caller_principal (services/service-com-amazonaws-identitystore-awsidentitystore/src/state.rs, the `caller_principal` fn ~line 1160) builds the stored principal as `engine::emulator_arn("iam", "", format!("user/{akid}"))`, i.e. an IAM-user ARN whose resource part is the SigV4 access key id taken off the request headers (and, for an unsigned request, the framework's `EMULATOR<hex>` fallback access key id). Every create/update handler stamps that string into Group.created_by / Group.updated_by / User.created_by / User.updated_by, and every describe/list serializes it back out.

Cause 1 of 3: DescribeUser.CreatedBy / DescribeUser.UpdatedBy.
AWS: Smithy DescribeUser output members CreatedBy and UpdatedBy, both target StringType, documented as "The identifier of the user or system that created the user" / "... that last updated the user". The model constrains the string no further, so the shape is pinned by the recording only: the recorded responses carry an identifier for the creating principal whose shape the replay harness asserts (the value itself is redacted to '<shaped-not-asserted>' because the concrete principal is environment-specific). Recorded behavior only for the exact lexical form; an IAM-user ARN built from an access key id is NOT that shape. Hypothesis: AWS reports an identity-store/IAM Identity Center principal identifier rather than an `arn:aws:iam::<account>:user/<access-key-id>` string.
Emulator: both members come back as `arn:aws:iam::<account-id>:user/<access-key-id>` — e.g. an ARN whose resource is the fallback `EMULATOR<hex>` key id for an unsigned replay — and the harness reports `$.CreatedBy: value_mismatch` and `$.UpdatedBy: value_mismatch` with HTTP 200 on both sides.
Why: a client that reads CreatedBy/UpdatedBy to attribute a user record (audit export, drift detection, SCIM reconciliation) gets a string of a different class than AWS returns, so any parse or comparison against a principal identifier fails, and the identifier it does get is an access key id that changes whenever the caller's credentials or signing mode change.
Fix: crate::state::caller_principal and the create/update handlers that stamp it (ops/create_user.rs, ops/update_user.rs) — the derivation of the stored principal, not the serializer, which faithfully emits what state holds.
Traces: identitystore-create-user-describe-reflects-fields#task#4 (representative), identitystore-user-without-optional-fields-omitted#task#4, identitystore-user-without-optional-fields-omitted#task#6.

Cause 2 of 3: DescribeGroup.CreatedBy / DescribeGroup.UpdatedBy.
AWS: Smithy DescribeGroup output members CreatedBy and UpdatedBy (StringType), "The identifier of the user or system that created the group" / "... that last updated the group"; same shape pinned by the recording as in Cause 1.
Emulator: DescribeGroup returns the same synthesized `arn:aws:iam::<account-id>:user/<access-key-id>` for both members (HTTP 200 expected and actual; `$.CreatedBy` and `$.UpdatedBy` value_mismatch), because describe_group.rs copies group.created_by / group.updated_by straight out of the record created by caller_principal.
Why: identical to Cause 1 for group records — group audit attribution reads as an IAM access-key ARN instead of the principal identifier AWS reports.
Fix: same helper plus ops/create_group.rs and ops/update_group.rs, which stamp the record; ops/describe_group.rs needs no change.
Traces: identitystore-group-without-description-field-omitted#task#4 (representative of this cause), identitystore-group-without-description-field-omitted#task#7, identitystore-update-group-description-persists#task#6.

Cause 3 of 3: ListGroups Groups[].CreatedBy / Groups[].UpdatedBy.
AWS: Smithy shape Group carries the same CreatedBy/UpdatedBy members, and ListGroups returns a required list of Group, so each listed element must carry the identifier in the same shape a describe does.
Emulator: every listed element carries the synthesized IAM access-key ARN — `$.Groups[0].CreatedBy` and `$.Groups[0].UpdatedBy` value_mismatch at HTTP 200 — so the divergence is present in the paginated projection as well as in the single-resource read, and a fix applied only to the describe path would leave the list path wrong.
Why: a client enumerating groups and attributing each one gets the wrong class of identifier for the whole page.
Fix: same stored-principal derivation; the Group record-to-modeled conversion in state.rs (Group::to_modeled) and ops/list_groups.rs just forward it.
Traces: identitystore-group-without-description-field-omitted#task#5.

No open issue covers this (list_existing_issues for 'identitystore' returns none).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from mainline, 3 divergence clusters, 7 affected steps, all HTTP 200 on both sides:

1) fixture identitystore-create-user-describe-reflects-fields step task#4, DescribeUser:
   $.CreatedBy: value_mismatch (expected '<shaped-not-asserted>', got 'arn:aws:iam::000000000000:user/EMULATOR4DD12BD16FC0')
   $.UpdatedBy: value_mismatch (expected '<shaped-not-asserted>', got 'arn:aws:iam::000000000000:user/EMULATOR4DD12BD16FC0')
   also affected: identitystore-user-without-optional-fields-omitted#task#4, identitystore-user-without-optional-fields-omitted#task#6

2) fixture identitystore-group-without-description-field-omitted step task#4, DescribeGroup:
   $.CreatedBy / $.UpdatedBy: value_mismatch (expected '<shaped-not-asserted>', got 'arn:aws:iam::000000000000:user/EMULATOR7693A9EB161E')
   also affected: identitystore-group-without-description-field-omitted#task#7, identitystore-update-group-description-persists#task#6

3) fixture identitystore-group-without-description-field-omitted step task#5, ListGroups:
   $.Groups[0].CreatedBy / $.Groups[0].UpdatedBy: value_mismatch (expected '<shaped-not-asserted>', got 'arn:aws:iam::000000000000:user/EMULATOR7693A9EB161E')

The account field (000000000000) and the concrete access key id (EMULATOR...) are environment/per-run values and are NOT the requirement; what diverges is the CLASS of identifier serialized into CreatedBy/UpdatedBy. Source of the value: services/service-com-amazonaws-identitystore-awsidentitystore/src/state.rs, caller_principal() -> engine::emulator_arn("iam", "", format!("user/{akid}")) with akid from engine::extract_akid(headers).

## Steps to Reproduce
1. Clone mainline and boot the emulator; identitystore is bootstrapped (service-com-amazonaws-identitystore-awsidentitystore is in get_bootstrap_status().bootstrapped).
2. Replay fixture identitystore-create-user-describe-reflects-fields and stop at step task#4: CreateUser into the seeded identity store, then DescribeUser on the returned UserId. Inspect $.CreatedBy / $.UpdatedBy in the 200 body.
3. Replay fixture identitystore-group-without-description-field-omitted: step task#4 (DescribeGroup) and step task#5 (ListGroups) show the same identifier in $.CreatedBy/$.UpdatedBy and $.Groups[0].CreatedBy/$.Groups[0].UpdatedBy.
4. Each one returns an arn:aws:iam::<account>:user/<access-key-id> string instead of the principal identifier shape the recordings pin.
