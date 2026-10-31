Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 00c98a52cd0296ba6fc32dc46f579b7371348d52

# ListSessions entries never report Description, although the model documents that member as the approval TEAM's description

_Filed: 2026-10-29T07:41:02Z_
_Operation: ListSessions | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `00c98a52cd0296ba6fc32dc46f579b7371348d52`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every `ListSessions` entry fills `Description` from the stored session's own `description` field, which the one session constructor hard-codes to `None` (no operation of this model supplies a session description). So `Description` is absent from every entry of every page, forever.

The Smithy model, however, documents the two session reads' `Description` members DIFFERENTLY, and the list entry's is not the session's:

* `GetSessionResponse$Description` — "Description for the session." (absent is right: nothing records one)
* `ListSessionsResponseSession$Description` — "Description for the team."

The team's description is `@required` on `CreateApprovalTeam`, is stored on the team record, and is reported by `GetApprovalTeam` and by `ListApprovalTeams`; the session record even carries `approval_team_arn`/`approval_team_name`, so the value the member is documented to carry is resolvable from the store the handler already holds the lock on.

The cause is visible in the handler's own rationale: the entry is built "member by member off the record `GetSession` reports from, so the two reads cannot disagree" — which is right for every other member, but collapses the team-scoped `Description` of the list shape onto the session-scoped (never-set) one, so this documented member is served empty on every call.

## Evidence

The model sentence that states what the member carries — `smithy_query(query="doc", shape="ListSessionsResponseSession", member="Description")`:
```
"documentation": "<p>Description for the team.</p>"
```
and the contrasting sentence for the single-session read, `smithy_query(query="doc", shape="GetSessionResponse", member="Description")`:
```
"documentation": "<p>Description for the session.</p>"
```

What the handler reports instead — services/service-com-amazonaws-mpa-awsfluffycoreservice/src/ops/list_sessions.rs:294-296 (inside `fn entry(session: &crate::state::Session)`):
```
        // `Description` is `length(min: 1, …)`, so a session that recorded none reports
        // none rather than an empty string.
        description: session.description.clone(),
```

Why that value is ALWAYS absent — services/service-com-amazonaws-mpa-awsfluffycoreservice/src/state.rs:1993-1996, the one place a session record is built (`Session::for_team`):
```
            // `RequesterComment` and `Description` are what a requester supplies, and no
            // input that starts a session in this model carries either.
            requester_comment: None,
            description: None,
```

The value the member is documented to carry IS in the store, and the session names its team — state.rs:1291 (`ApprovalTeam`) and state.rs:1899-1901 (`Session`):
```
    pub description: String,
```
```
    /// The team whose approvers the session asks, and the name that team carries.
    pub approval_team_arn: String,
    pub approval_team_name: String,
```
and the sibling read reports it — src/ops/get_approval_team.rs:234:
```
            description: Some(team.description.clone()),
```
(`CreateApprovalTeam` input `Description` is `@required`, OPERATIONS.md:93.)

Test coverage: nothing pins the current (absent) value, so populating it leaves every existing assertion unchanged. tests/all/session_listing.rs:276-288 compares a FIXED member list between a listed entry and `GetSession` — `SessionArn`, `ApprovalTeamArn`, `ApprovalTeamName`, `Status`, `ActionName`, `ProtectedResourceArn`, `RequesterServicePrincipal`, `RequesterPrincipalArn`, `RequesterAccountId`, `RequesterRegion`, `ActionCompletionStrategy` — and tests/all/session_listing.rs:327-332 enumerates the members an outstanding session must NOT report:
```
            for member in ["CompletionTime", "StatusCode", "StatusMessage"] {
                assert!(
                    entry.get(member).is_none_or(Value::is_null),
```
`Description` appears in neither list, and `grep '"Description"' tests/all` has no hit in session_listing.rs.

## Steps to Reproduce

1. Record the caller as the organization's management account so the gated create is served (as tests/all/approval_team_lifecycle.rs:194 does), then `POST /approval-teams` with `Description: "the team's description"`, one approver and a seeded `Policies` reference.
2. Wait out `state::INVITATION_RESPONSE_DELAY` so the team reports `ACTIVE`, then `POST /approval-teams/{Arn}/baseline` to start a session for it.
3. `POST /approval-teams/{Arn}/sessions/?List` with body `{"MaxResults": 20}`.
4. Observe that the entry carries no `Description` at all; expected the approval team's description, which is what `ListSessionsResponseSession$Description` ("Description for the team.") names.
5. `GET /approval-teams/{Arn}` and observe `Description: "the team's description"` — the value is stored and reported elsewhere, so the listing is dropping a member it can compute from the same record.
6. For contrast, `GET /sessions/{SessionArn}` legitimately reports no `Description`: that member is documented "Description for the session." and nothing in this model supplies one.
