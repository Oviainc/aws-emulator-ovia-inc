Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: dc6b9d697210888fba6fd46cd7393ea22255dcd4

# Stopped product subscription vanishes instantly instead of staying observable with Status UNSUBSCRIBED

_Filed: 2026-10-24T12:48:28Z_
_Operation: StopProductSubscription | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `dc6b9d697210888fba6fd46cd7393ea22255dcd4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

StopProductSubscription deletes the product-user record from state synchronously and only decorates the in-memory copy it returns with Status=UNSUBSCRIBED/SubscriptionEndDate. The UNSUBSCRIBED state therefore exists ONLY inside that one reply: the very next ListProductSubscriptions for the same identity provider/product reports nothing for that user, and a second StopProductSubscription (or any read addressed by ProductUserArn) answers ResourceNotFoundException.

Expected: ending a user-based subscription is a teardown the service reports while it runs, exactly as this emulator already reports every other lifecycle of this service (REGISTERING->REGISTERED, DEREGISTERING, ASSOCIATING->ASSOCIATED, DISASSOCIATING, PROVISIONING->PROVISIONED->DELETING->DELETED). The product user should remain reported by ListProductSubscriptions carrying the ended status and SubscriptionEndDate until the teardown the clock owes completes, so that (a) a client polling the list can observe the subscription ending instead of it disappearing between two reads, and (b) the documented `Status` filter of ListProductSubscriptions can select on anything other than SUBSCRIBED — today SUBSCRIBED is the only status any listed product user can ever carry.

Actual: the record, its indexes and its tags are all removed inside the stop call, so the ended subscription is unobservable and the Status filter is effectively a constant.

This is the same divergence class that was already filed and fixed for the three sibling teardowns of this service (DeregisterIdentityProvider "vanishes instantly instead of being observable in DEREGISTERING", DisassociateUser "...in DISASSOCIATING", DeleteLicenseServerEndpoint "...in DELETING and then DELETED"); the product-subscription teardown is the one that still deletes synchronously.

## Evidence

Spec: `ProductUserSummary` declares `Status` (@required, "The status of a product for this user"), `SubscriptionEndDate` ("The end date of a subscription") and `ListProductSubscriptions.Filters` documents `Status` as a filterable attribute (smithy_query shape=ProductUserSummary / ListProductSubscriptionsRequest) — a Status filter and an end date are only meaningful if a list can report a subscription that is no longer SUBSCRIBED. Handler: `src/ops/stop_product_subscription.rs` lines ~191-201 call `state.remove_product_user(&arn)` (which in `src/state.rs` removes the record from `product_users`, from `product_user_order`, from `product_user_index` and drops its `tags`) and then mutate the returned, now-detached clone: `ended.status = STATUS_UNSUBSCRIBED; ended.subscription_end_date = Some(iso8601(now))`. Nothing schedules a transition (`schedule_transition` is never called here) and `src/state.rs` has no UNSUBSCRIBE span constant beside REGISTRATION_SPAN / ASSOCIATION_SPAN / DISASSOCIATION_SPAN / ENDPOINT_DELETION_SPAN. Tests: `tests/all/identity_provider_subscriptions.rs::product_subscriptions_round_trip_and_stop_removes_them` asserts the CURRENT instant-removal behaviour and no test asserts an UNSUBSCRIBED entry is ever listed.

## Steps to Reproduce

1. RegisterIdentityProvider for an Active Directory provider with Product=VISUAL_STUDIO_ENTERPRISE.
2. StartProductSubscription with Username=alice through that provider/product — note the ProductUserArn and Status=SUBSCRIBED.
3. StopProductSubscription by that ProductUserArn — the reply carries Status=UNSUBSCRIBED and a SubscriptionEndDate.
4. Immediately call ListProductSubscriptions for the same IdentityProvider and Product: alice is absent. Expected: alice still reported with the ended status and SubscriptionEndDate until the teardown completes, then dropped.
5. Also call ListProductSubscriptions with Filters=[{Attribute:Status, Operation:eq, Value:UNSUBSCRIBED}]: always empty, because no listed product user can ever carry a status other than SUBSCRIBED.
