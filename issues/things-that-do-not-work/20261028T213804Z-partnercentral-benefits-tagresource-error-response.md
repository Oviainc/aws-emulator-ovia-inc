Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 772fb5ab5b7496e2df886721dbd75d9882bc460e

# The reserved-tag-key refusal states its requirement as a violation of the tag COLLECTION, the form this crate reserves for the collection's own @length/@uniqueItems

_Filed: 2026-10-28T21:38:04Z_
_Operation: TagResource | Class: error-response_

> _Note: this issue was identified against the code base at commit `772fb5ab5b7496e2df886721dbd75d9882bc460e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`state::reserved_tag_key_refusal` is the one construction of the `aws:`-namespace refusal that `TagResource` (member `tags`), `UntagResource` (member `tagKeys`) and `CreateBenefitApplication` (member `Tags`) all answer with. Its own doc says the path "collapses onto the collection member, the way the walk reports a violation on a collection's element" — but it then hands the bare requirement to `member_validation_exception`, skipping `crate::service::element_requirement`, which is the crate's single construction of an element's requirement and the other half of what `service.rs` documents as "the way the walk reports a violation on a collection's element".

The consequence is a refusal that contradicts this crate's own documented reading of its message format. Per `service.rs`, a requirement stated flat against a collection member means the violation is on the collection ITSELF (its `@length`, its `@uniqueItems`); the nested `Member must satisfy constraint: [ … ]` form is "what tells a caller the violation is on an element rather than on the collection". A reserved tag key is a fact about ONE element of `tagKeys` (or about `Key` inside one element of `tags`), yet the caller is shown the shape the crate reserves for "the whole list is wrong". Neither of the two forms `service.rs` enumerates is produced: for `tagKeys` (a list of strings) the walk would nest the requirement under the collapsed `tagKeys` path; for `tags` (a list of structures) the walk would keep the member's own full path, because `service.rs` states that a violation "on a member INSIDE a structure element … keeps its own full path".

So one input member of this service is described two different ways depending on whether the walk or this helper refused it — the exact drift the helper's own doc claims to prevent.

## Evidence

The helper's claim and the code under it — services/service-com-amazonaws-partnercentralbenefits-partnercentralbenefitsservice/src/state.rs:2761-2782:
    /// member refusal of this crate goes through ([`member_validation_exception`], which
    /// renders the message with [`crate::service::member_clause`] and
    /// [`crate::service::validation_envelope`]) and echoes NOTHING of the rejected value; the
    /// element path collapses onto the collection member, the way the walk reports a violation
    /// on a collection's element.
    ...
    pub fn reserved_tag_key_refusal<'a>(
        member: &str,
        keys: impl IntoIterator<Item = &'a str>,
    ) -> Option<crate::ops::types::ValidationException> {
        if !keys.into_iter().any(is_reserved_tag_key) {
            return None;
        }
        Some(member_validation_exception(
            member,
            &reserved_tag_key_requirement(),
            VALIDATION_REASON_FIELD,
        ))
    }
(no call to `crate::service::element_requirement`).

What "the way the walk reports a violation on a collection's element" means in this crate — services/service-com-amazonaws-partnercentralbenefits-partnercentralbenefitsservice/src/service.rs:155-160:
    /// A violation on an ELEMENT of a collection is reported against the COLLECTION MEMBER
    /// itself — no element index and no `member` step — with the element's requirement
    /// nested inside `Member must satisfy constraint: [ … ]`, which is what tells a caller
    /// the violation is on an element rather than on the collection. A violation of the
    /// collection ITSELF (its `@length`, its `@uniqueItems`) is not nested, and neither is
    /// one on a member INSIDE a structure element, which keeps its own full path.

The existing helper the refusal path does not call — services/service-com-amazonaws-partnercentralbenefits-partnercentralbenefitsservice/src/service.rs:209-213:
    /// One requirement as it reads for a collection ELEMENT: the element's requirement set,
    /// bracketed — the constraint it failed, then the requirement that it be present at all.
    pub fn element_requirement(requirement: &str) -> String {
        format!("Member must satisfy constraint: [{requirement}, {ELEMENT_NOT_NULL}]")
    }
and the path it is applied on — services/service-com-amazonaws-partnercentralbenefits-partnercentralbenefitsservice/src/service.rs:182-186:
        while let Some(collection) = enclosing_collection(path) {
            requirement = element_requirement(&requirement);
            path = collection;
            depth += 1;
        }

The three call sites that answer with the flat form:
- services/service-com-amazonaws-partnercentralbenefits-partnercentralbenefitsservice/src/ops/tag_resource.rs:202-207 (`reserved_tag_key_refusal("tags", input.tags.iter().map(|tag| tag.key.as_str()))`)
- services/service-com-amazonaws-partnercentralbenefits-partnercentralbenefitsservice/src/ops/untag_resource.rs:186-191 (`reserved_tag_key_refusal("tagKeys", input.tag_keys.iter().map(String::as_str))`)
- services/service-com-amazonaws-partnercentralbenefits-partnercentralbenefitsservice/src/ops/create_benefit_application.rs:247-258 (`reserved_tag_key_refusal("Tags", …)`)

Test coverage: the flat requirement is asserted, but no test states a reason for NOT nesting it. tests/all/member_refusal_one_construction.rs:254-272 pins the string only:
            member: "tags".to_string(),
            requirement: reserved_tag_key_requirement(),
...
            member: "tagKeys".to_string(),
            requirement: reserved_tag_key_requirement(),
and tests/all/refusal_member_path.rs:10-16 states its rule purely about the PATH, not the requirement's nesting: "**A refusal of an input member names that member by the path the centralized constraint walk reports for it — the member in lowerCamel — in the clause of the AWS validation envelope and in `ValidationExceptionField$Name` alike.**" A fix keeps that path (`tags` / `tagKeys`) unchanged.

## Steps to Reproduce

1. Boot the emulator and create a benefit application (tests/all/benefit_application_lifecycle.rs Harness), keeping its Arn.
2. Call UntagResource with {"resourceArn": <that Arn>, "tagKeys": ["aws:createdBy"]}. Observe 400 ValidationException whose Message is `1 validation error detected: Value at 'tagKeys' failed to satisfy constraint: Member must not use the reserved tag key prefix: aws:` — the un-nested form src/service.rs:155-160 reserves for a violation of the collection itself.
3. For comparison, call UntagResource with a tagKeys entry that breaks the modelled `TagKey` constraint instead (e.g. a 200-character key), so the centralized walk refuses it: observe the same collapsed `tagKeys` path but the requirement nested as `Member must satisfy constraint: [<requirement>, Member must not be null]`, per src/service.rs:182-186 and src/service.rs:209-213.
4. Repeat step 2 against TagResource with {"tags": [{"Key": "aws:createdBy", "Value": "mine"}]}; the refusal names `tags` flat, where src/service.rs:160 says a violation on a member inside a structure element "keeps its own full path".
