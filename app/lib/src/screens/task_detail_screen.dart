import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:go_router/go_router.dart';
import 'package:intl/intl.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';
import 'package:taskveil/src/core/providers.dart';
import 'package:taskveil/src/core/task_tree.dart';
import 'package:taskveil/src/core/task_due.dart';
import 'package:taskveil/src/generated/l10n/app_localizations.dart';
import 'package:taskveil/src/notifications/reminder_notifications.dart';
import 'package:taskveil/src/rust/api.dart';
import 'package:taskveil/src/screens/templates_screen.dart'
    show showTaskSeriesDialog;
import 'package:taskveil/src/ui/dialogs.dart';
import 'package:taskveil/src/ui/bridge_error_messages.dart';
import 'package:taskveil/src/ui/states.dart';
import 'package:taskveil/src/ui/task_components.dart';
import 'package:taskveil/src/ui/theme.dart';

/// The task detail screen (route `/lists/:listId/tasks/:taskId`).
///
/// F-02 "シンプルUI" skeleton plus M3 task field editing.
class TaskDetailScreen extends ConsumerWidget {
  const TaskDetailScreen({
    super.key,
    required this.listId,
    required this.taskId,
  });

  final String listId;
  final String taskId;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final l10n = AppLocalizations.of(context)!;
    final detailAsync = ref.watch(
      taskDetailProvider((listId: listId, taskId: taskId)),
    );
    final tasksAsync = ref.watch(tasksProvider(listId));

    return Scaffold(
      backgroundColor: AppColors.canvas,
      appBar: AppBar(
        backgroundColor: AppColors.canvas,
        title: const SizedBox.shrink(),
        actions: [
          detailAsync.maybeWhen(
            data: (task) {
              if (task == null) {
                return const SizedBox.shrink();
              }
              return PopupMenuButton<_TaskDetailAction>(
                tooltip: l10n.taskActionsTooltip,
                onSelected: (action) {
                  switch (action) {
                    case _TaskDetailAction.markDone:
                      unawaited(_setTaskStatus(context, ref, task, 'done'));
                    case _TaskDetailAction.markWontDo:
                      unawaited(_setTaskStatus(context, ref, task, 'wont_do'));
                    case _TaskDetailAction.reopen:
                      unawaited(_setTaskStatus(context, ref, task, 'todo'));
                    case _TaskDetailAction.saveAsTemplate:
                      unawaited(_saveAsTemplate(context, ref, task));
                    case _TaskDetailAction.createTaskSeries:
                      unawaited(_createTaskSeries(context, ref, task));
                    case _TaskDetailAction.delete:
                      unawaited(_deleteTask(context, ref, task));
                  }
                },
                itemBuilder: (context) =>
                    _taskDetailMenuItems(l10n: l10n, task: task),
              );
            },
            orElse: () => const SizedBox.shrink(),
          ),
        ],
      ),
      body: tasksAsync.when(
        loading: () => const AppLoadingState(),
        error: (error, stackTrace) => AppErrorState(
          message: l10n.failedToLoadTask(bridgeErrorMessage(l10n, error)),
        ),
        data: (tasks) {
          final task = _findTaskById(tasks, taskId);
          if (task == null) {
            return AppEmptyState(
              icon: LucideIcons.searchX300,
              title: l10n.taskNotFound,
            );
          }
          final stats = descendantStatsOf(task.id, tasks);
          final subtaskNodes = flattenTaskTree(
            descendantTaskTreeOf(task.id, tasks),
          );
          final parentTask = task.parentTaskId == null
              ? null
              : _findTaskById(tasks, task.parentTaskId!);
          final theme = Theme.of(context);
          final colorScheme = theme.colorScheme;
          final locale = Localizations.localeOf(context).toLanguageTag();
          final remindersAsync = ref.watch(taskRemindersProvider(task.id));
          final reminders =
              remindersAsync.asData?.value ?? const <ReminderDto>[];
          final completedSessionsAsync = ref.watch(
            completedTimerSessionsProvider(task.id),
          );
          final completedSessions =
              completedSessionsAsync.asData?.value ??
              const <CompletedTimerSessionDto>[];
          final actualDuration = Duration(
            milliseconds: completedSessions.fold<int>(
              0,
              (total, session) => total + session.activeDurationMs.toInt(),
            ),
          );
          return Align(
            alignment: Alignment.topCenter,
            child: ConstrainedBox(
              constraints: const BoxConstraints(maxWidth: 760),
              child: ListView(
                padding: const EdgeInsets.fromLTRB(
                  AppSpacing.md,
                  AppSpacing.sm,
                  AppSpacing.md,
                  AppSpacing.xl,
                ),
                children: [
                  Padding(
                    padding: const EdgeInsets.fromLTRB(0, 8, 0, 18),
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        if (parentTask != null) ...[
                          _ParentTaskLink(
                            parentTask: parentTask,
                            tooltip: l10n.parentTaskLinkTooltip(
                              parentTask.title,
                            ),
                            semanticLabel: l10n.parentTaskLinkSemantics(
                              parentTask.title,
                            ),
                            onTap: () => context.push(
                              '/lists/$listId/tasks/${parentTask.id}',
                            ),
                          ),
                          const SizedBox(height: AppSpacing.xs),
                        ],
                        Row(
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: [
                            Padding(
                              padding: const EdgeInsets.only(
                                top: AppSpacing.xs,
                              ),
                              child: AppTaskCheckbox(
                                checkboxKey: ValueKey(
                                  'task-detail-done-${task.id}',
                                ),
                                isDone: isTaskClosed(task),
                                tooltip: isTaskClosed(task)
                                    ? l10n.reopenTaskTooltip
                                    : l10n.completeTaskTooltip,
                                onToggleDone: () {
                                  unawaited(
                                    isTaskClosed(task)
                                        ? _setTaskStatus(
                                            context,
                                            ref,
                                            task,
                                            'todo',
                                          )
                                        : _setTaskStatus(
                                            context,
                                            ref,
                                            task,
                                            'done',
                                          ),
                                  );
                                },
                              ),
                            ),
                            const SizedBox(width: AppSpacing.xs),
                            Expanded(
                              child: _InlineTitleEditor(
                                key: ValueKey('task-title-editor-${task.id}'),
                                title: task.title,
                                isClosed: isTaskClosed(task),
                                semanticLabel: l10n.editTaskTitleSemantics,
                                onSave: (title) => _updateTaskFields(
                                  context,
                                  ref,
                                  task,
                                  title: title,
                                ),
                              ),
                            ),
                          ],
                        ),
                        const SizedBox(height: AppSpacing.sm),
                        _InlineNoteEditor(
                          key: ValueKey('task-note-editor-${task.id}'),
                          note: task.note,
                          placeholder: l10n.addNotePlaceholder,
                          semanticLabel: l10n.editTaskNoteSemantics,
                          onSave: (note) =>
                              _updateTaskFields(context, ref, task, note: note),
                        ),
                        const SizedBox(height: AppSpacing.md),
                        _EditableTaskMetadata(
                          task: task,
                          reminders: reminders,
                          stats: stats,
                          actualDuration: actualDuration,
                          locale: locale,
                          onStartFocus: isTaskClosed(task)
                              ? null
                              : () => context.push(
                                  '/focus/${task.listId}/${task.id}',
                                ),
                          onSelectDueDate: () => _selectDue(context, ref, task),
                          onClearDueDate: task.due == null
                              ? null
                              : () => _updateTaskFields(
                                  context,
                                  ref,
                                  task,
                                  due: null,
                                ),
                          onSelectPlan: () => _selectPlan(context, ref, task),
                          onSelectPriority: () =>
                              _selectPriority(context, ref, task),
                          onSelectReminder: () =>
                              _manageReminders(context, ref, task, reminders),
                        ),
                        const SizedBox(height: AppSpacing.sm),
                        Text(
                          l10n.taskCreatedAt(
                            formatAbsoluteDate(locale, task.createdAt),
                          ),
                          style: theme.textTheme.bodySmall?.copyWith(
                            color: colorScheme.onSurfaceVariant,
                          ),
                        ),
                      ],
                    ),
                  ),
                  Divider(color: colorScheme.outlineVariant),
                  const SizedBox(height: AppSpacing.lg),
                  Row(
                    children: [
                      Container(
                        width: 3,
                        height: 18,
                        decoration: BoxDecoration(
                          color: colorScheme.primary,
                          borderRadius: BorderRadius.circular(999),
                        ),
                      ),
                      const SizedBox(width: AppSpacing.sm),
                      Text(
                        l10n.subtasksTitle,
                        style: theme.textTheme.titleMedium,
                      ),
                    ],
                  ),
                  const SizedBox(height: AppSpacing.sm),
                  if (subtaskNodes.isEmpty)
                    AppEmptyState(
                      icon: LucideIcons.gitBranch300,
                      title: l10n.subtasksEmpty,
                    )
                  else
                    for (final node in subtaskNodes)
                      Builder(
                        key: ValueKey('subtask-row-${node.task.id}'),
                        builder: (context) {
                          final subtask = node.task;
                          final subtaskStats = descendantStatsOf(
                            subtask.id,
                            tasks,
                          );
                          return AppTaskRow(
                            key: ValueKey('task-row-${subtask.id}'),
                            title: subtask.title,
                            isDone: isTaskClosed(subtask),
                            depth: node.depth,
                            checkboxKey: ValueKey('task-done-${subtask.id}'),
                            priority: subtask.priority,
                            priorityDotKey: ValueKey(
                              'task-priority-dot-${subtask.id}',
                            ),
                            prioritySemanticLabel: l10n.taskPriority(
                              taskPriorityLabel(l10n, subtask.priority),
                            ),
                            semanticLabel: _detailTaskRowSemanticLabel(
                              l10n: l10n,
                              title: subtask.title,
                              status: taskStatusLabel(l10n, subtask.status),
                              priority: taskPriorityLabel(
                                l10n,
                                subtask.priority,
                              ),
                              dueLabel: subtask.due == null
                                  ? null
                                  : formatRelativeDueDate(
                                      l10n,
                                      locale,
                                      subtask.due,
                                    ),
                              depth: node.depth,
                            ),
                            hierarchyGuideKey: ValueKey(
                              'task-hierarchy-guide-${subtask.id}',
                            ),
                            hierarchyGuideHorizontalKey: ValueKey(
                              'task-hierarchy-horizontal-${subtask.id}',
                            ),
                            isLastSibling: node.isLastSibling,
                            ancestorLineContinuations:
                                node.ancestorLineContinuations,
                            toggleDoneTooltip: isTaskClosed(subtask)
                                ? l10n.reopenTaskTooltip
                                : l10n.completeTaskTooltip,
                            metadata: taskMetadataItemsFor(
                              l10n: l10n,
                              locale: locale,
                              task: subtask,
                              stats: subtaskStats,
                              includeSubtaskProgress: false,
                            ),
                            framed: false,
                            onToggleDone: () {
                              unawaited(
                                isTaskClosed(subtask)
                                    ? _setTaskStatus(
                                        context,
                                        ref,
                                        subtask,
                                        'todo',
                                      )
                                    : _setTaskStatus(
                                        context,
                                        ref,
                                        subtask,
                                        'done',
                                      ),
                              );
                            },
                            onTap: () => context.push(
                              '/lists/$listId/tasks/${subtask.id}',
                            ),
                          );
                        },
                      ),
                  const SizedBox(height: AppSpacing.sm),
                  Align(
                    alignment: AlignmentDirectional.centerStart,
                    child: TextButton.icon(
                      icon: const Icon(LucideIcons.plus300, size: 18),
                      label: Text(l10n.addSubtaskButton),
                      onPressed: () => _createSubtask(context, ref, task),
                    ),
                  ),
                ],
              ),
            ),
          );
        },
      ),
    );
  }

  Future<void> _createSubtask(
    BuildContext context,
    WidgetRef ref,
    TaskDto task,
  ) async {
    final l10n = AppLocalizations.of(context)!;
    final title = await showAppTextInputDialog(
      context: context,
      title: l10n.newSubtaskTitle,
      label: l10n.titleLabel,
      cancelLabel: l10n.cancelButton,
      submitLabel: l10n.createButton,
    );
    if (title == null || title.trim().isEmpty) {
      return;
    }
    await ref
        .read(tasksProvider(listId).notifier)
        .createTask(title.trim(), parentTaskId: task.id);
  }

  Future<bool> _updateTaskFields(
    BuildContext context,
    WidgetRef ref,
    TaskDto task, {
    String? title,
    String? note,
    int? priority,
    Object? due = _unchangedDue,
    Object? scheduledAt = _unchangedScheduledAt,
    Object? estimatedMinutes = _unchangedEstimatedMinutes,
  }) async {
    final nextTitle = title ?? task.title;
    final nextNote = note ?? task.note;
    final nextPriority = priority ?? task.priority;
    final nextDue = identical(due, _unchangedDue)
        ? task.due
        : due as TaskDueDto?;
    final nextScheduledAt = identical(scheduledAt, _unchangedScheduledAt)
        ? task.scheduledAt
        : scheduledAt as int?;
    final nextEstimatedMinutes =
        identical(estimatedMinutes, _unchangedEstimatedMinutes)
        ? task.estimatedMinutes
        : estimatedMinutes as int?;

    if (nextTitle == task.title &&
        nextNote == task.note &&
        nextPriority == task.priority &&
        nextDue == task.due &&
        nextScheduledAt == task.scheduledAt &&
        nextEstimatedMinutes == task.estimatedMinutes) {
      return true;
    }

    try {
      await ref
          .read(tasksProvider(listId).notifier)
          .updateTask(
            taskId: task.id,
            title: nextTitle,
            note: nextNote,
            priority: nextPriority,
            due: nextDue,
            scheduledAt: nextScheduledAt,
            estimatedMinutes: nextEstimatedMinutes,
          );
      ref.invalidate(homeTasksProvider);
      if (context.mounted) {
        await _showLatestUndoSnackBar(context);
      }
      return true;
    } catch (error) {
      if (context.mounted) {
        final l10n = AppLocalizations.of(context)!;
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(
            content: Text(
              l10n.failedToSaveTask(bridgeErrorMessage(l10n, error)),
            ),
            margin: const EdgeInsets.all(AppSpacing.md),
          ),
        );
      }
      return false;
    }
  }

  Future<void> _selectPlan(
    BuildContext context,
    WidgetRef ref,
    TaskDto task,
  ) async {
    final value = await showTaskPlanSheet(
      context: context,
      initialValue: TaskPlanValue(
        scheduledAt: task.scheduledAt,
        estimatedMinutes: task.estimatedMinutes,
      ),
    );
    if (value == null || !context.mounted) return;
    await _updateTaskFields(
      context,
      ref,
      task,
      scheduledAt: value.scheduledAt,
      estimatedMinutes: value.estimatedMinutes,
    );
  }

  Future<void> _selectPriority(
    BuildContext context,
    WidgetRef ref,
    TaskDto task,
  ) async {
    final selection = await showTaskPrioritySheet(
      context: context,
      selectedPriority: task.priority,
    );
    if (selection == null || !context.mounted) return;
    await _updateTaskFields(context, ref, task, priority: selection.value);
  }

  Future<void> _selectDue(
    BuildContext context,
    WidgetRef ref,
    TaskDto task,
  ) async {
    final l10n = AppLocalizations.of(context)!;
    final mode = await showModalBottomSheet<_TaskDueMode>(
      context: context,
      showDragHandle: true,
      builder: (context) => SafeArea(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            ListTile(
              leading: const Icon(LucideIcons.calendarDays300),
              title: Text(l10n.setDueDateButton),
              onTap: () => Navigator.of(context).pop(_TaskDueMode.date),
            ),
            ListTile(
              leading: const Icon(LucideIcons.calendarClock300),
              title: Text(l10n.setDueDateTimeButton),
              onTap: () => Navigator.of(context).pop(_TaskDueMode.dateTime),
            ),
          ],
        ),
      ),
    );
    if (mode == null || !context.mounted) {
      return;
    }
    final initialDate = task.due == null
        ? DateTime.now()
        : taskDueDisplayDate(task.due!);
    final picked = await showDatePicker(
      context: context,
      initialDate: initialDate,
      firstDate: DateTime(2000),
      lastDate: DateTime(2100),
    );
    if (picked == null || !context.mounted) {
      return;
    }
    if (mode == _TaskDueMode.date) {
      await _updateTaskFields(context, ref, task, due: dateOnlyDue(picked));
      return;
    }
    final pickedTime = await showTimePicker(
      context: context,
      initialTime: TimeOfDay.fromDateTime(initialDate),
    );
    if (pickedTime == null || !context.mounted) {
      return;
    }
    final localDateTime = DateTime(
      picked.year,
      picked.month,
      picked.day,
      pickedTime.hour,
      pickedTime.minute,
    );
    if (localDateTime.year != picked.year ||
        localDateTime.month != picked.month ||
        localDateTime.day != picked.day ||
        localDateTime.hour != pickedTime.hour ||
        localDateTime.minute != pickedTime.minute) {
      return;
    }
    final savedTimeZone = taskDueSavedTimeZone(task.due);
    final timeZone =
        savedTimeZone ??
        await ref.read(bridgeServiceProvider).getLocalTimeZone();
    if (!context.mounted) {
      return;
    }
    TaskDueDto due;
    try {
      due = dateTimeDue(localDateTime: localDateTime, timeZone: timeZone);
    } on FormatException {
      return;
    }
    await _updateTaskFields(context, ref, task, due: due);
  }

  Future<void> _manageReminders(
    BuildContext context,
    WidgetRef ref,
    TaskDto task,
    List<ReminderDto> initialReminders,
  ) async {
    var reminders = [...initialReminders]..sort(_compareReminderDtos);
    await showModalBottomSheet<void>(
      context: context,
      isScrollControlled: true,
      showDragHandle: true,
      builder: (sheetContext) => StatefulBuilder(
        builder: (sheetContext, setSheetState) {
          final l10n = AppLocalizations.of(sheetContext)!;
          final locale = Localizations.localeOf(sheetContext).toLanguageTag();
          return SafeArea(
            child: ConstrainedBox(
              constraints: BoxConstraints(
                maxHeight: MediaQuery.sizeOf(sheetContext).height * 0.78,
              ),
              child: Padding(
                padding: const EdgeInsets.fromLTRB(
                  AppSpacing.md,
                  0,
                  AppSpacing.md,
                  AppSpacing.md,
                ),
                child: Column(
                  key: const ValueKey('task-reminders-sheet'),
                  mainAxisSize: MainAxisSize.min,
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    Text(
                      l10n.manageRemindersTitle,
                      style: Theme.of(sheetContext).textTheme.titleLarge,
                    ),
                    const SizedBox(height: AppSpacing.sm),
                    Flexible(
                      child: ListView.separated(
                        shrinkWrap: true,
                        itemCount: reminders.length,
                        separatorBuilder: (_, _) => const Divider(height: 1),
                        itemBuilder: (context, index) {
                          final reminder = reminders[index];
                          final effectiveAt = effectiveReminderAt(reminder);
                          final isPast =
                              effectiveAt <=
                              DateTime.now().millisecondsSinceEpoch;
                          return ListTile(
                            key: ValueKey('reminder-row-${reminder.id}'),
                            contentPadding: EdgeInsets.zero,
                            title: Text(
                              formatReminderDateTime(locale, effectiveAt),
                            ),
                            subtitle: isPast
                                ? Text(l10n.reminderPastLabel)
                                : null,
                            trailing: Row(
                              mainAxisSize: MainAxisSize.min,
                              children: [
                                IconButton(
                                  key: ValueKey('edit-reminder-${reminder.id}'),
                                  tooltip: l10n.editReminderTooltip,
                                  onPressed: () async {
                                    final initial =
                                        DateTime.fromMillisecondsSinceEpoch(
                                          effectiveAt,
                                        ).toLocal();
                                    final selected =
                                        await _pickCustomReminderTime(
                                          sheetContext,
                                          initial: initial,
                                        );
                                    if (selected == null ||
                                        !sheetContext.mounted) {
                                      return;
                                    }
                                    final updated = await _saveReminder(
                                      sheetContext,
                                      ref,
                                      task,
                                      reminders,
                                      selected,
                                      existing: reminder,
                                    );
                                    if (updated != null &&
                                        sheetContext.mounted) {
                                      setSheetState(() {
                                        reminders =
                                            reminders
                                                .map(
                                                  (item) =>
                                                      item.id == updated.id
                                                      ? updated
                                                      : item,
                                                )
                                                .toList()
                                              ..sort(_compareReminderDtos);
                                      });
                                    }
                                  },
                                  icon: const Icon(LucideIcons.pencil300),
                                ),
                                IconButton(
                                  key: ValueKey(
                                    'delete-reminder-${reminder.id}',
                                  ),
                                  tooltip: l10n.deleteReminderTooltip,
                                  onPressed: () async {
                                    final deleted = await _deleteReminder(
                                      sheetContext,
                                      ref,
                                      task,
                                      reminder,
                                    );
                                    if (deleted && sheetContext.mounted) {
                                      setSheetState(() {
                                        reminders.removeWhere(
                                          (item) => item.id == reminder.id,
                                        );
                                      });
                                    }
                                  },
                                  icon: const Icon(LucideIcons.trash2300),
                                ),
                              ],
                            ),
                          );
                        },
                      ),
                    ),
                    const SizedBox(height: AppSpacing.md),
                    FilledButton.tonalIcon(
                      key: const ValueKey('add-reminder-button'),
                      onPressed: reminders.length >= _maxTaskReminders
                          ? null
                          : () async {
                              final selected = await _pickNewReminderTime(
                                sheetContext,
                                task,
                                reminders,
                              );
                              if (selected == null || !sheetContext.mounted) {
                                return;
                              }
                              final created = await _saveReminder(
                                sheetContext,
                                ref,
                                task,
                                reminders,
                                selected,
                              );
                              if (created != null && sheetContext.mounted) {
                                setSheetState(() {
                                  reminders = [...reminders, created]
                                    ..sort(_compareReminderDtos);
                                });
                              }
                            },
                      icon: const Icon(LucideIcons.plus300),
                      label: Text(l10n.addReminderButton),
                    ),
                    if (reminders.length >= _maxTaskReminders) ...[
                      const SizedBox(height: AppSpacing.xs),
                      Text(
                        l10n.reminderLimitReached,
                        textAlign: TextAlign.center,
                        style: Theme.of(sheetContext).textTheme.bodySmall,
                      ),
                    ],
                  ],
                ),
              ),
            ),
          );
        },
      ),
    );
  }

  Future<DateTime?> _pickNewReminderTime(
    BuildContext context,
    TaskDto task,
    List<ReminderDto> reminders,
  ) async {
    final l10n = AppLocalizations.of(context)!;
    final now = DateTime.now();
    final dueAt = taskDueInstant(task.due)?.toLocal();
    final usedTimes = reminders.map((reminder) => reminder.remindAt).toSet();
    final options = dueAt == null
        ? const <_ReminderQuickOption>[]
        : [
            _ReminderQuickOption(
              keyName: '5m',
              label: l10n.reminderFiveMinutesBefore,
              time: dueAt.subtract(const Duration(minutes: 5)),
            ),
            _ReminderQuickOption(
              keyName: '30m',
              label: l10n.reminderThirtyMinutesBefore,
              time: dueAt.subtract(const Duration(minutes: 30)),
            ),
            _ReminderQuickOption(
              keyName: '1h',
              label: l10n.reminderOneHourBefore,
              time: dueAt.subtract(const Duration(hours: 1)),
            ),
            _ReminderQuickOption(
              keyName: '1d',
              label: l10n.reminderOneDayBefore,
              time: dueAt.subtract(const Duration(days: 1)),
            ),
          ];
    final choice = await showModalBottomSheet<_ReminderTimeChoice>(
      context: context,
      isScrollControlled: true,
      showDragHandle: true,
      builder: (choiceContext) => SafeArea(
        child: Padding(
          padding: const EdgeInsets.fromLTRB(
            AppSpacing.md,
            0,
            AppSpacing.md,
            AppSpacing.md,
          ),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              if (options.isNotEmpty) ...[
                Text(
                  l10n.reminderQuickOptionsTitle,
                  style: Theme.of(choiceContext).textTheme.titleMedium,
                ),
                const SizedBox(height: AppSpacing.xs),
                for (final option in options)
                  ListTile(
                    key: ValueKey('reminder-preset-${option.keyName}'),
                    contentPadding: EdgeInsets.zero,
                    title: Text(option.label),
                    subtitle: Text(
                      formatReminderDateTime(
                        Localizations.localeOf(choiceContext).toLanguageTag(),
                        option.time.millisecondsSinceEpoch,
                      ),
                    ),
                    enabled:
                        option.time.isAfter(now) &&
                        !usedTimes.contains(option.time.millisecondsSinceEpoch),
                    onTap:
                        option.time.isAfter(now) &&
                            !usedTimes.contains(
                              option.time.millisecondsSinceEpoch,
                            )
                        ? () => Navigator.of(
                            choiceContext,
                          ).pop(_ReminderTimeChoice(time: option.time))
                        : null,
                  ),
              ],
              ListTile(
                key: const ValueKey('reminder-custom-time'),
                contentPadding: EdgeInsets.zero,
                leading: const Icon(LucideIcons.calendarClock300),
                title: Text(l10n.reminderCustomTime),
                onTap: () => Navigator.of(
                  choiceContext,
                ).pop(const _ReminderTimeChoice.custom()),
              ),
            ],
          ),
        ),
      ),
    );
    if (choice == null) {
      return null;
    }
    if (!choice.isCustom) {
      return choice.time;
    }
    if (!context.mounted) {
      return null;
    }
    return _pickCustomReminderTime(
      context,
      initial: now.add(const Duration(hours: 1)),
    );
  }

  Future<DateTime?> _pickCustomReminderTime(
    BuildContext context, {
    required DateTime initial,
  }) async {
    final now = DateTime.now();
    final safeInitial = initial.isAfter(now)
        ? initial
        : now.add(const Duration(hours: 1));
    final pickedDate = await showDatePicker(
      context: context,
      initialDate: safeInitial,
      firstDate: DateTime(now.year, now.month, now.day),
      lastDate: DateTime(2100),
    );
    if (pickedDate == null || !context.mounted) {
      return null;
    }
    final pickedTime = await showTimePicker(
      context: context,
      initialTime: TimeOfDay.fromDateTime(safeInitial),
    );
    if (pickedTime == null) {
      return null;
    }
    return DateTime(
      pickedDate.year,
      pickedDate.month,
      pickedDate.day,
      pickedTime.hour,
      pickedTime.minute,
    );
  }

  Future<ReminderDto?> _saveReminder(
    BuildContext context,
    WidgetRef ref,
    TaskDto task,
    List<ReminderDto> reminders,
    DateTime remindAt, {
    ReminderDto? existing,
  }) async {
    final l10n = AppLocalizations.of(context)!;
    final remindAtMs = remindAt.millisecondsSinceEpoch;
    if (!remindAt.isAfter(DateTime.now())) {
      _showReminderMessage(context, l10n.reminderMustBeFuture);
      return null;
    }
    if (reminders.any(
      (reminder) =>
          reminder.id != existing?.id && reminder.remindAt == remindAtMs,
    )) {
      _showReminderMessage(context, l10n.reminderDuplicateTime);
      return null;
    }
    if (existing == null && reminders.length >= _maxTaskReminders) {
      _showReminderMessage(context, l10n.reminderLimitReached);
      return null;
    }

    final notificationService = ref.read(reminderNotificationServiceProvider);
    final permissionsGranted = await notificationService.requestPermissions();
    try {
      final notifier = ref.read(taskRemindersProvider(task.id).notifier);
      final reminder = existing == null
          ? await notifier.createReminder(remindAtMs)
          : await notifier.updateReminder(existing.id, remindAtMs);
      if (permissionsGranted) {
        notificationService.requestReconciliation();
      } else {
        if (context.mounted) {
          _showReminderMessage(context, l10n.reminderPermissionDenied);
        }
      }
      return reminder;
    } catch (error) {
      if (context.mounted) {
        _showReminderMessage(
          context,
          l10n.failedToSaveReminder(bridgeErrorMessage(l10n, error)),
        );
      }
      return null;
    }
  }

  Future<bool> _deleteReminder(
    BuildContext context,
    WidgetRef ref,
    TaskDto task,
    ReminderDto reminder,
  ) async {
    final l10n = AppLocalizations.of(context)!;
    try {
      await ref
          .read(taskRemindersProvider(task.id).notifier)
          .deleteReminder(reminder.id);
      return true;
    } catch (error) {
      if (context.mounted) {
        _showReminderMessage(
          context,
          l10n.failedToSaveReminder(bridgeErrorMessage(l10n, error)),
        );
      }
      return false;
    }
  }

  void _showReminderMessage(BuildContext context, String message) {
    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(
        content: Text(message),
        margin: const EdgeInsets.all(AppSpacing.md),
      ),
    );
  }

  Future<void> _deleteTask(
    BuildContext context,
    WidgetRef ref,
    TaskDto task,
  ) async {
    final l10n = AppLocalizations.of(context)!;
    final descendantCount = await ref
        .read(tasksProvider(listId).notifier)
        .countDescendants(task.id);
    if (!context.mounted) {
      return;
    }
    final message = descendantCount == 0
        ? l10n.deleteTaskDialogMessage
        : l10n.deleteTaskDialogMessageWithDescendants(descendantCount);
    final confirmed = await showAppConfirmDialog(
      context: context,
      title: l10n.deleteTaskDialogTitle,
      message: message,
      cancelLabel: l10n.cancelButton,
      confirmLabel: l10n.deleteButton,
      isDestructive: true,
    );
    if (!confirmed) {
      return;
    }
    await ref.read(tasksProvider(listId).notifier).deleteTask(task.id);
    ref.invalidate(homeTasksProvider);
    if (context.mounted) {
      context.pop();
    }
  }

  Future<void> _setTaskStatus(
    BuildContext context,
    WidgetRef ref,
    TaskDto task,
    String status,
  ) async {
    if (status == 'done' || status == 'wont_do') {
      final tasks = await ref.read(tasksProvider(listId).future);
      if (!context.mounted) {
        return;
      }
      if (hasIncompleteDescendants(task.id, tasks)) {
        final l10n = AppLocalizations.of(context)!;
        final confirmed = await showAppConfirmDialog(
          context: context,
          title: status == 'wont_do'
              ? l10n.wontDoTaskDialogTitle
              : l10n.completeTaskDialogTitle,
          message: status == 'wont_do'
              ? l10n.wontDoTaskDialogMessage
              : l10n.completeTaskDialogMessage,
          cancelLabel: l10n.cancelButton,
          confirmLabel: l10n.continueButton,
        );
        if (!confirmed) {
          return;
        }
      }
    }

    await ref.read(tasksProvider(listId).notifier).setStatus(task.id, status);
    ref.invalidate(homeTasksProvider);
    if (context.mounted && (status == 'done' || status == 'wont_do')) {
      await _showLatestUndoSnackBar(context);
    }
  }
}

enum _TaskDetailAction {
  markDone,
  markWontDo,
  reopen,
  saveAsTemplate,
  createTaskSeries,
  delete,
}

List<PopupMenuEntry<_TaskDetailAction>> _taskDetailMenuItems({
  required AppLocalizations l10n,
  required TaskDto task,
}) {
  final items = <PopupMenuEntry<_TaskDetailAction>>[];
  if (task.status == 'todo' || task.status == 'in_progress') {
    items.addAll([
      PopupMenuItem(
        value: _TaskDetailAction.markDone,
        child: Text(l10n.markTaskDoneMenuItem),
      ),
      PopupMenuItem(
        value: _TaskDetailAction.markWontDo,
        child: Text(l10n.markTaskWontDoMenuItem),
      ),
    ]);
  } else if (isTaskClosed(task)) {
    items.add(
      PopupMenuItem(
        value: _TaskDetailAction.reopen,
        child: Text(l10n.reopenTaskMenuItem),
      ),
    );
  }
  if (items.isNotEmpty) {
    items.add(const PopupMenuDivider());
  }
  items.add(
    PopupMenuItem(
      value: _TaskDetailAction.saveAsTemplate,
      child: Text(l10n.saveAsTemplateMenuItem),
    ),
  );
  items.add(
    PopupMenuItem(
      value: _TaskDetailAction.createTaskSeries,
      child: Text(l10n.createTaskSeriesMenuItem),
    ),
  );
  items.add(const PopupMenuDivider());
  items.add(
    PopupMenuItem(
      value: _TaskDetailAction.delete,
      child: Text(l10n.deleteTaskMenuItem),
    ),
  );
  return items;
}

Future<void> _saveAsTemplate(
  BuildContext context,
  WidgetRef ref,
  TaskDto task,
) async {
  final l10n = AppLocalizations.of(context)!;
  final name = await showAppTextInputDialog(
    context: context,
    title: l10n.saveAsTemplateTitle,
    label: l10n.nameLabel,
    initialValue: task.title,
    cancelLabel: l10n.cancelButton,
    submitLabel: l10n.saveButton,
  );
  if (name == null || name.trim().isEmpty) return;
  await ref
      .read(bridgeServiceProvider)
      .saveTaskAsTemplate(
        taskId: task.id,
        name: name.trim(),
        defaultListId: task.listId,
      );
  if (context.mounted) {
    ScaffoldMessenger.of(
      context,
    ).showSnackBar(SnackBar(content: Text(l10n.templateSavedMessage)));
  }
}

Future<void> _createTaskSeries(
  BuildContext context,
  WidgetRef ref,
  TaskDto task,
) async {
  final bridge = ref.read(bridgeServiceProvider);
  final defaultTimeZone = await bridge.getLocalTimeZone();
  if (!context.mounted) return;
  final input = await showTaskSeriesDialog(context, null, defaultTimeZone);
  if (input == null) return;
  try {
    await bridge.validateRecurrenceRule(
      rrule: input.rrule,
      startsAt: input.startsAt,
      timeZone: input.timeZone,
    );
  } catch (_) {
    if (context.mounted) {
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(
          content: Text(AppLocalizations.of(context)!.scheduleValidationFailed),
        ),
      );
    }
    return;
  }
  await bridge.createTaskSeriesFromTask(
    taskId: task.id,
    targetListId: task.listId,
    rrule: input.rrule,
    startsAt: input.startsAt,
    timeZone: input.timeZone,
  );
  if (context.mounted) {
    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(
        content: Text(AppLocalizations.of(context)!.taskSeriesCreatedMessage),
      ),
    );
  }
}

Future<void> _showLatestUndoSnackBar(BuildContext context) async {
  final container = ProviderScope.containerOf(context, listen: false);
  container.invalidate(latestTaskUndoProvider);
  final undo = await container.read(latestTaskUndoProvider.future);
  if (!context.mounted || undo == null) {
    return;
  }

  final l10n = AppLocalizations.of(context)!;
  final messenger = ScaffoldMessenger.of(context);
  messenger.hideCurrentSnackBar();
  messenger.showSnackBar(
    SnackBar(
      duration: const Duration(seconds: 4),
      persist: false,
      content: Text(_undoMessage(l10n, undo.operationType)),
      margin: const EdgeInsets.all(AppSpacing.md),
      action: SnackBarAction(
        label: l10n.undoActionLabel,
        onPressed: () {
          unawaited(_applyUndo(container, messenger, l10n, undo.id));
        },
      ),
    ),
  );
}

Future<void> _applyUndo(
  ProviderContainer container,
  ScaffoldMessengerState messenger,
  AppLocalizations l10n,
  String undoId,
) async {
  messenger.hideCurrentSnackBar();
  try {
    await container.read(latestTaskUndoProvider.notifier).undo(undoId);
    messenger.showSnackBar(
      SnackBar(
        duration: const Duration(seconds: 4),
        persist: false,
        content: Text(l10n.undoSuccessMessage),
        margin: const EdgeInsets.all(AppSpacing.md),
      ),
    );
  } catch (error) {
    messenger.showSnackBar(
      SnackBar(
        duration: const Duration(seconds: 4),
        persist: false,
        content: Text(l10n.undoFailedMessage(bridgeErrorMessage(l10n, error))),
        margin: const EdgeInsets.all(AppSpacing.md),
      ),
    );
  }
}

String _undoMessage(AppLocalizations l10n, String operationType) {
  return switch (operationType) {
    'complete' => l10n.undoCloseMessage,
    'edit' => l10n.undoEditMessage,
    _ => l10n.undoEditMessage,
  };
}

String _detailTaskRowSemanticLabel({
  required AppLocalizations l10n,
  required String title,
  required String status,
  required String priority,
  required String? dueLabel,
  required int depth,
}) {
  final parts = <String>[
    title,
    l10n.taskRowStatusSemantics(status),
    l10n.taskPriority(priority),
    if (dueLabel != null) l10n.taskRowDueSemantics(dueLabel),
    if (depth > 0) l10n.taskRowSubtaskLevelSemantics(depth + 1),
    l10n.taskRowOpenHint,
  ];
  return parts.join('. ');
}

String formatReminderDateTime(String locale, int epochMs) {
  final dateTime = DateTime.fromMillisecondsSinceEpoch(epochMs).toLocal();
  return '${DateFormat.MMMd(locale).format(dateTime)} '
      '${DateFormat.jm(locale).format(dateTime)}';
}

String _focusSummaryLabel(
  AppLocalizations l10n, {
  required int? estimatedMinutes,
  required Duration actualDuration,
}) {
  final hasActual = actualDuration > Duration.zero;
  final hasEstimate = estimatedMinutes != null;
  if (!hasActual && !hasEstimate) {
    return l10n.focusNoActualValue;
  }
  final actual = _compactDuration(actualDuration);
  if (!hasEstimate) {
    return l10n.focusActualOnlyValue(actual);
  }
  return l10n.focusEstimateActualValue(
    hasActual ? actual : _compactDuration(Duration.zero),
    _compactDuration(Duration(minutes: estimatedMinutes)),
  );
}

String _compactDuration(Duration duration) {
  final minutes = duration.inMinutes;
  if (minutes < 60) {
    return '${minutes}m';
  }
  final hours = minutes ~/ 60;
  final remainder = minutes % 60;
  return remainder == 0 ? '${hours}h' : '${hours}h ${remainder}m';
}

TaskDto? _findTaskById(List<TaskDto> tasks, String taskId) {
  for (final task in tasks) {
    if (task.id == taskId) {
      return task;
    }
  }
  return null;
}

const Object _unchangedDue = Object();
const Object _unchangedScheduledAt = Object();
const Object _unchangedEstimatedMinutes = Object();

enum _TaskDueMode { date, dateTime }

const _maxTaskReminders = 5;

int _compareReminderDtos(ReminderDto left, ReminderDto right) {
  final byTime = effectiveReminderAt(
    left,
  ).compareTo(effectiveReminderAt(right));
  if (byTime != 0) {
    return byTime;
  }
  return left.id.compareTo(right.id);
}

class _ReminderQuickOption {
  const _ReminderQuickOption({
    required this.keyName,
    required this.label,
    required this.time,
  });

  final String keyName;
  final String label;
  final DateTime time;
}

class _ReminderTimeChoice {
  const _ReminderTimeChoice({required this.time}) : isCustom = false;

  const _ReminderTimeChoice.custom() : time = null, isCustom = true;

  final DateTime? time;
  final bool isCustom;
}

const EdgeInsets _inlineEditorPadding = EdgeInsets.all(AppSpacing.sm);

class _InlineTitleEditor extends StatefulWidget {
  const _InlineTitleEditor({
    super.key,
    required this.title,
    required this.isClosed,
    required this.semanticLabel,
    required this.onSave,
  });

  final String title;
  final bool isClosed;
  final String semanticLabel;
  final Future<bool> Function(String title) onSave;

  @override
  State<_InlineTitleEditor> createState() => _InlineTitleEditorState();
}

class _InlineTitleEditorState extends State<_InlineTitleEditor> {
  late final TextEditingController _controller;
  late final FocusNode _focusNode;
  bool _editing = false;
  bool _saving = false;

  @override
  void initState() {
    super.initState();
    _controller = TextEditingController(text: widget.title);
    _focusNode = FocusNode();
    _focusNode.addListener(_handleFocusChange);
  }

  @override
  void didUpdateWidget(covariant _InlineTitleEditor oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (!_editing && oldWidget.title != widget.title) {
      _controller.text = widget.title;
    }
  }

  @override
  void dispose() {
    _focusNode.removeListener(_handleFocusChange);
    _focusNode.dispose();
    _controller.dispose();
    super.dispose();
  }

  void _handleFocusChange() {
    if (_editing && !_focusNode.hasFocus) {
      unawaited(_commit());
    }
  }

  void _startEditing() {
    if (_editing) {
      return;
    }
    setState(() {
      _editing = true;
      _controller.text = widget.title;
      _controller.selection = TextSelection(
        baseOffset: 0,
        extentOffset: _controller.text.length,
      );
    });
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) {
        _focusNode.requestFocus();
      }
    });
  }

  bool get _hasComposingRange {
    final range = _controller.value.composing;
    return range.isValid && !range.isCollapsed;
  }

  Future<void> _commit({bool fromSubmitted = false}) async {
    if (_saving) {
      return;
    }
    if (fromSubmitted && _hasComposingRange) {
      return;
    }
    final nextTitle = _controller.text.trim();
    if (nextTitle.isEmpty) {
      _controller.text = widget.title;
      if (mounted) {
        setState(() => _editing = false);
      }
      return;
    }
    if (nextTitle == widget.title) {
      if (mounted) {
        setState(() => _editing = false);
      }
      return;
    }
    setState(() => _saving = true);
    final saved = await widget.onSave(nextTitle);
    if (!mounted) {
      return;
    }
    setState(() {
      _saving = false;
      _editing = !saved;
    });
    if (!saved) {
      _focusNode.requestFocus();
    }
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final colorScheme = theme.colorScheme;
    final titleStyle = theme.textTheme.headlineSmall?.copyWith(
      fontWeight: FontWeight.w500,
      letterSpacing: -0.35,
      decoration: widget.isClosed ? TextDecoration.lineThrough : null,
      color: widget.isClosed ? colorScheme.onSurfaceVariant : null,
    );
    final titleStrut = StrutStyle.fromTextStyle(
      titleStyle ?? const TextStyle(),
      forceStrutHeight: true,
    );
    if (_editing) {
      return SizedBox(
        width: double.infinity,
        child: Semantics(
          label: widget.semanticLabel,
          textField: true,
          child: Padding(
            padding: _inlineEditorPadding,
            child: EditableText(
              key: const ValueKey('task-title-inline-field'),
              controller: _controller,
              focusNode: _focusNode,
              cursorColor: colorScheme.primary,
              backgroundCursorColor: colorScheme.surfaceContainerHighest,
              readOnly: _saving,
              autofocus: true,
              minLines: 1,
              maxLines: null,
              style: titleStyle ?? const TextStyle(),
              strutStyle: titleStrut,
              keyboardType: TextInputType.multiline,
              textInputAction: TextInputAction.done,
              onSubmitted: (_) => unawaited(_commit(fromSubmitted: true)),
              onTapOutside: (_) => _focusNode.unfocus(),
            ),
          ),
        ),
      );
    }

    return SizedBox(
      width: double.infinity,
      child: Semantics(
        button: true,
        label: widget.semanticLabel,
        child: InkWell(
          borderRadius: BorderRadius.circular(8),
          onTap: _startEditing,
          child: Padding(
            padding: _inlineEditorPadding,
            child: AppAnimatedTaskTitle(
              widget.title,
              textKey: const ValueKey('task-title-inline-read-text'),
              isDone: widget.isClosed,
              strutStyle: titleStrut,
              style: titleStyle,
            ),
          ),
        ),
      ),
    );
  }
}

class _InlineNoteEditor extends StatefulWidget {
  const _InlineNoteEditor({
    super.key,
    required this.note,
    required this.placeholder,
    required this.semanticLabel,
    required this.onSave,
  });

  final String note;
  final String placeholder;
  final String semanticLabel;
  final Future<bool> Function(String note) onSave;

  @override
  State<_InlineNoteEditor> createState() => _InlineNoteEditorState();
}

class _InlineNoteEditorState extends State<_InlineNoteEditor> {
  late final TextEditingController _controller;
  late final FocusNode _focusNode;
  bool _editing = false;
  bool _saving = false;

  @override
  void initState() {
    super.initState();
    _controller = TextEditingController(text: widget.note);
    _focusNode = FocusNode();
    _focusNode.addListener(_handleFocusChange);
  }

  @override
  void didUpdateWidget(covariant _InlineNoteEditor oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (!_editing && oldWidget.note != widget.note) {
      _controller.text = widget.note;
    }
  }

  @override
  void dispose() {
    _focusNode.removeListener(_handleFocusChange);
    _focusNode.dispose();
    _controller.dispose();
    super.dispose();
  }

  void _handleFocusChange() {
    if (_editing && !_focusNode.hasFocus) {
      unawaited(_commit());
    }
  }

  void _startEditing() {
    if (_editing) {
      return;
    }
    setState(() {
      _editing = true;
      _controller.text = widget.note;
      _controller.selection = TextSelection.collapsed(
        offset: _controller.text.length,
      );
    });
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) {
        _focusNode.requestFocus();
      }
    });
  }

  Future<void> _commit() async {
    if (_saving) {
      return;
    }
    final nextNote = _controller.text;
    if (nextNote == widget.note) {
      if (mounted) {
        setState(() => _editing = false);
      }
      return;
    }
    setState(() => _saving = true);
    final saved = await widget.onSave(nextNote);
    if (!mounted) {
      return;
    }
    setState(() {
      _saving = false;
      _editing = !saved;
    });
    if (!saved) {
      _focusNode.requestFocus();
    }
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final colorScheme = theme.colorScheme;
    final noteStyle = theme.textTheme.bodyLarge?.copyWith(
      color: colorScheme.onSurfaceVariant,
      height: 1.35,
    );
    final noteStrut = StrutStyle.fromTextStyle(
      noteStyle ?? const TextStyle(height: 1.35),
      forceStrutHeight: true,
    );
    if (_editing) {
      return SizedBox(
        width: double.infinity,
        child: Semantics(
          label: widget.semanticLabel,
          textField: true,
          child: Padding(
            padding: _inlineEditorPadding,
            child: EditableText(
              key: const ValueKey('task-note-inline-field'),
              controller: _controller,
              focusNode: _focusNode,
              cursorColor: colorScheme.primary,
              backgroundCursorColor: colorScheme.surfaceContainerHighest,
              readOnly: _saving,
              autofocus: true,
              minLines: 2,
              maxLines: 6,
              style: noteStyle ?? const TextStyle(height: 1.35),
              strutStyle: noteStrut,
              keyboardType: TextInputType.multiline,
              onTapOutside: (_) => _focusNode.unfocus(),
            ),
          ),
        ),
      );
    }

    final text = widget.note.isEmpty ? widget.placeholder : widget.note;
    return SizedBox(
      width: double.infinity,
      child: Semantics(
        button: true,
        label: widget.semanticLabel,
        child: InkWell(
          borderRadius: BorderRadius.circular(8),
          onTap: _startEditing,
          child: Padding(
            padding: _inlineEditorPadding,
            child: Text(
              text,
              key: const ValueKey('task-note-inline-read-text'),
              strutStyle: noteStrut,
              style: noteStyle,
            ),
          ),
        ),
      ),
    );
  }
}

class _ParentTaskLink extends StatelessWidget {
  const _ParentTaskLink({
    required this.parentTask,
    required this.tooltip,
    required this.semanticLabel,
    required this.onTap,
  });

  final TaskDto parentTask;
  final String tooltip;
  final String semanticLabel;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final colorScheme = theme.colorScheme;
    return Tooltip(
      message: tooltip,
      child: Semantics(
        label: semanticLabel,
        button: true,
        child: Material(
          type: MaterialType.transparency,
          child: InkWell(
            borderRadius: BorderRadius.circular(8),
            onTap: onTap,
            child: ConstrainedBox(
              constraints: const BoxConstraints(minHeight: 48),
              child: Padding(
                padding: const EdgeInsetsDirectional.fromSTEB(
                  AppSpacing.sm,
                  AppSpacing.xs,
                  AppSpacing.sm,
                  AppSpacing.xs,
                ),
                child: Row(
                  children: [
                    Icon(
                      LucideIcons.cornerUpLeft300,
                      size: 18,
                      color: colorScheme.primary,
                    ),
                    const SizedBox(width: AppSpacing.xs),
                    Expanded(
                      child: Text(
                        parentTask.title,
                        key: ValueKey('parent-task-link-${parentTask.id}'),
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: theme.textTheme.labelLarge?.copyWith(
                          color: colorScheme.primary,
                        ),
                      ),
                    ),
                  ],
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }
}

class _EditableTaskMetadata extends StatelessWidget {
  const _EditableTaskMetadata({
    required this.task,
    required this.reminders,
    required this.stats,
    required this.actualDuration,
    required this.locale,
    required this.onStartFocus,
    required this.onSelectDueDate,
    required this.onClearDueDate,
    required this.onSelectPlan,
    required this.onSelectPriority,
    required this.onSelectReminder,
  });

  final TaskDto task;
  final List<ReminderDto> reminders;
  final SubtaskStats stats;
  final Duration actualDuration;
  final String locale;
  final VoidCallback? onStartFocus;
  final VoidCallback onSelectDueDate;
  final VoidCallback? onClearDueDate;
  final VoidCallback onSelectPlan;
  final VoidCallback onSelectPriority;
  final VoidCallback onSelectReminder;

  @override
  Widget build(BuildContext context) {
    final l10n = AppLocalizations.of(context)!;
    final overdue = isTaskOverdue(task);
    final dueLabel = formatRelativeDueDate(l10n, locale, task.due);
    final reminderLabel = switch (reminders.length) {
      0 => l10n.reminderChipEmpty,
      1 => formatReminderDateTime(
        locale,
        effectiveReminderAt(reminders.single),
      ),
      _ => l10n.reminderCount(reminders.length),
    };
    return Column(
      children: [
        if (onStartFocus != null)
          _DetailPropertyRow(
            key: ValueKey('task-focus-row-${task.id}'),
            icon: LucideIcons.timer300,
            property: l10n.focusTitle,
            label: l10n.focusStartButton,
            tooltip: l10n.focusStartButton,
            onTap: onStartFocus,
          ),
        _DetailPropertyRow(
          key: ValueKey('task-focus-summary-${task.id}'),
          icon: LucideIcons.clock300,
          property: l10n.focusEstimateActualLabel,
          label: _focusSummaryLabel(
            l10n,
            estimatedMinutes: task.estimatedMinutes,
            actualDuration: actualDuration,
          ),
        ),
        _DetailPropertyRow(
          icon: taskStatusIcon(task.status),
          property: taskStatusLabel(l10n, task.status),
          label: '',
        ),
        _DetailPropertyRow(
          key: ValueKey('task-due-chip-${task.id}'),
          icon: LucideIcons.calendarDays300,
          property: l10n.dueDateLabel,
          label: dueLabel,
          tooltip: task.due == null
              ? l10n.setDueDateButton
              : l10n.changeDueDateTooltip,
          semanticLabel: overdue ? l10n.taskDueOverdue(dueLabel) : null,
          emphasisColor: overdue ? priorityDotColor(3) : null,
          onTap: onSelectDueDate,
          clearKey: ValueKey('task-clear-due-${task.id}'),
          clearTooltip: l10n.clearDueDateButton,
          onClear: onClearDueDate,
        ),
        _DetailPropertyRow(
          key: ValueKey('task-plan-row-${task.id}'),
          icon: LucideIcons.calendarClock300,
          property: l10n.taskCreatePlanLabel,
          label: formatTaskPlanValue(
            l10n,
            locale: locale,
            scheduledAt: task.scheduledAt,
            estimatedMinutes: task.estimatedMinutes,
          ),
          tooltip: l10n.taskCreatePlanTooltip,
          onTap: onSelectPlan,
        ),
        _DetailPropertyRow(
          key: ValueKey('task-priority-chip-${task.id}'),
          icon: LucideIcons.flag300,
          property: l10n.priorityLabel,
          label: taskPriorityLabel(l10n, task.priority),
          tooltip: l10n.changePriorityTooltip,
          semanticLabel: l10n.taskPriority(
            taskPriorityLabel(l10n, task.priority),
          ),
          marker: task.priority == 0
              ? null
              : ExcludeSemantics(
                  child: PriorityDot(
                    key: ValueKey('task-priority-dot-${task.id}'),
                    priority: task.priority,
                    isMuted: isTaskClosed(task),
                  ),
                ),
          onTap: onSelectPriority,
        ),
        _DetailPropertyRow(
          key: ValueKey('task-reminder-chip-${task.id}'),
          icon: LucideIcons.bell300,
          property: l10n.reminderChipEmpty,
          label: reminderLabel,
          tooltip: reminders.isEmpty
              ? l10n.reminderChipTooltipSet
              : l10n.reminderChipTooltipChange,
          onTap: onSelectReminder,
        ),
        if (stats.hasDescendants)
          _DetailPropertyRow(
            icon: LucideIcons.gitBranch300,
            property: l10n.subtasksTitle,
            label: l10n.subtaskProgress(stats.doneCount, stats.totalCount),
          ),
      ],
    );
  }
}

class _DetailPropertyRow extends StatelessWidget {
  const _DetailPropertyRow({
    super.key,
    required this.icon,
    required this.property,
    required this.label,
    this.tooltip,
    this.semanticLabel,
    this.emphasisColor,
    this.onTap,
    this.clearKey,
    this.clearTooltip,
    this.onClear,
    this.marker,
  });

  final IconData icon;
  final String property;
  final String label;
  final String? tooltip;
  final String? semanticLabel;
  final Color? emphasisColor;
  final VoidCallback? onTap;
  final Key? clearKey;
  final String? clearTooltip;
  final VoidCallback? onClear;
  final Widget? marker;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final colorScheme = theme.colorScheme;
    final tint =
        emphasisColor ??
        (onTap == null ? colorScheme.onSurfaceVariant : colorScheme.primary);
    final content = DecoratedBox(
      decoration: BoxDecoration(
        border: Border(
          bottom: BorderSide(color: colorScheme.outlineVariant, width: 0.7),
        ),
      ),
      child: ConstrainedBox(
        constraints: const BoxConstraints(minHeight: 52),
        child: Row(
          children: [
            SizedBox.square(
              dimension: 36,
              child: Icon(icon, size: 17, color: colorScheme.onSurfaceVariant),
            ),
            const SizedBox(width: AppSpacing.xs),
            Expanded(
              flex: 4,
              child: Text(
                property,
                style: theme.textTheme.labelMedium?.copyWith(
                  color: colorScheme.onSurfaceVariant,
                ),
              ),
            ),
            if (label.isNotEmpty) ...[
              const SizedBox(width: AppSpacing.sm),
              if (marker != null) ...[
                marker!,
                const SizedBox(width: AppSpacing.xs),
              ],
              Flexible(
                flex: 5,
                child: Text(
                  label,
                  textAlign: TextAlign.end,
                  softWrap: true,
                  style: theme.textTheme.bodyMedium?.copyWith(color: tint),
                ),
              ),
            ],
            if (onClear != null)
              IconButton(
                key: clearKey,
                tooltip: clearTooltip,
                icon: const Icon(LucideIcons.x300, size: 16),
                onPressed: onClear,
              )
            else if (onTap != null)
              const SizedBox(
                width: 40,
                child: Icon(LucideIcons.chevronRight300, size: 16),
              ),
          ],
        ),
      ),
    );
    final wrapped = onTap == null
        ? content
        : Material(
            type: MaterialType.transparency,
            child: InkWell(onTap: onTap, child: content),
          );
    final effectiveSemanticLabel =
        semanticLabel ?? (onTap == null ? null : label);
    final semantics = effectiveSemanticLabel == null && onTap == null
        ? wrapped
        : Semantics(
            label: effectiveSemanticLabel,
            button: onTap != null,
            enabled: onTap != null,
            child: wrapped,
          );
    if (tooltip == null) {
      return semantics;
    }
    return Tooltip(message: tooltip!, child: semantics);
  }
}
