part of '../task_components.dart';

class PriorityDot extends StatelessWidget {
  const PriorityDot({
    super.key,
    required this.priority,
    this.semanticLabel,
    required this.isMuted,
  });

  final int priority;
  final String? semanticLabel;
  final bool isMuted;

  @override
  Widget build(BuildContext context) {
    if (priority <= 0) {
      return const SizedBox.shrink();
    }

    final color = priorityDotColor(priority);
    final dot = Container(
      width: 11,
      height: 11,
      decoration: BoxDecoration(
        color: isMuted ? color.withValues(alpha: 0.45) : color,
        shape: BoxShape.circle,
      ),
    );

    final label = semanticLabel;
    if (label == null) {
      return dot;
    }

    return Tooltip(
      message: label,
      child: Semantics(label: label, child: dot),
    );
  }
}

/// Design-direction priority dot color for `priority` (1=low, 2=medium,
/// 3=high). Priority "none" (0 or below) is not represented by a dot at all.
Color priorityDotColor(int priority) {
  return switch (priority) {
    1 => _priorityLowSoftSage,
    2 => _priorityMediumAmber,
    3 => _priorityHighCoral,
    _ => Colors.transparent,
  };
}
