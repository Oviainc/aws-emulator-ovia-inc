Issue-Source: trace-sweep-divergence
Issue-Trace: iam-get-user-self-and-list-tags
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iam-get-user-self-and-list-tags iam-get-user-self-and-not-found iam-get-user-self-identity iam-read-only-inspection-of-operator-identity iam-self-user-access-key-not-found iam-credential-conflict-and-malformed-errors iam-credential-not-found-errors iam-list-caller-policies-readonly iam-access-key-error-paths
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam|GetUser|2xx->4xx|$.GetUserResponse
Issue-Key: iam|GetUser|2xx->4xx|status
Issue-Key: iam|ListAccessKeys|2xx->2xx|$.ListAccessKeysResponse.ListAccessKeysResult.AccessKeyMetadata
Issue-Key: iam|ListUsers|2xx->2xx|$.ListUsersResponse.ListUsersResult.Users.member.Arn
Issue-Key: iam|ListUsers|2xx->2xx|$.ListUsersResponse.ListUsersResult.Users.member.UserName
Issue-Commit: d4c3381173b09dc1be1da315e64a87a78d438272

# Caller identity IAM operations fail when UserName is omitted

> _Note: this issue was identified against the code base at commit `d4c3381173b09dc1be1da315e64a87a78d438272`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetUser

## Description
Cause 1 of 3 - GetUser with no UserName. AWS: UserName is optional on GetUser and, when omitted, the operation returns the user making the request; the recorded response is 200 with GetUserResult/User{Path, UserName, Arn, UserId, CreateDate} for the calling principal. Emulator: HTTP 400. Why: 'who am I' is the first call almost every IAM client and credential-bootstrap flow makes; a 400 there breaks the flow before anything else runs, and in this corpus it also leaves the operator identity uncaptured so later steps compare against an unresolved value. Fix: the GetUser handler must default UserName to the calling principal rather than treating the absent member as a bad request.
Traces: iam-get-user-self-and-list-tags#task#2; iam-credential-conflict-and-malformed-errors#setup#2.

Cause 2 of 3 - ListAccessKeys with no UserName returns an empty list. AWS: UserName is optional and defaults to the calling user, and the response carries that user's keys - the recording shows AccessKeyMetadata/member{AccessKeyId, UserName, Status 'Active', CreateDate} for the caller. Emulator: AccessKeyMetadata is serialised empty (''), i.e. the caller's own key is invisible. Why: a client rotating its own credentials enumerates no key to rotate and concludes it has none. Fix: default UserName to the caller in the ListAccessKeys handler and return the credential the caller is authenticating with.
Traces: iam-access-key-error-paths#task#2.

Cause 3 of 3 - the operator principal reported by ListUsers does not match the one GetUser reports. AWS: the account's user list contains the calling operator under the same UserName and Arn that GetUser returns for it, so a client can correlate the two. Emulator: because GetUser (cause 1) never answers, the identity is never established, and ListUsers reports the emulator's own built-in principal ('emulator', arn:aws:iam::<accountId>:user/emulator) rather than the operator the corpus authenticated as. Why: a client cannot join its own identity to the account inventory - every self-scoped authorization or tagging decision keyed on that join is wrong. Fix: make the caller-identity resolution shared between GetUser and the user store so the authenticated principal appears in ListUsers as itself.
Traces: iam-read-only-inspection-of-operator-identity#task#2.

## Evidence
Cause 1: expected (recorded AWS) HTTP 200 GetUserResponse/GetUserResult/User{Path '/', UserName <caller>, Arn arn:aws:iam::<accountId>:user/<caller>, UserId, CreateDate}; actual HTTP 400 with no GetUserResponse at all (`$.GetUserResponse: missing (expected {...}, got None)`). Reproduced on 5 steps across iam-get-user-self-and-list-tags, iam-get-user-self-and-not-found, iam-get-user-self-identity, iam-read-only-inspection-of-operator-identity and iam-self-user-access-key-not-found, and on 3 more in setup phases.
Cause 2: expected ListAccessKeysResult/AccessKeyMetadata/member{AccessKeyId, UserName, Status 'Active', CreateDate}; actual AccessKeyMetadata = '' (type_mismatch).
Cause 3: expected ListUsersResult/Users/member/UserName and /Arn to equal the operator identity captured from GetUser; actual 'emulator' and 'arn:aws:iam::000000000000:user/emulator'.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Replay iam-get-user-self-and-list-tags#task#2: GetUser with no UserName parameter -> HTTP 400 instead of 200 with the caller's User.
3. Replay iam-access-key-error-paths#task#2: ListAccessKeys with no UserName -> empty AccessKeyMetadata instead of the caller's key.
4. Replay iam-read-only-inspection-of-operator-identity#task#1-2: the self GetUser fails, and ListUsers then reports the built-in 'emulator' principal rather than the authenticated operator.
