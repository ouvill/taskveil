part of '../tasks_screen.dart';

class _HomeTasksHeader extends StatelessWidget {
  const _HomeTasksHeader({
    required this.sortMenu,
    required this.listActionsMenu,
  });

  final Widget sortMenu;
  final Widget? listActionsMenu;

  @override
  Widget build(BuildContext context) {
    final l10n = AppLocalizations.of(context)!;
    final theme = Theme.of(context);
    final colorScheme = theme.colorScheme;
    final locale = Localizations.localeOf(context).toLanguageTag();
    final today = formatHomeHeaderDate(locale, DateTime.now());

    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Row(
          crossAxisAlignment: CrossAxisAlignment.end,
          children: [
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    today,
                    style: theme.textTheme.labelMedium?.copyWith(
                      color: colorScheme.onSurfaceVariant,
                      fontWeight: FontWeight.w600,
                      letterSpacing: 0.7,
                    ),
                  ),
                  const SizedBox(height: 2),
                  Text(
                    l10n.homeTitle,
                    style: theme.textTheme.headlineMedium?.copyWith(
                      color: colorScheme.onSurface,
                      fontWeight: FontWeight.w700,
                      letterSpacing: -0.6,
                      height: 1.05,
                    ),
                  ),
                ],
              ),
            ),
            if (listActionsMenu != null) ...[
              listActionsMenu!,
              const SizedBox(width: AppSpacing.xs),
            ],
            const AppHeaderSearchAction(),
            Padding(padding: const EdgeInsets.only(bottom: 1), child: sortMenu),
          ],
        ),
      ],
    );
  }
}

class _HomeClearState extends StatelessWidget {
  const _HomeClearState({required this.l10n});

  final AppLocalizations l10n;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final colorScheme = theme.colorScheme;
    return Padding(
      padding: const EdgeInsets.fromLTRB(4, 28, 4, 30),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          DecoratedBox(
            decoration: BoxDecoration(
              color: colorScheme.primaryContainer,
              shape: BoxShape.circle,
            ),
            child: Padding(
              padding: const EdgeInsets.all(12),
              child: Icon(
                LucideIcons.sprout300,
                size: 24,
                color: colorScheme.primary,
              ),
            ),
          ),
          const SizedBox(height: AppSpacing.lg),
          Text(
            l10n.homeClearTitle,
            style: theme.textTheme.headlineSmall?.copyWith(
              fontWeight: FontWeight.w700,
            ),
          ),
          const SizedBox(height: AppSpacing.sm),
          Text(
            l10n.homeClearBody,
            style: theme.textTheme.bodyLarge?.copyWith(
              color: colorScheme.onSurfaceVariant,
            ),
          ),
        ],
      ),
    );
  }
}

enum _HomeSectionKind { today }

enum _HomeDueTiming { overdue, today, future }

class _HomeSectionData {
  const _HomeSectionData({
    required this.kind,
    required this.count,
    required this.rows,
  });

  final _HomeSectionKind kind;
  final int count;
  final List<_HomeSectionRowData> rows;
}

class _HomeSectionRowData {
  const _HomeSectionRowData({
    required this.node,
    required this.rootListId,
    required this.parentTaskName,
    this.countsInSection = false,
    this.pendingCompletionKey,
    this.disableInteractions = false,
    this.isPendingRoot = false,
  });

  final FlattenedTaskTreeNode node;
  final String rootListId;
  final String? parentTaskName;
  final bool countsInSection;
  final String? pendingCompletionKey;
  final bool disableInteractions;
  final bool isPendingRoot;
}

class _PendingHomeCompletion {
  const _PendingHomeCompletion({required this.rows, required this.section});

  final List<_HomeSectionRowData> rows;
  final _HomeSectionKind section;
}

class _PendingListCompletion {
  const _PendingListCompletion({required this.root});

  final TaskTreeNode root;
}

class _HomeSectionsPanelSliver extends StatelessWidget {
  const _HomeSectionsPanelSliver({
    required this.sections,
    required this.isTodayCollapsed,
    required this.onToggleSection,
    required this.rowBuilder,
  });

  final List<_HomeSectionData> sections;
  final bool isTodayCollapsed;
  final ValueChanged<_HomeSectionKind> onToggleSection;
  final Widget Function(
    BuildContext context,
    _HomeSectionRowData row,
    _HomeSectionKind section,
  )
  rowBuilder;

  @override
  Widget build(BuildContext context) {
    return SliverMainAxisGroup(
      slivers: [
        for (var index = 0; index < sections.length; index += 1) ...[
          _HomeSectionSliver(
            data: sections[index],
            isExpanded: !isTodayCollapsed,
            onToggle: () => onToggleSection(sections[index].kind),
            rowBuilder: rowBuilder,
          ),
          if (index < sections.length - 1)
            const SliverToBoxAdapter(child: SizedBox(height: AppSpacing.lg)),
        ],
      ],
    );
  }
}

class _HomeSectionSliver extends StatelessWidget {
  const _HomeSectionSliver({
    required this.data,
    required this.isExpanded,
    required this.onToggle,
    required this.rowBuilder,
  });

  final _HomeSectionData data;
  final bool isExpanded;
  final VoidCallback onToggle;
  final Widget Function(
    BuildContext context,
    _HomeSectionRowData row,
    _HomeSectionKind section,
  )
  rowBuilder;

  @override
  Widget build(BuildContext context) {
    return SliverMainAxisGroup(
      slivers: [
        SliverToBoxAdapter(
          child: _HomeSectionHeader(
            data: data,
            isExpanded: isExpanded,
            onToggle: onToggle,
          ),
        ),
        if (isExpanded && data.rows.isNotEmpty)
          SliverPadding(
            padding: const EdgeInsets.only(top: AppSpacing.xs),
            sliver: SliverList.builder(
              itemCount: data.rows.length * 2 - 1,
              itemBuilder: (context, index) {
                if (index.isOdd) {
                  return const SizedBox(height: 2);
                }
                return rowBuilder(context, data.rows[index ~/ 2], data.kind);
              },
            ),
          ),
      ],
    );
  }
}

class _HomeSectionHeader extends StatelessWidget {
  const _HomeSectionHeader({
    required this.data,
    required this.isExpanded,
    required this.onToggle,
  });

  final _HomeSectionData data;
  final bool isExpanded;
  final VoidCallback onToggle;

  @override
  Widget build(BuildContext context) {
    final l10n = AppLocalizations.of(context)!;
    final theme = Theme.of(context);
    final colorScheme = theme.colorScheme;
    final title = _homeSectionTitle(l10n, data.kind);
    final tooltip = isExpanded
        ? l10n.hideHomeSectionTooltip(title)
        : l10n.showHomeSectionTooltip(title);
    return Tooltip(
      message: tooltip,
      child: Semantics(
        button: true,
        label: tooltip,
        child: InkWell(
          borderRadius: BorderRadius.circular(AppRadius.sm),
          onTap: onToggle,
          child: Padding(
            padding: const EdgeInsets.symmetric(vertical: 4),
            child: Row(
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
                Expanded(
                  child: Text(
                    title,
                    style: theme.textTheme.labelLarge?.copyWith(
                      color: colorScheme.onSurface,
                      fontWeight: FontWeight.w700,
                      letterSpacing: 0.6,
                    ),
                  ),
                ),
                _HomeCountLabel(
                  key: ValueKey('home-section-count-${data.kind.name}'),
                  count: data.count,
                ),
                const SizedBox(width: AppSpacing.xs),
                SizedBox(
                  width: 40,
                  height: 40,
                  child: Center(
                    child: Icon(
                      isExpanded
                          ? LucideIcons.chevronUp300
                          : LucideIcons.chevronDown300,
                      size: 18,
                      color: colorScheme.onSurfaceVariant,
                    ),
                  ),
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

class _HomeCountLabel extends StatelessWidget {
  const _HomeCountLabel({super.key, required this.count});

  final int count;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final colorScheme = theme.colorScheme;
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: AppSpacing.xs),
      child: Text(
        '$count',
        style: theme.textTheme.labelMedium?.copyWith(
          color: colorScheme.onSurfaceVariant,
          fontWeight: FontWeight.w500,
        ),
      ),
    );
  }
}

String _homeSectionTitle(AppLocalizations l10n, _HomeSectionKind section) {
  return switch (section) {
    _HomeSectionKind.today => l10n.todayTitle,
  };
}

_HomeDueTiming _homeDueTiming(
  TaskDueDto due,
  ({int todayStartMs, int tomorrowStartMs, int dayAfterTomorrowStartMs}) ranges,
) {
  if (taskDueIsOverdue(due)) {
    return _HomeDueTiming.overdue;
  }
  final localDate = taskDueLocalDate(due);
  final localMs = DateTime(
    localDate.year,
    localDate.month,
    localDate.day,
  ).millisecondsSinceEpoch;
  if (localMs < ranges.tomorrowStartMs) {
    return _HomeDueTiming.today;
  }
  return _HomeDueTiming.future;
}

_HomeSectionKind? _homeSectionForTask(
  TaskDto task,
  ({int todayStartMs, int tomorrowStartMs, int dayAfterTomorrowStartMs}) ranges,
) {
  final due = task.due;
  if (due != null && taskDueIsOverdue(due)) {
    return _HomeSectionKind.today;
  }
  final scheduledAt = task.scheduledAt;
  if (scheduledAt != null &&
      scheduledAt >= ranges.todayStartMs &&
      scheduledAt < ranges.tomorrowStartMs) {
    return _HomeSectionKind.today;
  }
  if (due == null) {
    return null;
  }
  return _homeDueTiming(due, ranges) == _HomeDueTiming.today
      ? _HomeSectionKind.today
      : null;
}

int _compareHomeEntries(HomeTaskDto a, HomeTaskDto b, TaskSortMode sortMode) {
  final dueComparison = compareTaskDue(a.task.due, b.task.due);
  if (dueComparison != 0) {
    return dueComparison;
  }
  return compareTasksForSortMode(a.task, b.task, sortMode);
}

TaskDto _taskSnapshotWithStatus(TaskDto task, String status) {
  final isClosed = status == 'done' || status == 'wont_do';
  return TaskDto(
    id: task.id,
    listId: task.listId,
    parentTaskId: task.parentTaskId,
    title: task.title,
    note: task.note,
    status: status,
    priority: task.priority,
    due: task.due,
    scheduledAt: task.scheduledAt,
    estimatedMinutes: task.estimatedMinutes,
    sortOrder: task.sortOrder,
    completedAt: isClosed
        ? task.completedAt ?? DateTime.now().millisecondsSinceEpoch
        : null,
    closedReason: status == 'wont_do' ? task.closedReason : null,
    deletedAt: task.deletedAt,
    assignee: task.assignee,
    createdAt: task.createdAt,
    updatedAt: task.updatedAt,
  );
}

bool _taskTreeContains(TaskTreeNode root, String taskId) {
  if (root.task.id == taskId) {
    return true;
  }
  return root.children.any((child) => _taskTreeContains(child, taskId));
}
