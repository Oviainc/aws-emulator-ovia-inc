Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: securityhub.create-members-lifecycle-created-then-deleted-returns-empty
Issue-Shape-Id: com.amazonaws.securityhub#SecurityHubAPIService
Issue-Key: securityhub.create-members-lifecycle-created-then-deleted-returns-empty|run#17|AssertionError|assert got_after["UnprocessedAccounts"] == [], got_after
Issue-Commit: 856373aafc0d758f44d6676eb38cecf9a8c6e509

# GetMembers synthesizes an UnprocessedAccounts entry for an unknown/deleted account instead of returning both lists empty

> _Note: this issue was identified against the code base at commit `856373aafc0d758f44d6676eb38cecf9a8c6e509`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMembers

## Description
One root cause. 1 cause total.

Cause 1 of 1: GetMembers for an account that is not a member of the caller's administrator account (here, an account that was created with CreateMembers and then removed with DeleteMembers) must come back as an empty response — neither a Members entry nor an UnprocessedAccounts entry. The emulator instead fabricates an UnprocessedAccounts result for the unknown account.

AWS: Smithy: GetMembers POST /members/get, input AccountIds (AccountIdList, required); output GetMembersResponse { Members: MemberList, UnprocessedAccounts: ResultList of Result{AccountId, ProcessingResult} }, HTTP 200. The model says nothing about how an unknown account id is reported, so: recorded behavior only — the behavioral check records that after DeleteMembers removes the account (and ListMembers no longer lists it), a subsequent GetMembers for that same account id answers 200 with Members == [] and UnprocessedAccounts == []. Security Hub does not resurrect the deleted record, and it also does not report the id as unprocessed; UnprocessedAccounts carries only accounts the administrator could identify but failed to process.

Emulator: answers 200 with Members: [] (correct) but UnprocessedAccounts: [{"AccountId": "<the requested account id>", "ProcessingResult": "The request is rejected because the account <AccountId> is not an associated member account of <AdministratorAccountId>"}]. The fixed text is "The request is rejected because the account " ... " is not an associated member account of "; the variable parts are the requested AccountId and the administrator account id (the emulator's own account, 000000000000 in the replay). That whole entry must not be emitted at all for an account the administrator does not know.

Why: clients treat UnprocessedAccounts as the partial-failure channel and branch on it — retrying, alarming, or failing a reconcile step when it is non-empty. A caller that confirms a DeleteMembers by re-reading the account sees a permanent "unprocessed" result that never clears, so deletion never looks complete; conversely a caller that uses GetMembers to probe whether an account is a member gets a spurious error-shaped payload instead of the empty answer AWS gives.

Fix: the GetMembers handler's membership lookup / response assembly in the securityhub service (services/service-com-amazonaws-securityhub-securityhubapiservice/src/ops/get_members.rs) — an account id with no record in the administrator's member state must simply be omitted from both Members and UnprocessedAccounts, not turned into a synthesized Result.

Related but not covering this: open issue 20261029T180317Z-securityhub-mutations-are-not-reflected-by-later-reads.md (Cause 2) reports the inverse case — an account that IS associated being wrongly listed in UnprocessedAccounts. This check's earlier GetMembers DID resolve the live member correctly, so that cause's lookup path is not what fails here; what fails is emitting an UnprocessedAccounts entry for a genuinely unknown id.

Traces: behavioral check securityhub.create-members-lifecycle-created-then-deleted-returns-empty (assertion `got_after["UnprocessedAccounts"] == []`).

## Evidence
Check securityhub.create-members-lifecycle-created-then-deleted-returns-empty, verdict mismatch.
Sequence: EnableSecurityHub -> CreateMembers(AccountDetails=[{AccountId: <fake id>, Email: ...}]) (UnprocessedAccounts == [], passed) -> GetMembers([id]) returns 1 Member with MemberStatus=Created (passed) -> DeleteMembers([id]) -> ListMembers no longer contains the id (passed) -> GetMembers([id]).
Expected (recorded AWS): HTTP 200, {"Members": [], "UnprocessedAccounts": []}.
Actual (emulator): HTTP 200, {"Members": [], "UnprocessedAccounts": [{"AccountId": "913281122222", "ProcessingResult": "The request is rejected because the account 913281122222 is not an associated member account of 000000000000"}]}.
Failure: AssertionError at check line 54 — `assert got_after["UnprocessedAccounts"] == []`. The account id and administrator id above are per-run values filling the message template, not requirements; the requirement is that no UnprocessedAccounts entry is produced.

## Steps to Reproduce
1. Boot the emulator and point a securityhub client at it.
2. EnableSecurityHub(EnableDefaultStandards=false) (tolerate ResourceConflictException).
3. CreateMembers(AccountDetails=[{AccountId: <any 12-digit id not otherwise known>, Email: "x@example.com"}]); assert UnprocessedAccounts == [].
4. GetMembers(AccountIds=[<that id>]) — observe one Member with MemberStatus=Created.
5. DeleteMembers(AccountIds=[<that id>]); confirm ListMembers no longer lists it.
6. GetMembers(AccountIds=[<that id>]) again — emulator returns an UnprocessedAccounts entry ("... is not an associated member account of ..."); AWS returns Members == [] and UnprocessedAccounts == [].
Equivalently: replay behavioral check securityhub.create-members-lifecycle-created-then-deleted-returns-empty.
