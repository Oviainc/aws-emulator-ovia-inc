Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: d9cda5a9b6bc2cdf59d194d1abe409fa5e2cae3f

# dryRun=true is refused with 400 BadRequestException instead of performing a validate-only create

_Filed: 2026-10-24T13:29:38Z_
_Operation: CreateChannel | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d9cda5a9b6bc2cdf59d194d1abe409fa5e2cae3f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: `DryRun` is a modelled member of CreateChannelRequest. A create carrying dryRun=true is a VALIDATE-ONLY request: the service checks the configuration and answers successfully (2xx) without creating a channel, or answers the validation error the configuration earns. It does not reject the parameter itself.

Actual: the handler short-circuits on `input.dry_run == Some(true)` and returns 400 BadRequestException with the message "dryRun is not implemented by this emulator; omit dryRun to create the channel". Every client that passes the documented flag — including SDK/CLI call sites that set it to pre-validate a channel definition before committing — gets a hard client error for a request the service accepts. Note the crate already honours the equivalent flag elsewhere (StartMonitorDeployment maps dryRun onto the DRY_RUN_DEPLOYMENT_IN_PROGRESS status), so the refusal is specific to this operation.

## Evidence

Spec: CreateChannel input member `DryRun` (__boolean) is declared by the model (smithy_query operation=CreateChannel); the operation's BadRequestException is for requests the service cannot process, not for a member it defines. Handler: src/ops/create_channel.rs:185-193 returns the 400 unconditionally when dry_run is true. Contrast: src/ops/start_monitor_deployment.rs:229 honours its own `DryRun` member via `begin_monitor_deployment(input.dry_run...)`. Tests: no test in tests/all/channel_lifecycle.rs or elsewhere exercises dryRun, so nothing asserts the refusal.

## Steps to Reproduce

1. POST /prod/channels with a valid channel body plus "dryRun": true.
2. Expected: a success response for the validate-only call (and no channel created — GET /prod/channels shows none).
3. Actual: 400 BadRequestException with message "dryRun is not implemented by this emulator; omit dryRun to create the channel".
