Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: c0c2239955f7987f5942ca86a100c431d5defb9f

# The self-naming-destination refusal echoes the imageSetId, breaking the crate's own rule for a requirement about one member's relation to another

_Filed: 2026-10-29T03:23:44Z_
_Operation: CopyImageSet | Class: error-response_

> _Note: this issue was identified against the code base at commit `c0c2239955f7987f5942ca86a100c431d5defb9f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::handler_requirement_error` is the one formatter this crate words every handler-raised `ValidationException` through, and its doc states exactly when the rejected value is echoed and when it is withheld: it is withheld when the caller passes `None` "because the requirement is about the member's presence or its relation to another member rather than about what it held".

`CopyImageSet`'s refusal of a request whose `copyImageSetInformation.destinationImageSet.imageSetId` names the `sourceImageSetId` is precisely a requirement about ONE MEMBER'S RELATION TO ANOTHER: the value itself is a perfectly legal `ImageSetId` (it satisfies `^[0-9a-z]{32}$` and it names an image set the data store holds — the handler just read it), and what makes the request incoherent is only that it equals the other member, `sourceImageSetId`. Yet this is the single handler-raised refusal in the crate that passes `Some(..)` there, so the image set identifier is interpolated into the error body.

The sibling refusal nine lines above it in the SAME handler — the promote/destination clash, which is the other "two members that cannot hold together" case — passes `None` and withholds its value, and every handler-raised refusal in `update_image_set_metadata.rs` does the same (`refuse()` hard-codes `None`). The only other `Some(..)` call sites in the crate (`search_image_sets.rs:417-430`, `613-617`) echo an ENUM value the caller sent, not a resource identifier.

The same shape is stated as a service-wide rule in the wording-rules block of `state.rs`: a refusal "carries no identifier naming the resource the request addressed — a store id, an image set id, a job id, an ARN or a token", the one exception being a count measured against a bound the sentence names. This refusal carries an image set id.

The fix is to pass `None` at that one call site; the HTTP status (400) and the error code (`ValidationException`) are already correct and do not change.

## Evidence

The rule, in the formatter every handler-raised refusal of this crate goes through — services/service-com-amazonaws-medicalimaging-ahigatewayservice/src/state.rs:585-596:
```
/// `value` is the rejected value, and is echoed under exactly the rule the boundary
/// refusals follow: withheld for a member [`withholds_value`] covers, and withheld when
/// the caller passes `None` because the requirement is about the member's presence or
/// its relation to another member rather than about what it held.
pub fn handler_requirement_error(path: &str, value: Option<&str>, requirement: &str) -> String {
    let clause = match value {
        Some(shown) if !withholds_value(path) => {
            format!("Value '{shown}' at '{path}'{CONSTRAINT_SEPARATOR}{requirement}")
        }
        _ => format!("{path}{CONSTRAINT_SEPARATOR}{requirement}"),
    };
```

The code that breaks it — src/ops/copy_image_set.rs:509-522 (note the requirement text itself names the OTHER member):
```
        // A copy whose destination IS the source has no coherent answer: the one image
        // set would have to be the read-only source and the locked destination of the
        // same copy. Refused as the input-constraint failure it is, naming the member.
        if let Some(destination) = &information.destination_image_set {
            if destination.image_set_id == input.source_image_set_id {
                return Err(CopyImageSetErrorModeled::ValidationException(ValidationException {
                    message: crate::state::handler_requirement_error(
                        DESTINATION_IMAGE_SET_ID_PATH,
                        Some(&destination.image_set_id),
                        "Member must not name the source image set",
                    ),
```
`withholds_value` does not cover this path, so the branch taken is the echoing one — src/state.rs:501-508 lists the withholding members (`datastorename`, `jobname`, `kmskeyarn`, `copiableattributes`, `updatableattributes`, `removableattributes`) and `imagesetid` is not among them.

The sibling refusal of the same handler, for the other "two members that cannot hold together" case, withholds it — src/ops/copy_image_set.rs:498-506:
```
        if promote_to_primary && information.destination_image_set.is_some() {
            return Err(CopyImageSetErrorModeled::ValidationException(ValidationException {
                message: crate::state::handler_requirement_error(
                    DESTINATION_IMAGE_SET_PATH,
                    None,
                    "Member must not be present when promoteToPrimary is requested",
                ),
```
and so does every handler-raised refusal of `UpdateImageSetMetadata` — src/ops/update_image_set_metadata.rs:641-645:
```
fn refuse(path: &str, requirement: &str) -> UpdateImageSetMetadataError {
    UpdateImageSetMetadataErrorModeled::ValidationException(ValidationException {
        message: crate::state::handler_requirement_error(path, None, requirement),
    })
```

The service-wide shape the same module states — src/state.rs:265-271:
```
// Over all three, and over the state-conflict texts below them, one shape holds: a
// refusal sentence states the RULE the request broke, as a constant of this module, and
// carries no identifier naming the resource the request addressed — a store id, an image
// set id, a job id, an ARN or a token. The only request-derived value any of them
// interpolates is a COUNT measured against a bound the same sentence names
```

Test coverage: the behaviour is exercised by `tests/all/image_set_copy.rs:859-899` (`a_destination_naming_the_source_is_refused`), which asserts only the status (400), the code (`ValidationException`) and that `message.contains("copyImageSetInformation.destinationImageSet.imageSetId")` — src/ops/.../tests/all/image_set_copy.rs:886-890:
```
        let message = refused.string(&["message"]);
        assert!(
            message.contains("copyImageSetInformation.destinationImageSet.imageSetId"),
            "case versions={versions} claimed={claimed}: the refusal must name the member, got {message:?}"
        );
```
It makes no assertion about the echoed value, so withholding it leaves that test (and every other) passing.

## Steps to Reproduce

1. Create a data store and wait for it to reach ACTIVE, then obtain an image set in it (the suites seed one through the crate's `state` module, since no implemented operation mints one yet).
2. POST /datastore/{datastoreId}/imageSet/{imageSetId}/copyImageSet with body {"sourceImageSet":{"latestVersionId":"1"},"destinationImageSet":{"imageSetId":"<the same imageSetId>","latestVersionId":"1"}}.
3. Observe HTTP 400 ValidationException whose message is `1 validation error detected: Value '<imageSetId>' at 'copyImageSetInformation.destinationImageSet.imageSetId' failed to satisfy constraint: Member must not name the source image set` — the image set identifier is interpolated into the body.
4. Compare with the sibling refusal: POST the same route with ?promoteToPrimary=true and a destinationImageSet present. The message is `1 validation error detected: copyImageSetInformation.destinationImageSet failed to satisfy constraint: Member must not be present when promoteToPrimary is requested` — no value echoed, which is the form `handler_requirement_error`'s doc prescribes for a requirement about one member's relation to another.
