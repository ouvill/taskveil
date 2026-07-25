# Billing Overview

This document summarizes Taskveil's public billing principles. Detailed pricing experiments, revenue scenarios, channel comparisons, and operational notes are maintained outside the public repository.

## Principles

- Local-first task management remains available without an account or subscription.
- Paid plans are intended for server-dependent capabilities such as encrypted multi-device sync, encrypted cloud backup, and organization sharing.
- Losing paid status must not make local data unreadable or uneditable.
- Billing state is separated from end-to-end encrypted content. The server may know whether an account has access to a paid capability, but it must not learn task contents.
- Upsell prompts should appear only in relevant product contexts and must not rely on disruptive advertising or dark patterns.
- Product analytics for billing should be minimized and should not undermine Taskveil's privacy promises.

## Public Plan Shape

Taskveil's planned plan structure is:

| Plan | Intended audience | Public boundary |
|---|---|---|
| Free | Single-device local use | Local task management, local storage, local export/backup flows, and privacy-preserving core features |
| Pro | Individuals who want server-backed convenience | Encrypted sync and server-backed backup for personal use |
| Org | Teams and organizations | Shared encrypted workspaces, member management, and organization-oriented controls |

Final pricing, trial details, eligibility rules, and launch timing are not committed in this public overview.

## Release Gate

Taskveil will not make its first general release until the billing foundation is complete. Store submission, release tags, and public launch announcements remain blocked until all of the following are verified:

- iOS App Store and Android Google Play purchase and restore work end to end
  in their respective store sandboxes.
- Receipts, transactions, and billing events are verified on the server and applied idempotently.
- Server-side entitlements are the authorization source for paid sync access.
- Expiration and revocation stop server-backed paid capabilities without making local data unreadable or uneditable.
- Re-activation restores normal sync without a second billing state maintained only by the client.

The selected implementation provider, public product identifiers, entitlement lookup keys, and state-machine behavior are reviewable implementation facts and may appear in the public technical specification and source. Concrete prices, provider comparisons or contract notes, launch experiments, revenue assumptions, legal review, and operational credentials remain non-public. The public implementation and tests must make the security boundary and release-gate evidence reviewable.

## E2EE and Entitlements

Paid capabilities are represented as entitlements associated with an account or organization. Entitlements control access to server-dependent features only. They do not grant the server access to plaintext task data, keys, notes, list names, or other encrypted content.

The technical design must preserve these boundaries:

- Local data remains controlled by the user.
- Encrypted sync payloads stay opaque to the server.
- Billing state must not become a recovery mechanism for encrypted data.
- Revoking access to sync or sharing must not erase local data.

## User Experience

Billing prompts should be quiet and contextual. Appropriate moments include setting up an additional device, enabling encrypted cloud backup, or accepting an organization invitation. Taskveil should avoid aggressive paywalls, surprise restrictions on existing local functionality, and sales notifications.

Cancellation and renewal flows should be clear. If a paid capability expires, Taskveil may stop server-backed sync or sharing, but local data remains available on the device.

The mobile client uses RevenueCat on iOS and Android. Each build receives the
platform-specific public SDK key through
`TASKVEIL_REVENUECAT_IOS_API_KEY` or
`TASKVEIL_REVENUECAT_ANDROID_API_KEY`, together with
`TASKVEIL_REVENUECAT_ENVIRONMENT`. The environment must match the server-issued
billing environment. Purchase, restore, and the store-provided subscription
management URL are available on both mobile platforms. Desktop builds do not
initialize a billing provider; their local features remain available, while
subscription changes must be made on a supported mobile platform.

## Non-Public Detail

The following information belongs in the private repository unless intentionally summarized for public release:

- concrete prices, discount levels, unselected trial experiments, and launch offers
- revenue scenarios, conversion assumptions, and financial forecasts
- provider comparisons, contract notes, fee comparisons, and operating-cost calculations
- raw billing event schemas beyond what is needed for implementation transparency
- unfinished operational notes or decision drafts
