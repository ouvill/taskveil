import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';
import 'package:taskveil/main.dart';
import 'package:taskveil/src/core/providers.dart';
import 'package:taskveil/src/generated/l10n/app_localizations.dart';
import 'package:taskveil/src/rust/api.dart' show FrontendSettingKeyDto;
import 'package:taskveil/src/screens/menu_screen.dart';
import 'package:taskveil/src/ui/theme.dart';

import 'support/fake_bridge_service.dart';

void main() {
  testWidgets('Menu opens Account and back returns to Menu', (tester) async {
    final fake = FakeBridgeService();
    await fake.createDefaultList(name: 'Inbox', sortOrder: 'a0');
    await _pumpApp(tester, fake);

    await tester.tap(find.text('Menu'));
    await tester.pumpAndSettle();

    expect(
      find.text('Your workspace, account, and reusable tools.'),
      findsOneWidget,
    );
    expect(find.byKey(const ValueKey('menu-account')), findsOneWidget);
    expect(
      find.byKey(const ValueKey('menu-calendar-settings')),
      findsOneWidget,
    );
    expect(find.byKey(const ValueKey('menu-templates')), findsOneWidget);

    await tester.tap(find.byKey(const ValueKey('menu-account')));
    await tester.pumpAndSettle();

    expect(find.text('Account'), findsOneWidget);
    expect(find.byTooltip('Back'), findsOneWidget);
    expect(
      tester.getTopLeft(find.byIcon(LucideIcons.arrowLeft300)).dx,
      tester.getTopLeft(find.text('Account')).dx,
    );

    await tester.tap(find.byTooltip('Back'));
    await tester.pumpAndSettle();

    expect(find.text('Menu'), findsWidgets);
    expect(find.byKey(const ValueKey('menu-account')), findsOneWidget);
  });

  testWidgets('Menu opens Calendar settings and saves Monday start', (
    tester,
  ) async {
    final fake = FakeBridgeService();
    await fake.createDefaultList(name: 'Inbox', sortOrder: 'a0');
    await _pumpApp(tester, fake);

    await tester.tap(find.text('Menu'));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const ValueKey('menu-calendar-settings')));
    await tester.pumpAndSettle();

    expect(find.text('Calendar settings'), findsOneWidget);
    expect(
      tester.getTopLeft(find.byIcon(LucideIcons.arrowLeft300)).dx,
      tester.getTopLeft(find.text('Calendar settings')).dx,
    );
    expect(
      find.byKey(const ValueKey('calendar-week-start-system')),
      findsOneWidget,
    );

    await tester.tap(find.byKey(const ValueKey('calendar-week-start-monday')));
    await tester.pumpAndSettle();

    expect(
      await fake.getFrontendSetting(key: calendarWeekStartSettingKey),
      mondayCalendarWeekStart,
    );

    await tester.tap(find.byTooltip('Back'));
    await tester.pumpAndSettle();
    expect(find.text('Monday'), findsOneWidget);
  });

  testWidgets(
    'Calendar setting keeps the persisted value after failure and retries',
    (tester) async {
      final fake = _FailingCalendarSettingsBridge();
      await fake.createDefaultList(name: 'Inbox', sortOrder: 'a0');
      await fake.setFrontendSetting(
        key: calendarWeekStartSettingKey,
        value: mondayCalendarWeekStart,
      );
      fake.failCalendarWeekStartWrites = true;
      await _pumpApp(tester, fake);

      await tester.tap(find.text('Menu'));
      await tester.pumpAndSettle();
      expect(find.text('Monday'), findsOneWidget);
      await tester.tap(find.byKey(const ValueKey('menu-calendar-settings')));
      await tester.pumpAndSettle();
      await tester.tap(
        find.byKey(const ValueKey('calendar-week-start-sunday')),
      );
      await tester.pumpAndSettle();

      expect(
        find.text('Could not save the calendar setting. Try again.'),
        findsOneWidget,
      );
      expect(
        await fake.getFrontendSetting(key: calendarWeekStartSettingKey),
        mondayCalendarWeekStart,
      );
      expect(
        find.descendant(
          of: find.byKey(const ValueKey('calendar-week-start-monday')),
          matching: find.byIcon(LucideIcons.circleCheck300),
        ),
        findsOneWidget,
      );
      expect(
        find.byKey(const ValueKey('calendar-week-start-sunday')),
        findsOneWidget,
      );

      await tester.tap(find.byTooltip('Back'));
      await tester.pumpAndSettle();
      expect(find.text('Monday'), findsOneWidget);

      fake.failCalendarWeekStartWrites = false;
      await tester.tap(find.byKey(const ValueKey('menu-calendar-settings')));
      await tester.pumpAndSettle();
      await tester.tap(
        find.byKey(const ValueKey('calendar-week-start-sunday')),
      );
      await tester.pumpAndSettle();

      expect(
        await fake.getFrontendSetting(key: calendarWeekStartSettingKey),
        sundayCalendarWeekStart,
      );
      await tester.tap(find.byTooltip('Back'));
      await tester.pumpAndSettle();
      expect(find.text('Sunday'), findsOneWidget);
    },
  );

  testWidgets('Menu opens Templates', (tester) async {
    final fake = FakeBridgeService();
    await fake.createDefaultList(name: 'Inbox', sortOrder: 'a0');
    await _pumpApp(tester, fake);

    await tester.tap(find.text('Menu'));
    await tester.pumpAndSettle();

    await tester.tap(find.byKey(const ValueKey('menu-templates')));
    await tester.pumpAndSettle();

    expect(find.text('Templates'), findsWidgets);

    await tester.tap(find.byTooltip('Back'));
    await tester.pumpAndSettle();

    expect(find.text('Menu'), findsWidgets);
    expect(find.byKey(const ValueKey('menu-templates')), findsOneWidget);
  });

  testWidgets('Menu shows the signed-in account identity', (tester) async {
    final fake = FakeBridgeService();
    await fake.accountLogin(
      email: 'alice@example.com',
      password: 'correct password',
    );

    await tester.pumpWidget(
      ProviderScope(
        overrides: [bridgeServiceProvider.overrideWithValue(fake)],
        child: MaterialApp(
          theme: buildTaskveilTheme(Brightness.light),
          localizationsDelegates: AppLocalizations.localizationsDelegates,
          supportedLocales: AppLocalizations.supportedLocales,
          home: const MenuScreen(),
        ),
      ),
    );
    await tester.pumpAndSettle();

    expect(find.text('alice@example.com'), findsOneWidget);
  });
}

Future<void> _pumpApp(WidgetTester tester, FakeBridgeService fake) async {
  await tester.pumpWidget(
    TaskveilApp(overrides: [bridgeServiceProvider.overrideWithValue(fake)]),
  );
  await tester.pumpAndSettle();
}

class _FailingCalendarSettingsBridge extends FakeBridgeService {
  bool failCalendarWeekStartWrites = false;

  @override
  Future<void> setFrontendSetting({
    required FrontendSettingKeyDto key,
    required String value,
  }) {
    if (key == calendarWeekStartSettingKey && failCalendarWeekStartWrites) {
      throw StateError('calendar setting write failed');
    }
    return super.setFrontendSetting(key: key, value: value);
  }
}
