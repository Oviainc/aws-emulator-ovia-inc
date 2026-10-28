Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 21d30b53374928fd663edcc8ae486344ab510a4e

# A Java CodeLocation with no ClassName is accepted and stored, although the member documents it as required for Java

_Filed: 2026-10-28T04:30:24Z_
_Operation: CreateInstrumentationConfiguration | Class: validation_

> _Note: this issue was identified against the code base at commit `21d30b53374928fd663edcc8ae486344ab510a4e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CodeLocation.ClassName` is modelled optional, but its member doc states the conditional rule the model cannot express: "This is required for Java and optional for Python module-level functions." `CreateInstrumentationConfiguration` never checks it: a `Location` of `{"CodeLocation":{"Language":"Java","FilePath":"..."}}` passes the boundary validator (nothing is `@required` there) and the handler goes straight on to hash the location and store the record, answering 200 with an `ARN` and a `LocationHash` for an instrumentation point that names no class to find the method in. Nothing in the crate reads `ClassName` other than the hash (`state.rs:4096`), so there is no check anywhere.

This is exactly the class of rule this handler already enforces for its OTHER conditional member: five lines before the location is hashed, `requested_expiry` refuses an `ExpiresAt` that the instrumentation type does not support, with a comment saying the model cannot express "this member is only supported for one value of that one" so the documented rule "[is] enforced here and refused with 400 rather than silently coerced". The same crate also enforces the inexpressible `SliConfig`/`RequestBasedSliConfig` exclusion, the `StartTime`/`EndTime` pairing of `GetInstrumentationConfigurationStatus`, and `GroupingName` uniqueness in `PutGroupingConfiguration` — so the machinery and the position are both already there; the Java-ClassName rule is the one documented conditional requirement of this operation that is skipped.

The same omission reaches `GetInstrumentationConfiguration`, `DeleteInstrumentationConfiguration` and `GetInstrumentationConfigurationStatus` through `LocationIdentifier::CodeLocation`, which is hashed by the same `crate::state::location_hash`; the create is where the invalid location is first accepted and persisted.

## Evidence

MODEL — `smithy_query(query="doc", shape="CodeLocation", member="ClassName")`:
    "The class or type name that contains the method. This is required for Java and optional for Python module-level functions."
and `smithy_query(query="shape", shape="ProgrammingLanguage")` -> `["Java", "Python", "Javascript"]`, so `Language` names the value the rule keys on.

HANDLER — no check on the pair; the location goes straight to the hash and the store:
`services/service-com-amazonaws-applicationsignals-applicationsignals/src/ops/create_instrumentation_configuration.rs:233`
    `let location_hash = crate::state::location_hash(&input.location);`
`...:284`  `location: input.location.clone(),`
The only other reader of the member is the hash itself — `src/state.rs:4096`: `code.class_name.as_deref(),` (a grep for `ClassName|class_name` over `src/` returns only that line and the field declaration in `src/ops/types/shared_types.rs:219`).

SAME HANDLER, the conditional-member rule it DOES enforce — `create_instrumentation_configuration.rs:217-226`:
    `// The model cannot express "this member is only supported for one value of that`
    `// one", nor a window measured from the time of the request, so both parts of the`
    `// documented ExpiresAt rule are enforced here and refused with 400 rather than`
    `// silently coerced.`
    `let expires_at = crate::state::requested_expiry(`
    `    &input.instrumentation_type, input.expires_at.as_ref(), &now,`
    `).map_err(crate::scaffolded::Scaffolded::validation)?;`
and the sibling rule in `src/state.rs:1099-1101`: `/// Documented on both members — "You can't specify both RequestBasedSliConfig and SliConfig /// in the same operation" — and inexpressible in the model, so the boundary validator admits /// the pair and every operation that accepts it refuses here, through one wording.`

TEST COVERAGE that currently assumes the opposite, and whose Java rows would have to carry a `ClassName` — `tests/all/error_identity.rs:202-212`:
    `/// Every member but `Service` is valid, so the only constraint a row can violate is the one`
    `/// it sets out to violate.`
    ...
    `"CodeLocation": { "Language": "Java", "FilePath": "src/main/java/App.java" }`
(that row still passes after a fix: its `Service` violates a modelled `@pattern`, which the boundary validator refuses before the handler runs). The rows that would need a `ClassName` added are `tests/all/instrumentation_configuration_identity.rs:211-220`, `tests/all/instrumentation_configuration_tags.rs:416-420`, and the Java rows of `tests/all/get_instrumentation_configuration.rs` / `tests/all/instrumentation_configurations.rs` that omit it (both suites label this dimension "WHICH of `CodeLocation`'s optional members the location supplies", e.g. `tests/all/instrumentation_configurations.rs:192-193`).

## Steps to Reproduce

1. Start the emulator and point an `application-signals` client at it.
2. `POST /create-instrumentation-configuration` with `{"InstrumentationType":"BREAKPOINT","Service":"checkout","Environment":"prod","SignalType":"SNAPSHOT","Location":{"CodeLocation":{"Language":"Java","FilePath":"src/main/java/App.java"}},"CaptureConfiguration":{"CodeCapture":{"CaptureLimits":{}}}}` — a Java code location with no `ClassName`.
3. Expected: HTTP 400 `ValidationException` naming the member and the requirement, as the handler already answers for an `ExpiresAt` the instrumentation type does not support; actual: HTTP 200 with a minted `ARN` and `LocationHash`, and the record persisted.
4. `POST /get-instrumentation-configuration` for the same identity: the stored configuration comes back, so the invalid location is now readable and must be deleted explicitly.
5. Send the same body with `"Language":"Python"` and no `ClassName`: identical 200 — the emulator makes no distinction, although the member doc draws one between Java and Python.
