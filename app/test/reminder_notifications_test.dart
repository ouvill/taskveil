import 'dart:async';
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
      final (_, task) = await _createListAndTask(fakeBridge);
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
      expect(gateway.scheduled.single.payload.taskId, isNull);
      expect(gateway.scheduled.single.payload.listId, isNull);
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
    final timers = _ManualRetryTimers();
    final (_, task) = await _createListAndTask(fakeBridge);
    final reminder = await fakeBridge.createTaskReminder(
      taskId: task.id,
      remindAt: _futureMs(hours: 1),
    );
    final first = ReminderNotificationService(
      reminderBridge: fakeBridge,
      gateway: gateway,
      retryDelays: const [Duration(seconds: 1)],
      retryTimerFactory: timers.create,
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
      reminderBridge: fakeBridge,
      gateway: gateway,
    );
    first.dispose();
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
      final timers = _ManualRetryTimers();
      final service = ReminderNotificationService(
        reminderBridge: fakeBridge,
        gateway: gateway,
        retryDelays: const [Duration(seconds: 1)],
        retryTimerFactory: timers.create,
      );
      await service.initialize(_content);
      final reminder = await fakeBridge.createTaskReminder(
        taskId: task.id,
        remindAt: _futureMs(hours: 1),
      );
      await service.reconcilePending();
      final platformId = gateway.scheduled.single.notificationId;
      gateway.cancelFailuresRemaining = 1;

      // Mutate through the bridge directly so the provider's fire-and-forget
      // reconciliation cannot consume the injected failure before this test's
      // explicit first service attempt.
      await fakeBridge.deleteReminder(reminderId: reminder.id);
      expect(await fakeBridge.getTaskReminders(taskId: task.id), isEmpty);
      await service.reconcilePending();
      expect(gateway.scheduled.single.notificationId, platformId);

      final restarted = ReminderNotificationService(
        reminderBridge: fakeBridge,
        gateway: gateway,
      );
      service.dispose();
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
      final (_, task) = await _createListAndTask(fakeBridge);
      final reminder = await fakeBridge.createTaskReminder(
        taskId: task.id,
        remindAt: _futureMs(hours: 2),
      );
      final first = ReminderNotificationService(
        reminderBridge: fakeBridge,
        gateway: gateway,
      );
      await first.initialize(_content);
      await first.reconcilePending(rebuild: true);
      final canonicalId = gateway.scheduled.single.notificationId;
      await gateway.schedule(
        notificationId: 2_000_000_000,
        scheduledAt: DateTime.now().add(const Duration(hours: 1)),
        content: _content,
        payload: const ReminderNotificationPayload(
          reminderId: 'removed-reminder',
        ),
      );
      await gateway.schedule(
        notificationId: 1_999_999_999,
        scheduledAt: DateTime.now().add(const Duration(hours: 1)),
        content: _content,
        payload: ReminderNotificationPayload(reminderId: reminder.id),
      );

      final restarted = ReminderNotificationService(
        reminderBridge: fakeBridge,
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
        reminderBridge: fakeBridge,
        gateway: gateway,
      );
      await service.initialize(_content);
      await service.reconcilePending(rebuild: true);
      final platformId = gateway.scheduled.single.notificationId;

      await service.handleResponse(
        ReminderNotificationResponse(
          actionId: reminderSnoozeActionId,
          payload: ReminderNotificationPayload.decode(
            '{"owner":"taskveil_reminder_v1",'
            '"reminderId":"${reminder.id}",'
            '"taskId":"${task.id}","listId":"${list.id}"}',
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
      final (_, task) = await _createListAndTask(fakeBridge);
      final reminder = await fakeBridge.createTaskReminder(
        taskId: task.id,
        remindAt: _futureMs(hours: 1),
      );
      final service = ReminderNotificationService(
        reminderBridge: fakeBridge,
        gateway: gateway,
      );
      await service.initialize(_content);
      await service.reconcilePending(rebuild: true);
      await fakeBridge.setTaskStatus(taskId: task.id, status: 'done');

      await service.handleResponse(
        ReminderNotificationResponse(
          actionId: reminderSnoozeActionId,
          payload: ReminderNotificationPayload(reminderId: reminder.id),
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

  test(
    'transient schedule failure retries in the same foreground service',
    () async {
      final fakeBridge = FakeBridgeService();
      final gateway = _FakeReminderNotificationGateway(
        scheduleFailuresRemaining: 1,
      );
      final timers = _ManualRetryTimers();
      final (_, task) = await _createListAndTask(fakeBridge);
      final reminder = await fakeBridge.createTaskReminder(
        taskId: task.id,
        remindAt: _futureMs(hours: 1),
      );
      final service = ReminderNotificationService(
        reminderBridge: fakeBridge,
        gateway: gateway,
        retryDelays: const [Duration(seconds: 1), Duration(seconds: 2)],
        retryTimerFactory: timers.create,
      );
      addTearDown(service.dispose);
      await service.initialize(_content);

      await service.reconcilePending();
      expect(gateway.scheduled, isEmpty);
      expect(timers.delays, [const Duration(seconds: 1)]);
      expect(timers.activeCount, 1);

      timers.fireNext();
      await service.settleForTesting();

      expect(gateway.scheduled.single.payload.reminderId, reminder.id);
      expect(timers.activeCount, 0);
    },
  );

  test(
    'transient cancel failure retries in the same foreground service',
    () async {
      final fakeBridge = FakeBridgeService();
      final gateway = _FakeReminderNotificationGateway();
      final timers = _ManualRetryTimers();
      final (_, task) = await _createListAndTask(fakeBridge);
      final reminder = await fakeBridge.createTaskReminder(
        taskId: task.id,
        remindAt: _futureMs(hours: 1),
      );
      final service = ReminderNotificationService(
        reminderBridge: fakeBridge,
        gateway: gateway,
        retryDelays: const [Duration(seconds: 1)],
        retryTimerFactory: timers.create,
      );
      addTearDown(service.dispose);
      await service.initialize(_content);
      await service.reconcilePending();
      final platformId = gateway.scheduled.single.notificationId;
      await fakeBridge.deleteReminder(reminderId: reminder.id);
      gateway.cancelFailuresRemaining = 1;

      await service.reconcilePending();
      expect(gateway.scheduled.single.notificationId, platformId);
      expect(timers.activeCount, 1);

      timers.fireNext();
      await service.settleForTesting();

      expect(gateway.scheduled, isEmpty);
      expect(gateway.cancelled, contains(platformId));
    },
  );

  test(
    'cleanup failure keeps rebuild intent and retries in the same service',
    () async {
      final fakeBridge = FakeBridgeService();
      final gateway = _FakeReminderNotificationGateway();
      final timers = _ManualRetryTimers();
      final (_, task) = await _createListAndTask(fakeBridge);
      final reminder = await fakeBridge.createTaskReminder(
        taskId: task.id,
        remindAt: _futureMs(hours: 2),
      );
      final service = ReminderNotificationService(
        reminderBridge: fakeBridge,
        gateway: gateway,
        retryDelays: const [Duration(seconds: 1)],
        retryTimerFactory: timers.create,
      );
      addTearDown(service.dispose);
      await service.initialize(_content);
      await service.reconcilePending(rebuild: true);
      final canonicalId = gateway.scheduled.single.notificationId;
      const orphanId = 2_000_000_000;
      await gateway.schedule(
        notificationId: orphanId,
        scheduledAt: DateTime.now().add(const Duration(hours: 1)),
        content: _content,
        payload: const ReminderNotificationPayload(reminderId: 'orphan'),
      );
      gateway.cancelFailuresRemaining = 1;

      await service.reconcilePending(rebuild: true);
      expect(
        gateway.scheduled.map((notification) => notification.notificationId),
        contains(orphanId),
      );
      expect(timers.activeCount, 1);

      timers.fireNext();
      await service.settleForTesting();

      expect(
        gateway.scheduled.map((notification) => notification.notificationId),
        [canonicalId],
      );
      expect(gateway.cancelled, contains(orphanId));
      expect(gateway.scheduled.single.payload.reminderId, reminder.id);
      expect(gateway.scheduled.single.payload.taskId, isNull);
      expect(gateway.scheduled.single.payload.listId, isNull);
    },
  );

  test(
    'retry budget is bounded and foreground resume resets it without leaks',
    () async {
      final fakeBridge = FakeBridgeService();
      final gateway = _FakeReminderNotificationGateway(
        scheduleFailuresRemaining: 4,
      );
      final timers = _ManualRetryTimers();
      final (_, task) = await _createListAndTask(fakeBridge);
      final reminder = await fakeBridge.createTaskReminder(
        taskId: task.id,
        remindAt: _futureMs(hours: 1),
      );
      final service = ReminderNotificationService(
        reminderBridge: fakeBridge,
        gateway: gateway,
        retryDelays: const [Duration(seconds: 1), Duration(seconds: 2)],
        retryTimerFactory: timers.create,
      );
      await service.initialize(_content);

      await service.reconcilePending();
      expect(timers.activeCount, 1);
      service.setForeground(false);
      expect(timers.activeCount, 0);
      service.setForeground(true);
      await service.settleForTesting();
      timers.fireNext();
      await service.settleForTesting();
      timers.fireNext();
      await service.settleForTesting();

      expect(timers.delays, const [
        Duration(seconds: 1),
        Duration(seconds: 1),
        Duration(seconds: 2),
      ]);
      expect(timers.activeCount, 0);
      expect(gateway.scheduled, isEmpty);

      gateway.scheduleFailuresRemaining = 0;
      service.setForeground(false);
      service.setForeground(true);
      await service.settleForTesting();
      expect(gateway.scheduled, hasLength(1));

      gateway.scheduleFailuresRemaining = 1;
      await fakeBridge.updateReminder(
        reminderId: reminder.id,
        remindAt: _futureMs(hours: 2),
      );
      await service.reconcilePending();
      expect(timers.activeCount, 1);
      service.dispose();
      expect(timers.activeCount, 0);
      service.requestReconciliation(rebuild: true);
      expect(timers.activeCount, 0);
    },
  );

  test('permission success requests reconciliation immediately', () async {
    final fakeBridge = FakeBridgeService();
    final gateway = _FakeReminderNotificationGateway(permissionsGranted: false);
    final timers = _ManualRetryTimers();
    final (_, task) = await _createListAndTask(fakeBridge);
    await fakeBridge.createTaskReminder(
      taskId: task.id,
      remindAt: _futureMs(hours: 1),
    );
    final service = ReminderNotificationService(
      reminderBridge: fakeBridge,
      gateway: gateway,
      retryDelays: const [Duration(seconds: 1)],
      retryTimerFactory: timers.create,
    );
    addTearDown(service.dispose);
    await service.initialize(_content);
    await service.reconcilePending();
    expect(timers.activeCount, 1);

    gateway.permissionsGranted = true;
    expect(await service.requestPermissions(), isTrue);
    await service.settleForTesting();

    expect(gateway.scheduled, hasLength(1));
    expect(timers.activeCount, 0);
  });

  test('payload rejects unrelated and malformed notification ownership', () {
    expect(ReminderNotificationPayload.decode(null), isNull);
    expect(ReminderNotificationPayload.decode('{}'), isNull);
    expect(
      ReminderNotificationPayload.decode(
        '{"owner":"timer","reminderId":"r","taskId":"t","listId":"l"}',
      ),
      isNull,
    );
    const payload = ReminderNotificationPayload(reminderId: 'r');
    expect(
      ReminderNotificationPayload.decode(payload.encode())?.reminderId,
      'r',
    );
    expect(payload.encode(), isNot(contains('taskId')));
    expect(payload.encode(), isNot(contains('listId')));
    final ownedLegacy = ReminderNotificationPayload.decode(
      '{"owner":"taskveil_reminder_v1","reminderId":"r",'
      '"taskId":"t","listId":"l"}',
    );
    expect(ownedLegacy?.taskId, 't');
    expect(ownedLegacy?.listId, 'l');
    final ownerlessLegacy = ReminderNotificationPayload.decodeLegacy(
      '{"reminderId":"r","taskId":"t","listId":"l"}',
    );
    expect(ownerlessLegacy?.reminderId, 'r');
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

  bool permissionsGranted;
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

class _ManualRetryTimers {
  final List<Duration> delays = [];
  final List<_ManualRetryTimer> _timers = [];

  Timer create(Duration delay, void Function() callback) {
    delays.add(delay);
    final timer = _ManualRetryTimer(callback);
    _timers.add(timer);
    return timer;
  }

  int get activeCount => _timers.where((timer) => timer.isActive).length;

  void fireNext() {
    _timers.firstWhere((timer) => timer.isActive).fire();
  }
}

class _ManualRetryTimer implements Timer {
  _ManualRetryTimer(this._callback);

  final void Function() _callback;
  var _active = true;

  void fire() {
    if (!_active) {
      return;
    }
    _active = false;
    _callback();
  }

  @override
  void cancel() => _active = false;

  @override
  bool get isActive => _active;

  @override
  int get tick => _active ? 0 : 1;
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
