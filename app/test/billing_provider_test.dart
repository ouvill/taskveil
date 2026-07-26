import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/semantics.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:taskveil/src/billing/billing_store.dart';
import 'package:taskveil/src/core/providers.dart';
import 'package:taskveil/src/generated/l10n/app_localizations.dart';
import 'package:taskveil/src/rust/api.dart';
import 'package:taskveil/src/screens/account_screen.dart';

import 'support/fake_bridge_service.dart';

void main() {
  test(
    'billing uses only the server-issued App User ID and server refresh',
    () async {
      final bridge = _BillingBridge();
      final store = _FakeBillingStore();
      final container = ProviderContainer(
        overrides: [
          bridgeServiceProvider.overrideWithValue(bridge),
          billingStoreProvider.overrideWithValue(store),
        ],
      );
      addTearDown(container.dispose);
      await container
          .read(accountProvider.notifier)
          .login(email: 'alice@example.com', password: 'correct password');

      final initial = await container.read(billingProvider.future);
      expect(initial?.entitlement.status, 'free');
      expect(store.configuredAppUserId, _appUserId);
      expect(store.configuredEnvironment, 'sandbox');
      expect(initial?.products.single.price, 'Localized monthly price');

      await container
          .read(billingProvider.notifier)
          .purchase('com.taskveil.app.pro.monthly');

      final active = container.read(billingProvider).value;
      expect(bridge.refreshCalls, 1);
      expect(active?.entitlement.status, 'active');
      expect(active?.entitlement.syncAllowed, isTrue);
    },
  );

  for (final outcome in [
    BillingPurchaseOutcome.cancelled,
    BillingPurchaseOutcome.pending,
    BillingPurchaseOutcome.failed,
  ]) {
    test(
      'purchase $outcome does not ask the server to trust a receipt',
      () async {
        final bridge = _BillingBridge();
        final store = _FakeBillingStore()..purchaseOutcome = outcome;
        final container = ProviderContainer(
          overrides: [
            bridgeServiceProvider.overrideWithValue(bridge),
            billingStoreProvider.overrideWithValue(store),
          ],
        );
        addTearDown(container.dispose);
        await container
            .read(accountProvider.notifier)
            .login(email: 'alice@example.com', password: 'correct password');
        await container.read(billingProvider.future);

        await container
            .read(billingProvider.notifier)
            .purchase('com.taskveil.app.pro.monthly');

        final state = container.read(billingProvider).value!;
        expect(state.lastOutcome, outcome);
        expect(state.busy, isFalse);
        expect(state.entitlement.syncAllowed, isFalse);
        expect(bridge.refreshCalls, 0);
      },
    );
  }

  test('store exception becomes a recoverable failed outcome', () async {
    final bridge = _BillingBridge();
    final store = _FakeBillingStore()..throwOnPurchase = true;
    final container = ProviderContainer(
      overrides: [
        bridgeServiceProvider.overrideWithValue(bridge),
        billingStoreProvider.overrideWithValue(store),
      ],
    );
    addTearDown(container.dispose);
    await container
        .read(accountProvider.notifier)
        .login(email: 'alice@example.com', password: 'correct password');
    await container.read(billingProvider.future);

    await container
        .read(billingProvider.notifier)
        .purchase('com.taskveil.app.pro.monthly');

    final state = container.read(billingProvider).value!;
    expect(state.lastOutcome, BillingPurchaseOutcome.failed);
    expect(state.busy, isFalse);
    expect(bridge.refreshCalls, 0);
  });

  test(
    'successful purchase keeps its outcome when entitlement refresh fails',
    () async {
      final bridge = _BillingBridge()..refreshFailuresRemaining = 1;
      final store = _FakeBillingStore();
      final container = ProviderContainer(
        overrides: [
          bridgeServiceProvider.overrideWithValue(bridge),
          billingStoreProvider.overrideWithValue(store),
        ],
      );
      addTearDown(container.dispose);
      await container
          .read(accountProvider.notifier)
          .login(email: 'alice@example.com', password: 'correct password');
      await container.read(billingProvider.future);

      await container
          .read(billingProvider.notifier)
          .purchase('com.taskveil.app.pro.monthly');

      final stale = container.read(billingProvider).value!;
      expect(store.purchaseCalls, 1);
      expect(stale.lastOutcome, BillingPurchaseOutcome.purchased);
      expect(stale.entitlement.status, 'free');
      expect(stale.isStale, isTrue);
      expect(stale.lastRefreshError?.code, BridgeErrorCodeDto.syncFailure);
      expect(stale.busy, isFalse);

      await container.read(billingProvider.notifier).refreshFromServer();

      final recovered = container.read(billingProvider).value!;
      expect(recovered.entitlement.status, 'active');
      expect(recovered.isStale, isFalse);
      expect(recovered.lastRefreshError, isNull);
      expect(bridge.refreshCalls, 2);
    },
  );

  test('restore activates UI only after a fresh server snapshot', () async {
    final bridge = _BillingBridge();
    final store = _FakeBillingStore();
    final container = ProviderContainer(
      overrides: [
        bridgeServiceProvider.overrideWithValue(bridge),
        billingStoreProvider.overrideWithValue(store),
      ],
    );
    addTearDown(container.dispose);
    await container
        .read(accountProvider.notifier)
        .login(email: 'alice@example.com', password: 'correct password');
    await container.read(billingProvider.future);

    await container.read(billingProvider.notifier).restore();

    expect(bridge.refreshCalls, 1);
    expect(
      container.read(billingProvider).value?.entitlement.syncAllowed,
      isTrue,
    );
  });

  test(
    'bootstrap failure falls back to the display-only cached snapshot',
    () async {
      final bridge = _BillingBridge()
        ..failBootstrap = true
        ..cachedState = _billingState(
          status: 'in_grace_period',
          syncAllowed: true,
        );
      final store = _FakeBillingStore();
      final container = ProviderContainer(
        overrides: [
          bridgeServiceProvider.overrideWithValue(bridge),
          billingStoreProvider.overrideWithValue(store),
        ],
      );
      addTearDown(container.dispose);
      await container
          .read(accountProvider.notifier)
          .login(email: 'alice@example.com', password: 'correct password');

      final cached = await container.read(billingProvider.future);

      expect(cached?.entitlement.status, 'in_grace_period');
      expect(cached?.entitlement.syncAllowed, isTrue);
      expect(cached?.products, isEmpty);
      expect(store.configuredAppUserId, isNull);
    },
  );

  test('cache read failure still attempts the server bootstrap', () async {
    final bridge = _BillingBridge()..throwOnCachedBilling = true;
    final container = ProviderContainer(
      overrides: [
        bridgeServiceProvider.overrideWithValue(bridge),
        billingStoreProvider.overrideWithValue(_FakeBillingStore()),
      ],
    );
    addTearDown(container.dispose);
    await container
        .read(accountProvider.notifier)
        .login(email: 'alice@example.com', password: 'correct password');

    final billing = await container.read(billingProvider.future);

    expect(bridge.cacheCalls, 1);
    expect(bridge.bootstrapCalls, 1);
    expect(billing?.entitlement.status, 'free');
    expect(billing?.isStale, isFalse);
  });

  test('cache and bootstrap failure surface only a typed error', () async {
    final bridge = _BillingBridge()
      ..throwOnCachedBilling = true
      ..failBootstrap = true;
    final container = ProviderContainer(
      overrides: [
        bridgeServiceProvider.overrideWithValue(bridge),
        billingStoreProvider.overrideWithValue(_FakeBillingStore()),
      ],
    );
    addTearDown(container.dispose);
    await container
        .read(accountProvider.notifier)
        .login(email: 'alice@example.com', password: 'correct password');

    await expectLater(
      container.read(billingProvider.future),
      throwsA(
        isA<BridgeErrorDto>().having(
          (error) => error.code,
          'code',
          BridgeErrorCodeDto.internal,
        ),
      ),
    );
    expect(bridge.cacheCalls, 1);
    expect(bridge.bootstrapCalls, 1);
  });

  test(
    'store configuration failure keeps the latest server entitlement',
    () async {
      final bridge = _BillingBridge()
        ..cachedState = _billingState(
          status: 'in_grace_period',
          syncAllowed: true,
        );
      final store = _FakeBillingStore()..throwOnConfigure = true;
      final container = ProviderContainer(
        overrides: [
          bridgeServiceProvider.overrideWithValue(bridge),
          billingStoreProvider.overrideWithValue(store),
        ],
      );
      addTearDown(container.dispose);
      await container
          .read(accountProvider.notifier)
          .login(email: 'alice@example.com', password: 'correct password');

      final latest = await container.read(billingProvider.future);

      expect(latest?.entitlement.status, 'free');
      expect(latest?.entitlement.syncAllowed, isFalse);
      expect(latest?.products, isEmpty);
      expect(container.read(billingProvider).hasValue, isTrue);
    },
  );

  test(
    'catalog failure after server refresh keeps the refreshed entitlement',
    () async {
      final bridge = _BillingBridge();
      final store = _FakeBillingStore();
      final container = ProviderContainer(
        overrides: [
          bridgeServiceProvider.overrideWithValue(bridge),
          billingStoreProvider.overrideWithValue(store),
        ],
      );
      addTearDown(container.dispose);
      await container
          .read(accountProvider.notifier)
          .login(email: 'alice@example.com', password: 'correct password');
      await container.read(billingProvider.future);
      store.throwOnProducts = true;

      await container.read(billingProvider.notifier).refreshFromServer();

      final latest = container.read(billingProvider);
      expect(latest.hasValue, isTrue);
      expect(latest.value?.entitlement.status, 'active');
      expect(latest.value?.entitlement.syncAllowed, isTrue);
      expect(latest.value?.products, isEmpty);
      expect(latest.value?.busy, isFalse);
      expect(bridge.refreshCalls, 1);
    },
  );

  test(
    'refresh preserves a stale snapshot after failure and then recovers',
    () async {
      final bridge = _BillingBridge()..refreshFailuresRemaining = 1;
      final container = ProviderContainer(
        overrides: [
          bridgeServiceProvider.overrideWithValue(bridge),
          billingStoreProvider.overrideWithValue(_FakeBillingStore()),
        ],
      );
      addTearDown(container.dispose);
      await container
          .read(accountProvider.notifier)
          .login(email: 'alice@example.com', password: 'correct password');
      await container.read(billingProvider.future);

      await container.read(billingProvider.notifier).refreshFromServer();

      final stale = container.read(billingProvider);
      expect(stale.hasValue, isTrue);
      expect(stale.value?.entitlement.status, 'free');
      expect(stale.value?.isStale, isTrue);
      expect(
        stale.value?.lastRefreshError?.code,
        BridgeErrorCodeDto.syncFailure,
      );
      expect(stale.value?.busy, isFalse);

      await container.read(billingProvider.notifier).refreshFromServer();

      final recovered = container.read(billingProvider).value!;
      expect(recovered.entitlement.status, 'active');
      expect(recovered.isStale, isFalse);
      expect(recovered.lastRefreshError, isNull);
      expect(bridge.refreshCalls, 2);
    },
  );

  test('concurrent refreshes share one server request', () async {
    final bridge = _BillingBridge()..refreshGate = Completer<BillingStateDto>();
    final container = ProviderContainer(
      overrides: [
        bridgeServiceProvider.overrideWithValue(bridge),
        billingStoreProvider.overrideWithValue(_FakeBillingStore()),
      ],
    );
    addTearDown(container.dispose);
    await container
        .read(accountProvider.notifier)
        .login(email: 'alice@example.com', password: 'correct password');
    await container.read(billingProvider.future);

    final first = container.read(billingProvider.notifier).refreshFromServer();
    final second = container.read(billingProvider.notifier).refreshFromServer();
    await Future<void>.delayed(Duration.zero);

    expect(bridge.refreshCalls, 1);
    bridge.refreshGate!.complete(
      _billingState(status: 'active', syncAllowed: true),
    );
    await Future.wait([first, second]);
    expect(container.read(billingProvider).value?.busy, isFalse);
    expect(bridge.refreshCalls, 1);
  });

  test(
    'refresh waits for purchase and duplicate purchase shares one store call',
    () async {
      final bridge = _BillingBridge();
      final store = _FakeBillingStore()
        ..purchaseGate = Completer<BillingPurchaseOutcome>();
      final container = ProviderContainer(
        overrides: [
          bridgeServiceProvider.overrideWithValue(bridge),
          billingStoreProvider.overrideWithValue(store),
        ],
      );
      addTearDown(container.dispose);
      await container
          .read(accountProvider.notifier)
          .login(email: 'alice@example.com', password: 'correct password');
      await container.read(billingProvider.future);

      final firstPurchase = container
          .read(billingProvider.notifier)
          .purchase('com.taskveil.app.pro.monthly');
      final duplicatePurchase = container
          .read(billingProvider.notifier)
          .purchase('com.taskveil.app.pro.monthly');
      final queuedRefresh = container
          .read(billingProvider.notifier)
          .refreshFromServer();
      await Future<void>.delayed(Duration.zero);

      expect(store.purchaseCalls, 1);
      expect(bridge.refreshCalls, 0);
      expect(container.read(billingProvider).value?.busy, isTrue);

      store.purchaseGate!.complete(BillingPurchaseOutcome.purchased);
      await Future.wait([firstPurchase, duplicatePurchase, queuedRefresh]);

      expect(store.purchaseCalls, 1);
      expect(bridge.refreshCalls, 2);
      expect(container.read(billingProvider).value?.busy, isFalse);
      expect(
        container.read(billingProvider).value?.entitlement.status,
        'active',
      );
    },
  );

  test(
    'only exact duplicate store actions coalesce and distinct actions stay FIFO',
    () async {
      final bridge = _BillingBridge();
      final store = _FakeBillingStore()
        ..purchaseGate = Completer<BillingPurchaseOutcome>();
      final container = ProviderContainer(
        overrides: [
          bridgeServiceProvider.overrideWithValue(bridge),
          billingStoreProvider.overrideWithValue(store),
        ],
      );
      addTearDown(container.dispose);
      await container
          .read(accountProvider.notifier)
          .login(email: 'alice@example.com', password: 'correct password');
      await container.read(billingProvider.future);

      final monthly = container
          .read(billingProvider.notifier)
          .purchase('com.taskveil.app.pro.monthly');
      final monthlyDuplicate = container
          .read(billingProvider.notifier)
          .purchase('com.taskveil.app.pro.monthly');
      final restore = container.read(billingProvider.notifier).restore();
      final yearly = container
          .read(billingProvider.notifier)
          .purchase('com.taskveil.app.pro.yearly');
      await Future<void>.delayed(Duration.zero);

      expect(store.actionLog, ['purchase:com.taskveil.app.pro.monthly']);

      store.purchaseGate!.complete(BillingPurchaseOutcome.purchased);
      await Future.wait([monthly, monthlyDuplicate, restore, yearly]);

      expect(store.actionLog, [
        'purchase:com.taskveil.app.pro.monthly',
        'restore',
        'purchase:com.taskveil.app.pro.yearly',
      ]);
      expect(store.purchaseCalls, 2);
      expect(store.restoreCalls, 1);
      expect(bridge.refreshCalls, 3);
    },
  );

  test(
    'account switch serializes configure and reasserts the latest identity',
    () async {
      const nextAppUserId = '00000000-0000-4000-8000-000000000002';
      final bridge = _BillingBridge();
      final store = _FakeBillingStore()..firstConfigureGate = Completer<void>();
      final container = ProviderContainer(
        overrides: [
          bridgeServiceProvider.overrideWithValue(bridge),
          billingStoreProvider.overrideWithValue(store),
        ],
      );
      addTearDown(container.dispose);
      await container
          .read(accountProvider.notifier)
          .login(email: 'alice@example.com', password: 'correct password');
      final firstBilling = container.read(billingProvider.future);
      await Future<void>.delayed(Duration.zero);
      expect(store.configureCalls, [_appUserId]);

      bridge.providerAppUserId = nextAppUserId;
      await container
          .read(accountProvider.notifier)
          .login(email: 'bob@example.com', password: 'correct password');
      final nextBilling = container.read(billingProvider.future);
      await Future<void>.delayed(Duration.zero);
      expect(store.configureCalls, [_appUserId]);

      await container
          .read(billingProvider.notifier)
          .purchase('com.taskveil.app.pro.monthly');
      expect(store.actionLog, isEmpty);

      store.firstConfigureGate!.complete();
      await firstBilling;
      final switched = await nextBilling;
      expect(switched?.entitlement.providerAppUserId, nextAppUserId);
      expect(store.configureCalls, [_appUserId, nextAppUserId]);
      expect(store.configuredAppUserId, nextAppUserId);

      await container
          .read(billingProvider.notifier)
          .purchase('com.taskveil.app.pro.monthly');

      expect(store.configureCalls.last, nextAppUserId);
      expect(store.purchaseAppUserIds, [nextAppUserId]);
    },
  );

  test(
    'realtime connection recovery retries a failed billing refresh',
    () async {
      final bridge = _BillingBridge()..refreshFailuresRemaining = 1;
      final container = ProviderContainer(
        overrides: [
          bridgeServiceProvider.overrideWithValue(bridge),
          billingStoreProvider.overrideWithValue(_FakeBillingStore()),
        ],
      );
      addTearDown(container.dispose);
      await container
          .read(accountProvider.notifier)
          .login(email: 'alice@example.com', password: 'correct password');
      await container.read(billingProvider.future);
      await container.read(billingProvider.notifier).refreshFromServer();
      expect(container.read(billingProvider).value?.isStale, isTrue);

      await container.read(syncStatusProvider.future);
      container.read(syncStatusProvider.notifier).setRealtimeConnected(false);
      container.read(syncStatusProvider.notifier).setRealtimeConnected(true);
      await Future<void>.delayed(Duration.zero);

      expect(bridge.refreshCalls, 2);
      expect(container.read(billingProvider).value?.isStale, isFalse);
      expect(container.read(billingProvider).value?.lastRefreshError, isNull);
    },
  );

  test(
    'first connected event rebuilds a failed bootstrap without a cache',
    () async {
      final bridge = _BillingBridge()
        ..failBootstrap = true
        ..returnDefaultCachedState = false;
      final container = ProviderContainer(
        overrides: [
          bridgeServiceProvider.overrideWithValue(bridge),
          billingStoreProvider.overrideWithValue(_FakeBillingStore()),
        ],
      );
      addTearDown(container.dispose);
      await container
          .read(accountProvider.notifier)
          .login(email: 'alice@example.com', password: 'correct password');
      await expectLater(
        container.read(billingProvider.future),
        throwsA(isA<BridgeErrorDto>()),
      );
      await container.read(syncStatusProvider.future);

      bridge.failBootstrap = false;
      container.read(syncStatusProvider.notifier).setRealtimeConnected(true);
      await Future<void>.delayed(Duration.zero);

      final recovered = await container.read(billingProvider.future);
      expect(recovered?.entitlement.status, 'free');
      expect(recovered?.isStale, isFalse);
      expect(bridge.bootstrapCalls, 2);
      expect(bridge.refreshCalls, 0);
    },
  );

  test(
    'first connected event does not refresh an already fresh snapshot',
    () async {
      final bridge = _BillingBridge();
      final container = ProviderContainer(
        overrides: [
          bridgeServiceProvider.overrideWithValue(bridge),
          billingStoreProvider.overrideWithValue(_FakeBillingStore()),
        ],
      );
      addTearDown(container.dispose);
      await container
          .read(accountProvider.notifier)
          .login(email: 'alice@example.com', password: 'correct password');
      await container.read(billingProvider.future);
      await container.read(syncStatusProvider.future);

      container.read(syncStatusProvider.notifier).setRealtimeConnected(true);
      await Future<void>.delayed(Duration.zero);

      expect(bridge.refreshCalls, 0);
    },
  );

  test(
    'logout during refresh prevents the disposed notifier from publishing',
    () async {
      final bridge = _BillingBridge()
        ..refreshGate = Completer<BillingStateDto>();
      final container = ProviderContainer(
        overrides: [
          bridgeServiceProvider.overrideWithValue(bridge),
          billingStoreProvider.overrideWithValue(_FakeBillingStore()),
        ],
      );
      addTearDown(container.dispose);
      await container
          .read(accountProvider.notifier)
          .login(email: 'alice@example.com', password: 'correct password');
      await container.read(billingProvider.future);

      final refresh = container
          .read(billingProvider.notifier)
          .refreshFromServer();
      await Future<void>.delayed(Duration.zero);
      expect(bridge.refreshCalls, 1);

      await container.read(accountProvider.notifier).logout();
      bridge.refreshGate!.complete(
        _billingState(status: 'active', syncAllowed: true),
      );

      await expectLater(refresh, completes);
      expect(await container.read(billingProvider.future), isNull);
    },
  );

  test(
    'logout closes billing admission before slow work and next account reopens it',
    () async {
      const nextAppUserId = '00000000-0000-4000-8000-000000000002';
      final bridge = _BillingBridge();
      final store = _FakeBillingStore();
      final container = ProviderContainer(
        overrides: [
          bridgeServiceProvider.overrideWithValue(bridge),
          billingStoreProvider.overrideWithValue(store),
        ],
      );
      addTearDown(container.dispose);
      await container
          .read(accountProvider.notifier)
          .login(email: 'alice@example.com', password: 'correct password');
      await container.read(billingProvider.future);

      store.managementGate = Completer<Uri?>();
      final oldManagementUrl = container
          .read(billingProvider.notifier)
          .managementUrl();
      await Future<void>.delayed(Duration.zero);
      expect(store.managementCalls, 1);
      final queuedOldPurchase = container
          .read(billingProvider.notifier)
          .purchase('com.taskveil.app.pro.monthly');
      await Future<void>.delayed(Duration.zero);

      bridge.logoutGate = Completer<void>();
      final logout = container.read(accountProvider.notifier).logout();
      await Future.wait([
        container
            .read(billingProvider.notifier)
            .purchase('com.taskveil.app.pro.monthly'),
        container.read(billingProvider.notifier).restore(),
      ]);
      final blockedManagementUrl = await container
          .read(billingProvider.notifier)
          .managementUrl();

      expect(store.purchaseCalls, 0);
      expect(store.restoreCalls, 0);
      expect(store.managementCalls, 1);
      expect(blockedManagementUrl, isNull);

      store.managementGate!.complete(
        Uri.parse('https://apps.apple.com/account/subscriptions'),
      );
      expect(await oldManagementUrl, isNull);
      await queuedOldPurchase;
      expect(store.purchaseCalls, 0);
      bridge.logoutGate!.complete();
      await logout;

      bridge.providerAppUserId = nextAppUserId;
      await container
          .read(accountProvider.notifier)
          .login(email: 'bob@example.com', password: 'correct password');
      await container.read(billingProvider.future);
      await container
          .read(billingProvider.notifier)
          .purchase('com.taskveil.app.pro.monthly');

      expect(store.purchaseCalls, 1);
      expect(store.purchaseAppUserIds, [nextAppUserId]);
    },
  );

  test(
    'slow logout invalidation cannot reopen the old account admission',
    () async {
      final bridge = _BillingBridge();
      final store = _FakeBillingStore();
      final container = ProviderContainer(
        overrides: [
          bridgeServiceProvider.overrideWithValue(bridge),
          billingStoreProvider.overrideWithValue(store),
        ],
      );
      addTearDown(container.dispose);
      await container
          .read(accountProvider.notifier)
          .login(email: 'alice@example.com', password: 'correct password');
      await container.read(billingProvider.future);
      final configureCallsBeforeLogout = store.configureCalls.length;
      final bootstrapCallsBeforeLogout = bridge.bootstrapCalls;

      bridge.logoutGate = Completer<void>();
      final logout = container.read(accountProvider.notifier).logout();
      container.invalidate(accountProvider);
      final rebuiltAccount = await container.read(accountProvider.future);
      expect(rebuiltAccount.loggedIn, isTrue);

      container.invalidate(billingProvider);
      expect(await container.read(billingProvider.future), isNull);
      expect(store.configureCalls.length, configureCallsBeforeLogout);
      expect(bridge.bootstrapCalls, bootstrapCallsBeforeLogout);

      await container
          .read(billingProvider.notifier)
          .purchase('com.taskveil.app.pro.monthly');
      expect(store.purchaseCalls, 0);

      bridge.logoutGate!.complete();
      await logout;
    },
  );

  for (final operation in ['login', 'register', 'logout']) {
    test(
      '$operation failure restores the unchanged account admission',
      () async {
        final bridge = _BillingBridge();
        final store = _FakeBillingStore();
        final container = ProviderContainer(
          overrides: [
            bridgeServiceProvider.overrideWithValue(bridge),
            billingStoreProvider.overrideWithValue(store),
          ],
        );
        addTearDown(container.dispose);
        await container
            .read(accountProvider.notifier)
            .login(email: 'alice@example.com', password: 'correct password');
        final originalAccount = await container.read(accountProvider.future);
        await container.read(billingProvider.future);

        Future<void> failedOperation;
        switch (operation) {
          case 'login':
            bridge.failLogin = true;
            failedOperation = container
                .read(accountProvider.notifier)
                .login(email: 'bob@example.com', password: 'correct password');
          case 'register':
            await container
                .read(accountProvider.notifier)
                .registrationBegin(email: 'bob@example.com');
            await container
                .read(accountProvider.notifier)
                .registrationVerifyOtp('12345678');
            bridge.failRegister = true;
            failedOperation = container
                .read(accountProvider.notifier)
                .registrationComplete(password: 'correct password');
          default:
            bridge.failLogout = true;
            failedOperation = container.read(accountProvider.notifier).logout();
        }

        await expectLater(failedOperation, throwsA(isA<BridgeErrorDto>()));
        expect(container.read(accountProvider).value, originalAccount);
        final recovered = await container.read(billingProvider.future);
        expect(recovered?.entitlement.providerAppUserId, _appUserId);

        await container
            .read(billingProvider.notifier)
            .purchase('com.taskveil.app.pro.monthly');
        expect(store.purchaseCalls, 1);
      },
    );
  }

  test(
    'unknown session after auth failure remains a typed visible error',
    () async {
      final bridge = _BillingBridge();
      final container = ProviderContainer(
        overrides: [
          bridgeServiceProvider.overrideWithValue(bridge),
          billingStoreProvider.overrideWithValue(_FakeBillingStore()),
        ],
      );
      addTearDown(container.dispose);
      await container
          .read(accountProvider.notifier)
          .login(email: 'alice@example.com', password: 'correct password');
      await container.read(accountProvider.future);

      bridge
        ..failLogin = true
        ..sessionReadFailuresRemaining = 1;
      await expectLater(
        container
            .read(accountProvider.notifier)
            .login(email: 'bob@example.com', password: 'correct password'),
        throwsA(
          isA<BridgeErrorDto>().having(
            (error) => error.code,
            'code',
            BridgeErrorCodeDto.internal,
          ),
        ),
      );

      expect(
        container.read(accountProvider).error,
        isA<BridgeErrorDto>().having(
          (error) => error.code,
          'code',
          BridgeErrorCodeDto.internal,
        ),
      );
      await expectLater(
        container.read(billingProvider.future),
        throwsA(isA<BridgeErrorDto>()),
      );
    },
  );

  test(
    'a hung old refresh does not block the next account generation',
    () async {
      const nextAppUserId = '00000000-0000-4000-8000-000000000002';
      final bridge = _BillingBridge();
      final store = _FakeBillingStore();
      final container = ProviderContainer(
        overrides: [
          bridgeServiceProvider.overrideWithValue(bridge),
          billingStoreProvider.overrideWithValue(store),
        ],
      );
      addTearDown(container.dispose);
      await container
          .read(accountProvider.notifier)
          .login(email: 'alice@example.com', password: 'correct password');
      await container.read(billingProvider.future);

      final oldRefreshGate = Completer<BillingStateDto>();
      bridge.refreshGate = oldRefreshGate;
      final oldRefresh = container
          .read(billingProvider.notifier)
          .refreshFromServer();
      await Future<void>.delayed(Duration.zero);
      expect(bridge.refreshCalls, 1);

      bridge
        ..refreshGate = null
        ..providerAppUserId = nextAppUserId;
      await container
          .read(accountProvider.notifier)
          .login(email: 'bob@example.com', password: 'correct password');
      await container.read(billingProvider.future);
      await container.read(billingProvider.notifier).refreshFromServer();
      await container
          .read(billingProvider.notifier)
          .purchase('com.taskveil.app.pro.monthly');

      expect(bridge.refreshCalls, 3);
      expect(store.purchaseAppUserIds, [nextAppUserId]);

      oldRefreshGate.complete(_billingState());
      await oldRefresh;
    },
  );

  test(
    'the next account waits for an uncancellable in-flight store purchase',
    () async {
      const nextAppUserId = '00000000-0000-4000-8000-000000000002';
      final bridge = _BillingBridge();
      final store = _FakeBillingStore()
        ..purchaseGate = Completer<BillingPurchaseOutcome>();
      final container = ProviderContainer(
        overrides: [
          bridgeServiceProvider.overrideWithValue(bridge),
          billingStoreProvider.overrideWithValue(store),
        ],
      );
      addTearDown(container.dispose);
      await container
          .read(accountProvider.notifier)
          .login(email: 'alice@example.com', password: 'correct password');
      await container.read(billingProvider.future);

      final oldPurchase = container
          .read(billingProvider.notifier)
          .purchase('com.taskveil.app.pro.monthly');
      await Future<void>.delayed(Duration.zero);
      expect(store.purchaseCalls, 1);

      bridge.providerAppUserId = nextAppUserId;
      await container
          .read(accountProvider.notifier)
          .login(email: 'bob@example.com', password: 'correct password');
      final nextBilling = container.read(billingProvider.future);
      var nextReady = false;
      nextBilling.then((_) => nextReady = true);
      await Future<void>.delayed(Duration.zero);
      expect(nextReady, isFalse);

      store.purchaseGate!.complete(BillingPurchaseOutcome.cancelled);
      await oldPurchase;
      expect((await nextBilling)?.entitlement.providerAppUserId, nextAppUserId);
    },
  );

  test(
    'logout drains an in-flight purchase even after billing invalidation',
    () async {
      final bridge = _BillingBridge();
      final store = _FakeBillingStore()
        ..purchaseGate = Completer<BillingPurchaseOutcome>();
      final container = ProviderContainer(
        overrides: [
          bridgeServiceProvider.overrideWithValue(bridge),
          billingStoreProvider.overrideWithValue(store),
        ],
      );
      addTearDown(container.dispose);
      await container
          .read(accountProvider.notifier)
          .login(email: 'alice@example.com', password: 'correct password');
      await container.read(billingProvider.future);

      final purchase = container
          .read(billingProvider.notifier)
          .purchase('com.taskveil.app.pro.monthly');
      await Future<void>.delayed(Duration.zero);
      expect(store.purchaseCalls, 1);

      container.invalidate(billingProvider);
      final logout = container.read(accountProvider.notifier).logout();
      await Future<void>.delayed(Duration.zero);
      expect(bridge.logoutCalls, 0);

      store.purchaseGate!.complete(BillingPurchaseOutcome.cancelled);
      await Future.wait([purchase, logout]);

      expect(bridge.logoutCalls, 1);
      expect((await container.read(accountProvider.future)).loggedIn, isFalse);
      expect(await container.read(billingProvider.future), isNull);
    },
  );

  test('logout does not wait for post-purchase entitlement refresh', () async {
    final bridge = _BillingBridge()..refreshGate = Completer<BillingStateDto>();
    final store = _FakeBillingStore();
    final container = ProviderContainer(
      overrides: [
        bridgeServiceProvider.overrideWithValue(bridge),
        billingStoreProvider.overrideWithValue(store),
      ],
    );
    addTearDown(container.dispose);
    await container
        .read(accountProvider.notifier)
        .login(email: 'alice@example.com', password: 'correct password');
    await container.read(billingProvider.future);

    final purchase = container
        .read(billingProvider.notifier)
        .purchase('com.taskveil.app.pro.monthly');
    while (bridge.refreshCalls == 0) {
      await Future<void>.delayed(Duration.zero);
    }
    expect(store.purchaseCalls, 1);
    expect(container.read(billingProvider).value?.busy, isTrue);
    expect(
      container.read(billingProvider).value?.storeTransactionBusy,
      isFalse,
    );

    await container.read(accountProvider.notifier).logout();
    expect(bridge.logoutCalls, 1);
    expect((await container.read(accountProvider.future)).loggedIn, isFalse);

    bridge.refreshGate!.complete(
      _billingState(status: 'active', syncAllowed: true),
    );
    await purchase;
    expect(await container.read(billingProvider.future), isNull);
  });

  test(
    'explicit invalidation during refresh ignores the old completion',
    () async {
      final bridge = _BillingBridge()
        ..refreshGate = Completer<BillingStateDto>();
      final container = ProviderContainer(
        overrides: [
          bridgeServiceProvider.overrideWithValue(bridge),
          billingStoreProvider.overrideWithValue(_FakeBillingStore()),
        ],
      );
      addTearDown(container.dispose);
      await container
          .read(accountProvider.notifier)
          .login(email: 'alice@example.com', password: 'correct password');
      await container.read(billingProvider.future);

      final refresh = container
          .read(billingProvider.notifier)
          .refreshFromServer();
      await Future<void>.delayed(Duration.zero);
      expect(bridge.refreshCalls, 1);

      container.invalidate(billingProvider);
      bridge.refreshGate!.complete(
        _billingState(status: 'active', syncAllowed: true),
      );

      await expectLater(refresh, completes);
      final rebuilt = await container.read(billingProvider.future);
      expect(rebuilt?.entitlement.status, 'free');
      expect(rebuilt?.busy, isFalse);
    },
  );

  testWidgets('stale billing snapshot is explicit and manual retry recovers', (
    tester,
  ) async {
    final bridge = _BillingBridge()
      ..failBootstrap = true
      ..cachedState = _billingState();
    await bridge.accountLogin(
      email: 'alice@example.com',
      password: 'correct password',
    );

    await tester.pumpWidget(
      ProviderScope(
        overrides: [
          bridgeServiceProvider.overrideWithValue(bridge),
          billingStoreProvider.overrideWithValue(_FakeBillingStore()),
        ],
        child: const MaterialApp(
          localizationsDelegates: AppLocalizations.localizationsDelegates,
          supportedLocales: AppLocalizations.supportedLocales,
          home: AccountScreen(),
        ),
      ),
    );
    await tester.pumpAndSettle();

    expect(
      find.text(
        'Showing the last known subscription status. '
        'Sync access is still checked by the server.',
      ),
      findsOneWidget,
    );
    expect(find.widgetWithText(TextButton, 'Retry'), findsOneWidget);

    final retry = find.widgetWithText(TextButton, 'Retry');
    await tester.ensureVisible(retry);
    await tester.tap(retry);
    await tester.pumpAndSettle();

    expect(find.text('Active'), findsOneWidget);
    expect(find.textContaining('Showing the last known'), findsNothing);
    expect(bridge.refreshCalls, 1);
  });

  testWidgets(
    'bootstrap error retry has an accessible tap target and recovers',
    (tester) async {
      final bridge = _BillingBridge()
        ..failBootstrap = true
        ..returnDefaultCachedState = false;
      await bridge.accountLogin(
        email: 'alice@example.com',
        password: 'correct password',
      );

      await tester.pumpWidget(
        ProviderScope(
          overrides: [
            bridgeServiceProvider.overrideWithValue(bridge),
            billingStoreProvider.overrideWithValue(_FakeBillingStore()),
          ],
          child: const MaterialApp(
            localizationsDelegates: AppLocalizations.localizationsDelegates,
            supportedLocales: AppLocalizations.supportedLocales,
            home: AccountScreen(),
          ),
        ),
      );
      await tester.pumpAndSettle();

      final retry = find.widgetWithText(TextButton, 'Retry');
      await tester.ensureVisible(retry);
      await tester.pumpAndSettle();
      expect(retry, findsOneWidget);
      final retrySize = tester.getSize(retry);
      expect(retrySize.width, greaterThanOrEqualTo(48));
      expect(retrySize.height, greaterThanOrEqualTo(48));
      final semantics = tester.getSemantics(retry).getSemanticsData();
      expect(semantics.flagsCollection.isButton, isTrue);
      expect(semantics.hasAction(SemanticsAction.tap), isTrue);

      bridge.failBootstrap = false;
      await tester.tap(retry);
      await tester.pumpAndSettle();

      expect(find.text('Free'), findsOneWidget);
      expect(find.text('Billing is unavailable right now.'), findsNothing);
      expect(bridge.bootstrapCalls, 2);
    },
  );

  testWidgets('logout busy state disables billing purchase and restore', (
    tester,
  ) async {
    final bridge = _BillingBridge()..logoutGate = Completer<void>();
    await bridge.accountLogin(
      email: 'alice@example.com',
      password: 'correct password',
    );

    await tester.pumpWidget(
      ProviderScope(
        overrides: [
          bridgeServiceProvider.overrideWithValue(bridge),
          billingStoreProvider.overrideWithValue(_FakeBillingStore()),
        ],
        child: const MaterialApp(
          localizationsDelegates: AppLocalizations.localizationsDelegates,
          supportedLocales: AppLocalizations.supportedLocales,
          home: AccountScreen(),
        ),
      ),
    );
    await tester.pumpAndSettle();

    final logout = find.byKey(const ValueKey('account-logout'));
    await tester.ensureVisible(logout);
    await tester.tap(logout);
    await tester.pump();

    final purchase = find.widgetWithText(FilledButton, 'Start Pro');
    final restore = find.widgetWithText(TextButton, 'Restore purchases');
    await tester.ensureVisible(restore);
    await tester.pump();
    expect(tester.widget<FilledButton>(purchase).onPressed, isNull);
    expect(tester.widget<TextButton>(restore).onPressed, isNull);

    bridge.logoutGate!.complete();
    await tester.pumpAndSettle();
  });

  testWidgets(
    'store transaction disables logout but entitlement refresh does not',
    (tester) async {
      final bridge = _BillingBridge();
      await bridge.accountLogin(
        email: 'alice@example.com',
        password: 'correct password',
      );
      final store = _FakeBillingStore()
        ..purchaseGate = Completer<BillingPurchaseOutcome>();

      await tester.pumpWidget(
        ProviderScope(
          overrides: [
            bridgeServiceProvider.overrideWithValue(bridge),
            billingStoreProvider.overrideWithValue(store),
          ],
          child: const MaterialApp(
            localizationsDelegates: AppLocalizations.localizationsDelegates,
            supportedLocales: AppLocalizations.supportedLocales,
            home: AccountScreen(),
          ),
        ),
      );
      await tester.pumpAndSettle();

      final purchase = find.widgetWithText(FilledButton, 'Start Pro').first;
      final logout = find.byKey(const ValueKey('account-logout'));
      final logoutTapTarget = find.descendant(
        of: logout,
        matching: find.byType(InkWell),
      );
      await tester.ensureVisible(purchase);
      await tester.tap(purchase);
      await tester.pump();
      expect(tester.widget<InkWell>(logoutTapTarget).onTap, isNull);

      store.purchaseGate!.complete(BillingPurchaseOutcome.cancelled);
      await tester.pumpAndSettle();
      expect(tester.widget<InkWell>(logoutTapTarget).onTap, isNotNull);

      bridge.refreshGate = Completer<BillingStateDto>();
      final container = ProviderScope.containerOf(
        tester.element(find.byType(AccountScreen)),
      );
      final refresh = container
          .read(billingProvider.notifier)
          .refreshFromServer();
      await tester.pump();
      expect(tester.widget<InkWell>(logoutTapTarget).onTap, isNotNull);

      bridge.refreshGate!.complete(_billingState());
      await refresh;
      await tester.pumpAndSettle();
    },
  );

  testWidgets('store-unavailable fallback disables store controls', (
    tester,
  ) async {
    final bridge = _BillingBridge()
      ..failBootstrap = true
      ..cachedState = _billingState(status: 'active', syncAllowed: true);
    await bridge.accountLogin(
      email: 'alice@example.com',
      password: 'correct password',
    );

    await tester.pumpWidget(
      ProviderScope(
        overrides: [
          bridgeServiceProvider.overrideWithValue(bridge),
          billingStoreProvider.overrideWithValue(_FakeBillingStore()),
        ],
        child: const MaterialApp(
          localizationsDelegates: AppLocalizations.localizationsDelegates,
          supportedLocales: AppLocalizations.supportedLocales,
          home: AccountScreen(),
        ),
      ),
    );
    await tester.pumpAndSettle();

    final restore = find.widgetWithText(TextButton, 'Restore purchases');
    final manage = find.widgetWithText(TextButton, 'Manage subscription');
    expect(tester.widget<TextButton>(restore).onPressed, isNull);
    expect(tester.widget<TextButton>(manage).onPressed, isNull);
    expect(
      tester
          .widget<TextButton>(find.widgetWithText(TextButton, 'Retry').first)
          .onPressed,
      isNotNull,
    );
  });

  testWidgets('Pro section is localized and remains readable at large type', (
    tester,
  ) async {
    final bridge = _BillingBridge();
    final store = _FakeBillingStore();
    await bridge.accountLogin(
      email: 'alice@example.com',
      password: 'correct password',
    );
    tester.platformDispatcher.localeTestValue = const Locale('ja');
    tester.platformDispatcher.localesTestValue = const [Locale('ja')];
    tester.platformDispatcher.textScaleFactorTestValue = 1.6;
    addTearDown(() {
      tester.platformDispatcher.clearLocaleTestValue();
      tester.platformDispatcher.clearLocalesTestValue();
      tester.platformDispatcher.clearTextScaleFactorTestValue();
    });

    await tester.pumpWidget(
      ProviderScope(
        overrides: [
          bridgeServiceProvider.overrideWithValue(bridge),
          billingStoreProvider.overrideWithValue(store),
        ],
        child: const MaterialApp(
          locale: Locale('ja'),
          localizationsDelegates: AppLocalizations.localizationsDelegates,
          supportedLocales: AppLocalizations.supportedLocales,
          home: AccountScreen(),
        ),
      ),
    );
    await tester.pumpAndSettle();

    expect(find.text('Pro'), findsOneWidget);
    expect(
      find.text('ProではE2EE同期と暗号化クラウドバックアップを利用できます。端末のサブスクリプション設定からいつでも解約できます。'),
      findsOneWidget,
    );
    expect(find.textContaining('トライアル'), findsNothing);
    expect(find.text('月額'), findsOneWidget);
    expect(find.text('Localized monthly price'), findsOneWidget);
    final semantics = tester.ensureSemantics();
    expect(
      find.semantics.byPredicate((node) {
        final label = node.getSemanticsData().label;
        return label.contains('月額') &&
            label.contains('Localized monthly price') &&
            !label.contains('トライアル');
      }),
      findsWidgets,
    );
    final purchaseSemantics = tester.getSemantics(
      find.widgetWithText(FilledButton, 'Proを始める'),
    );
    final purchaseSemanticsData = purchaseSemantics.getSemanticsData();
    expect(purchaseSemanticsData.label, 'Proを始める');
    expect(purchaseSemanticsData.flagsCollection.isButton, isTrue);
    expect(purchaseSemanticsData.hasAction(SemanticsAction.tap), isTrue);
    semantics.dispose();
    expect(tester.takeException(), isNull);
  });
}

const _appUserId = '00000000-0000-4000-8000-000000000001';

BillingStateDto _billingState({
  String status = 'free',
  bool syncAllowed = false,
  String appUserId = _appUserId,
}) => BillingStateDto(
  provider: 'revenuecat',
  providerAppUserId: appUserId,
  lookupKey: 'pro',
  status: status,
  syncAllowed: syncAllowed,
  storeProductIdentifier: syncAllowed ? 'com.taskveil.app.pro.monthly' : null,
  willRenew: syncAllowed,
  environment: 'sandbox',
);

class _BillingBridge extends FakeBridgeService {
  int cacheCalls = 0;
  int bootstrapCalls = 0;
  int refreshCalls = 0;
  int refreshFailuresRemaining = 0;
  int logoutCalls = 0;
  Completer<BillingStateDto>? refreshGate;
  Completer<void>? logoutGate;
  int sessionReadFailuresRemaining = 0;
  bool failBootstrap = false;
  bool failLogin = false;
  bool failRegister = false;
  bool failLogout = false;
  bool throwOnCachedBilling = false;
  bool returnDefaultCachedState = true;
  BillingStateDto? cachedState;
  String providerAppUserId = _appUserId;

  @override
  Future<AccountSessionStateDto> getAccountSessionState() async {
    if (sessionReadFailuresRemaining > 0) {
      sessionReadFailuresRemaining -= 1;
      throw StateError('account session unavailable');
    }
    return super.getAccountSessionState();
  }

  @override
  Future<AccountAuthResultDto> accountLogin({
    required String email,
    required String password,
    String? serverUrl,
    String? deviceName,
  }) async {
    if (failLogin) throw StateError('login unavailable');
    return super.accountLogin(
      email: email,
      password: password,
      serverUrl: serverUrl,
      deviceName: deviceName,
    );
  }

  @override
  Future<AccountAuthResultDto> accountRegistrationComplete({
    required String password,
    String? deviceName,
  }) async {
    if (failRegister) throw StateError('registration unavailable');
    return super.accountRegistrationComplete(
      password: password,
      deviceName: deviceName,
    );
  }

  @override
  Future<void> accountLogout() async {
    await logoutGate?.future;
    if (failLogout) throw StateError('logout unavailable');
    logoutCalls += 1;
    await super.accountLogout();
  }

  @override
  Future<BillingStateDto> billingBootstrap() async {
    bootstrapCalls += 1;
    if (failBootstrap) throw StateError('billing bootstrap unavailable');
    return _billingState(appUserId: providerAppUserId);
  }

  @override
  Future<BillingStateDto?> getCachedBilling() async {
    cacheCalls += 1;
    if (throwOnCachedBilling) {
      throw StateError('billing cache unavailable');
    }
    return cachedState ??
        (returnDefaultCachedState
            ? _billingState(appUserId: providerAppUserId)
            : null);
  }

  @override
  Future<BillingStateDto> refreshBilling() async {
    refreshCalls += 1;
    if (refreshFailuresRemaining > 0) {
      refreshFailuresRemaining -= 1;
      throw const BridgeErrorDto(
        code: BridgeErrorCodeDto.syncFailure,
        arguments: [],
        retryable: true,
      );
    }
    final gate = refreshGate;
    if (gate != null) return gate.future;
    return _billingState(
      status: 'active',
      syncAllowed: true,
      appUserId: providerAppUserId,
    );
  }
}

class _FakeBillingStore implements BillingStore {
  String? configuredAppUserId;
  String? configuredEnvironment;
  final List<String> configureCalls = [];
  final List<String> actionLog = [];
  final List<String?> purchaseAppUserIds = [];
  int purchaseCalls = 0;
  int restoreCalls = 0;
  int managementCalls = 0;
  Completer<void>? firstConfigureGate;
  Completer<BillingPurchaseOutcome>? purchaseGate;
  Completer<Uri?>? managementGate;
  BillingPurchaseOutcome purchaseOutcome = BillingPurchaseOutcome.purchased;
  bool throwOnConfigure = false;
  bool throwOnProducts = false;
  bool throwOnPurchase = false;

  @override
  Future<void> configure({
    required String appUserId,
    required String environment,
  }) async {
    configureCalls.add(appUserId);
    if (configureCalls.length == 1) {
      await firstConfigureGate?.future;
    }
    if (throwOnConfigure) throw StateError('store unavailable');
    configuredAppUserId = appUserId;
    configuredEnvironment = environment;
  }

  @override
  Future<List<BillingProduct>> products() async {
    if (throwOnProducts) throw StateError('catalog unavailable');
    return const [
      BillingProduct(
        identifier: 'com.taskveil.app.pro.monthly',
        title: 'Taskveil Pro Monthly',
        description: 'Monthly Pro',
        price: 'Localized monthly price',
        isAnnual: false,
      ),
    ];
  }

  @override
  Future<BillingPurchaseOutcome> purchase(String productIdentifier) async {
    purchaseCalls += 1;
    actionLog.add('purchase:$productIdentifier');
    purchaseAppUserIds.add(configuredAppUserId);
    if (throwOnPurchase) throw StateError('store unavailable');
    final gate = purchaseGate;
    if (gate != null) return gate.future;
    return purchaseOutcome;
  }

  @override
  Future<BillingPurchaseOutcome> restore() async {
    restoreCalls += 1;
    actionLog.add('restore');
    return BillingPurchaseOutcome.purchased;
  }

  @override
  Future<Uri?> managementUrl() async {
    managementCalls += 1;
    actionLog.add('manage');
    final gate = managementGate;
    if (gate != null) return gate.future;
    return Uri.parse('https://apps.apple.com/account/subscriptions');
  }

  @override
  Future<void> accountLoggedOut() async {}
}
