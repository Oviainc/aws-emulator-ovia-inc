Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 52e42c3071caaf4a5ac680f43132e302f2bbfe9a

# GetEndpoint refuses a request whose only fault is an authenticationMethod the model documents as independent of the device members

_Filed: 2026-10-29T00:31:14Z_
_Operation: GetEndpoint | Class: validation_

> _Note: this issue was identified against the code base at commit `52e42c3071caaf4a5ac680f43132e302f2bbfe9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler invents a coupling between `authenticationMethod` and which device-identity member the request carries, and refuses the mismatch with a 400 `ValidationException`:

* `authenticationMethod=SignatureVersion4` + any device named WITHOUT `deviceRoleArn` -> 400;
* `authenticationMethod=X509ClientCertificate` + a device named by `deviceRoleArn` alone -> 400.

The Smithy model states no such rule. Each of `thingArn`, `certificateArn` and `deviceRoleArn` is documented as "This is an optional parameter", and `authenticationMethod` is documented only as "The authentication method used during the device connection" — nothing makes one member required because another member carries a particular value. So `GET /endpoint?thingArn=<iot thing arn>` is answered with a host (200), and the SAME request with the optional `authenticationMethod=SignatureVersion4` query added is refused (400): adding an optional parameter the model documents as independent turns a legal request into a client error.

The crate also contradicts itself here. The authentication method is not part of the device identity in this crate at all — `crate::state::endpoint_for(region, &DeviceIdentity)` takes only the region and the three ARNs, and the crate's own test states "the authentication method says how the device connects, not which device it is" and asserts two rows with the same ARNs but different methods get the SAME host. A member that provably cannot change the answer is nevertheless the sole reason the request is answered with an error instead of that host.

## Evidence

The model, `smithy_query(query="shape",shape="GetEndpointRequest")` — every identity member is optional and `authenticationMethod` is coupled to none of them:
```
"thingArn": {... "doc": "<p>The thing ARN of the device. This is an optional parameter.</p>"},
"certificateArn": {... "doc": "<p>The certificate ARN of the device. This is an optional parameter.</p>"},
"deviceRoleArn": {... "doc": "<p>The device role ARN of the device. This is an optional parameter.</p>"},
"authenticationMethod": {... "doc": "<p>The authentication method used during the device connection.</p>"}
```
The handler that makes `deviceRoleArn` (resp. `thingArn`/`certificateArn`) conditionally REQUIRED — services/service-com-amazonaws-iotdeviceadvisor-iotsenateservice/src/ops/get_endpoint.rs:204-222:
```
        if !device.names_no_device() {
            match input.authentication_method.as_deref() {
                Some(SIGNATURE_VERSION_4) if device.device_role_arn.is_none() => {
                    return Err(validation(format!(
                        "authenticationMethod {SIGNATURE_VERSION_4} identifies the device by \
                         deviceRoleArn, which this request does not carry"
                    )));
                }
                Some(X509_CLIENT_CERTIFICATE)
                    if device.thing_arn.is_none() && device.certificate_arn.is_none() =>
                {
                    return Err(validation(format!(
                        "authenticationMethod {X509_CLIENT_CERTIFICATE} identifies the device by \
                         thingArn or certificateArn, neither of which this request carries"
                    )));
                }
```
The method is NOT part of the identity the answer is computed from — src/state.rs:1288-1294:
```
pub fn endpoint_for(region: &str, device: &DeviceIdentity<'_>) -> String {
    if device.names_no_device() {
        endpoint_for_region(region)
    } else {
        endpoint_for_device(region, device)
    }
}
```
and `DeviceIdentity` carries only the three ARNs (src/state.rs:1027-1032). The crate states that itself — tests/all/endpoint.rs:435-438:
```
    /// The DEVICE IDENTITY the row names — the ARNs it carries, and nothing else. Two
    /// rows with the same identity must get the same host: the authentication method says
    /// how the device connects, not which device it is.
```
Test coverage: the refusal is pinned by tests/all/endpoint.rs:642-655 (rows "SigV4 with no device role" and "X.509 with only a device role") whose only stated reason is the crate's own invented rule — endpoint.rs:604-606: "An ARN that is not one for the member carrying it, and an authentication method with no identity it can use, are refused". The accepting side is pinned at endpoint.rs:451-486 (rows "thing only", "the same thing with X.509", "the same role with no method named"), which is what makes the divergence visible: thingArn alone and thingArn+X509 are 200s, thingArn+SigV4 is a 400.

## Steps to Reproduce

1. Boot the emulator and call `GET /endpoint?thingArn=arn%3Aaws%3Aiot%3Aus-east-1%3A000000000000%3Athing%2FT` signed for us-east-1. Observed: HTTP 200 with that device's host.
2. Re-send the identical request with the optional `authenticationMethod=SignatureVersion4` query added: `GET /endpoint?thingArn=arn%3Aaws%3Aiot%3Aus-east-1%3A000000000000%3Athing%2FT&authenticationMethod=SignatureVersion4`. Observed: HTTP 400 ValidationException "authenticationMethod SignatureVersion4 identifies the device by deviceRoleArn, which this request does not carry"; expected per the model (deviceRoleArn "is an optional parameter", and the host derivation in state.rs:1288 ignores the method): the same 200 and the same host as step 1.
3. Call `GET /endpoint?deviceRoleArn=arn%3Aaws%3Aiam%3A%3A000000000000%3Arole%2FDeviceRole&authenticationMethod=X509ClientCertificate`. Observed: HTTP 400; drop the `authenticationMethod` query and the same request is answered 200 with a host — the optional member is the only difference.
