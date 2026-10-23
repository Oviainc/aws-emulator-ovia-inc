Issue-Severity: HIGH
Issue-Gap-Class: release-gate-failure
Issue-Commit: 3fe364395b32126959d291f25a0681e252d08c4c

# Release gate blocked 3fe364395b32 (changes_requested)

The release gate refused to advance `release` onto `3fe364395b32126959d291f25a0681e252d08c4c`, so this
commit is NOT released. The gate runs the merge gate's reviewer set against the
commit; the verdicts below are what it returned.

## Verdict
Gate ABORTED: 2 required reviewer(s) requested changes (exfil_scan, path_allowlist). Address the findings and call create_agent_review again.

## Reviewers
- `exfil_scan` (required) — **CHANGES_REQUESTED**: 1 exfil/escape signature finding(s) across 51030 changed file(s)
- `path_allowlist` (required) — **CHANGES_REQUESTED**: 156 pinned path(s) changed out of 101782

## Findings
- finding (`services/service-com-amazonaws-amp-amazonprometheusservice/src/ops/put_rule_groups_namespace.rs`): 
- pinned path changed (`.emulator/license-header`): .emulator/license-header is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`.emulator/scaffolded-specs.json`): .emulator/scaffolded-specs.json is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`.gitignore`): .gitignore is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`AGENTS.md`): AGENTS.md is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`Cargo.toml`): Cargo.toml is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`LICENSE`): LICENSE is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`README.md`): README.md is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`SECURITY.md`): SECURITY.md is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`core/embedded/Cargo.toml`): core/embedded/Cargo.toml is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`core/embedded/README.md`): core/embedded/README.md is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`core/embedded/src/lib.rs`): core/embedded/src/lib.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`core/embedded/tests/boot_serve.rs`): core/embedded/tests/boot_serve.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`core/router/Cargo.toml`): core/router/Cargo.toml is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`core/router/README.md`): core/router/README.md is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`core/router/build.rs`): core/router/build.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`core/router/src/lib.rs`): core/router/src/lib.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`core/router/src/main.rs`): core/router/src/main.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`issues/things-that-do-not-work/.gitkeep`): issues/things-that-do-not-work/.gitkeep is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`issues/things-that-now-work/.gitkeep`): issues/things-that-now-work/.gitkeep is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/LICENSE`): vendor/LICENSE is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/NOTICE`): vendor/NOTICE is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/README.md`): vendor/README.md is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/build-support/Cargo.toml`): vendor/build-support/Cargo.toml is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/build-support/src/lib.rs`): vendor/build-support/src/lib.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/emulator-paths/Cargo.toml`): vendor/emulator-paths/Cargo.toml is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/emulator-paths/src/lib.rs`): vendor/emulator-paths/src/lib.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/emulator-telemetry/Cargo.toml`): vendor/emulator-telemetry/Cargo.toml is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/emulator-telemetry/src/backend_contract.json`): vendor/emulator-telemetry/src/backend_contract.json is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/emulator-telemetry/src/backend_contract.rs`): vendor/emulator-telemetry/src/backend_contract.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/emulator-telemetry/src/client.rs`): vendor/emulator-telemetry/src/client.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/emulator-telemetry/src/config.rs`): vendor/emulator-telemetry/src/config.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/emulator-telemetry/src/integration_tests.rs`): vendor/emulator-telemetry/src/integration_tests.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/emulator-telemetry/src/lib.rs`): vendor/emulator-telemetry/src/lib.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/emulator-telemetry/src/metadata.rs`): vendor/emulator-telemetry/src/metadata.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/emulator-telemetry/src/outbox.rs`): vendor/emulator-telemetry/src/outbox.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/emulator-telemetry/src/policy.rs`): vendor/emulator-telemetry/src/policy.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/emulator-telemetry/src/test_support.rs`): vendor/emulator-telemetry/src/test_support.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/emulator-telemetry/src/types.rs`): vendor/emulator-telemetry/src/types.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/emulator-telemetry/src/wire.rs`): vendor/emulator-telemetry/src/wire.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/engine/Cargo.toml`): vendor/engine/Cargo.toml is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/engine/src/lib.rs`): vendor/engine/src/lib.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/server/Cargo.toml`): vendor/server/Cargo.toml is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/server/src/aws_credentials.rs`): vendor/server/src/aws_credentials.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/server/src/cli.rs`): vendor/server/src/cli.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/server/src/credential_store.rs`): vendor/server/src/credential_store.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/server/src/credential_store_tests.rs`): vendor/server/src/credential_store_tests.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/server/src/embedded.rs`): vendor/server/src/embedded.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/server/src/lib.rs`): vendor/server/src/lib.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/server/src/listener_guard.rs`): vendor/server/src/listener_guard.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/server/src/listeners.rs`): vendor/server/src/listeners.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/server/src/request_authentication.rs`): vendor/server/src/request_authentication.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/server/src/request_authentication_tests.rs`): vendor/server/src/request_authentication_tests.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/smithy-macros/Cargo.toml`): vendor/smithy-macros/Cargo.toml is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/smithy-macros/src/client.rs`): vendor/smithy-macros/src/client.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/smithy-macros/src/codegen.rs`): vendor/smithy-macros/src/codegen.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/smithy-macros/src/lib.rs`): vendor/smithy-macros/src/lib.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/smithy-schema-server/Cargo.toml`): vendor/smithy-schema-server/Cargo.toml is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/smithy-schema-server/src/aws_chunked.rs`): vendor/smithy-schema-server/src/aws_chunked.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/smithy-schema-server/src/constraints.rs`): vendor/smithy-schema-server/src/constraints.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/smithy-schema-server/src/http_binding.rs`): vendor/smithy-schema-server/src/http_binding.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/smithy-schema-server/src/json.rs`): vendor/smithy-schema-server/src/json.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/smithy-schema-server/src/lib.rs`): vendor/smithy-schema-server/src/lib.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/smithy-schema-server/src/protocol.rs`): vendor/smithy-schema-server/src/protocol.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/smithy-schema-server/src/query.rs`): vendor/smithy-schema-server/src/query.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/smithy-schema-server/src/rest_json.rs`): vendor/smithy-schema-server/src/rest_json.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/smithy-schema-server/src/rest_xml.rs`): vendor/smithy-schema-server/src/rest_xml.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/smithy-schema-server/src/rpc.rs`): vendor/smithy-schema-server/src/rpc.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/smithy-schema-server/src/xml.rs`): vendor/smithy-schema-server/src/xml.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/smithy-server-runtime/Cargo.toml`): vendor/smithy-server-runtime/Cargo.toml is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/smithy-server-runtime/src/errors.rs`): vendor/smithy-server-runtime/src/errors.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/smithy-server-runtime/src/extract.rs`): vendor/smithy-server-runtime/src/extract.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/smithy-server-runtime/src/lib.rs`): vendor/smithy-server-runtime/src/lib.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/smithy-server-runtime/src/multi_service_router.rs`): vendor/smithy-server-runtime/src/multi_service_router.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/smithy-server-runtime/src/operation.rs`): vendor/smithy-server-runtime/src/operation.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/smithy-server-runtime/src/pre_dispatch.rs`): vendor/smithy-server-runtime/src/pre_dispatch.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/smithy-server-runtime/src/router.rs`): vendor/smithy-server-runtime/src/router.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/smithy-server-runtime/src/service.rs`): vendor/smithy-server-runtime/src/service.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/smithy-server-runtime/src/wire.rs`): vendor/smithy-server-runtime/src/wire.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`.emulator/license-header`): .emulator/license-header is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`.emulator/scaffolded-specs.json`): .emulator/scaffolded-specs.json is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`.gitignore`): .gitignore is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`AGENTS.md`): AGENTS.md is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`Cargo.toml`): Cargo.toml is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`LICENSE`): LICENSE is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`README.md`): README.md is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`SECURITY.md`): SECURITY.md is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`core/embedded/Cargo.toml`): core/embedded/Cargo.toml is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`core/embedded/README.md`): core/embedded/README.md is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`core/embedded/src/lib.rs`): core/embedded/src/lib.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`core/embedded/tests/boot_serve.rs`): core/embedded/tests/boot_serve.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`core/router/Cargo.toml`): core/router/Cargo.toml is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`core/router/README.md`): core/router/README.md is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`core/router/build.rs`): core/router/build.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`core/router/src/lib.rs`): core/router/src/lib.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`core/router/src/main.rs`): core/router/src/main.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`issues/things-that-do-not-work/.gitkeep`): issues/things-that-do-not-work/.gitkeep is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`issues/things-that-now-work/.gitkeep`): issues/things-that-now-work/.gitkeep is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/LICENSE`): vendor/LICENSE is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/NOTICE`): vendor/NOTICE is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/README.md`): vendor/README.md is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/build-support/Cargo.toml`): vendor/build-support/Cargo.toml is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/build-support/src/lib.rs`): vendor/build-support/src/lib.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/emulator-paths/Cargo.toml`): vendor/emulator-paths/Cargo.toml is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/emulator-paths/src/lib.rs`): vendor/emulator-paths/src/lib.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/emulator-telemetry/Cargo.toml`): vendor/emulator-telemetry/Cargo.toml is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/emulator-telemetry/src/backend_contract.json`): vendor/emulator-telemetry/src/backend_contract.json is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/emulator-telemetry/src/backend_contract.rs`): vendor/emulator-telemetry/src/backend_contract.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/emulator-telemetry/src/client.rs`): vendor/emulator-telemetry/src/client.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/emulator-telemetry/src/config.rs`): vendor/emulator-telemetry/src/config.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/emulator-telemetry/src/integration_tests.rs`): vendor/emulator-telemetry/src/integration_tests.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/emulator-telemetry/src/lib.rs`): vendor/emulator-telemetry/src/lib.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/emulator-telemetry/src/metadata.rs`): vendor/emulator-telemetry/src/metadata.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/emulator-telemetry/src/outbox.rs`): vendor/emulator-telemetry/src/outbox.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/emulator-telemetry/src/policy.rs`): vendor/emulator-telemetry/src/policy.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/emulator-telemetry/src/test_support.rs`): vendor/emulator-telemetry/src/test_support.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/emulator-telemetry/src/types.rs`): vendor/emulator-telemetry/src/types.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/emulator-telemetry/src/wire.rs`): vendor/emulator-telemetry/src/wire.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/engine/Cargo.toml`): vendor/engine/Cargo.toml is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/engine/src/lib.rs`): vendor/engine/src/lib.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/server/Cargo.toml`): vendor/server/Cargo.toml is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/server/src/aws_credentials.rs`): vendor/server/src/aws_credentials.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/server/src/cli.rs`): vendor/server/src/cli.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/server/src/credential_store.rs`): vendor/server/src/credential_store.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/server/src/credential_store_tests.rs`): vendor/server/src/credential_store_tests.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/server/src/embedded.rs`): vendor/server/src/embedded.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/server/src/lib.rs`): vendor/server/src/lib.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/server/src/listener_guard.rs`): vendor/server/src/listener_guard.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/server/src/listeners.rs`): vendor/server/src/listeners.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/server/src/request_authentication.rs`): vendor/server/src/request_authentication.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/server/src/request_authentication_tests.rs`): vendor/server/src/request_authentication_tests.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/smithy-macros/Cargo.toml`): vendor/smithy-macros/Cargo.toml is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/smithy-macros/src/client.rs`): vendor/smithy-macros/src/client.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/smithy-macros/src/codegen.rs`): vendor/smithy-macros/src/codegen.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/smithy-macros/src/lib.rs`): vendor/smithy-macros/src/lib.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/smithy-schema-server/Cargo.toml`): vendor/smithy-schema-server/Cargo.toml is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/smithy-schema-server/src/aws_chunked.rs`): vendor/smithy-schema-server/src/aws_chunked.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/smithy-schema-server/src/constraints.rs`): vendor/smithy-schema-server/src/constraints.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/smithy-schema-server/src/http_binding.rs`): vendor/smithy-schema-server/src/http_binding.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/smithy-schema-server/src/json.rs`): vendor/smithy-schema-server/src/json.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/smithy-schema-server/src/lib.rs`): vendor/smithy-schema-server/src/lib.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/smithy-schema-server/src/protocol.rs`): vendor/smithy-schema-server/src/protocol.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/smithy-schema-server/src/query.rs`): vendor/smithy-schema-server/src/query.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/smithy-schema-server/src/rest_json.rs`): vendor/smithy-schema-server/src/rest_json.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/smithy-schema-server/src/rest_xml.rs`): vendor/smithy-schema-server/src/rest_xml.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/smithy-schema-server/src/rpc.rs`): vendor/smithy-schema-server/src/rpc.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/smithy-schema-server/src/xml.rs`): vendor/smithy-schema-server/src/xml.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/smithy-server-runtime/Cargo.toml`): vendor/smithy-server-runtime/Cargo.toml is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/smithy-server-runtime/src/errors.rs`): vendor/smithy-server-runtime/src/errors.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/smithy-server-runtime/src/extract.rs`): vendor/smithy-server-runtime/src/extract.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/smithy-server-runtime/src/lib.rs`): vendor/smithy-server-runtime/src/lib.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/smithy-server-runtime/src/multi_service_router.rs`): vendor/smithy-server-runtime/src/multi_service_router.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/smithy-server-runtime/src/operation.rs`): vendor/smithy-server-runtime/src/operation.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/smithy-server-runtime/src/pre_dispatch.rs`): vendor/smithy-server-runtime/src/pre_dispatch.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/smithy-server-runtime/src/router.rs`): vendor/smithy-server-runtime/src/router.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/smithy-server-runtime/src/service.rs`): vendor/smithy-server-runtime/src/service.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.
- pinned path changed (`vendor/smithy-server-runtime/src/wire.rs`): vendor/smithy-server-runtime/src/wire.rs is pinned: a fix may only change files under services/, core/router/tests/, the issue files and Cargo.lock.

## Steps to Reproduce
Re-run the release gate for `3fe364395b32126959d291f25a0681e252d08c4c` on `mainline`.
