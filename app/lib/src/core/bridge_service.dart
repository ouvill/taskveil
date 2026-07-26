import 'package:taskveil/src/rust/api.dart' as rust_api;

import 'bridge_ports.dart';
export 'bridge_ports.dart';

/// Abstracts the FRB-generated Rust bridge functions behind a plain Dart
/// interface.
///
/// Riverpod providers depend on this interface rather than calling the
/// generated `package:taskveil/src/rust/api.dart` functions directly. This
/// lets widget tests override [bridgeServiceProvider] (see
/// `src/core/providers.dart`) with an in-memory fake implementation, so the
/// whole screen/provider/router stack can be exercised without loading the
/// native Rust library or calling `initCore`.
abstract class BridgeService
    implements
        AccountBridgePort,
        SyncBridgePort,
        BillingBridgePort,
        ListBridgePort,
        TemplateBridgePort,
        TaskBridgePort,
        TimerBridgePort,
        SettingsBridgePort,
        ReminderBridgePort {
  @override
  Future<rust_api.SyncNowOutcomeDto> syncNowOutcome() async =>
      rust_api.SyncNowOutcomeDto.synced(status: await syncNow());

  @override
  Future<rust_api.BillingStateDto> billingBootstrap() =>
      Future.error(UnimplementedError('billingBootstrap'));

  @override
  Future<rust_api.BillingStateDto> refreshBilling() =>
      Future.error(UnimplementedError('refreshBilling'));

  @override
  Future<rust_api.BillingStateDto?> getCachedBilling() async => null;

  @override
  Future<List<rust_api.TemplateDto>> getTemplates() =>
      Future.error(UnimplementedError('getTemplates'));

  @override
  Future<List<rust_api.TaskSeriesDto>> getTaskSeries() =>
      Future.error(UnimplementedError('getTaskSeries'));

  @override
  Future<String> validateRecurrenceRule({
    required String rrule,
    required int startsAt,
    required String timeZone,
  }) => Future.error(UnimplementedError('validateRecurrenceRule'));

  @override
  Future<rust_api.TemplateDto> saveTaskAsTemplate({
    required String taskId,
    required String name,
    String? defaultListId,
  }) => Future.error(UnimplementedError('saveTaskAsTemplate'));

  @override
  Future<rust_api.TemplateDto> createTemplate({
    required String name,
    String? defaultListId,
    required List<rust_api.TaskBlueprintNodeDto> nodes,
  }) => Future.error(UnimplementedError('createTemplate'));

  @override
  Future<rust_api.TemplateDto> updateTemplate({
    required String templateId,
    required String name,
    String? defaultListId,
    required List<rust_api.TaskBlueprintNodeDto> nodes,
  }) => Future.error(UnimplementedError('updateTemplate'));

  @override
  Future<rust_api.TemplateDto> replaceTemplateBlueprint({
    required String templateId,
    required String taskId,
  }) => Future.error(UnimplementedError('replaceTemplateBlueprint'));

  @override
  Future<List<rust_api.TaskDto>> instantiateTemplate({
    required String templateId,
  }) => Future.error(UnimplementedError('instantiateTemplate'));

  @override
  Future<rust_api.TaskSeriesDto> createTaskSeriesFromTemplate({
    required String templateId,
    required String rrule,
    required int startsAt,
    required String timeZone,
  }) => Future.error(UnimplementedError('createTaskSeriesFromTemplate'));

  @override
  Future<rust_api.TaskSeriesDto> createTaskSeriesFromTask({
    required String taskId,
    String? targetListId,
    required String rrule,
    required int startsAt,
    required String timeZone,
  }) => Future.error(UnimplementedError('createTaskSeriesFromTask'));

  @override
  Future<rust_api.TaskSeriesDto> updateTaskSeries({
    required String seriesId,
    String? targetListId,
    required List<rust_api.TaskBlueprintNodeDto> nodes,
    required String rrule,
    required int startsAt,
    required String timeZone,
    required bool enabled,
  }) => Future.error(UnimplementedError('updateTaskSeries'));

  @override
  Future<void> deleteTaskSeries({required String seriesId}) =>
      Future.error(UnimplementedError('deleteTaskSeries'));

  @override
  Future<void> deleteTemplate({required String templateId}) =>
      Future.error(UnimplementedError('deleteTemplate'));

  @override
  Future<rust_api.SettlementSummaryDto> settleDueSeries({required int atMs}) =>
      Future.error(UnimplementedError('settleDueSeries'));

  @override
  Future<rust_api.StreakDto> getTaskSeriesStreak({
    required String seriesId,
    required int atMs,
  }) => Future.error(UnimplementedError('getTaskSeriesStreak'));
}

/// Default [BridgeService] implementation backed by the FRB-generated
/// bindings in `src/rust/api.dart`.
class FrbBridgeService implements BridgeService {
  const FrbBridgeService();

  @override
  Future<rust_api.AccountSessionStateDto> getAccountSessionState() =>
      rust_api.getAccountSessionState();

  @override
  Future<rust_api.AccountRegistrationPendingDto> accountRegistrationBegin({
    required String email,
    String? serverUrl,
  }) => rust_api.accountRegistrationBegin(email: email, serverUrl: serverUrl);

  @override
  Future<rust_api.AccountRegistrationStateDto?>
  accountRegistrationState() async => rust_api.accountRegistrationState();

  @override
  Future<void> accountRegistrationCancel() async {
    rust_api.accountRegistrationCancel();
  }

  @override
  Future<rust_api.AccountRegistrationPendingDto> accountRegistrationResend() =>
      rust_api.accountRegistrationResend();

  @override
  Future<void> accountRegistrationVerifyOtp({required String otp}) =>
      rust_api.accountRegistrationVerifyOtp(otp: otp);

  @override
  Future<rust_api.AccountAuthResultDto> accountRegistrationComplete({
    required String password,
    String? deviceName,
  }) => rust_api.accountRegistrationComplete(
    password: password,
    deviceName: deviceName,
  );

  @override
  Future<void> accountRegistrationAckRecoveryKey() async {
    rust_api.accountRegistrationAckRecoveryKey();
  }

  @override
  Future<String?> accountRegistrationRecoveryKey() async =>
      rust_api.accountRegistrationRecoveryKey();

  @override
  Future<rust_api.AccountAuthResultDto> accountLogin({
    required String email,
    required String password,
    String? serverUrl,
    String? deviceName,
  }) => rust_api.accountLogin(
    email: email,
    password: password,
    serverUrl: serverUrl,
    deviceName: deviceName,
  );

  @override
  Future<void> accountLogout() => rust_api.accountLogout();

  @override
  Future<rust_api.OrganizationSafetyStateDto> organizationSafetyNumber({
    required String tenantId,
    required String memberUserId,
  }) => rust_api.organizationSafetyNumber(
    tenantId: tenantId,
    memberUserId: memberUserId,
  );

  @override
  Future<rust_api.OrganizationSafetyStateDto> confirmOrganizationSafetyNumber({
    required String tenantId,
    required String memberUserId,
    required String digest,
  }) => rust_api.confirmOrganizationSafetyNumber(
    tenantId: tenantId,
    memberUserId: memberUserId,
    digest: digest,
  );

  @override
  Future<rust_api.SyncStatusDto> getSyncStatus() => rust_api.getSyncStatus();

  @override
  Future<rust_api.SyncStatusDto> syncNow() => rust_api.syncNow();

  @override
  Future<rust_api.SyncNowOutcomeDto> syncNowOutcome() =>
      rust_api.syncNowOutcome();

  @override
  Future<rust_api.BillingStateDto> billingBootstrap() =>
      rust_api.billingBootstrap();

  @override
  Future<rust_api.BillingStateDto> refreshBilling() =>
      rust_api.refreshBilling();

  @override
  Future<rust_api.BillingStateDto?> getCachedBilling() =>
      rust_api.getCachedBilling();

  @override
  Future<rust_api.RealtimeTicketDto> getRealtimeTicket() =>
      rust_api.getRealtimeTicket();

  @override
  Future<String> getSyncServerUrl() => rust_api.getSyncServerUrl();

  @override
  Future<void> setSyncServerUrl({required String serverUrl}) =>
      rust_api.setSyncServerUrl(serverUrl: serverUrl);

  @override
  Future<String> getLocalTimeZone() => rust_api.getLocalTimeZone();

  @override
  Future<rust_api.ListDto> createList({
    required String name,
    required String sortOrder,
  }) => rust_api.createList(name: name, sortOrder: sortOrder);

  @override
  Future<List<rust_api.ListDto>> getLists() => rust_api.getLists();

  @override
  Future<List<rust_api.ListDto>> getArchivedLists() =>
      rust_api.getArchivedLists();

  @override
  Future<List<rust_api.TemplateDto>> getTemplates() => rust_api.getTemplates();

  @override
  Future<List<rust_api.TaskSeriesDto>> getTaskSeries() =>
      rust_api.getTaskSeries();

  @override
  Future<String> validateRecurrenceRule({
    required String rrule,
    required int startsAt,
    required String timeZone,
  }) => rust_api.validateRecurrenceRule(
    rrule: rrule,
    startsAt: startsAt,
    timeZone: timeZone,
  );

  @override
  Future<rust_api.TemplateDto> saveTaskAsTemplate({
    required String taskId,
    required String name,
    String? defaultListId,
  }) => rust_api.saveTaskAsTemplate(
    taskId: taskId,
    name: name,
    defaultListId: defaultListId,
  );

  @override
  Future<rust_api.TemplateDto> createTemplate({
    required String name,
    String? defaultListId,
    required List<rust_api.TaskBlueprintNodeDto> nodes,
  }) => rust_api.createTemplate(
    name: name,
    defaultListId: defaultListId,
    nodes: nodes,
  );

  @override
  Future<rust_api.TemplateDto> updateTemplate({
    required String templateId,
    required String name,
    String? defaultListId,
    required List<rust_api.TaskBlueprintNodeDto> nodes,
  }) => rust_api.updateTemplate(
    templateId: templateId,
    name: name,
    defaultListId: defaultListId,
    nodes: nodes,
  );

  @override
  Future<rust_api.TemplateDto> replaceTemplateBlueprint({
    required String templateId,
    required String taskId,
  }) =>
      rust_api.replaceTemplateBlueprint(templateId: templateId, taskId: taskId);

  @override
  Future<List<rust_api.TaskDto>> instantiateTemplate({
    required String templateId,
  }) => rust_api.instantiateTemplate(templateId: templateId);

  @override
  Future<rust_api.TaskSeriesDto> createTaskSeriesFromTemplate({
    required String templateId,
    required String rrule,
    required int startsAt,
    required String timeZone,
  }) => rust_api.createTaskSeriesFromTemplate(
    templateId: templateId,
    rrule: rrule,
    startsAt: startsAt,
    timeZone: timeZone,
  );

  @override
  Future<rust_api.TaskSeriesDto> createTaskSeriesFromTask({
    required String taskId,
    String? targetListId,
    required String rrule,
    required int startsAt,
    required String timeZone,
  }) => rust_api.createTaskSeriesFromTask(
    taskId: taskId,
    targetListId: targetListId,
    rrule: rrule,
    startsAt: startsAt,
    timeZone: timeZone,
  );

  @override
  Future<rust_api.TaskSeriesDto> updateTaskSeries({
    required String seriesId,
    String? targetListId,
    required List<rust_api.TaskBlueprintNodeDto> nodes,
    required String rrule,
    required int startsAt,
    required String timeZone,
    required bool enabled,
  }) => rust_api.updateTaskSeries(
    seriesId: seriesId,
    targetListId: targetListId,
    nodes: nodes,
    rrule: rrule,
    startsAt: startsAt,
    timeZone: timeZone,
    enabled: enabled,
  );

  @override
  Future<void> deleteTaskSeries({required String seriesId}) =>
      rust_api.deleteTaskSeries(seriesId: seriesId);

  @override
  Future<void> deleteTemplate({required String templateId}) =>
      rust_api.deleteTemplate(templateId: templateId);

  @override
  Future<rust_api.SettlementSummaryDto> settleDueSeries({required int atMs}) =>
      rust_api.settleDueSeries(atMs: atMs);

  @override
  Future<rust_api.StreakDto> getTaskSeriesStreak({
    required String seriesId,
    required int atMs,
  }) => rust_api.getTaskSeriesStreak(seriesId: seriesId, atMs: atMs);

  @override
  Future<rust_api.ListDto> renameList({
    required String listId,
    required String name,
  }) => rust_api.renameList(listId: listId, name: name);

  @override
  Future<rust_api.ListDto> archiveList({required String listId}) =>
      rust_api.archiveList(listId: listId);

  @override
  Future<rust_api.ListDto> unarchiveList({required String listId}) =>
      rust_api.unarchiveList(listId: listId);

  @override
  Future<rust_api.TaskDto> createTask({
    required String listId,
    required String title,
    String? parentTaskId,
    rust_api.TaskDueInput? due,
    String note = '',
    int priority = 0,
    int? scheduledAt,
    int? estimatedMinutes,
  }) => rust_api.createTask(
    listId: listId,
    title: title,
    parentTaskId: parentTaskId,
    due: due,
    note: note.isEmpty ? null : note,
    priority: priority,
    scheduledAt: scheduledAt,
    estimatedMinutes: estimatedMinutes,
  );

  @override
  Future<List<rust_api.TaskDto>> getTasks({required String listId}) =>
      rust_api.getTasks(listId: listId);

  @override
  Future<rust_api.ActiveTimerSessionDto?> getActiveTimerSession() =>
      rust_api.getActiveTimerSession();

  @override
  Future<rust_api.ActiveTimerStartOutcomeDto> startActiveTimerSession({
    required rust_api.ActiveTimerSessionDto session,
  }) => rust_api.startActiveTimerSession(session: session);

  @override
  Future<void> updateActiveTimerSession({
    required rust_api.ActiveTimerSessionDto session,
  }) => rust_api.updateActiveTimerSession(session: session);

  @override
  Future<DateTime> pomodoroTargetReachedAt({
    required rust_api.ActiveTimerSessionDto session,
  }) => rust_api.pomodoroTargetReachedAt(session: session);

  @override
  Future<bool> discardActiveTimerSession({required String expectedSessionId}) =>
      rust_api.discardActiveTimerSession(expectedSessionId: expectedSessionId);

  @override
  Future<bool> finishActiveTimerSession({
    required rust_api.CompletedTimerSessionDto session,
  }) => rust_api.finishActiveTimerSession(session: session);

  @override
  Future<List<rust_api.CompletedTimerSessionDto>> getCompletedTimerSessions({
    required String taskId,
  }) => rust_api.getCompletedTimerSessions(taskId: taskId);

  @override
  Future<List<rust_api.TaskDto>> searchTasks({required String query}) =>
      rust_api.searchTasks(query: query);

  @override
  Future<List<rust_api.HomeTaskDto>> getHomeTasks({
    required int todayStartMs,
    required int tomorrowStartMs,
  }) => rust_api.getHomeTasks(
    todayStartMs: todayStartMs,
    tomorrowStartMs: tomorrowStartMs,
  );

  @override
  Future<List<rust_api.CalendarOccurrenceDto>> getCalendarOccurrences({
    required rust_api.CalendarRangeInput range,
  }) => rust_api.getCalendarOccurrences(range: range);

  @override
  Future<int> countTasksInList({required String listId}) =>
      rust_api.countTasksInList(listId: listId);

  @override
  Future<rust_api.TaskDto> updateTask({
    required String taskId,
    required String title,
    required String note,
    required int priority,
    rust_api.TaskDueInput? due,
    int? scheduledAt,
    int? estimatedMinutes,
  }) => rust_api.updateTask(
    taskId: taskId,
    title: title,
    note: note,
    priority: priority,
    due: due,
    scheduledAt: scheduledAt,
    estimatedMinutes: estimatedMinutes,
  );

  @override
  Future<rust_api.TaskDto> setTaskStatus({
    required String taskId,
    required String status,
    String? closedReason,
  }) => rust_api.setTaskStatus(
    taskId: taskId,
    status: status,
    closedReason: closedReason,
  );

  @override
  Future<int> countTaskDescendants({required String taskId}) =>
      rust_api.countTaskDescendants(taskId: taskId);

  @override
  Future<void> deleteTask({required String taskId}) =>
      rust_api.deleteTask(taskId: taskId);

  @override
  Future<void> deleteList({required String listId}) =>
      rust_api.deleteList(listId: listId);

  @override
  Future<rust_api.TaskDto> reorderTask({
    required String taskId,
    String? previousTaskId,
    String? nextTaskId,
  }) => rust_api.reorderTask(
    taskId: taskId,
    previousTaskId: previousTaskId,
    nextTaskId: nextTaskId,
  );

  @override
  Future<rust_api.TaskUndoDto?> getLatestTaskUndo() =>
      rust_api.getLatestTaskUndo();

  @override
  Future<rust_api.TaskDto> undoTaskOperation({required String undoId}) =>
      rust_api.undoTaskOperation(undoId: undoId);

  @override
  Future<String?> getFrontendSetting({
    required rust_api.FrontendSettingKeyDto key,
  }) => rust_api.getFrontendSetting(key: key);

  @override
  Future<void> setFrontendSetting({
    required rust_api.FrontendSettingKeyDto key,
    required String value,
  }) => rust_api.setFrontendSetting(key: key, value: value);

  @override
  Future<rust_api.ReminderDto> createTaskReminder({
    required String taskId,
    required int remindAt,
  }) => rust_api.createTaskReminder(taskId: taskId, remindAt: remindAt);

  @override
  Future<rust_api.ReminderDto> updateReminder({
    required String reminderId,
    required int remindAt,
  }) => rust_api.updateReminder(reminderId: reminderId, remindAt: remindAt);

  @override
  Future<rust_api.ReminderDto> deleteReminder({required String reminderId}) =>
      rust_api.deleteReminder(reminderId: reminderId);

  @override
  Future<List<rust_api.ReminderDto>> clearTaskReminders({
    required String taskId,
  }) => rust_api.clearTaskReminders(taskId: taskId);

  @override
  Future<List<rust_api.ReminderDto>> getTaskReminders({
    required String taskId,
  }) => rust_api.getTaskReminders(taskId: taskId);

  @override
  Future<List<rust_api.ReminderDto>> getTaskSubtreeReminders({
    required String taskId,
  }) => rust_api.getTaskSubtreeReminders(taskId: taskId);

  @override
  Future<List<rust_api.ReminderDto>> getListReminders({
    required String listId,
  }) => rust_api.getListReminders(listId: listId);

  @override
  Future<List<rust_api.ReminderDto>> listPendingReminders({
    required int nowMs,
  }) => rust_api.listPendingReminders(nowMs: nowMs);

  @override
  Future<rust_api.ReminderDto> snoozeReminder({
    required String reminderId,
    required int snoozedUntil,
  }) => rust_api.snoozeReminder(
    reminderId: reminderId,
    snoozedUntil: snoozedUntil,
  );

  @override
  Future<List<rust_api.ReminderNotificationCommandDto>>
  prepareReminderNotificationReconciliation({required int nowMs}) =>
      rust_api.prepareReminderNotificationReconciliation(nowMs: nowMs);

  @override
  Future<List<rust_api.ReminderNotificationCommandDto>>
  listReminderNotificationCommands({required int nowMs, required int limit}) =>
      rust_api.listReminderNotificationCommands(nowMs: nowMs, limit: limit);

  @override
  Future<bool> ackReminderNotificationCommand({
    required String reminderId,
    required int revision,
  }) => rust_api.ackReminderNotificationCommand(
    reminderId: reminderId,
    revision: revision,
  );
}
