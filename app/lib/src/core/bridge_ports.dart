import 'package:taskveil/src/rust/api.dart' as rust_api;

/// Account authentication, session, and organization identity operations.
abstract interface class AccountBridgePort {
  Future<rust_api.AccountSessionStateDto> getAccountSessionState();

  Future<rust_api.AccountAuthResultDto> accountRegister({
    required String email,
    required String password,
    String? serverUrl,
    String? deviceName,
  });

  Future<rust_api.AccountAuthResultDto> accountLogin({
    required String email,
    required String password,
    String? serverUrl,
    String? deviceName,
  });

  Future<void> accountLogout();

  Future<rust_api.OrganizationSafetyStateDto> organizationSafetyNumber({
    required String tenantId,
    required String memberUserId,
  });

  Future<rust_api.OrganizationSafetyStateDto> confirmOrganizationSafetyNumber({
    required String tenantId,
    required String memberUserId,
    required String digest,
  });
}

/// Synchronization, realtime connection, and sync endpoint operations.
abstract interface class SyncBridgePort {
  Future<rust_api.SyncStatusDto> getSyncStatus();

  Future<rust_api.SyncStatusDto> syncNow();

  Future<rust_api.SyncNowOutcomeDto> syncNowOutcome();

  Future<rust_api.RealtimeTicketDto> getRealtimeTicket();

  Future<String> getSyncServerUrl();

  Future<void> setSyncServerUrl({required String serverUrl});

  Future<String> getLocalTimeZone();
}

/// Billing bootstrap and cached entitlement operations.
abstract interface class BillingBridgePort {
  Future<rust_api.BillingStateDto> billingBootstrap();

  Future<rust_api.BillingStateDto> refreshBilling();

  Future<rust_api.BillingStateDto?> getCachedBilling();
}

/// List lifecycle operations.
abstract interface class ListBridgePort {
  Future<rust_api.ListDto> createList({
    required String name,
    required String sortOrder,
  });

  Future<List<rust_api.ListDto>> getLists();

  Future<List<rust_api.ListDto>> getArchivedLists();

  Future<rust_api.ListDto> renameList({
    required String listId,
    required String name,
  });

  Future<rust_api.ListDto> archiveList({required String listId});

  Future<rust_api.ListDto> unarchiveList({required String listId});

  Future<int> countTasksInList({required String listId});

  Future<void> deleteList({required String listId});
}

/// Reusable task template and recurring series operations.
abstract interface class TemplateBridgePort {
  Future<List<rust_api.TemplateDto>> getTemplates();

  Future<List<rust_api.TaskSeriesDto>> getTaskSeries();

  Future<String> validateRecurrenceRule({
    required String rrule,
    required int startsAt,
    required String timeZone,
  });

  Future<rust_api.TemplateDto> saveTaskAsTemplate({
    required String taskId,
    required String name,
    String? defaultListId,
  });

  Future<rust_api.TemplateDto> createTemplate({
    required String name,
    String? defaultListId,
    required List<rust_api.TaskBlueprintNodeDto> nodes,
  });

  Future<rust_api.TemplateDto> updateTemplate({
    required String templateId,
    required String name,
    String? defaultListId,
    required List<rust_api.TaskBlueprintNodeDto> nodes,
  });

  Future<rust_api.TemplateDto> replaceTemplateBlueprint({
    required String templateId,
    required String taskId,
  });

  Future<List<rust_api.TaskDto>> instantiateTemplate({
    required String templateId,
  });

  Future<rust_api.TaskSeriesDto> createTaskSeriesFromTemplate({
    required String templateId,
    required String rrule,
    required int startsAt,
    required String timeZone,
  });

  Future<rust_api.TaskSeriesDto> createTaskSeriesFromTask({
    required String taskId,
    String? targetListId,
    required String rrule,
    required int startsAt,
    required String timeZone,
  });

  Future<rust_api.TaskSeriesDto> updateTaskSeries({
    required String seriesId,
    String? targetListId,
    required List<rust_api.TaskBlueprintNodeDto> nodes,
    required String rrule,
    required int startsAt,
    required String timeZone,
    required bool enabled,
  });

  Future<void> deleteTaskSeries({required String seriesId});

  Future<void> deleteTemplate({required String templateId});

  Future<rust_api.SettlementSummaryDto> settleDueSeries({required int atMs});

  Future<rust_api.StreakDto> getTaskSeriesStreak({
    required String seriesId,
    required int atMs,
  });
}

/// Task query, mutation, tree, ordering, and undo operations.
abstract interface class TaskBridgePort {
  Future<rust_api.TaskDto> createTask({
    required String listId,
    required String title,
    String? parentTaskId,
    rust_api.TaskDueInput? due,
    String note = '',
    int priority = 0,
    int? scheduledAt,
    int? estimatedMinutes,
  });

  Future<List<rust_api.TaskDto>> getTasks({required String listId});

  Future<List<rust_api.TaskDto>> searchTasks({required String query});

  Future<List<rust_api.HomeTaskDto>> getHomeTasks({
    required int todayStartMs,
    required int tomorrowStartMs,
  });

  Future<List<rust_api.CalendarOccurrenceDto>> getCalendarOccurrences({
    required rust_api.CalendarRangeInput range,
  });

  Future<rust_api.TaskDto> updateTask({
    required String taskId,
    required String title,
    required String note,
    required int priority,
    rust_api.TaskDueInput? due,
    int? scheduledAt,
    int? estimatedMinutes,
  });

  Future<rust_api.TaskDto> setTaskStatus({
    required String taskId,
    required String status,
    String? closedReason,
  });

  Future<int> countTaskDescendants({required String taskId});

  Future<void> deleteTask({required String taskId});

  Future<rust_api.TaskDto> reorderTask({
    required String taskId,
    String? previousTaskId,
    String? nextTaskId,
  });

  Future<rust_api.TaskUndoDto?> getLatestTaskUndo();

  Future<rust_api.TaskDto> undoTaskOperation({required String undoId});
}

/// Active timer and completed session operations.
abstract interface class TimerBridgePort {
  Future<rust_api.ActiveTimerSessionDto?> getActiveTimerSession();

  Future<rust_api.ActiveTimerStartOutcomeDto> startActiveTimerSession({
    required rust_api.ActiveTimerSessionDto session,
  });

  Future<void> updateActiveTimerSession({
    required rust_api.ActiveTimerSessionDto session,
  });

  Future<DateTime> pomodoroTargetReachedAt({
    required rust_api.ActiveTimerSessionDto session,
  });

  Future<bool> discardActiveTimerSession({required String expectedSessionId});

  Future<bool> finishActiveTimerSession({
    required rust_api.CompletedTimerSessionDto session,
  });

  Future<List<rust_api.CompletedTimerSessionDto>> getCompletedTimerSessions({
    required String taskId,
  });
}

/// Durable application setting operations.
abstract interface class SettingsBridgePort {
  Future<String?> getFrontendSetting({
    required rust_api.FrontendSettingKeyDto key,
  });

  Future<void> setFrontendSetting({
    required rust_api.FrontendSettingKeyDto key,
    required String value,
  });
}

/// Task reminder lifecycle, lookup, and snooze operations.
abstract interface class ReminderBridgePort {
  Future<rust_api.ReminderDto> createTaskReminder({
    required String taskId,
    required int remindAt,
  });

  Future<rust_api.ReminderDto> updateReminder({
    required String reminderId,
    required int remindAt,
  });

  Future<rust_api.ReminderDto> deleteReminder({required String reminderId});

  Future<List<rust_api.ReminderDto>> clearTaskReminders({
    required String taskId,
  });

  Future<List<rust_api.ReminderDto>> getTaskReminders({required String taskId});

  Future<List<rust_api.ReminderDto>> getTaskSubtreeReminders({
    required String taskId,
  });

  Future<List<rust_api.ReminderDto>> getListReminders({required String listId});

  Future<List<rust_api.ReminderDto>> listPendingReminders({required int nowMs});

  Future<rust_api.ReminderDto> snoozeReminder({
    required String reminderId,
    required int snoozedUntil,
  });
}
