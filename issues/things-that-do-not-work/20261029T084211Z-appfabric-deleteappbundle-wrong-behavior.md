Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 503006d3ba8cd79659b55d355ff4fa6b9fbb777f

# DeleteAppBundle's audit record names the caller's identifier instead of the resolved bundle record, unlike every sibling delete

_Filed: 2026-10-29T08:42:11Z_
_Operation: DeleteAppBundle | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `503006d3ba8cd79659b55d355ff4fa6b9fbb777f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeleteAppBundle` resolves the bundle, copies the record's own store key into `bundle_key` expressly so that nothing else can be named, removes the record by that key — and then writes its audit record with `&input.app_bundle_identifier`, i.e. whatever string the caller happened to put in the URL.

The three sibling deletes in this same crate all do the opposite on their success path: each copies the resolved record's own id and passes THAT id to `crate::state::audit_resource_change`, which the helper's own doc describes as recording "which server-minted resource changed" and as "the one place this service writes an audit record, so every create and delete records the same fields".

Two consequences, both inside this crate:

* the same successful deletion of the same bundle produces two DIFFERENT audit records depending only on which of the two `Identifier` forms the caller used (the bare UUID or the ARN), so an audit trail cannot be correlated with the bundle id every other record for that bundle carries (`CreateAppBundle` logs `&id`, the bare UUID);
* the field stops being the server-minted resource id for this one operation, which is the single thing the shared helper exists to record uniformly across every create and delete.

Nothing in the handler states a reason for the divergence — the comment two lines above says the opposite ("The record's own key, read off the record the lookup resolved, so the remove below cannot name anything else") — and no test exercises `DeleteAppBundle` with the bare-UUID identifier form, so nothing pins the echo. The `"already absent"` branch legitimately logs the request identifier (there is no record to read a key from); it is the `"deleted"` branch that has `bundle_key` in hand and does not use it.

## Evidence

The helper's contract — services/service-com-amazonaws-appfabric-fabricfrontendservice/src/state.rs:1964-1971:

    /// Record one state-changing AppFabric operation: what the caller asked for, who
    /// asked (the access key id the request presented), which server-minted resource
    /// changed and how the call ended.
    ///
    /// The one place this service writes an audit record, so every create and delete
    /// records the same fields.

The offending path — src/ops/delete_app_bundle.rs:175-177 and 195-202:

        // The record's own key, read off the record the lookup resolved, so the
        // remove below cannot name anything else.
        let bundle_key = bundle.id.clone();
    ...
        store.app_bundles.remove(&bundle_key);
        crate::state::audit_resource_change(
            ctx.parts(),
            Self::NAME,
            "AppBundle",
            &input.app_bundle_identifier,
            "deleted",
        );

— `bundle_key` is computed and used for the removal, but the audit record gets the caller's raw `appBundleIdentifier`.

The sibling that does it the other way — src/ops/delete_ingestion.rs:229-239:

        // The record's own key, read off the record the lookup above resolved, so
        // the remove cannot name anything else.
        let ingestion_key = ingestion.id.clone();
        bundle.ingestions.remove(&ingestion_key);
        crate::state::audit_resource_change(
            ctx.parts(),
            Self::NAME,
            "Ingestion",
            &ingestion_key,
            "deleted",
        );

The same shape in the other two deletes — src/ops/delete_app_authorization.rs:229-236 (`bundle.app_authorizations.remove(&authorization_key);` then `audit_resource_change(..., &authorization_key, "deleted")`) and src/ops/delete_ingestion_destination.rs:255-263 (`ingestion.destinations.remove(&destination_key);` then `audit_resource_change(..., &destination_key, "deleted")`). `CreateAppBundle` logs the minted id too — src/ops/create_app_bundle.rs:268: `crate::state::audit_resource_change(ctx.parts(), Self::NAME, "AppBundle", &id, "created");`

Test coverage: `state_changing_operations_write_an_audit_record` (tests/all/app_bundle_ingestion_lifecycle.rs:1573) asserts one record per state-changing call. Its table asserts the bare UUIDs for the child deletes — lines 1697-1703 (`"a deleted ingestion", "DeleteIngestion", "Ingestion", ingestion_id.clone(), "deleted"`) and 1683-1689 (`destination_id.clone()`) — but line 1711-1717 uses the ARN for `DeleteAppBundle` (`"a deleted app bundle", "DeleteAppBundle", "AppBundle", bundle.clone(), "deleted"`), and the suite never calls `delete_app_bundle` with the bare-UUID form, so the caller-dependent id is not covered by any assertion.

## Steps to Reproduce

1. Boot the emulator with audit logging captured (as tests/all/app_bundle_ingestion_lifecycle.rs's `audit_capture` does).
2. POST /appbundles with {} and note the returned appBundle.arn and its bare UUID.
3. DELETE /appbundles/<bare-uuid> (the bundle holds no app authorizations, so the delete succeeds with HTTP 204).
4. Read the captured audit record: it carries resource_id=<bare-uuid>.
5. Repeat from step 2 in a fresh store, but DELETE /appbundles/<url-encoded full ARN>. The captured record now carries resource_id=<full ARN> for the same operation on the same kind of resource — the id depends on the caller's identifier form.
6. Contrast with the same two forms against DELETE /appbundles/{b}/ingestions/{i}: both produce resource_id=<ingestion bare uuid>, because that handler passes the resolved `ingestion_key`.
