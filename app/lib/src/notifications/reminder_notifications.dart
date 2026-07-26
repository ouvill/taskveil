import 'dart:async';
import 'dart:convert';

import 'package:flutter/foundation.dart';
import 'package:flutter_local_notifications/flutter_local_notifications.dart';
import 'package:taskveil/src/core/bridge_ports.dart';
import 'package:taskveil/src/rust/api.dart';
import 'package:timezone/data/latest_all.dart' as tz_data;
import 'package:timezone/timezone.dart' as tz;

const reminderNotificationCategoryId = 'taskveil_reminder_v1';
const reminderSnoozeActionId = 'taskveil_snooze_1h';
const reminderSnoozeDuration = Duration(hours: 1);

typedef NotificationResponseHandler =
    Future<void> Function(ReminderNotificationResponse response);

class ReminderNotificationPayload {
  const ReminderNotificationPayload({required this.reminderId})
    : taskId = null,
      listId = null;

  const ReminderNotificationPayload._legacy({
    required this.reminderId,
    required this.taskId,
    required this.listId,
  });

  final String reminderId;
  final String? taskId;
  final String? listId;

  String encode() => jsonEncode({
    'owner': reminderNotificationCategoryId,
    'reminderId': reminderId,
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
    final isCurrent =
        owner == reminderNotificationCategoryId &&
        decoded.length == 2 &&
        decoded.containsKey('reminderId');
    final isOwnedLegacy =
        owner == reminderNotificationCategoryId &&
        decoded.length == 4 &&
        decoded.containsKey('reminderId') &&
        decoded.containsKey('taskId') &&
        decoded.containsKey('listId');
    final isOwnerlessLegacy =
        allowLegacy &&
        owner == null &&
        decoded.length == 3 &&
        decoded.containsKey('reminderId') &&
        decoded.containsKey('taskId') &&
        decoded.containsKey('listId');
    if (!isCurrent && !isOwnedLegacy && !isOwnerlessLegacy) {
      return null;
    }
    final reminderId = decoded['reminderId'];
    final taskId = decoded['taskId'];
    final listId = decoded['listId'];
    if (reminderId is! String) {
      return null;
    }
    if ((isOwnedLegacy || isOwnerlessLegacy) &&
        (taskId is! String || listId is! String)) {
      return null;
    }
    if (isCurrent) {
      return ReminderNotificationPayload(reminderId: reminderId);
    }
    return ReminderNotificationPayload._legacy(
      reminderId: reminderId,
      taskId: taskId as String,
      listId: listId as String,
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
              request.id != _legacyNotificationId(legacy.reminderId)) {
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
  ReminderNotificationService({
    required this.reminderBridge,
    required this.gateway,
    List<Duration> retryDelays = _defaultRetryDelays,
    this.retryTimerFactory = _systemRetryTimerFactory,
  }) : _retryDelays = List.unmodifiable(retryDelays),
       assert(
         retryDelays.isNotEmpty &&
             retryDelays.every((delay) => delay > Duration.zero),
       );

  final ReminderBridgePort reminderBridge;
  final ReminderNotificationGateway gateway;
  final ReminderRetryTimerFactory retryTimerFactory;
  final List<Duration> _retryDelays;
  ReminderNotificationContent? _content;
  Future<void>? _reconciliation;
  Timer? _retryTimer;
  bool _reconcileAgain = false;
  bool _rebuildRequested = false;
  bool _foreground = true;
  bool _disposed = false;
  int _retryAttempt = 0;

  static const _commandBatchSize = 128;

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
      final granted = await gateway.requestPermissions();
      if (granted) {
        requestReconciliation(rebuild: true);
      }
      return granted;
    } catch (_) {
      return false;
    }
  }

  void requestReconciliation({bool rebuild = false}) {
    _requestReconciliation(rebuild: rebuild, resetRetryBudget: true);
  }

  void setForeground(bool foreground) {
    if (_disposed) {
      return;
    }
    if (!foreground) {
      _foreground = false;
      _retryTimer?.cancel();
      _retryTimer = null;
      return;
    }
    _foreground = true;
    _requestReconciliation(rebuild: true, resetRetryBudget: true);
  }

  void dispose() {
    if (_disposed) {
      return;
    }
    _disposed = true;
    _foreground = false;
    _retryTimer?.cancel();
    _retryTimer = null;
    _reconcileAgain = false;
    _rebuildRequested = false;
  }

  void _requestReconciliation({
    required bool rebuild,
    required bool resetRetryBudget,
  }) {
    if (_disposed) {
      return;
    }
    if (resetRetryBudget) {
      _retryAttempt = 0;
      _retryTimer?.cancel();
      _retryTimer = null;
    }
    _reconcileAgain = true;
    _rebuildRequested = _rebuildRequested || rebuild;
    if (!_foreground) {
      return;
    }
    unawaited(
      _startReconciliation().catchError((Object error) {
        debugPrint('Taskveil reminder reconciliation deferred after failure.');
      }),
    );
  }

  Future<void> reconcilePending({bool rebuild = false}) {
    if (_disposed) {
      return Future.value();
    }
    _retryAttempt = 0;
    _retryTimer?.cancel();
    _retryTimer = null;
    _reconcileAgain = true;
    _rebuildRequested = _rebuildRequested || rebuild;
    if (!_foreground) {
      return Future.value();
    }
    return _startReconciliation();
  }

  @visibleForTesting
  Future<void> settleForTesting() async {
    while (true) {
      final running = _reconciliation;
      if (running == null) {
        return;
      }
      await running;
    }
  }

  Future<void> _startReconciliation() {
    final running = _reconciliation;
    if (running != null) {
      return running;
    }
    final reconciliation = _runReconciliation();
    _reconciliation = reconciliation;
    return reconciliation;
  }

  Future<void> _runReconciliation() async {
    var needsRetry = false;
    var activeRebuild = false;
    try {
      while (_reconcileAgain && _foreground && !_disposed) {
        _reconcileAgain = false;
        final rebuild = _rebuildRequested;
        _rebuildRequested = false;
        activeRebuild = rebuild;
        final outcome = await _reconcileOnce(rebuild: rebuild);
        activeRebuild = false;
        if (outcome.needsRetry) {
          needsRetry = true;
          _reconcileAgain = true;
          _rebuildRequested = _rebuildRequested || outcome.requiresRebuild;
          break;
        }
      }
    } catch (_) {
      needsRetry = true;
      _reconcileAgain = true;
      _rebuildRequested = _rebuildRequested || activeRebuild;
      debugPrint('Taskveil reminder reconciliation deferred after failure.');
    } finally {
      _reconciliation = null;
      if (needsRetry || _reconcileAgain) {
        _scheduleRetry();
      } else {
        _retryAttempt = 0;
      }
    }
  }

  Future<_ReconciliationOutcome> _reconcileOnce({required bool rebuild}) async {
    final content = _content;
    if (content == null) {
      return const _ReconciliationOutcome.complete();
    }
    final nowMs = DateTime.now().millisecondsSinceEpoch;
    final initialCommands = rebuild
        ? await reminderBridge.prepareReminderNotificationReconciliation(
            nowMs: nowMs,
          )
        : await reminderBridge.listReminderNotificationCommands(
            nowMs: nowMs,
            limit: _commandBatchSize,
          );
    final cleanupSucceeded =
        !rebuild ||
        await _removeNoncanonicalPendingNotifications(initialCommands);
    var commands = initialCommands;
    while (commands.isNotEmpty) {
      if (!_foreground || _disposed) {
        return _ReconciliationOutcome.retry(requiresRebuild: rebuild);
      }
      var failedCount = 0;
      var staleAckObserved = false;
      for (final command in commands) {
        if (!_foreground || _disposed) {
          return _ReconciliationOutcome.retry(requiresRebuild: rebuild);
        }
        try {
          await _applyCommand(command, content);
          final acknowledged = await reminderBridge
              .ackReminderNotificationCommand(
                reminderId: command.reminderId,
                revision: command.revision,
              );
          staleAckObserved = staleAckObserved || !acknowledged;
        } catch (_) {
          failedCount += 1;
        }
      }
      if (failedCount > 0) {
        debugPrint(
          'Taskveil reminder reconciliation deferred '
          '$failedCount notification command(s).',
        );
        return _ReconciliationOutcome.retry(requiresRebuild: !cleanupSucceeded);
      }
      if (!staleAckObserved &&
          (rebuild || commands.length < _commandBatchSize)) {
        return cleanupSucceeded
            ? const _ReconciliationOutcome.complete()
            : const _ReconciliationOutcome.retry(requiresRebuild: true);
      }
      commands = await reminderBridge.listReminderNotificationCommands(
        nowMs: DateTime.now().millisecondsSinceEpoch,
        limit: _commandBatchSize,
      );
    }
    return cleanupSucceeded
        ? const _ReconciliationOutcome.complete()
        : const _ReconciliationOutcome.retry(requiresRebuild: true);
  }

  Future<bool> _removeNoncanonicalPendingNotifications(
    List<ReminderNotificationCommandDto> commands,
  ) async {
    final expected = {
      for (final command in commands)
        if (command.action == ReminderNotificationActionDto.schedule)
          command.reminderId: command.platformId,
    };
    final scheduled = await gateway.pendingReminderNotifications();
    var failedCount = 0;
    for (final notification in scheduled) {
      if (!_foreground || _disposed) {
        return false;
      }
      if (expected[notification.payload.reminderId] ==
          notification.notificationId) {
        continue;
      }
      try {
        await gateway.cancel(notification.notificationId);
      } catch (_) {
        failedCount += 1;
      }
    }
    if (failedCount > 0) {
      debugPrint(
        'Taskveil reminder reconciliation deferred '
        '$failedCount stale notification cancellation(s).',
      );
    }
    return failedCount == 0;
  }

  void _scheduleRetry() {
    if (_disposed ||
        !_foreground ||
        _retryTimer != null ||
        _retryAttempt >= _retryDelays.length) {
      return;
    }
    final delay = _retryDelays[_retryAttempt];
    _retryAttempt += 1;
    _retryTimer = retryTimerFactory(delay, () {
      _retryTimer = null;
      if (_disposed || !_foreground) {
        return;
      }
      unawaited(
        _startReconciliation().catchError((Object error) {
          debugPrint(
            'Taskveil reminder reconciliation deferred after failure.',
          );
        }),
      );
    });
  }

  Future<void> _applyCommand(
    ReminderNotificationCommandDto command,
    ReminderNotificationContent content,
  ) async {
    switch (command.action) {
      case ReminderNotificationActionDto.schedule:
        final taskId = command.taskId;
        final listId = command.listId;
        final scheduledAt = command.scheduledAt;
        if (taskId == null || listId == null || scheduledAt == null) {
          throw StateError('schedule command is missing context');
        }
        await gateway.schedule(
          notificationId: command.platformId,
          scheduledAt: DateTime.fromMillisecondsSinceEpoch(scheduledAt),
          content: content,
          payload: ReminderNotificationPayload(reminderId: command.reminderId),
        );
        return;
      case ReminderNotificationActionDto.cancel:
        await gateway.cancel(command.platformId);
        return;
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
    final snoozedUntil = DateTime.now()
        .add(reminderSnoozeDuration)
        .millisecondsSinceEpoch;
    try {
      await reminderBridge.snoozeReminder(
        reminderId: payload.reminderId,
        snoozedUntil: snoozedUntil,
      );
    } catch (_) {
      requestReconciliation();
      return;
    }
    requestReconciliation();
  }
}

typedef ReminderRetryTimerFactory =
    Timer Function(Duration delay, void Function() callback);

const _defaultRetryDelays = <Duration>[
  Duration(seconds: 1),
  Duration(seconds: 2),
  Duration(seconds: 4),
  Duration(seconds: 8),
  Duration(seconds: 16),
  Duration(seconds: 30),
];

Timer _systemRetryTimerFactory(Duration delay, void Function() callback) =>
    Timer(delay, callback);

class _ReconciliationOutcome {
  const _ReconciliationOutcome.complete()
    : needsRetry = false,
      requiresRebuild = false;

  const _ReconciliationOutcome.retry({required this.requiresRebuild})
    : needsRetry = true;

  final bool needsRetry;
  final bool requiresRebuild;
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

int _legacyNotificationId(String reminderId) {
  var hash = 0x811c9dc5;
  for (final codeUnit in reminderId.codeUnits) {
    hash ^= codeUnit;
    hash = (hash * 0x01000193) & 0x7fffffff;
  }
  return hash == 0 ? 1 : hash;
}
