Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: a399f03ccc4f5afa40c0af0ce737308e8fb96c4f

# decode_page_token honours uppercase/mixed-case hex NextToken spellings encode_page_token never mints, so a token this service never issued is served as a cursor

_Filed: 2026-10-29T16:51:20Z_
_Operation: ListAccountAssignmentCreationStatus | Class: pagination_

> _Note: this issue was identified against the code base at commit `a399f03ccc4f5afa40c0af0ce737308e8fb96c4f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every paginated read of this crate resolves its `NextToken` through the one shared codec in `crate::state`: `encode_page_token` renders `ssoadmin:v1:<scope>:<key>` with `::hex::encode`, which emits only the lowercase digits `0-9a-f`, and `decode_page_token` inverts it with `::hex::decode`, which accepts `A-F` as well and even a mixed-case body (hex 0.4, the version Cargo.lock pins). So `token.to_uppercase()` of any cursor this service issued — a spelling `encode_page_token` can never produce, i.e. a token this service did not issue — decodes to the same scope and key and is honoured as a resume point, and so is any hand-written mixed-case rewrite of one.

That is exactly the input the codec's own doc says is refused, and the refusal exists for a stated reason: `cursor_after` answers `BAD_CURSOR_MESSAGE` so a token the service did not issue cannot silently resume (or silently end) a caller's enumeration. The handler of every paginated operation maps that refusal onto its modelled `ValidationException`; here it never fires, and the read answers 200 off a cursor it never handed out. `ListAccountAssignmentCreationStatus` is the first operation on the checklist whose handler calls the shared helper (`src/ops/list_account_assignment_creation_status.rs:204`); the same codec backs every other paginated read in the crate (ListApplications, ListInstances, ListPermissionSets, ListTagsForResource, ListRegions, …), so the hole is one fix in `crate::state`.

## Evidence

The rule, stated by the codec itself — services/service-com-amazonaws-ssoadmin-swbexternalservice/src/state.rs:4652-4662:

    /// The cursor is SELF-IDENTIFYING — it carries a version marker and that scope, so a
    /// token this service never issued (including one that happens to be valid hex) and a
    /// token issued for a DIFFERENT scope (another operation, another instance, another
    /// account or permission-set filter) are both rejected rather than silently resuming
    /// inside an enumeration whose keys mean something else. Hex keeps the token inside
    /// `Token`'s `^[-a-zA-Z0-9+=/_]*$` pattern …
    pub fn encode_page_token(scope: &str, key: &str) -> String {
        ::hex::encode(format!("ssoadmin:v1:{scope}:{key}"))
    }

The inverse, which checks only the prefix after a CASE-INSENSITIVE hex decode — state.rs:4664-4670:

    /// Decode a cursor [`encode_page_token`] issued for `scope`; `None` for anything else.
    pub fn decode_page_token(scope: &str, token: &str) -> Option<String> {
        let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
        decoded
            .strip_prefix(&format!("ssoadmin:v1:{scope}:"))
            .map(str::to_string)
    }

`::hex::encode` writes lowercase only; `::hex::decode` of the same crate (Cargo.lock:3177 `name = "hex"`) accepts upper, lower and mixed case, so the uppercased spelling of an issued token passes `strip_prefix` unchanged.

What the refusal is FOR — state.rs:4672-4686:

    /// The key a paginated read resumes after, from the caller's `NextToken`.
    ///
    /// `Ok(None)` starts at the beginning (no token). `Err` is the refusal the caller's own
    /// `ValidationException` carries: answering an empty page for a token this service did
    /// not issue would silently end a caller's enumeration early.
    pub fn cursor_after(…)

and state.rs:4643-4646: `pub const BAD_CURSOR_MESSAGE: &str = "NextToken is not a pagination token issued by this service for this enumeration";`

The named operation's own call site — src/ops/list_account_assignment_creation_status.rs:200-204:

    // The cursor is scoped to THIS enumeration: the operation and the whole filter
    // that selected the rows, since the page key only distinguishes rows within it.
    let scope =
        crate::state::cursor_account_assignment_creation_status(&input.instance_arn, filter);
    let after = crate::state::cursor_after(&scope, input.next_token.as_deref()).map_err(

No test pins the case rewrite: the suite's forged-token table lists every spelling it covers — tests/all/ssoadmin_permission_sets.rs:764-779 — "not hex at all, hex of a plain word, hex of a stored key with no cursor tag, hex of a cursor of the wrong kind, and a cursor of another enumeration verbatim" (`"notatoken", "zz", "0", hex_of_a_word, hex_of_a_key, hex_of_another_kind, foreign_cursor`) — and none of them is an uppercase/mixed-case rewrite of an issued cursor.

## Steps to Reproduce

1. Start the emulator and call `ListInstances` (X-Amz-Target: SWBExternalService.ListInstances) to settle the account's ambient instance, then `UpdateInstance` with `PermissionSetsEnabled: true` so the account-assignment surface is served.
2. `CreatePermissionSet` twice, then `CreateAccountAssignment` twice (two different principals) so the instance tracks two creation-status records.
3. POST `ListAccountAssignmentCreationStatus` with `{"InstanceArn": <arn>, "MaxResults": 1}` and capture the `NextToken` (lowercase hex).
4. POST `ListAccountAssignmentCreationStatus` again with the SAME body but `NextToken` replaced by its uppercase spelling (`token.to_uppercase()`).
5. Observed: 200 with the second page — a token this service never minted is honoured as a cursor. Expected, per `encode_page_token`'s doc (state.rs:4652-4656) and `BAD_CURSOR_MESSAGE`: 400 `ValidationException` with "NextToken is not a pagination token issued by this service for this enumeration".
6. The same two calls against any other paginated read of the crate (e.g. `ListPermissionSets`, `ListApplications`, `ListTagsForResource`) behave identically, since they share `crate::state::decode_page_token`.
