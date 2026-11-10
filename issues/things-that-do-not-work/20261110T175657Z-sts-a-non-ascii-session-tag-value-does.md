Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sts.assume-role-session-tags-unicode-value-round-trip
Issue-Shape-Id: com.amazonaws.sts#AWSSecurityTokenServiceV20110615
Issue-Key: sts|ListRoles|AccessDenied
Issue-Commit: c8d92fbfadf9264bdfc8938c9915e2f34d8396fc

# STS: a non-ASCII session tag value does not round-trip into aws:PrincipalTag, so the exactly-matching session is refused the action its role grants

> _Note: this issue was identified against the code base at commit `c8d92fbfadf9264bdfc8938c9915e2f34d8396fc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssumeRole

## Description
One root cause (Cause 1 of 1): a session tag VALUE containing non-ASCII characters is accepted by AssumeRole but no longer compares equal to the same string in an aws:PrincipalTag condition, so the session that carries the exact value is denied instead of allowed.

AWS: Smithy (com.amazonaws.sts) models AssumeRole.Tags as a list of Tag{Key: tagKeyType @length 1..128 @pattern ^[\p{L}\p{Z}\p{N}_.:/=+\-@]+$, Value: tagValueType @length 0..256 @pattern ^[\p{L}\p{Z}\p{N}_.:/=+\-@]*$}. Both patterns are built from the Unicode general categories \p{L}/\p{Z}/\p{N}, so a value made of accented Latin letters and CJK ideographs is a LEGAL session tag value — AWS accepts it and does not transform it. AWS preserves the value codepoint-for-codepoint and exposes it as the condition key aws:PrincipalTag/<key>; a StringEquals condition on that key holds if and only if the session's stored value equals the condition's string exactly (case-sensitive, codepoint-for-codepoint). Consequently a role whose identity policy allows iam:ListRoles under Condition StringEquals {aws:PrincipalTag/Team: <value>} ALLOWS the call (HTTP 200, the role list is returned) when the session was minted with Tags=[{Key: Team, Value: <that same value>}]; it refuses AccessDenied/403 only when the session carries no such tag, or a DIFFERENT value — including one differing only by stripped diacritics.

Emulator: AssumeRole accepts Tags=[{Key: "Team", Value: "Café-Zürich-北京"}] and mints working credentials (GetCallerIdentity under them succeeds), but the subsequent iam:ListRoles call under that session is REFUSED with HTTP 403 / error code AccessDenied and the message:
  "User: arn:aws:sts::<account>:assumed-role/<role-name>/<session-name> is not authorized to perform: iam:ListRoles on resource: arn:aws:iam::<account>:role/ because no identity-based policy allows the iam:ListRoles action"
In that message the FIXED text is "User: " … " is not authorized to perform: " … " on resource: " … " because no identity-based policy allows the " … " action"; the VARIABLE parts are the session ARN (account id, role name and RoleSessionName from the AssumeRole request), the action and the resource ARN — all taken from the request and the emulator's state. The message itself is the correct AWS refusal wording for this class; the defect is that this class is reached at all. The same check's two NEGATIVE legs — a session with no Team tag, and a session tagged with the diacritics-stripped "Cafe-Zurich-北京" — are each refused with exactly that message and PASS, which proves the condition engine runs, finds the role's inline policy and evaluates aws:PrincipalTag/Team. Only the leg whose tag value is byte-identical to the condition's string fails, so what diverges is the fidelity of the non-ASCII tag VALUE between the AssumeRole that stored it and the aws:PrincipalTag comparison that reads it back — the comparison sees two different strings where AWS sees one.

Why: a client that scopes access with session tags whose values contain any non-ASCII character (accented personal or team names, CJK names) is locked out of every resource its role grants under that condition, while the ASCII-only equivalent works. The failure mode is silent and under-permissive: AssumeRole returns 200, the credential is live, and only the authorized call is refused — with a message that blames the caller's identity policy rather than the tag round trip, so a client debugging it is pointed at the wrong component. It also makes the emulator unusable as a parity target for tag-based access control in any non-English deployment.

Fix: two components, both on the path this value travels. (1) The STS AssumeRole handler's session-tag capture in the sts crate's ops/assume_role.rs and the session record it writes in that crate's src/state.rs — the Tags arrive as awsQuery form-encoded members and must be percent-decoded as UTF-8 and stored unchanged, with no normalization, transliteration or lossy re-encoding. (2) The shared condition evaluator that resolves aws:PrincipalTag/<key> and performs the StringEquals comparison (services/common/src/iam_authorization.rs, the evaluator the IAM crate's state/authorization.rs already calls) — the comparison must be an exact codepoint comparison of the stored value against the policy string, with the two sides decoded identically. Hypothesis (neither the model nor the recording states which of the two it is): the form value is percent-decoded to bytes and then re-read as latin-1, or the value is Unicode-normalized/ASCII-folded on one side only, so the stored tag and the policy string diverge before they are compared. A fixer can localize it by reading back the stored tag value for a unicode-tagged session and comparing it to the submitted bytes.

Related but NOT covering this: open issue 20261029T120000Z-sts-3-of-the-8-session-scoping-causes-remain-open.md carries the SINGULAR check id sts.assume-role-session-tag-unicode-value-round-trip, whose divergence is the OPPOSITE one — its cause 1 is that no session-scoped authorization is evaluated at all, so there the NON-matching session SUCCEEDS at s3:ListAllMyBuckets (over-permissive). The check filed here (sts.assume-role-session-tags-unicode-value-round-trip, plural) is not among that issue's replay keys, and its divergence is an under-permissive refusal at IAM, where session scoping demonstrably IS evaluated. Fixing that issue's cause 1 would not make the exactly-matching unicode tag compare equal.

Traces: sts.assume-role-session-tags-unicode-value-round-trip (the exact-value leg; the two negative legs of the same check pass).

## Evidence
Check sts.assume-role-session-tags-unicode-value-round-trip, verdict [error].

The check creates an IAM user, a role trusting that user for sts:AssumeRole and sts:TagSession, and attaches to the role the inline policy:
  {"Effect":"Allow","Action":"iam:ListRoles","Resource":"*",
   "Condition":{"StringEquals":{"aws:PrincipalTag/Team":"Café-Zürich-北京"}}}

Leg 1 (passes) — session "no-tag-session", no Tags: iam:ListRoles refused AccessDenied with
  "User: arn:aws:sts::<account>:assumed-role/<role>/no-tag-session is not authorized to perform: iam:ListRoles on resource: arn:aws:iam::<account>:role/ because no identity-based policy allows the iam:ListRoles action"
Leg 2 (passes) — session "mismatched-tag-session", Tags=[{Team: "Cafe-Zurich-北京"}]: refused AccessDenied with the same message template.

Leg 3 (FAILS) — session "tagged-session", Tags=[{Team: "Café-Zürich-北京"}], byte-identical to the condition string.
  Expected (AWS): HTTP 200, result["Roles"] contains the created role name.
  Actual (emulator): botocore.exceptions.ClientError: An error occurred (AccessDenied) when calling the ListRoles operation: User: arn:aws:sts::000000000000:assumed-role/sim2real-behav-tagrt-role-<suffix>/tagged-session is not authorized to perform: iam:ListRoles on resource: arn:aws:iam::000000000000:role/ because no identity-based policy allows the iam:ListRoles action
  (The account id, role-name suffix and session name above are per-run values filling the message template, not requirements.)

That legs 1 and 2 produce the correct refusal while leg 3 produces the identical refusal is the diagnosis: the aws:PrincipalTag/Team comparison never holds for the non-ASCII value, so the session is treated as if it carried no matching tag.

Model basis: smithy_query shape Tag / tagValueType in services/service-com-amazonaws-sts-awssecuritytokenservicev20110615/smithy-model/com-amazonaws-sts-awssecuritytokenservicev20110615.json — Value @length 0..256, @pattern ^[\p{L}\p{Z}\p{N}_.:/=+\-@]*$, i.e. Unicode letters are admitted, confirming the value is legal input rather than something AWS would reject.

## Steps to Reproduce
1. Boot the emulator and replay check sts.assume-role-session-tags-unicode-value-round-trip.
2. Or by hand, against the emulator endpoint:
   a. iam.create_user(UserName=U); iam.create_access_key(UserName=U); iam.put_user_policy(UserName=U, PolicyDocument allowing sts:AssumeRole on "*").
   b. iam.create_role(RoleName=R, AssumeRolePolicyDocument allowing the user U both sts:AssumeRole and sts:TagSession).
   c. iam.put_role_policy(RoleName=R, PolicyDocument={"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"iam:ListRoles","Resource":"*","Condition":{"StringEquals":{"aws:PrincipalTag/Team":"Café-Zürich-北京"}}}]}).
   d. With U's long-term key: sts.assume_role(RoleArn=R_arn, RoleSessionName="tagged-session", Tags=[{"Key":"Team","Value":"Café-Zürich-北京"}]) — succeeds.
   e. With the returned temporary credentials: iam.list_roles().
3. AWS returns HTTP 200 and the role list. The emulator returns HTTP 403 AccessDenied, "User: arn:aws:sts::<account>:assumed-role/<R>/tagged-session is not authorized to perform: iam:ListRoles on resource: arn:aws:iam::<account>:role/ because no identity-based policy allows the iam:ListRoles action".
4. Control: repeat with an ASCII-only tag value (e.g. "Cafe-Zurich") on both the condition and the Tags — the call is allowed, isolating the non-ASCII value as the trigger.
5. Control: repeat step 2d with Tags=[{"Key":"Team","Value":"Cafe-Zurich-北京"}] (diacritics stripped) — correctly refused, confirming the condition is evaluated.
