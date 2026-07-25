import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';
import 'package:path_provider/path_provider.dart';
import 'package:taskveil/src/rust/api.dart';
import 'package:taskveil/src/rust/frb_generated.dart';

const _phase = String.fromEnvironment('TASKVEIL_ANDROID_PARITY_PHASE');
const _runId = String.fromEnvironment('TASKVEIL_ANDROID_PARITY_RUN_ID');
const _email = String.fromEnvironment('TASKVEIL_ANDROID_PARITY_EMAIL');
const _password = String.fromEnvironment('TASKVEIL_ANDROID_PARITY_PASSWORD');
const _serverUrl = String.fromEnvironment(
  'TASKVEIL_ANDROID_PARITY_SERVER_URL',
  defaultValue: 'http://127.0.0.1:8080',
);

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('registers, logs in, and synchronizes two Android profiles', (
    tester,
  ) async {
    expect(_runId, isNotEmpty);
    expect(_email, isNotEmpty);
    expect(_password, isNotEmpty);
    expect(const {
      'register',
      'device_a_push',
      'device_b_roundtrip',
      'device_a_verify',
    }, contains(_phase));

    await RustLib.init();
    final support = await getApplicationSupportDirectory();
    final device = _phase == 'device_b_roundtrip' ? 'b' : 'a';
    final profile = Directory(
      '${support.path}/android-account-sync-$_runId-$device',
    );
    await profile.create(recursive: true);
    await initCore(dbDir: profile.path, defaultInboxName: 'Inbox');

    switch (_phase) {
      case 'register':
        final result = await accountRegister(
          email: _email,
          password: _password,
          serverUrl: _serverUrl,
          deviceName: 'Android emulator A',
        );
        expect(result.session.loggedIn, isTrue);
        expect(result.session.email, _email);
        expect(result.recoveryKey, isNotNull);
        return;
      case 'device_a_push':
        final session = await getAccountSessionState();
        expect(session.loggedIn, isTrue);
        final list = await createList(
          name: 'Android parity list $_runId',
          sortOrder: 'unused-by-client',
        );
        await createTask(
          listId: list.id,
          title: 'Created on Android A $_runId',
        );
        final status = await syncNow();
        expect(status.lastError, isNull);
        expect(status.pushedCount, greaterThan(0));
        return;
      case 'device_b_roundtrip':
        final result = await accountLogin(
          email: _email,
          password: _password,
          serverUrl: _serverUrl,
          deviceName: 'Android emulator B',
        );
        expect(result.session.loggedIn, isTrue);
        expect(result.recoveryKey, isNull);
        final pulled = await syncNow();
        expect(pulled.lastError, isNull);
        expect(
          await searchTasks(query: 'Created on Android A $_runId'),
          hasLength(1),
        );
        final lists = await getLists();
        final sharedList = lists.singleWhere(
          (list) => list.name == 'Android parity list $_runId',
        );
        await createTask(
          listId: sharedList.id,
          title: 'Created on Android B $_runId',
        );
        final pushed = await syncNow();
        expect(pushed.lastError, isNull);
        expect(pushed.pushedCount, greaterThan(0));
        return;
      case 'device_a_verify':
        final session = await getAccountSessionState();
        expect(session.loggedIn, isTrue);
        final status = await syncNow();
        expect(status.lastError, isNull);
        expect(
          await searchTasks(query: 'Created on Android B $_runId'),
          hasLength(1),
        );
        return;
    }
  });
}
