part of '../tasks_screen.dart';

class _TasksBody extends StatefulWidget {
  const _TasksBody({
    required this.listId,
    required this.listName,
    required this.isHome,
    required this.isTodaySmartView,
    required this.tasks,
    required this.sortMode,
    required this.sortMenu,
    required this.listActionsMenu,
    required this.homeListNameByTaskId,
    required this.homeTaskEntries,
    required this.onCompleteTask,
    required this.onReopenTask,
    required this.onMoveTask,
  });

  final String listId;
  final String? listName;
  final bool isHome;
  final bool isTodaySmartView;
  final List<TaskDto> tasks;
  final TaskSortMode sortMode;
  final Widget sortMenu;
  final Widget? listActionsMenu;
  final Map<String, String> homeListNameByTaskId;
  final List<HomeTaskDto> homeTaskEntries;
  final Future<bool> Function(TaskDto task, {bool descendantsConfirmed})
  onCompleteTask;
  final Future<void> Function(TaskDto task) onReopenTask;
  final Future<void> Function({
    required TaskDto task,
    required String? previousTaskId,
    required String? nextTaskId,
  })
  onMoveTask;

  @override
  State<_TasksBody> createState() => _TasksBodyState();
}

class _TasksBodyState extends State<_TasksBody> {
  bool _showCompleted = false;
  bool _isTodayCollapsed = false;
  final Map<String, _PendingHomeCompletion> _pendingHomeCompletions = {};
  final Map<String, _PendingListCompletion> _pendingListCompletions = {};
  final Map<String, Future<bool>> _homeCompletionOperations = {};
  final Map<String, Future<bool>> _listCompletionOperations = {};
  final Set<String> _optimisticHomeCompletionIds = {};
  late final TaskCompletionRetentionController<String>
  _completionRetentionController;
  _TaskDropIndicator? _dropIndicator;

  @override
  void initState() {
    super.initState();
    _completionRetentionController = TaskCompletionRetentionController<String>()
      ..addListener(_handleCompletionRetentionChanged);
  }

  @override
  void didUpdateWidget(covariant _TasksBody oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (_showCompleted && !_hasClosedRoot(widget.tasks)) {
      _showCompleted = false;
    }
    _syncPendingHomeCompletionsWithWidget();
    _syncPendingListCompletionsWithWidget();
    _syncOptimisticHomeCompletionsWithWidget();
  }

  @override
  void dispose() {
    _completionRetentionController
      ..removeListener(_handleCompletionRetentionChanged)
      ..dispose();
    super.dispose();
  }

  void _handleCompletionRetentionChanged() {
    if (!mounted) {
      return;
    }
    final retainedKeys = _completionRetentionController.keys.toSet();
    setState(() {
      _pendingHomeCompletions.removeWhere(
        (taskId, _) => !retainedKeys.contains(taskId),
      );
      _pendingListCompletions.removeWhere(
        (taskId, _) => !retainedKeys.contains(taskId),
      );
    });
  }

  @override
  Widget build(BuildContext context) {
    final l10n = AppLocalizations.of(context)!;
    if (widget.isHome) {
      final closedRows = _buildHomeClosedRowData();
      final homeSections = _buildHomeSections();
      final hasVisibleHomeTasks = homeSections.any(
        (section) => section.rows.isNotEmpty || section.count > 0,
      );
      final visibleHomeSections = hasVisibleHomeTasks || closedRows.isNotEmpty
          ? homeSections
          : const <_HomeSectionData>[];
      return SafeArea(
        top: true,
        child: Align(
          alignment: Alignment.topCenter,
          child: ConstrainedBox(
            constraints: const BoxConstraints(maxWidth: 920),
            child: CustomScrollView(
              slivers: [
                SliverPadding(
                  padding: const EdgeInsets.fromLTRB(
                    AppSpacing.md,
                    12,
                    AppSpacing.md,
                    AppSpacing.xl * 3,
                  ),
                  sliver: SliverMainAxisGroup(
                    slivers: [
                      SliverToBoxAdapter(
                        child: _HomeTasksHeader(
                          sortMenu: widget.sortMenu,
                          listActionsMenu: widget.listActionsMenu,
                        ),
                      ),
                      const SliverToBoxAdapter(
                        child: SizedBox(height: AppSpacing.md),
                      ),
                      if (visibleHomeSections.isEmpty)
                        SliverToBoxAdapter(child: _HomeClearState(l10n: l10n))
                      else
                        _HomeSectionsPanelSliver(
                          sections: visibleHomeSections,
                          isTodayCollapsed: _isTodayCollapsed,
                          onToggleSection: (section) {
                            setState(
                              () => _isTodayCollapsed = !_isTodayCollapsed,
                            );
                          },
                          rowBuilder: (context, row, section) =>
                              _buildHomeTaskRow(
                                context,
                                row.node,
                                section,
                                rootListId: row.rootListId,
                                parentTaskName: row.parentTaskName,
                                countsInSection: row.countsInSection,
                                pendingCompletionKey: row.pendingCompletionKey,
                                disableInteractions:
                                    row.disableInteractions ||
                                    _isCompletionExiting(
                                      row.pendingCompletionKey,
                                    ),
                                isPendingRoot: row.isPendingRoot,
                              ),
                        ),
                      if (closedRows.isNotEmpty) ...[
                        const SliverToBoxAdapter(
                          child: SizedBox(height: AppSpacing.lg),
                        ),
                        SliverToBoxAdapter(
                          child: _CompletedSectionHeader(
                            count: closedRows.length,
                            isExpanded: _showCompleted,
                            title: l10n.calendarCompletedTitle,
                            showTooltip: l10n.calendarShowCompletedTooltip,
                            hideTooltip: l10n.calendarHideCompletedTooltip,
                            onTap: () => setState(
                              () => _showCompleted = !_showCompleted,
                            ),
                          ),
                        ),
                        if (_showCompleted)
                          SliverList.builder(
                            itemCount: closedRows.length * 2,
                            itemBuilder: (context, index) {
                              if (index.isEven) {
                                return const SizedBox(height: AppSpacing.sm);
                              }
                              final row = closedRows[index ~/ 2];
                              return _buildHomeTaskRow(
                                context,
                                row.node,
                                _HomeSectionKind.today,
                                rootListId: row.rootListId,
                                parentTaskName: row.parentTaskName,
                              );
                            },
                          ),
                      ],
                    ],
                  ),
                ),
              ],
            ),
          ),
        ),
      );
    }

    final roots = buildTaskTree(widget.tasks, sortMode: widget.sortMode);
    final activeRoots = <TaskTreeNode>[];
    for (final root in roots) {
      final pending = _pendingListCompletions[root.task.id];
      if (pending != null) {
        activeRoots.add(pending.root);
      } else if (!isTaskClosed(root.task)) {
        activeRoots.add(root);
      }
    }
    final completedRoots = roots
        .where(
          (node) =>
              isTaskClosed(node.task) &&
              !_pendingListCompletions.containsKey(node.task.id),
        )
        .toList(growable: false);
    final activeNodes = flattenTaskTree(activeRoots);
    final completedNodes = flattenTaskTree(completedRoots);
    final activeReorderTasks = activeNodes
        .map((node) => node.task)
        .where((task) => !isTaskClosed(task))
        .toList(growable: false);
    final activeRowTasks = activeNodes
        .map((node) => node.task)
        .toList(growable: false);
    if (activeNodes.isEmpty && completedNodes.isEmpty) {
      return AppEmptyState(
        icon: LucideIcons.listChecks300,
        title: l10n.tasksEmptyTitle,
        body: l10n.tasksEmptyBody,
      );
    }

    return SafeArea(
      top: false,
      child: CustomScrollView(
        slivers: [
          SliverPadding(
            padding: const EdgeInsets.fromLTRB(
              AppSpacing.md,
              AppSpacing.md,
              AppSpacing.md,
              AppSpacing.xl * 3,
            ),
            sliver: SliverMainAxisGroup(
              slivers: [
                if (activeNodes.isNotEmpty)
                  _TaskRowsSliver(
                    nodes: activeNodes,
                    separatorHeight: AppSpacing.sm,
                    rowBuilder: (context, node) => _buildTaskRow(
                      context,
                      node,
                      activeReorderTasks,
                      isCompletedSection: false,
                      reorderShellScope: activeRowTasks,
                      pendingCompletionKey: _pendingListCompletionKeyForTask(
                        node.task.id,
                      ),
                    ),
                  ),
                if (completedNodes.isNotEmpty) ...[
                  SliverToBoxAdapter(
                    child: SizedBox(
                      height: activeNodes.isEmpty
                          ? AppSpacing.sm
                          : AppSpacing.lg,
                    ),
                  ),
                  SliverToBoxAdapter(
                    child: _CompletedSectionHeader(
                      count: completedRoots.length,
                      isExpanded: _showCompleted,
                      onTap: () =>
                          setState(() => _showCompleted = !_showCompleted),
                    ),
                  ),
                  if (_showCompleted)
                    SliverList.builder(
                      itemCount: completedNodes.length * 2,
                      itemBuilder: (context, index) {
                        if (index.isEven) {
                          return const SizedBox(height: AppSpacing.sm);
                        }
                        return _buildTaskRow(
                          context,
                          completedNodes[index ~/ 2],
                          const <TaskDto>[],
                          isCompletedSection: true,
                        );
                      },
                    ),
                ],
              ],
            ),
          ),
        ],
      ),
    );
  }

  List<_HomeSectionData> _buildHomeSections() {
    final ranges = homeLocalRangesMs();
    final pendingRowsByTaskId = _pendingHomeCompletionRowsByTaskId();
    final pendingRootIds = _pendingHomeCompletions.keys.toSet();
    final sortedEntries =
        widget.homeTaskEntries
            .map((entry) {
              final pendingRow = pendingRowsByTaskId[entry.task.id];
              if (pendingRow == null) {
                return entry;
              }
              return HomeTaskDto(
                task: pendingRow.node.task,
                listName: entry.listName,
                isHomeTarget:
                    entry.isHomeTarget ||
                    pendingRootIds.contains(entry.task.id),
              );
            })
            .toList(growable: false)
          ..sort((a, b) => _compareHomeEntries(a, b, widget.sortMode));
    final pendingIds = _pendingHomeCompletionTaskIds();
    final bySection = {
      for (final section in _HomeSectionKind.values)
        section: <_HomeSectionRowData>[],
    };
    final countBySection = {
      for (final section in _HomeSectionKind.values) section: 0,
    };
    final taskById = {
      for (final entry in sortedEntries) entry.task.id: entry.task,
    };
    final targetSectionByTaskId = <String, _HomeSectionKind>{};
    for (final entry in sortedEntries.where((entry) => entry.isHomeTarget)) {
      final pending = _pendingHomeCompletions[entry.task.id];
      if (pending != null) {
        targetSectionByTaskId[entry.task.id] = pending.section;
        countBySection[pending.section] = countBySection[pending.section]! + 1;
        continue;
      }
      if (pendingIds.contains(entry.task.id)) {
        continue;
      }
      if (isTaskClosed(entry.task)) {
        continue;
      }
      final section = _homeSectionForTask(entry.task, ranges);
      if (section == null) {
        continue;
      }
      targetSectionByTaskId[entry.task.id] = section;
      countBySection[section] = countBySection[section]! + 1;
    }
    final standaloneTaskIds = targetSectionByTaskId.keys.toSet();
    final childrenByParent = <String, List<TaskDto>>{};
    for (final entry in sortedEntries) {
      final parentId = entry.task.parentTaskId;
      if (parentId == null) {
        continue;
      }
      childrenByParent.putIfAbsent(parentId, () => <TaskDto>[]).add(entry.task);
    }
    for (final children in childrenByParent.values) {
      children.sort((a, b) => compareTasksForSortMode(a, b, widget.sortMode));
    }

    TaskTreeNode buildHomeNode(TaskDto task, int depth, Set<String> path) {
      if (path.contains(task.id)) {
        return TaskTreeNode(task: task, depth: depth, children: const []);
      }
      final nextPath = {...path, task.id};
      return TaskTreeNode(
        task: task,
        depth: depth,
        children: [
          for (final child in childrenByParent[task.id] ?? const <TaskDto>[])
            if (!standaloneTaskIds.contains(child.id))
              buildHomeNode(child, depth + 1, nextPath),
        ],
      );
    }

    for (final entry in sortedEntries.where((entry) => entry.isHomeTarget)) {
      final task = entry.task;
      if (pendingIds.contains(task.id) && !pendingRootIds.contains(task.id)) {
        continue;
      }
      final section = targetSectionByTaskId[task.id];
      if (section == null) {
        continue;
      }
      final roots = [buildHomeNode(task, 0, const <String>{})];
      bySection[section]!.addAll(
        flattenTaskTree(roots).map(
          (node) => _HomeSectionRowData(
            node: node,
            rootListId: task.listId,
            parentTaskName:
                pendingRowsByTaskId[node.task.id]?.parentTaskName ??
                (node.depth == 0
                    ? taskById[node.task.parentTaskId]?.title
                    : null),
            countsInSection: node.depth == 0,
            pendingCompletionKey:
                pendingRowsByTaskId[node.task.id]?.pendingCompletionKey,
            disableInteractions:
                pendingRowsByTaskId[node.task.id]?.disableInteractions ?? false,
            isPendingRoot: pendingRootIds.contains(node.task.id),
          ),
        ),
      );
    }
    return [
      for (final section in _HomeSectionKind.values)
        _HomeSectionData(
          kind: section,
          count: countBySection[section]!,
          rows: bySection[section]!,
        ),
    ];
  }

  List<_HomeSectionRowData> _buildHomeClosedRowData() {
    final pendingIds = _pendingHomeCompletionTaskIds();
    final closedRoots =
        widget.homeTaskEntries
            .map((entry) => entry.task)
            .where((task) => task.parentTaskId == null && isTaskClosed(task))
            .where((task) => !pendingIds.contains(task.id))
            .toList(growable: false)
          ..sort((a, b) => compareTasksForSortMode(a, b, widget.sortMode));
    return [
      for (final task in closedRoots)
        _HomeSectionRowData(
          node: FlattenedTaskTreeNode(
            node: TaskTreeNode(task: task, depth: 0, children: const []),
            isLastSibling: task == closedRoots.last,
            ancestorLineContinuations: const <bool>[],
          ),
          rootListId: task.listId,
          parentTaskName: null,
        ),
    ];
  }

  Widget _buildHomeTaskRow(
    BuildContext context,
    FlattenedTaskTreeNode node,
    _HomeSectionKind section, {
    required String rootListId,
    required String? parentTaskName,
    bool countsInSection = false,
    String? pendingCompletionKey,
    bool disableInteractions = false,
    bool isPendingRoot = false,
  }) {
    final l10n = AppLocalizations.of(context)!;
    final sourceTask = node.task;
    final task =
        _optimisticHomeCompletionIds.contains(sourceTask.id) &&
            !isTaskClosed(sourceTask)
        ? _taskSnapshotWithStatus(sourceTask, 'done')
        : sourceTask;
    final locale = Localizations.localeOf(context).toLanguageTag();
    final dueLabel = task.due == null
        ? null
        : formatRelativeDueDate(l10n, locale, task.due);
    final row = _TaskEntryMotion(
      slide: false,
      child: AppHomeTaskRow(
        key: ValueKey('task-row-${task.id}'),
        checkboxKey: ValueKey('task-done-${task.id}'),
        title: task.title,
        isDone: isTaskClosed(task),
        depth: node.depth,
        hierarchyGuideKey: ValueKey('task-hierarchy-guide-${task.id}'),
        hierarchyGuideHorizontalKey: ValueKey(
          'task-hierarchy-horizontal-${task.id}',
        ),
        isLastSibling: node.isLastSibling,
        ancestorLineContinuations: node.ancestorLineContinuations,
        parentTaskName: parentTaskName,
        parentTaskSemanticLabel: parentTaskName == null
            ? null
            : l10n.parentTaskLinkSemantics(parentTaskName),
        listName: node.depth > 0 && task.listId == rootListId
            ? ''
            : widget.homeListNameByTaskId[task.id] ?? '',
        dueLabel: dueLabel,
        dueTone: task.due == null
            ? HomeDueDateTone.today
            : switch (_homeDueTiming(task.due!, homeLocalRangesMs())) {
                _HomeDueTiming.overdue => HomeDueDateTone.overdue,
                _HomeDueTiming.today => HomeDueDateTone.today,
                _HomeDueTiming.future => HomeDueDateTone.future,
              },
        dueSemanticLabel:
            task.due != null &&
                _homeDueTiming(task.due!, homeLocalRangesMs()) ==
                    _HomeDueTiming.overdue &&
                dueLabel != null
            ? l10n.taskDueOverdue(dueLabel)
            : null,
        priority: task.priority,
        priorityDotKey: ValueKey('task-priority-dot-${task.id}'),
        prioritySemanticLabel: l10n.taskPriority(
          taskPriorityLabel(l10n, task.priority),
        ),
        semanticLabel: _taskRowSemanticLabel(
          l10n: l10n,
          title: task.title,
          status: taskStatusLabel(l10n, task.status),
          priority: taskPriorityLabel(l10n, task.priority),
          dueLabel: dueLabel,
          listName: node.depth > 0 && task.listId == rootListId
              ? null
              : widget.homeListNameByTaskId[task.id],
          parentTaskName: parentTaskName,
          depth: node.depth,
        ),
        toggleDoneTooltip: isTaskClosed(task)
            ? l10n.reopenTaskTooltip
            : l10n.completeTaskTooltip,
        onToggleDone: disableInteractions
            ? null
            : isTaskClosed(task)
            ? () => _handleHomeReopenTask(task)
            : () => _handleHomeCompleteTask(
                context,
                node,
                section,
                rootListId: rootListId,
                parentTaskName: parentTaskName,
                countsInSection: countsInSection,
              ),
        onTap: () => context.push('/lists/${task.listId}/tasks/${task.id}'),
      ),
    );
    final isExiting = _isCompletionExiting(pendingCompletionKey);
    final effectiveRow = !isExiting
        ? row
        : AppTaskCompletionExit(
            key: isPendingRoot
                ? const ValueKey('home-pending-completion-exit')
                : ValueKey('home-pending-completion-exit-${task.id}'),
            isExiting: true,
            child: row,
          );
    final swipeRow = _TaskSwipeActions(
      key: ValueKey('task-swipe-actions-${task.id}'),
      task: task,
      isClosed: isTaskClosed(task),
      onLeadingAction: disableInteractions
          ? () async {}
          : isTaskClosed(task)
          ? () => _handleHomeReopenTask(task)
          : () => _handleHomeCompleteTask(
              context,
              node,
              section,
              rootListId: rootListId,
              parentTaskName: parentTaskName,
              countsInSection: countsInSection,
            ),
      child: effectiveRow,
    );
    return IgnorePointer(
      key: ValueKey('task-home-row-shell-${task.id}'),
      ignoring: disableInteractions,
      child: swipeRow,
    );
  }

  Future<void> _handleHomeCompleteTask(
    BuildContext context,
    FlattenedTaskTreeNode node,
    _HomeSectionKind section, {
    required String rootListId,
    required String? parentTaskName,
    required bool countsInSection,
  }) async {
    final task = node.task;
    if (_pendingHomeCompletionTaskIds().contains(task.id) ||
        _optimisticHomeCompletionIds.contains(task.id)) {
      return;
    }
    if (MediaQuery.disableAnimationsOf(context)) {
      await widget.onCompleteTask(task);
      return;
    }
    final needsConfirmation = hasIncompleteDescendants(task.id, widget.tasks);
    if (!countsInSection) {
      _startOptimisticHomeCompletion(task.id);
      final operation = widget.onCompleteTask(task);
      _homeCompletionOperations[task.id] = operation;
      try {
        final completed = await operation;
        _homeCompletionOperations.remove(task.id);
        if (!completed) {
          _cancelOptimisticHomeCompletion(task.id);
        }
      } catch (_) {
        _cancelOptimisticHomeCompletion(task.id);
        rethrow;
      }
      return;
    }

    if (needsConfirmation) {
      final operation = widget.onCompleteTask(task);
      _homeCompletionOperations[task.id] = operation;
      try {
        final completed = await operation;
        _homeCompletionOperations.remove(task.id);
        if (completed && mounted) {
          _startPendingHomeCompletion(
            task: task,
            node: node,
            section: section,
            rootListId: rootListId,
            parentTaskName: parentTaskName,
            countsInSection: countsInSection,
          );
        }
      } catch (_) {
        _homeCompletionOperations.remove(task.id);
        rethrow;
      }
      return;
    }

    _startPendingHomeCompletion(
      task: task,
      node: node,
      section: section,
      rootListId: rootListId,
      parentTaskName: parentTaskName,
      countsInSection: countsInSection,
    );
    final operation = widget.onCompleteTask(task);
    _homeCompletionOperations[task.id] = operation;
    try {
      final completed = await operation;
      _homeCompletionOperations.remove(task.id);
      if (!completed) {
        _cancelPendingHomeCompletion(task.id);
      }
    } catch (_) {
      _cancelPendingHomeCompletion(task.id);
      rethrow;
    }
  }

  Future<void> _handleHomeReopenTask(TaskDto task) async {
    final operation = _homeCompletionOperations[task.id];
    if (operation != null) {
      await operation;
      _homeCompletionOperations.remove(task.id);
    }
    _cancelPendingHomeCompletion(task.id);
    _cancelOptimisticHomeCompletion(task.id);
    await widget.onReopenTask(task);
  }

  void _startOptimisticHomeCompletion(String taskId) {
    setState(() {
      _optimisticHomeCompletionIds.add(taskId);
    });
  }

  void _cancelOptimisticHomeCompletion(String taskId) {
    if (!_optimisticHomeCompletionIds.contains(taskId)) {
      return;
    }
    if (!mounted) {
      _optimisticHomeCompletionIds.remove(taskId);
      return;
    }
    setState(() {
      _optimisticHomeCompletionIds.remove(taskId);
    });
  }

  void _startPendingHomeCompletion({
    required TaskDto task,
    required FlattenedTaskTreeNode node,
    required _HomeSectionKind section,
    required String rootListId,
    required String? parentTaskName,
    required bool countsInSection,
  }) {
    final completedTask = _taskSnapshotWithStatus(task, 'done');
    final completedTree = TaskTreeNode(
      task: completedTask,
      depth: node.node.depth,
      children: node.node.children,
    );
    final rows = flattenTaskTree([completedTree])
        .asMap()
        .entries
        .map(
          (entry) => _HomeSectionRowData(
            node: entry.value,
            rootListId: rootListId,
            parentTaskName: entry.key == 0 ? parentTaskName : null,
            countsInSection: entry.key == 0 && countsInSection,
            pendingCompletionKey: task.id,
            disableInteractions: entry.key != 0,
            isPendingRoot: entry.key == 0,
          ),
        )
        .toList(growable: false);
    setState(() {
      _pendingHomeCompletions[task.id] = _PendingHomeCompletion(
        rows: rows,
        section: section,
      );
    });
    _completionRetentionController.retain(task.id);
  }

  void _cancelPendingHomeCompletion(String taskId) {
    _homeCompletionOperations.remove(taskId);
    _completionRetentionController.cancel(taskId);
    if (!mounted) {
      _pendingHomeCompletions.remove(taskId);
      return;
    }
    if (_pendingHomeCompletions.containsKey(taskId)) {
      setState(() => _pendingHomeCompletions.remove(taskId));
    }
  }

  void _syncPendingHomeCompletionsWithWidget() {
    if (_pendingHomeCompletions.isEmpty) {
      return;
    }
    final taskById = {
      for (final entry in widget.homeTaskEntries) entry.task.id: entry.task,
    };
    final restoredTaskIds = <String>[];
    for (final pending in _pendingHomeCompletions.values) {
      final task = taskById[pending.rows.first.node.task.id];
      if (task != null &&
          !isTaskClosed(task) &&
          !_homeCompletionOperations.containsKey(task.id) &&
          task.updatedAt > pending.rows.first.node.task.updatedAt) {
        restoredTaskIds.add(task.id);
      }
    }
    for (final taskId in restoredTaskIds) {
      _cancelPendingHomeCompletion(taskId);
    }
  }

  void _syncOptimisticHomeCompletionsWithWidget() {
    if (_optimisticHomeCompletionIds.isEmpty) {
      return;
    }
    final taskById = {
      for (final entry in widget.homeTaskEntries) entry.task.id: entry.task,
    };
    _optimisticHomeCompletionIds.removeWhere((taskId) {
      final task = taskById[taskId];
      return task == null || isTaskClosed(task);
    });
  }

  Set<String> _pendingHomeCompletionTaskIds() {
    return {
      for (final pending in _pendingHomeCompletions.values)
        for (final row in pending.rows) row.node.task.id,
    };
  }

  bool _isCompletionExiting(String? key) {
    return key != null &&
        _completionRetentionController.phaseOf(key) ==
            TaskCompletionRetentionPhase.exiting;
  }

  Map<String, _HomeSectionRowData> _pendingHomeCompletionRowsByTaskId() {
    return {
      for (final pending in _pendingHomeCompletions.values)
        for (final row in pending.rows) row.node.task.id: row,
    };
  }

  Future<void> _handleListCompleteTask(
    BuildContext context,
    FlattenedTaskTreeNode node,
  ) async {
    final task = node.task;
    if (node.depth > 0 || MediaQuery.disableAnimationsOf(context)) {
      await widget.onCompleteTask(task);
      return;
    }
    if (_pendingListCompletions.containsKey(task.id)) {
      return;
    }
    final needsConfirmation = hasIncompleteDescendants(task.id, widget.tasks);
    if (needsConfirmation) {
      final l10n = AppLocalizations.of(context)!;
      final confirmed = await showAppConfirmDialog(
        context: context,
        title: l10n.completeTaskDialogTitle,
        message: l10n.completeTaskDialogMessage,
        cancelLabel: l10n.cancelButton,
        confirmLabel: l10n.continueButton,
      );
      if (!confirmed || !mounted) {
        return;
      }
    }

    _startPendingListCompletion(node);
    final operation = widget.onCompleteTask(
      task,
      descendantsConfirmed: needsConfirmation,
    );
    _listCompletionOperations[task.id] = operation;
    try {
      final completed = await operation;
      _listCompletionOperations.remove(task.id);
      if (!completed) {
        _cancelPendingListCompletion(task.id);
      }
    } catch (_) {
      _cancelPendingListCompletion(task.id);
    }
  }

  void _startPendingListCompletion(FlattenedTaskTreeNode node) {
    final task = node.task;
    final completedRoot = TaskTreeNode(
      task: _taskSnapshotWithStatus(task, 'done'),
      depth: node.node.depth,
      children: node.node.children,
    );
    setState(() {
      _pendingListCompletions[task.id] = _PendingListCompletion(
        root: completedRoot,
      );
    });
    _completionRetentionController.retain(task.id);
  }

  void _cancelPendingListCompletion(String taskId) {
    _listCompletionOperations.remove(taskId);
    _completionRetentionController.cancel(taskId);
    if (!mounted) {
      _pendingListCompletions.remove(taskId);
      return;
    }
    if (_pendingListCompletions.containsKey(taskId)) {
      setState(() => _pendingListCompletions.remove(taskId));
    }
  }

  void _syncPendingListCompletionsWithWidget() {
    if (_pendingListCompletions.isEmpty) {
      return;
    }
    final taskById = {for (final task in widget.tasks) task.id: task};
    final restoredTaskIds = <String>[];
    for (final entry in _pendingListCompletions.entries) {
      final task = taskById[entry.key];
      if (task != null &&
          !isTaskClosed(task) &&
          !_listCompletionOperations.containsKey(task.id) &&
          task.updatedAt > entry.value.root.task.updatedAt) {
        restoredTaskIds.add(task.id);
      }
    }
    for (final taskId in restoredTaskIds) {
      _cancelPendingListCompletion(taskId);
    }
  }

  String? _pendingListCompletionKeyForTask(String taskId) {
    for (final entry in _pendingListCompletions.entries) {
      if (entry.key == taskId || _taskTreeContains(entry.value.root, taskId)) {
        return entry.key;
      }
    }
    return null;
  }

  Widget _buildTaskRow(
    BuildContext context,
    FlattenedTaskTreeNode node,
    List<TaskDto> reorderScope, {
    required bool isCompletedSection,
    bool framed = false,
    List<TaskDto>? reorderShellScope,
    String? pendingCompletionKey,
  }) {
    final l10n = AppLocalizations.of(context)!;
    final task = node.task;
    final stats = descendantStatsOf(task.id, widget.tasks);
    final usesReorderShell =
        !widget.isHome &&
        !isCompletedSection &&
        (!isTaskClosed(task) ||
            task.status == 'done' ||
            pendingCompletionKey != null) &&
        widget.sortMode == TaskSortMode.manual;
    final shellSiblings = usesReorderShell
        ? _siblingsOf(task, reorderShellScope ?? reorderScope)
        : const <TaskDto>[];
    final shellSiblingIndex = shellSiblings.indexWhere(
      (sibling) => sibling.id == task.id,
    );
    final canDragReorder =
        usesReorderShell && !isTaskClosed(task) && pendingCompletionKey == null;
    final siblings = canDragReorder
        ? _siblingsOf(task, reorderScope)
        : const <TaskDto>[];
    final siblingIndex = siblings.indexWhere(
      (sibling) => sibling.id == task.id,
    );
    final row = _TaskEntryMotion(
      child: AppTaskRow(
        key: ValueKey('task-row-${task.id}'),
        checkboxKey: ValueKey('task-done-${task.id}'),
        title: task.title,
        isDone: isTaskClosed(task),
        depth: node.depth,
        priority: task.priority,
        priorityDotKey: ValueKey('task-priority-dot-${task.id}'),
        prioritySemanticLabel: l10n.taskPriority(
          taskPriorityLabel(l10n, task.priority),
        ),
        semanticLabel: _taskRowSemanticLabel(
          l10n: l10n,
          title: task.title,
          status: taskStatusLabel(l10n, task.status),
          priority: taskPriorityLabel(l10n, task.priority),
          dueLabel: task.due == null
              ? null
              : formatRelativeDueDate(
                  l10n,
                  Localizations.localeOf(context).toLanguageTag(),
                  task.due,
                ),
          listName: widget.isTodaySmartView
              ? widget.homeListNameByTaskId[task.id]
              : null,
          parentTaskName: null,
          depth: node.depth,
        ),
        hierarchyGuideKey: ValueKey('task-hierarchy-guide-${task.id}'),
        hierarchyGuideHorizontalKey: ValueKey(
          'task-hierarchy-horizontal-${task.id}',
        ),
        isLastSibling: node.isLastSibling,
        ancestorLineContinuations: node.ancestorLineContinuations,
        toggleDoneTooltip: isTaskClosed(task)
            ? l10n.reopenTaskTooltip
            : l10n.completeTaskTooltip,
        metadata: taskMetadataItemsFor(
          l10n: l10n,
          locale: Localizations.localeOf(context).toLanguageTag(),
          task: task,
          stats: stats,
          includeSubtaskProgress: false,
          includeWontDoStatus: !widget.isTodaySmartView,
          listName: widget.isTodaySmartView
              ? widget.homeListNameByTaskId[task.id]
              : null,
        ).take(2).toList(growable: false),
        framed: framed,
        onToggleDone: pendingCompletionKey != null
            ? null
            : isTaskClosed(task)
            ? () => widget.onReopenTask(task)
            : () => _handleListCompleteTask(context, node),
        onTap: () => context.push('/lists/${task.listId}/tasks/${task.id}'),
      ),
    );
    final swipeRow = _TaskSwipeActions(
      key: ValueKey('task-swipe-actions-${task.id}'),
      task: task,
      isClosed: isTaskClosed(task),
      onLeadingAction: pendingCompletionKey != null
          ? () async {}
          : isTaskClosed(task)
          ? () => widget.onReopenTask(task)
          : () => _handleListCompleteTask(context, node),
      child: row,
    );

    final retainedRow = IgnorePointer(
      key: ValueKey('task-list-row-shell-${task.id}'),
      ignoring: pendingCompletionKey != null,
      child: AppTaskCompletionExit(
        key: ValueKey('task-list-completion-exit-${task.id}'),
        isExiting:
            pendingCompletionKey != null &&
            _isCompletionExiting(pendingCompletionKey),
        child: swipeRow,
      ),
    );

    if (!usesReorderShell || shellSiblingIndex < 0) {
      return retainedRow;
    }

    return _TaskDragReorderTarget(
      key: ValueKey('task-drop-target-${task.id}'),
      enabled: canDragReorder && siblingIndex >= 0,
      task: task,
      siblings: siblings,
      siblingIndex: siblingIndex < 0 ? 0 : siblingIndex,
      dropIndicator: _dropIndicator,
      onHover: (indicator) => setState(() => _dropIndicator = indicator),
      onLeave: () => setState(() => _dropIndicator = null),
      onDrop:
          ({
            required draggedTask,
            required targetTask,
            required dropAfterTarget,
          }) async {
            setState(() => _dropIndicator = null);
            final boundary = _reorderBoundaryForDrop(
              draggedTask: draggedTask,
              targetTask: targetTask,
              dropAfterTarget: dropAfterTarget,
              siblings: _siblingsOf(targetTask, reorderScope),
            );
            if (boundary == null) {
              return;
            }
            await widget.onMoveTask(
              task: draggedTask,
              previousTaskId: boundary.previousTaskId,
              nextTaskId: boundary.nextTaskId,
            );
          },
      onMoveUp: siblingIndex > 0
          ? () {
              final boundary = _reorderBoundaryForAdjacentMove(
                siblingIndex: siblingIndex,
                siblings: siblings,
                direction: _TaskMoveDirection.up,
              );
              unawaited(
                widget.onMoveTask(
                  task: task,
                  previousTaskId: boundary.previousTaskId,
                  nextTaskId: boundary.nextTaskId,
                ),
              );
            }
          : null,
      onMoveDown: siblingIndex < siblings.length - 1
          ? () {
              final boundary = _reorderBoundaryForAdjacentMove(
                siblingIndex: siblingIndex,
                siblings: siblings,
                direction: _TaskMoveDirection.down,
              );
              unawaited(
                widget.onMoveTask(
                  task: task,
                  previousTaskId: boundary.previousTaskId,
                  nextTaskId: boundary.nextTaskId,
                ),
              );
            }
          : null,
      child: retainedRow,
    );
  }
}
