Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam.sep-policy-composition-request-and-resource-tag-conditions-and-together-denied-when-either-fails
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam.sep-policy-composition-request-and-resource-tag-conditions-and-together-denied-when-either-fails|_settle#20|Timeout|time.sleep(interval_s)
Issue-Commit: 2e55e3f174321d59189949bdc160ce3c5e1a8b8d

# iam: request-time authorization supplies no condition context, so an Allow guarded by aws:ResourceTag AND aws:RequestTag never grants the signed TagUser call

> _Note: this issue was identified against the code base at commit `2e55e3f174321d59189949bdc160ce3c5e1a8b8d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagUser

## Description
One net-new root cause (Cause 1 of 1) in the bootstrapped `iam` service, grounded in a failing behavioral check the replay report marks neither ALREADY FILED nor OWNED.

Cause 1 of 1: the IAM request authorizer builds its request with an EMPTY condition context, so a statement whose Condition reads aws:ResourceTag/<key> (from the target resource) and aws:RequestTag/<key> (from the request body) is permanently undecidable and can never permit a real signed call.

AWS: IAM evaluates a statement's Condition against the request context, which AWS itself populates — aws:ResourceTag/<key> from the tags stored on the TARGET resource, aws:RequestTag/<key> from the tag the request body writes, aws:TagKeys from the set of keys the request writes, aws:PrincipalTag/<key> from the signing principal. Several keys in one Condition block AND together. So an Allow on `iam:TagUser` over [<user X arn>, <user Y arn>] with Condition {StringEquals: {"aws:ResourceTag/owner": "<X>", "aws:RequestTag/env": "dev"}, ForAllValues:StringEquals: {"aws:TagKeys": ["env"]}} permits TagUser(env=dev) on the owner=<X> user and reads back; the SAME request shape writing env=prod is refused `AccessDenied` (HTTP 403) and leaves the stored tag unchanged; env=dev against the owner=<Y> user is refused `AccessDenied` and writes nothing. Retagging the second target to owner=<X> flips its outcome to permitted. TagUser models ConcurrentModification 409 / InvalidInput 400 / LimitExceeded 409 / NoSuchEntity 404 / ServiceFailure 500 (Smithy com.amazonaws.iam#TagUser); `AccessDenied` 403 is the authorization refusal the service answers for every operation, so the per-key sourcing and the message are recorded behavior rather than modeled.

Emulator: no conditioned Allow can ever grant. services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/state/authorization.rs::authorize builds `let request = iam::Request::new(action, resource, &principals);` — the constructor documented as a request with no condition context (`context: &[]`) — and never calls `Request::with_context`. services/common/src/iam_authorization.rs then applies its fail-closed reading: a clause over a key the context does not carry leaves the statement UNDECIDED, and "an `Allow` whose condition may not hold is not a grant". So the signed TagUser is refused `AccessDenied` on the matching combination too, forever. The check's bounded poll for a successful TagUser (`_settle`, streak of 3, 150 s ceiling) therefore cannot converge and the harness ends the check at its 60 s cap. Hypothesis: the simulator leg may be implicated as well — the check first polls SimulatePrincipalPolicy with ContextEntries carrying all three keys and expects allowed / implicitDeny / implicitDeny; src/ops/simulate_principal_policy.rs DOES build a context from ContextEntries (`supplied_context`) and the shared evaluator does decide the set qualifiers, so the simulator leg is expected to pass and the certain defect is the authorizer's empty context.

Why: every ABAC grant a client writes is silently inert. The matching call is refused where AWS permits it, so a tag-based least-privilege design cannot be validated at all — and a client that works around the refusal by deleting the Condition ships something strictly MORE permissive than the policy it meant to test. Because the refusal is identical for the matching and the non-matching combinations, the emulator also cannot show that a refused write leaves the target's tags untouched, which is the integrity half of the contract.

Fix: the IAM request authorizer — services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/state/authorization.rs — must assemble the request's condition context and pass it through `iam::Request::with_context`: at minimum aws:ResourceTag/* read from the target entity in src/state.rs, aws:RequestTag/* and aws:TagKeys from the calling operation's tag input, aws:PrincipalTag/* from the resolved signing principal's stored tags, and aws:SourceIp from the connection. The per-operation call sites that invoke `authorize` must hand it the request's tag input so aws:RequestTag/aws:TagKeys can be populated for the Tag*/Untag* family.

Traces: iam.sep-policy-composition-request-and-resource-tag-conditions-and-together-denied-when-either-fails. The same authorizer gap is already described as Cause 3 of the open issue 20261025T102129Z-iam-10-net-new-behavioral-root-causes-simulateprincipalpolicy.md, whose replay set names only iam.sep-policy-composition-principal-tag-condition-simulated-with-real-tags-denied-on-mismatch (an aws:PrincipalTag case); this check is a different recorded unit — it is the only one that exercises aws:ResourceTag and aws:RequestTag AND-composed in one Condition block, and the requirement that a refused write leave no tag — and the replay report marked it neither ALREADY FILED nor OWNED, so it is recorded here. Note also that this check later calls iam ListUserTags, whose 501 scaffold stub is separately filed; that call is reached only after the poll above converges, so it is not what fails here.

## Evidence
Replay output:
  [mismatch] iam.sep-policy-composition-request-and-resource-tag-conditions-and-together-denied-when-either-fails
  Error: Check timed out after 60.0s

The check has only polling waits, and each raises on exhaustion rather than returning a last value: `_settle(probe, want, streak=3, timeout_s=150, interval_s=2)` for the real signed calls and `_sim_settle` (streak 1) for the simulator. Its sequence is: CreateUser target_x Tags=[{owner: <X>}], CreateUser target_y Tags=[{owner: <Y>}], CreateUser caller, PutUserPolicy on the caller with `Allow iam:GetUser on <caller arn>` plus `Allow iam:TagUser on [<x arn>, <y arn>] Condition {StringEquals: {"aws:ResourceTag/owner": "<X>", "aws:RequestTag/env": "dev"}, ForAllValues:StringEquals: {"aws:TagKeys": ["env"]}}`, CreateAccessKey, then `_settle` on the unconditioned GetUser (positive control) and `_settle` on `tag_user(target_x, env=dev)` expecting "OK". That second poll is the one that cannot converge.

Corroborating code in this checkout:
  services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/state/authorization.rs line 111: `let request = iam::Request::new(action, resource, &principals);` — `with_context` appears nowhere in the file, so the context is `&[]`.
  services/common/src/iam_authorization.rs module docs: "...nothing here guesses a condition value" and the fail-closed rule that "an `Allow` whose condition may not hold is not a grant"; a key absent from the context leaves the clause undecided.

Expected (AWS): `tag_user(target_x, [{env: dev}])` succeeds and `env=dev` reads back on target_x; `tag_user(target_x, [{env: prod}])` is refused error code `AccessDenied` (403) with target_x's env tag still `dev`; `tag_user(target_y, [{env: dev}])` is refused `AccessDenied` with no env tag on target_y; after TagUser(target_y, owner=<X>) the same call succeeds. Actual (emulator): every one of the three is refused `AccessDenied`, including the matching combination, so the poll for the permitted call never returns.

## Steps to Reproduce
1. Build and boot the emulator from this checkout (the iam service is bootstrapped and serving).
2. iam CreateUser target_x with Tags=[{Key: "owner", Value: "<X>"}]; CreateUser target_y with Tags=[{Key: "owner", Value: "<Y>"}]; CreateUser caller.
3. PutUserPolicy on caller with one document containing: Allow iam:GetUser on <caller arn>; and Allow iam:TagUser on [<target_x arn>, <target_y arn>] with Condition {"StringEquals": {"aws:ResourceTag/owner": "<X>", "aws:RequestTag/env": "dev"}, "ForAllValues:StringEquals": {"aws:TagKeys": ["env"]}}.
4. CreateAccessKey for caller and sign as caller against the same endpoint. GetUser(UserName=caller) succeeds — the unconditioned Allow works.
5. As caller, TagUser(UserName=target_x, Tags=[{Key: "env", Value: "dev"}]). Expect success, with ListUserTags(target_x) showing env=dev. Observe `AccessDenied` 403, forever, although both condition keys match.
6. As caller, TagUser(UserName=target_x, Tags=[{Key: "env", Value: "prod"}]) and TagUser(UserName=target_y, Tags=[{Key: "env", Value: "dev"}]). AWS refuses both `AccessDenied` and leaves the stored tags untouched; the emulator refuses them with the same code for the wrong reason (undecidable condition rather than a failing one), so the two outcomes are indistinguishable from step 5.
7. TagUser(UserName=target_y, Tags=[{Key: "owner", Value: "<X>"}]) as the administrative principal, then repeat step 6's second call — on AWS it now succeeds.
8. Equivalently: re-replay iam.sep-policy-composition-request-and-resource-tag-conditions-and-together-denied-when-either-fails.
