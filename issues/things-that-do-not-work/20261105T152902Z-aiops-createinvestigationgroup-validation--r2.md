Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 24dde0726d05602291869d9bf6349d012f3eed57

# A chatbotNotificationChannel entry whose ARN list is empty (or holds more than 5) is accepted and stored, though nothing applies the modelled ChatConfigurationArns 1..5

_Filed: 2026-11-05T15:29:02Z_
_Operation: CreateInvestigationGroup | Class: validation_

> _Note: this issue was identified against the code base at commit `24dde0726d05602291869d9bf6349d012f3eed57`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`chatbotNotificationChannel` is modelled as `map<SNSTopicArn, ChatConfigurationArns>`, and the VALUE shape `ChatConfigurationArns` carries `@length min 1, max 5`. Nothing in the request path applies that bound:

* the centralized constraint walk skips it, because the value list is a NESTED collection (a list written inside a map frame) and the framework explicitly does not enforce a nested list's own `@length`;
* the handler's own chain only bounds the NUMBER OF ENTRIES of the map (`crate::state::collection_bounds_rejection`, against the emulator's `MAX_COLLECTION_ENTRIES`), never the size of any entry's list;
* `crate::state::map_key_rejection`, the crate's existing mechanism for exactly the class of modelled constraint the walk cannot reach, re-declares the map's KEY traits only.

So `CreateInvestigationGroup` (and `UpdateInvestigationGroup`, which runs the same chain) answers 201/200 for `chatbotNotificationChannel: {"<sns-topic-arn>": []}` and for an entry carrying 6 or more configuration ARNs, persists it, and `GetInvestigationGroup` then reports a map entry the model's own 1..5 bound forbids — the entry's list is also unbounded, so an entry can hold an arbitrary number of caller-supplied ARNs.

This contradicts the crate's own stated invariant in `state.rs`, which asserts that this very bound is enforced by the walk and that "no caller-sized member reaches the store unchecked". The ELEMENTS of the same list ARE validated (the crate's tests pin that), so the gap is specifically the list's own entry count: minimum 1 and maximum 5.

## Evidence

Smithy model (`smithy_query(query="shape",shape="ChatbotNotificationChannel,ChatConfigurationArns")`):
```
"ChatbotNotificationChannel": {"type": "map", "key": {"target": "SNSTopicArn", ...}, "value": {"target": "ChatConfigurationArns", "length": {"min": 1, "max": 5}, "type": "list"}}
"ChatConfigurationArns": {"type": "list", "length": {"min": 1, "max": 5}, "member": {"target": "ChatConfigurationArn", "length": {"min": 20, "max": 2048}, "pattern": "^arn:.*$"}}
```
(the map shape itself carries NO `@length`, so no outer bound lands on the inner list either).

The crate's claim — services/service-com-amazonaws-aiops-aiops/src/state.rs:96-103:
```
// Every remaining caller-sized member is bounded by the model: `crossAccountConfigurations` (0..25),
// each channel's `ChatConfigurationArns` list (1..5) and its SNS topic key (20..2048), each tag key
// (1..128) and value (1..256), ... The centralized
// walk refuses every one of those BUT a map's KEY, which it never reaches; those are
// applied by [`map_key_rejection`] instead, so no caller-sized member reaches the
// store unchecked.
```

The framework says otherwise — vendor/smithy-schema-server/src/constraints.rs:1280-1285:
```
/// A nested list's *own* `@length` therefore goes unenforced: enforcing it
/// needs a per-level element schema to carry it, which codegen does not emit.
/// That is the safe direction — the bound is checked one level up or not at
/// all, rather than against the wrong value — but it is not free.
```
and the code that skips it, constraints.rs:1308-1310:
```
            if !is_collection_element {
                this.check_collection_length(schema, index, &format!("{index} items"));
            }
```
with a list written inside a map frame arriving as `is_collection_element = true` — constraints.rs:1040-1051:
```
    fn write_list(&mut self, schema: &Schema, write_elements: ...) -> Result<(), SerdeError> {
        ...
        self.in_collection_child(schema, |this, sch, elem| {
            this.walk_list(sch, elem, write_elements)
        })
```

The handler chain that does not cover it — services/service-com-amazonaws-aiops-aiops/src/ops/create_investigation_group.rs:257-267:
```
        if let Some(why) = crate::state::write_configuration_rejection(
            input.encryption_configuration.as_ref(),
            input.cross_account_configurations.as_deref(),
            input.tag_key_boundaries.as_deref(),
            input.chatbot_notification_channel.as_ref(),
        ) {
```
and what that chain checks for this member — state.rs:1090-1096 (`collection_bounds_rejection`):
```
        too_many(
            "chatbotNotificationChannel",
            chatbot_notification_channel.map_or(0, std::collections::HashMap::len),
            MAX_COLLECTION_ENTRIES,
        )
```
— the map's entry COUNT only; no entry's list is ever measured. The value is then stored as given — create_investigation_group.rs:366-370:
```
            chatbot_notification_channel: input
                .chatbot_notification_channel
                .unwrap_or_default()
                .into_iter()
                .collect(),
```
and the same on the update path — update_investigation_group.rs:282-285:
```
        if let Some(chatbot_notification_channel) = input.chatbot_notification_channel {
            group.chatbot_notification_channel =
                chatbot_notification_channel.into_iter().collect();
        }
```

Test coverage shows the gap is untested while the neighbouring rules are pinned: tests/all/map_member_refusal_paths.rs:48-55 and :284-303 pin that an ELEMENT of the entry's list is refused through the walk (`CHAT_ARN_REQUIREMENTS`, "from the scalar shape its value list holds"), and tests/all/investigation_groups.rs:889-904 + :953-974 pin only the map's ENTRY COUNT (`chatbot_channels(MAX_COLLECTION_ENTRIES + 1)`), with every row building entries of exactly one ARN (`json!([format!("arn:aws:chatbot::{ACCOUNT}:chat-configuration/slack-channel/c{n}")])`). No test sends an entry with 0 or with more than 5 ARNs.

## Steps to Reproduce

1. POST /investigationGroups signed for us-west-2 with body {"name":"channels","roleArn":"arn:aws:iam::000000000000:role/AIOpsAssistantRole","chatbotNotificationChannel":{"arn:aws:sns:us-west-2:000000000000:my-topic":[]}} — the emulator answers 201 although the modelled `ChatConfigurationArns` requires at least 1 entry
2. GET /investigationGroups/<url-encoded ARN> — the response reports `chatbotNotificationChannel` with that entry's list empty, a value the model's 1..5 bound forbids
3. Repeat the create (in another Region) with six ARNs under one SNS topic key, e.g. {"arn:aws:sns:us-west-2:000000000000:my-topic":["arn:aws:chatbot::000000000000:chat-configuration/slack-channel/c1", ... "c6"]} — the emulator answers 201 and GET reports all six, past the modelled maximum of 5
4. PATCH /investigationGroups/<url-encoded ARN> with the same oversized/empty entry — accepted too, since update_investigation_group.rs runs the same `write_configuration_rejection` chain
5. Compare with services/.../src/state.rs:96-103, which states that each channel's `ChatConfigurationArns` list (1..5) is refused by the centralized walk, and with vendor/smithy-schema-server/src/constraints.rs:1280-1285, which states a nested list's own `@length` goes unenforced
