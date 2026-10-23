Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: mpa.get-approval-team-not-found-unknown-arn mpa.start-active-approval-team-deletion-when-not-active-rejected mpa.delete-inactive-approval-team-version-nonexistent-is-noop mpa.create-approval-team-idempotent-client-token mpa.get-resource-policy-invalid-policy-name-rejected mpa.tag-resource-exceeds-limit-rejected mpa.tag-resource-round-trip-on-approval-team mpa.get-policy-version-malformed-arn-rejected
Issue-Shape-Id: com.amazonaws.mpa#AWSFluffyCoreService
Issue-Key: mpa.get-approval-team-not-found-unknown-arn|run#6|AssertionError|assert err == ("ResourceNotFoundException", NOT_FOUND), (
Issue-Key: mpa.start-active-approval-team-deletion-when-not-active-rejected|run#6|AssertionError|assert err == ("ResourceNotFoundException", NOT_FOUND), (
Issue-Key: mpa.delete-inactive-approval-team-version-nonexistent-is-noop|run#7|AssertionError|assert get_err == ("ResourceNotFoundException", NOT_FOUND), get_err
Issue-Key: mpa.create-approval-team-idempotent-client-token|run#7|AssertionError|assert err == ("ResourceNotFoundException", NOT_FOUND), err
Issue-Key: mpa.get-resource-policy-invalid-policy-name-rejected|run#6|AssertionError|assert err == ("ResourceNotFoundException", NOT_FOUND), err
Issue-Key: mpa.tag-resource-exceeds-limit-rejected|run#8|AssertionError|assert err == not_found, err
Issue-Key: mpa.tag-resource-round-trip-on-approval-team|run#7|AssertionError|assert tag_err == not_found, tag_err
Issue-Key: mpa.get-policy-version-malformed-arn-rejected|run#3|AssertionError|assert err == ("ValidationException", "Invalid Resource Arn"), err
Issue-Commit: d46c9ea0387b1ea18140666d9f1aeb4877a1d699

# mpa: error-message fidelity across the not-found and malformed-ARN paths (4 causes) — RNFE prose instead of AWS's fixed message, tag-family message not the ResourceArn, GetResourcePolicy resolves PolicyName before ResourceArn, GetPolicyVersion emits the generic constraint report instead of "Invalid Resource Arn"

> _Note: this issue was identified against the code base at commit `d46c9ea0387b1ea18140666d9f1aeb4877a1d699`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetApprovalTeam

## Description
Four net-new root causes in the mpa (AWSFluffyCoreService) emulator, all in how rejections are produced. In every one of them the HTTP status and the error code already match real AWS; what diverges is the message produced by a KNOWN rule (a service-wide constant, or a message built from the request's ResourceArn), and in Cause 2 the order in which two preconditions are evaluated. None of these failures is a scaffold stub: each operation answered.

Related already-open issues (NOT duplicates of anything below, named here for the fixer): 20261023T061844Z-mpa-listtagsforresource-and-listresourcepolicies-answer-501-notimplemented-instead.md (ListTagsForResource/ListResourcePolicies answer 501 on a malformed ResourceArn), 20261023T100857Z-mpa-createapprovalteam-returns-400-validationexception-instead-of-the.md (CreateApprovalTeam ordering), and the GetPolicyVersion / GetIdentitySource / ListIdentitySources stub issues. Note that several of the checks below ALSO use GetPolicyVersion as a trailing positive control, so they will not go green until that stub issue is fixed too; the causes filed here are the ones those checks fail on FIRST.

=== Cause 1 of 4: ResourceNotFoundException carries emulator-invented prose instead of MPA's fixed not-found message ===
AWS: Smithy (com.amazonaws.mpa): ResourceNotFoundException is a client error, httpError 404, doc "The specified resource doesn't exist. Check the resource ID, and try again.", with a single required Message member (String, @length max 1000). The model does not fix the message text, so: recorded behavior only — the recordings show MPA emitting ONE constant message for the resource-lookup failures of all of its ARN-addressed read/delete operations, independent of the operation and of the ARN supplied: 404 ResourceNotFoundException with Message exactly "The requested resource does not exist." Four independent recorded operations (GetApprovalTeam, StartActiveApprovalTeamDeletion, ListPolicyVersions, GetResourcePolicy) agree on that one string, so the rule is a service-wide constant and not per-operation prose.
Emulator: returns the correct code and status but synthesises a per-operation sentence that embeds the request ARN — e.g. GetApprovalTeam and StartActiveApprovalTeamDeletion answer "No approval team found for ARN 'arn:aws:mpa:us-east-1:<account>:approval-team/<name>'.", and ListPolicyVersions answers "No policy found for ARN 'arn:aws:mpa:::aws:policy/IdentityCenter/<name>'."
Why: a client that branches on, logs or asserts the not-found message — and anything that treats MPA's not-found text as the stable, resource-opaque constant AWS returns — sees a different string per operation, and sees the requested ARN leaked into a message AWS keeps resource-free. Error-message assertions and log-based alarms written against AWS fail against the emulator.
Fix: the shared not-found rejection path for the mpa ARN-addressed operations (the ResourceNotFoundException construction used by src/ops/get_approval_team.rs, src/ops/start_active_approval_team_deletion.rs and src/ops/list_policy_versions.rs in services/service-com-amazonaws-mpa-awsfluffycoreservice) should emit the one service constant rather than formatting the request ARN into the message.
Traces: mpa.get-approval-team-not-found-unknown-arn, mpa.start-active-approval-team-deletion-when-not-active-rejected, mpa.delete-inactive-approval-team-version-nonexistent-is-noop (its DeleteInactiveApprovalTeamVersion no-op already matches; it fails on the contrasting GetApprovalTeam lookup), mpa.create-approval-team-idempotent-client-token (ListPolicyVersions against a well-formed but nonexistent PolicyArn).

=== Cause 2 of 4: GetResourcePolicy evaluates PolicyName/PolicyType before it resolves ResourceArn existence ===
AWS: Smithy: GetResourcePolicy POST /GetResourcePolicy, input ResourceArn/PolicyName/PolicyType all required, errors include ResourceNotFoundException (404). Recorded behavior: against a syntactically valid but nonexistent approval-team ResourceArn, the call fails on the resource lookup FIRST and returns 404 ResourceNotFoundException with the service's constant message "The requested resource does not exist." — the outcome does not depend on the PolicyName supplied, because no policy-name matching is ever reached.
Emulator: returns 404 ResourceNotFoundException, but with "No AWS_MANAGED policy named '<policy-name>' is attached to resource '<resource-arn>'." — i.e. it reports a policy-not-attached-to-resource failure, which shows the handler ran policy-name/policy-type matching against a resource it never established exists, and then described the lookup in those terms.
Why: a client cannot distinguish "this team does not exist" from "this team exists but has no such policy attached" the way AWS lets it, and the message varies with a request field AWS does not consult on this path, so retry/branch logic keyed on the not-found reason diverges.
Fix: GetResourcePolicy's handler (services/service-com-amazonaws-mpa-awsfluffycoreservice/src/ops/get_resource_policy.rs) must resolve ResourceArn against the resource store before evaluating PolicyName/PolicyType, and raise the shared constant-message ResourceNotFoundException from that resource lookup.
Traces: mpa.get-resource-policy-invalid-policy-name-rejected.

=== Cause 3 of 4: the tag-family ResourceNotFoundException message must be the ResourceArn itself ===
AWS: Smithy: TagResource (PUT /tags/{ResourceArn}) and UntagResource take a required ResourceArn (String, @length max 1000, no pattern) and model both ResourceNotFoundException (404) and TooManyTagsException (400). Recorded behavior: against a syntactically valid but nonexistent approval-team ARN, TagResource, UntagResource and ListTagsForResource each independently return 404 ResourceNotFoundException whose Message is exactly the ResourceArn that was sent — the message is BUILT FROM THE REQUEST (message == the ResourceArn member, verbatim, with no surrounding prose), which is why this is a rule and not a recorded sample. The recordings also show the resource-existence check running BEFORE the tag-count quota: 51 tags against the nonexistent ARN still yields ResourceNotFoundException, never TooManyTagsException (the emulator agrees on that ordering).
Emulator: wraps it — "No resource found for ARN 'arn:aws:mpa:us-east-1:<account>:approval-team/<name>'." — so the message is prose containing the ARN rather than the bare ARN.
Why: callers that read the tag-family not-found message as the offending resource identifier (the AWS contract here) get an unparseable sentence instead of an ARN.
Fix: the tag-family rejection path (services/service-com-amazonaws-mpa-awsfluffycoreservice/src/ops/tag_resource.rs, src/ops/untag_resource.rs and the ListTagsForResource handler) must set ResourceNotFoundException.Message to the request's ResourceArn verbatim.
Traces: mpa.tag-resource-exceeds-limit-rejected, mpa.tag-resource-round-trip-on-approval-team.

=== Cause 4 of 4: malformed-ARN ValidationException is the generic Smithy constraint report instead of MPA's "Invalid Resource Arn" ===
AWS: Smithy: GetPolicyVersion GET /policy-versions/{PolicyVersionArn}; PolicyVersionArn is @required with @length max 1224 and @pattern ^arn:.{1,63}:mpa:::aws:policy/[a-zA-Z0-9_\\.-]{1,1023}/[a-zA-Z0-9_\\.-]{1,1023}/(?:[\\d]+|\\$DEFAULT)$; ValidationException is a client error, httpError 400. The model does not fix the message, so: recorded behavior only — MPA rejects an ARN that is not a well-formed MPA ARN with 400 ValidationException and the fixed message "Invalid Resource Arn", i.e. a service-level constant for the malformed-ARN class rather than a per-member constraint report. The same constant is recorded for the malformed-ResourceArn rejection of ListTagsForResource, ListResourcePolicies and GetResourcePolicy, so the rule is service-wide and not specific to GetPolicyVersion.
Emulator: returns 400 ValidationException (correct status and code) carrying the framework's generic constraint report — "1 validation error detected: Value 'arn:aws:iam::123456789012:policy/foo' at 'policyVersionArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:.{1,63}:mpa:::aws:policy/...$" — which echoes the member name and the raw regex AWS never exposes here.
Why: the emulator leaks the internal member name and the model regex to clients, and any handling keyed on MPA's malformed-ARN message misses. It also means the malformed-ARN class is being answered by a generic model-constraint validator rather than by MPA's own ARN validation, which is the same mis-layering that makes other MPA operations validate shape where AWS applies a service rule first.
Fix: mpa's request-validation layer must map an ARN-shaped member whose @pattern fails onto the service's own ValidationException with the constant malformed-ARN message, rather than rendering the generic constraint report (GetPolicyVersion's validation in services/service-com-amazonaws-mpa-awsfluffycoreservice/src/ops/get_policy_version.rs plus the shared validator). Note this operation is also an unimplemented stub past validation (see 20261023T100857Z-mpa-getpolicyversion-stub.md) — this cause is about the message of the rejection it DOES produce.
Traces: mpa.get-policy-version-malformed-arn-rejected.

## Evidence
All evidence is from the simparity-behavioral-aws-v1 replay against the booted emulator (mpa 3/19 passed). Status/code matched AWS in every case below; the quoted divergences are the message (and, in Cause 2, the precondition order the message reveals).

Cause 1:
- mpa.get-approval-team-not-found-unknown-arn — expected ('ResourceNotFoundException', 'The requested resource does not exist.'); got ('ResourceNotFoundException', "No approval team found for ARN 'arn:aws:mpa:us-east-1:000000000000:approval-team/sim2real-nonexistent-133020e04e7048f287eac940bffc9a5f'.")
- mpa.start-active-approval-team-deletion-when-not-active-rejected — same expectation; got "No approval team found for ARN 'arn:aws:mpa:us-east-1:000000000000:approval-team/sim2real-nonexistent-55087658e48e4f36bb782675b8ae56bb'."
- mpa.delete-inactive-approval-team-version-nonexistent-is-noop — the DeleteInactiveApprovalTeamVersion no-op succeeded as AWS does; the check then failed on the contrasting GetApprovalTeam lookup with "No approval team found for ARN 'arn:aws:mpa:us-east-1:000000000000:approval-team/sim2real-nonexistent-7aefcd34050d43bd87bb131e3d10f6c7'."
- mpa.create-approval-team-idempotent-client-token — ListPolicyVersions on a well-formed nonexistent PolicyArn: expected ('ResourceNotFoundException', 'The requested resource does not exist.'); got ('ResourceNotFoundException', "No policy found for ARN 'arn:aws:mpa:::aws:policy/IdentityCenter/sim2real-nonexistent-policy'.")

Cause 2:
- mpa.get-resource-policy-invalid-policy-name-rejected — GetResourcePolicy(ResourceArn=<nonexistent approval-team ARN>, PolicyType='AWS_MANAGED', PolicyName='sim2real-nonexistent-policy-name'): expected ('ResourceNotFoundException', 'The requested resource does not exist.'); got ('ResourceNotFoundException', "No AWS_MANAGED policy named 'sim2real-nonexistent-policy-name' is attached to resource 'arn:aws:mpa:us-east-1:000000000000:approval-team/sim2real-nonexistent-d3fe0c648526476c846c2033aa7890f8'.") — the message names the PolicyName/PolicyType, so matching ran before the resource lookup AWS fails on.

Cause 3:
- mpa.tag-resource-exceeds-limit-rejected — TagResource with 51 tags on a nonexistent team ARN: expected ('ResourceNotFoundException', '<the ResourceArn verbatim>'); got ('ResourceNotFoundException', "No resource found for ARN 'arn:aws:mpa:us-east-1:000000000000:approval-team/sim2real-nonexistent-90819a374aa04092a92f0c5988578e9f'.") (correctly NOT TooManyTagsException).
- mpa.tag-resource-round-trip-on-approval-team — same divergence on TagResource (and the check would hit it identically on ListTagsForResource/UntagResource): got "No resource found for ARN 'arn:aws:mpa:us-east-1:000000000000:approval-team/sim2real-nonexistent-3acfe12df3444e8f9e7d327859d42356'."

Cause 4:
- mpa.get-policy-version-malformed-arn-rejected — GetPolicyVersion(PolicyVersionArn='arn:aws:iam::123456789012:policy/foo'): expected ('ValidationException', 'Invalid Resource Arn'); got ('ValidationException', "1 validation error detected: Value 'arn:aws:iam::123456789012:policy/foo' at 'policyVersionArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:.{1,63}:mpa:::aws:policy/[a-zA-Z0-9_\\.-]{1,1023}/[a-zA-Z0-9_\\.-]{1,1023}/(?:[\\d]+|\\$DEFAULT)$")

Model evidence (services/service-com-amazonaws-mpa-awsfluffycoreservice/smithy-model/com-amazonaws-mpa-awsfluffycoreservice.json): ResourceNotFoundException {client, httpError 404, required Message @length max 1000}; ValidationException {client, httpError 400, required Message}; TooManyTagsException {client, httpError 400}; TagResource PUT /tags/{ResourceArn} with ResourceArn @length max 1000 (no pattern); GetApprovalTeam/StartActiveApprovalTeamDeletion Arn @pattern ^arn:aws(-[^:]+)?:mpa:[a-z0-9-]{1,20}:[0-9]{12}:approval-team/[a-zA-Z0-9._-]+$; GetPolicyVersion PolicyVersionArn @pattern as quoted above.

## Steps to Reproduce
Boot the emulator and point a botocore mpa client at it in us-east-1 (MPA is single-Region), then:

Cause 1:
1. mpa.get_approval_team(Arn='arn:aws:mpa:us-east-1:<account>:approval-team/does-not-exist') — expect 404 ResourceNotFoundException with Message exactly 'The requested resource does not exist.'; observe the per-operation prose embedding the ARN.
2. mpa.start_active_approval_team_deletion(Arn=<the same nonexistent team ARN>) — same expectation, same divergence.
3. mpa.list_policy_versions(PolicyArn='arn:aws:mpa:::aws:policy/IdentityCenter/does-not-exist') — same expectation; observe "No policy found for ARN '...'".
4. mpa.delete_inactive_approval_team_version(Arn=<the same nonexistent team ARN>, VersionId='1') — this correctly no-ops; the divergence is on the GetApprovalTeam in step 1.

Cause 2:
5. mpa.get_resource_policy(ResourceArn=<nonexistent approval-team ARN>, PolicyType='AWS_MANAGED', PolicyName='anything') — expect the resource lookup to fail first: 404 ResourceNotFoundException, Message 'The requested resource does not exist.', independent of PolicyName. Observe a message naming the PolicyName/PolicyType instead. Repeat with a different PolicyName: on AWS the response is byte-identical, on the emulator the message changes.

Cause 3:
6. mpa.tag_resource(ResourceArn=<nonexistent approval-team ARN>, Tags={f'k{i}': 'v' for i in range(51)}) — expect 404 ResourceNotFoundException whose Message is exactly that ResourceArn (NOT TooManyTagsException); observe "No resource found for ARN '...'".
7. Repeat with a single in-quota tag, and with mpa.untag_resource(ResourceArn=<same>, TagKeys=['k0']) and mpa.list_tags_for_resource(ResourceArn=<same>) — AWS returns the bare ResourceArn as the message from all three.

Cause 4:
8. mpa.get_policy_version(PolicyVersionArn='arn:aws:iam::123456789012:policy/foo') — expect 400 ValidationException with Message 'Invalid Resource Arn'; observe the generic '1 validation error detected: Value ... at \'policyVersionArn\' failed to satisfy constraint: Member must satisfy regular expression pattern: ...' report.

Replay the whole set with: mpa.get-approval-team-not-found-unknown-arn, mpa.start-active-approval-team-deletion-when-not-active-rejected, mpa.delete-inactive-approval-team-version-nonexistent-is-noop, mpa.create-approval-team-idempotent-client-token, mpa.get-resource-policy-invalid-policy-name-rejected, mpa.tag-resource-exceeds-limit-rejected, mpa.tag-resource-round-trip-on-approval-team, mpa.get-policy-version-malformed-arn-rejected. (Several of these also end with a GetPolicyVersion positive control, which is blocked by the separately filed GetPolicyVersion stub issue.)
