part of '../task_components.dart';

/// Design-direction priority accent tokens (`docs/design/visual-direction.md`
/// Design Tokens table): high=coral, medium=amber, low=softSage.
const _priorityHighCoral = Color(0xFFE8755A);
const _priorityMediumAmber = Color(0xFFEDB73E);
const _priorityLowSoftSage = Color(0xFFA8BEA8);
const _homeTaskRowRootLeadingStart = 11.0;
const _taskRowRootLeadingStart = 12.0;
const _taskRowDepthIndent = AppSpacing.lg;
const _taskCheckboxTapSize = 48.0;
const _taskCheckboxVisualSize = 22.0;
const _taskCheckboxVisualCenterOffset = _taskCheckboxTapSize / 2;
const _taskCheckboxVisualRadius = _taskCheckboxVisualSize / 2;
const _taskHierarchyHorizontalEndGap = 4.0;
const _taskCompletionHaloKey = ValueKey('task-completion-halo');
const _taskStrikethroughOverlayKey = ValueKey('task-strikethrough-overlay');
