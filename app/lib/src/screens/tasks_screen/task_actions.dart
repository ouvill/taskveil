part of '../tasks_screen.dart';

class _CompletedSectionHeader extends StatelessWidget {
  const _CompletedSectionHeader({
    required this.count,
    required this.isExpanded,
    required this.onTap,
    this.title,
    this.showTooltip,
    this.hideTooltip,
  });

  final int count;
  final bool isExpanded;
  final VoidCallback onTap;
  final String? title;
  final String? showTooltip;
  final String? hideTooltip;

  @override
  Widget build(BuildContext context) {
    final l10n = AppLocalizations.of(context)!;
    final theme = Theme.of(context);
    final colorScheme = theme.colorScheme;
    final tooltip = isExpanded
        ? hideTooltip ?? l10n.hideCompletedTasksTooltip
        : showTooltip ?? l10n.showCompletedTasksTooltip;
    return Tooltip(
      message: tooltip,
      child: Semantics(
        button: true,
        label: tooltip,
        child: Material(
          color: Colors.transparent,
          child: InkWell(
            key: const ValueKey('completed-section-toggle'),
            borderRadius: BorderRadius.circular(14),
            onTap: onTap,
            child: Padding(
              padding: const EdgeInsets.symmetric(
                horizontal: AppSpacing.xs,
                vertical: AppSpacing.xs,
              ),
              child: Row(
                children: [
                  Expanded(
                    child: Text(
                      title ?? l10n.completedTasksTitle,
                      style: theme.textTheme.labelMedium?.copyWith(
                        color: colorScheme.onSurfaceVariant,
                        fontWeight: FontWeight.w600,
                        letterSpacing: 0.35,
                      ),
                    ),
                  ),
                  _HomeCountLabel(
                    key: const ValueKey('completed-section-count'),
                    count: count,
                  ),
                  const SizedBox(width: AppSpacing.xs),
                  SizedBox(
                    width: 48,
                    height: 48,
                    child: Center(
                      child: Icon(
                        isExpanded
                            ? LucideIcons.chevronUp300
                            : LucideIcons.chevronDown300,
                        color: colorScheme.onSurfaceVariant,
                      ),
                    ),
                  ),
                ],
              ),
            ),
          ),
        ),
      ),
    );
  }
}

class _ListActionsMenu extends StatelessWidget {
  const _ListActionsMenu({
    required this.list,
    required this.isDefaultInbox,
    required this.onRename,
    required this.onArchive,
    required this.onUnarchive,
    required this.onDelete,
  });

  final ListDto list;
  final bool isDefaultInbox;
  final Future<void> Function() onRename;
  final Future<void> Function() onArchive;
  final Future<void> Function() onUnarchive;
  final Future<void> Function() onDelete;

  @override
  Widget build(BuildContext context) {
    final l10n = AppLocalizations.of(context)!;
    final isArchived = list.archivedAt != null;
    return PopupMenuButton<_ListAction>(
      key: const ValueKey('list-actions-menu'),
      icon: const Icon(LucideIcons.moreHorizontal300),
      tooltip: l10n.listActionsTooltip,
      onSelected: (action) {
        switch (action) {
          case _ListAction.rename:
            unawaited(onRename());
            break;
          case _ListAction.archive:
            unawaited(onArchive());
            break;
          case _ListAction.unarchive:
            unawaited(onUnarchive());
            break;
          case _ListAction.delete:
            unawaited(onDelete());
            break;
        }
      },
      itemBuilder: (context) => [
        PopupMenuItem(
          value: _ListAction.rename,
          child: Text(l10n.renameListMenuItem),
        ),
        if (!isDefaultInbox && !isArchived)
          PopupMenuItem(
            value: _ListAction.archive,
            child: Text(l10n.archiveListMenuItem),
          ),
        if (!isDefaultInbox && isArchived)
          PopupMenuItem(
            value: _ListAction.unarchive,
            child: Text(l10n.unarchiveListMenuItem),
          ),
        if (!isDefaultInbox)
          PopupMenuItem(
            value: _ListAction.delete,
            child: Text(l10n.deleteListMenuItem),
          ),
      ],
    );
  }
}

enum _ListAction { rename, archive, unarchive, delete }

ListDto? _findList(String listId, List<ListDto>? lists) {
  if (lists == null) {
    return null;
  }
  for (final list in lists) {
    if (list.id == listId) {
      return list;
    }
  }
  return null;
}

bool _hasClosedRoot(List<TaskDto> tasks) {
  return buildTaskTree(tasks).any((node) => isTaskClosed(node.task));
}

class _TaskSortMenu extends StatelessWidget {
  const _TaskSortMenu({
    required this.selectedMode,
    required this.availableModes,
    required this.onSelected,
  });

  final TaskSortMode selectedMode;
  final List<TaskSortMode> availableModes;
  final ValueChanged<TaskSortMode> onSelected;

  @override
  Widget build(BuildContext context) {
    final l10n = AppLocalizations.of(context)!;
    return PopupMenuButton<TaskSortMode>(
      key: const ValueKey('task-sort-menu'),
      icon: const Icon(LucideIcons.arrowDownUp300),
      tooltip: l10n.taskSortTooltip,
      initialValue: selectedMode,
      onSelected: onSelected,
      itemBuilder: (context) {
        return [
          for (final mode in availableModes)
            PopupMenuItem<TaskSortMode>(
              value: mode,
              child: ConstrainedBox(
                constraints: const BoxConstraints(minWidth: 168),
                child: Row(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    Icon(
                      selectedMode == mode
                          ? LucideIcons.circleCheck300
                          : LucideIcons.arrowDownUp300,
                      size: 18,
                    ),
                    const SizedBox(width: AppSpacing.sm),
                    Flexible(
                      child: Text(_taskSortLabel(l10n, mode), softWrap: true),
                    ),
                  ],
                ),
              ),
            ),
        ];
      },
    );
  }
}

String _taskSortLabel(AppLocalizations l10n, TaskSortMode mode) {
  return switch (mode) {
    TaskSortMode.manual => l10n.taskSortManual,
    TaskSortMode.dueDate => l10n.taskSortDueDate,
    TaskSortMode.priority => l10n.taskSortPriority,
    TaskSortMode.createdAt => l10n.taskSortCreatedAt,
  };
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
        content: Text(l10n.undoFailedMessage(error.toString())),
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

String _taskRowSemanticLabel({
  required AppLocalizations l10n,
  required String title,
  required String status,
  required String priority,
  required String? dueLabel,
  required String? listName,
  required String? parentTaskName,
  required int depth,
}) {
  final parts = <String>[
    title,
    l10n.taskRowStatusSemantics(status),
    l10n.taskPriority(priority),
    if (dueLabel != null) l10n.taskRowDueSemantics(dueLabel),
    if (parentTaskName != null) l10n.parentTaskLinkSemantics(parentTaskName),
    if (listName != null && listName.isNotEmpty)
      l10n.taskRowListSemantics(listName),
    if (depth > 0) l10n.taskRowSubtaskLevelSemantics(depth + 1),
    l10n.taskRowOpenHint,
  ];
  return parts.join('. ');
}
