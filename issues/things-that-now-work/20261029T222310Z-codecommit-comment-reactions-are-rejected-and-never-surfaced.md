Issue-Source: trace-sweep-divergence
Issue-Trace: codecommit-comments-and-reactions-workflow
Issue-Step: task#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codecommit-comments-and-reactions-workflow
Issue-Shape-Id: com.amazonaws.codecommit#CodeCommit_20150413
Issue-Key: codecommit|PutCommentReaction|2xx->4xx|$
Issue-Key: codecommit|PutCommentReaction|2xx->4xx|status
Issue-Key: codecommit|GetComment|2xx->2xx|$.comment.reactionCounts
Issue-Key: codecommit|GetCommentReactions|2xx->2xx|$.reactionsForComment[*]
Issue-Key: codecommit|GetCommentsForComparedCommit|2xx->2xx|$.commentsForComparedCommitData[*]
Issue-Commit: 170d008231b721c2b3436293a90aa925526f50a2

# CodeCommit comment reactions are rejected and never surfaced on any comment read path (4 causes)

> _Note: this issue was identified against the code base at commit `170d008231b721c2b3436293a90aa925526f50a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutCommentReaction

## Description
FOUR net-new causes: the reaction feature is non-functional end to end — the write is refused, and the three read paths that must carry reaction state omit it. A fixer must close ALL FOUR before submitting. Related but NOT covering: open issue '20261022T213415Z-codecommit-putcommentreaction-is-a-501-stub-cmk-disablement' describes PutCommentReaction answering 501; it now answers 400 instead, and that issue carries no wire replay key, so these steps are unowned.

=== Cause 1 of 4: PutCommentReaction rejects a valid reaction value ===
AWS: Smithy PutCommentReaction takes commentId (@required) and reactionValue (@required, ReactionValue) and models InvalidReactionValueException (client, httpError 400 [defaulted]) for values it does not recognise; output is empty (Unit). Recorded behavior: the reaction name form 'THUMBSUP' is ACCEPTED — AWS answers HTTP 200 with an empty body — and the stored reaction is subsequently reported with emoji '\ud83d\udc4d', shortCode ':thumbsup:' and unicode 'U+1F44D'. So AWS accepts the name form alongside the emoji/short-code/unicode forms.
Emulator: HTTP 400 with {'__type': 'InvalidReactionValueException', 'message': 'Unrecognized Reaction Value: THUMBSUP'} — the accepted-value set is too narrow, so the write never lands.
Why: every reaction a client posts through the documented name form is refused; this is also the upstream cause of Causes 3 and 4 observing no reactions.
Fix: services/service-com-amazonaws-codecommit-codecommit-20150413/src/ops/put_comment_reaction.rs — accept the reaction NAME form (in addition to emoji, short code and unicode) and normalise it to the stored reaction before raising InvalidReactionValueException.
Traces: codecommit-comments-and-reactions-workflow#task#9.

=== Cause 2 of 4: GetComment omits Comment.reactionCounts ===
AWS: Smithy Comment models reactionCounts (ReactionCountsMap, 'a string to integer map that represents the number of individual users who have responded to a comment with the specified reactions') and callerReactions. Recorded behavior: a comment with no reactions still serializes reactionCounts as an EMPTY MAP, not as an absent member.
Emulator: the member is absent.
Why: a client that reads comment.reactionCounts to render counts gets null and must special-case the emulator; absence is also indistinguishable from 'reactions unsupported'.
Fix: src/ops/get_comment.rs / the shared Comment projection — always emit reactionCounts (empty map when there are none).
Traces: codecommit-comments-and-reactions-workflow#task#7.

=== Cause 3 of 4: GetCommentReactions returns an empty reactionsForComment list ===
AWS: GetCommentReactionsOutput.reactionsForComment is @required and carries one entry per distinct reaction, each with a `reaction` (emoji, shortCode, unicode) and the `reactionUsers` ARNs that submitted it.
Emulator: the list has no entries, so no reaction a client posted is ever readable.
Why: the operation is the only way to enumerate who reacted; it reports nothing.
Fix: src/ops/get_comment_reactions.rs plus the reaction store the handler reads (and Cause 1, which prevents anything being stored).
Traces: codecommit-comments-and-reactions-workflow#task#10.

=== Cause 4 of 4: GetCommentsForComparedCommit comments omit callerReactions and reactionCounts ===
AWS: each embedded Comment carries callerReactions (the reactions submitted by the calling identity, e.g. the reaction names) and reactionCounts (reaction -> count).
Emulator: both members are absent from every embedded comment; the rest of the comment (id, content, authorArn, deleted, clientRequestToken) is present.
Why: review UIs read reaction state from this bulk path rather than per-comment calls, so reactions are invisible there too.
Fix: the shared Comment projection used by src/ops/get_comments_for_compared_commit.rs.
Traces: codecommit-comments-and-reactions-workflow#task#11.

## Evidence
Cause 1 — expected HTTP 200 with an empty JSON body; actual HTTP 400 {'__type': 'InvalidReactionValueException', 'message': 'Unrecognized Reaction Value: THUMBSUP'}. The reaction value is a request value; the requirement is that the name form is accepted.
Cause 2 — status 200 as expected; expected $.comment.reactionCounts {} (present, empty), actual missing.
Cause 3 — status 200 as expected; expected $.reactionsForComment[0] = {'reaction': {'emoji': '\ud83d\udc4d', 'shortCode': ':thumbsup:', 'unicode': 'U+1F44D'}, 'reactionUsers': [<caller ARN>]}, actual None (empty list). The caller ARN is environment-derived; the entry's presence and its reaction triple are the requirement.
Cause 4 — status 200 as expected; expected each $.commentsForComparedCommitData[0].comments[i] to include 'callerReactions' (e.g. ['THUMBSUP']) and 'reactionCounts' (e.g. {'THUMBSUP': 1}), actual both members absent. Recorded commentIds/blobIds/commitIds/timestamps are per-run values, not requirements.

## Steps to Reproduce
1. Replay fixture codecommit-comments-and-reactions-workflow in order.
2. Step task#9 posts PutCommentReaction with reactionValue 'THUMBSUP' — must be 200, is 400 InvalidReactionValueException.
3. Step task#7 (GetComment) must carry comment.reactionCounts, even when empty.
4. Step task#10 (GetCommentReactions) must list the reaction and its reactionUsers.
5. Step task#11 (GetCommentsForComparedCommit) must carry callerReactions and reactionCounts on each embedded comment.
