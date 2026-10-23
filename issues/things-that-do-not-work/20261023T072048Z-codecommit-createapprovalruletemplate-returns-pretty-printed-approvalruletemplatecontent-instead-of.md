Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codecommit.approval-rule-template-content-preserved-verbatim
Issue-Shape-Id: com.amazonaws.codecommit#CodeCommit_20150413
Issue-Key: codecommit.approval-rule-template-content-preserved-verbatim|run#10|AssertionError|assert actual_created == canonical_content, (
Issue-Commit: 0b023ce22a329ff2896e758c2b0769a212a04448

# CodeCommit CreateApprovalRuleTemplate returns pretty-printed approvalRuleTemplateContent instead of AWS's canonical minified JSON

> _Note: this issue was identified against the code base at commit `0b023ce22a329ff2896e758c2b0769a212a04448`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApprovalRuleTemplate

## Description
Cause 1 of 1: the approvalRuleTemplateContent CodeCommit hands back is re-serialized with indentation and newlines, not canonicalized to AWS's minified form.

AWS: Smithy (com.amazonaws.codecommit#CodeCommit_20150413) CreateApprovalRuleTemplate takes approvalRuleTemplateName (@required, @length 1..100) and approvalRuleTemplateContent (@required, ApprovalRuleTemplateContent, @length min 1 max 3000) and returns approvalRuleTemplate (@required) carrying approvalRuleTemplateContent back. The model says nothing about the encoding of that string, so: recorded behavior only — AWS does not echo the submitted bytes verbatim, it canonicalizes the document to MINIFIED JSON (no spaces after ':' or ',', no newlines, no indentation) while preserving key order and values, and GetApprovalRuleTemplate returns that same canonical form. The recording's canonical value for the submitted template is '{"Version":"2018-11-08","DestinationReferences":["refs/heads/main","refs/heads/release/*"],"Statements":[{"Type":"Approvers","NumberOfApprovalsNeeded":2,"ApprovalPoolMembers":["arn:aws:sts::<account>:assumed-role/CodeCommitReview/*","arn:aws:iam::<account>:user/Mary_Major"]}]}'. The requirement is the canonicalization rule (minified, key order and values preserved), not that literal — the account id and the pool members come from the request.

Emulator: the returned approvalRuleTemplateContent is pretty-printed — two-space indentation and embedded newlines ('{\n  "Version": "2018-11-08",\n  "DestinationReferences": [\n    "refs/heads/main",\n ... \n}'). The JSON is semantically equal but its serialization is the emulator's own formatter, not the service's canonical form.

Why: a client that stores the returned content and later diffs it against what it submitted, or against what GetApprovalRuleTemplate / ListApprovalRuleTemplates returns, or that hashes it to detect drift, sees a spurious change on every round trip. The content also counts against ApprovalRuleTemplateContent's @length max 3000: the emulator's indentation inflates the stored length well past what AWS would store, so a template that fits in AWS can be pushed over the limit by the emulator's own formatting.

Fix: the approvalRuleTemplateContent canonicalization in services/service-com-amazonaws-codecommit-codecommit-20150413/src/ops/create_approval_rule_template.rs — the content must be re-serialized minified (and stored minified, so get_approval_rule_template.rs and update_approval_rule_template_content.rs return the same form) rather than pretty-printed.

Traces: codecommit.approval-rule-template-content-preserved-verbatim.

## Evidence
codecommit.approval-rule-template-content-preserved-verbatim [mismatch], at check_codecommit_approval_rule_template_content_preserved_verbatim.py:53 `assert actual_created == canonical_content`:

AssertionError: expected canonicalized (minified) content, expected='{"Version":"2018-11-08","DestinationReferences":["refs/heads/main","refs/heads/release/*"],"Statements":[{"Type":"Approvers","NumberOfApprovalsNeeded":2,"ApprovalPoolMembers":["arn:aws:sts::000000000000:assumed-role/CodeCommitReview/*","arn:aws:iam::000000000000:user/Mary_Major"]}]}' actual='{\n  "Version": "2018-11-08",\n  "DestinationReferences": [\n    "refs/heads/main",\n    "refs/heads/release/*"\n  ],\n  "Statements": [\n    {\n      "Type": "Approvers",\n      "NumberOfApprovalsNeeded": 2,\n      "ApprovalPoolMembers": [\n        "arn:aws:sts::000000000000:assumed-role/CodeCommitReview/*",\n        "arn:aws:iam::000000000000:user/Mary_Major"\n      ]\n    }\n  ]\n}'

The call succeeded with HTTP 200; the divergence is purely the serialization of the returned approvalRuleTemplateContent. Expected: minified JSON, key order and values preserved. Actual: two-space-indented, newline-separated JSON. The account id 000000000000 in the ARNs comes from the request and varies per environment; the minification is the requirement.

## Steps to Reproduce
1. Boot the emulator and point botocore at it. cc = client('codecommit').
2. content = '{\n  "Version": "2018-11-08",\n  "DestinationReferences": ["refs/heads/main", "refs/heads/release/*"],\n  "Statements": [{"Type": "Approvers", "NumberOfApprovalsNeeded": 2, "ApprovalPoolMembers": ["arn:aws:sts::<account>:assumed-role/CodeCommitReview/*", "arn:aws:iam::<account>:user/Mary_Major"]}]\n}'  (deliberately submitted with whitespace).
3. created = cc.create_approval_rule_template(approvalRuleTemplateName=T, approvalRuleTemplateContent=content)['approvalRuleTemplate']['approvalRuleTemplateContent'].
4. Compare created against json.dumps(json.loads(content), separators=(',', ':')) -> expect equality (AWS canonicalizes to minified JSON, preserving key order and values); observe a two-space-indented, newline-separated document instead.
5. got = cc.get_approval_rule_template(approvalRuleTemplateName=T)['approvalRuleTemplate']['approvalRuleTemplateContent'] -> expect the same minified string as step 4.
6. Re-replay codecommit.approval-rule-template-content-preserved-verbatim.
