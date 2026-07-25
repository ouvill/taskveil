part of '../tasks_screen.dart';

class _TaskSwipeActions extends StatelessWidget {
  const _TaskSwipeActions({
    super.key,
    required this.task,
    required this.isClosed,
    required this.onLeadingAction,
    required this.child,
  });

  final TaskDto task;
  final bool isClosed;
  final Future<void> Function() onLeadingAction;
  final Widget child;

  @override
  Widget build(BuildContext context) {
    final l10n = AppLocalizations.of(context)!;
    final colorScheme = Theme.of(context).colorScheme;
    return Slidable(
      key: ValueKey('task-slidable-${task.id}'),
      startActionPane: ActionPane(
        motion: const DrawerMotion(),
        extentRatio: 0.28,
        children: [
          SlidableAction(
            key: ValueKey('task-swipe-leading-${task.id}'),
            onPressed: (_) => unawaited(onLeadingAction()),
            backgroundColor: colorScheme.primary,
            foregroundColor: colorScheme.onPrimary,
            icon: isClosed ? LucideIcons.circle300 : LucideIcons.circleCheck300,
            label: isClosed
                ? l10n.reopenTaskMenuItem
                : l10n.markTaskDoneMenuItem,
          ),
        ],
      ),
      endActionPane: isClosed
          ? null
          : ActionPane(
              motion: const DrawerMotion(),
              extentRatio: 0.34,
              children: [
                SlidableAction(
                  key: ValueKey('task-swipe-focus-${task.id}'),
                  onPressed: (_) =>
                      context.push('/focus/${task.listId}/${task.id}'),
                  backgroundColor: colorScheme.primaryContainer,
                  foregroundColor: colorScheme.onPrimaryContainer,
                  icon: LucideIcons.timer300,
                  label: l10n.focusTitle,
                ),
              ],
            ),
      child: child,
    );
  }
}

class _TaskEntryMotion extends StatefulWidget {
  const _TaskEntryMotion({required this.child, this.slide = true});

  final Widget child;
  final bool slide;

  @override
  State<_TaskEntryMotion> createState() => _TaskEntryMotionState();
}

class _TaskEntryMotionState extends State<_TaskEntryMotion>
    with SingleTickerProviderStateMixin {
  late final AnimationController _controller;
  late final Animation<double> _opacity;
  late final Animation<Offset> _offset;

  @override
  void initState() {
    super.initState();
    _controller = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 180),
    );
    final curved = CurvedAnimation(
      parent: _controller,
      curve: Curves.easeOutCubic,
    );
    _opacity = Tween<double>(begin: 0, end: 1).animate(curved);
    _offset = Tween<Offset>(
      begin: widget.slide ? const Offset(0, 0.04) : Offset.zero,
      end: Offset.zero,
    ).animate(curved);
    _controller.forward();
  }

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return FadeTransition(
      opacity: _opacity,
      child: SlideTransition(position: _offset, child: widget.child),
    );
  }
}

class _TaskRowsSliver extends StatelessWidget {
  const _TaskRowsSliver({
    required this.nodes,
    required this.separatorHeight,
    required this.rowBuilder,
  });

  final List<FlattenedTaskTreeNode> nodes;
  final double separatorHeight;
  final Widget Function(BuildContext context, FlattenedTaskTreeNode node)
  rowBuilder;

  @override
  Widget build(BuildContext context) {
    if (nodes.isEmpty) {
      return const SliverToBoxAdapter(child: SizedBox.shrink());
    }
    return SliverList.builder(
      itemCount: nodes.length * 2 - 1,
      itemBuilder: (context, index) {
        if (index.isOdd) {
          return SizedBox(height: separatorHeight / 2);
        }
        return rowBuilder(context, nodes[index ~/ 2]);
      },
    );
  }
}
