import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:taskveil/main.dart';
import 'package:taskveil/src/core/providers.dart';
import 'package:taskveil/src/rust/api.dart' show FrontendSettingKeyDto;

import 'support/fake_bridge_service.dart';

class _FailingOnboardingSettingsFake extends FakeBridgeService {
  _FailingOnboardingSettingsFake() : super(onboardingCompleted: false);

  @override
  Future<void> setFrontendSetting({
    required FrontendSettingKeyDto key,
    required String value,
  }) async {
    if (key == onboardingCompletedSettingKey) {
      throw Exception('settings unavailable');
    }
    await super.setFrontendSetting(key: key, value: value);
  }
}

Future<void> _pumpFirstRun(WidgetTester tester, FakeBridgeService fake) async {
  await fake.createDefaultList(name: 'Inbox', sortOrder: 'a0');
  await tester.pumpWidget(
    TaskveilApp(overrides: [bridgeServiceProvider.overrideWithValue(fake)]),
  );
  await tester.pumpAndSettle();
}

Future<void> _advanceToLastPage(WidgetTester tester) async {
  await tester.tap(find.byKey(const ValueKey('onboarding-primary-action')));
  await tester.pumpAndSettle();
  await tester.tap(find.byKey(const ValueKey('onboarding-primary-action')));
  await tester.pumpAndSettle();
}

void main() {
  testWidgets('first run persists completion before opening Home', (
    tester,
  ) async {
    final fake = FakeBridgeService(onboardingCompleted: false);
    await _pumpFirstRun(tester, fake);

    expect(find.text('Make room for what matters'), findsOneWidget);
    expect(fake.syncNowCalls, 0);

    await _advanceToLastPage(tester);
    expect(find.text('Begin with one small thing'), findsOneWidget);

    await tester.tap(find.byKey(const ValueKey('onboarding-primary-action')));
    await tester.pumpAndSettle();

    expect(
      await fake.getFrontendSetting(key: onboardingCompletedSettingKey),
      '1',
    );
    expect(find.text('Make room for what matters'), findsNothing);
    expect(find.text('Lists'), findsOneWidget);
    expect(find.byKey(const ValueKey('quick-add-open')), findsOneWidget);

    await tester.pumpWidget(
      TaskveilApp(overrides: [bridgeServiceProvider.overrideWithValue(fake)]),
    );
    await tester.pumpAndSettle();
    expect(find.text('Make room for what matters'), findsNothing);
  });

  testWidgets('first run blocks foreground resume sync until completion', (
    tester,
  ) async {
    final fake = FakeBridgeService(onboardingCompleted: false);
    await fake.accountLogin(email: 'person@example.com', password: 'secret');
    await _pumpFirstRun(tester, fake);

    expect(fake.syncNowCalls, 0);

    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
    await tester.pumpAndSettle();

    expect(fake.syncNowCalls, 0);
    expect(find.text('Make room for what matters'), findsOneWidget);
  });

  testWidgets('failed completion stays on onboarding and can retry', (
    tester,
  ) async {
    final fake = _FailingOnboardingSettingsFake();
    await _pumpFirstRun(tester, fake);
    await _advanceToLastPage(tester);

    await tester.tap(find.byKey(const ValueKey('onboarding-primary-action')));
    await tester.pumpAndSettle();

    expect(find.byKey(const ValueKey('onboarding-save-error')), findsOneWidget);
    expect(find.text('Begin with one small thing'), findsOneWidget);
    expect(find.text('Lists'), findsNothing);
    expect(find.byKey(const ValueKey('quick-add-open')), findsNothing);
    expect(
      find.byKey(const ValueKey('onboarding-primary-action')),
      findsOneWidget,
    );
  });

  testWidgets('reduce motion advances onboarding without animation', (
    tester,
  ) async {
    tester.platformDispatcher.accessibilityFeaturesTestValue =
        const FakeAccessibilityFeatures(disableAnimations: true);
    addTearDown(tester.platformDispatcher.clearAccessibilityFeaturesTestValue);
    final fake = FakeBridgeService(onboardingCompleted: false);
    await _pumpFirstRun(tester, fake);

    await tester.tap(find.byKey(const ValueKey('onboarding-primary-action')));
    await tester.pump();

    expect(find.text('Private by design'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  testWidgets('onboarding supports narrow Dynamic Type', (tester) async {
    tester.view.physicalSize = const Size(320, 640);
    tester.view.devicePixelRatio = 1;
    tester.platformDispatcher.textScaleFactorTestValue = 2;
    addTearDown(() {
      tester.view.resetPhysicalSize();
      tester.view.resetDevicePixelRatio();
      tester.platformDispatcher.clearTextScaleFactorTestValue();
    });
    final fake = FakeBridgeService(onboardingCompleted: false);
    await _pumpFirstRun(tester, fake);

    expect(tester.takeException(), isNull);
    await _advanceToLastPage(tester);
    expect(find.text('Begin with one small thing'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });
}
