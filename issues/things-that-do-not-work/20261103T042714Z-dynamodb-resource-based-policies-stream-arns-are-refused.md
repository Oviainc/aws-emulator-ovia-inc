Issue-Severity: HIGH
Issue-Commit: a2c0c19d1874b6d8f510c899cc1a51333f7d4b07

# DynamoDB resource-based policies: stream ARNs are refused, every ARN/JSON refusal is invented, policy semantics are unvalidated and the IAM default Version is never filled in (8 causes)

> _Note: this issue was identified against the code base at commit `a2c0c19d1874b6d8f510c899cc1a51333f7d4b07`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutResourcePolicy

## Description
Eight net-new root causes in PutResourcePolicy / GetResourcePolicy. Open issues 20261030T060227Z-dynamodb-resource-policy-refusals-name-the-wrong-resource.md, 20261029T180026Z-dynamodb-partiql-resource-policy-arn-addressing-crc32-header.md and 20261103T022510Z-dynamodb-control-plane-batch-transact-apis-legacy-parameters.md are adjacent; the suite did not mark any check below as owned by them.

=== Cause 1 of 8: a stream ARN is refused outright, so a stream cannot hold its own policy ===
AWS: Smithy models PutResourcePolicy.ResourceArn as ResourceArnString (@length 1..1283) and AWS accepts BOTH a table ARN and that table's stream ARN — a table and its stream hold independent resource policies, and a table-scoped action placed in a STREAM policy is refused on the ACTION, with the stream ARN itself accepted.
Emulator: any ARN with a /stream/ segment is refused with ValidationException 'Invalid TableArn: Invalid ResourceArn provided as input <arn>', so no stream policy can be written and the action-scope rule is unreachable.
Why: stream-level access control cannot be configured or read at all.
Fix: ResourceArn parsing in the resource-policy handler must accept the stream ARN form and route it to stream-scoped policy state.
Traces: ddb-rbp-stream-and-table-hold-independent-policies, ddb-rbp-table-action-in-a-stream-policy-rejected.

=== Cause 2 of 8: the invalid-ARN refusal uses the emulator's sentence ===
AWS: an ARN that is not a table or stream ARN — an index ARN, or a table ARN for another region — is ValidationException with AWS's fixed sentences: "One or more parameter values were invalid: Invalid resource arn provided, only table or stream is accepted. Provided resource arn: <arn>" for a non-table/stream resource type, and "One or more parameter values were invalid: Invalid resource Arn: <arn>" for a cross-region ARN. Only the ARN is substituted.
Emulator: both answer 'Invalid TableArn: Invalid ResourceArn provided as input <arn>'.
Why: the message is the only thing telling the caller WHICH rule it broke; a client cannot distinguish a wrong resource type from a wrong region.
Fix: the ARN validator in the resource-policy handler, with one sentence per defect class.
Traces: ddb-rbp-another-regions-table-arn-rejected, ddb-rbp-index-arn-as-policy-resource-rejected.

=== Cause 3 of 8: the not-found refusal names the ARN instead of the table ===
AWS: GetResourcePolicy/PutResourcePolicy against a table that does not exist (never created, or deleted) is ResourceNotFoundException "Requested resource not found: Table: <tableName> not found" — the TABLE NAME, not the ARN.
Emulator: 'Requested resource not found: ResourceArn: <full arn> not found'.
Why: exact-match error handling fails and the message leaks a different identifier than AWS's.
Fix: the not-found renderer in the resource-policy handler must extract and name the table name.
Traces: ddb-rbp-never-created-table-arn-rejected, ddb-rbp-deleted-table-drops-out-of-the-policy-derivation.

=== Cause 4 of 8: the malformed-policy refusal leaks the JSON parser's position ===
AWS: a policy document that is not valid JSON is ValidationException "One or more parameter values were invalid: Invalid policy document: This policy contains invalid Json" — fixed text, no parser detail.
Emulator: 'Resource-based policy is not valid JSON: EOF while parsing a string at line 1 column 234', which is the serde parser's own message.
Why: the emulator's internals are exposed and the message does not match AWS's.
Fix: map the policy-document parse failure onto AWS's single sentence in the resource-policy handler.
Traces: ddb-rbp-malformed-policy-json-rejected.

=== Cause 5 of 8: the 20480-byte policy ceiling is refused with the wrong error class ===
AWS: a resource policy document over 20480 bytes is refused with ValidationException. (LimitExceededException IS in PutResourcePolicy's modeled error set, but recorded AWS behavior for the document-size ceiling is ValidationException.)
Emulator: LimitExceededException with the emulator-authored message 'Policy size exceeds the maximum supported size of 20480 bytes for resource <arn>: <n> bytes'.
Why: LimitExceededException is a throttling/quota class that SDK retry policies treat differently from a validation failure, so an unfixable oversized policy is retried.
Fix: the policy-size check in the resource-policy handler must raise ValidationException with AWS's sentence.
Traces: ddb-rbp-policy-over-20480-bytes-rejected.

=== Cause 6 of 8: policy CONTENT is never validated ===
AWS: PutResourcePolicy refuses a policy whose Principal is the public wildcard, whose Action names a service other than dynamodb:, or whose Principal names a principal that does not exist — each with ValidationException.
Emulator: all three succeed and return a RevisionId.
Why: a policy AWS would never accept is stored and then read back, so an operator believes an access rule is in force that real AWS would have refused — and in the wildcard case the emulator has accepted a policy granting the world access.
Fix: policy-document semantic validation (principal form, principal existence, action namespace) in the resource-policy handler before the document is stored.
Traces: ddb-rbp-public-principal-policy-rejected, ddb-rbp-foreign-service-action-rejected, ddb-ltr-resource-policy-naming-a-nonexistent-principal-rejected.

=== Cause 7 of 8: an omitted policy Version is not defaulted ===
AWS: a policy document submitted without a Version element is stored with the IAM default Version "2008-10-17" filled in, and GetResourcePolicy returns it.
Emulator: the document is echoed back verbatim with no Version member.
Why: a caller reading the stored policy cannot tell which IAM policy grammar version applies, and a round-trip put/get diff shows a document that differs from AWS's.
Fix: the policy-document normaliser in the resource-policy write path.
Traces: ddb-rbp-omitted-policy-version-defaults-to-2008-10-17.

=== Cause 8 of 8: a deleted IAM user principal is never rewritten to its unique id, and the check never terminates ===
AWS: when a policy's Principal is an IAM user that is later deleted, AWS lazily rewrites the stored principal to that user's unique id on the next read, WITHOUT revoking the grant. Recorded behavior only.
Emulator: the rewrite never happens, so the check polls to the 60 s harness cap and reaches no verdict.
Why: an operator auditing a policy sees a stale principal ARN and cannot tell that the identity is gone.
Fix: principal resolution in the resource-policy read path/state.
Traces: ddb-rbp-deleted-principal-is-lazily-rewritten-to-its-unique-id-without-revoking-the-grant.

## Evidence
Cause 1 — ddb-rbp-stream-and-table-hold-independent-policies: botocore ClientError (ValidationException) when calling PutResourcePolicy: 'Invalid TableArn: Invalid ResourceArn provided as input arn:aws:dynamodb:us-west-2:<acct>:table/<t>/stream/<label>'. ddb-rbp-table-action-in-a-stream-policy-rejected: unexpected message 'Invalid TableArn: Invalid ResourceArn provided as input arn:aws:dynamodb:...:table/<t>/stream/<label>'.

Cause 2 — ddb-rbp-index-arn-as-policy-resource-rejected: answered ('ValidationException', 'Invalid TableArn: Invalid ResourceArn provided as input <table>/index/byG'), expected ('ValidationException', 'One or more parameter values were invalid: Invalid resource arn provided, only table or stream is accepted. Provided resource arn: <table>/index/byG'). ddb-rbp-another-regions-table-arn-rejected: answered 'Invalid TableArn: Invalid ResourceArn provided as input <us-west-2 arn>', expected 'One or more parameter values were invalid: Invalid resource Arn: <us-west-2 arn>'.

Cause 3 — ddb-rbp-never-created-table-arn-rejected: expected 'Requested resource not found: Table: <name> not found', got 'Requested resource not found: ResourceArn: arn:aws:dynamodb:us-west-2:<acct>:table/<name> not found'. ddb-rbp-deleted-table-drops-out-of-the-policy-derivation: same divergence.

Cause 4 — ddb-rbp-malformed-policy-json-rejected: answered 'Resource-based policy is not valid JSON: EOF while parsing a string at line 1 column 234', expected 'One or more parameter values were invalid: Invalid policy document: This policy contains invalid Json'.

Cause 5 — ddb-rbp-policy-over-20480-bytes-rejected: a 21381-byte policy answered 'LimitExceededException'; real AWS refuses anything over 20480 bytes with ValidationException. put-resource-policy message observed: 'Policy size exceeds the maximum supported size of 20480 bytes for resource <arn>: 500253 bytes'.

Cause 6 — ddb-rbp-public-principal-policy-rejected: the call succeeded ({'RevisionId': '...'}); real AWS rejects it. ddb-rbp-foreign-service-action-rejected: the call succeeded ({'RevisionId': '...'}); real AWS rejects it. ddb-ltr-resource-policy-naming-a-nonexistent-principal-rejected: the call succeeded; real AWS rejects it.

Cause 7 — ddb-rbp-omitted-policy-version-defaults-to-2008-10-17: a policy put without Version is stored as {'Statement': [...]} with no Version; real AWS fills in the IAM default Version 2008-10-17.

Cause 8 — ddb-rbp-deleted-principal-is-lazily-rewritten-to-its-unique-id-without-revoking-the-grant: Check timed out after 60.0s.

## Steps to Reproduce
1. Create a stream-enabled table; take LatestStreamArn.
2. ddb.put_resource_policy(ResourceArn=<streamArn>, Policy=<valid stream policy>) — expect success, observe ValidationException 'Invalid TableArn: ...'.
3. ddb.put_resource_policy(ResourceArn=<tableArn>+'/index/<idx>', Policy=...) and read Error.Message.
4. ddb.get_resource_policy(ResourceArn=<arn of a never-created table>) — expect 'Requested resource not found: Table: <name> not found'.
5. ddb.put_resource_policy(ResourceArn=<tableArn>, Policy='{"Version":"2012-10-17","Statement":[') — expect 'One or more parameter values were invalid: Invalid policy document: This policy contains invalid Json'.
6. ddb.put_resource_policy with a >20480-byte document and read Error.Code — expect ValidationException.
7. ddb.put_resource_policy with Principal {'AWS': '*'}, then with Action ['s3:GetObject'], then naming a nonexistent principal — each must be refused; observe RevisionId returned.
8. ddb.put_resource_policy with a document carrying no Version, then ddb.get_resource_policy and look for Version '2008-10-17'.
Then replay the dynamodb checks named in trace_ids.
