import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:go_router/go_router.dart';
import 'package:intl/intl.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';
import 'package:taskveil/src/core/civil_time.dart';
import 'package:taskveil/src/core/providers.dart';
import 'package:taskveil/src/core/task_due.dart';
import 'package:taskveil/src/core/task_tree.dart';
import 'package:taskveil/src/generated/l10n/app_localizations.dart';
import 'package:taskveil/src/rust/api.dart';
import 'package:taskveil/src/ui/bridge_error_messages.dart';
import 'package:taskveil/src/ui/dialogs.dart';
import 'package:taskveil/src/ui/header_actions.dart';
import 'package:taskveil/src/ui/states.dart';
import 'package:taskveil/src/ui/task_completion_motion.dart';
import 'package:taskveil/src/ui/task_components.dart';
import 'package:taskveil/src/ui/theme.dart';

enum CalendarViewMode { week, month }

@immutable
class CalendarOccurrenceKey {
  const CalendarOccurrenceKey({
    required this.taskId,
    required this.kind,
    required this.marker,
  });

  factory CalendarOccurrenceKey.fromOccurrence(
    CalendarOccurrenceDto occurrence,
  ) {
    return CalendarOccurrenceKey(
      taskId: occurrence.task.id,
      kind: _occurrenceKindToken(occurrence.kind),
      marker: _occurrenceMarker(occurrence.kind),
    );
  }

  final String taskId;
  final String kind;
  final String marker;

  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      other is CalendarOccurrenceKey &&
          taskId == other.taskId &&
          kind == other.kind &&
          marker == other.marker;

  @override
  int get hashCode => Object.hash(taskId, kind, marker);

  @override
  String toString() => '$taskId:$kind:$marker';
}

class CalendarScreen extends ConsumerStatefulWidget {
  const CalendarScreen({super.key});

  @override
  ConsumerState<CalendarScreen> createState() => _CalendarScreenState();
}

class _CalendarScreenState extends ConsumerState<CalendarScreen> {
  late DateTime _selectedDay;
  late DateTime _anchorDay;
  late final TaskCompletionRetentionController<CalendarOccurrenceKey>
  _completionRetentionController;
  final Map<CalendarOccurrenceKey, CalendarOccurrenceDto> _retainedOccurrences =
      {};
  CalendarViewMode _viewMode = CalendarViewMode.week;
  bool _showCompleted = false;

  @override
  void initState() {
    super.initState();
    _selectedDay = _dateOnly(DateTime.now());
    _anchorDay = _selectedDay;
    _completionRetentionController =
        TaskCompletionRetentionController<CalendarOccurrenceKey>()
          ..addListener(_onCompletionRetentionChanged);
  }

  @override
  void dispose() {
    _completionRetentionController
      ..removeListener(_onCompletionRetentionChanged)
      ..dispose();
    super.dispose();
  }

  void _onCompletionRetentionChanged() {
    if (!mounted) {
      return;
    }
    final retainedKeys = _completionRetentionController.keys.toSet();
    setState(() {
      _retainedOccurrences.removeWhere((key, _) => !retainedKeys.contains(key));
    });
  }

  @override
  Widget build(BuildContext context) {
    final l10n = AppLocalizations.of(context)!;
    final weekStart =
        ref.watch(calendarWeekStartProvider).value ?? defaultCalendarWeekStart;
    final range = _calendarRange(context, weekStart);
    final occurrences = ref.watch(calendarOccurrencesProvider(range));
    return Scaffold(
      backgroundColor: AppColors.canvas,
      body: SafeArea(
        child: Align(
          alignment: Alignment.topCenter,
          child: ConstrainedBox(
            constraints: const BoxConstraints(maxWidth: 1120),
            child: Column(
              children: [
                Padding(
                  padding: const EdgeInsets.fromLTRB(
                    AppSpacing.md,
                    12,
                    AppSpacing.sm,
                    AppSpacing.sm,
                  ),
                  child: _buildHeader(context, l10n),
                ),
                const Divider(height: 1, color: AppColors.hairline),
                Expanded(
                  child: occurrences.when(
                    loading: () => Semantics(
                      label: l10n.calendarLoadingSemantics,
                      child: const AppLoadingState(),
                    ),
                    error: (error, stackTrace) => AppEmptyState(
                      icon: LucideIcons.calendarX300,
                      title: l10n.calendarLoadFailed,
                      action: TextButton(
                        onPressed: () =>
                            ref.invalidate(calendarOccurrencesProvider(range)),
                        child: Text(l10n.calendarRetryButton),
                      ),
                    ),
                    data: (items) => _buildCalendar(context, items, weekStart),
                  ),
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }

  Widget _buildHeader(BuildContext context, AppLocalizations l10n) {
    final theme = Theme.of(context);
    final width = MediaQuery.sizeOf(context).width;
    final scaledUnit = MediaQuery.textScalerOf(context).scale(1);
    final splitControls = width < 360 || scaledUnit > 1.3;
    final modeSwitch = KeyedSubtree(
      key: const ValueKey('calendar-header-mode-row'),
      child: _buildModeSwitch(context, l10n),
    );
    final periodControls = Row(
      key: const ValueKey('calendar-header-period-row'),
      mainAxisSize: MainAxisSize.min,
      children: [
        _CalendarHeaderButton(
          tooltip: l10n.calendarPreviousPeriodTooltip,
          icon: LucideIcons.chevronLeft300,
          onPressed: () => _movePeriod(-1),
        ),
        _CalendarHeaderButton(
          tooltip: l10n.calendarGoToToday,
          label: l10n.calendarGoToToday,
          onPressed: _goToToday,
        ),
        _CalendarHeaderButton(
          tooltip: l10n.calendarNextPeriodTooltip,
          icon: LucideIcons.chevronRight300,
          onPressed: () => _movePeriod(1),
        ),
      ],
    );
    return Column(
      children: [
        Row(
          children: [
            Expanded(
              child: Text(
                l10n.calendarTitle,
                style: theme.textTheme.headlineMedium?.copyWith(
                  fontWeight: FontWeight.w700,
                  letterSpacing: -0.5,
                ),
              ),
            ),
            const AppHeaderSearchAction(),
          ],
        ),
        const SizedBox(height: AppSpacing.xs),
        if (splitControls) ...[
          modeSwitch,
          const SizedBox(height: AppSpacing.xs),
          Align(
            alignment: AlignmentDirectional.centerEnd,
            child: periodControls,
          ),
        ] else
          Row(
            children: [
              Expanded(child: modeSwitch),
              const SizedBox(width: AppSpacing.sm),
              periodControls,
            ],
          ),
      ],
    );
  }

  Widget _buildModeSwitch(BuildContext context, AppLocalizations l10n) {
    return Row(
      children: [
        Expanded(
          child: _CalendarModeButton(
            key: const ValueKey('calendar-mode-week'),
            label: l10n.calendarWeekTab,
            selected: _viewMode == CalendarViewMode.week,
            onTap: () => _setViewMode(CalendarViewMode.week),
          ),
        ),
        Expanded(
          child: _CalendarModeButton(
            key: const ValueKey('calendar-mode-month'),
            label: l10n.calendarMonthTab,
            selected: _viewMode == CalendarViewMode.month,
            onTap: () => _setViewMode(CalendarViewMode.month),
          ),
        ),
      ],
    );
  }

  Widget _buildCalendar(
    BuildContext context,
    List<CalendarOccurrenceDto> occurrences,
    String weekStart,
  ) {
    final isTwoPane = MediaQuery.sizeOf(context).width >= 1024;
    final selector = _viewMode == CalendarViewMode.week
        ? _buildWeekSelector(context, occurrences, weekStart)
        : _buildMonthSelector(context, occurrences, weekStart);
    final agenda = _buildAgenda(context, occurrences);
    if (isTwoPane) {
      return Row(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Expanded(
            flex: 11,
            child: SingleChildScrollView(
              padding: const EdgeInsets.all(AppSpacing.lg),
              child: selector,
            ),
          ),
          const VerticalDivider(width: 1, color: AppColors.hairline),
          Expanded(
            flex: 9,
            child: SingleChildScrollView(
              padding: const EdgeInsets.fromLTRB(
                AppSpacing.lg,
                AppSpacing.lg,
                AppSpacing.lg,
                AppSpacing.xl * 3,
              ),
              child: agenda,
            ),
          ),
        ],
      );
    }
    return SingleChildScrollView(
      padding: const EdgeInsets.fromLTRB(
        AppSpacing.md,
        AppSpacing.lg,
        AppSpacing.md,
        AppSpacing.xl * 3,
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          selector,
          const SizedBox(height: AppSpacing.lg),
          const Divider(height: 1, color: AppColors.hairline),
          const SizedBox(height: AppSpacing.lg),
          agenda,
        ],
      ),
    );
  }

  Widget _buildWeekSelector(
    BuildContext context,
    List<CalendarOccurrenceDto> occurrences,
    String weekStart,
  ) {
    final locale = Localizations.localeOf(context).toLanguageTag();
    final start = _weekStart(context, _anchorDay, weekStart);
    final days = [
      for (var index = 0; index < 7; index++)
        localCivilDay(start, dayOffset: index),
    ];
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(
          _weekRangeLabel(locale, days.first, days.last),
          style: Theme.of(
            context,
          ).textTheme.titleMedium?.copyWith(fontWeight: FontWeight.w600),
        ),
        const SizedBox(height: AppSpacing.md),
        Row(
          children: [
            for (final day in days)
              Expanded(
                child: _CalendarDayTarget(
                  key: ValueKey('calendar-week-day-${_civilDate(day)}'),
                  date: day,
                  selected: _sameDay(day, _selectedDay),
                  isToday: _sameDay(day, DateTime.now()),
                  count: _activeCountForDay(occurrences, day),
                  weekdayLabel: DateFormat.E(locale).format(day),
                  onSelected: () => _selectDay(day),
                  onMove: _moveOccurrence,
                ),
              ),
          ],
        ),
      ],
    );
  }

  Widget _buildMonthSelector(
    BuildContext context,
    List<CalendarOccurrenceDto> occurrences,
    String weekStart,
  ) {
    final locale = Localizations.localeOf(context).toLanguageTag();
    final monthStart = DateTime(_anchorDay.year, _anchorDay.month);
    final gridStart = _weekStart(context, monthStart, weekStart);
    final days = [
      for (var index = 0; index < 42; index++)
        localCivilDay(gridStart, dayOffset: index),
    ];
    final weekdays = days.take(7).toList(growable: false);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(
          DateFormat.yMMMM(locale).format(monthStart),
          style: Theme.of(
            context,
          ).textTheme.titleMedium?.copyWith(fontWeight: FontWeight.w600),
        ),
        const SizedBox(height: AppSpacing.md),
        Row(
          children: [
            for (final day in weekdays)
              Expanded(
                child: Center(
                  child: Text(
                    DateFormat.E(locale).format(day),
                    maxLines: 1,
                    style: Theme.of(context).textTheme.labelSmall?.copyWith(
                      color: AppColors.muted,
                      fontWeight: FontWeight.w600,
                    ),
                  ),
                ),
              ),
          ],
        ),
        const SizedBox(height: AppSpacing.xs),
        GridView.builder(
          key: const ValueKey('calendar-month-grid'),
          shrinkWrap: true,
          physics: const NeverScrollableScrollPhysics(),
          itemCount: days.length,
          gridDelegate: const SliverGridDelegateWithFixedCrossAxisCount(
            crossAxisCount: 7,
            mainAxisExtent: 54,
          ),
          itemBuilder: (context, index) {
            final day = days[index];
            return _CalendarMonthDayTarget(
              key: ValueKey('calendar-month-day-${_civilDate(day)}'),
              date: day,
              inMonth: day.month == monthStart.month,
              selected: _sameDay(day, _selectedDay),
              isToday: _sameDay(day, DateTime.now()),
              count: _activeCountForDay(occurrences, day),
              onSelected: () => _selectDay(day),
              onMove: _moveOccurrence,
            );
          },
        ),
      ],
    );
  }

  Widget _buildAgenda(
    BuildContext context,
    List<CalendarOccurrenceDto> occurrences,
  ) {
    final l10n = AppLocalizations.of(context)!;
    final locale = Localizations.localeOf(context).toLanguageTag();
    final selected =
        occurrences
            .where(
              (occurrence) =>
                  _sameDay(_occurrenceDate(occurrence.kind), _selectedDay),
            )
            .toList(growable: false)
          ..sort(_compareOccurrences);
    final retainingTaskIds = _retainedOccurrences.values
        .map((occurrence) => occurrence.task.id)
        .toSet();
    final completed = selected
        .where(_isCompletedOccurrence)
        .where((occurrence) => !retainingTaskIds.contains(occurrence.task.id))
        .toList();
    final activeByKey = <CalendarOccurrenceKey, CalendarOccurrenceDto>{
      for (final occurrence in selected.where(
        (item) => !_isCompletedOccurrence(item),
      ))
        CalendarOccurrenceKey.fromOccurrence(occurrence): occurrence,
    };
    for (final entry in _retainedOccurrences.entries) {
      if (_sameDay(_occurrenceDate(entry.value.kind), _selectedDay)) {
        activeByKey[entry.key] = _completedSnapshot(entry.value);
      }
    }
    final active = activeByKey.entries.toList(growable: false)
      ..sort((a, b) => _compareOccurrences(a.value, b.value));
    final treeContexts = _calendarTreeContexts(
      active.map((entry) => entry.value).toList(growable: false),
    );
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Row(
          children: [
            Expanded(
              child: Text(
                DateFormat.MMMMEEEEd(locale).format(_selectedDay),
                style: Theme.of(
                  context,
                ).textTheme.titleMedium?.copyWith(fontWeight: FontWeight.w600),
              ),
            ),
            Text(
              l10n.calendarDayTaskCount(active.length),
              style: Theme.of(
                context,
              ).textTheme.labelMedium?.copyWith(color: AppColors.muted),
            ),
          ],
        ),
        const SizedBox(height: AppSpacing.sm),
        if (active.isEmpty)
          AppEmptyState(
            icon: LucideIcons.calendarCheck300,
            title: l10n.calendarEmptyTitle,
            body: l10n.calendarEmptyBody,
          )
        else
          for (final entry in active) ...[
            _buildOccurrenceRow(
              context,
              entry.key,
              entry.value,
              treeContext:
                  treeContexts[entry.value.task.id] ??
                  const _CalendarTreeContext(),
              completionGroup: active
                  .where(
                    (candidate) =>
                        candidate.value.task.id == entry.value.task.id,
                  )
                  .toList(growable: false),
            ),
            const SizedBox(height: 2),
          ],
        if (completed.isNotEmpty) ...[
          const SizedBox(height: AppSpacing.md),
          _CalendarCompletedDisclosure(
            count: completed.length,
            expanded: _showCompleted,
            onTap: () => setState(() => _showCompleted = !_showCompleted),
          ),
          if (_showCompleted)
            for (final occurrence in completed) ...[
              const SizedBox(height: 2),
              _buildOccurrenceRow(
                context,
                CalendarOccurrenceKey.fromOccurrence(occurrence),
                occurrence,
                treeContext: const _CalendarTreeContext(),
                completionGroup: const [],
              ),
            ],
        ],
      ],
    );
  }

  Widget _buildOccurrenceRow(
    BuildContext context,
    CalendarOccurrenceKey key,
    CalendarOccurrenceDto occurrence, {
    required _CalendarTreeContext treeContext,
    required List<MapEntry<CalendarOccurrenceKey, CalendarOccurrenceDto>>
    completionGroup,
  }) {
    final l10n = AppLocalizations.of(context)!;
    final task = occurrence.task;
    final completed =
        _isCompletedOccurrence(occurrence) ||
        _retainedOccurrences.containsKey(key);
    final movingAllowed = !_isCompletedOccurrence(occurrence) && !completed;
    final kindLabel = _occurrenceKindLabel(l10n, occurrence.kind);
    final timeLabel = _occurrenceTimeLabel(context, occurrence.kind);
    final listName = occurrence.listArchived
        ? l10n.calendarArchivedListContext(occurrence.listName)
        : occurrence.listName;
    final row = AppHomeTaskRow(
      key: ValueKey('calendar-occurrence-row-$key'),
      checkboxKey: ValueKey('calendar-occurrence-check-$key'),
      title: task.title,
      isDone: completed,
      depth: treeContext.depth,
      listName: listName,
      parentTaskName: treeContext.parentTaskName,
      parentTaskSemanticLabel: treeContext.parentTaskName == null
          ? null
          : l10n.parentTaskLinkSemantics(treeContext.parentTaskName!),
      dueLabel: timeLabel.isEmpty ? kindLabel : '$kindLabel · $timeLabel',
      dueTone: _occurrenceDueTone(occurrence),
      priority: task.priority,
      priorityDotKey: ValueKey('calendar-priority-$key'),
      hierarchyGuideKey: ValueKey('calendar-hierarchy-guide-$key'),
      hierarchyGuideHorizontalKey: ValueKey(
        'calendar-hierarchy-horizontal-$key',
      ),
      isLastSibling: treeContext.isLastSibling,
      ancestorLineContinuations: treeContext.ancestorLineContinuations,
      prioritySemanticLabel: l10n.taskPriority(
        taskPriorityLabel(l10n, task.priority),
      ),
      semanticLabel: l10n.calendarOccurrenceSemantics(
        task.title,
        listName,
        kindLabel,
        timeLabel,
      ),
      toggleDoneTooltip: completed
          ? l10n.reopenTaskTooltip
          : l10n.completeTaskTooltip,
      onToggleDone: completed
          ? () => _reopenTask(occurrence)
          : () => _completeTask(completionGroup),
      onTap: () => context.push('/calendar/tasks/${task.listId}/${task.id}'),
    );
    final exiting =
        _completionRetentionController.phaseOf(key) ==
        TaskCompletionRetentionPhase.exiting;
    final animated = AppTaskCompletionExit(
      key: ValueKey('calendar-completion-exit-$key'),
      isExiting: exiting,
      child: row,
    );
    final content = Row(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Expanded(child: animated),
        if (movingAllowed)
          Semantics(
            label: l10n.calendarMoveOccurrenceSemantics(kindLabel, task.title),
            button: true,
            onTap: () => _showMoveMenu(occurrence),
            child: ExcludeSemantics(
              child: SizedBox.square(
                dimension: 48,
                child: IconButton(
                  key: ValueKey('calendar-move-menu-$key'),
                  tooltip:
                      occurrence.kind is CalendarOccurrenceKindDto_Scheduled
                      ? l10n.calendarMoveScheduledTooltip
                      : l10n.calendarMoveDueTooltip,
                  onPressed: () => _showMoveMenu(occurrence),
                  icon: const Icon(LucideIcons.calendarCog300, size: 18),
                ),
              ),
            ),
          ),
      ],
    );
    return LongPressDraggable<CalendarOccurrenceDto>(
      key: ValueKey('calendar-draggable-$key'),
      data: occurrence,
      maxSimultaneousDrags: movingAllowed ? 1 : 0,
      feedback: _CalendarDragFeedback(
        title: task.title,
        detail: '$kindLabel · $timeLabel',
      ),
      childWhenDragging: Opacity(opacity: 0.35, child: content),
      child: content,
    );
  }

  Future<void> _completeTask(
    List<MapEntry<CalendarOccurrenceKey, CalendarOccurrenceDto>> group,
  ) async {
    if (group.isEmpty) {
      return;
    }
    final occurrence = group.first.value;
    final tasks = await ref.read(tasksProvider(occurrence.task.listId).future);
    if (!mounted) {
      return;
    }
    if (hasIncompleteDescendants(occurrence.task.id, tasks)) {
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
    if (MediaQuery.disableAnimationsOf(context)) {
      await _setTaskStatus(occurrence, 'done');
      return;
    }
    for (final entry in group) {
      _completionRetentionController.retain(entry.key);
    }
    setState(() {
      for (final entry in group) {
        _retainedOccurrences[entry.key] = entry.value;
      }
    });
    try {
      await _setTaskStatus(occurrence, 'done');
    } catch (error) {
      for (final entry in group) {
        _completionRetentionController.cancel(entry.key);
      }
      if (mounted) {
        setState(() {
          for (final entry in group) {
            _retainedOccurrences.remove(entry.key);
          }
        });
      }
      rethrow;
    }
  }

  Future<void> _reopenTask(CalendarOccurrenceDto occurrence) {
    return _setTaskStatus(occurrence, 'todo');
  }

  Future<void> _setTaskStatus(
    CalendarOccurrenceDto occurrence,
    String status,
  ) async {
    await ref
        .read(tasksProvider(occurrence.task.listId).notifier)
        .setStatus(occurrence.task.id, status);
    if (status == 'done' && mounted) {
      await _showLatestUndoSnackBar(context);
    }
  }

  Future<void> _showMoveMenu(CalendarOccurrenceDto occurrence) async {
    final l10n = AppLocalizations.of(context)!;
    final selection = await showModalBottomSheet<_MoveDateChoice>(
      context: context,
      useRootNavigator: true,
      showDragHandle: true,
      builder: (context) => SafeArea(
        child: Padding(
          padding: const EdgeInsets.only(bottom: AppSpacing.sm),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Padding(
                padding: const EdgeInsets.fromLTRB(
                  AppSpacing.lg,
                  AppSpacing.sm,
                  AppSpacing.lg,
                  AppSpacing.sm,
                ),
                child: Text(
                  l10n.calendarMoveSheetTitle,
                  style: Theme.of(context).textTheme.titleMedium,
                ),
              ),
              ListTile(
                title: Text(l10n.calendarMoveToToday),
                onTap: () => Navigator.pop(context, _MoveDateChoice.today),
              ),
              ListTile(
                title: Text(l10n.calendarMoveToTomorrow),
                onTap: () => Navigator.pop(context, _MoveDateChoice.tomorrow),
              ),
              ListTile(
                title: Text(l10n.calendarPickDate),
                onTap: () => Navigator.pop(context, _MoveDateChoice.pick),
              ),
            ],
          ),
        ),
      ),
    );
    if (!mounted || selection == null) {
      return;
    }
    final today = _dateOnly(DateTime.now());
    final target = switch (selection) {
      _MoveDateChoice.today => today,
      _MoveDateChoice.tomorrow => DateTime(
        today.year,
        today.month,
        today.day + 1,
      ),
      _MoveDateChoice.pick => await showDatePicker(
        context: context,
        initialDate: _occurrenceDate(occurrence.kind),
        firstDate: DateTime(2000),
        lastDate: DateTime(2100),
      ),
    };
    if (!mounted || target == null) {
      return;
    }
    await _moveOccurrence(occurrence, target);
  }

  Future<void> _moveOccurrence(
    CalendarOccurrenceDto occurrence,
    DateTime targetDate,
  ) {
    final weekStart =
        ref.read(calendarWeekStartProvider).value ?? defaultCalendarWeekStart;
    return ref
        .read(
          calendarOccurrencesProvider(
            _calendarRange(context, weekStart),
          ).notifier,
        )
        .moveOccurrence(occurrence: occurrence, targetDate: targetDate);
  }

  CalendarRange _calendarRange(BuildContext context, String weekStart) {
    if (_viewMode == CalendarViewMode.week) {
      final start = _weekStart(context, _anchorDay, weekStart);
      return CalendarRange.local(
        start: start,
        end: localCivilDay(start, dayOffset: 7),
      );
    }
    final monthStart = DateTime(_anchorDay.year, _anchorDay.month);
    final start = _weekStart(context, monthStart, weekStart);
    return CalendarRange.local(
      start: start,
      end: localCivilDay(start, dayOffset: 42),
    );
  }

  DateTime _weekStart(BuildContext context, DateTime day, String weekStart) {
    final firstDay = switch (weekStart) {
      mondayCalendarWeekStart => DateTime.monday,
      sundayCalendarWeekStart => DateTime.sunday % 7,
      _ => MaterialLocalizations.of(context).firstDayOfWeekIndex,
    };
    final weekday = day.weekday % 7;
    final offset = (weekday - firstDay + 7) % 7;
    return localCivilDay(day, dayOffset: -offset);
  }

  void _selectDay(DateTime day) {
    setState(() {
      _selectedDay = _dateOnly(day);
      if (_viewMode == CalendarViewMode.month &&
          (_selectedDay.year != _anchorDay.year ||
              _selectedDay.month != _anchorDay.month)) {
        _anchorDay = _selectedDay;
      }
    });
  }

  void _setViewMode(CalendarViewMode mode) {
    if (_viewMode == mode) {
      return;
    }
    setState(() {
      _viewMode = mode;
      _anchorDay = _selectedDay;
    });
  }

  void _movePeriod(int direction) {
    setState(() {
      if (_viewMode == CalendarViewMode.week) {
        _anchorDay = DateTime(
          _anchorDay.year,
          _anchorDay.month,
          _anchorDay.day + (7 * direction),
        );
      } else {
        _anchorDay = DateTime(_anchorDay.year, _anchorDay.month + direction);
      }
      _selectedDay = _anchorDay;
    });
  }

  void _goToToday() {
    final today = _dateOnly(DateTime.now());
    setState(() {
      _selectedDay = today;
      _anchorDay = today;
    });
  }

  int _activeCountForDay(
    List<CalendarOccurrenceDto> occurrences,
    DateTime day,
  ) {
    return occurrences
        .where((item) => !_isCompletedOccurrence(item))
        .where((item) => _sameDay(_occurrenceDate(item.kind), day))
        .length;
  }
}

class _CalendarHeaderButton extends StatelessWidget {
  const _CalendarHeaderButton({
    required this.tooltip,
    required this.onPressed,
    this.icon,
    this.label,
  });

  final String tooltip;
  final VoidCallback onPressed;
  final IconData? icon;
  final String? label;

  @override
  Widget build(BuildContext context) {
    return SizedBox(
      height: 48,
      child: icon != null
          ? IconButton(
              tooltip: tooltip,
              onPressed: onPressed,
              icon: Icon(icon, size: 19),
            )
          : TextButton(onPressed: onPressed, child: Text(label!, maxLines: 1)),
    );
  }
}

class _CalendarModeButton extends StatelessWidget {
  const _CalendarModeButton({
    super.key,
    required this.label,
    required this.selected,
    required this.onTap,
  });

  final String label;
  final bool selected;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final color = selected ? AppColors.forest : AppColors.muted;
    return Semantics(
      button: true,
      selected: selected,
      child: InkWell(
        onTap: onTap,
        child: ConstrainedBox(
          constraints: const BoxConstraints(minHeight: 48),
          child: Padding(
            padding: const EdgeInsets.symmetric(vertical: AppSpacing.xs),
            child: Column(
              mainAxisSize: MainAxisSize.min,
              mainAxisAlignment: MainAxisAlignment.end,
              children: [
                Text(
                  label,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  softWrap: false,
                  style: Theme.of(context).textTheme.labelLarge?.copyWith(
                    color: color,
                    fontWeight: selected ? FontWeight.w700 : FontWeight.w500,
                  ),
                ),
                const SizedBox(height: AppSpacing.sm),
                AnimatedContainer(
                  duration: const Duration(milliseconds: 180),
                  width: selected ? 36 : 0,
                  height: 2,
                  color: AppColors.forest,
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

class _CalendarDayTarget extends StatelessWidget {
  const _CalendarDayTarget({
    super.key,
    required this.date,
    required this.selected,
    required this.isToday,
    required this.count,
    required this.weekdayLabel,
    required this.onSelected,
    required this.onMove,
  });

  final DateTime date;
  final bool selected;
  final bool isToday;
  final int count;
  final String weekdayLabel;
  final VoidCallback onSelected;
  final Future<void> Function(CalendarOccurrenceDto, DateTime) onMove;

  @override
  Widget build(BuildContext context) {
    final l10n = AppLocalizations.of(context)!;
    final dateLabel = DateFormat.yMMMMEEEEd(
      Localizations.localeOf(context).toLanguageTag(),
    ).format(date);
    return DragTarget<CalendarOccurrenceDto>(
      onWillAcceptWithDetails: (details) =>
          !_isCompletedOccurrence(details.data),
      onAcceptWithDetails: (details) => unawaited(onMove(details.data, date)),
      builder: (context, candidates, rejected) {
        final highlighted = candidates.isNotEmpty;
        return Semantics(
          button: true,
          selected: selected,
          label: '$dateLabel, ${l10n.calendarDayTaskCount(count)}',
          child: InkWell(
            onTap: onSelected,
            child: ConstrainedBox(
              constraints: const BoxConstraints(minHeight: 68),
              child: DecoratedBox(
                decoration: BoxDecoration(
                  border: Border(
                    bottom: BorderSide(
                      color: selected || highlighted
                          ? AppColors.forest
                          : AppColors.hairline,
                      width: selected || highlighted ? 2 : 0.7,
                    ),
                  ),
                ),
                child: Column(
                  mainAxisAlignment: MainAxisAlignment.center,
                  children: [
                    Text(
                      weekdayLabel,
                      maxLines: 1,
                      style: Theme.of(
                        context,
                      ).textTheme.labelSmall?.copyWith(color: AppColors.muted),
                    ),
                    const SizedBox(height: AppSpacing.xs),
                    Text(
                      '${date.day}',
                      style: Theme.of(context).textTheme.titleSmall?.copyWith(
                        color: selected || isToday
                            ? AppColors.forest
                            : AppColors.ink,
                        fontWeight: selected || isToday
                            ? FontWeight.w700
                            : FontWeight.w500,
                      ),
                    ),
                    const SizedBox(height: 3),
                    _CalendarCountMark(count: count),
                  ],
                ),
              ),
            ),
          ),
        );
      },
    );
  }
}

class _CalendarMonthDayTarget extends StatelessWidget {
  const _CalendarMonthDayTarget({
    super.key,
    required this.date,
    required this.inMonth,
    required this.selected,
    required this.isToday,
    required this.count,
    required this.onSelected,
    required this.onMove,
  });

  final DateTime date;
  final bool inMonth;
  final bool selected;
  final bool isToday;
  final int count;
  final VoidCallback onSelected;
  final Future<void> Function(CalendarOccurrenceDto, DateTime) onMove;

  @override
  Widget build(BuildContext context) {
    final l10n = AppLocalizations.of(context)!;
    final dateLabel = DateFormat.yMMMMd(
      Localizations.localeOf(context).toLanguageTag(),
    ).format(date);
    return DragTarget<CalendarOccurrenceDto>(
      onWillAcceptWithDetails: (details) =>
          !_isCompletedOccurrence(details.data),
      onAcceptWithDetails: (details) => unawaited(onMove(details.data, date)),
      builder: (context, candidates, rejected) {
        final highlighted = candidates.isNotEmpty;
        return Semantics(
          button: true,
          selected: selected,
          label: '$dateLabel, ${l10n.calendarDayTaskCount(count)}',
          child: InkWell(
            onTap: onSelected,
            child: DecoratedBox(
              decoration: BoxDecoration(
                color: highlighted
                    ? AppColors.forest.withValues(alpha: 0.08)
                    : Colors.transparent,
                border: Border(
                  bottom: BorderSide(
                    color: selected ? AppColors.forest : AppColors.hairline,
                    width: selected ? 2 : 0.7,
                  ),
                ),
              ),
              child: Column(
                mainAxisAlignment: MainAxisAlignment.center,
                children: [
                  Text(
                    '${date.day}',
                    style: Theme.of(context).textTheme.labelLarge?.copyWith(
                      color: !inMonth
                          ? AppColors.muted.withValues(alpha: 0.5)
                          : selected || isToday
                          ? AppColors.forest
                          : AppColors.ink,
                      fontWeight: selected || isToday
                          ? FontWeight.w700
                          : FontWeight.w500,
                    ),
                  ),
                  const SizedBox(height: 4),
                  _CalendarCountMark(count: count),
                ],
              ),
            ),
          ),
        );
      },
    );
  }
}

class _CalendarCountMark extends StatelessWidget {
  const _CalendarCountMark({required this.count});

  final int count;

  @override
  Widget build(BuildContext context) {
    if (count == 0) {
      return const SizedBox(height: 4);
    }
    return Container(
      width: count > 1 ? 12 : 4,
      height: 4,
      decoration: BoxDecoration(
        color: AppColors.forest.withValues(alpha: 0.72),
        borderRadius: BorderRadius.circular(999),
      ),
    );
  }
}

class _CalendarCompletedDisclosure extends StatelessWidget {
  const _CalendarCompletedDisclosure({
    required this.count,
    required this.expanded,
    required this.onTap,
  });

  final int count;
  final bool expanded;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final l10n = AppLocalizations.of(context)!;
    final tooltip = expanded
        ? l10n.calendarHideCompletedTooltip
        : l10n.calendarShowCompletedTooltip;
    return Semantics(
      button: true,
      expanded: expanded,
      label: tooltip,
      child: InkWell(
        key: const ValueKey('calendar-completed-toggle'),
        onTap: onTap,
        child: ConstrainedBox(
          constraints: const BoxConstraints(minHeight: 48),
          child: Row(
            children: [
              const Icon(
                LucideIcons.circleCheck300,
                size: 16,
                color: AppColors.muted,
              ),
              const SizedBox(width: AppSpacing.sm),
              Expanded(
                child: Text(
                  l10n.calendarCompletedTitle,
                  style: Theme.of(context).textTheme.labelMedium?.copyWith(
                    color: AppColors.muted,
                    fontWeight: FontWeight.w600,
                  ),
                ),
              ),
              Text(
                '$count',
                style: Theme.of(
                  context,
                ).textTheme.labelMedium?.copyWith(color: AppColors.muted),
              ),
              const SizedBox(width: AppSpacing.xs),
              AnimatedRotation(
                turns: expanded ? 0.5 : 0,
                duration: MediaQuery.disableAnimationsOf(context)
                    ? Duration.zero
                    : const Duration(milliseconds: 180),
                child: const Icon(
                  LucideIcons.chevronDown300,
                  size: 16,
                  color: AppColors.muted,
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

class _CalendarDragFeedback extends StatelessWidget {
  const _CalendarDragFeedback({required this.title, required this.detail});

  final String title;
  final String detail;

  @override
  Widget build(BuildContext context) {
    return Material(
      color: AppColors.canvas,
      elevation: 2,
      child: Container(
        width: 260,
        padding: const EdgeInsets.all(AppSpacing.md),
        decoration: const BoxDecoration(
          border: Border(bottom: BorderSide(color: AppColors.hairline)),
        ),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(title, style: Theme.of(context).textTheme.titleSmall),
            const SizedBox(height: AppSpacing.xs),
            Text(
              detail,
              style: Theme.of(
                context,
              ).textTheme.labelMedium?.copyWith(color: AppColors.muted),
            ),
          ],
        ),
      ),
    );
  }
}

enum _MoveDateChoice { today, tomorrow, pick }

class _CalendarTreeContext {
  const _CalendarTreeContext({
    this.depth = 0,
    this.parentTaskName,
    this.isLastSibling = true,
    this.ancestorLineContinuations = const <bool>[],
  });

  final int depth;
  final String? parentTaskName;
  final bool isLastSibling;
  final List<bool> ancestorLineContinuations;
}

Map<String, _CalendarTreeContext> _calendarTreeContexts(
  List<CalendarOccurrenceDto> occurrences,
) {
  final taskById = <String, TaskDto>{};
  final order = <String>[];
  for (final occurrence in occurrences) {
    if (!taskById.containsKey(occurrence.task.id)) {
      order.add(occurrence.task.id);
      taskById[occurrence.task.id] = occurrence.task;
    }
  }
  final siblingIdsByParent = <String?, List<String>>{};
  for (final taskId in order) {
    final task = taskById[taskId]!;
    final effectiveParent = taskById.containsKey(task.parentTaskId)
        ? task.parentTaskId
        : null;
    siblingIdsByParent.putIfAbsent(effectiveParent, () => []).add(taskId);
  }

  List<TaskDto> ancestorChain(TaskDto task) {
    final reversed = <TaskDto>[];
    final seen = <String>{task.id};
    var parentId = task.parentTaskId;
    while (parentId != null &&
        taskById.containsKey(parentId) &&
        seen.add(parentId) &&
        reversed.length < 4) {
      final parent = taskById[parentId]!;
      reversed.add(parent);
      parentId = parent.parentTaskId;
    }
    return reversed.reversed.toList(growable: false);
  }

  bool isLast(TaskDto task) {
    final effectiveParent = taskById.containsKey(task.parentTaskId)
        ? task.parentTaskId
        : null;
    final siblings = siblingIdsByParent[effectiveParent] ?? const <String>[];
    return siblings.isEmpty || siblings.last == task.id;
  }

  return {
    for (final task in taskById.values)
      task.id: (() {
        final ancestors = ancestorChain(task);
        return _CalendarTreeContext(
          depth: ancestors.length,
          parentTaskName: ancestors.isEmpty ? null : ancestors.last.title,
          isLastSibling: isLast(task),
          ancestorLineContinuations: [
            for (final ancestor in ancestors.take(
              ancestors.isEmpty ? 0 : ancestors.length - 1,
            ))
              !isLast(ancestor),
          ],
        );
      })(),
  };
}

DateTime _dateOnly(DateTime value) {
  return localCivilDay(value);
}

bool _sameDay(DateTime a, DateTime b) =>
    a.year == b.year && a.month == b.month && a.day == b.day;

String _civilDate(DateTime value) =>
    '${value.year.toString().padLeft(4, '0')}-'
    '${value.month.toString().padLeft(2, '0')}-'
    '${value.day.toString().padLeft(2, '0')}';

DateTime _civilDateValue(String value) {
  final parts = value.split('-').map(int.parse).toList(growable: false);
  return DateTime(parts[0], parts[1], parts[2]);
}

DateTime _occurrenceDate(CalendarOccurrenceKindDto kind) {
  return kind.when(
    dateDue: (dueOn) => _civilDateValue(dueOn),
    dateTimeDue: (dueAt, timeZone) => dueAt.toLocal(),
    scheduled: (scheduledAt) => scheduledAt.toLocal(),
    completed: (completedAt) => completedAt.toLocal(),
  );
}

String _occurrenceKindToken(CalendarOccurrenceKindDto kind) {
  return kind.when(
    dateDue: (_) => 'date_due',
    dateTimeDue: (_, _) => 'datetime_due',
    scheduled: (_) => 'scheduled',
    completed: (_) => 'completed',
  );
}

String _occurrenceMarker(CalendarOccurrenceKindDto kind) {
  return kind.when(
    dateDue: (dueOn) => dueOn,
    dateTimeDue: (dueAt, timeZone) => dueAt.toUtc().toIso8601String(),
    scheduled: (scheduledAt) => scheduledAt.toUtc().toIso8601String(),
    completed: (completedAt) => completedAt.toUtc().toIso8601String(),
  );
}

bool _isCompletedOccurrence(CalendarOccurrenceDto occurrence) =>
    occurrence.kind is CalendarOccurrenceKindDto_Completed;

String _occurrenceKindLabel(
  AppLocalizations l10n,
  CalendarOccurrenceKindDto kind,
) {
  return switch (kind) {
    CalendarOccurrenceKindDto_DateDue() ||
    CalendarOccurrenceKindDto_DateTimeDue() => l10n.calendarDueKind,
    CalendarOccurrenceKindDto_Scheduled() => l10n.calendarScheduledKind,
    CalendarOccurrenceKindDto_Completed() => l10n.calendarCompletedKind,
  };
}

String _occurrenceTimeLabel(
  BuildContext context,
  CalendarOccurrenceKindDto kind,
) {
  final locale = Localizations.localeOf(context).toLanguageTag();
  return kind.when(
    dateDue: (_) => '',
    dateTimeDue: (dueAt, timeZone) {
      final savedZoneDate = taskDueDisplayDate(
        TaskDueDto.dateTime(dueAt: dueAt, timeZone: timeZone),
      );
      return '${DateFormat.jm(locale).format(savedZoneDate)} '
          '$timeZone (${taskDueUtcOffsetLabel(savedZoneDate)})';
    },
    scheduled: (scheduledAt) =>
        DateFormat.jm(locale).format(scheduledAt.toLocal()),
    completed: (completedAt) =>
        DateFormat.jm(locale).format(completedAt.toLocal()),
  );
}

HomeDueDateTone _occurrenceDueTone(CalendarOccurrenceDto occurrence) {
  final dueKind =
      occurrence.kind is CalendarOccurrenceKindDto_DateDue ||
      occurrence.kind is CalendarOccurrenceKindDto_DateTimeDue;
  if (dueKind &&
      _occurrenceDate(occurrence.kind).isBefore(_dateOnly(DateTime.now()))) {
    return HomeDueDateTone.overdue;
  }
  return HomeDueDateTone.future;
}

int _compareOccurrences(CalendarOccurrenceDto a, CalendarOccurrenceDto b) {
  final dateComparison = _occurrenceDate(
    a.kind,
  ).compareTo(_occurrenceDate(b.kind));
  if (dateComparison != 0) {
    return dateComparison;
  }
  final kindComparison = _occurrenceKindToken(
    a.kind,
  ).compareTo(_occurrenceKindToken(b.kind));
  if (kindComparison != 0) {
    return kindComparison;
  }
  return a.task.title.compareTo(b.task.title);
}

String _weekRangeLabel(String locale, DateTime start, DateTime end) {
  if (start.month == end.month) {
    return '${DateFormat.MMMd(locale).format(start)}–${end.day}';
  }
  return '${DateFormat.MMMd(locale).format(start)}–'
      '${DateFormat.MMMd(locale).format(end)}';
}

CalendarOccurrenceDto _completedSnapshot(CalendarOccurrenceDto occurrence) {
  final task = occurrence.task;
  return CalendarOccurrenceDto(
    task: TaskDto(
      id: task.id,
      listId: task.listId,
      parentTaskId: task.parentTaskId,
      title: task.title,
      note: task.note,
      status: 'done',
      priority: task.priority,
      due: task.due,
      scheduledAt: task.scheduledAt,
      estimatedMinutes: task.estimatedMinutes,
      sortOrder: task.sortOrder,
      completedAt: task.completedAt ?? DateTime.now().millisecondsSinceEpoch,
      closedReason: task.closedReason,
      deletedAt: task.deletedAt,
      assignee: task.assignee,
      createdAt: task.createdAt,
      updatedAt: task.updatedAt,
    ),
    listName: occurrence.listName,
    listArchived: occurrence.listArchived,
    kind: occurrence.kind,
  );
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
      behavior: SnackBarBehavior.floating,
      content: Text(l10n.undoCompleteMessage),
      margin: const EdgeInsets.all(AppSpacing.md),
      action: SnackBarAction(
        label: l10n.undoActionLabel,
        onPressed: () {
          messenger.hideCurrentSnackBar();
          unawaited(
            (() async {
              try {
                await container
                    .read(latestTaskUndoProvider.notifier)
                    .undo(undo.id);
              } catch (error) {
                if (!context.mounted) return;
                messenger.showSnackBar(
                  SnackBar(
                    content: Text(
                      l10n.undoFailedMessage(bridgeErrorMessage(l10n, error)),
                    ),
                  ),
                );
              }
            })(),
          );
        },
      ),
    ),
  );
}
