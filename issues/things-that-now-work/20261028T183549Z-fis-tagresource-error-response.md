Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 86a1fe66fd012b6ef331ae9dc1d2d842007fb4fe

# An over-long map KEY is refused with prose that breaks the crate's own stated rule for a value past its member's @length maximum

_Filed: 2026-10-28T18:35:49Z_
_Operation: TagResource | Class: error-response_

> _Note: this issue was identified against the code base at commit `86a1fe66fd012b6ef331ae9dc1d2d842007fb4fe`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate states, in `src/service.rs`, the ONE rule it words a refusal of "a value longer than its member's `@length` maximum" with: `"Maximum length exceeded: <member>"`, naming the member and nothing else. It applies that rule to every length violation the boundary validator catches, by rewording the framework refusal in `override_refusal`.

A map's KEY is the one modelled leaf the boundary validator does not reach, so the crate enforces the key target's `@length` itself in `crate::state::map_key_refusal` — its own doc says "This is where those bounds are enforced, for every map this service persists". But that handler-level enforcement of the SAME modelled `@length` maximum words its refusal a completely different way: `"{member}: {noun} is {min_length} to {max_length} characters long; this request carries one of {length}."`

So one and the same constraint class — a value past its member's modelled `@length` maximum — produces two different messages from the same service depending only on whether the over-long value happens to sit in a map key (handler path) or anywhere else (boundary path). A caller that keys on the message rule the crate itself documents gets the documented form for `description`, `roleArn`, a tag VALUE, … and the emulator's own prose for a tag key, a target name, an action name or a parameter name. `TagResource` is the shortest reproduction (it is the first handler that calls `map_keys_refusal` directly), but the same refusal reaches `CreateExperimentTemplate`, `StartExperiment` and `UpdateExperimentTemplate` through `template_refusal` -> `template_map_key_refusal`.

Status (400) and error code (`ValidationException`) are already right on both paths; what diverges is the message rule the crate states for itself. Only the LENGTH branch is in scope: `service.rs` explicitly leaves "every other refusal class" (the `@pattern`/whitespace branch) to the framework's wording, "because FIS publishes no rule for how it words those".

## Evidence

The rule the crate states for a length refusal, and the code that applies it:

`services/service-com-amazonaws-fis-faultinjectionsimulator/src/service.rs:108-112`
```
        // FIS states a value longer than its member's `@length` maximum as
        // "Maximum length exceeded: <member>" — a message built from the request, naming
        // the member that was too long and nothing else: the rejected value is NOT echoed
        // back. Every other refusal class keeps the framework's wording, because FIS
        // publishes no rule for how it words those.
```
`services/service-com-amazonaws-fis-faultinjectionsimulator/src/service.rs:125-128`
```
        Some(Refused {
            code: VALIDATION_EXCEPTION.to_string(),
            message: format!("Maximum length exceeded: {}", member_of(&violation.path)),
        })
```

The path that stands in for the boundary validator on a map key, and breaks that rule for the same `@length` maximum:

`services/service-com-amazonaws-fis-faultinjectionsimulator/src/state.rs:3050-3053`
```
/// The boundary validator captures a map's KEY without checking it (its VALUE target IS
/// checked), so a request carrying a key outside the modelled bounds of the map's key
/// target reaches the handler. This is where those bounds are enforced, for every map this
/// service persists — see [`MapKeyBounds`].
```
`services/service-com-amazonaws-fis-faultinjectionsimulator/src/state.rs:3067-3073`
```
    let length = key.chars().count();
    if !(min_length..=max_length).contains(&length) {
        return Some(invalid_input(format!(
            "{member}: {noun} is {min_length} to {max_length} characters long; this \
             request carries one of {length}."
        )));
    }
```

That the bound being enforced there IS a modelled `@length` maximum, in the crate's own words:

`services/service-com-amazonaws-fis-faultinjectionsimulator/src/state.rs:2999-3001`
```
/// What `TagMap`'s key target `TagKey` admits: `@length(min: 0, max: 128)` with
/// `pattern ^[\s\S]+$`, which matches any character and needs at least one — so a key of
/// whitespace alone IS a tag key ([`TAG_KEY_MIN_LENGTH`], [`TAG_KEY_MAX_LENGTH`]).
```

The caller that reaches it:

`services/service-com-amazonaws-fis-faultinjectionsimulator/src/ops/tag_resource.rs:160-168`
```
        // A map's KEY target is not reached by the boundary validator (its VALUE target
        // is), so `TagKey`'s modelled bounds are enforced here.
        if let Some(refusal) = crate::state::map_keys_refusal(
            "tags",
            crate::state::TAG_KEY_BOUNDS,
            input.tags.keys().map(String::as_str),
        ) {
            return Err(TagResourceErrorModeled::ValidationException(refusal).into());
        }
```

Test coverage: `tests/all/modelled_map_key_bounds.rs` is the suite that pins this refusal, and it pins only the status, the error code and that the message NAMES the map — `assert_refused` at `tests/all/modelled_map_key_bounds.rs:387-390`:
```
    let message = answer.body["message"].as_str().unwrap_or_default();
    assert!(
        message.contains(site.member_names()),
        "case {case:?}: expected the refusal to name {:?}, got {message:?}",
```
with `member_names()` returning `"tags"` / `"targets"` / `"actions"` / `"resourceTags"` / `"parameters"` (`tests/all/modelled_map_key_bounds.rs:120-128`). A message of the form the crate's own rule states (`"Maximum length exceeded: tags"`) still satisfies it, so bringing the two paths onto one rule changes no existing test.

## Steps to Reproduce

1. Boot the emulator with the fis service and create an experiment template (CreateExperimentTemplate) so there is a resource to tag.
2. Call TagResource on that template's ARN with a tag key 129 characters long (one past `TagKey`'s modelled `@length(max: 128)`) and any value.
3. Observe 400 ValidationException with message `tags: a tag key is 1 to 128 characters long; this request carries one of 129.`
4. Now provoke the SAME constraint class on a member the boundary validator reaches: call CreateExperimentTemplate with a `description` 513 characters long (one past its modelled `@length(max: 512)`).
5. Observe 400 ValidationException with message `Maximum length exceeded: description` — the form `src/service.rs:108-112` states this service words a value past its member's `@length` maximum with.
6. Compare: the same service refuses the same constraint class two different ways, and the map-key path is the one that departs from the rule the crate states for itself.
