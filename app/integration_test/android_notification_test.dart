import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';
import 'package:taskveil/src/notifications/reminder_notifications.dart';

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  testWidgets(
    'initializes Android notifications and schedules a snoozable reminder',
    (tester) async {
      final gateway = FlutterLocalReminderNotificationGateway();
      await gateway.initialize(
        snoozeActionTitle: '+1 hour',
        onResponse: (_) async {},
      );

      expect(await gateway.requestPermissions(), isTrue);

      const notificationId = 19072501;
      const payload = ReminderNotificationPayload(
        reminderId: 'android-emulator-reminder',
        taskId: 'android-emulator-task',
        listId: 'android-emulator-list',
      );
      addTearDown(() => gateway.cancel(notificationId));

      await gateway.schedule(
        notificationId: notificationId,
        scheduledAt: DateTime.now().add(const Duration(minutes: 10)),
        content: const ReminderNotificationContent(
          title: 'Taskveil reminder',
          body: 'A task reminder is due.',
          snoozeActionTitle: '+1 hour',
        ),
        payload: payload,
      );

      final pending = await gateway.pendingReminderNotifications();
      expect(
        pending,
        contains(
          isA<PendingReminderNotification>()
              .having(
                (notification) => notification.notificationId,
                'notificationId',
                notificationId,
              )
              .having(
                (notification) => notification.payload.reminderId,
                'reminderId',
                payload.reminderId,
              ),
        ),
      );
    },
  );
}
