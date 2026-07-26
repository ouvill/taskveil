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

const _typedFailure = BridgeErrorDto(
  code: BridgeErrorCodeDto.accountBoundUnavailable,
  arguments: [],
  retryable: false,
);

enum _AccountFailurePoint {
  accountLoad,
  serverUrlLoad,
  serverUrlSave,
  register,
  login,
  logout,
  organizationSafety,
  syncStatus,
}

class _FailingAccountBridgeService extends FakeBridgeService {
  _FailingAccountBridgeService(this.point, {this.failure = _typedFailure});

  final _AccountFailurePoint point;
  final Object failure;

  Never _fail() => throw failure;

  @override
  Future<AccountSessionStateDto> getAccountSessionState() async {
    if (point == _AccountFailurePoint.accountLoad) _fail();
    return super.getAccountSessionState();
  }

  @override
  Future<String> getSyncServerUrl() async {
    if (point == _AccountFailurePoint.serverUrlLoad) _fail();
    return super.getSyncServerUrl();
  }

  @override
  Future<void> setSyncServerUrl({required String serverUrl}) async {
    if (point == _AccountFailurePoint.serverUrlSave) _fail();
    return super.setSyncServerUrl(serverUrl: serverUrl);
  }

  @override
  Future<AccountAuthResultDto> accountRegister({
    required String email,
    required String password,
    String? serverUrl,
    String? deviceName,
  }) async {
    if (point == _AccountFailurePoint.register) _fail();
    return super.accountRegister(
      email: email,
      password: password,
      serverUrl: serverUrl,
      deviceName: deviceName,
    );
  }

  @override
  Future<AccountAuthResultDto> accountLogin({
    required String email,
    required String password,
    String? serverUrl,
    String? deviceName,
  }) async {
    if (point == _AccountFailurePoint.login) _fail();
    return super.accountLogin(
      email: email,
      password: password,
      serverUrl: serverUrl,
      deviceName: deviceName,
    );
  }

  @override
  Future<void> accountLogout() async {
    if (point == _AccountFailurePoint.logout) _fail();
    return super.accountLogout();
  }

  @override
  Future<OrganizationSafetyStateDto> organizationSafetyNumber({
    required String tenantId,
    required String memberUserId,
  }) async {
    if (point == _AccountFailurePoint.organizationSafety) _fail();
    return super.organizationSafetyNumber(
      tenantId: tenantId,
      memberUserId: memberUserId,
    );
  }

  @override
  Future<SyncStatusDto> getSyncStatus() async {
    if (point == _AccountFailurePoint.syncStatus) {
      return _syncFailureStatus;
    }
    return super.getSyncStatus();
  }

  @override
  Future<SyncStatusDto> syncNow() async {
    if (point == _AccountFailurePoint.syncStatus) {
      return _syncFailureStatus;
    }
    return super.syncNow();
  }

  static const _syncFailureStatus = SyncStatusDto(
    loggedIn: true,
    running: false,
    lastError: _typedFailure,
    pushedCount: 0,
    pushAckedCount: 0,
    pushSupersededCount: 0,
    pulledCount: 0,
    appliedCount: 0,
    deletedCount: 0,
    decryptFailedCount: 0,
    repushCount: 0,
    missingKeyQuarantinedCount: 0,
    corruptionQuarantinedCount: 0,
    resolvedQuarantineCount: 0,
    upgradeRequired: false,
  );
}

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

  final typedFailureCases =
      <
        ({
          String name,
          _AccountFailurePoint point,
          bool signedIn,
          Future<void> Function(WidgetTester tester) act,
        })
      >[
        (
          name: 'account load',
          point: _AccountFailurePoint.accountLoad,
          signedIn: false,
          act: (_) async {},
        ),
        (
          name: 'server URL load',
          point: _AccountFailurePoint.serverUrlLoad,
          signedIn: false,
          act: (_) async {},
        ),
        (
          name: 'server URL save',
          point: _AccountFailurePoint.serverUrlSave,
          signedIn: false,
          act: (tester) async {
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
          },
        ),
        (
          name: 'registration',
          point: _AccountFailurePoint.register,
          signedIn: false,
          act: (tester) async {
            await tester.tap(find.text('Register'));
            await tester.pumpAndSettle();
            await _enterCredentials(tester);
            await tester.tap(
              find.widgetWithText(FilledButton, 'Register').last,
            );
          },
        ),
        (
          name: 'login',
          point: _AccountFailurePoint.login,
          signedIn: false,
          act: (tester) async {
            await _enterCredentials(tester);
            await tester.tap(find.widgetWithText(FilledButton, 'Log in').last);
          },
        ),
        (
          name: 'logout',
          point: _AccountFailurePoint.logout,
          signedIn: true,
          act: (tester) async {
            await tester.scrollUntilVisible(
              find.byKey(const ValueKey('account-logout')),
              160,
              scrollable: _accountScrollable(),
            );
            await tester.tap(find.byKey(const ValueKey('account-logout')));
          },
        ),
        (
          name: 'organization safety',
          point: _AccountFailurePoint.organizationSafety,
          signedIn: true,
          act: (tester) async {
            await tester.tap(
              find.byKey(const ValueKey('organization-safety-open')),
            );
            await tester.pumpAndSettle();
            await tester.enterText(
              find.byKey(const ValueKey('organization-tenant-id')),
              'tenant',
            );
            await tester.enterText(
              find.byKey(const ValueKey('organization-member-id')),
              'member',
            );
            await tester.tap(
              find.byKey(const ValueKey('organization-safety-load')),
            );
          },
        ),
        (
          name: 'sync status',
          point: _AccountFailurePoint.syncStatus,
          signedIn: true,
          act: (_) async {},
        ),
      ];

  for (final testCase in typedFailureCases) {
    testWidgets('${testCase.name} localizes the typed bridge failure', (
      tester,
    ) async {
      final fake = _FailingAccountBridgeService(testCase.point);
      if (testCase.signedIn) {
        await fake.accountLogin(
          email: 'alice@example.com',
          password: 'correct password',
        );
      }
      await _pumpAccountScreen(tester, fake);
      await testCase.act(tester);
      await tester.pumpAndSettle();

      final text = tester
          .widgetList<Text>(find.byType(Text))
          .map((widget) => widget.data)
          .whereType<String>();
      expect(
        text,
        contains(
          'This device cannot access the account encryption keys. '
          'Sign in again.',
        ),
        reason: testCase.name,
      );
    });
  }

  testWidgets(
    'unknown account failure is fixed internal copy without payload',
    (tester) async {
      const secret = '/private/profile/alice/taskveil.db?token=secret';
      final fake = _FailingAccountBridgeService(
        _AccountFailurePoint.login,
        failure: StateError(secret),
      );
      await _pumpAccountScreen(tester, fake);
      await _enterCredentials(tester);
      await tester.tap(find.widgetWithText(FilledButton, 'Log in').last);
      await tester.pumpAndSettle();

      expect(
        find.text('Taskveil could not complete the operation.'),
        findsOneWidget,
      );
      expect(find.textContaining(secret), findsNothing);
    },
  );
}
