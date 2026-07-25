part of '../tasks_screen.dart';

class _TaskDragReorderTarget extends StatelessWidget {
  const _TaskDragReorderTarget({
    super.key,
    required this.enabled,
    required this.task,
    required this.siblings,
    required this.siblingIndex,
    required this.dropIndicator,
    required this.onHover,
    required this.onLeave,
    required this.onDrop,
    required this.onMoveUp,
    required this.onMoveDown,
    required this.child,
  });

  final bool enabled;
  final TaskDto task;
  final List<TaskDto> siblings;
  final int siblingIndex;
  final _TaskDropIndicator? dropIndicator;
  final ValueChanged<_TaskDropIndicator> onHover;
  final VoidCallback onLeave;
  final Future<void> Function({
    required TaskDto draggedTask,
    required TaskDto targetTask,
    required bool dropAfterTarget,
  })
  onDrop;
  final VoidCallback? onMoveUp;
  final VoidCallback? onMoveDown;
  final Widget child;

  @override
  Widget build(BuildContext context) {
    final l10n = AppLocalizations.of(context)!;
    final semanticsActions = <CustomSemanticsAction, VoidCallback>{};
    final moveUp = onMoveUp;
    if (enabled && moveUp != null) {
      semanticsActions[CustomSemanticsAction(label: l10n.moveTaskUpTooltip)] =
          moveUp;
    }
    final moveDown = onMoveDown;
    if (enabled && moveDown != null) {
      semanticsActions[CustomSemanticsAction(label: l10n.moveTaskDownTooltip)] =
          moveDown;
    }

    return DragTarget<_TaskDragData>(
      onWillAcceptWithDetails: (details) =>
          enabled && _canAcceptDrop(details.data.task),
      onMove: (details) {
        if (!enabled || !_canAcceptDrop(details.data.task)) {
          return;
        }
        onHover(
          _TaskDropIndicator(
            taskId: task.id,
            dropAfter: _dropAfterFor(details.data.task),
          ),
        );
      },
      onLeave: (_) => onLeave(),
      onAcceptWithDetails: (details) async {
        if (!enabled || !_canAcceptDrop(details.data.task)) {
          onLeave();
          return;
        }
        await onDrop(
          draggedTask: details.data.task,
          targetTask: task,
          dropAfterTarget: _dropAfterFor(details.data.task),
        );
      },
      builder: (context, candidateData, rejectedData) {
        final indicatedBefore =
            dropIndicator?.taskId == task.id &&
            dropIndicator?.dropAfter == false;
        final indicatedAfter =
            dropIndicator?.taskId == task.id &&
            dropIndicator?.dropAfter == true;
        final row = Semantics(
          key: ValueKey('task-reorder-semantics-${task.id}'),
          container: true,
          label: task.title,
          customSemanticsActions: semanticsActions,
          child: _TaskDropIndicatorFrame(
            showBefore: indicatedBefore,
            showAfter: indicatedAfter,
            child: child,
          ),
        );
        return LongPressDraggable<_TaskDragData>(
          data: _TaskDragData(task),
          maxSimultaneousDrags: enabled && siblings.length > 1 ? 1 : 0,
          axis: Axis.vertical,
          feedback: _TaskDragFeedback(child: child),
          childWhenDragging: Opacity(opacity: 0.45, child: child),
          onDragEnd: (_) => onLeave(),
          onDraggableCanceled: (_, _) => onLeave(),
          child: row,
        );
      },
    );
  }

  bool _canAcceptDrop(TaskDto draggedTask) {
    if (draggedTask.id == task.id ||
        draggedTask.listId != task.listId ||
        draggedTask.parentTaskId != task.parentTaskId ||
        isTaskClosed(draggedTask) ||
        isTaskClosed(task)) {
      return false;
    }
    return siblings.any((sibling) => sibling.id == draggedTask.id) &&
        siblings.any((sibling) => sibling.id == task.id);
  }

  bool _dropAfterFor(TaskDto draggedTask) {
    final draggedIndex = siblings.indexWhere(
      (sibling) => sibling.id == draggedTask.id,
    );
    final targetIndex = siblings.indexWhere((sibling) => sibling.id == task.id);
    if (draggedIndex < 0 || targetIndex < 0) {
      return false;
    }
    return draggedIndex < targetIndex;
  }
}

class _TaskDragFeedback extends StatelessWidget {
  const _TaskDragFeedback({required this.child});

  final Widget child;

  @override
  Widget build(BuildContext context) {
    final width = MediaQuery.sizeOf(context).width - (AppSpacing.md * 2);
    final colorScheme = Theme.of(context).colorScheme;
    return Material(
      color: Colors.transparent,
      elevation: 1,
      shadowColor: colorScheme.shadow.withValues(alpha: 0.14),
      borderRadius: BorderRadius.circular(16),
      child: SizedBox(width: width, child: child),
    );
  }
}

class _TaskDropIndicatorFrame extends StatelessWidget {
  const _TaskDropIndicatorFrame({
    required this.showBefore,
    required this.showAfter,
    required this.child,
  });

  final bool showBefore;
  final bool showAfter;
  final Widget child;

  @override
  Widget build(BuildContext context) {
    final color = Theme.of(context).colorScheme.primary.withValues(alpha: 0.62);
    return Stack(
      clipBehavior: Clip.none,
      children: [
        child,
        if (showBefore)
          PositionedDirectional(
            start: 0,
            end: 0,
            top: -1,
            child: _TaskDropIndicatorLine(color: color),
          ),
        if (showAfter)
          PositionedDirectional(
            start: 0,
            end: 0,
            bottom: -1,
            child: _TaskDropIndicatorLine(color: color),
          ),
      ],
    );
  }
}

class _TaskDropIndicatorLine extends StatelessWidget {
  const _TaskDropIndicatorLine({required this.color});

  final Color color;

  @override
  Widget build(BuildContext context) {
    return DecoratedBox(
      decoration: BoxDecoration(
        color: color,
        borderRadius: BorderRadius.circular(999),
      ),
      child: const SizedBox(height: 1),
    );
  }
}

class _TaskDragData {
  const _TaskDragData(this.task);

  final TaskDto task;
}

class _TaskDropIndicator {
  const _TaskDropIndicator({required this.taskId, required this.dropAfter});

  final String taskId;
  final bool dropAfter;
}

enum _TaskMoveDirection { up, down }

({String? previousTaskId, String? nextTaskId}) _reorderBoundaryForAdjacentMove({
  required int siblingIndex,
  required List<TaskDto> siblings,
  required _TaskMoveDirection direction,
}) {
  return switch (direction) {
    _TaskMoveDirection.up => (
      previousTaskId: siblingIndex >= 2 ? siblings[siblingIndex - 2].id : null,
      nextTaskId: siblings[siblingIndex - 1].id,
    ),
    _TaskMoveDirection.down => (
      previousTaskId: siblings[siblingIndex + 1].id,
      nextTaskId: siblingIndex + 2 < siblings.length
          ? siblings[siblingIndex + 2].id
          : null,
    ),
  };
}

({String? previousTaskId, String? nextTaskId})? _reorderBoundaryForDrop({
  required TaskDto draggedTask,
  required TaskDto targetTask,
  required bool dropAfterTarget,
  required List<TaskDto> siblings,
}) {
  if (draggedTask.id == targetTask.id ||
      draggedTask.parentTaskId != targetTask.parentTaskId) {
    return null;
  }
  final beforeIds = siblings.map((task) => task.id).toList(growable: false);
  if (!beforeIds.contains(draggedTask.id) ||
      !beforeIds.contains(targetTask.id)) {
    return null;
  }

  final remaining = siblings
      .where((task) => task.id != draggedTask.id)
      .toList(growable: false);
  final targetIndex = remaining.indexWhere((task) => task.id == targetTask.id);
  if (targetIndex < 0) {
    return null;
  }
  final insertIndex = targetIndex + (dropAfterTarget ? 1 : 0);
  final afterIds = [
    for (var index = 0; index < remaining.length; index += 1) ...[
      if (index == insertIndex) draggedTask.id,
      remaining[index].id,
    ],
    if (insertIndex == remaining.length) draggedTask.id,
  ];
  if (_sameStringOrder(beforeIds, afterIds)) {
    return null;
  }
  return (
    previousTaskId: insertIndex > 0 ? remaining[insertIndex - 1].id : null,
    nextTaskId: insertIndex < remaining.length
        ? remaining[insertIndex].id
        : null,
  );
}

bool _sameStringOrder(List<String> a, List<String> b) {
  if (a.length != b.length) {
    return false;
  }
  for (var index = 0; index < a.length; index += 1) {
    if (a[index] != b[index]) {
      return false;
    }
  }
  return true;
}

List<TaskDto> _siblingsOf(TaskDto task, List<TaskDto> tasks) {
  final siblings = tasks
      .where((candidate) => candidate.parentTaskId == task.parentTaskId)
      .toList();
  siblings.sort((a, b) {
    final sortOrder = a.sortOrder.compareTo(b.sortOrder);
    if (sortOrder != 0) {
      return sortOrder;
    }
    return a.id.compareTo(b.id);
  });
  return siblings;
}
