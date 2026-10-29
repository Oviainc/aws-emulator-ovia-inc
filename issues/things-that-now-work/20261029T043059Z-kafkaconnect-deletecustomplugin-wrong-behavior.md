Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 96b9d79b1b8395723cfce0b7aa8e9f6fed8726ec

# A custom plugin a stored connector is running on is deleted and dropped, leaving the dangling reference CreateConnector refuses to store

_Filed: 2026-10-29T04:30:59Z_
_Operation: DeleteCustomPlugin | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `96b9d79b1b8395723cfce0b7aa8e9f6fed8726ec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeleteCustomPlugin` resolves the plugin by ARN and region and immediately marks it `DELETING` with a `Remove` due on the clock; it never looks at `state.connectors`. `settle_due` then drops the record outright. Nothing else repairs the connectors that reference it, so after one `DeleteCustomPlugin` the store holds a RUNNING connector whose `plugins[0].customPlugin.customPluginArn` names a plugin this store no longer holds, and `DescribeConnector` / `ListConnectors` echo that ARN back (`plugin_descriptions()` clones the stored reference verbatim). `DescribeCustomPlugin` on the very ARN the connector reports then answers 404.

That is precisely the state this crate's `CreateConnector` path says a connector may not be stored in: it refuses a `plugins` entry whose plugin is not `ACTIVE` ("cannot back a connector") and refuses an absent reference so it is not "stored as a dangling reference the read operations would then echo back". The create path and the delete path therefore disagree about the same invariant: one spends four refusals enforcing it, the other walks the store straight into the forbidden state, and the reads the create path was protecting are exactly the ones that now echo an unresolvable ARN.

The same defect is present verbatim in `delete_worker_configuration.rs`, whose removal leaves `DescribeConnector$workerConfiguration` naming a configuration the store no longer holds, while `CreateConnector` refuses that reference both while the configuration is `DELETING` ("The worker configuration is not ACTIVE.") and once it is gone (`NotFoundException`). It is one rule, so one fix covers both delete handlers.

Nothing pins the current behaviour: the lifecycle test deletes the CONNECTOR first and only then the plugin, so the cross-resource order is untested.

## Evidence

Side 1 — the invariant the crate enforces on the create path.

`src/ops/create_connector.rs:316-331`:
```
            match state.custom_plugin_in(&region, &reference.custom_plugin_arn) {
                Some(stored) if stored.state != crate::state::CUSTOM_PLUGIN_ACTIVE => {
                    // A plugin the service has not finished taking on (or is
                    // deleting) cannot back a connector: its content is not
                    // usable yet, so the request is refused rather than stored
                    // against a plugin that may never become usable.
                    return Err(CreateConnectorErrorModeled::BadRequestException(
```
`src/ops/create_connector.rs:342-350`:
```
                None => {
                    return Err(CreateConnectorErrorModeled::NotFoundException(NotFoundException {
                        message: Some(crate::state::invalid_parameter_because(
                            "customPluginArn",
                            crate::state::RESOURCE_DOES_NOT_EXIST,
```
`src/ops/create_connector.rs:354-358` states the reason in full:
```
        // A connector's `workerConfiguration` names a worker configuration of
        // this account by ARN and revision. A reference the service never
        // minted names no configuration, so it cannot back a connector: the
        // create is refused rather than stored as a dangling reference the read
        // operations would then echo back.
```

Side 2 — the delete path that creates that state. `src/ops/delete_custom_plugin.rs:189-204` (the whole body after the ARN check; no `connectors` lookup anywhere in the file):
```
        let Some(plugin) = state.custom_plugin_in_mut(&region, &input.custom_plugin_arn) else {
...
        if plugin.state != crate::state::CUSTOM_PLUGIN_DELETING {
            plugin.state = crate::state::CUSTOM_PLUGIN_DELETING.to_string();
            plugin.pending = Some(crate::state::PendingChange::in_a_moment(
                crate::state::PendingKind::Remove,
            ));
        }
```
and the removal, `src/state.rs:1396-1397`:
```
        self.custom_plugins
            .retain(|_, plugin| !removal_due(plugin.pending, now));
```

What the reads then answer — `src/ops/describe_connector.rs:242`:
```
            plugins: Some(connector.plugin_descriptions()),
```
backed by `src/state.rs:882-890`, which echoes the stored ARN unconditionally:
```
    pub fn plugin_descriptions(&self) -> Vec<PluginDescription> {
        self.plugins
            .iter()
            .map(|plugin| PluginDescription {
                custom_plugin: Some(CustomPluginDescription {
                    custom_plugin_arn: Some(plugin.custom_plugin.custom_plugin_arn.clone()),
```

The twin — `src/ops/delete_worker_configuration.rs:212-217`:
```
        if worker.state != crate::state::WORKER_CONFIGURATION_DELETING {
            worker.state = crate::state::WORKER_CONFIGURATION_DELETING.to_string();
            worker.pending = Some(crate::state::PendingChange::in_a_moment(
                crate::state::PendingKind::Remove,
            ));
        }
```
against `src/ops/create_connector.rs:377-388` ("The worker configuration is not ACTIVE.") and `src/ops/describe_connector.rs:244` (`worker_configuration: connector.worker_configuration_description(),`).

Test coverage: `tests/all/custom_plugin_and_connector_lifecycle.rs:1222-1257` deletes the CONNECTOR first ("Once the clock has dropped it, it is gone from both reads") and only then runs "The same two steps for the plugin", so the plugin-before-connector order is never exercised; `tests/all/connector_capacity_and_worker_configuration.rs:541,580` pins only the CREATE-side refusal of a deleting/removed reference ("between the delete and the removal the clock owes, the reference …", "Once the clock has dropped it, the same reference names nothing at all"). Nothing pins the state this issue describes.

## Steps to Reproduce

1. POST /v1/custom-plugins with a readable s3Location and name "p1"; note customPluginArn. Wait past crate::state::TRANSITION_SPAN so the plugin is ACTIVE.
2. POST /v1/connectors referencing that customPluginArn at revision 1; note connectorArn. Wait past TRANSITION_SPAN so the connector is RUNNING.
3. DELETE /v1/custom-plugins/<customPluginArn> — answers 200 with customPluginState DELETING, with no regard for the connector that is running on it.
4. Wait past TRANSITION_SPAN, then GET /v1/custom-plugins/<customPluginArn> — 404 NotFoundException: the plugin record is gone.
5. GET /v1/connectors/<connectorArn> — 200, and plugins[0].customPlugin.customPluginArn is still the deleted ARN, i.e. a reference the reads echo back that resolves to nothing.
6. Contrast: POST /v1/connectors with that same plugins entry is refused (404 NotFoundException, 'Invalid parameter customPluginArn: The requested resource doesn’t exist.'), so the store now holds a connector the create path explicitly refuses to store.
7. Repeat steps 1-5 with a worker configuration (POST /v1/worker-configurations, reference it from CreateConnector, DELETE /v1/worker-configurations/<arn>) to see the identical outcome for DescribeConnector$workerConfiguration.
