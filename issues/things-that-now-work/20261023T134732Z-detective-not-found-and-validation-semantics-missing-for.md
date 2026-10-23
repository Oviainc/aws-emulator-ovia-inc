Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: detective.get-investigation-nonexistent-id-not-found detective.get-investigation-malformed-id-pattern-rejected detective.reject-invitation-nonexistent-graph-not-found-rejected detective.start-monitoring-member-non-member-not-found-rejected detective.tag-resource-reserved-aws-prefix-rejected detective.create-members-invalid-account-id-pattern-rejected detective.create-members-malformed-account-id-validation-rejected detective.update-datasource-packages-invalid-enum-rejected detective.delete-graph-removes-from-list-graphs
Issue-Shape-Id: com.amazonaws.detective#AmazonDetective
Issue-Key: detective.get-investigation-nonexistent-id-not-found|run#5|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code!r}"
Issue-Key: detective.get-investigation-malformed-id-pattern-rejected|run#10|AssertionError|assert control_code == "ResourceNotFoundException", (
Issue-Key: detective.reject-invitation-nonexistent-graph-not-found-rejected|run#5|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code!r}"
Issue-Key: detective.start-monitoring-member-non-member-not-found-rejected|run#5|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code!r}"
Issue-Key: detective.tag-resource-reserved-aws-prefix-rejected|run#5|AssertionError|assert code == "BadRequestException", f"expected BadRequestException, got {code!r}"
Issue-Key: detective.create-members-invalid-account-id-pattern-rejected|run#8|AssertionError|assert message == "Account.AccountId did not match the pattern.", f"unexpected message: {message!r}"
Issue-Key: detective.create-members-malformed-account-id-validation-rejected|run#6|AssertionError|assert message == "Account.AccountId did not match the pattern.", f"unexpected message: {message!r}"
Issue-Key: detective.update-datasource-packages-invalid-enum-rejected|run#5|AssertionError|assert message == (
Issue-Key: detective|ListMembers|ResourceNotFoundException
Issue-Commit: a59989a9f63c403b66485a4e93f54ab6df22c2a6

# Detective: not-found and validation semantics missing for GetInvestigation, RejectInvitation, StartMonitoringMember, TagResource key rules, CreateMembers/UpdateDatasourcePackages validation and deleted-graph reads

> _Note: this issue was identified against the code base at commit `a59989a9f63c403b66485a4e93f54ab6df22c2a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetInvestigation

## Description
Seven independent root causes in the emulator's Detective service (services/service-com-amazonaws-detective-amazondetective), each grounded in failing behavioral checks. A fixer must close ALL SEVEN before submitting.

=== Cause 1 of 7: GetInvestigation returns NotImplemented instead of ResourceNotFoundException for a well-formed unknown InvestigationId ===
AWS: Smithy: GetInvestigation (POST /investigations/getInvestigation) takes GraphArn and InvestigationId (@length 21/21, @pattern ^[0-9]+$) and models ResourceNotFoundException with httpError 404. A 21-digit id that was never created must be answered with ResourceNotFoundException (404); recorded message, as an example: "The specified investigation does not exist". Note the framework-level pattern rejection for a malformed id already matches AWS (ValidationException 400, "1 validation error detected: Value ... at 'investigationId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9]+$") — only the handler path diverges.
Emulator: the request reaches a scaffold stub and comes back as error code 'NotImplemented' ("operation GetInvestigation is scaffolded but not implemented in this emulator").
Why: a client cannot distinguish "no such investigation" from a broken service; NotImplemented is not a modeled error for this operation, so SDK error handling and not-found branches never fire.
Fix: implement the GetInvestigation handler (ops/get_investigation.rs) over the investigation records in state.rs, returning ResourceNotFoundException (404) for an unknown id after the modeled input validation.
Traces: detective.get-investigation-nonexistent-id-not-found, detective.get-investigation-malformed-id-pattern-rejected.

=== Cause 2 of 7: RejectInvitation returns NotImplemented instead of ResourceNotFoundException for a never-created graph ===
AWS: Smithy: RejectInvitation (POST /invitation/removal) takes a @required GraphArn and models ResourceNotFoundException with httpError 404 (plus ConflictException 409, ValidationException 400). A syntactically valid GraphArn that was never created must be rejected with ResourceNotFoundException (404).
Emulator: error code 'NotImplemented' from the scaffold stub.
Why: an invitee's reject path is unusable and an unmodeled error code reaches the client where a 404 is contracted.
Fix: implement ops/reject_invitation.rs, resolving the graph in state.rs and returning ResourceNotFoundException (404) when absent.
Traces: detective.reject-invitation-nonexistent-graph-not-found-rejected.

=== Cause 3 of 7: StartMonitoringMember returns NotImplemented instead of ResourceNotFoundException for a non-member account ===
AWS: Smithy: StartMonitoringMember (POST /graph/member/monitoringstate) takes @required GraphArn and AccountId (@length 12/12, @pattern ^[0-9]+$) and models ResourceNotFoundException 404, ConflictException 409, ValidationException 400. An account that is not a member of the graph must be rejected with ResourceNotFoundException (404).
Emulator: error code 'NotImplemented' from the scaffold stub.
Why: callers that enable monitoring for a member cannot tell "not a member" from an outage, and the modeled 404 never appears.
Fix: implement ops/start_monitoring_member.rs against the graph's member records in state.rs.
Traces: detective.start-monitoring-member-non-member-not-found-rejected.

=== Cause 4 of 7: TagResource does not enforce the reserved 'aws:' tag-key rule (currently NotImplemented) ===
AWS: recorded behavior only for the status/code pairing (the model's TagResource declares ValidationException 400 but not this case). Recording: a tag key beginning with the reserved prefix 'aws:' is refused with error code BadRequestException at HTTP 400, raised ahead of the service's normal ValidationException path, and the key never becomes visible via ListTagsForResource; an ordinary key on the same resource still tags successfully. Recorded message, as an example: "Invalid tag key. Customers cannot set tag keys starting with aws: (case insensitive)".
Emulator: the call returns error code 'NotImplemented', so the reserved-prefix rule is not enforced at all.
Why: a client that relies on AWS refusing reserved keys gets an unmodeled error for both reserved and ordinary keys, and once the stub is filled in without this rule a reserved key could be silently accepted — an overly permissive target.
Fix: TagResource handler/validation (ops/tag_resource.rs): reject tag keys matching the reserved 'aws:' prefix case-insensitively with BadRequestException (HTTP 400) before applying any tag. NOTE: an issue for the bare "TagResource is scaffolded but not implemented" stub is already filed by the replay report (naming ops/tag_resource.rs); this cause records the additional reserved-key rejection rule, which that stub issue does not state, and must be satisfied by the implementation.
Traces: detective.tag-resource-reserved-aws-prefix-rejected.

=== Cause 5 of 7: CreateMembers AccountId pattern rejection uses the generic framework message instead of Detective's own ===
AWS: Smithy: CreateMembers Accounts -> AccountList of Account{AccountId @length 12/12 @pattern ^[0-9]+$}; ValidationException httpError 400. The recordings establish the rule, not just one string: two different 12-character non-numeric AccountIds ('1234567890AB' and '12345678901A') both produce the SAME fixed, input-independent message "Account.AccountId did not match the pattern." — i.e. Detective validates this member itself rather than via the generic constraint reporter. (Contrast GetInvestigation's investigationId, where the generic "1 validation error detected: ..." form IS what AWS returns and the emulator matches it.)
Emulator: code ValidationException (400) is correct, but the message is the generic constraint report, e.g. "1 validation error detected: Value '1234567890AB' at 'accounts.1.member.accountId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9]+$".
Why: clients and test suites that assert on Detective's documented per-field message (or parse it to identify the offending field in its API-level spelling, Account.AccountId) mis-handle the rejection; the divergence also reveals the AccountId check is happening in the generic protocol layer rather than CreateMembers' own validation, which is where the per-account partitioning between Members and UnprocessedAccounts lives.
Fix: CreateMembers input validation (ops/create_members.rs) must perform the AccountId pattern check itself and emit ValidationException (400) with the fixed Detective message form.
Traces: detective.create-members-invalid-account-id-pattern-rejected, detective.create-members-malformed-account-id-validation-rejected.

=== Cause 6 of 7: UpdateDatasourcePackages reports an unknown package with the generic enum message instead of Detective's unknown-package message ===
AWS: Smithy: UpdateDatasourcePackages DatasourcePackages -> DatasourcePackageList of the DatasourcePackage enum (DETECTIVE_CORE, EKS_AUDIT, ASFF_SECURITYHUB_FINDING), ValidationException httpError 400. Recording shows the message is built from the request by the service, listing the offending values: "UpdateDatasourcePackages request includes unknown datasource package(s): [NOT_A_REAL_DATASOURCE_PACKAGE]" (the companion rule, also service-level, is "missing mandatory datasource package(s): [DETECTIVE_CORE]" when DETECTIVE_CORE is omitted).
Emulator: code ValidationException (400) is correct, but the message is the generic enum constraint report: "1 validation error detected: Value 'NOT_A_REAL_DATASOURCE_PACKAGE' at 'datasourcePackages.1.member' failed to satisfy constraint: Member must satisfy enum value set: [DETECTIVE_CORE, EKS_AUDIT, ASFF_SECURITYHUB_FINDING]".
Why: the message is the only place AWS names which packages were unknown (or which mandatory one was missing), so a client that surfaces or parses it loses that information; it also shows package-set validation is not being done by the operation, which is the same place the mandatory-DETECTIVE_CORE rule must live.
Fix: UpdateDatasourcePackages validation (ops/update_datasource_packages.rs) must validate the requested package set itself and emit the unknown/missing-package ValidationException (400) forms. Related but distinct: the "UpdateDatasourcePackages is scaffolded but not implemented" stub is already filed by the replay report; this cause is about the validation message form the implementation owes.
Traces: detective.update-datasource-packages-invalid-enum-rejected.

=== Cause 7 of 7: reads scoped to a deleted graph report ResourceNotFoundException with a leaked-ARN message instead of Detective's fixed message ===
AWS: recorded behavior only. After DeleteGraph, ListMembers scoped to that graph is refused with ResourceNotFoundException (404) and the fixed, input-independent message "Graph not found" — the message does not echo the graph ARN.
Emulator: the code ResourceNotFoundException (404) is correct, but the message is constructed from the request, e.g. "behavior graph arn:aws:detective:<region>:<account>:graph:<id> does not exist".
Why: minor — a client asserting on the documented message mis-handles the not-found, and the emulator echoes a caller-supplied ARN into error text where AWS does not. Lowest-severity cause in this bundle; the status and code already match.
Fix: the not-found error construction for graph-scoped reads (ops/list_members.rs and the shared graph-resolution helper in state.rs) should emit the fixed message form.
Traces: detective.delete-graph-removes-from-list-graphs.

## Evidence
Cause 1 — detective.get-investigation-nonexistent-id-not-found: "expected ResourceNotFoundException, got 'NotImplemented'". detective.get-investigation-malformed-id-pattern-rejected: malformed-id assertion passed; "expected ResourceNotFoundException for the well-formed control id, got 'NotImplemented'".
Cause 2 — detective.reject-invitation-nonexistent-graph-not-found-rejected: "expected ResourceNotFoundException, got 'NotImplemented'".
Cause 3 — detective.start-monitoring-member-non-member-not-found-rejected: "expected ResourceNotFoundException, got 'NotImplemented'".
Cause 4 — detective.tag-resource-reserved-aws-prefix-rejected: "expected BadRequestException, got 'NotImplemented'" for Tags={"aws:sim2real-reserved": "v"}; expected HTTP 400 / BadRequestException and the key absent from ListTagsForResource.
Cause 5 — detective.create-members-invalid-account-id-pattern-rejected: expected message "Account.AccountId did not match the pattern.", got "1 validation error detected: Value '1234567890AB' at 'accounts.1.member.accountId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9]+$". detective.create-members-malformed-account-id-validation-rejected: same expected message for input '12345678901A', got the same generic form — two different inputs, one constant AWS message, which is the rule.
Cause 6 — detective.update-datasource-packages-invalid-enum-rejected: expected "UpdateDatasourcePackages request includes unknown datasource package(s): [NOT_A_REAL_DATASOURCE_PACKAGE]", got "1 validation error detected: Value 'NOT_A_REAL_DATASOURCE_PACKAGE' at 'datasourcePackages.1.member' failed to satisfy constraint: Member must satisfy enum value set: [DETECTIVE_CORE, EKS_AUDIT, ASFF_SECURITYHUB_FINDING]".
Cause 7 — detective.delete-graph-removes-from-list-graphs: code ResourceNotFoundException correct; "ListMembers on deleted graph: unexpected message 'behavior graph arn:aws:detective:us-west-2:000000000000:graph:76a599b25372df4210df7c4ef7f79819 does not exist'" vs expected "Graph not found".

## Steps to Reproduce
1. Cause 1: arn = detective.create_graph()['GraphArn']; detective.get_investigation(GraphArn=arn, InvestigationId='1'*21) -> expect ResourceNotFoundException (404), observe NotImplemented.
2. Cause 2: detective.reject_invitation(GraphArn='arn:aws:detective:<region>:<acct>:graph:<32 hex never created>') -> expect ResourceNotFoundException (404), observe NotImplemented.
3. Cause 3: detective.start_monitoring_member(GraphArn=arn, AccountId='9'+11 digits of a non-member) -> expect ResourceNotFoundException (404), observe NotImplemented.
4. Cause 4: detective.tag_resource(ResourceArn=arn, Tags={'aws:reserved':'v'}) -> expect BadRequestException at HTTP 400 and the key absent from list_tags_for_resource; observe NotImplemented.
5. Cause 5: detective.create_members(GraphArn=arn, Accounts=[{'AccountId':'1234567890AB','EmailAddress':'x@example.com'}], DisableEmailNotification=True) -> ValidationException, compare message to the fixed Detective form.
6. Cause 6: detective.update_datasource_packages(GraphArn=arn, DatasourcePackages=['NOT_A_REAL_DATASOURCE_PACKAGE']) -> ValidationException, compare message to the unknown-package form.
7. Cause 7: arn=create_graph(); delete_graph(GraphArn=arn); detective.list_members(GraphArn=arn) -> ResourceNotFoundException, compare message to "Graph not found".
