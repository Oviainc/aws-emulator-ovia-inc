Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codecommit.approval-rule-template-name-already-exists-rejected codecommit.approval-rule-template-name-conflict-rejected codecommit.create-approval-rule-template-duplicate-name-rejected codecommit.associate-nonexistent-approval-rule-template-blocked codecommit.approval-rule-template-invalid-content-rejected codecommit.malformed-approval-rule-content-rejected codecommit.list-approval-rule-templates-invalid-continuation-token-rejected codecommit.list-approval-rule-templates-invalid-max-results-rejected codecommit.empty-branch-name-rejected codecommit.put-comment-reaction-invalid-value-rejected codecommit.tag-resource-invalid-arn codecommit.comment-lifecycle-post-get-update-delete codecommit.delete-comment-content-twice-rejected codecommit.comment-content-required-rejected
Issue-Shape-Id: com.amazonaws.codecommit#CodeCommit_20150413
Issue-Key: codecommit.approval-rule-template-name-already-exists-rejected|run#10|AssertionError|assert message == expected_message, (
Issue-Key: codecommit.approval-rule-template-name-conflict-rejected|run#9|AssertionError|assert create_message == expected_message, (
Issue-Key: codecommit.create-approval-rule-template-duplicate-name-rejected|run#11|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: codecommit.associate-nonexistent-approval-rule-template-blocked|run#9|AssertionError|assert message == expected_message, (
Issue-Key: codecommit.approval-rule-template-invalid-content-rejected|run#9|AssertionError|assert message == expected_message, (
Issue-Key: codecommit.malformed-approval-rule-content-rejected|run#11|AssertionError|assert message == (
Issue-Key: codecommit.list-approval-rule-templates-invalid-continuation-token-rejected|run#5|AssertionError|assert message == f"Error parsing next token: {token}", message
Issue-Key: codecommit.list-approval-rule-templates-invalid-max-results-rejected|run#4|AssertionError|assert message == "A valid limit is between 1 and 1000", message
Issue-Key: codecommit.empty-branch-name-rejected|run#9|AssertionError|assert message == "Branch name cannot be empty", message
Issue-Key: codecommit.put-comment-reaction-invalid-value-rejected|run#12|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: codecommit.tag-resource-invalid-arn|run#12|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: codecommit.comment-lifecycle-post-get-update-delete|run#23|AssertionError|assert message == expected_message, (
Issue-Key: codecommit.delete-comment-content-twice-rejected|run#14|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: codecommit.comment-content-required-rejected|run#10|AssertionError|assert message == expected_message, (
Issue-Commit: 0b023ce22a329ff2896e758c2b0769a212a04448

# CodeCommit client-error messages diverge from AWS: 10 error classes return emulator-authored prose instead of AWS's fixed or request-derived message text

> _Note: this issue was identified against the code base at commit `0b023ce22a329ff2896e758c2b0769a212a04448`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApprovalRuleTemplate

## Description
Ten net-new root causes in the codecommit crate's error serialization. In every one the HTTP status (400) and the error CODE are already correct — the assertion that fails is the one immediately after the code assertion — and only the Message diverges. Each of these messages has a KNOWN rule: it is either a fixed constant for that error class (recorded identically across runs, independent of the request) or a template built from a named request value. The rule is the requirement; the generated repository/template/token names in the examples are not. A fixer must close all ten before submitting.

Cause 1 of 10: ApprovalRuleTemplateNameAlreadyExistsException carries emulator prose instead of AWS's fixed text.
AWS: Smithy CreateApprovalRuleTemplate (and UpdateApprovalRuleTemplateName) model ApprovalRuleTemplateNameAlreadyExistsException as a client error with httpError 400. Recorded behavior: the message is the FIXED string 'The approval rule template cannot be created because a template with that name already exists in your AWS account.' — it names no template and no Region, i.e. it is a constant for this class.
Emulator: 'an approval rule template named <templateName> already exists in this Region' — a different, request-interpolated template that also asserts Region rather than account scope.
Why: AWS scopes approval-rule-template names per ACCOUNT; the emulator's text tells a caller the conflict is per Region, which is the wrong mental model for a multi-Region deployment, and any tool matching the documented text fails.
Fix: the ApprovalRuleTemplateNameAlreadyExistsException message construction in services/service-com-amazonaws-codecommit-codecommit-20150413/src/ops/create_approval_rule_template.rs and the shared name-conflict helper used by src/ops/update_approval_rule_template_name.rs.
Traces: codecommit.approval-rule-template-name-already-exists-rejected, codecommit.approval-rule-template-name-conflict-rejected, codecommit.create-approval-rule-template-duplicate-name-rejected.

Cause 2 of 10: ApprovalRuleTemplateDoesNotExistException drops the request-derived not-found template.
AWS: Smithy AssociateApprovalRuleTemplateWithRepository models ApprovalRuleTemplateDoesNotExistException (client, httpError 400). Recorded behavior: the message is built from the request as 'No approval rule template exists in this AWS Region with the following name: <approvalRuleTemplateName>. Make sure that you are signed in to the AWS Region where you created the rule, and then try again.'
Emulator: 'the approval rule template <approvalRuleTemplateName> does not exist'.
Why: the AWS text tells the caller the likely cause (wrong Region) — the single most common reason this call fails. The emulator's text drops that, and clients matching the documented template do not recognize the error.
Fix: the ApprovalRuleTemplateDoesNotExistException message construction in the codecommit crate's shared approval-rule-template lookup, used by src/ops/associate_approval_rule_template_with_repository.rs and src/ops/get_approval_rule_template.rs.
Traces: codecommit.associate-nonexistent-approval-rule-template-blocked.

Cause 3 of 10: the invalid-approval-rule-content errors leak a JSON-parser diagnostic instead of AWS's fixed text.
AWS: Smithy models InvalidApprovalRuleTemplateContentException on CreateApprovalRuleTemplate and InvalidApprovalRuleContentException on CreatePullRequestApprovalRule, both client errors with httpError 400. Recorded behavior: the message is the FIXED string 'The JSON in the approval rule template is not valid. Review the JSON and make sure it is valid, and then try again.' — it exposes no parser internals and no offsets.
Emulator: 'the content of the approval rule template is not valid: the content is not a valid JSON document: key must be a string at line 1 column 3' (and the same shape with 'line 1 column 2' on the other check) — the underlying deserializer's diagnostic is forwarded to the caller.
Why: the line/column offsets are an implementation detail of whichever JSON parser the emulator links, so the message is neither stable nor AWS's; a client that matches the documented text, or that logs the message as a user-facing validation hint, gets parser internals instead.
Fix: the error construction for InvalidApprovalRuleTemplateContentException / InvalidApprovalRuleContentException in services/service-com-amazonaws-codecommit-codecommit-20150413/src/ops/create_approval_rule_template.rs and src/ops/create_pull_request_approval_rule.rs — the parse error must be swallowed, not interpolated.
Traces: codecommit.approval-rule-template-invalid-content-rejected, codecommit.malformed-approval-rule-content-rejected.

Cause 4 of 10: InvalidContinuationTokenException message is lower-cased and so is not the AWS template.
AWS: Smithy ListApprovalRuleTemplates models InvalidContinuationTokenException (client, httpError 400). Recorded behavior: the message is built from the request as 'Error parsing next token: <nextToken>' — capital 'E'.
Emulator: 'error parsing next token: not-a-real-continuation-token' — the same template with a lower-cased first word.
Why: the template IS the requirement here (it embeds the request's token), and clients/tests that compare it byte-for-byte against AWS fail on every invalid-token path.
Fix: the InvalidContinuationTokenException message construction in services/service-com-amazonaws-codecommit-codecommit-20150413/src/ops/list_approval_rule_templates.rs (and the shared pagination-token validator).
Traces: codecommit.list-approval-rule-templates-invalid-continuation-token-rejected.

Cause 5 of 10: InvalidMaxResultsException wraps AWS's fixed text in extra prose.
AWS: Smithy ListApprovalRuleTemplates models InvalidMaxResultsException (client, httpError 400). Recorded behavior: the message is exactly the FIXED string 'A valid limit is between 1 and 1000'.
Emulator: 'the maxResults value 0 is not valid: a valid limit is between 1 and 1000' — the AWS sentence is present but prefixed with a request-interpolated clause and de-capitalized.
Why: same as above — the message for this class is a constant in AWS, so any equality comparison diverges, and the emulator is strictly more informative than the service clients are written against.
Fix: the InvalidMaxResultsException message construction in services/service-com-amazonaws-codecommit-codecommit-20150413/src/ops/list_approval_rule_templates.rs (and the shared maxResults validator used by the other paginated codecommit operations).
Traces: codecommit.list-approval-rule-templates-invalid-max-results-rejected.

Cause 6 of 10: InvalidBranchNameException for an empty branchName returns the generic Smithy constraint message instead of AWS's text.
AWS: Smithy CreateCommit and DeleteFile both model InvalidBranchNameException (client, httpError 400); BranchName is @length min 1 max 256. Recorded behavior: an explicitly empty branchName is refused with InvalidBranchNameException whose message is the FIXED string 'Branch name cannot be empty'.
Emulator: 'The specified reference name is not valid. (member: branchName)' — the framework's generic constraint-violation text with the member name appended, identical on both operations.
Why: the '(member: <name>)' suffix is the emulator's own validation layer showing through; it is not an AWS message shape at all, so it marks every constraint rejection as emulator-generated rather than service-generated and breaks message-matching clients.
Fix: the empty/invalid branchName rejection in services/service-com-amazonaws-codecommit-codecommit-20150413/src/ops/create_commit.rs and src/ops/delete_file.rs — the operation must raise its own InvalidBranchNameException with the service message instead of letting the generic length-constraint error serialize.
Traces: codecommit.empty-branch-name-rejected.

Cause 7 of 10: InvalidReactionValueException message is not the request-derived AWS template.
AWS: Smithy PutCommentReaction takes commentId and reactionValue (both @required) and models InvalidReactionValueException (client, httpError 400). Recorded behavior: the message is built from the request as 'Unrecognized Reaction Value: <reactionValue>'.
Emulator: 'the reaction value <reactionValue> is not valid'.
Why: clients that surface the rejected shortcode to a user parse the documented prefix; the different template breaks that, and differential clients flag every unsupported-reaction path.
Note: the open issue '20261022T213415Z-codecommit-putcommentreaction-is-a-501-stub-cmk-disablement.md' Cause 1 reports PutCommentReaction answering 501, with the replay key on that check's `assert code == "InvalidReactionValueException"` line. That cause is now satisfied — the code assertion passes — and the check fails one assertion later on the message, so this is a distinct residual cause of the same check.
Fix: the InvalidReactionValueException message construction in services/service-com-amazonaws-codecommit-codecommit-20150413/src/ops/put_comment_reaction.rs.
Traces: codecommit.put-comment-reaction-invalid-value-rejected.

Cause 8 of 10: InvalidResourceArnException message is request-interpolated where AWS's is fixed.
AWS: Smithy TagResource takes resourceArn (@required, ResourceArn) and tags (@required) and models InvalidResourceArnException (client, httpError 400). Recorded behavior: the message is exactly the FIXED string 'Inputted resource arn is not formatted correctly.' — it echoes no ARN.
Emulator: 'the value for the resource ARN <resourceArn> is not valid'.
Why: the emulator echoes the caller's malformed ARN back into the message, which AWS does not; clients comparing against the documented text fail and anything that logs the message gains an unexpected request echo.
Fix: the InvalidResourceArnException message construction in services/service-com-amazonaws-codecommit-codecommit-20150413/src/ops/tag_resource.rs and the shared resource-ARN parser used by untag_resource.rs / list_tags_for_resource.rs.
Traces: codecommit.tag-resource-invalid-arn.

Cause 9 of 10: the comment-already-deleted messages collapse AWS's double spaces to single spaces.
AWS: Smithy models CommentDeletedException (client, httpError 400) on GetComment/UpdateComment/DeleteCommentContent. Recorded behavior: the messages are FIXED strings that contain DOUBLE spaces after the sentence-ending periods — 'The specified comment has already been marked as deleted.  Verify that you specified a valid comment ID, and then try again.' on the read/update path, and 'You cannot delete this comment.  The specified comment has already been marked as deleted.  Verify that you specified a valid comment ID, and then try again.' on the delete path.
Emulator: byte-identical except that every double space is a single space.
Why: the message for this class is a constant in AWS, so a client, fixture or snapshot test that compares it exactly diverges on every already-deleted-comment path even though the words match; the whitespace is part of the recorded constant, not formatting latitude.
Fix: the CommentDeletedException message literals in services/service-com-amazonaws-codecommit-codecommit-20150413/src/ops/delete_comment_content.rs and the shared comment-lookup helper used by get_comment.rs / update_comment.rs — the stored literals must keep AWS's double spaces (and nothing in the serializer may normalize whitespace in messages).
Traces: codecommit.comment-lifecycle-post-get-update-delete, codecommit.delete-comment-content-twice-rejected.

Cause 10 of 10: CommentContentRequiredException carries emulator prose instead of AWS's fixed text.
AWS: Smithy models CommentContentRequiredException (client, httpError 400) on the PostCommentFor* / UpdateComment operations. Recorded behavior: the message is the FIXED string 'The comment could not be saved because no content was provided. Comments cannot be empty.'
Emulator: 'the comment content is required'.
Why: the AWS text states both the failure and the rule ('Comments cannot be empty'); clients that surface it to a reviewer, or match it, get neither.
Fix: the CommentContentRequiredException message construction in the codecommit crate's shared comment-content validator used by src/ops/post_comment_for_compared_commit.rs, post_comment_for_pull_request.rs, post_comment_reply.rs and update_comment.rs.
Traces: codecommit.comment-content-required-rejected.

## Evidence
Every item below is the verbatim AssertionError from the simparity-behavioral-aws-v1 replay report. In all fourteen checks the preceding error-CODE assertion passed, so HTTP 400 and the error code are already correct and only the Message diverges. Generated template/repository/token names vary per run and are examples; the message TEMPLATE or CONSTANT is the requirement.

Cause 1 — codecommit.approval-rule-template-name-already-exists-rejected [mismatch]: "expected message 'The approval rule template cannot be created because a template with that name already exists in your AWS account.', got 'an approval rule template named sim2real-cc-template-69277dbf3203 already exists in this Region'". codecommit.approval-rule-template-name-conflict-rejected [mismatch]: same expected string, got 'an approval rule template named sim2real-behav-artnc-a-9a55dd1cfca8 already exists in this Region'. codecommit.create-approval-rule-template-duplicate-name-rejected [mismatch]: same expected string, got 'an approval rule template named sim2real-behav-artdup-0e8961c1d76c already exists in this Region'. Three independent checks, one message template.

Cause 2 — codecommit.associate-nonexistent-approval-rule-template-blocked [mismatch]: "expected message 'No approval rule template exists in this AWS Region with the following name: sim2real-behav-ghost-template-06953e77567e. Make sure that you are signed in to the AWS Region where you created the rule, and then try again.', got 'the approval rule template sim2real-behav-ghost-template-06953e77567e does not exist'".

Cause 3 — codecommit.approval-rule-template-invalid-content-rejected [mismatch]: "expected message 'The JSON in the approval rule template is not valid. Review the JSON and make sure it is valid, and then try again.', got 'the content of the approval rule template is not valid: the content is not a valid JSON document: key must be a string at line 1 column 3'". codecommit.malformed-approval-rule-content-rejected [mismatch]: AssertionError 'the content of the approval rule template is not valid: the content is not a valid JSON document: key must be a string at line 1 column 2' against the same expected text. The 'line 1 column N' offsets differ between the two checks, confirming the parser diagnostic is forwarded rather than a fixed string.

Cause 4 — codecommit.list-approval-rule-templates-invalid-continuation-token-rejected [mismatch]: assertion `assert message == f"Error parsing next token: {token}"`; actual 'error parsing next token: not-a-real-continuation-token'.

Cause 5 — codecommit.list-approval-rule-templates-invalid-max-results-rejected [mismatch]: assertion `assert message == "A valid limit is between 1 and 1000"`; actual 'the maxResults value 0 is not valid: a valid limit is between 1 and 1000'.

Cause 6 — codecommit.empty-branch-name-rejected [mismatch]: assertion `assert message == "Branch name cannot be empty"`; actual 'The specified reference name is not valid. (member: branchName)'. The `assert code == "InvalidBranchNameException"` immediately before it passed, so only the message diverges; the check then asserts the same message on DeleteFile.

Cause 7 — codecommit.put-comment-reaction-invalid-value-rejected [mismatch]: "expected message 'Unrecognized Reaction Value: not-a-real-emoji-shortcode', got 'the reaction value not-a-real-emoji-shortcode is not valid'".

Cause 8 — codecommit.tag-resource-invalid-arn [mismatch]: "expected message 'Inputted resource arn is not formatted correctly.', got 'the value for the resource ARN not-a-valid-arn is not valid'".

Cause 9 — codecommit.comment-lifecycle-post-get-update-delete [mismatch]: "expected message 'The specified comment has already been marked as deleted.  Verify that you specified a valid comment ID, and then try again.', got 'The specified comment has already been marked as deleted. Verify that you specified a valid comment ID, and then try again.'" (two spaces after 'deleted.' expected, one observed). codecommit.delete-comment-content-twice-rejected [mismatch]: "expected message 'You cannot delete this comment.  The specified comment has already been marked as deleted.  Verify that you specified a valid comment ID, and then try again.', got 'You cannot delete this comment. The specified comment has already been marked as deleted. Verify that you specified a valid comment ID, and then try again.'" (both double spaces collapsed).

Cause 10 — codecommit.comment-content-required-rejected [mismatch]: "expected message 'The comment could not be saved because no content was provided. Comments cannot be empty.', got 'the comment content is required'".

## Steps to Reproduce
Boot the emulator and point botocore at it. cc = client('codecommit'). In each step the status (400) and error code already match; compare only the Message.

1. Cause 1: content = '{"Version": "2018-11-08", "Statements": [{"Type": "Approvers", "NumberOfApprovalsNeeded": 1}]}'; cc.create_approval_rule_template(approvalRuleTemplateName=T, approvalRuleTemplateContent=content); repeat the identical call -> expect 400 ApprovalRuleTemplateNameAlreadyExistsException with message exactly 'The approval rule template cannot be created because a template with that name already exists in your AWS account.'; observe 'an approval rule template named <T> already exists in this Region'. Same expectation via cc.update_approval_rule_template_name(oldApprovalRuleTemplateName=A, newApprovalRuleTemplateName=B) where B already exists.
2. Cause 2: cc.create_repository(repositoryName=R); cc.associate_approval_rule_template_with_repository(approvalRuleTemplateName='<a name never created>', repositoryName=R) -> expect 400 ApprovalRuleTemplateDoesNotExistException with 'No approval rule template exists in this AWS Region with the following name: <name>. Make sure that you are signed in to the AWS Region where you created the rule, and then try again.'; observe 'the approval rule template <name> does not exist'.
3. Cause 3: cc.create_approval_rule_template(approvalRuleTemplateName=T3, approvalRuleTemplateContent='{bad json') -> expect 400 InvalidApprovalRuleTemplateContentException with 'The JSON in the approval rule template is not valid. Review the JSON and make sure it is valid, and then try again.'; observe a message ending '... is not a valid JSON document: key must be a string at line 1 column N'. Same for cc.create_pull_request_approval_rule(pullRequestId=P, approvalRuleName='r', approvalRuleContent='{bad json') / InvalidApprovalRuleContentException.
4. Cause 4: cc.list_approval_rule_templates(nextToken='not-a-real-continuation-token') -> expect 400 InvalidContinuationTokenException with 'Error parsing next token: not-a-real-continuation-token'; observe the same sentence lower-cased.
5. Cause 5: cc.list_approval_rule_templates(maxResults=0) -> expect 400 InvalidMaxResultsException with exactly 'A valid limit is between 1 and 1000'; observe 'the maxResults value 0 is not valid: a valid limit is between 1 and 1000'.
6. Cause 6: u = client('codecommit', config=Config(parameter_validation=False)); u.create_commit(repositoryName=R6, branchName='', parentCommitId=C, putFiles=[{'filePath':'x.txt','fileContent':b'x'}]) -> expect 400 InvalidBranchNameException with 'Branch name cannot be empty'; observe 'The specified reference name is not valid. (member: branchName)'. Repeat with u.delete_file(repositoryName=R6, branchName='', filePath='README.md', parentCommitId=C).
7. Cause 7: post a comment, then cc.put_comment_reaction(commentId=CID, reactionValue='not-a-real-emoji-shortcode') -> expect 400 InvalidReactionValueException with 'Unrecognized Reaction Value: not-a-real-emoji-shortcode'; observe 'the reaction value not-a-real-emoji-shortcode is not valid'.
8. Cause 8: cc.tag_resource(resourceArn='not-a-valid-arn', tags={'k':'v'}) -> expect 400 InvalidResourceArnException with exactly 'Inputted resource arn is not formatted correctly.'; observe 'the value for the resource ARN not-a-valid-arn is not valid'.
9. Cause 9: post a comment, cc.delete_comment_content(commentId=CID) once (expect 200), then cc.get_comment(commentId=CID) -> expect 400 CommentDeletedException with 'The specified comment has already been marked as deleted.' + TWO spaces + 'Verify that you specified a valid comment ID, and then try again.'; and cc.delete_comment_content(commentId=CID) again -> expect 'You cannot delete this comment.' + TWO spaces + the same two sentences, each separated by two spaces. Observe every double space collapsed to one.
10. Cause 10: cc.post_comment_for_compared_commit(repositoryName=R10, beforeCommitId=C1, afterCommitId=C2, content='') -> expect 400 CommentContentRequiredException with 'The comment could not be saved because no content was provided. Comments cannot be empty.'; observe 'the comment content is required'.
11. Re-replay every check id in trace_ids.
