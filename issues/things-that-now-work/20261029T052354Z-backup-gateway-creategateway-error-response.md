Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 5bec88b641fb55fa8c22f4e210041b873076e418

# Hand-applied tag-rule refusals name the member in a different convention from the boundary validator the helper's own doc says they copy

_Filed: 2026-10-29T05:23:54Z_
_Operation: CreateGateway | Class: error-response_

> _Note: this issue was identified against the code base at commit `5bec88b641fb55fa8c22f4e210041b873076e418`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::validate_tag_pair` is the one place this crate words a tag-rule refusal, and its doc states the rule it follows: the refusal names the offending member "the way the boundary validator names a member it refuses itself". The crate's boundary renderer (`src/service.rs`) names a member by its lowerCamel VALIDATION PATH — the crate's own `SENSITIVE_MEMBERS` constant documents that ("a path segment is lowerCamel, so a violation on one of them is reported at `password` / `username`") — and wraps the clause in the AWS envelope `N validation error(s) detected: `.

The two call sites of `validate_tag_pair` break that rule: they pass the PascalCase MODEL member names `"Tags"` and `"VmwareToAwsTagMappings"`, and the resulting message is returned to the client as the whole `ValidationException$Message` with no envelope. So one service answers two different conventions for the identical "this member failed a constraint" condition in the identical sentence template:

* a `Tag$Key` pattern violation (refused at the boundary) -> `1 validation error detected: Value 'x' at 'tags.0.key' failed to satisfy constraint: Member must satisfy regular expression pattern: ...`
* an `aws:`-prefixed key or a spaced value on the SAME member (refused in the handler) -> `Value 'x' at 'Tags' failed to satisfy constraint: a tag key must not start with the reserved prefix 'aws:'`

The member name differs in case, carries no element position for a member that is a LIST (so a caller with several pairs cannot tell which one was refused), and the envelope is absent. Affected paths, all through the one helper: `CreateGateway$Tags` and `ImportHypervisorConfiguration$Tags` (via `tag_map`), `TagResource$Tags` (via `merged_tags`, including its tag-count refusal) and `PutHypervisorPropertyMappings$VmwareToAwsTagMappings` (via `validate_tag_mappings`).

## Evidence

The rule the crate states for itself — `src/state.rs:2429-2431` (doc of `validate_tag_pair`):

    /// Check ONE Amazon Web Services tag key/value pair, wherever in this model a request
    /// carries such a pair to be persisted — `member` is the member it arrived in, which the
    /// refusal names the way the boundary validator names a member it refuses itself.

How the boundary validator really names a member — `src/service.rs:63-71`:

    /// The members this service marks `@sensitive`, as a validation path spells them.
    ...
    /// `TestHypervisorConfigurationInput` and `UpdateHypervisorInput`; a path segment is
    /// lowerCamel, so a violation on one of them is reported at `password` / `username`.
    const SENSITIVE_MEMBERS: &[&str] = &["password", "username"];

and the clause/envelope it renders — `src/service.rs:131-134` and `src/service.rs:166`:

    Some(format!(
        "{subject} at '{}' failed to satisfy constraint: {requirement}",
        violation.path
    ))
    ...
        1 => Some(format!("1 validation error detected: {}", clauses[0])),

Side that BREAKS the rule — `src/state.rs:2499` (inside `merged_tags`) and `src/state.rs:2475-2479` (inside `validate_tag_mappings`):

        validate_tag_pair(&tag.key, &tag.value, "Tags")?;

        validate_tag_pair(
            &mapping.aws_tag_key,
            &mapping.aws_tag_value,
            "VmwareToAwsTagMappings",
        )?;

and the sentence that is handed to the client whole, with no envelope — `src/state.rs:2448-2459`:

    if key.to_ascii_lowercase().starts_with(RESERVED_TAG_KEY_PREFIX) {
        return Err(format!(
            "Value '{key}' at '{member}' failed to satisfy constraint: a tag key must not \
             start with the reserved prefix '{RESERVED_TAG_KEY_PREFIX}'"
        ));
    }

The first handler that reaches it — `src/ops/create_gateway.rs:167` (the same shape in `import_hypervisor_configuration.rs:167`, `tag_resource.rs:188-193`, `put_hypervisor_property_mappings.rs:211-220`), which puts the string straight into the modelled member:

        let tags = crate::state::tag_map(input.tags.as_deref().unwrap_or(&[])).map_err(
            |message| {
                CreateGatewayErrorModeled::ValidationException(ValidationException {
                    error_code: None,
                    message: Some(message),
                })
            },
        )?;

That the boundary form really is lowerCamel + envelope on this very crate is recorded in the crate's own fixed issue `issues/.../20261022T030803Z-backup-gateway-negative-paths-succeed-or-return-the.md`, whose replay quotes this emulator emitting `"1 validation error detected: Value 'bad\tpassword' at 'password' failed to satisfy constraint: ..."` for the model member `Password`.

Test coverage: `tests/all/aws_tag_rules_span_every_tag_pair.rs:122` pins the current PascalCase spelling —

        stated.contains("at 'VmwareToAwsTagMappings' failed to satisfy constraint: "),

with the stated purpose at `:111` being only "naming the member that failed its constraint and the value it refused" — it states no reason for a convention different from the boundary validator's, which is the rule `validate_tag_pair`'s own doc claims to follow. `tests/all/negative_paths.rs:53-61` pins the boundary form for the same service.

## Steps to Reproduce

1. Start the emulator with the backup-gateway service mounted and complete the activation handshake so a CreateGateway can succeed.
2. Call CreateGateway with Tags = [{"Key": "ok", "Value": "fine"}, {"Key": "aws:owner", "Value": "x"}] -> HTTP 400 ValidationException whose Message is exactly "Value 'aws:owner' at 'Tags' failed to satisfy constraint: a tag key must not start with the reserved prefix 'aws:'": the member is named 'Tags' (PascalCase, no element position, so the caller cannot tell which of the two pairs was refused) and there is no "1 validation error detected: " envelope.
3. Call CreateGateway with Tags = [{"Key": "ok", "Value": "fine"}, {"Key": "", "Value": "x"}] so the SAME member fails a modelled constraint at the boundary -> HTTP 400 ValidationException whose message is "1 validation error detected: Value '' at 'tags.1.key' failed to satisfy constraint: Member must have length greater than or equal to 1": lowerCamel path, element position, envelope.
4. Repeat with PutHypervisorPropertyMappings and VmwareToAwsTagMappings = [{..., "AwsTagKey": "aws:owner", "AwsTagValue": "x"}] -> "at 'VmwareToAwsTagMappings'", versus the boundary form 'vmwareToAwsTagMappings.0.awsTagKey' for a modelled violation on the same member.
5. Compare the two messages: one condition class, two member-naming conventions, although src/state.rs:2429-2431 says the handler form copies the boundary form.
