Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 20ab55bd9317ee9a3c91528cd79e8ca342e82eb9

# The tag writes are the only state changes in this crate that leave no record, breaking the attribution discipline every sibling mutator states

_Filed: 2026-10-28T21:50:34Z_
_Operation: TagResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `20ab55bd9317ee9a3c91528cd79e8ca342e82eb9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every other state-changing handler in this crate emits exactly one structured record naming the resource it changed and the outcome, and each one states that discipline in a comment above the call ("Attributable: …", "The acceptance is a state change, so it is attributable", "A state-changing call that changed nothing is still attributable, so the failure path records its outcome rather than passing silently"). `PutAccountSettings` goes further and puts its record on the dedicated `emulator::audit` target with the caller's access-key id and credential-scope region, read from `ctx`.

The two tag WRITES skip the discipline entirely. `TagResource` mutates persisted state (`resource.tags = tags;`) and `UntagResource` mutates it (`resource.tags.remove(key);`), yet neither emits any record at all — not on the audit target, not on the default one — and both open their handler body by discarding the very value the sibling reads the caller identity and region from (`_ctx: &smithy_server_runtime::OperationContext<'_>`). Their refusal paths are equally silent, while `ExportComplianceInquiry` and `PutComplianceInquiryFeedback` log `outcome = "refused"` for theirs.

So in one crate, writing the account's notification status is fully attributable and writing (or deleting) a resource's whole tag set is invisible: nothing records who tagged what, or that a tag was removed. No response body or status changes either way — the gap is purely that these two mutations are unobservable where every sibling mutation is.

## Evidence

The discipline, stated five times in this crate:

* `src/ops/put_account_settings.rs:160-163` — "// The settings record is account-wide, so nothing in the request metadata shapes\n        // it; the request's credential scope is read only to ATTRIBUTE the change below.\n        let akid = ::emulator_server::extract_akid(&ctx.parts().headers);\n        let region = crate::state::request_region(ctx.parts());"
* `src/ops/put_account_settings.rs:167-175` — "// A state-changing call that changed nothing is still attributable, so the\n            // failure path records its outcome rather than passing silently.\n            ::tracing::info!(\n                target: \"emulator::audit\",\n                operation = \"PutAccountSettings\",\n                akid = %akid," (and the success record at `put_account_settings.rs:198-214`).
* `src/ops/create_compliance_inquiry.rs:322-329` — "// Attributable: the resource created and how it was submitted. NEVER the\n        // inquiry's content or name — both are `@sensitive` in the model.\n        ::tracing::info!(\n            operation = \"CreateComplianceInquiry\","
* `src/ops/get_term_for_report.rs:208-211` — "// The acceptance is a state change, so it is attributable: the report version\n        // accepted, never the token itself.\n        ::tracing::info!("
* `src/ops/put_compliance_inquiry_feedback.rs:357-360` — "// Attributable: the inquiry rated, which query, and how. NEVER the `comment` —\n        // `@sensitive` in the model — nor the inquiry's name, content or queries.\n        ::tracing::info!(" (refusals at `:251`, `:286`, `:305`).
* `src/ops/export_compliance_inquiry.rs:250-254` — "// Attributable: the inquiry exported and the export minted for it. … ::tracing::info!(" (refusal at `:218`).

The two handlers that break it, writing state with no record and discarding `ctx`:

* `src/ops/tag_resource.rs:152-156` — "async fn handle(\n        &self,\n        input: Self::Input,\n        _ctx: &smithy_server_runtime::OperationContext<'_>,\n    ) -> Result<Self::Output, Self::Error> {" — the write it makes, `tag_resource.rs:199`: "resource.tags = tags;" then `tag_resource.rs:204`: "Ok(TagResourceOutput {})". A grep of the whole file for `tracing` matches nothing.
* `src/ops/untag_resource.rs:146-150` — the same `_ctx:` signature — and the write at `untag_resource.rs:179-183`: "for key in &input.tag_keys {\n            // A key the resource does not carry is already in the state the request asks\n            // for, which is what makes the operation `@idempotent`.\n            resource.tags.remove(key);\n        }". Also no `tracing` call anywhere in the file.

A repo grep for `tracing::info` under `services/service-com-amazonaws-artifact-artifact/src/ops` lists export, create, get_term_for_report, get_report, put_compliance_inquiry_feedback and put_account_settings — never a tag write.

Test coverage: the only suite that captures `tracing` output is `tests/all/account_settings.rs` (`every_write_emits_a_structured_audit_record`, `account_settings.rs:463-554`), whose rows are all `PutAccountSettings`; `tests/all/resource_tag_writes.rs` and `tests/all/resource_tags.rs` assert HTTP status and body only (no subscriber is installed there), so emitting the missing records leaves every response and every existing assertion unchanged.

## Steps to Reproduce

1. Run the emulator with the `emulator::audit` target (and the default INFO target) captured, as `tests/all/account_settings.rs:442` installs a subscriber.
2. PUT /v1/account-settings/put with {"notificationSubscriptionStatus":"SUBSCRIBED"} -> 200, and observe one record: `emulator::audit operation=PutAccountSettings akid=… region=… notification_subscription_status=SUBSCRIBED changed=true outcome=recorded` (src/ops/put_account_settings.rs:202-214).
3. POST /v1/compliance-inquiry/create with a text inquiry -> 202, and observe one record `operation=CreateComplianceInquiry compliance_inquiry_id=… outcome=created` (src/ops/create_compliance_inquiry.rs:323-329).
4. POST /tags/{inquiryArn} with {"tags":{"team":"audit"}} -> 204. Observe NO record at all, although the stored tag set changed (src/ops/tag_resource.rs:199).
5. DELETE /tags/{inquiryArn}?tagKeys=team -> 204. Again no record, although the tag was removed (src/ops/untag_resource.rs:182).
6. Compare with step 2/3: within one crate, two mutations are unobservable while every other mutation leaves exactly one attributable line.
