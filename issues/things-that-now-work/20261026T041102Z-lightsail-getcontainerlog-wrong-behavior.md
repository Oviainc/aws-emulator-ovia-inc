Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: b314e0f3433a6118e8049d188afadfe9e3c07a46

# GetContainerLog refuses a container of a superseded deployment, on the strength of a comment the crate now contradicts

_Filed: 2026-10-26T04:11:02Z_
_Operation: GetContainerLog | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `b314e0f3433a6118e8049d188afadfe9e3c07a46`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetContainerLog` decides whether a container name is one it will answer about by looking ONLY at the container service's CURRENT deployment (`has_run_container`). A container declared by a deployment that has since been superseded by a later `CreateContainerServiceDeployment` call therefore earns a `DoesNotExist` refusal, even though it "previously ran" on the service — which is exactly the case the modelled `containerName` member admits.

The handler itself explains the narrowing by asserting that nothing can give a service a second deployment because `CreateContainerServiceDeployment` is a 501 scaffold. That premise is no longer true: `CreateContainerServiceDeployment` is implemented in this same crate and installs a `next_deployment` which the clock then promotes, dropping the previous one. So the condition the comment attached its own correctness to has lapsed, and the refusal now fires for container names the model documents as valid.

The mechanism of the loss is in the crate's own settle step: when the clock activates the waiting deployment, the previous one is marked `INACTIVE` and then immediately discarded, so no record of the containers that ran before survives for this handler (or for anything else) to read.

## Evidence

Model (smithy_query doc, GetContainerLogRequest$containerName): "The name of the container that is either running or previously ran on the container\n      service for which to return a log."

Handler narrowing — services/service-com-amazonaws-lightsail-lightsail-20161128/src/ops/get_container_log.rs:345-350:
```
fn has_run_container(container: &str, service: &crate::state::StoredContainerService) -> bool {
    service
        .current_deployment
        .as_ref()
        .is_some_and(|deployment| deployment.containers.contains_key(container))
}
```
and the refusal it drives, get_container_log.rs:260-266:
```
            if !has_run_container(&input.container_name, service) {
                return Err(container_does_not_exist(
```

The stale premise — get_container_log.rs:255-259:
```
            // (Nothing can give a service a second deployment while
            // `CreateContainerServiceDeployment` is a scaffold; when that lands
            // and the store retains the deployments that ran before — what
            // `GetContainerServiceDeployments` reports — this set widens to the
            // containers of every deployment that was active, in that change.)
```
But that operation is implemented — services/service-com-amazonaws-lightsail-lightsail-20161128/src/ops/create_container_service_deployment.rs:213-214:
```
            service.next_deployment = Some(deployment);
            service.next_deployment_version = version.saturating_add(1);
```
and the superseded deployment is dropped when the clock promotes the waiting one — services/service-com-amazonaws-lightsail-lightsail-20161128/src/state.rs:8198-8205:
```
            if let Some(mut next) = self.next_deployment.take() {
                next.state = DEPLOYMENT_STATE_ACTIVE;
                // "The container service can have only one deployment in an
                // active state at a time": whatever was running steps down.
                if let Some(mut previous) = self.current_deployment.replace(next) {
                    previous.state = DEPLOYMENT_STATE_INACTIVE;
                }
            }
```
(`previous` is given the INACTIVE state and then dropped, so the record is lost.)

Test coverage: tests/all/container_service_log.rs covers a service with no deployment and a deployment still waiting on the clock (`a_container_of_a_deployment_that_has_not_activated_yet_is_refused`, `a_service_with_no_deployment_has_no_container_to_report`); no test exercises a container of a SUPERSEDED deployment, so nothing pins the current refusal.

## Steps to Reproduce

1. CreateContainerService with a deployment declaring container "first-app" (power/scale as the test suite does), and wait until GetContainerServices reports currentDeployment with that container.
2. Call GetContainerLog with serviceName=<service>, containerName="first-app" — it succeeds (empty logEvents).
3. Call CreateContainerServiceDeployment on the same service with a deployment declaring a different container, e.g. "second-app", and wait until GetContainerServices reports currentDeployment carrying "second-app".
4. Call GetContainerLog again with containerName="first-app" — the handler answers the DoesNotExist envelope ("No container named \"first-app\" is running, or previously ran, on the container service ..."), although the model documents containerName as a container "that is either running or previously ran on the container service".
