Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 1d8e7a96d86692d82e19cf4e2c81cc5cb0602adc

# GetMetadata's module summary says every reported feature carries "the value a PutMetadataFlag last wrote for it", but the handler reports a default for features nothing wrote

_Filed: 2026-11-09T10:29:10Z_
_Operation: GetMetadata | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `1d8e7a96d86692d82e19cf4e2c81cc5cb0602adc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The `GetMetadata` module summary states two things at once that cannot both hold: that the answer carries EVERY feature of the service's catalogue, and that each of them carries the value a `PutMetadataFlag` last wrote for it. For an app environment nobody has written a flag on — the common case, since the service models no create-metadata call — no `PutMetadataFlag` wrote anything, yet the handler still reports all five catalogue features, each carrying the emulator's own starting value `"false"`. The rule the summary states is therefore broken by the code directly under it, and a reader of that summary cannot tell whether an untouched app environment answers an empty map, an absent member, or five defaults. The same file's inline comment, the state helper it calls and the integration suite all state the missing branch explicitly, so the summary is the only place in the crate that gets the rule wrong.

## Evidence

The rule the module summary states:

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/get_metadata.rs:4-6
    //! Reports the UI-builder metadata of an app environment from [`crate::state`]: every
    //! feature in the catalogue the service holds, each with the value a `PutMetadataFlag`
    //! last wrote for it. Reporting only — it writes nothing.

What the code under it does — the default branch the summary omits:

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/get_metadata.rs:166-174
        // `GetMetadataResponse$features` is `@required`, so the map is always
        // reported: every feature of the catalogue, carrying what a
        // `PutMetadataFlag` wrote for this app environment or the default for one
        // nothing has written. An app environment nobody has touched is not an
        // error ...
        let features = state
            .lock()
            .metadata_features(&input.app_id, &input.environment_name);

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/state.rs:809-818
        METADATA_FEATURES
            .iter()
            .map(|feature| {
                let value = written
                    .and_then(|flags| flags.get(*feature))
                    .cloned()
                    .unwrap_or_else(|| METADATA_FEATURE_DEFAULT.to_string());

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/state.rs:137-144
    /// What `GetMetadata` reports for a feature no `PutMetadataFlag` has set.
    ...
    pub const METADATA_FEATURE_DEFAULT: &str = "false";

The existing test that pins the behaviour the summary contradicts:

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/tests/all/metadata_flags.rs:125
    /// Before anything is written, `GetMetadata` reports every feature of the catalogue at its
  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/tests/all/metadata_flags.rs:145-146
                json!(METADATA_FEATURE_DEFAULT),
                "case {case:?}: feature {feature:?} must start at {METADATA_FEATURE_DEFAULT:?}, \

Test coverage today: tests/all/metadata_flags.rs pins both the untouched-environment defaults and the written values, so the behaviour is correct and only the summary sentence is wrong — correcting it changes no response and no assertion.

## Steps to Reproduce

1. Start the emulator and call GET /app/d1234abcd/environment/staging/metadata without ever calling PutMetadataFlag for that app environment.
2. Observed: HTTP 200 with features = {autoGenerateForms: "false", autoGenerateViews: "false", isRelationshipSupported: "false", isNonModelSupported: "false", isGraphQLEnabled: "false"} — five values no `PutMetadataFlag` ever wrote.
3. Read src/ops/get_metadata.rs:4-6: the summary says each reported feature carries "the value a `PutMetadataFlag` last wrote for it", which is true of none of them.
4. Compare the same file's handler comment (get_metadata.rs:166-171), the helper it calls (state.rs:809-818) and state.rs:137-144, which all state the "or the default for one nothing has written" branch the summary leaves out.
