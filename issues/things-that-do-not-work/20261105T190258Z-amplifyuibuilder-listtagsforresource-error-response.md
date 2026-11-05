Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: f4ad776043a8e85dca4073be4e52e068eea62419

# Tagging operations report an id another app environment owns as absent (404), where every component and form path addressed by the same triple answers the wrong-scope 400

_Filed: 2026-11-05T19:02:58Z_
_Operation: ListTagsForResource | Class: error-response_

> _Note: this issue was identified against the code base at commit `f4ad776043a8e85dca4073be4e52e068eea62419`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The tagging surface resolves a `resourceArn` down to exactly the (appId, environmentName, id) triple every other reader of this store is keyed by (`crate::state::TaggedResource`), then looks the record up with the same `state.component(..)` / `state.form(..)` / `state.theme(..)` accessors the resource's own reader uses. But where `GetComponent`, `DeleteComponent`, `UpdateComponent`, `GetForm`, `DeleteForm`, `UpdateForm` and `GetCodegenJob` all consult the crate's `*_in_other_scope` predicate and answer the wrong-scope `InvalidParameterException` (400, "The item requests does not belong to that app or environment") for an id that resolves under a DIFFERENT app environment, `resource_tags` / `with_resource_tags_mut` never consult it: every miss collapses to `None`, and all three tagging handlers turn that single `None` into `item_not_found()` (404, "The item specified was not found").

So one and the same situation — `id` X lives in (appId=d1234abcd, env=staging), the request addresses (appId=d9999zzzz, env=staging, id=X) — is two different failure classes depending on which operation asks: 400 wrong-scope through `GET /app/{appId}/environment/{environmentName}/components/{id}`, 404 absent through `GET /tags/arn:aws:amplifyuibuilder:...:app/d9999zzzz/environment/staging/components/X`. Because this service strips every error-type discriminator from the wire (`src/service/untyped_errors.rs`), the status and the message literal ARE the only name the failure class has, so the two answers are not interchangeable to a client.

This directly contradicts the rule the crate writes down over the one place that test lives. All three tagging operations share the gap (`ListTagsForResource` through `resource_tags`, `TagResource` and `UntagResource` through `with_resource_tags_mut`), because both resolvers have the same shape: resolve the triple, look up, map, and hand a bare `Option` back.

## Evidence

The crate states the rule as absolute, over the helper that implements it:

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/state.rs:1364-1371
    /// Whether `id` names a record some OTHER app environment owns, in one of this
    /// store's (appId, environmentName)-keyed collections.
    ///
    /// The one place the wrong-scope test lives: every resource of this service that
    /// is addressed by (appId, environmentName, id) classifies a miss the same way,
    /// so the per-kind methods above ([`...::component_in_other_scope`])
    /// and its counterparts) differ only in which collection they search and where
    /// the record carries its id.

The tagging resolver IS addressed by that very triple, and says so:

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/state.rs:1959-1967
    /// The resource one `resourceArn` names: its kind, plus the (appId,
    /// environmentName, id) every reader of this store is keyed by.
    pub struct TaggedResource {
        pub kind: TaggedKind,
        pub app_id: String,
        pub environment_name: String,
        pub id: String,
    }

…yet it never classifies the miss — there is no `*_in_other_scope` branch anywhere in either resolver:

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/state.rs:2023-2035
    match resource.kind {
        TaggedKind::Component => state
            .component(app, environment, id)
            .map(|component| component.tags.clone()),
        TaggedKind::Form => state
            .form(app, environment, id)
            .map(|form| form.tags.clone().unwrap_or_default()),
        TaggedKind::Theme => state
            .theme(app, environment, id)
            .map(|theme| theme.tags.clone()),
    }

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/state.rs:2056-2059
    match resource.kind {
        TaggedKind::Component => state
            .component_mut(app, environment, id)
            .map(|component| change(&mut component.tags)),

so the three handlers answer one class for every miss:

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/list_tags_for_resource.rs:163-172
        let Some(tags) = crate::state::resource_tags(&store, &input.resource_arn) else {
            // An ARN that names no resource this store holds — including one that is not
            // an ARN of this service at all, which names no resource either — is absent:
            // the modelled `ResourceNotFoundException` (404) ...
            return Err(ListTagsForResourceErrorModeled::ResourceNotFoundException(
                crate::state::item_not_found(),
            )
            .into());
        };

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/tag_resource.rs:167-175  (same shape)
  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/untag_resource.rs:169-177 (same shape)

The sibling paths over the identical triple, which DO classify it:

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/get_component.rs:157-174
        let Some(component) = store.component(&input.app_id, &input.environment_name, &input.id)
        else {
            // ... an id a DIFFERENT app environment owns is a request made in the wrong
            // scope — the modelled `InvalidParameterException` (400) ...
            if store.component_in_other_scope(&input.app_id, &input.environment_name, &input.id) {
                return Err(GetComponentErrorModeled::InvalidParameterException(
                    crate::state::item_wrong_scope(),
                )
                .into());
            }
            return Err(GetComponentErrorModeled::ResourceNotFoundException(
                crate::state::item_not_found(),
            ) ... )

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/get_form.rs:161-165    (same, via `form_in_other_scope`)
  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/delete_form.rs:184-188  (same)
  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/update_form.rs:182-187   (same)

And the two literals the untyped wire makes load-bearing:

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/state.rs:1848
    pub const ITEM_NOT_FOUND_MESSAGE: &str = "The item specified was not found";
  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/state.rs:1871-1872
    pub const WRONG_SCOPE_MESSAGE: &str =
        "The item requests does not belong to that app or environment";

Test coverage today: the current answer is pinned, with no reason given for diverging from the sibling classification — tests/all/resource_tagging.rs:571-578 lists "a live id under another app" and "a live id under another environment" among rows it asserts 404 + ITEM_NOT_FOUND_MESSAGE for (lines 606-628), while tests/all/read_and_delete_scope_classes.rs and tests/all/wrong_scope_sentence.rs pin the 400 + WRONG_SCOPE_MESSAGE for the component and form paths over the same triple. A fix has to move those two rows.

## Steps to Reproduce

1. POST /app/d1234abcd/environment/staging/components with componentToCreate = {"name":"Hero","componentType":"Text","properties":{},"variants":[],"overrides":{},"bindingProperties":{}} — note the returned id X.
2. GET /app/d9999zzzz/environment/staging/components/X — observed HTTP 400 with message "The item requests does not belong to that app or environment" (the wrong-scope class, via get_component.rs:165).
3. GET /tags/arn%3Aaws%3Aamplifyuibuilder%3Aeu-west-1%3A000000000000%3Aapp%2Fd9999zzzz%2Fenvironment%2Fstaging%2Fcomponents%2FX — observed HTTP 404 with message "The item specified was not found", for the exact same (appId=d9999zzzz, environmentName=staging, id=X) triple.
4. Repeat step 3 as POST /tags/{arn} (TagResource) and DELETE /tags/{arn}?tagKeys=env (UntagResource): both answer the same 404.
5. The classification differs purely by which operation asked, although state.rs:1364-1371 declares that "every resource of this service that is addressed by (appId, environmentName, id) classifies a miss the same way", and the tagging resolver is addressed by that triple (state.rs:1959-1961).
