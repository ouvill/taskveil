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
        throwsA(isA<StateError>()),
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
}) => BillingStateDto(
  provider: 'revenuecat',
  providerAppUserId: _appUserId,
  lookupKey: 'pro',
  status: status,
  syncAllowed: syncAllowed,
  storeProductIdentifier: syncAllowed ? 'com.taskveil.app.pro.monthly' : null,
  willRenew: syncAllowed,
  environment: 'sandbox',
);

class _BillingBridge extends FakeBridgeService {
  int bootstrapCalls = 0;
  int refreshCalls = 0;
  int refreshFailuresRemaining = 0;
  Completer<BillingStateDto>? refreshGate;
  bool failBootstrap = false;
  bool returnDefaultCachedState = true;
  BillingStateDto? cachedState;

  @override
  Future<BillingStateDto> billingBootstrap() async {
    bootstrapCalls += 1;
    if (failBootstrap) throw StateError('billing bootstrap unavailable');
    return _billingState();
  }

  @override
  Future<BillingStateDto?> getCachedBilling() async =>
      cachedState ?? (returnDefaultCachedState ? _billingState() : null);

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
    return _billingState(status: 'active', syncAllowed: true);
  }
}

class _FakeBillingStore implements BillingStore {
  String? configuredAppUserId;
  String? configuredEnvironment;
  int purchaseCalls = 0;
  Completer<BillingPurchaseOutcome>? purchaseGate;
  BillingPurchaseOutcome purchaseOutcome = BillingPurchaseOutcome.purchased;
  bool throwOnConfigure = false;
  bool throwOnProducts = false;
  bool throwOnPurchase = false;

  @override
  Future<void> configure({
    required String appUserId,
    required String environment,
  }) async {
    configuredAppUserId = appUserId;
    configuredEnvironment = environment;
    if (throwOnConfigure) throw StateError('store unavailable');
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
    if (throwOnPurchase) throw StateError('store unavailable');
    final gate = purchaseGate;
    if (gate != null) return gate.future;
    return purchaseOutcome;
  }

  @override
  Future<BillingPurchaseOutcome> restore() async =>
      BillingPurchaseOutcome.purchased;

  @override
  Future<Uri?> managementUrl() async =>
      Uri.parse('https://apps.apple.com/account/subscriptions');

  @override
  Future<void> accountLoggedOut() async {}
}
