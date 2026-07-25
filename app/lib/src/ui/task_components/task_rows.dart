part of '../task_components.dart';

class AppHomeTaskRow extends StatelessWidget {
  const AppHomeTaskRow({
    super.key,
    required this.title,
    required this.isDone,
    required this.listName,
    required this.parentTaskName,
    required this.parentTaskSemanticLabel,
    required this.dueLabel,
    required this.dueTone,
    required this.onTap,
    this.depth = 0,
    this.semanticLabel,
    this.checkboxKey,
    this.priority = 0,
    this.priorityDotKey,
    this.prioritySemanticLabel,
    this.dueSemanticLabel,
    this.hierarchyGuideKey,
    this.hierarchyGuideHorizontalKey,
    this.isLastSibling = true,
    this.ancestorLineContinuations = const <bool>[],
    this.toggleDoneTooltip,
    this.onToggleDone,
  });

  final String title;
  final bool isDone;
  final int depth;
  final String listName;
  final String? parentTaskName;
  final String? parentTaskSemanticLabel;
  final String? dueLabel;
  final HomeDueDateTone dueTone;
  final String? semanticLabel;
  final Key? checkboxKey;
  final int priority;
  final Key? priorityDotKey;
  final String? prioritySemanticLabel;
  final String? dueSemanticLabel;
  final Key? hierarchyGuideKey;
  final Key? hierarchyGuideHorizontalKey;
  final bool isLastSibling;
  final List<bool> ancestorLineContinuations;
  final String? toggleDoneTooltip;
  final VoidCallback? onToggleDone;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final colorScheme = theme.colorScheme;
    final effectiveDepth = math.min(depth, 4);
    return Material(
      color: Colors.transparent,
      shape: const RoundedRectangleBorder(),
      child: Stack(
        children: [
          if (effectiveDepth > 0)
            _TaskHierarchyGuide(
              depth: effectiveDepth,
              isLastSibling: isLastSibling,
              ancestorLineContinuations: ancestorLineContinuations,
              rootLeadingStart: _homeTaskRowRootLeadingStart,
              currentVerticalKey: hierarchyGuideKey,
              horizontalKey: hierarchyGuideHorizontalKey,
            ),
          Semantics(
            container: true,
            explicitChildNodes: true,
            button: true,
            label: semanticLabel,
            child: InkWell(
              borderRadius: BorderRadius.circular(AppRadius.sm),
              onTap: onTap,
              child: Padding(
                padding: EdgeInsetsDirectional.only(
                  start:
                      _homeTaskRowRootLeadingStart +
                      (effectiveDepth * _taskRowDepthIndent),
                  top: AppSpacing.xs,
                  end: 12,
                  bottom: AppSpacing.xs,
                ),
                child: Row(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    AppTaskCheckbox(
                      checkboxKey: checkboxKey,
                      isDone: isDone,
                      tooltip: toggleDoneTooltip,
                      onToggleDone: onToggleDone,
                    ),
                    const SizedBox(width: AppSpacing.xs),
                    Expanded(
                      child: Padding(
                        padding: const EdgeInsets.only(top: 13, bottom: 3),
                        child: Column(
                          mainAxisSize: MainAxisSize.min,
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: [
                            AppAnimatedTaskTitle(
                              title,
                              isDone: isDone,
                              maxLines: 3,
                              overflow: TextOverflow.ellipsis,
                              style: theme.textTheme.titleMedium?.copyWith(
                                decoration: isDone
                                    ? TextDecoration.lineThrough
                                    : null,
                                color: isDone
                                    ? colorScheme.onSurfaceVariant
                                    : colorScheme.onSurface,
                              ),
                            ),
                            if (parentTaskName != null ||
                                listName.isNotEmpty ||
                                priority > 0 ||
                                dueLabel != null) ...[
                              const SizedBox(height: AppSpacing.xs),
                              _HomeTaskMetadata(
                                priority: priority,
                                priorityDotKey: priorityDotKey,
                                prioritySemanticLabel: prioritySemanticLabel,
                                parentTaskName: parentTaskName,
                                parentTaskSemanticLabel:
                                    parentTaskSemanticLabel,
                                listName: listName,
                                dueLabel: dueLabel,
                                dueSemanticLabel: dueSemanticLabel,
                                dueTone: dueTone,
                                isMuted: isDone,
                              ),
                            ],
                          ],
                        ),
                      ),
                    ),
                  ],
                ),
              ),
            ),
          ),
        ],
      ),
    );
  }
}

class _HomeTaskMetadata extends StatelessWidget {
  const _HomeTaskMetadata({
    required this.priority,
    required this.parentTaskName,
    required this.parentTaskSemanticLabel,
    required this.listName,
    required this.dueLabel,
    required this.dueTone,
    required this.isMuted,
    this.priorityDotKey,
    this.prioritySemanticLabel,
    this.dueSemanticLabel,
  });

  final int priority;
  final String? parentTaskName;
  final String? parentTaskSemanticLabel;
  final String listName;
  final String? dueLabel;
  final HomeDueDateTone dueTone;
  final bool isMuted;
  final Key? priorityDotKey;
  final String? prioritySemanticLabel;
  final String? dueSemanticLabel;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final defaultColor = theme.colorScheme.onSurfaceVariant.withValues(
      alpha: isMuted ? 0.78 : 1,
    );
    final contextLabel = parentTaskName ?? (listName.isEmpty ? null : listName);
    final contextSemantics = parentTaskName == null
        ? null
        : parentTaskSemanticLabel;
    final dueColor = isMuted
        ? defaultColor
        : switch (dueTone) {
            HomeDueDateTone.overdue => _priorityHighCoral,
            _ => defaultColor,
          };
    return Wrap(
      spacing: AppSpacing.xs,
      runSpacing: 2,
      crossAxisAlignment: WrapCrossAlignment.center,
      children: [
        if (priority > 0)
          PriorityDot(
            key: priorityDotKey,
            priority: priority,
            semanticLabel: prioritySemanticLabel,
            isMuted: isMuted,
          ),
        if (contextLabel != null)
          Semantics(
            label: contextSemantics,
            child: Text(
              contextLabel,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: theme.textTheme.labelMedium?.copyWith(
                color: defaultColor,
                fontWeight: FontWeight.w500,
              ),
            ),
          ),
        if (contextLabel != null && dueLabel != null)
          Text(
            '·',
            style: theme.textTheme.labelMedium?.copyWith(color: defaultColor),
          ),
        if (dueLabel != null)
          Semantics(
            label: dueSemanticLabel,
            child: Text(
              dueLabel!,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: theme.textTheme.labelMedium?.copyWith(
                color: dueColor,
                fontWeight: FontWeight.w500,
              ),
            ),
          ),
      ],
    );
  }
}

class AppTaskRow extends StatelessWidget {
  const AppTaskRow({
    super.key,
    required this.title,
    required this.isDone,
    required this.metadata,
    required this.onTap,
    this.depth = 0,
    this.semanticLabel,
    this.checkboxKey,
    this.priority = 0,
    this.priorityDotKey,
    this.prioritySemanticLabel,
    this.hierarchyGuideKey,
    this.hierarchyGuideHorizontalKey,
    this.isLastSibling = true,
    this.ancestorLineContinuations = const <bool>[],
    this.toggleDoneTooltip,
    this.framed = true,
    this.onToggleDone,
    this.trailing,
  });

  final String title;
  final bool isDone;
  final int depth;
  final String? semanticLabel;
  final Key? checkboxKey;
  final int priority;
  final Key? priorityDotKey;
  final String? prioritySemanticLabel;
  final Key? hierarchyGuideKey;
  final Key? hierarchyGuideHorizontalKey;
  final bool isLastSibling;
  final List<bool> ancestorLineContinuations;
  final String? toggleDoneTooltip;
  final List<TaskMetadataItem> metadata;
  final bool framed;
  final VoidCallback? onToggleDone;
  final Widget? trailing;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final colorScheme = theme.colorScheme;
    final effectiveDepth = math.min(depth, 4);

    return Material(
      color: Colors.transparent,
      shape: const RoundedRectangleBorder(),
      child: Stack(
        children: [
          if (effectiveDepth > 0)
            _TaskHierarchyGuide(
              depth: effectiveDepth,
              isLastSibling: isLastSibling,
              ancestorLineContinuations: ancestorLineContinuations,
              rootLeadingStart: _taskRowRootLeadingStart,
              currentVerticalKey: hierarchyGuideKey,
              horizontalKey: hierarchyGuideHorizontalKey,
            ),
          // Density-compressed row (task-30/task-43): a metadata-less task is
          // just the leading control and title; priority lives in the
          // metadata row so wrapped titles keep a stable left edge.
          Semantics(
            container: true,
            explicitChildNodes: true,
            button: true,
            label: semanticLabel,
            child: InkWell(
              onTap: onTap,
              child: Padding(
                padding: EdgeInsetsDirectional.only(
                  start:
                      _taskRowRootLeadingStart +
                      (effectiveDepth * _taskRowDepthIndent),
                  top: AppSpacing.xs,
                  end: AppSpacing.sm,
                  bottom: AppSpacing.xs,
                ),
                child: Row(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    AppTaskCheckbox(
                      checkboxKey: checkboxKey,
                      isDone: isDone,
                      tooltip: toggleDoneTooltip,
                      onToggleDone: onToggleDone,
                    ),
                    const SizedBox(width: AppSpacing.xs),
                    Expanded(
                      child: Padding(
                        padding: const EdgeInsets.only(top: 13, bottom: 3),
                        child: Column(
                          mainAxisSize: MainAxisSize.min,
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: [
                            Row(
                              crossAxisAlignment: CrossAxisAlignment.center,
                              children: [
                                Expanded(
                                  child: AppAnimatedTaskTitle(
                                    title,
                                    isDone: isDone,
                                    softWrap: true,
                                    style: theme.textTheme.titleMedium
                                        ?.copyWith(
                                          decoration: isDone
                                              ? TextDecoration.lineThrough
                                              : null,
                                          color: isDone
                                              ? colorScheme.onSurfaceVariant
                                              : colorScheme.onSurface,
                                        ),
                                  ),
                                ),
                              ],
                            ),
                            if (metadata.isNotEmpty) ...[
                              const SizedBox(height: AppSpacing.xs),
                              TaskMetadata(
                                items: metadata,
                                priority: priority,
                                priorityDotKey: priorityDotKey,
                                prioritySemanticLabel: prioritySemanticLabel,
                                isPriorityMuted: isDone,
                              ),
                            ] else if (priority > 0) ...[
                              const SizedBox(height: AppSpacing.xs),
                              TaskMetadata(
                                items: const [],
                                priority: priority,
                                priorityDotKey: priorityDotKey,
                                prioritySemanticLabel: prioritySemanticLabel,
                                isPriorityMuted: isDone,
                              ),
                            ],
                          ],
                        ),
                      ),
                    ),
                    if (trailing != null) ...[
                      const SizedBox(width: AppSpacing.xs),
                      SizedBox(height: 48, child: Center(child: trailing)),
                    ],
                  ],
                ),
              ),
            ),
          ),
        ],
      ),
    );
  }
}

class _TaskHierarchyGuide extends StatelessWidget {
  const _TaskHierarchyGuide({
    required this.depth,
    required this.isLastSibling,
    required this.ancestorLineContinuations,
    required this.rootLeadingStart,
    this.currentVerticalKey,
    this.horizontalKey,
  });

  static const double _lineWidth = 1.5;
  static const double _leadingCenterY =
      AppSpacing.xs + (_taskCheckboxTapSize / 2);

  final int depth;
  final bool isLastSibling;
  final List<bool> ancestorLineContinuations;
  final double rootLeadingStart;
  final Key? currentVerticalKey;
  final Key? horizontalKey;

  @override
  Widget build(BuildContext context) {
    final color = Theme.of(context).colorScheme.outlineVariant;
    final children = <Widget>[];
    final ancestorCount = math.max(0, depth - 1);

    for (var level = 0; level < ancestorCount; level += 1) {
      if (level >= ancestorLineContinuations.length ||
          !ancestorLineContinuations[level]) {
        continue;
      }
      children.add(
        PositionedDirectional(
          start: _guideXForLevel(level) - (_lineWidth / 2),
          top: 0,
          bottom: 0,
          child: _GuideLine(color: color, width: _lineWidth),
        ),
      );
    }

    final currentLevel = depth - 1;
    final currentX = _guideXForLevel(currentLevel);
    final childCenterX = _checkboxCenterXForDepth(depth);
    final horizontalEndX =
        childCenterX -
        _taskCheckboxVisualRadius -
        _taskHierarchyHorizontalEndGap;
    children.addAll([
      PositionedDirectional(
        start: currentX - (_lineWidth / 2),
        top: 0,
        height: _leadingCenterY,
        child: _GuideLine(
          key: currentVerticalKey,
          color: color,
          width: _lineWidth,
        ),
      ),
      if (!isLastSibling)
        PositionedDirectional(
          start: currentX - (_lineWidth / 2),
          top: _leadingCenterY,
          bottom: 0,
          child: _GuideLine(color: color, width: _lineWidth),
        ),
      PositionedDirectional(
        start: currentX,
        top: _leadingCenterY - (_lineWidth / 2),
        child: _GuideLine(
          key: horizontalKey,
          color: color,
          width: math.max(0, horizontalEndX - currentX),
          height: _lineWidth,
        ),
      ),
    ]);

    return Positioned.fill(
      child: IgnorePointer(child: Stack(children: children)),
    );
  }

  double _guideXForLevel(int level) {
    return _checkboxCenterXForDepth(level);
  }

  double _checkboxCenterXForDepth(int targetDepth) {
    if (targetDepth == 0) {
      return rootLeadingStart + _taskCheckboxVisualCenterOffset;
    }
    return rootLeadingStart +
        (targetDepth * _taskRowDepthIndent) +
        _taskCheckboxVisualCenterOffset;
  }
}

class _GuideLine extends StatelessWidget {
  const _GuideLine({
    super.key,
    required this.color,
    required this.width,
    this.height,
  });

  final Color color;
  final double width;
  final double? height;

  @override
  Widget build(BuildContext context) {
    return DecoratedBox(
      decoration: BoxDecoration(
        color: color,
        borderRadius: BorderRadius.circular(999),
      ),
      child: SizedBox(width: width, height: height),
    );
  }
}
