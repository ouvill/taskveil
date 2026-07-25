part of '../task_components.dart';

typedef TaskCreateCallback =
    Future<void> Function({
      required String listId,
      required String title,
      required String note,
      required TaskDueDto? due,
      required int priority,
      required int? scheduledAt,
      required int? estimatedMinutes,
    });

/// Planned start and effort estimate edited together by the production Plan
/// sheet. The two fields intentionally stay independent from Due/Reminder.
class TaskPlanValue {
  const TaskPlanValue({this.scheduledAt, this.estimatedMinutes});

  final int? scheduledAt;
  final int? estimatedMinutes;

  bool get isEmpty => scheduledAt == null && estimatedMinutes == null;
}

/// A priority sheet result. The wrapper lets `0` (None) remain distinct from
/// dismissing the sheet, whose result is null.
class TaskPrioritySelection {
  const TaskPrioritySelection(this.value);

  final int value;
}

Future<void> showTaskCreateSheet({
  required BuildContext context,
  required List<ListDto> listOptions,
  required String initialListId,
  required TaskDueDto? initialDue,
  required String errorMessage,
  required TaskCreateCallback onCreate,
}) {
  return showModalBottomSheet<void>(
    context: context,
    useRootNavigator: true,
    isScrollControlled: true,
    useSafeArea: true,
    barrierColor: Theme.of(context).colorScheme.scrim.withValues(alpha: 0.24),
    backgroundColor: Colors.transparent,
    builder: (context) => TaskCreateSheet(
      listOptions: listOptions,
      initialListId: initialListId,
      initialDue: initialDue,
      errorMessage: errorMessage,
      onCreate: onCreate,
    ),
  );
}

class CircularCaptureAction extends StatelessWidget {
  const CircularCaptureAction({
    super.key,
    required this.listOptions,
    required this.initialListId,
    required this.initialDue,
    required this.errorMessage,
    required this.onCreate,
    this.size = 52,
  });

  final List<ListDto> listOptions;
  final String? initialListId;
  final TaskDueDto? initialDue;
  final String errorMessage;
  final TaskCreateCallback onCreate;
  final double size;

  @override
  Widget build(BuildContext context) {
    final l10n = AppLocalizations.of(context)!;
    final colorScheme = Theme.of(context).colorScheme;
    final enabled =
        initialListId != null &&
        listOptions.any((list) => list.id == initialListId);
    return Tooltip(
      message: l10n.quickAddOpenTooltip,
      child: Semantics(
        button: true,
        enabled: enabled,
        label: l10n.quickAddOpenSemantics,
        child: Material(
          color: enabled
              ? colorScheme.primary
              : colorScheme.surfaceContainerHighest,
          elevation: 2,
          shadowColor: colorScheme.shadow.withValues(alpha: 0.14),
          shape: const CircleBorder(),
          child: InkWell(
            key: const ValueKey('quick-add-open'),
            customBorder: const CircleBorder(),
            onTap: !enabled
                ? null
                : () => showTaskCreateSheet(
                    context: context,
                    listOptions: listOptions,
                    initialListId: initialListId!,
                    initialDue: initialDue,
                    errorMessage: errorMessage,
                    onCreate: onCreate,
                  ),
            child: SizedBox.square(
              dimension: size,
              child: Icon(
                LucideIcons.plus300,
                size: 20,
                color: enabled
                    ? colorScheme.onPrimary
                    : colorScheme.onSurfaceVariant,
              ),
            ),
          ),
        ),
      ),
    );
  }
}

class QuickAddBar extends StatefulWidget {
  const QuickAddBar({
    super.key,
    required this.listOptions,
    required this.initialListId,
    required this.initialDue,
    required this.errorMessage,
    required this.onCreate,
  });

  final List<ListDto> listOptions;
  final String? initialListId;
  final TaskDueDto? initialDue;
  final String errorMessage;
  final Future<void> Function({
    required String listId,
    required String title,
    required String note,
    required TaskDueDto? due,
    required int priority,
    required int? scheduledAt,
    required int? estimatedMinutes,
  })
  onCreate;

  @override
  State<QuickAddBar> createState() => _QuickAddBarState();
}

class _QuickAddBarState extends State<QuickAddBar> {
  @override
  Widget build(BuildContext context) {
    final viewInsets = MediaQuery.viewInsetsOf(context);
    return AnimatedPadding(
      duration: const Duration(milliseconds: 180),
      curve: Curves.easeOutCubic,
      padding: EdgeInsets.only(bottom: viewInsets.bottom),
      child: CircularCaptureAction(
        listOptions: widget.listOptions,
        initialListId: widget.initialListId,
        initialDue: widget.initialDue,
        errorMessage: widget.errorMessage,
        onCreate: widget.onCreate,
      ),
    );
  }
}

class TaskCreateSheet extends StatefulWidget {
  const TaskCreateSheet({
    super.key,
    required this.listOptions,
    required this.initialListId,
    required this.initialDue,
    required this.errorMessage,
    required this.onCreate,
  });

  final List<ListDto> listOptions;
  final String initialListId;
  final TaskDueDto? initialDue;
  final String errorMessage;
  final Future<void> Function({
    required String listId,
    required String title,
    required String note,
    required TaskDueDto? due,
    required int priority,
    required int? scheduledAt,
    required int? estimatedMinutes,
  })
  onCreate;

  @override
  State<TaskCreateSheet> createState() => _TaskCreateSheetState();
}

class _TaskCreateSheetState extends State<TaskCreateSheet> {
  late String _selectedListId;
  late TaskDueDto? _due;
  int _priority = 0;
  int? _scheduledAt;
  int? _estimatedMinutes;
  final TextEditingController _titleController = TextEditingController();
  final TextEditingController _noteController = TextEditingController();
  final FocusNode _titleFocusNode = FocusNode();
  bool _submitting = false;

  bool get _hasComposingRange {
    final range = _titleController.value.composing;
    return range.isValid && !range.isCollapsed;
  }

  bool get _canSubmit =>
      !_submitting &&
      !_hasComposingRange &&
      _titleController.text.trim().isNotEmpty;

  @override
  void initState() {
    super.initState();
    _selectedListId = widget.initialListId;
    _due = widget.initialDue;
    _titleController.addListener(_onTitleChanged);
  }

  @override
  void dispose() {
    _titleController
      ..removeListener(_onTitleChanged)
      ..dispose();
    _noteController.dispose();
    _titleFocusNode.dispose();
    super.dispose();
  }

  void _onTitleChanged() {
    setState(() {});
  }

  Future<void> _submit() async {
    if (!_canSubmit) {
      return;
    }
    setState(() => _submitting = true);
    try {
      await widget.onCreate(
        listId: _selectedListId,
        title: _titleController.text.trim(),
        note: _noteController.text.trim(),
        due: _due,
        priority: _priority,
        scheduledAt: _scheduledAt,
        estimatedMinutes: _estimatedMinutes,
      );
      if (!mounted) {
        return;
      }
      _titleController.clear();
      _noteController.clear();
      _titleFocusNode.requestFocus();
    } catch (_) {
      if (!mounted) {
        return;
      }
      ScaffoldMessenger.of(context)
        ..hideCurrentSnackBar()
        ..showSnackBar(SnackBar(content: Text(widget.errorMessage)));
      _titleFocusNode.requestFocus();
    } finally {
      if (mounted) {
        setState(() => _submitting = false);
      }
    }
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final colorScheme = theme.colorScheme;
    final l10n = AppLocalizations.of(context)!;
    final viewInsets = MediaQuery.viewInsetsOf(context);
    final selectedList = widget.listOptions.firstWhere(
      (list) => list.id == _selectedListId,
    );
    return AnimatedPadding(
      duration: const Duration(milliseconds: 180),
      curve: Curves.easeOutCubic,
      padding: EdgeInsets.only(bottom: viewInsets.bottom),
      child: DecoratedBox(
        decoration: BoxDecoration(
          color: colorScheme.surface,
          borderRadius: const BorderRadius.vertical(top: Radius.circular(12)),
          border: Border.all(color: colorScheme.outlineVariant),
          boxShadow: [
            BoxShadow(
              color: colorScheme.shadow.withValues(alpha: 0.10),
              blurRadius: 24,
              offset: const Offset(0, -8),
            ),
          ],
        ),
        child: SafeArea(
          top: false,
          child: ConstrainedBox(
            constraints: BoxConstraints(
              maxHeight: MediaQuery.sizeOf(context).height * 0.86,
            ),
            child: SingleChildScrollView(
              padding: const EdgeInsets.fromLTRB(
                AppSpacing.lg,
                AppSpacing.sm,
                AppSpacing.lg,
                AppSpacing.lg,
              ),
              child: Column(
                mainAxisSize: MainAxisSize.min,
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  Center(
                    child: DecoratedBox(
                      decoration: BoxDecoration(
                        color: colorScheme.primary.withValues(alpha: 0.22),
                        borderRadius: BorderRadius.circular(999),
                      ),
                      child: const SizedBox(width: 38, height: 4),
                    ),
                  ),
                  const SizedBox(height: AppSpacing.lg),
                  TextField(
                    key: const ValueKey('task-create-title-field'),
                    controller: _titleController,
                    focusNode: _titleFocusNode,
                    autofocus: true,
                    readOnly: _submitting,
                    textInputAction: TextInputAction.done,
                    minLines: 1,
                    maxLines: 2,
                    onEditingComplete: () {},
                    onSubmitted: (_) => unawaited(_submit()),
                    decoration: InputDecoration(
                      hintText: l10n.taskCreateTitleHint,
                      border: InputBorder.none,
                      enabledBorder: InputBorder.none,
                      focusedBorder: InputBorder.none,
                      disabledBorder: InputBorder.none,
                      filled: false,
                      isDense: true,
                      contentPadding: EdgeInsets.zero,
                    ),
                    style: theme.textTheme.headlineSmall?.copyWith(
                      color: colorScheme.onSurface,
                      fontWeight: FontWeight.w400,
                      height: 1.1,
                    ),
                  ),
                  const SizedBox(height: AppSpacing.sm),
                  TextField(
                    key: const ValueKey('task-create-note-field'),
                    controller: _noteController,
                    readOnly: _submitting,
                    minLines: 1,
                    maxLines: 3,
                    decoration: InputDecoration(
                      hintText: l10n.noteLabel,
                      border: InputBorder.none,
                      enabledBorder: InputBorder.none,
                      focusedBorder: InputBorder.none,
                      disabledBorder: InputBorder.none,
                      filled: false,
                      isDense: true,
                      contentPadding: EdgeInsets.zero,
                    ),
                    style: theme.textTheme.bodyLarge?.copyWith(
                      color: colorScheme.onSurfaceVariant,
                      height: 1.25,
                    ),
                  ),
                  const SizedBox(height: AppSpacing.md),
                  Column(
                    children: [
                      _TaskCreateListProperty(
                        selectedList: selectedList,
                        listOptions: widget.listOptions,
                        onSelected: _submitting
                            ? null
                            : (listId) =>
                                  setState(() => _selectedListId = listId),
                      ),
                      _TaskCreateDueProperty(
                        due: _due,
                        onTap: _submitting ? null : _showDueOptions,
                      ),
                      _TaskCreatePropertyAction(
                        key: const ValueKey('task-create-plan-property-row'),
                        icon: LucideIcons.calendarClock300,
                        property: l10n.taskCreatePlanLabel,
                        value: formatTaskPlanValue(
                          l10n,
                          locale: Localizations.localeOf(
                            context,
                          ).toLanguageTag(),
                          scheduledAt: _scheduledAt,
                          estimatedMinutes: _estimatedMinutes,
                        ),
                        tooltip: l10n.taskCreatePlanTooltip,
                        onTap: _submitting ? null : _showPlanOptions,
                      ),
                      _TaskCreatePropertyAction(
                        key: const ValueKey(
                          'task-create-priority-property-row',
                        ),
                        icon: LucideIcons.flag300,
                        property: l10n.priorityLabel,
                        value: taskPriorityLabel(l10n, _priority),
                        tooltip: l10n.taskCreatePriorityTooltip,
                        marker: _priority == 0
                            ? null
                            : PriorityDot(priority: _priority, isMuted: false),
                        onTap: _submitting ? null : _showPriorityOptions,
                      ),
                    ],
                  ),
                  const SizedBox(height: AppSpacing.md),
                  FilledButton.icon(
                    key: const ValueKey('task-create-submit'),
                    onPressed: _canSubmit ? () => unawaited(_submit()) : null,
                    icon: _submitting
                        ? SizedBox(
                            width: 18,
                            height: 18,
                            child: CircularProgressIndicator(
                              strokeWidth: 2,
                              color: colorScheme.onPrimary,
                            ),
                          )
                        : const Icon(LucideIcons.plus300),
                    label: Text(l10n.addTaskButton),
                    style: FilledButton.styleFrom(
                      minimumSize: const Size.fromHeight(50),
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

  Future<void> _showDueOptions() async {
    final selection = await showModalBottomSheet<_TaskCreateDueSelection>(
      context: context,
      useRootNavigator: true,
      isScrollControlled: true,
      showDragHandle: true,
      builder: (context) => const _TaskCreateDueSheet(),
    );
    if (!mounted || selection == null) {
      return;
    }
    switch (selection) {
      case _TaskCreateDueSelection.today:
        setState(() => _due = dateOnlyDue(DateTime.now()));
        break;
      case _TaskCreateDueSelection.tomorrow:
        setState(
          () => _due = dateOnlyDue(DateTime.now().add(const Duration(days: 1))),
        );
        break;
      case _TaskCreateDueSelection.pickDate:
        final initialDate = _due == null
            ? DateTime.now()
            : taskDueDisplayDate(_due!);
        final picked = await showDatePicker(
          context: context,
          initialDate: initialDate,
          firstDate: DateTime(2000),
          lastDate: DateTime(2100),
        );
        if (!mounted || picked == null) {
          return;
        }
        setState(() => _due = dateOnlyDue(picked));
        break;
      case _TaskCreateDueSelection.pickDateTime:
        final initial = _due == null
            ? DateTime.now()
            : taskDueDisplayDate(_due!);
        final pickedDate = await showDatePicker(
          context: context,
          initialDate: initial,
          firstDate: DateTime(2000),
          lastDate: DateTime(2100),
        );
        if (!mounted || pickedDate == null) {
          return;
        }
        final pickedTime = await showTimePicker(
          context: context,
          initialTime: TimeOfDay.fromDateTime(initial),
        );
        if (!mounted || pickedTime == null) {
          return;
        }
        final localDateTime = DateTime(
          pickedDate.year,
          pickedDate.month,
          pickedDate.day,
          pickedTime.hour,
          pickedTime.minute,
        );
        if (localDateTime.year != pickedDate.year ||
            localDateTime.month != pickedDate.month ||
            localDateTime.day != pickedDate.day ||
            localDateTime.hour != pickedTime.hour ||
            localDateTime.minute != pickedTime.minute) {
          return;
        }
        final savedTimeZone = taskDueSavedTimeZone(_due);
        final timeZone =
            savedTimeZone ??
            await ProviderScope.containerOf(
              context,
              listen: false,
            ).read(bridgeServiceProvider).getLocalTimeZone();
        if (!mounted) {
          return;
        }
        TaskDueDto exactDue;
        try {
          exactDue = dateTimeDue(
            localDateTime: localDateTime,
            timeZone: timeZone,
          );
        } on FormatException {
          return;
        }
        setState(() => _due = exactDue);
        break;
      case _TaskCreateDueSelection.clear:
        setState(() => _due = null);
        break;
    }
    _titleFocusNode.requestFocus();
  }

  Future<void> _showPlanOptions() async {
    _titleFocusNode.unfocus();
    final value = await showTaskPlanSheet(
      context: context,
      initialValue: TaskPlanValue(
        scheduledAt: _scheduledAt,
        estimatedMinutes: _estimatedMinutes,
      ),
    );
    if (!mounted || value == null) return;
    setState(() {
      _scheduledAt = value.scheduledAt;
      _estimatedMinutes = value.estimatedMinutes;
    });
    _titleFocusNode.requestFocus();
  }

  Future<void> _showPriorityOptions() async {
    _titleFocusNode.unfocus();
    final selection = await showTaskPrioritySheet(
      context: context,
      selectedPriority: _priority,
    );
    if (!mounted || selection == null) return;
    setState(() => _priority = selection.value);
    _titleFocusNode.requestFocus();
  }
}

class _TaskCreateListProperty extends StatelessWidget {
  const _TaskCreateListProperty({
    required this.selectedList,
    required this.listOptions,
    required this.onSelected,
  });

  final ListDto selectedList;
  final List<ListDto> listOptions;
  final ValueChanged<String>? onSelected;

  @override
  Widget build(BuildContext context) {
    final l10n = AppLocalizations.of(context)!;
    return PopupMenuButton<String>(
      key: const ValueKey('task-create-list-chip'),
      tooltip: l10n.taskCreateListTooltip,
      enabled: onSelected != null,
      onSelected: onSelected,
      itemBuilder: (context) => [
        for (final list in listOptions)
          PopupMenuItem<String>(
            key: ValueKey('task-create-list-option-${list.id}'),
            value: list.id,
            child: Text(list.name),
          ),
      ],
      child: _TaskCreatePropertyRow(
        key: const ValueKey('task-create-list-property-row'),
        icon: LucideIcons.inbox300,
        property: l10n.taskCreateListChip,
        value: selectedList.name,
        enabled: onSelected != null,
      ),
    );
  }
}

class _TaskCreateDueProperty extends StatelessWidget {
  const _TaskCreateDueProperty({required this.due, required this.onTap});

  final TaskDueDto? due;
  final VoidCallback? onTap;

  @override
  Widget build(BuildContext context) {
    final l10n = AppLocalizations.of(context)!;
    final locale = Localizations.localeOf(context).toLanguageTag();
    final value = formatRelativeDueDate(l10n, locale, due);
    return Tooltip(
      message: l10n.taskCreateDueTooltip,
      child: Semantics(
        button: true,
        enabled: onTap != null,
        label: l10n.taskCreateDueChipSemantics(value),
        child: InkWell(
          key: const ValueKey('task-create-due-chip'),
          onTap: onTap,
          child: _TaskCreatePropertyRow(
            key: const ValueKey('task-create-due-property-row'),
            icon: LucideIcons.calendarDays300,
            property: l10n.taskCreateDueChip,
            value: value,
            enabled: onTap != null,
          ),
        ),
      ),
    );
  }
}

class _TaskCreatePropertyAction extends StatelessWidget {
  const _TaskCreatePropertyAction({
    super.key,
    required this.icon,
    required this.property,
    required this.value,
    required this.tooltip,
    required this.onTap,
    this.marker,
  });

  final IconData icon;
  final String property;
  final String value;
  final String tooltip;
  final VoidCallback? onTap;
  final Widget? marker;

  @override
  Widget build(BuildContext context) {
    return Tooltip(
      message: tooltip,
      child: Semantics(
        button: true,
        enabled: onTap != null,
        label: '$property, $value',
        excludeSemantics: true,
        child: Material(
          type: MaterialType.transparency,
          child: InkWell(
            onTap: onTap,
            child: _TaskCreatePropertyRow(
              icon: icon,
              property: property,
              value: value,
              enabled: onTap != null,
              marker: marker,
            ),
          ),
        ),
      ),
    );
  }
}

class _TaskCreatePropertyRow extends StatelessWidget {
  const _TaskCreatePropertyRow({
    super.key,
    required this.icon,
    required this.property,
    required this.value,
    required this.enabled,
    this.marker,
  });

  final IconData icon;
  final String property;
  final String value;
  final bool enabled;
  final Widget? marker;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final colorScheme = theme.colorScheme;
    return DecoratedBox(
      decoration: BoxDecoration(
        border: Border(
          bottom: BorderSide(color: colorScheme.outlineVariant, width: 0.7),
        ),
      ),
      child: ConstrainedBox(
        constraints: const BoxConstraints(minHeight: 52),
        child: Row(
          children: [
            SizedBox(
              width: 36,
              child: Icon(
                icon,
                size: 17,
                applyTextScaling: false,
                color: enabled
                    ? colorScheme.primary
                    : colorScheme.onSurfaceVariant,
              ),
            ),
            const SizedBox(width: AppSpacing.xs),
            Expanded(
              flex: 4,
              child: Text(
                property,
                style: theme.textTheme.labelMedium?.copyWith(
                  color: colorScheme.onSurfaceVariant,
                ),
              ),
            ),
            const SizedBox(width: AppSpacing.sm),
            if (marker != null) ...[
              ExcludeSemantics(child: marker!),
              const SizedBox(width: AppSpacing.xs),
            ],
            Flexible(
              flex: 5,
              child: Text(
                value,
                textAlign: TextAlign.end,
                softWrap: true,
                style: theme.textTheme.bodyMedium?.copyWith(
                  color: enabled
                      ? colorScheme.primary
                      : colorScheme.onSurfaceVariant,
                ),
              ),
            ),
            SizedBox(
              width: 40,
              child: Icon(
                LucideIcons.chevronRight300,
                size: 16,
                applyTextScaling: false,
                color: colorScheme.onSurfaceVariant,
              ),
            ),
          ],
        ),
      ),
    );
  }
}

class _TaskCreateDueSheet extends StatelessWidget {
  const _TaskCreateDueSheet();

  @override
  Widget build(BuildContext context) {
    final l10n = AppLocalizations.of(context)!;
    return SafeArea(
      child: ListView(
        shrinkWrap: true,
        padding: const EdgeInsets.only(bottom: AppSpacing.sm),
        children: [
          ListTile(title: Text(l10n.dueDateLabel)),
          ListTile(
            key: const ValueKey('task-create-due-today'),
            leading: const Icon(LucideIcons.calendarCheck300),
            title: Text(l10n.dueToday),
            onTap: () =>
                Navigator.of(context).pop(_TaskCreateDueSelection.today),
          ),
          ListTile(
            key: const ValueKey('task-create-due-tomorrow'),
            leading: const Icon(LucideIcons.calendarPlus300),
            title: Text(l10n.dueTomorrow),
            onTap: () =>
                Navigator.of(context).pop(_TaskCreateDueSelection.tomorrow),
          ),
          ListTile(
            key: const ValueKey('task-create-due-pick-date'),
            leading: const Icon(LucideIcons.calendarDays300),
            title: Text(l10n.setDueDateButton),
            onTap: () =>
                Navigator.of(context).pop(_TaskCreateDueSelection.pickDate),
          ),
          ListTile(
            key: const ValueKey('task-create-due-pick-date-time'),
            leading: const Icon(LucideIcons.calendarClock300),
            title: Text(l10n.setDueDateTimeButton),
            onTap: () =>
                Navigator.of(context).pop(_TaskCreateDueSelection.pickDateTime),
          ),
          ListTile(
            key: const ValueKey('task-create-due-clear'),
            leading: const Icon(LucideIcons.calendarX300),
            title: Text(l10n.clearDueDateButton),
            onTap: () =>
                Navigator.of(context).pop(_TaskCreateDueSelection.clear),
          ),
        ],
      ),
    );
  }
}

enum _TaskCreateDueSelection { today, tomorrow, pickDate, pickDateTime, clear }

String formatTaskPlanValue(
  AppLocalizations l10n, {
  required String locale,
  required int? scheduledAt,
  required int? estimatedMinutes,
}) {
  if (scheduledAt == null && estimatedMinutes == null) return l10n.planNotSet;
  final parts = <String>[];
  if (scheduledAt != null) {
    final date = DateTime.fromMillisecondsSinceEpoch(scheduledAt).toLocal();
    parts.add(
      '${DateFormat.MMMd(locale).format(date)} · ${DateFormat.jm(locale).format(date)}',
    );
  }
  if (estimatedMinutes != null) {
    parts.add(l10n.estimateMinutes(estimatedMinutes));
  }
  return parts.join(' · ');
}

Future<TaskPlanValue?> showTaskPlanSheet({
  required BuildContext context,
  required TaskPlanValue initialValue,
}) {
  return showModalBottomSheet<TaskPlanValue>(
    context: context,
    useRootNavigator: true,
    isScrollControlled: true,
    backgroundColor: Colors.transparent,
    barrierColor: Theme.of(context).colorScheme.scrim.withValues(alpha: 0.24),
    builder: (context) => TaskPlanSheet(initialValue: initialValue),
  );
}

class TaskPlanSheet extends StatefulWidget {
  const TaskPlanSheet({super.key, required this.initialValue});

  final TaskPlanValue initialValue;

  @override
  State<TaskPlanSheet> createState() => _TaskPlanSheetState();
}

class _TaskPlanSheetState extends State<TaskPlanSheet> {
  late int? _scheduledAt;
  late int? _estimatedMinutes;

  @override
  void initState() {
    super.initState();
    _scheduledAt = widget.initialValue.scheduledAt;
    _estimatedMinutes = widget.initialValue.estimatedMinutes;
  }

  Future<void> _pickStart() async {
    final initial = _scheduledAt == null
        ? DateTime.now().add(const Duration(hours: 1))
        : DateTime.fromMillisecondsSinceEpoch(_scheduledAt!).toLocal();
    final date = await showDatePicker(
      context: context,
      initialDate: initial,
      firstDate: DateTime(2000),
      lastDate: DateTime(2100),
    );
    if (date == null || !mounted) return;
    final time = await showTimePicker(
      context: context,
      initialTime: TimeOfDay.fromDateTime(initial),
    );
    if (time == null || !mounted) return;
    final local = DateTime(
      date.year,
      date.month,
      date.day,
      time.hour,
      time.minute,
    );
    setState(() => _scheduledAt = local.millisecondsSinceEpoch);
  }

  void _adjustEstimate(int delta) {
    final current = _estimatedMinutes ?? 0;
    final next = (current + delta).clamp(0, 24 * 60);
    setState(() => _estimatedMinutes = next == 0 ? null : next);
  }

  @override
  Widget build(BuildContext context) {
    final l10n = AppLocalizations.of(context)!;
    final theme = Theme.of(context);
    final colors = theme.colorScheme;
    final locale = Localizations.localeOf(context).toLanguageTag();
    final startLabel = _scheduledAt == null
        ? l10n.planNotSet
        : DateFormat.yMMMd(locale).add_jm().format(
            DateTime.fromMillisecondsSinceEpoch(_scheduledAt!).toLocal(),
          );
    return DecoratedBox(
      decoration: BoxDecoration(
        color: AppColors.canvas,
        borderRadius: const BorderRadius.vertical(top: Radius.circular(12)),
        border: Border.all(color: colors.outlineVariant, width: 0.7),
      ),
      child: SafeArea(
        top: false,
        child: ConstrainedBox(
          constraints: BoxConstraints(
            maxHeight: MediaQuery.sizeOf(context).height * 0.88,
          ),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              Flexible(
                child: ListView(
                  shrinkWrap: true,
                  padding: const EdgeInsets.fromLTRB(
                    AppSpacing.lg,
                    AppSpacing.lg,
                    AppSpacing.lg,
                    AppSpacing.md,
                  ),
                  children: [
                    Text(
                      l10n.planSheetTitle,
                      style: theme.textTheme.titleLarge,
                    ),
                    const SizedBox(height: AppSpacing.md),
                    _TaskCreatePropertyAction(
                      key: const ValueKey('plan-start-row'),
                      icon: LucideIcons.calendarClock300,
                      property: l10n.plannedStartLabel,
                      value: startLabel,
                      tooltip: l10n.setPlannedStartButton,
                      onTap: _pickStart,
                    ),
                    const SizedBox(height: AppSpacing.lg),
                    Text(
                      l10n.estimateLabel,
                      style: theme.textTheme.labelLarge?.copyWith(
                        color: colors.onSurfaceVariant,
                      ),
                    ),
                    const SizedBox(height: AppSpacing.sm),
                    Row(
                      children: [
                        IconButton.outlined(
                          key: const ValueKey('plan-estimate-decrease'),
                          tooltip: l10n.decreaseEstimateTooltip,
                          onPressed: _estimatedMinutes == null
                              ? null
                              : () => _adjustEstimate(-5),
                          icon: const Icon(LucideIcons.minus300),
                        ),
                        Expanded(
                          child: Semantics(
                            liveRegion: true,
                            child: Text(
                              _estimatedMinutes == null
                                  ? l10n.estimateNotSet
                                  : l10n.estimateMinutes(_estimatedMinutes!),
                              textAlign: TextAlign.center,
                              style: theme.textTheme.titleMedium,
                            ),
                          ),
                        ),
                        IconButton.outlined(
                          key: const ValueKey('plan-estimate-increase'),
                          tooltip: l10n.increaseEstimateTooltip,
                          onPressed: () => _adjustEstimate(5),
                          icon: const Icon(LucideIcons.plus300),
                        ),
                      ],
                    ),
                    const SizedBox(height: AppSpacing.md),
                    Wrap(
                      spacing: AppSpacing.sm,
                      runSpacing: AppSpacing.sm,
                      children: [
                        for (final minutes in const [25, 45, 60])
                          OutlinedButton(
                            key: ValueKey('plan-estimate-preset-$minutes'),
                            onPressed: () =>
                                setState(() => _estimatedMinutes = minutes),
                            style: OutlinedButton.styleFrom(
                              minimumSize: const Size(72, 48),
                              shape: RoundedRectangleBorder(
                                borderRadius: BorderRadius.circular(8),
                              ),
                            ),
                            child: Text(l10n.estimateMinutes(minutes)),
                          ),
                      ],
                    ),
                  ],
                ),
              ),
              DecoratedBox(
                decoration: BoxDecoration(
                  color: AppColors.canvas,
                  border: Border(
                    top: BorderSide(color: colors.outlineVariant, width: 0.7),
                  ),
                ),
                child: Padding(
                  padding: const EdgeInsets.fromLTRB(
                    AppSpacing.lg,
                    AppSpacing.sm,
                    AppSpacing.lg,
                    AppSpacing.sm,
                  ),
                  child: Wrap(
                    alignment: WrapAlignment.end,
                    spacing: AppSpacing.sm,
                    runSpacing: AppSpacing.xs,
                    children: [
                      TextButton(
                        key: const ValueKey('plan-clear'),
                        onPressed: () =>
                            Navigator.of(context).pop(const TaskPlanValue()),
                        child: Text(l10n.clearPlanButton),
                      ),
                      FilledButton(
                        key: const ValueKey('plan-apply'),
                        onPressed: () => Navigator.of(context).pop(
                          TaskPlanValue(
                            scheduledAt: _scheduledAt,
                            estimatedMinutes: _estimatedMinutes,
                          ),
                        ),
                        child: Text(l10n.planSaveButton),
                      ),
                    ],
                  ),
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

Future<TaskPrioritySelection?> showTaskPrioritySheet({
  required BuildContext context,
  required int selectedPriority,
}) {
  return showModalBottomSheet<TaskPrioritySelection>(
    context: context,
    useRootNavigator: true,
    backgroundColor: Colors.transparent,
    barrierColor: Theme.of(context).colorScheme.scrim.withValues(alpha: 0.24),
    builder: (context) =>
        _TaskPrioritySheet(selectedPriority: selectedPriority),
  );
}

class _TaskPrioritySheet extends StatelessWidget {
  const _TaskPrioritySheet({required this.selectedPriority});

  final int selectedPriority;

  @override
  Widget build(BuildContext context) {
    final l10n = AppLocalizations.of(context)!;
    final theme = Theme.of(context);
    final colors = theme.colorScheme;
    return DecoratedBox(
      decoration: BoxDecoration(
        color: AppColors.canvas,
        borderRadius: const BorderRadius.vertical(top: Radius.circular(12)),
        border: Border.all(color: colors.outlineVariant, width: 0.7),
      ),
      child: SafeArea(
        top: false,
        child: ListView(
          shrinkWrap: true,
          padding: const EdgeInsets.fromLTRB(
            AppSpacing.lg,
            AppSpacing.lg,
            AppSpacing.lg,
            AppSpacing.sm,
          ),
          children: [
            Text(l10n.prioritySheetTitle, style: theme.textTheme.titleLarge),
            const SizedBox(height: AppSpacing.sm),
            for (final priority in const [0, 1, 2, 3])
              Builder(
                builder: (context) {
                  final label = taskPriorityLabel(l10n, priority);
                  final selected = priority == selectedPriority;
                  return Semantics(
                    button: true,
                    selected: selected,
                    label: selected
                        ? l10n.selectedOptionSemantics(label)
                        : label,
                    excludeSemantics: true,
                    child: InkWell(
                      key: ValueKey('priority-option-$priority'),
                      onTap: () => Navigator.of(
                        context,
                      ).pop(TaskPrioritySelection(priority)),
                      child: ConstrainedBox(
                        constraints: const BoxConstraints(minHeight: 52),
                        child: Row(
                          children: [
                            SizedBox(
                              width: 28,
                              child: priority == 0
                                  ? const SizedBox.shrink()
                                  : PriorityDot(
                                      priority: priority,
                                      isMuted: false,
                                    ),
                            ),
                            Expanded(child: Text(label)),
                            if (selected)
                              Icon(
                                LucideIcons.check300,
                                size: 18,
                                color: colors.primary,
                              ),
                          ],
                        ),
                      ),
                    ),
                  );
                },
              ),
          ],
        ),
      ),
    );
  }
}

/// Builds the compact metadata labels shown below a task title.
///
/// Row/subtask-row usage (the default) intentionally omits status and
/// priority labels: status is conveyed by the checkbox/strikethrough, and
/// priority by the metadata dot. Pass [includeStatus] where status text is
/// explicitly required.
