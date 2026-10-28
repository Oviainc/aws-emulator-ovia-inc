Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 86a1fe66fd012b6ef331ae9dc1d2d842007fb4fe

# UpdateConfig judges the request body before resolving {configType}/{configId}, against the comment on its own not-found check which says the lookup is "Asked FIRST, before the body is judged at all"

_Filed: 2026-10-28T18:34:10Z_
_Operation: UpdateConfig | Class: error-response_

> _Note: this issue was identified against the code base at commit `86a1fe66fd012b6ef331ae9dc1d2d842007fb4fe`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateConfig`'s handler documents an explicit precedence for its two client refusals, in a comment sitting directly on the not-found closure: the config the URI addresses is resolved FIRST, "before the body is judged at all, so what the caller hears first is that the config it addressed is not there".

The code does the opposite. The first thing the handler does — before it ever touches the store — is derive the config type from `configData` and refuse a mismatch with `InvalidParameterException` (HTTP 431, `parameterName: "configData"`). The existence check that the comment calls "FIRST" runs about forty lines later, under the first lock.

So a request addressed at a `(configType, configId)` pair this account holds no config under, whose body happens to select a different union member, is answered 431 "configData selects config type X, but this config is of type Y" — a sentence about "this config" that asserts a stored type for a config that does not exist — instead of the 434 `ResourceNotFoundException` with `config_mutation_not_found_message` that the comment promises and that the crate's other refusals for a dead config pair answer. The caller is told its body is wrong about a resource it was never told is missing.

The same handler gets the ordering right for its other two gates — `validate_config_data` is deliberately run only after `configs.contains_key(&key)` under the same lock, and the post-await re-lookup repeats the not-found — so the one check that precedes the lookup is the outlier inside this handler.

## Evidence

The body judgement, which runs first — `services/service-com-amazonaws-groundstation-groundstation/src/ops/update_config.rs:159-171`:
```
        let data_type = crate::state::config_type_of(&input.config_data);
        if data_type != input.config_type {
            return Err(UpdateConfigErrorModeled::InvalidParameterException(
                InvalidParameterException {
                    message: Some(format!(
                        "configData selects config type {data_type}, but this config is of type {}",
                        input.config_type
                    )),
                    parameter_name: Some("configData".to_string()),
                },
            )
            .into());
        }
```

The comment it contradicts, on the not-found check — `src/ops/update_config.rs:173-178`:
```
        // A config is addressed by BOTH labels, so an id whose stored type is not the
        // `{configType}` label names no config to update — just as an unknown id does
        // not. Asked FIRST, before the body is judged at all, so what the caller hears
        // first is that the config it addressed is not there. The text is this MUTATION
        // path's own: the pair the request addressed plus the account the write was
        // scoped to, where a read of the same dead pair answers the read path's text.
```

and the lookup that actually runs after it — `src/ops/update_config.rs:196-204`:
```
        {
            let guard = state.lock();
            if !guard.configs.contains_key(&key) {
                return Err(not_found().into());
            }
            guard
                .validate_config_data(&input.config_data)
                .map_err(UpdateConfigErrorModeled::InvalidParameterException)?;
        }
```

Test coverage: the dead-pair refusal is pinned only with a MATCHING body, so nothing pins the mismatch ordering — `tests/all/config_defaults_and_references.rs:804-829` updates each deleted config with `&config_data` of its own type and asserts `434` / `ResourceNotFoundException` / `config_mutation_not_found_message(config_type, &config_id)`, and `tests/all/config_defaults_and_references.rs:844-860` does the same for `ABSENT_UUID` with a matching `s3RecordingConfig` body. No test sends a never-created pair together with a `configData` of another type.

## Steps to Reproduce

1. `PUT /config/s3-recording/00000000-0000-4000-8000-000000000000` (a well-formed `Uuid` the account holds no config under) with body `{"name": "rename", "configData": {"trackingConfig": {"autotrack": "PREFERRED"}}}`.
2. Observed: HTTP 431 `InvalidParameterException`, `parameterName: "configData"`, message "configData selects config type tracking, but this config is of type s3-recording" — a claim about the stored type of a config that does not exist.
3. Expected per the handler's own comment at src/ops/update_config.rs:175 ("Asked FIRST, before the body is judged at all, so what the caller hears first is that the config it addressed is not there"): HTTP 434 `ResourceNotFoundException` carrying `config_mutation_not_found_message("s3-recording", "00000000-0000-4000-8000-000000000000")`.
4. Repeat with the body's union member matching the label (`{"s3RecordingConfig": {...}}`) — the same dead pair now answers 434, as tests/all/config_defaults_and_references.rs:812-829 asserts, showing the status a caller gets for a missing config depends on its request body.
