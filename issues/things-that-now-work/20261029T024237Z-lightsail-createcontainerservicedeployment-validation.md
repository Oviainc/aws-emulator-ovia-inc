Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: cd4f62d4622d99c848b6778d8f7c3e7dcd5cf5b1

# A public endpoint's healthCheck.successCodes is stored verbatim, skipping the value domain its own modelled documentation states and the two sibling members of the same structure are checked against

_Filed: 2026-10-29T02:42:37Z_
_Operation: CreateContainerServiceDeployment | Class: validation_

> _Note: this issue was identified against the code base at commit `cd4f62d4622d99c848b6778d8f7c3e7dcd5cf5b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ContainerServiceHealthCheckConfig$successCodes` documents its value domain: "The HTTP codes to use when checking for a successful response from a container. You can specify values between 200 and 499. You can specify multiple values (for example, 200,202) or a range of values (for example, 200-299)."

`crate::state::health_check_from_request` — the one place this crate builds a health check from a request, called by both `CreateContainerServiceDeployment` and `CreateContainerService` — copies the member straight out of the request with no check whatsoever, while in the same function the two sibling members that carry a documented domain (`timeoutSeconds`, `intervalSeconds`) are each refused when they fall outside it. So the function enforces its members' documented domains selectively: a deployment can be created whose health check declares `successCodes: "banana"`, `"600"`, `""`, `"199-999"` or `"300-200"`, and `GetContainerServices` / `GetContainerServiceDeployments` report that value back as the configuration of the service's public endpoint.

The handler's own stated contract for this path is that every request-only rule the model cannot express is answered before the first write (`deployment_from_request`'s doc: "A public endpoint must name one of those containers, on a port that container opens, and its health check is filled in with the documented defaults for whatever the request left out") — and the container-name and container-port halves of exactly that sentence ARE enforced a few lines above. The success codes are the one documented part of the health check left unchecked.

## Evidence

The modelled value domain — smithy_query(query="doc", shape="ContainerServiceHealthCheckConfig", member="successCodes"):
```
The HTTP codes to use when checking for a successful response from a container. You can
specify values between 200 and 499. You can specify multiple values
(for example, 200,202) or a range of values (for example, 200-299).
```

The member stored verbatim — services/service-com-amazonaws-lightsail-lightsail-20161128/src/state.rs:9928-9932:
```
        success_codes: Some(
            request
                .and_then(|config| config.success_codes.clone())
                .unwrap_or_else(|| HEALTH_CHECK_DEFAULT_SUCCESS_CODES.to_string()),
        ),
```

The two sibling members of the SAME structure, checked in the SAME function — src/state.rs:9890-9909:
```
    let timeout_seconds = request
        .and_then(|config| config.timeout_seconds)
        .unwrap_or(HEALTH_CHECK_DEFAULT_TIMEOUT_SECONDS);
    let (timeout_min, timeout_max) = HEALTH_CHECK_TIMEOUT_SECONDS_RANGE;
    if !(timeout_min..=timeout_max).contains(&timeout_seconds) {
        return Err(format!(
            "deployment.publicEndpoint.healthCheck.timeoutSeconds must be between \
             {timeout_min} and {timeout_max}, got {timeout_seconds}"
        ));
    }
    let interval_seconds = request
        .and_then(|config| config.interval_seconds)
        .unwrap_or(HEALTH_CHECK_DEFAULT_INTERVAL_SECONDS);
    let (interval_min, interval_max) = HEALTH_CHECK_INTERVAL_SECONDS_RANGE;
    if !(interval_min..=interval_max).contains(&interval_seconds) {
        return Err(format!(
            "deployment.publicEndpoint.healthCheck.intervalSeconds must be between \
             {interval_min} and {interval_max}, got {interval_seconds}"
        ));
    }
```

The function's stated contract and the request-only rules it DOES enforce for the same public endpoint — src/state.rs:9799-9805 and 9841-9863:
```
/// A deployment launches containers, so it must name at least one, and each must
/// name the image it runs. A public endpoint must name one of those containers,
/// on a port that container opens, and its health check is filled in with the
/// documented defaults for whatever the request left out.
...
            let container = containers.get(&endpoint.container_name).ok_or_else(|| {
                format!(
                    "deployment.publicEndpoint.containerName '{}' names no container of this \
                     deployment, which describes [{}]",
...
            if !open_ports.contains_key(&endpoint.container_port.to_string()) {
```

The handler that calls it — src/ops/create_container_service_deployment.rs:206-207:
```
            let deployment = crate::state::deployment_from_request(version, now, &requested)
                .map_err(invalid)?;
```
(`CreateContainerService` reaches the same function at src/ops/create_container_service.rs:244.)

Test coverage: only well-formed values are exercised — tests/all/container_service_lifecycle.rs:357 (`success_codes: Some("200-299")`) and :400 (`success_codes: None`), asserted back at :823-825; tests/all/container_service_deployment.rs:459-461 asserts the documented default. No test sends a value outside 200..=499, so nothing pins the current acceptance.

## Steps to Reproduce

1. Call CreateContainerService for "svc" with power="nano" and scale=1, and wait for it to become READY.
2. Call CreateContainerServiceDeployment with serviceName="svc", containers={"app":{"image":"nginx","ports":{"80":"HTTP"}}} and publicEndpoint={"containerName":"app","containerPort":80,"healthCheck":{"successCodes":"banana"}}.
3. Observe HTTP 200 — the deployment is created and stored.
4. Call GetContainerServices (or GetContainerServiceDeployments) for "svc": the public endpoint's healthCheck.successCodes is reported as "banana", a value the member's own documentation excludes ("You can specify values between 200 and 499").
5. Repeat with successCodes="600" or "" — both accepted. For contrast, send healthCheck={"timeoutSeconds":9999}: refused with InvalidInputException from the same function (src/state.rs:9893-9899).
