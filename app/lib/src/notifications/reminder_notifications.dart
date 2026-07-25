import 'dart:convert';

import 'package:flutter/foundation.dart';
import 'package:flutter_local_notifications/flutter_local_notifications.dart';
import 'package:taskveil/src/core/bridge_service.dart';
import 'package:taskveil/src/rust/api.dart';
import 'package:timezone/data/latest_all.dart' as tz_data;
import 'package:timezone/timezone.dart' as tz;

const reminderNotificationCategoryId = 'taskveil_reminder_v1';
const reminderSnoozeActionId = 'taskveil_snooze_1h';
const reminderSnoozeDuration = Duration(hours: 1);

typedef NotificationResponseHandler =
    Future<void> Function(ReminderNotificationResponse response);

class ReminderNotificationPayload {
  const ReminderNotificationPayload({
    required this.reminderId,
    required this.taskId,
    required this.listId,
  });

  final String reminderId;
  final String taskId;
  final String listId;

  String encode() => jsonEncode({
    'owner': reminderNotificationCategoryId,
    'reminderId': reminderId,
    'taskId': taskId,
    'listId': listId,
  });

  static ReminderNotificationPayload? decode(String? value) {
    return _decode(value, allowLegacy: false);
  }

  static ReminderNotificationPayload? decodeLegacy(String? value) {
    return _decode(value, allowLegacy: true);
  }

  static ReminderNotificationPayload? _decode(
    String? value, {
    required bool allowLegacy,
  }) {
    if (value == null || value.isEmpty) {
      return null;
    }
    final Object? decoded;
    try {
      decoded = jsonDecode(value);
    } on FormatException {
      return null;
    }
    if (decoded is! Map<String, Object?>) {
      return null;
    }
    final owner = decoded['owner'];
    final isCurrent = owner == reminderNotificationCategoryId;
    final isLegacy =
        allowLegacy &&
        owner == null &&
        decoded.length == 3 &&
        decoded.containsKey('reminderId') &&
        decoded.containsKey('taskId') &&
        decoded.containsKey('listId');
    if (!isCurrent && !isLegacy) {
      return null;
    }
    final reminderId = decoded['reminderId'];
    final taskId = decoded['taskId'];
    final listId = decoded['listId'];
    if (reminderId is! String || taskId is! String || listId is! String) {
      return null;
    }
    return ReminderNotificationPayload(
      reminderId: reminderId,
      taskId: taskId,
      listId: listId,
    );
  }
}

class ReminderNotificationResponse {
  const ReminderNotificationResponse({required this.actionId, this.payload});

  final String actionId;
  final ReminderNotificationPayload? payload;
}

class PendingReminderNotification {
  const PendingReminderNotification({
    required this.notificationId,
    required this.payload,
  });

  final int notificationId;
  final ReminderNotificationPayload payload;
}

class ReminderNotificationContent {
  const ReminderNotificationContent({
    required this.title,
    required this.body,
    required this.snoozeActionTitle,
  });

  final String title;
  final String body;
  final String snoozeActionTitle;
}

abstract class ReminderNotificationGateway {
  Future<ReminderNotificationResponse?> initialize({
    required String snoozeActionTitle,
    required NotificationResponseHandler onResponse,
  });
  Future<bool> requestPermissions();
  Future<void> schedule({
    required int notificationId,
    required DateTime scheduledAt,
    required ReminderNotificationContent content,
    required ReminderNotificationPayload payload,
  });
  Future<void> cancel(int notificationId);
  Future<List<PendingReminderNotification>> pendingReminderNotifications();
}

class FlutterLocalReminderNotificationGateway
    implements ReminderNotificationGateway {
  FlutterLocalReminderNotificationGateway({
    FlutterLocalNotificationsPlugin? plugin,
  }) : _plugin = plugin ?? FlutterLocalNotificationsPlugin();

  final FlutterLocalNotificationsPlugin _plugin;
  bool _timeZonesInitialized = false;

  @override
  Future<ReminderNotificationResponse?> initialize({
    required String snoozeActionTitle,
    required NotificationResponseHandler onResponse,
  }) async {
    _initializeTimeZones();
    final category = DarwinNotificationCategory(
      reminderNotificationCategoryId,
      actions: [
        DarwinNotificationAction.plain(
          reminderSnoozeActionId,
          snoozeActionTitle,
          options: {DarwinNotificationActionOption.foreground},
        ),
      ],
    );
    final settings = InitializationSettings(
      iOS: DarwinInitializationSettings(
        requestAlertPermission: false,
        requestBadgePermission: false,
        requestSoundPermission: false,
        notificationCategories: [category],
      ),
      macOS: DarwinInitializationSettings(
        requestAlertPermission: false,
        requestBadgePermission: false,
        requestSoundPermission: false,
        notificationCategories: [category],
      ),
      android: const AndroidInitializationSettings('@mipmap/ic_launcher'),
    );
    await _plugin.initialize(
      settings: settings,
      onDidReceiveNotificationResponse: (response) {
        onResponse(_fromPluginResponse(response));
      },
    );
    final launchDetails = await _plugin.getNotificationAppLaunchDetails();
    final launchResponse = launchDetails?.notificationResponse;
    return launchResponse == null ? null : _fromPluginResponse(launchResponse);
  }

  @override
  Future<bool> requestPermissions() async {
    final ios = await _plugin
        .resolvePlatformSpecificImplementation<
          IOSFlutterLocalNotificationsPlugin
        >()
        ?.requestPermissions(alert: true, badge: false, sound: true);
    final macos = await _plugin
        .resolvePlatformSpecificImplementation<
          MacOSFlutterLocalNotificationsPlugin
        >()
        ?.requestPermissions(alert: true, badge: false, sound: true);
    final android = await _plugin
        .resolvePlatformSpecificImplementation<
          AndroidFlutterLocalNotificationsPlugin
        >()
        ?.requestNotificationsPermission();
    return ios ?? macos ?? android ?? true;
  }

  @override
  Future<void> schedule({
    required int notificationId,
    required DateTime scheduledAt,
    required ReminderNotificationContent content,
    required ReminderNotificationPayload payload,
  }) async {
    _initializeTimeZones();
    final scheduled = tz.TZDateTime.from(scheduledAt.toLocal(), tz.local);
    await _plugin.zonedSchedule(
      id: notificationId,
      title: content.title,
      body: content.body,
      scheduledDate: scheduled,
      notificationDetails: NotificationDetails(
        iOS: const DarwinNotificationDetails(
          categoryIdentifier: reminderNotificationCategoryId,
        ),
        macOS: const DarwinNotificationDetails(
          categoryIdentifier: reminderNotificationCategoryId,
        ),
        android: AndroidNotificationDetails(
          'taskveil_reminders',
          'Taskveil reminders',
          channelDescription: 'Local reminders scheduled by Taskveil',
          actions: [
            AndroidNotificationAction(
              reminderSnoozeActionId,
              content.snoozeActionTitle,
              showsUserInterface: true,
            ),
          ],
        ),
      ),
      androidScheduleMode: AndroidScheduleMode.inexactAllowWhileIdle,
      payload: payload.encode(),
    );
  }

  @override
  Future<void> cancel(int notificationId) {
    return _plugin.cancel(id: notificationId);
  }

  @override
  Future<List<PendingReminderNotification>>
  pendingReminderNotifications() async {
    final pending = await _plugin.pendingNotificationRequests();
    return pending
        .map((request) {
          final current = ReminderNotificationPayload.decode(request.payload);
          if (current != null) {
            return PendingReminderNotification(
              notificationId: request.id,
              payload: current,
            );
          }
          final legacy = ReminderNotificationPayload.decodeLegacy(
            request.payload,
          );
          if (legacy == null ||
              request.id != notificationIdForReminder(legacy.reminderId)) {
            return null;
          }
          return PendingReminderNotification(
            notificationId: request.id,
            payload: legacy,
          );
        })
        .whereType<PendingReminderNotification>()
        .toList(growable: false);
  }

  void _initializeTimeZones() {
    if (_timeZonesInitialized) {
      return;
    }
    tz_data.initializeTimeZones();
    _timeZonesInitialized = true;
  }
}

class ReminderNotificationService {
  ReminderNotificationService({required this.bridge, required this.gateway});

  final BridgeService bridge;
  final ReminderNotificationGateway gateway;
  ReminderNotificationContent? _content;

  Future<void> initialize(ReminderNotificationContent content) async {
    _content = content;
    final launchResponse = await gateway.initialize(
      snoozeActionTitle: content.snoozeActionTitle,
      onResponse: handleResponse,
    );
    if (launchResponse != null) {
      await handleResponse(launchResponse);
    }
  }

  Future<bool> requestPermissions() async {
    try {
      return await gateway.requestPermissions();
    } catch (_) {
      return false;
    }
  }

  Future<void> scheduleReminder({
    required ReminderDto reminder,
    required String listId,
    required ReminderNotificationContent content,
  }) async {
    final scheduledAt = DateTime.fromMillisecondsSinceEpoch(
      effectiveReminderAt(reminder),
    );
    if (!scheduledAt.isAfter(DateTime.now())) {
      return;
    }
    await gateway.schedule(
      notificationId: notificationIdForReminder(reminder.id),
      scheduledAt: scheduledAt,
      content: content,
      payload: ReminderNotificationPayload(
        reminderId: reminder.id,
        taskId: reminder.taskId,
        listId: listId,
      ),
    );
  }

  Future<void> cancelReminder(ReminderDto reminder) {
    return gateway.cancel(notificationIdForReminder(reminder.id));
  }

  Future<void> cancelReminders(Iterable<ReminderDto> reminders) async {
    for (final reminder in reminders) {
      await cancelReminder(reminder);
    }
  }

  Future<void> scheduleTaskReminders(String taskId) async {
    final content = _content;
    if (content == null) {
      return;
    }
    final task = await _findTask(taskId);
    if (task == null || task.status == 'done' || task.status == 'wont_do') {
      return;
    }
    final reminders = await bridge.getTaskReminders(taskId: taskId);
    var failedCount = 0;
    for (final reminder in reminders) {
      try {
        await scheduleReminder(
          reminder: reminder,
          listId: task.listId,
          content: content,
        );
      } catch (_) {
        failedCount += 1;
      }
    }
    if (failedCount > 0) {
      debugPrint(
        'Taskveil reminder reopen could not schedule '
        '$failedCount notification(s).',
      );
    }
  }

  Future<void> reconcilePending(ReminderNotificationContent content) async {
    final reminders = await bridge.listPendingReminders(
      nowMs: DateTime.now().millisecondsSinceEpoch,
    );
    final reminderIds = reminders.map((reminder) => reminder.id).toSet();
    final scheduled = await gateway.pendingReminderNotifications();
    for (final notification in scheduled) {
      final isCanonicalId =
          notification.notificationId ==
          notificationIdForReminder(notification.payload.reminderId);
      if (!isCanonicalId ||
          !reminderIds.contains(notification.payload.reminderId)) {
        await gateway.cancel(notification.notificationId);
      }
    }
    var failedCount = 0;
    for (final reminder in reminders) {
      final task = await _findTask(reminder.taskId);
      if (task == null) {
        continue;
      }
      try {
        await scheduleReminder(
          reminder: reminder,
          listId: task.listId,
          content: content,
        );
      } catch (_) {
        failedCount += 1;
      }
    }
    if (failedCount > 0) {
      debugPrint(
        'Taskveil reminder reconciliation could not schedule '
        '$failedCount notification(s).',
      );
    }
  }

  Future<void> handleResponse(ReminderNotificationResponse response) async {
    final payload = response.payload;
    final content = _content;
    if (payload == null ||
        content == null ||
        response.actionId != reminderSnoozeActionId) {
      return;
    }
    final task = await _findTask(payload.taskId);
    if (task == null || task.status == 'done' || task.status == 'wont_do') {
      await gateway.cancel(notificationIdForReminder(payload.reminderId));
      return;
    }
    final snoozedUntil = DateTime.now()
        .add(reminderSnoozeDuration)
        .millisecondsSinceEpoch;
    final reminder = await bridge.snoozeReminder(
      reminderId: payload.reminderId,
      snoozedUntil: snoozedUntil,
    );
    await scheduleReminder(
      reminder: reminder,
      listId: payload.listId,
      content: content,
    );
  }

  Future<TaskDto?> _findTask(String taskId) async {
    final lists = await bridge.getLists();
    for (final list in lists) {
      final tasks = await bridge.getTasks(listId: list.id);
      for (final task in tasks) {
        if (task.id == taskId) {
          return task;
        }
      }
    }
    return null;
  }
}

ReminderNotificationResponse reminderResponseFromPlugin(
  NotificationResponse response,
) {
  return _fromPluginResponse(response);
}

ReminderNotificationResponse _fromPluginResponse(
  NotificationResponse response,
) {
  return ReminderNotificationResponse(
    actionId: response.actionId ?? '',
    payload:
        ReminderNotificationPayload.decode(response.payload) ??
        ReminderNotificationPayload.decodeLegacy(response.payload),
  );
}

int effectiveReminderAt(ReminderDto reminder) =>
    reminder.snoozedUntil ?? reminder.remindAt;

@visibleForTesting
int notificationIdForReminder(String reminderId) {
  var hash = 0x811c9dc5;
  for (final codeUnit in reminderId.codeUnits) {
    hash ^= codeUnit;
    hash = (hash * 0x01000193) & 0x7fffffff;
  }
  return hash == 0 ? 1 : hash;
}
