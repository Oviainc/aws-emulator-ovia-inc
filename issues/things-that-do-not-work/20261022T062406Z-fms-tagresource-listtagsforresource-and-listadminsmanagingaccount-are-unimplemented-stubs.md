Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: fms.tag-resource-nonexistent-policy-arn-not-found fms.list-tags-for-resource-malformed-arn-rejected fms.list-admins-managing-account-unconfigured-not-found
Issue-Shape-Id: com.amazonaws.fms#AWSFMS_20180101
Issue-Key: fms.tag-resource-nonexistent-policy-arn-not-found|run#6|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code}"
Issue-Key: fms.list-tags-for-resource-malformed-arn-rejected|run#5|AssertionError|assert malformed_code == "ValidationException", f"expected ValidationException, got {malformed_code}"
Issue-Key: fms.list-admins-managing-account-unconfigured-not-found|run#3|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code}"
Issue-Commit: 360c4c1618594432a64f84ba94d0fa368ed8f619

# FMS: TagResource, ListTagsForResource and ListAdminsManagingAccount are unimplemented stubs answering 501 NotImplemented instead of their modeled 400 errors

> _Note: this issue was identified against the code base at commit `360c4c1618594432a64f84ba94d0fa368ed8f619`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
Three further Firewall Manager handlers in services/service-com-amazonaws-fms-awsfms-20180101/src/ops/ are scaffold stubs whose handle() ends in Err(crate::scaffolded::Scaffolded::not_implemented("<Op>")) (HTTP 501, Error.Code `NotImplemented`). Unlike the administrator-gated cluster filed separately, these three are NOT admin-gated: the suite's checks reach them in any account and expect the operations' own modeled 400 errors. This issue bundles THREE root causes — three separate handlers to implement. Each must be closed before submitting.

Cause 1 of 3: TagResource for a nonexistent resource ARN.
AWS: 400 ResourceNotFoundException. Smithy: TagResource models ResourceNotFoundException (errorKind client, httpError 400) alongside InternalErrorException / InvalidInputException / InvalidOperationException / LimitExceededException; input ResourceArn is required (ResourceArn, len 1..1024, pattern ^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$) and TagList is required (len 0..200). A syntactically well-formed FMS policy ARN that names no existing policy is not-found, and the call must leave no tag state behind. Recorded message example: "The referenced item does not exist."
Emulator: 501 / NotImplemented (services/service-com-amazonaws-fms-awsfms-20180101/src/ops/tag_resource.rs:166).
Why: tagging code paths cannot be exercised at all, and a 5xx with a non-AWS code is retried by the SDK instead of surfacing the not-found decision; worse, a caller cannot tell a refused tag from an accepted one.
Fix: implement the handler in src/ops/tag_resource.rs — resolve the ARN against the policy/resource-set/apps-list/protocols-list state and return the modeled ResourceNotFoundException when nothing matches, persisting tags only on a hit; delete the SCAFFOLDED marker.
Traces: fms.tag-resource-nonexistent-policy-arn-not-found.

Cause 2 of 3: ListTagsForResource ARN handling.
AWS: two distinct 400 outcomes. A string that is not an ARN at all ("this-is-not-an-arn-at-all") is refused by the request-shape validator with ValidationException before any lookup — note the modeled ResourceArn pattern would accept that string, so this is a service-level ARN-format check, recorded behavior only (recorded message example: "Incorrect arn format. error:ARNs must start with 'arn:': this-is-not-an-arn-at-all"). A well-formed ARN naming no existing resource instead yields ResourceNotFoundException, which the model does declare for this operation (httpError 400), with recorded message example "The referenced item does not exist."
Emulator: 501 / NotImplemented for both.
Why: clients that distinguish a malformed-ARN programming error from a missing resource see neither; both come back as a retryable server error.
Fix: implement src/ops/list_tags_for_resource.rs — add the ARN-format validation (returning the 400 ValidationException channel, Scaffolded::validation) ahead of state lookup, then the modeled ResourceNotFoundException for a missing resource.
Traces: fms.list-tags-for-resource-malformed-arn-rejected (also exercised as the no-side-effect read of fms.tag-resource-nonexistent-policy-arn-not-found).

Cause 3 of 3: ListAdminsManagingAccount with no administrator managing the caller.
AWS: 400 ResourceNotFoundException. Smithy: ListAdminsManagingAccount models InternalErrorException, InvalidInputException and ResourceNotFoundException (all client, httpError 400); it is paginated (NextToken/MaxResults 1..100) and returns AdminAccounts. When no Firewall Manager administrator manages the calling account there is no resource to enumerate, so the call is not-found rather than an empty list. Recorded message example: "The referenced item does not exist."
Emulator: 501 / NotImplemented.
Why: a caller probing whether it is managed by an FM administrator gets a retryable 5xx instead of the modeled not-found, and cannot distinguish "unmanaged" from a server outage.
Fix: implement src/ops/list_admins_managing_account.rs against the service state's administrator association (the same store get_admin_account.rs reads), returning the modeled ResourceNotFoundException when no administrator manages the caller.
Traces: fms.list-admins-managing-account-unconfigured-not-found.

## Evidence
Replay of suite simparity-behavioral-aws-v1, service fms: 0/24 checks passed.

- fms.tag-resource-nonexistent-policy-arn-not-found: AssertionError: expected ResourceNotFoundException, got NotImplemented (call: fms.tag_resource(ResourceArn="arn:aws:fms:<region>:<account>:policy/<uuid4>", TagList=[{"Key":"k","Value":"v"}]))
- fms.list-tags-for-resource-malformed-arn-rejected: AssertionError: expected ValidationException, got NotImplemented (call: fms.list_tags_for_resource(ResourceArn="this-is-not-an-arn-at-all"))
- fms.list-admins-managing-account-unconfigured-not-found: AssertionError: expected ResourceNotFoundException, got NotImplemented (call: fms.list_admins_managing_account())

Expected: HTTP 400 with the operation's modeled error code (ResourceNotFoundException; ValidationException for the malformed ARN). Actual: HTTP 501, Error.Code "NotImplemented".

Source evidence: services/service-com-amazonaws-fms-awsfms-20180101/src/ops/tag_resource.rs:149-167 — the handler binds input/ctx/engine and returns Err(crate::scaffolded::Scaffolded::not_implemented("TagResource")); list_tags_for_resource.rs and list_admins_managing_account.rs use the same scaffold template. An implemented handler for comparison: src/ops/get_admin_account.rs:145-168.

Model evidence: smithy_query(query="operation", operation="TagResource,ListTagsForResource,ListAdminsManagingAccount", model_path="services/service-com-amazonaws-fms-awsfms-20180101/smithy-model/com-amazonaws-fms-awsfms-20180101.json") — TagResource errors: InternalErrorException, InvalidInputException, InvalidOperationException, LimitExceededException, ResourceNotFoundException (400); ListTagsForResource errors: InternalErrorException, InvalidInputException, InvalidOperationException, ResourceNotFoundException (400); ListAdminsManagingAccount errors: InternalErrorException, InvalidInputException, ResourceNotFoundException (400). None models a 501/NotImplemented response.

## Steps to Reproduce
1. Boot the emulator from this checkout and point a botocore fms client at it.
2. fms.tag_resource(ResourceArn=f"arn:aws:fms:{region}:{account}:policy/{uuid4()}", TagList=[{"Key":"k","Value":"v"}]) -> expect 400 ResourceNotFoundException; observe 501 NotImplemented.
3. fms.list_tags_for_resource(ResourceArn="this-is-not-an-arn-at-all") -> expect 400 ValidationException; then fms.list_tags_for_resource(ResourceArn=f"arn:aws:fms:{region}:{account}:policy/{uuid4()}") -> expect 400 ResourceNotFoundException; observe 501 NotImplemented for both.
4. fms.list_admins_managing_account() -> expect 400 ResourceNotFoundException; observe 501 NotImplemented.
5. Re-replay the three checks in trace_ids to confirm a fix.
