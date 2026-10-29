Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: dbe80abdd81a79f1a95892fbee40e178463ee847

# Clearing an S3 object path to the empty string leaves the stale object version on the record

_Filed: 2026-10-29T12:53:11Z_
_Operation: UpdateEnvironment | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `dbe80abdd81a79f1a95892fbee40e178463ee847`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateEnvironment` merges each S3 object member independently: a path the request carries overwrites the stored path, and the object VERSION is written only when the request carries a version of its own. The empty string is a legal `RelativePath` value (`@length(0, 1024)`, pattern `.*`), and this crate's own rule says such a path "names no object". So `PATCH /environments/{Name}` with `{"PluginsS3Path": ""}` is accepted (the crate's own test row pins the acceptance), the merge stores `Some("")` for the path, and the previously stored `PluginsS3ObjectVersion` is left untouched.

The stored record is then self-contradictory, and `GetEnvironment` reports it verbatim: `PluginsS3Path: ""` beside `PluginsS3ObjectVersion: "<the old version>"` — an S3 object version identifying an object the record itself names none of. The same holds for `RequirementsS3Path`/`RequirementsS3ObjectVersion` and `StartupScriptS3Path`/`StartupScriptS3ObjectVersion`, which go through the identical merge lines.

A second symptom of the same unnormalised pair: an environment that names no plugins object because the member was never sent reads the member back as absent (pinned by `environment_lifecycle.rs:552-563`), while one whose path was cleared to `""` reads it back as the empty string — two records that both name no plugins object report differently.

## Evidence

The crate's own rule for what an empty path means — services/service-com-amazonaws-mwaa-amazonmwaa/src/state.rs:777-778 (doc of `RequestedS3Objects::validate`):
```
    /// `RelativePath` is `@length(0, 1024)`, so the empty string is a legal value that
    /// names no object: it carries no version requirement.
```
and state.rs:774 in the same doc block: "because a version stored for the object a previous call named does not identify the object this one names."

The merge that writes a cleared path but keeps the version — services/service-com-amazonaws-mwaa-amazonmwaa/src/ops/update_environment.rs:345-356:
```
        if let Some(value) = input.plugins_s3_path {
            spec.plugins_s3_path = Some(value);
        }
        if let Some(value) = input.plugins_s3_object_version {
            spec.plugins_s3_object_version = Some(value);
        }
        if let Some(value) = input.requirements_s3_path {
            spec.requirements_s3_path = Some(value);
        }
        if let Some(value) = input.requirements_s3_object_version {
            spec.requirements_s3_object_version = Some(value);
        }
```
The pairing check that cannot catch it, because it is judged on the REQUEST and an empty path "names no object" — state.rs:800-806:
```
            let names_object = path.is_some_and(|path| !path.is_empty());
            let names_version = version.is_some_and(|version| !version.is_empty());
            if names_object && !names_version {
                return Err(format!(
                    "{version_member} is required when {path_member} is specified."
                ));
```
What the reader then reports, both members verbatim — state.rs:996-1001 (`EnvironmentRecord::to_environment`):
```
            plugins_s3_path: self.spec.plugins_s3_path.clone(),
            plugins_s3_object_version: self.spec.plugins_s3_object_version.clone(),
            requirements_s3_path: self.spec.requirements_s3_path.clone(),
            requirements_s3_object_version: self.spec.requirements_s3_object_version.clone(),
            startup_script_s3_path: self.spec.startup_script_s3_path.clone(),
            startup_script_s3_object_version: self.spec.startup_script_s3_object_version.clone(),
```
Model text for the pair — `smithy_query(query="doc", shape="UpdateEnvironmentInput", member="PluginsS3ObjectVersion")`: "The version of the plugins.zip file on your Amazon S3 bucket. You must specify a version each time a plugins.zip file is updated." and member="PluginsS3Path": "The relative path to the plugins.zip file on your Amazon S3 bucket. For example, plugins.zip. If specified, then the plugins.zip version is required."

Test coverage: services/service-com-amazonaws-mwaa-amazonmwaa/tests/all/environment_business_rules.rs:399-403 pins only that the request is ACCEPTED, with its stated reason, and asserts nothing about what the record then reports:
```
        Row {
            label: "an empty path names no object, so it needs no version",
            members: json!({"PluginsS3Path": ""}),
            accepted: true,
        },
```
and services/service-com-amazonaws-mwaa-amazonmwaa/tests/all/environment_lifecycle.rs:552-563 pins that a path/version member the create never named reads back as `Null`:
```
    for member in [
        "PluginsS3Path",
        "PluginsS3ObjectVersion",
        ...
    ] {
        if !case.s3_pairs.iter().any(|(name, _)| *name == member) {
            assert_eq!(field(member), Value::Null, "case {name}: {member}");
        }
    }
```
No test covers a path cleared to the empty string on an environment that already carried that object, so nothing depends on the current answer.

## Steps to Reproduce

1. Create an environment that loads a plugins object: PUT /environments/Cleared with a valid body including "PluginsS3Path": "plugins.zip" and "PluginsS3ObjectVersion": "9VnR0zZr7Qz" (the harness in tests/all/environment_lifecycle.rs:406-410 builds such a body).
2. Wait past crate::state::TRANSITION_SPAN so the environment settles into AVAILABLE and accepts an update.
3. Clear the plugins object: PATCH /environments/Cleared with {"PluginsS3Path": ""} — accepted (HTTP 200), per the pinned row environment_business_rules.rs:399-403.
4. Wait past crate::state::TRANSITION_SPAN for the update's apply phase, then GET /environments/Cleared.
5. Observe Environment.PluginsS3Path == "" together with Environment.PluginsS3ObjectVersion == "9VnR0zZr7Qz": a version for an object the record names none of, while state.rs:777-778 states that an empty path "names no object".
6. Compare with an environment created without the member at all, which reports both members as absent (environment_lifecycle.rs:552-563).
