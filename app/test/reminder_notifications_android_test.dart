import 'dart:convert';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter_local_notifications/flutter_local_notifications.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:taskveil/src/notifications/reminder_notifications.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  AndroidFlutterLocalNotificationsPlugin.registerWith();

  const channel = MethodChannel('dexterous.com/flutter/local_notifications');
  final methodCalls = <MethodCall>[];

  setUp(() {
    debugDefaultTargetPlatformOverride = TargetPlatform.android;
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(channel, (call) async {
          methodCalls.add(call);
          return switch (call.method) {
            'initialize' => true,
            'getNotificationAppLaunchDetails' => null,
            'requestNotificationsPermission' => false,
            _ => null,
          };
        });
  });

  tearDown(() {
    debugDefaultTargetPlatformOverride = null;
    methodCalls.clear();
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(channel, null);
  });

  test('Android 13+ notification permission result is returned', () async {
    final gateway = FlutterLocalReminderNotificationGateway(
      plugin: FlutterLocalNotificationsPlugin(),
    );

    expect(await gateway.requestPermissions(), isFalse);
    expect(
      methodCalls,
      contains(isMethodCall('requestNotificationsPermission', arguments: null)),
    );
  });

  test(
    'Android reminder includes foreground snooze action and safe payload',
    () async {
      final gateway = FlutterLocalReminderNotificationGateway(
        plugin: FlutterLocalNotificationsPlugin(),
      );
      await gateway.initialize(
        snoozeActionTitle: '+1 hour',
        onResponse: (_) async {},
      );

      await gateway.schedule(
        notificationId: 7,
        scheduledAt: DateTime.now().add(const Duration(hours: 1)),
        content: const ReminderNotificationContent(
          title: 'Taskveil reminder',
          body: 'A task reminder is due.',
          snoozeActionTitle: '+1 hour',
        ),
        payload: const ReminderNotificationPayload(
          reminderId: 'reminder-id',
          taskId: 'task-id',
          listId: 'list-id',
        ),
      );

      final scheduledCall = methodCalls.singleWhere(
        (call) => call.method == 'zonedSchedule',
      );
      final arguments = scheduledCall.arguments! as Map<Object?, Object?>;
      final platformSpecifics =
          arguments['platformSpecifics']! as Map<Object?, Object?>;
      final actions = platformSpecifics['actions']! as List<Object?>;

      expect(actions, hasLength(1));
      expect(actions.single, containsPair('id', reminderSnoozeActionId));
      expect(actions.single, containsPair('title', '+1 hour'));
      expect(actions.single, containsPair('showsUserInterface', true));

      final payload =
          jsonDecode(arguments['payload']! as String) as Map<String, Object?>;
      expect(payload, {
        'owner': reminderNotificationCategoryId,
        'reminderId': 'reminder-id',
        'taskId': 'task-id',
        'listId': 'list-id',
      });
    },
  );

  test(
    'Android manifest supports permission, actions, and reboot reschedule',
    () {
      final manifest = File(
        'android/app/src/main/AndroidManifest.xml',
      ).readAsStringSync();

      expect(manifest, contains('android.permission.POST_NOTIFICATIONS'));
      expect(manifest, contains('android.permission.RECEIVE_BOOT_COMPLETED'));
      expect(manifest, contains('android.permission.INTERNET'));
      expect(manifest, contains('android:allowBackup="false"'));
      expect(manifest, contains('android:fullBackupContent="false"'));
      expect(
        manifest,
        contains(
          'com.dexterous.flutterlocalnotifications.ActionBroadcastReceiver',
        ),
      );
      expect(
        manifest,
        contains(
          'com.dexterous.flutterlocalnotifications.ScheduledNotificationReceiver',
        ),
      );
      expect(
        manifest,
        contains(
          'com.dexterous.flutterlocalnotifications.'
          'ScheduledNotificationBootReceiver',
        ),
      );
      expect(manifest, contains('android.intent.action.BOOT_COMPLETED'));
      expect(manifest, contains('android.intent.action.MY_PACKAGE_REPLACED'));
    },
  );
}
