part of '../task_components.dart';

class TaskMetadataItem {
  const TaskMetadataItem({
    required this.icon,
    required this.label,
    this.semanticLabel,
    this.emphasisColor,
  });

  final IconData icon;
  final String label;

  /// Overrides the accessible label for this pill (e.g. to add "overdue"
  /// context that isn't carried by color alone). Defaults to the visible
  /// [label] when null.
  final String? semanticLabel;

  /// Optional accent color (e.g. coral for an overdue due date) applied to
  /// the icon and text instead of the default primary tint.
  final Color? emphasisColor;
}

class TaskMetadata extends StatelessWidget {
  const TaskMetadata({
    super.key,
    required this.items,
    this.priority = 0,
    this.priorityDotKey,
    this.prioritySemanticLabel,
    this.isPriorityMuted = false,
  });

  final List<TaskMetadataItem> items;
  final int priority;
  final Key? priorityDotKey;
  final String? prioritySemanticLabel;
  final bool isPriorityMuted;

  @override
  Widget build(BuildContext context) {
    if (items.isEmpty && priority <= 0) {
      return const SizedBox.shrink();
    }

    return Wrap(
      spacing: AppSpacing.xs,
      runSpacing: AppSpacing.xs,
      crossAxisAlignment: WrapCrossAlignment.center,
      children: [
        if (priority > 0)
          PriorityDot(
            key: priorityDotKey,
            priority: priority,
            semanticLabel: prioritySemanticLabel,
            isMuted: isPriorityMuted,
          ),
        for (final item in items) _TaskMetadataLabel(item: item),
      ],
    );
  }
}

class _TaskMetadataLabel extends StatelessWidget {
  const _TaskMetadataLabel({required this.item});

  final TaskMetadataItem item;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final label = Text(
      item.label,
      maxLines: 1,
      overflow: TextOverflow.ellipsis,
      style: theme.textTheme.labelMedium?.copyWith(
        color: item.emphasisColor ?? theme.colorScheme.onSurfaceVariant,
        fontWeight: FontWeight.w500,
      ),
    );
    return item.semanticLabel == null
        ? label
        : Semantics(label: item.semanticLabel, child: label);
  }
}

List<TaskMetadataItem> taskMetadataItemsFor({
  required AppLocalizations l10n,
  required String locale,
  required TaskDto task,
  required SubtaskStats stats,
  bool includeNoDueDate = false,
  bool includeStatus = false,
  bool includeSubtaskProgress = true,
  bool includeWontDoStatus = true,
  String? listName,
}) {
  final overdue = isTaskOverdue(task);
  return [
    if (includeStatus || (includeWontDoStatus && task.status == 'wont_do'))
      TaskMetadataItem(
        icon: taskStatusIcon(task.status),
        label: taskStatusLabel(l10n, task.status),
      ),
    if (task.due != null || includeNoDueDate)
      TaskMetadataItem(
        icon: LucideIcons.calendarDays300,
        label: formatRelativeDueDate(l10n, locale, task.due),
        emphasisColor: overdue ? _priorityHighCoral : null,
        semanticLabel: overdue
            ? l10n.taskDueOverdue(formatRelativeDueDate(l10n, locale, task.due))
            : null,
      ),
    if (includeSubtaskProgress && stats.hasDescendants)
      TaskMetadataItem(
        icon: LucideIcons.gitBranch300,
        label: l10n.subtaskProgress(stats.doneCount, stats.totalCount),
      ),
    if (listName != null)
      TaskMetadataItem(icon: LucideIcons.listTodo300, label: listName),
  ];
}

String taskStatusLabel(AppLocalizations l10n, String status) {
  return switch (status) {
    'todo' => l10n.statusTodo,
    'in_progress' => l10n.statusInProgress,
    'done' => l10n.statusDone,
    'wont_do' => l10n.statusWontDo,
    _ => status,
  };
}

String taskPriorityLabel(AppLocalizations l10n, int priority) {
  return switch (priority) {
    1 => l10n.priorityLow,
    2 => l10n.priorityMedium,
    3 => l10n.priorityHigh,
    _ => l10n.priorityNone,
  };
}

String formatDueDate(AppLocalizations l10n, TaskDueDto? due) {
  if (due == null) {
    return l10n.noDueDate;
  }
  final date = taskDueDisplayDate(due);
  final dateLabel = DateFormat.yMMMd(l10n.localeName).format(date);
  if (taskDueIsDateOnly(due)) {
    return dateLabel;
  }
  final timeZone = taskDueSavedTimeZone(due)!;
  return '$dateLabel · ${DateFormat.jm(l10n.localeName).format(date)} '
      '$timeZone (${taskDueUtcOffsetLabel(date)})';
}

String formatHomeHeaderDate(String locale, DateTime date) {
  return DateFormat.MMMEd(locale).format(date);
}

/// Formats compact row metadata as "Today"/"Tomorrow"/a short localized date.
/// Datetime rows include the saved wall-clock time, while the full IANA zone
/// and UTC offset remain available in Task detail and Calendar. Long zone IDs
/// would otherwise dominate and truncate the task stream.
String formatRelativeDueDate(
  AppLocalizations l10n,
  String locale,
  TaskDueDto? due,
) {
  if (due == null) {
    return l10n.noDueDate;
  }
  final dueDateTime = taskDueDisplayDate(due);
  final dueDate = DateTime(
    dueDateTime.year,
    dueDateTime.month,
    dueDateTime.day,
  );
  final today = DateTime.now();
  final todayDate = DateTime(today.year, today.month, today.day);
  final dayDiff = DateTime.utc(dueDate.year, dueDate.month, dueDate.day)
      .difference(DateTime.utc(todayDate.year, todayDate.month, todayDate.day))
      .inDays;
  final dateLabel = switch (dayDiff) {
    0 => l10n.dueToday,
    1 => l10n.dueTomorrow,
    _ => DateFormat.MMMd(locale).format(dueDate),
  };
  if (taskDueIsDateOnly(due)) {
    return dateLabel;
  }
  return '$dateLabel · ${DateFormat.jm(locale).format(dueDateTime)}';
}

/// Whether [task] has a due date in the past and is not yet done. Used to
/// tint the Due pill coral without relying on color alone (see
/// [TaskMetadataItem.semanticLabel]).
bool isTaskOverdue(TaskDto task) {
  if (task.due == null || isTaskClosed(task)) {
    return false;
  }
  return taskDueIsOverdue(task.due);
}

bool isTaskClosed(TaskDto task) =>
    task.status == 'done' || task.status == 'wont_do';

IconData taskStatusIcon(String status) {
  return switch (status) {
    'done' => LucideIcons.circleCheck300,
    'wont_do' => LucideIcons.ban300,
    'in_progress' => LucideIcons.clock300,
    _ => LucideIcons.circle300,
  };
}

/// Formats an absolute epoch-millisecond timestamp (e.g. `Task.createdAt`)
/// as a localized calendar date, replacing the raw-epoch display bug.
String formatAbsoluteDate(String locale, int epochMs) {
  final date = DateTime.fromMillisecondsSinceEpoch(epochMs).toLocal();
  return DateFormat.yMMMd(locale).format(date);
}

enum HomeDueDateTone { overdue, today, future }
