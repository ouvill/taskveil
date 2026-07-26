import 'dart:async';
import 'package:flutter/material.dart';
import 'package:flutter/semantics.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_slidable/flutter_slidable.dart';
import 'package:go_router/go_router.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';
import 'package:taskveil/src/core/providers.dart';
import 'package:taskveil/src/core/task_tree.dart';
import 'package:taskveil/src/core/task_due.dart';
import 'package:taskveil/src/generated/l10n/app_localizations.dart';
import 'package:taskveil/src/rust/api.dart';
import 'package:taskveil/src/ui/dialogs.dart';
import 'package:taskveil/src/ui/bridge_error_messages.dart';
import 'package:taskveil/src/ui/header_actions.dart';
import 'package:taskveil/src/ui/states.dart';
import 'package:taskveil/src/ui/task_components.dart';
import 'package:taskveil/src/ui/task_completion_motion.dart';
import 'package:taskveil/src/ui/theme.dart';

part 'tasks_screen/tasks_body.dart';
part 'tasks_screen/task_row_shells.dart';
part 'tasks_screen/home_sections.dart';
part 'tasks_screen/task_actions.dart';
part 'tasks_screen/task_reorder.dart';

/// The task list screen for a single list (route
/// `/lists/:listId/tasks`).
///
/// F-02 "シンプルUI" skeleton: shows active tasks with a checkbox to mark
/// them done and a FAB to create a new one. Tapping a task navigates to its
/// detail screen.
class TasksScreen extends ConsumerWidget {
  const TasksScreen({
    super.key,
    required this.listId,
    this.listName,
    this.isHome = false,
  }) : isTodaySmartView = false;

  const TasksScreen.today({super.key})
    : listId = '_today',
      listName = null,
      isHome = true,
      isTodaySmartView = true;

  final String listId;
  final String? listName;
  final bool isHome;
  final bool isTodaySmartView;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final l10n = AppLocalizations.of(context)!;
    final AsyncValue<List<TaskDto>> tasksAsync;
    final Map<String, String> homeListNameByTaskId;
    final List<HomeTaskDto> homeTaskEntries;
    if (isTodaySmartView) {
      final homeTasksAsync = ref.watch(homeTasksProvider);
      homeTaskEntries = homeTasksAsync.value ?? const <HomeTaskDto>[];
      homeListNameByTaskId = {
        for (final homeTask in homeTaskEntries)
          homeTask.task.id: homeTask.listName,
      };
      tasksAsync = homeTasksAsync.whenData(
        (homeTasks) =>
            homeTasks.map((homeTask) => homeTask.task).toList(growable: false),
      );
    } else {
      homeTaskEntries = const <HomeTaskDto>[];
      homeListNameByTaskId = const {};
      tasksAsync = ref.watch(tasksProvider(listId));
    }
    final listsAsync = ref.watch(listsProvider);
    final archivedListsAsync = ref.watch(archivedListsProvider);
    final sortMode = ref.watch(taskSortModeProvider(listId));
    final effectiveSortMode =
        isTodaySmartView && sortMode == TaskSortMode.manual
        ? TaskSortMode.dueDate
        : sortMode;
    final activeLists = listsAsync.value;
    final archivedLists = archivedListsAsync.value;
    final currentList =
        _findList(listId, activeLists) ?? _findList(listId, archivedLists);
    final isDefaultInbox =
        currentList?.archivedAt == null && currentList?.isDefault == true;

    final sortMenu = _TaskSortMenu(
      selectedMode: effectiveSortMode,
      availableModes: isTodaySmartView
          ? const [
              TaskSortMode.dueDate,
              TaskSortMode.priority,
              TaskSortMode.createdAt,
            ]
          : TaskSortMode.values,
      onSelected: (mode) {
        ref.read(taskSortModeProvider(listId).notifier).setMode(mode);
      },
    );
    final listActionsMenu = isTodaySmartView || currentList == null
        ? null
        : _ListActionsMenu(
            list: currentList,
            isDefaultInbox: isDefaultInbox,
            onRename: () => _renameList(context, ref, currentList),
            onArchive: () => _archiveList(ref, currentList),
            onUnarchive: () => _unarchiveList(ref, currentList),
            onDelete: () => _deleteList(context, ref, currentList),
          );

    return Scaffold(
      appBar: isHome
          ? null
          : AppBar(
              title: Text(l10n.tasksTitle),
              actions: [
                const AppHeaderSearchAction(),
                ?listActionsMenu,
                sortMenu,
                const SizedBox(width: AppSpacing.sm),
              ],
            ),
      body: tasksAsync.when(
        loading: () => const AppLoadingState(),
        error: (error, stackTrace) => AppErrorState(
          message: l10n.failedToLoadTasks(bridgeErrorMessage(l10n, error)),
        ),
        data: (tasks) {
          return _TasksBody(
            listId: listId,
            listName: listName,
            isHome: isHome,
            isTodaySmartView: isTodaySmartView,
            tasks: tasks,
            sortMode: effectiveSortMode,
            sortMenu: sortMenu,
            listActionsMenu: listActionsMenu,
            homeListNameByTaskId: homeListNameByTaskId,
            homeTaskEntries: homeTaskEntries,
            onCompleteTask: (task, {descendantsConfirmed = false}) =>
                _completeTask(
                  context,
                  ref,
                  task,
                  tasks,
                  descendantsConfirmed: descendantsConfirmed,
                ),
            onReopenTask: (task) => _reopenTask(ref, task),
            onMoveTask: ({required task, previousTaskId, nextTaskId}) {
              return ref
                  .read(tasksProvider(listId).notifier)
                  .reorderTask(
                    taskId: task.id,
                    previousTaskId: previousTaskId,
                    nextTaskId: nextTaskId,
                  );
            },
          );
        },
      ),
    );
  }

  Future<bool> _completeTask(
    BuildContext context,
    WidgetRef ref,
    TaskDto task,
    List<TaskDto> tasks, {
    bool descendantsConfirmed = false,
  }) async {
    final descendantScope = isTodaySmartView
        ? await ref.read(tasksProvider(task.listId).future)
        : tasks;
    if (!context.mounted) {
      return false;
    }
    if (!descendantsConfirmed &&
        hasIncompleteDescendants(task.id, descendantScope)) {
      final l10n = AppLocalizations.of(context)!;
      final confirmed = await showAppConfirmDialog(
        context: context,
        title: l10n.completeTaskDialogTitle,
        message: l10n.completeTaskDialogMessage,
        cancelLabel: l10n.cancelButton,
        confirmLabel: l10n.continueButton,
      );
      if (!confirmed) {
        return false;
      }
    }

    if (isTodaySmartView) {
      await ref.read(homeTasksProvider.notifier).setStatus(task.id, 'done');
    } else {
      await ref.read(tasksProvider(listId).notifier).setStatus(task.id, 'done');
    }
    if (!context.mounted) {
      return true;
    }
    await _showLatestUndoSnackBar(context);
    return true;
  }

  Future<void> _reopenTask(WidgetRef ref, TaskDto task) {
    if (isTodaySmartView) {
      return ref.read(homeTasksProvider.notifier).setStatus(task.id, 'todo');
    }
    return ref.read(tasksProvider(listId).notifier).setStatus(task.id, 'todo');
  }

  Future<void> _renameList(
    BuildContext context,
    WidgetRef ref,
    ListDto list,
  ) async {
    final l10n = AppLocalizations.of(context)!;
    final name = await showAppTextInputDialog(
      context: context,
      title: l10n.renameListTitle,
      label: l10n.nameLabel,
      cancelLabel: l10n.cancelButton,
      submitLabel: l10n.saveButton,
      initialValue: list.name,
    );
    final trimmedName = name?.trim();
    if (trimmedName == null ||
        trimmedName.isEmpty ||
        trimmedName == list.name) {
      return;
    }
    await ref.read(listsProvider.notifier).renameList(list.id, trimmedName);
  }

  Future<void> _archiveList(WidgetRef ref, ListDto list) {
    return ref.read(listsProvider.notifier).archiveList(list.id);
  }

  Future<void> _unarchiveList(WidgetRef ref, ListDto list) {
    return ref.read(archivedListsProvider.notifier).unarchiveList(list.id);
  }

  Future<void> _deleteList(
    BuildContext context,
    WidgetRef ref,
    ListDto list,
  ) async {
    final l10n = AppLocalizations.of(context)!;
    final taskCount = await ref
        .read(listsProvider.notifier)
        .countTasks(list.id);
    if (!context.mounted) {
      return;
    }
    final confirmed = await showAppConfirmDialog(
      context: context,
      title: l10n.deleteListDialogTitle(list.name),
      message: l10n.deleteListDialogMessage(taskCount),
      cancelLabel: l10n.cancelButton,
      confirmLabel: l10n.deleteButton,
      isDestructive: true,
    );
    if (!confirmed) {
      return;
    }
    await ref.read(listsProvider.notifier).deleteList(list.id);
    if (!context.mounted) {
      return;
    }
    context.go('/lists');
  }
}
