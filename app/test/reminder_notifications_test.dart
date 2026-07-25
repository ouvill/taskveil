import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:taskveil/src/core/providers.dart';
import 'package:taskveil/src/notifications/reminder_notifications.dart';

import 'support/fake_bridge_service.dart';

void main() {
  test(
    'reminder provider saves schedules and clears local notifications',
    () async {
      final fakeBridge = FakeBridgeService();
      final gateway = _FakeReminderNotificationGateway();
      final list = await fakeBridge.createDefaultList(
        name: 'Inbox',
        sortOrder: 'a0',
      );
      final task = await fakeBridge.createTask(
        listId: list.id,
        title: 'Schedule reminder',
      );
      final container = ProviderContainer(
        overrides: [
          bridgeServiceProvider.overrideWithValue(fakeBridge),
          reminderNotificationGatewayProvider.overrideWithValue(gateway),
        ],
      );
      addTearDown(container.dispose);

      final service = container.read(reminderNotificationServiceProvider);
      final content = _content;
      final permission = await service.requestPermissions();
      final reminder = await container
          .read(taskRemindersProvider(task.id).notifier)
          .createReminder(
            DateTime.now().add(const Duration(hours: 1)).millisecondsSinceEpoch,
          );
      if (permission) {
        await service.scheduleReminder(
          reminder: reminder,
          listId: list.id,
          content: content,
        );
      }

      expect(gateway.permissionRequests, 1);
      expect(gateway.scheduled.single.payload.reminderId, reminder.id);
      expect(await fakeBridge.getTaskReminders(taskId: task.id), [reminder]);

      await container
          .read(taskRemindersProvider(task.id).notifier)
          .clearReminders();

      expect(gateway.cancelled, [notificationIdForReminder(reminder.id)]);
      expect(await fakeBridge.getTaskReminders(taskId: task.id), isEmpty);
    },
  );

  test(
    'permission denial saves the reminder without scheduling plugin work',
    () async {
      final fakeBridge = FakeBridgeService();
      final gateway = _FakeReminderNotificationGateway(
        permissionsGranted: false,
      );
      final list = await fakeBridge.createDefaultList(
        name: 'Inbox',
        sortOrder: 'a0',
      );
      final task = await fakeBridge.createTask(
        listId: list.id,
        title: 'Denied reminder',
      );
      final container = ProviderContainer(
        overrides: [
          bridgeServiceProvider.overrideWithValue(fakeBridge),
          reminderNotificationGatewayProvider.overrideWithValue(gateway),
        ],
      );
      addTearDown(container.dispose);

      final permission = await container
          .read(reminderNotificationServiceProvider)
          .requestPermissions();
      final reminder = await container
          .read(taskRemindersProvider(task.id).notifier)
          .createReminder(
            DateTime.now().add(const Duration(hours: 1)).millisecondsSinceEpoch,
          );

      expect(permission, isFalse);
      expect(await fakeBridge.getTaskReminders(taskId: task.id), [reminder]);
      expect(gateway.scheduled, isEmpty);
    },
  );

  test('permission request failures degrade to a denied result', () async {
    final service = ReminderNotificationService(
      bridge: FakeBridgeService(),
      gateway: _FakeReminderNotificationGateway(permissionRequestFails: true),
    );

    expect(await service.requestPermissions(), isFalse);
  });

  test('multiple reminders update and delete independently', () async {
    final fakeBridge = FakeBridgeService();
    final gateway = _FakeReminderNotificationGateway();
    final list = await fakeBridge.createDefaultList(
      name: 'Inbox',
      sortOrder: 'a0',
    );
    final task = await fakeBridge.createTask(
      listId: list.id,
      title: 'Multiple reminders',
    );
    final container = ProviderContainer(
      overrides: [
        bridgeServiceProvider.overrideWithValue(fakeBridge),
        reminderNotificationGatewayProvider.overrideWithValue(gateway),
      ],
    );
    addTearDown(container.dispose);
    final notifier = container.read(taskRemindersProvider(task.id).notifier);
    final service = container.read(reminderNotificationServiceProvider);
    final now = DateTime.now();
    final first = await notifier.createReminder(
      now.add(const Duration(minutes: 30)).millisecondsSinceEpoch,
    );
    final second = await notifier.createReminder(
      now.add(const Duration(hours: 1)).millisecondsSinceEpoch,
    );
    final third = await notifier.createReminder(
      now.add(const Duration(hours: 2)).millisecondsSinceEpoch,
    );
    await service.scheduleReminder(
      reminder: first,
      listId: list.id,
      content: _content,
    );
    await service.scheduleReminder(
      reminder: second,
      listId: list.id,
      content: _content,
    );
    await service.scheduleReminder(
      reminder: third,
      listId: list.id,
      content: _content,
    );

    expect(
      gateway.scheduled
          .map((notification) => notification.notificationId)
          .toSet(),
      hasLength(3),
    );

    final updated = await notifier.updateReminder(
      first.id,
      now.add(const Duration(minutes: 45)).millisecondsSinceEpoch,
    );
    await service.scheduleReminder(
      reminder: updated,
      listId: list.id,
      content: _content,
    );
    await notifier.deleteReminder(second.id);

    expect(updated.id, first.id);
    expect(
      gateway.scheduled
          .where(
            (notification) =>
                notification.notificationId ==
                notificationIdForReminder(first.id),
          )
          .length,
      2,
    );
    expect(gateway.cancelled, [notificationIdForReminder(second.id)]);
    expect(await fakeBridge.getTaskReminders(taskId: task.id), [
      updated,
      third,
    ]);
  });

  test(
    'closing cancels all reminders and reopening schedules future ones',
    () async {
      final fakeBridge = FakeBridgeService();
      final gateway = _FakeReminderNotificationGateway();
      final list = await fakeBridge.createDefaultList(
        name: 'Inbox',
        sortOrder: 'a0',
      );
      final task = await fakeBridge.createTask(
        listId: list.id,
        title: 'Close and reopen reminders',
      );
      final now = DateTime.now();
      final first = await fakeBridge.createTaskReminder(
        taskId: task.id,
        remindAt: now.add(const Duration(hours: 1)).millisecondsSinceEpoch,
      );
      final second = await fakeBridge.createTaskReminder(
        taskId: task.id,
        remindAt: now.add(const Duration(hours: 2)).millisecondsSinceEpoch,
      );
      final container = ProviderContainer(
        overrides: [
          bridgeServiceProvider.overrideWithValue(fakeBridge),
          reminderNotificationGatewayProvider.overrideWithValue(gateway),
        ],
      );
      addTearDown(container.dispose);
      final service = container.read(reminderNotificationServiceProvider);
      await service.initialize(_content);
      await service.scheduleReminder(
        reminder: first,
        listId: list.id,
        content: _content,
      );
      await service.scheduleReminder(
        reminder: second,
        listId: list.id,
        content: _content,
      );
      await container.read(tasksProvider(list.id).future);
      final notifier = container.read(tasksProvider(list.id).notifier);

      await notifier.setStatus(task.id, 'done');

      expect(gateway.scheduled, isEmpty);
      expect(gateway.cancelled.toSet(), {
        notificationIdForReminder(first.id),
        notificationIdForReminder(second.id),
      });

      gateway.cancelled.clear();
      await notifier.setStatus(task.id, 'todo');

      expect(
        gateway.scheduled.map(
          (notification) => notification.payload.reminderId,
        ),
        containsAll(<String>[first.id, second.id]),
      );
      expect(gateway.cancelled, isEmpty);
    },
  );

  test('completion undo schedules future reminders again', () async {
    final fakeBridge = FakeBridgeService();
    final gateway = _FakeReminderNotificationGateway();
    final list = await fakeBridge.createDefaultList(
      name: 'Inbox',
      sortOrder: 'a0',
    );
    final task = await fakeBridge.createTask(
      listId: list.id,
      title: 'Undo reminder completion',
    );
    final reminder = await fakeBridge.createTaskReminder(
      taskId: task.id,
      remindAt: DateTime.now()
          .add(const Duration(hours: 1))
          .millisecondsSinceEpoch,
    );
    final container = ProviderContainer(
      overrides: [
        bridgeServiceProvider.overrideWithValue(fakeBridge),
        reminderNotificationGatewayProvider.overrideWithValue(gateway),
      ],
    );
    addTearDown(container.dispose);
    final service = container.read(reminderNotificationServiceProvider);
    await service.initialize(_content);
    await service.scheduleReminder(
      reminder: reminder,
      listId: list.id,
      content: _content,
    );
    await container.read(tasksProvider(list.id).future);

    await container
        .read(tasksProvider(list.id).notifier)
        .setStatus(task.id, 'done');
    final undo = await container.read(latestTaskUndoProvider.future);
    expect(gateway.scheduled, isEmpty);
    expect(undo, isNotNull);

    await container.read(latestTaskUndoProvider.notifier).undo(undo!.id);

    expect(gateway.scheduled, hasLength(1));
    expect(gateway.scheduled.single.payload.reminderId, reminder.id);
  });

  test('non-reminder and malformed payloads are not recognized', () {
    expect(
      ReminderNotificationPayload.decode(
        '{"type":"timer","timerSessionId":"timer-1"}',
      ),
      isNull,
    );
    expect(
      ReminderNotificationPayload.decode(
        '{"reminderId":"r","taskId":"t","listId":"l"}',
      ),
      isNull,
    );
    expect(
      ReminderNotificationPayload.decodeLegacy(
        '{"reminderId":"r","taskId":"t","listId":"l"}',
      ),
      isNotNull,
    );
    expect(ReminderNotificationPayload.decode('not-json'), isNull);
  });

  test(
    'snooze notification action updates reminder and reschedules it',
    () async {
      final fakeBridge = FakeBridgeService();
      final gateway = _FakeReminderNotificationGateway();
      final list = await fakeBridge.createDefaultList(
        name: 'Inbox',
        sortOrder: 'a0',
      );
      final task = await fakeBridge.createTask(
        listId: list.id,
        title: 'Snooze reminder',
      );
      final reminder = await fakeBridge.createTaskReminder(
        taskId: task.id,
        remindAt: DateTime.now()
            .add(const Duration(minutes: 30))
            .millisecondsSinceEpoch,
      );
      final service = ReminderNotificationService(
        bridge: fakeBridge,
        gateway: gateway,
      );
      await service.initialize(_content);

      await service.handleResponse(
        ReminderNotificationResponse(
          actionId: reminderSnoozeActionId,
          payload: ReminderNotificationPayload(
            reminderId: reminder.id,
            taskId: task.id,
            listId: list.id,
          ),
        ),
      );

      final updated = (await fakeBridge.getTaskReminders(
        taskId: task.id,
      )).single;
      expect(updated.snoozedUntil, isNotNull);
      expect(gateway.scheduled.single.payload.reminderId, reminder.id);
      expect(
        gateway.scheduled.single.scheduledAt.millisecondsSinceEpoch,
        updated.snoozedUntil,
      );
    },
  );

  test('startup reschedules pending reminders for open tasks', () async {
    final fakeBridge = FakeBridgeService();
    final gateway = _FakeReminderNotificationGateway();
    final list = await fakeBridge.createDefaultList(
      name: 'Inbox',
      sortOrder: 'a0',
    );
    final task = await fakeBridge.createTask(
      listId: list.id,
      title: 'Startup reminder',
    );
    final reminder = await fakeBridge.createTaskReminder(
      taskId: task.id,
      remindAt: DateTime.now()
          .add(const Duration(hours: 2))
          .millisecondsSinceEpoch,
    );
    final service = ReminderNotificationService(
      bridge: fakeBridge,
      gateway: gateway,
    );

    await service.reconcilePending(_content);

    expect(gateway.scheduled.single.payload.reminderId, reminder.id);
  });

  test(
    'startup reconciliation removes orphan reminder notifications',
    () async {
      final fakeBridge = FakeBridgeService();
      final gateway = _FakeReminderNotificationGateway();
      final list = await fakeBridge.createDefaultList(
        name: 'Inbox',
        sortOrder: 'a0',
      );
      final task = await fakeBridge.createTask(
        listId: list.id,
        title: 'Reconcile reminders',
      );
      final reminder = await fakeBridge.createTaskReminder(
        taskId: task.id,
        remindAt: DateTime.now()
            .add(const Duration(hours: 2))
            .millisecondsSinceEpoch,
      );
      const orphanId = 'removed-reminder';
      await gateway.schedule(
        notificationId: notificationIdForReminder(orphanId),
        scheduledAt: DateTime.now().add(const Duration(hours: 1)),
        content: _content,
        payload: ReminderNotificationPayload(
          reminderId: orphanId,
          taskId: task.id,
          listId: list.id,
        ),
      );
      final wrongNotificationId = notificationIdForReminder(reminder.id) + 1;
      await gateway.schedule(
        notificationId: wrongNotificationId,
        scheduledAt: DateTime.now().add(const Duration(hours: 1)),
        content: _content,
        payload: ReminderNotificationPayload(
          reminderId: reminder.id,
          taskId: task.id,
          listId: list.id,
        ),
      );
      final service = ReminderNotificationService(
        bridge: fakeBridge,
        gateway: gateway,
      );

      await service.reconcilePending(_content);

      expect(gateway.cancelled, [
        notificationIdForReminder(orphanId),
        wrongNotificationId,
      ]);
      expect(gateway.scheduled.single.payload.reminderId, reminder.id);
    },
  );

  test('snooze ignores reminders for closed tasks', () async {
    final fakeBridge = FakeBridgeService();
    final gateway = _FakeReminderNotificationGateway();
    final list = await fakeBridge.createDefaultList(
      name: 'Inbox',
      sortOrder: 'a0',
    );
    final task = await fakeBridge.createTask(
      listId: list.id,
      title: 'Closed reminder',
    );
    final reminder = await fakeBridge.createTaskReminder(
      taskId: task.id,
      remindAt: DateTime.now()
          .add(const Duration(hours: 1))
          .millisecondsSinceEpoch,
    );
    await fakeBridge.setTaskStatus(taskId: task.id, status: 'done');
    final service = ReminderNotificationService(
      bridge: fakeBridge,
      gateway: gateway,
    );
    await service.initialize(_content);

    await service.handleResponse(
      ReminderNotificationResponse(
        actionId: reminderSnoozeActionId,
        payload: ReminderNotificationPayload(
          reminderId: reminder.id,
          taskId: task.id,
          listId: list.id,
        ),
      ),
    );

    expect(gateway.cancelled, [notificationIdForReminder(reminder.id)]);
    expect(
      (await fakeBridge.getTaskReminders(taskId: task.id)).single.snoozedUntil,
      isNull,
    );
  });
}

const _content = ReminderNotificationContent(
  title: 'Taskveil reminder',
  body: 'A task reminder is due.',
  snoozeActionTitle: '+1 hour',
);

class _FakeReminderNotificationGateway implements ReminderNotificationGateway {
  _FakeReminderNotificationGateway({
    this.permissionsGranted = true,
    this.permissionRequestFails = false,
  });

  final bool permissionsGranted;
  final bool permissionRequestFails;
  final scheduled = <_ScheduledReminder>[];
  final cancelled = <int>[];
  int permissionRequests = 0;
  NotificationResponseHandler? responseHandler;

  @override
  Future<ReminderNotificationResponse?> initialize({
    required String snoozeActionTitle,
    required NotificationResponseHandler onResponse,
  }) async {
    responseHandler = onResponse;
    return null;
  }

  @override
  Future<bool> requestPermissions() async {
    permissionRequests += 1;
    if (permissionRequestFails) {
      throw StateError('notification permission request failed');
    }
    return permissionsGranted;
  }

  @override
  Future<void> schedule({
    required int notificationId,
    required DateTime scheduledAt,
    required ReminderNotificationContent content,
    required ReminderNotificationPayload payload,
  }) async {
    scheduled.add(
      _ScheduledReminder(
        notificationId: notificationId,
        scheduledAt: scheduledAt,
        content: content,
        payload: payload,
      ),
    );
  }

  @override
  Future<void> cancel(int notificationId) async {
    cancelled.add(notificationId);
    scheduled.removeWhere(
      (notification) => notification.notificationId == notificationId,
    );
  }

  @override
  Future<List<PendingReminderNotification>>
  pendingReminderNotifications() async {
    final latestById = <int, _ScheduledReminder>{};
    for (final notification in scheduled) {
      latestById[notification.notificationId] = notification;
    }
    return latestById.values
        .map(
          (notification) => PendingReminderNotification(
            notificationId: notification.notificationId,
            payload: notification.payload,
          ),
        )
        .toList(growable: false);
  }
}

class _ScheduledReminder {
  const _ScheduledReminder({
    required this.notificationId,
    required this.scheduledAt,
    required this.content,
    required this.payload,
  });

  final int notificationId;
  final DateTime scheduledAt;
  final ReminderNotificationContent content;
  final ReminderNotificationPayload payload;
}
