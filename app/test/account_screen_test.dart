import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:taskveil/src/core/providers.dart';
import 'package:taskveil/src/generated/l10n/app_localizations.dart';
import 'package:taskveil/src/rust/api.dart';
import 'package:taskveil/src/screens/account_screen.dart';
import 'package:taskveil/src/ui/theme.dart';

import 'support/fake_bridge_service.dart';

Future<void> _pumpAccountScreen(
  WidgetTester tester,
  FakeBridgeService fake,
) async {
  await tester.pumpWidget(
    ProviderScope(
      overrides: [bridgeServiceProvider.overrideWithValue(fake)],
      child: MaterialApp(
        theme: buildTaskveilTheme(Brightness.light),
        localizationsDelegates: AppLocalizations.localizationsDelegates,
        supportedLocales: AppLocalizations.supportedLocales,
        home: AccountScreen(key: UniqueKey()),
      ),
    ),
  );
  await tester.pumpAndSettle();
}

Future<void> _enterCredentials(WidgetTester tester) async {
  await tester.enterText(find.byType(TextField).at(0), 'alice@example.com');
  await tester.enterText(find.byType(TextField).at(1), 'correct password');
}

Finder _accountScrollable() => find.byWidgetPredicate(
  (widget) =>
      widget is Scrollable && widget.axisDirection == AxisDirection.down,
);

void main() {
  testWidgets('shows signed-out account form', (tester) async {
    final fake = FakeBridgeService();
    await _pumpAccountScreen(tester, fake);

    expect(find.text('Account'), findsOneWidget);
    expect(
      find.text('Private sync, security, and account settings.'),
      findsOneWidget,
    );
    expect(find.text('Your encrypted workspace'), findsOneWidget);
    expect(find.text('Server URL'), findsOneWidget);
    expect(find.text('Log in'), findsWidgets);
    expect(find.text('Register'), findsOneWidget);
    expect(
      Theme.of(
        tester.element(find.text('Account')),
      ).textTheme.bodyMedium?.fontFamily,
      'Inter',
    );
    expect(
      Theme.of(tester.element(find.byType(Scaffold))).scaffoldBackgroundColor,
      AppColors.canvas,
    );
  });

  testWidgets('saves sync server URL', (tester) async {
    final fake = FakeBridgeService();
    await _pumpAccountScreen(tester, fake);

    await tester.enterText(
      find.byType(TextField).last,
      'http://127.0.0.1:4000',
    );
    await tester.scrollUntilVisible(
      find.byTooltip('Save server URL'),
      160,
      scrollable: _accountScrollable(),
    );
    await tester.tap(find.byTooltip('Save server URL'));
    await tester.pumpAndSettle();

    expect(await fake.getSyncServerUrl(), 'http://127.0.0.1:4000');
  });

  testWidgets('register shows recovery key once', (tester) async {
    final fake = FakeBridgeService();
    await _pumpAccountScreen(tester, fake);

    await tester.tap(find.text('Register'));
    await tester.pumpAndSettle();
    await tester.enterText(find.byType(TextField).first, 'alice@example.com');
    await tester.tap(find.widgetWithText(FilledButton, 'Register').last);
    await tester.pumpAndSettle();
    await tester.enterText(find.byType(TextField).first, '12345678');
    final verifyCode = find.widgetWithText(FilledButton, 'Verify code').last;
    await tester.ensureVisible(verifyCode);
    await tester.tap(verifyCode);
    await tester.pumpAndSettle();
    expect(find.text('Cancel registration'), findsNothing);
    await tester.enterText(find.byType(TextField).first, 'correct password');
    final finish = find
        .widgetWithText(FilledButton, 'Set password and finish')
        .last;
    await tester.ensureVisible(finish);
    await tester.tap(finish);
    await tester.pumpAndSettle();

    expect(find.byKey(const ValueKey('account-recovery-key')), findsOneWidget);
    expect(find.text('alice@example.com'), findsOneWidget);

    await _pumpAccountScreen(tester, fake);
    expect(find.byKey(const ValueKey('account-recovery-key')), findsOneWidget);

    await tester.scrollUntilVisible(
      find.byKey(const ValueKey('account-logout')),
      160,
      scrollable: _accountScrollable().first,
    );
    await tester.tap(find.byKey(const ValueKey('account-logout')));
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('account-recovery-key')), findsOneWidget);

    await tester.scrollUntilVisible(
      find.text('I saved my Recovery Key'),
      160,
      scrollable: _accountScrollable().first,
    );
    await tester.tap(find.text('I saved my Recovery Key'));
    await tester.pumpAndSettle();
    await _pumpAccountScreen(tester, fake);

    expect(find.text('alice@example.com'), findsOneWidget);
    expect(find.byKey(const ValueKey('account-recovery-key')), findsNothing);
  });

  testWidgets('register announces mailbox verification while pending', (
    tester,
  ) async {
    final fake = FakeBridgeService();
    await _pumpAccountScreen(tester, fake);

    await tester.tap(find.text('Register'));
    await tester.pumpAndSettle();
    await tester.enterText(find.byType(TextField).first, 'alice@example.com');
    await tester.tap(find.widgetWithText(FilledButton, 'Register').last);
    await tester.pumpAndSettle();

    final pending = find.byKey(const ValueKey('account-verification-pending'));
    expect(pending, findsOneWidget);
    expect(tester.getSemantics(pending).flagsCollection.isLiveRegion, isTrue);

    expect(find.text('Resend code'), findsOneWidget);
  });

  for (final restored in [
    ('email', 'Register'),
    ('otp', 'Verify code'),
    ('password', 'Set password and finish'),
  ]) {
    testWidgets('restores ${restored.$1} registration phase after restart', (
      tester,
    ) async {
      final now = DateTime.now().millisecondsSinceEpoch;
      final fake = FakeBridgeService(
        restoredRegistrationState: AccountRegistrationStateDto(
          phase: restored.$1,
          email: 'restored@example.com',
          expiresAtMs: now + 300000,
          nextRetryAtMs: restored.$1 == 'otp' ? now : null,
          canCancel: restored.$1 != 'password',
        ),
      );
      await _pumpAccountScreen(tester, fake);

      expect(find.text(restored.$2), findsWidgets);
      if (restored.$1 == 'email') {
        final email = tester.widget<TextField>(find.byType(TextField).first);
        expect(email.controller?.text, 'restored@example.com');
      }
    });
  }

  testWidgets('registration restore failure is fail-closed and retryable', (
    tester,
  ) async {
    final now = DateTime.now().millisecondsSinceEpoch;
    final fake = FakeBridgeService(
      registrationStateFailures: 1,
      restoredRegistrationState: AccountRegistrationStateDto(
        phase: 'otp',
        email: 'restored@example.com',
        expiresAtMs: now + 300000,
        nextRetryAtMs: now,
        canCancel: true,
      ),
    );
    await _pumpAccountScreen(tester, fake);

    expect(find.text('Could not load account state.'), findsOneWidget);
    expect(find.text('Try again'), findsOneWidget);
    expect(find.byType(TextField), findsNothing);

    await tester.tap(find.text('Try again'));
    await tester.pumpAndSettle();
    expect(
      find.byKey(const ValueKey('account-verification-pending')),
      findsOneWidget,
    );
  });

  testWidgets('Recovery Key restore failure blocks normal UI until retry', (
    tester,
  ) async {
    final fake = FakeBridgeService(recoveryKeyFailures: 1);
    fake.seedRecoveryPendingAccount('recovery@example.com');
    await _pumpAccountScreen(tester, fake);

    expect(find.text('Could not load account state.'), findsOneWidget);
    expect(find.byKey(const ValueKey('account-logout')), findsNothing);
    await tester.tap(find.text('Try again'));
    await tester.pumpAndSettle();

    expect(find.byKey(const ValueKey('account-recovery-key')), findsOneWidget);
  });

  testWidgets('OTP resend deadline is visible and disables resend', (
    tester,
  ) async {
    final now = DateTime.now().millisecondsSinceEpoch;
    final fake = FakeBridgeService(
      restoredRegistrationState: AccountRegistrationStateDto(
        phase: 'otp',
        email: 'restored@example.com',
        expiresAtMs: now + 300000,
        nextRetryAtMs: now + 5000,
        canCancel: true,
      ),
    );
    await _pumpAccountScreen(tester, fake);

    expect(find.textContaining('You can resend in'), findsOneWidget);
    final resend = tester.widget<TextButton>(
      find.widgetWithText(TextButton, 'Resend code'),
    );
    expect(resend.onPressed, isNull);
  });

  testWidgets('login shows email and logout returns to signed-out form', (
    tester,
  ) async {
    final fake = FakeBridgeService();
    await _pumpAccountScreen(tester, fake);

    await _enterCredentials(tester);
    await tester.tap(find.widgetWithText(FilledButton, 'Log in').last);
    await tester.pumpAndSettle();

    expect(find.text('alice@example.com'), findsOneWidget);

    await tester.scrollUntilVisible(
      find.byKey(const ValueKey('account-logout')),
      160,
      scrollable: _accountScrollable(),
    );
    await tester.tap(find.byKey(const ValueKey('account-logout')));
    await tester.pumpAndSettle();

    expect(find.text('Log in'), findsWidgets);
    expect(find.byKey(const ValueKey('account-logout')), findsNothing);
  });

  testWidgets('signed-in account shows sync status and manual sync', (
    tester,
  ) async {
    final fake = FakeBridgeService();
    await _pumpAccountScreen(tester, fake);

    await _enterCredentials(tester);
    await tester.tap(find.widgetWithText(FilledButton, 'Log in').last);
    await tester.pumpAndSettle();

    expect(find.text('Sync'), findsOneWidget);
    expect(find.textContaining('Last synced:'), findsOneWidget);

    final syncNow = find.widgetWithText(OutlinedButton, 'Sync now');
    await tester.scrollUntilVisible(
      syncNow,
      160,
      scrollable: _accountScrollable(),
    );
    await tester.tap(syncNow);
    await tester.pumpAndSettle();

    expect(find.textContaining('Last synced:'), findsOneWidget);
  });

  testWidgets('Safety number requires an out-of-band comparison', (
    tester,
  ) async {
    final fake = FakeBridgeService();
    await _pumpAccountScreen(tester, fake);

    await _enterCredentials(tester);
    await tester.tap(find.widgetWithText(FilledButton, 'Log in').last);
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const ValueKey('organization-safety-open')));
    await tester.pumpAndSettle();

    await tester.enterText(
      find.byKey(const ValueKey('organization-tenant-id')),
      '00000000-0000-4000-8000-000000000001',
    );
    await tester.enterText(
      find.byKey(const ValueKey('organization-member-id')),
      '00000000-0000-4000-8000-000000000002',
    );
    await tester.tap(find.byKey(const ValueKey('organization-safety-load')));
    await tester.pumpAndSettle();

    expect(
      find.byKey(const ValueKey('organization-safety-number')),
      findsOneWidget,
    );
    expect(
      find.byKey(const ValueKey('organization-safety-qr')),
      findsOneWidget,
    );
    final confirm = tester.widget<FilledButton>(
      find.byKey(const ValueKey('organization-safety-confirm')),
    );
    expect(confirm.onPressed, isNull);

    await tester.ensureVisible(find.byType(Checkbox));
    await tester.tap(find.byType(Checkbox));
    await tester.pumpAndSettle();
    await tester.ensureVisible(
      find.byKey(const ValueKey('organization-safety-confirm')),
    );
    await tester.tap(find.byKey(const ValueKey('organization-safety-confirm')));
    await tester.pumpAndSettle();

    expect(fake.organizationSafetyConfirmCalls, 1);
  });
}
