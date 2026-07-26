import 'dart:io';

import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:taskveil/src/core/providers.dart';
import 'package:taskveil/src/notifications/reminder_notifications.dart';
import 'package:taskveil/src/rust/api.dart';

import 'support/fake_bridge_service.dart';

void main() {
  test('startup requests reminder rebuild only after runApp', () {
    final source = File('lib/main.dart').readAsStringSync();
    final runAppOffset = source.indexOf('runApp(');
    final postFrameOffset = source.indexOf(
      'WidgetsBinding.instance.addPostFrameCallback',
    );
    final rebuildOffset = source.indexOf(
      'requestReconciliation(rebuild: true)',
    );
    expect(runAppOffset, greaterThanOrEqualTo(0));
    expect(postFrameOffset, greaterThan(runAppOffset));
    expect(rebuildOffset, greaterThan(postFrameOffset));
  });

  test(
    'provider commits reminder state before derived schedule and cancel',
    () async {
      final fakeBridge = FakeBridgeService();
      final gateway = _FakeReminderNotificationGateway();
      final (list, task) = await _createListAndTask(fakeBridge);
      final container = _container(fakeBridge, gateway);
      addTearDown(container.dispose);
      final service = container.read(reminderNotificationServiceProvider);
      await service.initialize(_content);

      final reminder = await container
          .read(taskRemindersProvider(task.id).notifier)
          .createReminder(_futureMs(hours: 1));
      expect(await fakeBridge.getTaskReminders(taskId: task.id), [reminder]);
      await service.reconcilePending();

      expect(gateway.scheduled.single.payload.reminderId, reminder.id);
      expect(gateway.scheduled.single.payload.listId, list.id);
      final platformId = gateway.scheduled.single.notificationId;

      await container
          .read(taskRemindersProvider(task.id).notifier)
          .clearReminders();
      expect(await fakeBridge.getTaskReminders(taskId: task.id), isEmpty);
      await service.reconcilePending();

      expect(gateway.cancelled, [platformId]);
    },
  );

  test(
    'permission denial keeps DB state and durable schedule command',
    () async {
      final fakeBridge = FakeBridgeService();
      final gateway = _FakeReminderNotificationGateway(
        permissionsGranted: false,
      );
      final (_, task) = await _createListAndTask(fakeBridge);
      final container = _container(fakeBridge, gateway);
      addTearDown(container.dispose);
      final service = container.read(reminderNotificationServiceProvider);
      await service.initialize(_content);

      expect(await service.requestPermissions(), isFalse);
      final reminder = await container
          .read(taskRemindersProvider(task.id).notifier)
          .createReminder(_futureMs(hours: 1));
      await service.reconcilePending();

      expect(await fakeBridge.getTaskReminders(taskId: task.id), [reminder]);
      expect(gateway.scheduled, isEmpty);
      expect(
        await fakeBridge.listReminderNotificationCommands(
          nowMs: DateTime.now().millisecondsSinceEpoch,
          limit: 128,
        ),
        hasLength(1),
      );
    },
  );

  test('schedule failure survives service restart and retries', () async {
    final fakeBridge = FakeBridgeService();
    final gateway = _FakeReminderNotificationGateway(
      scheduleFailuresRemaining: 1,
    );
    final (_, task) = await _createListAndTask(fakeBridge);
    final reminder = await fakeBridge.createTaskReminder(
      taskId: task.id,
      remindAt: _futureMs(hours: 1),
    );
    final first = ReminderNotificationService(
      bridge: fakeBridge,
      gateway: gateway,
    );
    await first.initialize(_content);

    await first.reconcilePending();
    expect(gateway.scheduled, isEmpty);
    expect(
      await fakeBridge.listReminderNotificationCommands(
        nowMs: DateTime.now().millisecondsSinceEpoch,
        limit: 128,
      ),
      hasLength(1),
    );

    final restarted = ReminderNotificationService(
      bridge: fakeBridge,
      gateway: gateway,
    );
    await restarted.initialize(_content);
    await restarted.reconcilePending(rebuild: true);

    expect(gateway.scheduled.single.payload.reminderId, reminder.id);
    expect(
      await fakeBridge.listReminderNotificationCommands(
        nowMs: DateTime.now().millisecondsSinceEpoch,
        limit: 128,
      ),
      isEmpty,
    );
  });

  test(
    'cancel failure does not roll back deletion and retries after restart',
    () async {
      final fakeBridge = FakeBridgeService();
      final gateway = _FakeReminderNotificationGateway();
      final (_, task) = await _createListAndTask(fakeBridge);
      final container = _container(fakeBridge, gateway);
      addTearDown(container.dispose);
      final service = container.read(reminderNotificationServiceProvider);
      await service.initialize(_content);
      final reminder = await container
          .read(taskRemindersProvider(task.id).notifier)
          .createReminder(_futureMs(hours: 1));
      await service.reconcilePending();
      final platformId = gateway.scheduled.single.notificationId;
      gateway.cancelFailuresRemaining = 1;

      await container
          .read(taskRemindersProvider(task.id).notifier)
          .deleteReminder(reminder.id);
      expect(await fakeBridge.getTaskReminders(taskId: task.id), isEmpty);
      await service.reconcilePending();
      expect(gateway.scheduled.single.notificationId, platformId);

      final restarted = ReminderNotificationService(
        bridge: fakeBridge,
        gateway: gateway,
      );
      await restarted.initialize(_content);
      await restarted.reconcilePending(rebuild: true);

      expect(gateway.scheduled, isEmpty);
      expect(gateway.cancelled, contains(platformId));
      expect(
        await fakeBridge.listReminderNotificationCommands(
          nowMs: DateTime.now().millisecondsSinceEpoch,
          limit: 128,
        ),
        isEmpty,
      );
    },
  );

  test(
    'task close and reopen converge every reminder through DB commands',
    () async {
      final fakeBridge = FakeBridgeService();
      final gateway = _FakeReminderNotificationGateway();
      final (_, task) = await _createListAndTask(fakeBridge);
      final first = await fakeBridge.createTaskReminder(
        taskId: task.id,
        remindAt: _futureMs(hours: 1),
      );
      final second = await fakeBridge.createTaskReminder(
        taskId: task.id,
        remindAt: _futureMs(hours: 2),
      );
      final container = _container(fakeBridge, gateway);
      addTearDown(container.dispose);
      final service = container.read(reminderNotificationServiceProvider);
      await service.initialize(_content);
      await service.reconcilePending(rebuild: true);
      await container.read(tasksProvider(task.listId).future);
      expect(
        gateway.scheduled.map((value) => value.payload.reminderId).toSet(),
        {first.id, second.id},
      );

      await container
          .read(tasksProvider(task.listId).notifier)
          .setStatus(task.id, 'done');
      await service.reconcilePending();
      expect(gateway.scheduled, isEmpty);

      await container
          .read(tasksProvider(task.listId).notifier)
          .setStatus(task.id, 'todo');
      await service.reconcilePending();
      expect(
        gateway.scheduled.map((value) => value.payload.reminderId).toSet(),
        {first.id, second.id},
      );
    },
  );

  test(
    'startup rebuild removes orphan and noncanonical platform IDs',
    () async {
      final fakeBridge = FakeBridgeService();
      final gateway = _FakeReminderNotificationGateway();
      final (list, task) = await _createListAndTask(fakeBridge);
      final reminder = await fakeBridge.createTaskReminder(
        taskId: task.id,
        remindAt: _futureMs(hours: 2),
      );
      final first = ReminderNotificationService(
        bridge: fakeBridge,
        gateway: gateway,
      );
      await first.initialize(_content);
      await first.reconcilePending(rebuild: true);
      final canonicalId = gateway.scheduled.single.notificationId;
      await gateway.schedule(
        notificationId: 2_000_000_000,
        scheduledAt: DateTime.now().add(const Duration(hours: 1)),
        content: _content,
        payload: ReminderNotificationPayload(
          reminderId: 'removed-reminder',
          taskId: task.id,
          listId: list.id,
        ),
      );
      await gateway.schedule(
        notificationId: 1_999_999_999,
        scheduledAt: DateTime.now().add(const Duration(hours: 1)),
        content: _content,
        payload: ReminderNotificationPayload(
          reminderId: reminder.id,
          taskId: task.id,
          listId: list.id,
        ),
      );

      final restarted = ReminderNotificationService(
        bridge: fakeBridge,
        gateway: gateway,
      );
      await restarted.initialize(_content);
      await restarted.reconcilePending(rebuild: true);

      expect(gateway.cancelled, containsAll([2_000_000_000, 1_999_999_999]));
      expect(gateway.scheduled.single.notificationId, canonicalId);
    },
  );

  test(
    'snooze persists first and reuses the durable platform mapping',
    () async {
      final fakeBridge = FakeBridgeService();
      final gateway = _FakeReminderNotificationGateway();
      final (list, task) = await _createListAndTask(fakeBridge);
      final reminder = await fakeBridge.createTaskReminder(
        taskId: task.id,
        remindAt: _futureMs(hours: 1),
      );
      final service = ReminderNotificationService(
        bridge: fakeBridge,
        gateway: gateway,
      );
      await service.initialize(_content);
      await service.reconcilePending(rebuild: true);
      final platformId = gateway.scheduled.single.notificationId;

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
      await service.reconcilePending();

      final updated = (await fakeBridge.getTaskReminders(
        taskId: task.id,
      )).single;
      expect(updated.snoozedUntil, isNotNull);
      expect(gateway.scheduled.single.notificationId, platformId);
      expect(
        gateway.scheduled.single.scheduledAt.millisecondsSinceEpoch,
        updated.snoozedUntil,
      );
    },
  );

  test(
    'closed-task snooze leaves state unchanged and reconciles cancel',
    () async {
      final fakeBridge = FakeBridgeService();
      final gateway = _FakeReminderNotificationGateway();
      final (list, task) = await _createListAndTask(fakeBridge);
      final reminder = await fakeBridge.createTaskReminder(
        taskId: task.id,
        remindAt: _futureMs(hours: 1),
      );
      final service = ReminderNotificationService(
        bridge: fakeBridge,
        gateway: gateway,
      );
      await service.initialize(_content);
      await service.reconcilePending(rebuild: true);
      await fakeBridge.setTaskStatus(taskId: task.id, status: 'done');

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
      await service.reconcilePending();

      expect(gateway.scheduled, isEmpty);
      expect(
        (await fakeBridge.getTaskReminders(
          taskId: task.id,
        )).single.snoozedUntil,
        isNull,
      );
    },
  );

  test('payload rejects unrelated and malformed notification ownership', () {
    expect(ReminderNotificationPayload.decode(null), isNull);
    expect(ReminderNotificationPayload.decode('{}'), isNull);
    expect(
      ReminderNotificationPayload.decode(
        '{"owner":"timer","reminderId":"r","taskId":"t","listId":"l"}',
      ),
      isNull,
    );
    const payload = ReminderNotificationPayload(
      reminderId: 'r',
      taskId: 't',
      listId: 'l',
    );
    expect(
      ReminderNotificationPayload.decode(payload.encode())?.reminderId,
      'r',
    );
  });
}

ProviderContainer _container(
  FakeBridgeService bridge,
  ReminderNotificationGateway gateway,
) {
  return ProviderContainer(
    overrides: [
      bridgeServiceProvider.overrideWithValue(bridge),
      reminderNotificationGatewayProvider.overrideWithValue(gateway),
    ],
  );
}

Future<(ListDto, TaskDto)> _createListAndTask(FakeBridgeService bridge) async {
  final list = await bridge.createDefaultList(name: 'Inbox', sortOrder: 'a0');
  final task = await bridge.createTask(listId: list.id, title: 'Reminder task');
  return (list, task);
}

int _futureMs({required int hours}) =>
    DateTime.now().add(Duration(hours: hours)).millisecondsSinceEpoch;

const _content = ReminderNotificationContent(
  title: 'Taskveil reminder',
  body: 'A task reminder is due.',
  snoozeActionTitle: '+1 hour',
);

class _FakeReminderNotificationGateway implements ReminderNotificationGateway {
  _FakeReminderNotificationGateway({
    this.permissionsGranted = true,
    this.scheduleFailuresRemaining = 0,
  });

  final bool permissionsGranted;
  int scheduleFailuresRemaining;
  int cancelFailuresRemaining = 0;
  final List<_ScheduledReminder> scheduled = [];
  final List<int> cancelled = [];
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
    return permissionsGranted;
  }

  @override
  Future<void> schedule({
    required int notificationId,
    required DateTime scheduledAt,
    required ReminderNotificationContent content,
    required ReminderNotificationPayload payload,
  }) async {
    if (!permissionsGranted || scheduleFailuresRemaining > 0) {
      if (scheduleFailuresRemaining > 0) {
        scheduleFailuresRemaining -= 1;
      }
      throw Exception('schedule unavailable');
    }
    scheduled.removeWhere(
      (notification) => notification.notificationId == notificationId,
    );
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
    if (cancelFailuresRemaining > 0) {
      cancelFailuresRemaining -= 1;
      throw Exception('cancel unavailable');
    }
    cancelled.add(notificationId);
    scheduled.removeWhere(
      (notification) => notification.notificationId == notificationId,
    );
  }

  @override
  Future<List<PendingReminderNotification>>
  pendingReminderNotifications() async {
    return scheduled
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
