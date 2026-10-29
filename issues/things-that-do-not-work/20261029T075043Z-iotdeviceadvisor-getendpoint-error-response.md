Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 5f34c6e445f07b3b212c2f20790cdf7255a4bd24

# Refusal messages built from a request-supplied ARN exceed the modelled Message length (1..2048)

_Filed: 2026-10-29T07:50:43Z_
_Operation: GetEndpoint | Class: error-response_

> _Note: this issue was identified against the code base at commit `5f34c6e445f07b3b212c2f20790cdf7255a4bd24`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every client-input refusal in this crate interpolates the caller's ARN into the error `message` WHOLE, and the ARN members the model allows are themselves up to 2048 characters long, so the resulting `message` is longer than the maximum its own shape declares. The emulator therefore answers a modelled error whose body the model says cannot exist.

Three shared message builders in `state.rs` are affected, and a request of the model's maximum size reaches all of them:

* `arn_member_violation` (state.rs:1115-1122) — reached from `GetEndpoint` (`device_violation`, get_endpoint.rs:181-185), from `CreateSuiteDefinition`/`UpdateSuiteDefinition` (`definition_configuration_violation` -> `devicePermissionRoleArn` / `devices[i]`) and from `StartSuiteRun` (`run_configuration_violation` -> `primaryDevice`). Its text is ~106 fixed characters plus `{value:?}` (the 2048-character value, quoted, and `Debug` escaping can double it), so the message is ~2156 characters.
* `parse_resource_arn`'s `malformed()` (state.rs:157-161) — reached from `TagResource`, `UntagResource` and `ListTagsForResource`: ~74 fixed characters plus the quoted 2048-character `resourceArn`.
* `not_found_message` (state.rs:711-713) — reached from the tagging operations with a well-formed but unheld ARN: `"Unable to find resource with resourceArn: "` (41 characters) plus the 2048-character value.

Nothing clips, truncates or elides the request-supplied value on any of these paths, and the framework does not catch it: output/error values are serialized without any constraint check (only the INPUT is validated), so the over-long `message` goes out on the wire as a 400/404 body.

The crate already reasons about this value's modelled bound on the way IN (state.rs:1112-1114) and about the value being request-supplied (state.rs:155-156, which quotes it so it cannot forge a second line), but not about the bound on the way OUT.

## Evidence

The model's bound on the member every refusal fills — `smithy_query(query="shape",shape="Message,ValidationException,AmazonResourceName")`:
```
"Message": {"type": "string", "length": {"min": 1, "max": 2048}}
"ValidationException": {"errorKind": "client", "httpError": 400,
   "members": {"message": {"target": "Message", "length": {"min": 1, "max": 2048}}}}
"AmazonResourceName": {"type": "string", "length": {"min": 20, "max": 2048}}
```
(`ResourceNotFoundException$message` targets the same `Message` shape.) So a legal `thingArn`/`resourceArn` is already as long as a legal `message` may be in total.

The builder that appends the whole value to a fixed prefix — services/service-com-amazonaws-iotdeviceadvisor-iotsenateservice/src/state.rs:1115-1122:
```
pub fn arn_member_violation(member: &str, value: &str, service: &str) -> Option<String> {
    (!is_arn_of_service(value, service)).then(|| {
        format!(
            "{member} must be an Amazon Resource Name of the form \
             arn:<partition>:{service}:<region>:<account>:<resource>, got {value:?}"
        )
    })
}
```
The same file reasons only about the INPUT bound, never the message's — state.rs:1112-1114:
```
/// The modelled length bounds (`AmazonResourceName` is `len 20..2048`) are enforced before
/// a handler runs; the ARN FORM is not modelled, so it is checked here and refused with the
/// operation's own client-input error.
```
The handler that returns it — src/ops/get_endpoint.rs:181-185:
```
        if let Some(violation) =
            crate::state::device_violation("", &device, crate::state::DeviceNaming::Optional)
        {
            return Err(validation(violation));
        }
```
The other two builders, same shape — state.rs:157-161:
```
    let malformed = || {
        format!(
            "resourceArn {arn:?} is not an IoT Device Advisor suite definition or suite run ARN"
        )
    };
```
and state.rs:711-713:
```
pub fn not_found_message(member: &str, value: &str) -> String {
    format!("Unable to find resource with {member}: {value}")
}
```
Why the framework does not save it — vendor/smithy-server-runtime/src/operation.rs:412 validates the INPUT only, and the error path at :447 serializes with no constraint pass:
```
        let violations = aws_smithy_schema_server::constraints::validate(&input, O::INPUT_SCHEMA);
```
```
                let mut resp = protocol.serialize_error(
                    e.as_serializable(),
                    e.variant_schema(),
```
Test coverage: none of the refusal tests uses a maximum-length value. tests/all/device_identity_validation.rs:39 states the opposite is under test — "`len 20..2048`), so what is under test is the ARN form, not the length bound" — and tests/all/definition_configuration_arn_rules.rs:422 keeps every value at 19 characters ("`AmazonResourceName` is len 20..2048; each value here is 19 characters."). tests/all/endpoint.rs:673 likewise notes the bound without exercising it, so nothing pins the message length.

## Steps to Reproduce

1. Start the emulator with this service mounted.
2. Send `GET /endpoint?thingArn=<2048 'x' characters>` with an `Authorization` header carrying a SigV4 credential scope for `us-east-1` (the value satisfies `AmazonResourceName`'s `len 20..2048`, so input validation passes and the handler runs).
3. Observe the HTTP 400 `ValidationException`: its `message` is `thingArn must be an Amazon Resource Name of the form arn:<partition>:iot:<region>:<account>:<resource>, got "xxx…"` — about 2156 characters, i.e. longer than the 2048-character maximum `Message` declares for that member.
4. Repeat against the tagging path: `GET /tags/<a 2048-character value that is not an iotdeviceadvisor ARN>` returns a `ValidationException` whose message is the whole value plus ~74 characters; a well-formed 2048-character `arn:aws:iotdeviceadvisor:us-east-1:123456789012:suitedefinition/<long id>` the store does not hold returns a `ResourceNotFoundException` whose message is the value plus 41 characters.
5. Repeat against `POST /suiteDefinitions` with `suiteDefinitionConfiguration.devicePermissionRoleArn` set to a 2048-character non-IAM value, and `POST /suiteDefinitions/{id}/suiteRuns` with a 2048-character `suiteRunConfiguration.primaryDevice.thingArn`: both produce the same over-long message.
